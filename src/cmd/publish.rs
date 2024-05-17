use bytes::Bytes;

#[derive(Debug)]
pub struct Publish {
    /// Name of the channel on which the message should be published.
    channel: String,

    /// The message to publish.
    message: Bytes,
}