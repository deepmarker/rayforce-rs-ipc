//! A blocking client connection to a Rayforce server.
//!
//! Blocking is deliberate: the callers this was written for send from a polling loop that owns
//! its own thread, and a batch is one write of a buffer they have already built. Nothing here
//! touches an async runtime, and the connection has no state of its own beyond the socket —
//! which is what makes it safe to use from whichever thread happens to be running, unlike an
//! embedded VM.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use crate::value::Value;
use crate::wire::{self, MsgType};
use crate::{Error, Result};

/// How long a connect and its handshake may take, and how to authenticate.
///
/// [`ConnectOptions::default`] allows 5 seconds and no credentials.
#[derive(Debug, Clone)]
pub struct ConnectOptions {
    /// The user name, sent only if the server asks for credentials.
    pub user: Option<String>,
    /// The password, sent only if the server asks for credentials.
    pub password: Option<String>,
    /// Bounds the connect and the handshake together; it does not bound later sends.
    pub timeout: Duration,
}

impl Default for ConnectOptions {
    fn default() -> Self {
        Self {
            user: None,
            password: None,
            timeout: Duration::from_secs(5),
        }
    }
}

impl ConnectOptions {
    /// Options with these credentials and the default timeout.
    #[must_use]
    pub fn with_credentials(user: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            user: Some(user.into()),
            password: Some(password.into()),
            ..Self::default()
        }
    }
}

/// A connection to a Rayforce server.
#[derive(Debug)]
pub struct Conn {
    stream: TcpStream,
    closed: bool,
}

impl Conn {
    /// Connect to `host:port` with the default options.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if nothing is listening, [`Error::WireVersion`] if the server speaks
    /// another wire version, and [`Error::Auth`] if it wants credentials this call does not have.
    pub fn connect(host: &str, port: u16) -> Result<Self> {
        Self::connect_with(host, port, &ConnectOptions::default())
    }

    /// Connect to `host:port`, handshake, and authenticate if asked.
    ///
    /// # Errors
    ///
    /// As [`Conn::connect`].
    pub fn connect_with(host: &str, port: u16, options: &ConnectOptions) -> Result<Self> {
        let address = (host, port)
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| Error::protocol(format!("{host}:{port} does not resolve")))?;
        let stream = TcpStream::connect_timeout(&address, options.timeout)?;
        // A batch is one write; Nagle would sit on it.
        stream.set_nodelay(true)?;
        // The deadline covers the handshake only: a slow server must not fail a later send.
        stream.set_read_timeout(Some(options.timeout))?;
        stream.set_write_timeout(Some(options.timeout))?;
        let mut conn = Self {
            stream,
            closed: false,
        };
        match conn.handshake(options) {
            Ok(()) => {
                conn.stream.set_read_timeout(None)?;
                conn.stream.set_write_timeout(None)?;
                Ok(conn)
            }
            Err(error) => {
                conn.close();
                Err(error)
            }
        }
    }

    /// The version handshake, then the credentials frame if the server asks for one.
    fn handshake(&mut self, options: &ConnectOptions) -> Result<()> {
        self.stream.write_all(&[wire::WIRE_VERSION, 0])?;
        let mut answer = [0_u8; 2];
        self.stream.read_exact(&mut answer)?;
        let (version, auth) = (answer[0], answer[1]);
        if version != wire::WIRE_VERSION {
            return Err(Error::WireVersion {
                client: wire::WIRE_VERSION,
                server: version,
            });
        }
        match auth {
            0 => Ok(()),
            1 => {
                let Some(password) = options.password.as_deref() else {
                    return Err(Error::Auth(
                        "server requires credentials and none were given".into(),
                    ));
                };
                let user = options.user.as_deref().unwrap_or_default();
                let mut credentials = Vec::with_capacity(user.len() + password.len() + 2);
                credentials.extend_from_slice(user.as_bytes());
                credentials.push(b':');
                credentials.extend_from_slice(password.as_bytes());
                credentials.push(0);
                if credentials.len() > 255 {
                    return Err(Error::protocol(format!(
                        "credentials are {} bytes; the wire allows 255",
                        credentials.len()
                    )));
                }
                let mut frame = Vec::with_capacity(credentials.len() + 1);
                frame.push(u8::try_from(credentials.len()).expect("capped at 255"));
                frame.extend_from_slice(&credentials);
                self.stream.write_all(&frame)?;
                let mut verdict = [1_u8; 1];
                self.stream.read_exact(&mut verdict)?;
                if verdict[0] != 0 {
                    return Err(Error::Auth("server rejected the credentials".into()));
                }
                Ok(())
            }
            other => Err(Error::protocol(format!(
                "server answered the handshake with an auth byte of {other}"
            ))),
        }
    }

    /// Whether [`Conn::close`] has run.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// Close the connection. Closing twice is harmless.
    pub fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }

    /// Fire and forget: the server evaluates the value through its `.ipc.on.async` hook and
    /// sends nothing back.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Closed`] after [`Conn::close`], [`Error::Protocol`] if the value cannot
    /// be encoded or is over the frame limit, and [`Error::Io`] if the write fails.
    pub fn send_async(&mut self, value: &Value) -> Result<()> {
        self.send(MsgType::Async, value)
    }

    /// Send a value and wait for the response frame.
    ///
    /// Frames the server pushes while we wait are decoded and passed to `on_push` rather than
    /// mistaken for the answer.
    ///
    /// # Errors
    ///
    /// As [`Conn::send_async`], plus [`Error::Protocol`] if the reply cannot be decoded.
    pub fn send_sync(&mut self, value: &Value) -> Result<Value> {
        self.send_sync_with(value, |_| {})
    }

    /// [`Conn::send_sync`], with a handler for the frames the server pushes in the meantime.
    ///
    /// # Errors
    ///
    /// As [`Conn::send_sync`].
    pub fn send_sync_with(
        &mut self,
        value: &Value,
        mut on_push: impl FnMut(Value),
    ) -> Result<Value> {
        self.send(MsgType::Sync, value)?;
        loop {
            match self.read_frame()? {
                (MsgType::Response, value) => return Ok(value),
                (MsgType::Async | MsgType::Sync, value) => on_push(value),
            }
        }
    }

    fn send(&mut self, msg_type: MsgType, value: &Value) -> Result<()> {
        if self.closed {
            return Err(Error::Closed);
        }
        let mut frame = Vec::with_capacity(wire::frame_size(value));
        wire::fill_frame(&mut frame, msg_type, value)?;
        self.stream.write_all(&frame)?;
        Ok(())
    }

    fn read_frame(&mut self) -> Result<(MsgType, Value)> {
        let mut header = [0_u8; wire::HEADER_LEN];
        self.stream.read_exact(&mut header).map_err(|error| {
            if error.kind() == std::io::ErrorKind::UnexpectedEof {
                Error::protocol("connection closed with a frame half read")
            } else {
                Error::Io(error)
            }
        })?;
        let mut at = 0;
        let header = wire::consume_header(&header, &mut at)?;
        let mut payload = vec![0_u8; header.payload_len];
        self.stream.read_exact(&mut payload)?;
        let mut at = 0;
        let value = wire::consume_payload(&payload, &mut at, &header)?;
        Ok((header.msg_type, value))
    }
}

impl Drop for Conn {
    fn drop(&mut self) {
        self.close();
    }
}
