use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration};
use bytes::Bytes;
use tokio::time::Instant;
use tokio::sync::{broadcast, Notify};
use tokio::time;
use log::debug;
use tokio_stream::StreamExt;

use crate::Result;

/// A wrapper around a `Db` instance. This exists to allow orderly cleanup
/// of the `Db` by signalling the background purge task to shut down when
/// this struct is dropped.
#[derive(Debug)]
pub(crate) struct DbDropGuard {
    db: Db,
}

#[derive(Debug, Clone)]
pub(crate) struct Db {
    shared: Arc<Shared>,
}

#[derive(Debug)]
struct Shared {
    state: Mutex<State>,

    background_task: Notify,
}

#[derive(Debug)]
struct State {
    entries: HashMap<String, Entry>,

    pub_sub: HashMap<String, broadcast::Sender<Bytes>>,

    expirations: BTreeMap<(Instant, u64), String>,

    next_id: u64,

    shutdown: bool,
}

#[derive(Debug)]
struct Entry {
    id: u64,

    data: Bytes,

    expired_at: Option<Instant>
}

impl DbDropGuard {
    pub(crate) fn new() ->  DbDropGuard {
        DbDropGuard {
            db : Db::new()
        }
    }

    pub(crate) fn db(&self) -> Db {
        self.db.clone()
    }
}

impl Drop for DbDropGuard {
    fn drop(&mut self) {
        // Signal the 'Db' instance to shut down the task that purges expired keys
        self.db.shutdown_purge_task();
    }
}

impl Db {
    pub(crate) fn new() -> Db {
        let shared = Arc::new(Shared{
            state: Mutex::new(State{
                entries: HashMap::new(),
                pub_sub: HashMap::new(),
                expirations: BTreeMap::new(),
                next_id: 0,
                shutdown: false
            }),
            background_task: Notify::new(),
        });

        tokio::spawn(purge_expired_tasks(shared.clone()));

        Db { shared }
    }

    pub(crate) fn get(&self, key: &str) -> Option<Bytes> {
        let state = self.shared.state.lock().unwrap();
        state.entries.get(key).map(|entry| entry.data.clone())
    }

    pub(crate) fn set(&self, key: String, value: Bytes, expire: Option<Duration>) {
        let mut state = self.shared.state.lock().unwrap();

        let id = state.next_id;
        state.next_id += 1;

        let mut notify = false;

        let expired_at = expire.map(|duration| {
            // `Instant` at which the key expires.
            let when = Instant::now() + duration;

            // Only notify the worker task if the newly inserted expiration is the
            // **next** key to evict. In this case, the worker needs to be woken up
            // to update its state.
            notify = state
                .next_expiration()
                .map(|expiration| expiration > when)
                .unwrap_or(true);

            // Track the expiration.
            state.expirations.insert((when, id), key.clone());
            when
        });

        let prev = state.entries.insert(
            key,
            Entry {
                id,
                data: value,
                expired_at,
            },
        );

        if let Some(prev) = prev {
            if let Some(when) = prev.expired_at {
                // clear expiration
                state.expirations.remove(&(when, prev.id));
            }
        }

        drop(state);

        if notify {
            // Finally, only notify the background task if it needs to update
            // its state to reflect a new expiration.
            self.shared.background_task.notify_one();
        }
    }

    /// Signals the purge background task to shut down. This is called by the
    /// `DbShutdown`s `Drop` implementation.
    fn shutdown_purge_task(&self) {
        // The background task must be signaled to shut down. This is done by
        // setting `State::shutdown` to `true` and signalling the task.
        let mut state = self.shared.state.lock().unwrap();
        state.shutdown = true;

        // Drop the lock before signalling the background task. This helps
        // reduce lock contention by ensuring the background task doesn't
        // wake up only to be unable to acquire the mutex.
        drop(state);
        self.shared.background_task.notify_one();
    }
}

impl State {
    fn next_expiration(&self) -> Option<Instant> {
        self.expirations
            .keys()
            .next()
            .map(|expiration| expiration.0)
    }
}


async fn purge_expired_tasks(shared: Arc<Shared>) {
    // If the shutdown flag is set, then the task should exit.
    while !shared.is_shutdown() {
        // Purge all keys that are expired. The function returns the instant at
        // which the **next** key will expire. The worker should wait until the
        // instant has passed then purge again.
        if let Some(when) = shared.purge_expired_keys() {
            // Wait until the next key expires **or** until the background task
            // is notified. If the task is notified, then it must reload its
            // state as new keys have been set to expire early. This is done by
            // looping.
            tokio::select! {
                _ = time::sleep_until(when) => {}
                _ = shared.background_task.notified() => {}
            }
        } else {
            // There are no keys expiring in the future. Wait until the task is
            // notified.
            shared.background_task.notified().await;
        }
    }

    debug!("Purge background task shut down")
}

impl Shared {
    fn purge_expired_keys(&self) -> Option<Instant> {
        let mut state = self.state.lock().unwrap();

        if state.shutdown {
            return None;
        }

        let state = &mut *state;

        let now = Instant::now();

        while let Some((&(when, id), key)) = state.expirations.iter().next() {
            if when > now {
                // 没有 key 需要被淘汰
                return Some(when);
            }

            state.entries.remove(key);
            state.expirations.remove(&(when, id));
        };

        None
    }

    fn is_shutdown(&self) -> bool {
        self.state.lock().unwrap().shutdown
    }
}