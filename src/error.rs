//! The one error type.
//!
//! Every malformed-frame path in this library goes through [`Error::Protocol`], including the
//! ones that mean "this value cannot be encoded" — the OCaml original raises a single
//! `Protocol_error` for all of them, and keeping one variant keeps the messages in one place.

use std::fmt;
use std::io;

/// What went wrong.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The peer sent something this client cannot parse, or a value cannot be encoded.
    Protocol(String),
    /// A socket-level failure: connect, read, write, or a closed peer.
    Io(io::Error),
    /// A send on a connection that was already closed.
    Closed,
    /// Credentials were required, were missing, or were rejected.
    Auth(String),
    /// The peer speaks a different wire version than this client.
    WireVersion {
        /// What this client speaks.
        client: u8,
        /// What the peer answered.
        server: u8,
    },
}

impl Error {
    /// Build a [`Error::Protocol`] from anything printable.
    pub(crate) fn protocol(message: impl Into<String>) -> Self {
        Self::Protocol(message.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Protocol(message) => f.write_str(message),
            Self::Io(error) => write!(f, "{error}"),
            Self::Closed => f.write_str("the connection is closed"),
            Self::Auth(message) => f.write_str(message),
            Self::WireVersion { client, server } => write!(
                f,
                "peer speaks wire version {server}, this client speaks {client}"
            ),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// The result type every fallible call in this crate returns.
pub type Result<T> = std::result::Result<T, Error>;
