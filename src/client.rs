use std::env::var;
use std::io::{Error, ErrorKind};
use std::time::Duration;
use bytes::Bytes;
use tokio::net::{TcpSocket, TcpStream, ToSocketAddrs};
use tracing::debug;
use crate::cmd::{Get, Set};
use crate::{Connection, Frame};

/// Established connection with a Redis server.
///
/// Backed by a single `TcpStream`, `Client` provides basic network client
/// functionality (no pooling, retrying, ...). Connections are established using
/// the [`connect`](fn@connect) function.
///
/// Requests are issued using the various methods of `Client`.
pub struct Client {
    /// The TCP connection decorated with the redis protocol encoder / decoder
    /// implemented using a buffered `TcpStream`.
    ///
    /// When `Listener` receives an inbound connection, the `TcpStream` is
    /// passed to `Connection::new`, which initializes the associated buffers.
    /// `Connection` allows the handler to operate at the "frame" level and keep
    /// the byte level protocol parsing details encapsulated in `Connection`.
    connection: Connection,
}

/// A client that has entered pub/sub mode.
///
/// Once clients subscribe to a channel, they may only perform pub/sub related
/// commands. The `Client` type is transitioned to a `Subscriber` type in order
/// to prevent non-pub/sub methods from being called.
pub struct Subscriber {
    /// The subscribed client.
    client: Client,

    /// The set of channels to which the `Subscriber` is currently subscribed.
    subscribed_channels: Vec<String>,
}

/// A message received on a subscribed channel.
#[derive(Debug, Clone)]
pub struct Message {
    pub channel: String,
    pub content: Bytes,
}

pub async fn connect<T: ToSocketAddrs>(addr: T) -> crate::Result<Client> {
    let socket = TcpStream::connect(addr).await?;

    let connection = Connection::new(socket);
    Ok(Client { connection })
}

impl Client {
    pub async fn get(&mut self, key: &str) -> crate::Result<Option<Bytes>> {
        let frame = Get::new(key).into_frame();

        self.connection.write_frame(&frame).await?;

        match self.read_response().await? {
            Frame::Simple(val) => Ok(Some(val.into())),
            Frame::Bulk(val) => Ok(Some(val)),
            Frame::Null => Ok(None),
            frame => Err(frame.to_error())
        }
    }

    pub async fn set(&mut self, key: &str, value: Bytes, expire: Option<Duration>) -> crate::Result<()> {
        let frame = Set::new(key, value, expire).into_frame();

        debug!(request = ?frame);

        self.connection.write_frame(&frame).await?;
        
        match self.read_response().await? {
            Frame::Simple(resp) if resp == "OK" => Ok(()),
            frame => Err(frame.to_error())
        }
    }

    async fn read_response(&mut self) -> crate::Result<Frame> {
        let response = self.connection.read_frame().await?;

        debug!(?response);

        match response {
            // Error frames are converted to `Err`
            Some(Frame::Error(msg)) => Err(msg.into()),
            Some(frame) => Ok(frame),
            None => {
                // Receiving `None` here indicates the server has closed the
                // connection without sending a frame. This is unexpected and is
                // represented as a "connection reset by peer" error.
                let err = Error::new(ErrorKind::ConnectionReset, "connection reset by server");

                Err(err.into())
            }
        }
    }
}