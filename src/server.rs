use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc, Semaphore};
use tokio::time;
use tracing::{debug, info, error, instrument};
use crate::{Command, Connection, Frame};
use crate::db::{Db, DbDropGuard};
use crate::shutdown::Shutdown;

#[derive(Debug)]
struct Listener {
    db_holder: DbDropGuard,

    listener: TcpListener,

    limit_connections: Arc<Semaphore>,

    notify_shutdown: broadcast::Sender<()>,

    shutdown_complete_rx: mpsc::Receiver<()>,
    shutdown_complete_tx: mpsc::Sender<()>, 
}

struct Handler {
    db: Db,

    connection: Connection,

    limit_connections: Arc<Semaphore>,

    shutdown: Shutdown,

    _shutdown_complete: mpsc::Sender<()>,
}

const MAX_CONNECTIONS: usize = 256;

pub async fn run(listener: TcpListener, shutdown: impl Future) {
    let (notify_shutdown, _) = broadcast::channel(1);

    let (shutdown_complete_tx, shutdown_complete_rx) = mpsc::channel(1);

    let mut server = Listener {
        listener,
        db_holder: DbDropGuard::new(),
        limit_connections: Arc::new(Semaphore::new(MAX_CONNECTIONS)),
        notify_shutdown,
        shutdown_complete_tx,
        shutdown_complete_rx,
    };

    tokio::select! {
        res = server.run() => {
            if let Err(err) = res {
                error!(cause = %err, "failed to accept");
            }
        }
        _ = shutdown => {
            info!("shutting down");
        }
    }

    let Listener {
        mut shutdown_complete_rx,
        shutdown_complete_tx,
        notify_shutdown,
        ..
    } = server;

    drop(notify_shutdown);
    drop(shutdown_complete_tx);
    let _ = shutdown_complete_rx.recv().await;
}

impl Listener {
    async fn run(&mut self) -> crate::Result<()> {
        info!("accepting inbound connections");

        loop {
            // Semaphore不会关闭，所以使用unwrap()是安全的。
            self.limit_connections.acquire().await.unwrap().forget();

            let socket = self.accept().await?;

            let mut handler = Handler {
                db: self.db_holder.db(),

                connection: Connection::new(socket),

                limit_connections: self.limit_connections.clone(),

                shutdown: Shutdown::new(self.notify_shutdown.subscribe()),

                _shutdown_complete: self.shutdown_complete_tx.clone(),
            };

            tokio::spawn(async move {
                if let Err(err) = handler.run().await {
                    error!(cause = ?err, "connection error");
                }
            });
        }
    }

    async fn accept(&mut self) -> crate::Result<TcpStream> {
        let mut backoff = 1;

        loop {
            match self.listener.accept().await {
                Ok((socket, _)) =>  return Ok(socket),
                Err(err) => {
                    if backoff > 32 {
                        return Err(err.into());
                    }
                },
            }

            time::sleep(Duration::from_secs(backoff)).await;

            backoff *= 2;
        }
    }
}


impl Handler {
    #[instrument(skip(self))]
    async fn run(&mut self) -> crate::Result<()> {
        while !self.shutdown.is_shutdown() {
            let maybe_frame = tokio::select! {
                res = self.connection.read_frame() => res?,
                _ = self.shutdown.recv() => {
                    return Ok(());
                }
            };

            let frame = match maybe_frame {
                None => return Ok(()),
                Some(frame) => frame,
            };

            let cmd = Command::from_frame(frame)?;
            debug!(?cmd);

            cmd.apply(&self.db, &mut self.connection, &mut self.shutdown).await?;
        }

        Ok(())
    }
}

impl Drop for Handler {
    fn drop(&mut self) {
        // 将一个许可返回到Semaphore中。
        // 这么做如果达到最大连接数，将解除监听器的阻塞状态。
        // 这通过Drop trait的实现来完成，目的是为了确保即使处理连接的任务发生恐慌（panic），许可也能被正确归还。
        // 如果add_permit方法被放在run函数的末尾，并且由于某些错误导致任务发生恐慌，那么许可将无法归还给Semaphore。
        // 通过在Drop实现中执行此操作，可以保证许可无论如何都会被归还。
        self.limit_connections.add_permits(1);
    }
}