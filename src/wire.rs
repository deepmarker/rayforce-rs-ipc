//! Frames: a 16-byte header and a payload.
//!
//! The header names the byte order it was written in and the receiver refuses anything else, so
//! the payload is native-endian — little on every machine this runs on.

use crate::value::Value;
use crate::{Error, Result};

/// The four bytes every frame starts with.
pub const PREFIX: u32 = 0xcefa_defa;

/// The wire format this client speaks: version 4, as of rayforce v3.0.2.
///
/// Version 4 adds the message to an error record (code, one length byte, message); a peer on
/// version 3 is refused at the handshake, as the server itself refuses it. The authority is
/// `src/store/serde.{h,c}` and `src/core/ipc.{h,c}` in the rayforce tree.
pub const WIRE_VERSION: u8 = 4;

/// Bytes of header before the payload.
pub const HEADER_LEN: usize = 16;

/// The largest payload either side will declare: 256 MiB.
pub const MAX_PAYLOAD: usize = 256 * 1024 * 1024;

/// What a frame is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MsgType {
    /// Fire and forget: the peer evaluates the value through its `.ipc.on.async` hook and sends
    /// nothing back.
    Async,
    /// The peer evaluates and answers with a [`MsgType::Response`].
    Sync,
    /// The answer to a [`MsgType::Sync`].
    Response,
}

impl MsgType {
    /// The byte this message type travels as.
    #[must_use]
    pub fn to_u8(self) -> u8 {
        match self {
            Self::Async => 0,
            Self::Sync => 1,
            Self::Response => 2,
        }
    }

    /// The message type this byte names.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Protocol`] for anything but 0, 1 or 2.
    pub fn from_u8(byte: u8) -> Result<Self> {
        match byte {
            0 => Ok(Self::Async),
            1 => Ok(Self::Sync),
            2 => Ok(Self::Response),
            other => Err(Error::protocol(format!(
                "message type {other} is not one of async, sync, response"
            ))),
        }
    }
}

/// A frame header, once parsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// What the frame is for.
    pub msg_type: MsgType,
    /// Whether the payload went through the delta-plus-RLE coding.
    pub compressed: bool,
    /// Payload bytes that follow the header.
    pub payload_len: usize,
}

/// Bytes a frame carrying `value` occupies, header included.
#[must_use]
pub fn frame_size(value: &Value) -> usize {
    HEADER_LEN + value.size()
}

/// Write a whole frame — header and payload — into `out`.
///
/// # Errors
///
/// Returns [`Error::Protocol`] if the payload is over [`MAX_PAYLOAD`] or the value cannot be
/// spelled.
pub fn fill_frame(out: &mut Vec<u8>, msg_type: MsgType, value: &Value) -> Result<()> {
    let payload_len = value.size();
    if payload_len > MAX_PAYLOAD {
        return Err(Error::protocol(format!(
            "payload of {payload_len} bytes is over the {MAX_PAYLOAD}-byte frame limit"
        )));
    }
    out.extend_from_slice(&PREFIX.to_le_bytes());
    out.push(WIRE_VERSION);
    // The flags byte: this client never compresses. Compression is a per-frame sender-side
    // decision with nothing to negotiate, so the only thing to do here is decode one.
    out.push(0);
    // 0 = little. A peer whose byte order differs rejects the frame rather than reading the
    // columns backwards, which is the whole reason the field is there; we only ever write ours.
    out.push(0);
    out.push(msg_type.to_u8());
    out.extend_from_slice(
        &i64::try_from(payload_len)
            .map_err(|_| Error::protocol("payload length does not fit in the header"))?
            .to_le_bytes(),
    );
    value.fill(out)
}

/// Parse a header from `buf`, advancing `at` past it.
///
/// # Errors
///
/// Returns [`Error::Protocol`] on a frame this peer cannot be speaking: a bad prefix, a
/// different wire version, the other byte order, an unknown message type, or an impossible
/// payload length.
pub fn consume_header(buf: &[u8], at: &mut usize) -> Result<Header> {
    let mut cursor = Cursor { buf, at: *at };
    let header = cursor.header();
    *at = cursor.at;
    header
}

