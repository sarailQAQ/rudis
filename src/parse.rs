use std::{fmt, str, vec};
use std::str::Utf8Error;
use bytes::Bytes;
use tracing::{debug, error};
use crate::Frame;

/// Utility for parsing a command
///
/// Commands are represented as array frames. Each entry in the frame is a
/// "token". A `Parse` is initialized with the array frame and provides a
/// cursor-like API. Each command struct includes a `parse_frame` method that
/// uses a `Parse` to extract its fields.
#[derive(Debug)]
pub(crate) struct Parse {
    /// Array frame iterator.
    parts: vec::IntoIter<Frame>,
}

#[derive(Debug)]
pub(crate) enum ParseError {
    /// Attempting to extract a value failed due to the frame being fully
    /// consumed.
    EndOfStream,

    /// All other errors
    Other(crate::Error),
}

impl Parse {
    pub(crate) fn new(frame: Frame) -> Result<Parse, ParseError> {
        let array = match frame {
            Frame::Array(array) => array,
            frame=> return Err(format!("protocol error; expected array, got {:?}", frame).into())
        };



        Ok(
            Parse{ parts: array.into_iter() }
        )
    }

    fn next(&mut self) -> Result<Frame, ParseError> {
        self
            .parts
            .next()
            .ok_or(ParseError::EndOfStream)
    }

    pub(crate) fn next_string(&mut self) -> Result<String, ParseError> {
        let frame = self.next()?;
        match frame {
            Frame::Simple(s) => Ok(s),
            Frame::Bulk(data) => str::from_utf8(&data[..])
                .map(|s| s.to_string())
                .map_err(|_| "protocol error; invalid string".into()),
            frame=> Err(format!(
                "protocol error; expected simple frame or bulk frame, got {:?}",
                frame
            ).into())
        }
    }

    pub(crate) fn next_bytes(&mut self) -> Result<Bytes, ParseError> {
        match self.next()? {
            // Both `Simple` and `Bulk` representation may be raw bytes.
            //
            // Although errors are stored as strings and could be represented as
            // raw bytes, they are considered separate types.
            Frame::Simple(s) => Ok(Bytes::from(s.into_bytes())),
            Frame::Bulk(data) => Ok(data),
            frame => Err(format!(
                "protocol error; expected simple frame or bulk frame, got {:?}",
                frame
            ).into()),
        }
    }

    pub(crate) fn next_int(&mut self) -> Result<u64, ParseError> {
        use atoi::atoi;

        const MSG: &str = "protocol error; invalid number";

        match self.next()? {
            // An integer frame type is already stored as an integer.
            Frame::Integer(v) => Ok(v),
            // Simple and bulk frames must be parsed as integers. If the parsing
            // fails, an error is returned.
            Frame::Simple(data) => atoi::<u64>(data.as_bytes()).ok_or_else(|| MSG.into()),
            Frame::Bulk(data) => atoi::<u64>(&data).ok_or_else(|| MSG.into()),
            frame => Err(format!("protocol error; expected int frame but got {:?}", frame).into()),
        }
    }

    /// Ensure there are no more entries in the array
    pub(crate) fn finish(&mut self) -> Result<(), ParseError> {
        if self.parts.next().is_none() {
            Ok(())
        } else {
            Err("protocol error; expected end of frame, but there was more".into())
        }
    }
}

impl From<String> for ParseError {
    fn from(src: String) -> ParseError {
        ParseError::Other(src.into())
    }
}

impl From<&str> for ParseError {
    fn from(src: &str) -> ParseError {
        src.to_string().into()
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::EndOfStream => "protocol error; unexpected end of stream".fmt(f),
            ParseError::Other(err) => err.fmt(f),
        }
    }
}

impl std::error::Error for ParseError {}