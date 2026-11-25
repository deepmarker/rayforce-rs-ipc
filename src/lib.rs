//! `RayforceDB`'s IPC protocol in pure Rust.
//!
//! Written against the wire format rather than against `librayforce`. No C stubs, no embedded
//! VM, no symbol table shared with a server, no dependencies, and nothing in the process that
//! cares which thread it runs on.
//!
//! ```no_run
//! use rayforce_ipc::{Conn, Sym, Value};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let mut conn = Conn::connect("127.0.0.1", 5010)?;
//! let batch = Value::table([
//!     ("instrument", Value::Syms(vec![Sym::intern("BTCUSDT"), Sym::intern("ETHUSDT")])),
//!     ("px", Value::F64s(vec![83_592.01, 2_543.56])),
//! ])?;
//! conn.send_async(&Value::List(vec![
//!     Value::Sym(Sym::intern("upd")),
//!     Value::Sym(Sym::intern("depth")),
//!     batch,
//! ]))?;
//! # Ok(())
//! # }
//! ```
//!
//! # What is here
//!
//! | Module | What it is |
//! |---|---|
//! | [`sym`] | Local intern table. A symbol travels as a string, so an id is ours alone — it exists so a column holds one int per row. |
//! | [`value`] | The value model. A column is an ordinary `Vec`, written straight into the frame. |
//! | [`wire`] | The 16-byte frame header, and the delta+RLE decoder for a compressed frame. |
//! | [`conn`] | A blocking client: connect, handshake, [`Conn::send_async`], [`Conn::send_sync`]. |
//! | [`temporal`] | Checked conversions between Unix time and Rayforce's 2000-01-01 epoch. |
//! | [`column`] | A column built a row at a time, already in wire layout. |
//!
//! # The format
//!
//! Wire version 4, as of rayforce v3.0.2. Version 4 adds the error message to an error record
//! (code, one length byte, message); a peer on version 3 is refused at the handshake, as the
//! server itself refuses it. The authority is `src/store/serde.{h,c}` and `src/core/ipc.{h,c}`
//! in the rayforce tree.
//!
//! - **Handshake** — the client sends `{4, 0}`; the server answers `{version, auth_required}`,
//!   and if it asked for credentials, one length byte plus `user:password\0` and a one-byte
//!   verdict.
//! - **Frame** — `prefix 0xcefadefa`, version, flags, endian, message type, and an `i64` payload
//!   length. The payload is native-endian, and a peer whose byte order differs refuses the frame
//!   rather than reading columns backwards.
//! - **Values** — a vector is `type, attrs, i64 length, payload`; an atom is `type, flags, value`
//!   with the type negated. A table is a symbol vector of column names followed by a list of
//!   columns.
//! - **Symbols** are NUL-terminated strings on the wire, so two processes need no coordination
//!   to agree on a name.
//! - **Compression** is a per-frame, sender-side decision with nothing to negotiate, so this
//!   client never compresses and always decompresses. Rayforce never compresses to a loopback or
//!   UNIX-domain peer anyway.
//!
//! A wire-version bump is a connect-time error here, not a silent misparse: the handshake
//! compares versions before anything is sent.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod column;
pub mod conn;
pub mod error;
pub mod sym;
pub mod tag;
pub mod temporal;
pub mod value;
pub mod wire;

pub use column::Column;
pub use conn::{Conn, ConnectOptions};
pub use error::{Error, Result};
pub use sym::Sym;
pub use temporal::{TemporalError, EPOCH_OFFSET_NANOS, EPOCH_OFFSET_SECONDS};
pub use value::{TableColumn, Value};
pub use wire::{Header, MsgType, WIRE_VERSION};