impl Cursor<'_> {
    fn header(&mut self) -> Result<Header> {
        let prefix = self.u32()?;
        if prefix != PREFIX {
            return Err(Error::protocol(format!(
                "frame prefix {prefix:#x} is not rayforce's {PREFIX:#x}"
            )));
        }
        let version = self.u8()?;
        if version != WIRE_VERSION {
            return Err(Error::WireVersion {
                client: WIRE_VERSION,
                server: version,
            });
        }
        let flags = self.u8()?;
        let endian = self.u8()?;
        if endian != 0 {
            return Err(Error::protocol("frame is big-endian; this client is not"));
        }
        let msg_type = MsgType::from_u8(self.u8()?)?;
        let declared = self.i64()?;
        if declared <= 0 || declared > i64::try_from(MAX_PAYLOAD).expect("fits") {
            return Err(Error::protocol(format!(
                "frame declares a {declared}-byte payload"
            )));
        }
        Ok(Header {
            msg_type,
            compressed: flags & 0x01 != 0,
            payload_len: usize::try_from(declared).expect("positive"),
        })
    }
}

/// Undo the delta-plus-RLE coding a server may apply to a frame.
///
/// Exposed because it is the one piece of the format with no inverse here: this client never
/// compresses, which the format allows.
///
/// # Errors
///
/// Returns [`Error::Protocol`] on a coding that does not decode to exactly `uncompressed_len`
/// bytes.
pub fn decompress(src: &[u8], uncompressed_len: usize) -> Result<Vec<u8>> {
    let mut dst = Vec::with_capacity(uncompressed_len);
    let mut si = 0;
    // Undo the RLE, then the delta, in one pass: the delta only ever refers to the byte before
    // it, which is already final by the time we get here.
    while si < src.len() && dst.len() < uncompressed_len {
        let count = src[si] as i8;
        si += 1;
        if count > 0 {
            let count = usize::try_from(count).expect("positive");
            let Some(&value) = src.get(si) else {
                return Err(Error::protocol("compressed frame: run with no value byte"));
            };
            si += 1;
            if dst.len() + count > uncompressed_len {
                return Err(Error::protocol(
                    "compressed frame: run overruns the declared size",
                ));
            }
            for _ in 0..count {
                let previous = dst.last().copied().unwrap_or(0);
                dst.push(value.wrapping_add(previous));
            }
        } else {
            let len = usize::try_from(-i32::from(count)).expect("positive");
            if si + len > src.len() || dst.len() + len > uncompressed_len {
                return Err(Error::protocol(
                    "compressed frame: literal overruns the declared size",
                ));
            }
            for &value in &src[si..si + len] {
                let previous = dst.last().copied().unwrap_or(0);
                dst.push(value.wrapping_add(previous));
            }
            si += len;
        }
    }
    if dst.len() != uncompressed_len {
        return Err(Error::protocol(format!(
            "compressed frame: got {} bytes, expected {uncompressed_len}",
            dst.len()
        )));
    }
    Ok(dst)
}

/// Decode a payload of `header.payload_len` bytes, decompressing first if the header says so.
/// `at` is advanced past the payload.
///
/// # Errors
///
/// Returns [`Error::Protocol`] on a truncated frame, a bad compression header, or a malformed
/// value.
pub fn consume_payload(buf: &[u8], at: &mut usize, header: &Header) -> Result<Value> {
    if !header.compressed {
        return Value::consume(buf, at);
    }
    let mut cursor = Cursor { buf, at: *at };
    let uncompressed_len = cursor.u32()? as usize;
    if uncompressed_len == 0 || uncompressed_len > MAX_PAYLOAD {
        return Err(Error::protocol(format!(
            "compressed frame declares {uncompressed_len} uncompressed bytes"
        )));
    }
    let Some(body) = header.payload_len.checked_sub(4) else {
        return Err(Error::protocol(
            "compressed frame is shorter than its size prefix",
        ));
    };
    let src = cursor.take(body)?;
    let payload = decompress(src, uncompressed_len)?;
    *at = cursor.at;
    let mut inner = 0;
    Value::consume(&payload, &mut inner)
}

/// A cursor over one frame, kept private so the bounds checks live in one place.
struct Cursor<'a> {
    buf: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self
            .at
            .checked_add(len)
            .filter(|end| *end <= self.buf.len())
            .ok_or_else(|| {
                Error::protocol(format!(
                    "frame ends after {} bytes, another {len} were asked for",
                    self.buf.len().saturating_sub(self.at)
                ))
            })?;
        let slice = &self.buf[self.at..end];
        self.at = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().expect("4 bytes"),
        ))
    }

    fn i64(&mut self) -> Result<i64> {
        Ok(i64::from_le_bytes(
            self.take(8)?.try_into().expect("8 bytes"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_starts_with_the_prefix_the_version_and_the_message_type() {
        let value = Value::I64s(vec![1, 2, 3]);
        let mut buf = Vec::new();
        fill_frame(&mut buf, MsgType::Async, &value).unwrap();
        assert_eq!(buf.len(), frame_size(&value));
        assert_eq!(&buf[0..4], &[0xfa, 0xde, 0xfa, 0xce]);
        assert_eq!(buf[4], 4);
        assert_eq!(buf[5], 0, "uncompressed");
        assert_eq!(buf[6], 0, "little endian");
        assert_eq!(buf[7], 0, "async");
        assert_eq!(buf[8..16], (value.size() as i64).to_le_bytes());

        let mut at = 0;
        let header = consume_header(&buf, &mut at).unwrap();
        assert_eq!(header.msg_type, MsgType::Async);
        assert!(!header.compressed);
        assert_eq!(header.payload_len, value.size());
        assert_eq!(at, HEADER_LEN, "the header is 16 bytes");
        let payload = consume_payload(&buf, &mut at, &header).unwrap();
        assert_eq!(payload, value);
        assert_eq!(at, buf.len());
    }

    #[test]
    fn message_types_round_trip() {
        for msg_type in [MsgType::Async, MsgType::Sync, MsgType::Response] {
            assert_eq!(MsgType::from_u8(msg_type.to_u8()).unwrap(), msg_type);
        }
        assert_eq!(
            MsgType::from_u8(3).unwrap_err().to_string(),
            "message type 3 is not one of async, sync, response"
        );
    }

    /// The OCaml suite's rejected-header cases were written before the wire version moved to 4,
    /// so two of them stopped testing what they meant to. These carry version 4 and check the
    /// message as well as the rejection.
    fn header_bytes(version: u8, endian: u8, flags: u8, msg_type: u8, len: i64) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&PREFIX.to_le_bytes());
        buf.push(version);
        buf.push(flags);
        buf.push(endian);
        buf.push(msg_type);
        buf.extend_from_slice(&len.to_le_bytes());
        buf
    }

    #[test]
    fn a_wrong_prefix_is_refused() {
        let mut buf = header_bytes(4, 0, 0, 0, 8);
        buf[0] = 0;
        let mut at = 0;
        assert_eq!(
            consume_header(&buf, &mut at).unwrap_err().to_string(),
            "frame prefix 0xcefade00 is not rayforce's 0xcefadefa"
        );
    }

    #[test]
    fn a_wrong_version_is_refused_at_the_handshake_not_silently_parsed() {
        let buf = header_bytes(2, 0, 0, 0, 8);
        let mut at = 0;
        let error = consume_header(&buf, &mut at).unwrap_err();
        assert_eq!(
            error.to_string(),
            "peer speaks wire version 2, this client speaks 4"
        );
        assert!(matches!(
            error,
            Error::WireVersion {
                client: 4,
                server: 2
            }
        ));
    }

    #[test]
    fn a_big_endian_frame_is_refused() {
        let buf = header_bytes(4, 1, 0, 0, 8);
        let mut at = 0;
        assert_eq!(
            consume_header(&buf, &mut at).unwrap_err().to_string(),
            "frame is big-endian; this client is not"
        );
    }

    #[test]
    fn an_empty_payload_is_refused() {
        let buf = header_bytes(4, 0, 0, 1, 0);
        let mut at = 0;
        assert_eq!(
            consume_header(&buf, &mut at).unwrap_err().to_string(),
            "frame declares a 0-byte payload"
        );
    }

    #[test]
    fn an_oversized_payload_is_refused() {
        let buf = header_bytes(4, 0, 0, 1, i64::MAX);
        let mut at = 0;
        assert_eq!(
            consume_header(&buf, &mut at).unwrap_err().to_string(),
            "frame declares a 9223372036854775807-byte payload"
        );
    }

    /// The delta-plus-RLE coding, transcribed from the server's `ray_ipc_compress_at` — the one
    /// piece of the format this client decodes but never writes.
    fn compress(src: &[u8]) -> Vec<u8> {
        let mut delta = Vec::with_capacity(src.len());
        let mut previous = 0_u8;
        for &byte in src {
            delta.push(byte.wrapping_sub(previous));
            previous = byte;
        }
        let mut out = Vec::new();
        let mut i = 0;
        while i < delta.len() {
            let mut run = 1;
            while i + run < delta.len() && delta[i + run] == delta[i] && run < 127 {
                run += 1;
            }
            if run >= 2 {
                out.push(u8::try_from(run).expect("capped at 127"));
                out.push(delta[i]);
                i += run;
            } else {
                let mut len = 0;
                while i + len < delta.len()
                    && len < 128
                    && (i + len + 1 >= delta.len() || delta[i + len] != delta[i + len + 1])
                {
                    len += 1;
                }
                out.push((-i32::try_from(len).expect("capped at 128") as i8) as u8);
                out.extend_from_slice(&delta[i..i + len]);
                i += len;
            }
        }
        out
    }

    fn round_trip_compression(source: &[u8]) {
        let compressed = compress(source);
        let decoded = decompress(&compressed, source.len()).unwrap();
        assert_eq!(decoded, source, "compressed form was {compressed:02x?}");
    }

    #[test]
    fn a_run_of_300_bytes_decodes() {
        let mut source = b"runs: ".to_vec();
        source.extend(std::iter::repeat_n(b'x', 300));
        round_trip_compression(&source);
    }

    #[test]
    fn mixed_runs_and_literals_decode() {
        let mut source = vec![b'a'; 40];
        source.extend_from_slice(b"bcdefgh");
        source.extend(std::iter::repeat_n(0_u8, 200));
        round_trip_compression(&source);
    }

    #[test]
    fn a_constant_delta_decodes() {
        let source: Vec<u8> = (0..512)
            .map(|i| u8::try_from(i % 251).expect("fits"))
            .collect();
        round_trip_compression(&source);
    }

    #[test]
    fn a_single_byte_decodes() {
        round_trip_compression(b"a");
    }

    #[test]
    fn a_bad_coding_is_reported() {
        // A run whose value byte is missing.
        assert_eq!(
            decompress(&[2], 2).unwrap_err().to_string(),
            "compressed frame: run with no value byte"
        );
        // A run that claims more bytes than were declared.
        assert_eq!(
            decompress(&[5, 1], 2).unwrap_err().to_string(),
            "compressed frame: run overruns the declared size"
        );
        // A literal that runs past the source.
        assert_eq!(
            decompress(&[0xfe, 1], 2).unwrap_err().to_string(),
            "compressed frame: literal overruns the declared size"
        );
        // A coding that stops short of the declared size.
        assert_eq!(
            decompress(&[1, 7], 2).unwrap_err().to_string(),
            "compressed frame: got 1 bytes, expected 2"
        );
    }

    #[test]
    fn a_compressed_frame_is_decompressed_before_decoding() {
        // A payload that was compressed as a whole, size prefix included.
        let inner = {
            let mut buf = Vec::new();
            Value::I64s(vec![7, 8, 9]).fill(&mut buf).unwrap();
            buf
        };
        let body = compress(&inner);
        let mut payload = Vec::new();
        payload.extend_from_slice(&u32::try_from(inner.len()).expect("fits").to_le_bytes());
        payload.extend_from_slice(&body);

        let mut frame = header_bytes(4, 0, 0x01, 2, i64::try_from(payload.len()).expect("fits"));
        frame.extend_from_slice(&payload);

        let mut at = 0;
        let header = consume_header(&frame, &mut at).unwrap();
        assert!(header.compressed);
        let value = consume_payload(&frame, &mut at, &header).unwrap();
        assert_eq!(value, Value::I64s(vec![7, 8, 9]));
        assert_eq!(at, frame.len());
    }
}
