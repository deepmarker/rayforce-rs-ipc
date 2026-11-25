//! A Rayforce value, in the shape the wire format describes it.
//!
//! This is not a mirror of Rayforce's in-memory `ray_t`: it carries only what the format
//! carries, which is why a column is an ordinary `Vec` and not a reference into a foreign heap.
//! Serializing writes those bytes straight into the frame, so the only copy between a producer's
//! accumulator and the socket is the one it makes itself.
//!
//! Columns are always written without the `HAS_NULLS` attribute. Rayforce encodes nulls as
//! per-type sentinels in the payload, so a null cell is a sentinel this library does not
//! interpret — except where a whole value is null, which is [`Value::Null`] or an
//! [`Value::AtomFlags`] with the typed-null bit set.
//!
//! The array-like constructors are the convenient way to write a value; [`Column`] is the way to
//! build one a row at a time without allocating.

use crate::column::Column;
use crate::sym::Sym;
use crate::tag;
use crate::{Error, Result};

/// The schema entries of a table: a column name and its data.
///
/// A table is written as a symbol vector of names followed by a list of columns, in the same
/// order.
#[derive(Debug, Clone, PartialEq)]
pub struct TableColumn {
    /// The column name.
    pub name: Sym,
    /// The column data.
    pub data: Value,
}

/// A value the wire format can carry.
///
/// The variant names follow the type tags rather than the OCaml original's constructors: `I32`
/// and `I32s` are what the OCaml calls `Int32` and `Int32s`, `F64`/`F64s` are its `Float`/
/// `Floats`, and `Str`/`Strs` are its `String`/`Strings`.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Value {
    /// The typed absence: one byte on the wire.
    Null,
    /// An atom carrying explicit wire flags, including typed null (bit 0) and quoted symbol
    /// (bit 5). Ordinary atoms with zero flags keep their own constructor.
    AtomFlags(u8, Box<Self>),
    /// A boolean atom.
    Bool(bool),
    /// An unsigned byte atom.
    U8(u8),
    /// A 16-bit signed integer atom.
    I16(i16),
    /// A 32-bit signed integer atom.
    I32(i32),
    /// A 64-bit signed integer atom.
    I64(i64),
    /// A 64-bit float atom.
    F64(f64),
    /// A 32-bit float atom, carried as its IEEE-754 bits.
    F32(u32),
    /// A time of day, in milliseconds.
    Time(i32),
    /// A GUID atom, exactly 16 raw bytes.
    Guid([u8; 16]),
    /// A timespan atom: signed nanoseconds, no epoch.
    Timespan(i64),
    /// A timestamp atom: nanoseconds since 2000-01-01, see [`crate::temporal`].
    Timestamp(i64),
    /// A date atom: days since 2000-01-01.
    Date(i32),
    /// A symbol atom.
    Sym(Sym),
    /// A string atom.
    Str(String),
    /// Booleans.
    Bools(Vec<bool>),
    /// Bytes.
    Bytes(Vec<u8>),
    /// 16-bit signed integers.
    I16s(Vec<i16>),
    /// 32-bit signed integers.
    I32s(Vec<i32>),
    /// Dates: days since 2000-01-01.
    Dates(Vec<i32>),
    /// Times of day, in milliseconds.
    Times(Vec<i32>),
    /// 32-bit floats, carried as their IEEE-754 bits.
    F32s(Vec<u32>),
    /// GUIDs, exactly 16 raw bytes each.
    Guids(Vec<[u8; 16]>),
    /// 64-bit signed integers.
    I64s(Vec<i64>),
    /// 64-bit floats.
    F64s(Vec<f64>),
    /// Timespans: signed nanoseconds. `i64::MIN` entries are null.
    Timespans(Vec<i64>),
    /// Timestamps: nanoseconds since 2000-01-01. `i64::MIN` entries are null.
    Timestamps(Vec<i64>),
    /// Symbols.
    Syms(Vec<Sym>),
    /// Strings.
    Strs(Vec<String>),
    /// A column built row by row, already in wire layout.
    Col(Column),
    /// A list of values.
    List(Vec<Self>),
    /// A table: named columns of equal length.
    Table(Vec<TableColumn>),
    /// A dictionary: keys and values.
    Dict(Box<Self>, Box<Self>),
    /// A lambda: parameters, body, and an optional captured environment.
    Lambda {
        /// The parameter list, usually a symbol vector.
        params: Box<Self>,
        /// The body.
        body: Box<Self>,
        /// The captured environment, when the lambda closed over one.
        closure: Option<Box<Self>>,
    },
    /// A named unary builtin.
    Unary(String),
    /// A named binary builtin.
    Binary(String),
    /// A named variadic builtin.
    Variadic(String),
    /// An error: a code of at most 7 ASCII bytes and a message of at most 255 bytes.
    Error {
        /// A Rayforce error code.
        code: String,
        /// The diagnostic text; may be empty.
        message: String,
    },
}

/// The schema names of a table are the one place the C serializer writes a non-zero attrs byte:
/// `RAY_SYM_W64`, the index width of the vector it came from. The decoder ignores it, but
/// writing the same byte keeps these frames comparable with Rayforce's own.
const SYM_W64: u8 = 3;

/// An error record's message is length-prefixed by one byte.
fn error_message_len(message: &str) -> usize {
    message.len().min(255)
}

impl Value {
    /// A table, in column order.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Protocol`] if the columns differ in length: a table with ragged columns
    /// is not a table.
    pub fn table<S: AsRef<str>>(columns: impl IntoIterator<Item = (S, Value)>) -> Result<Self> {
        let columns: Vec<TableColumn> = columns
            .into_iter()
            .map(|(name, data)| TableColumn {
                name: Sym::intern(name.as_ref()),
                data,
            })
            .collect();
        if let Some(first) = columns.first() {
            for column in &columns {
                if column.data.length() != first.data.length() {
                    return Err(Error::protocol(format!(
                        "table: column {} has {} rows, {} has {}",
                        column.name,
                        column.data.length(),
                        first.name,
                        first.data.length()
                    )));
                }
            }
        }
        Ok(Self::Table(columns))
    }

    /// Rows in this value: a scalar is one row, a list or table is its element count.
    #[must_use]
    pub fn length(&self) -> usize {
        match self {
            Self::Bools(cells) => cells.len(),
            Self::Bytes(cells) => cells.len(),
            Self::I16s(cells) => cells.len(),
            Self::I32s(cells) | Self::Dates(cells) | Self::Times(cells) => cells.len(),
            Self::F32s(cells) => cells.len(),
            Self::Guids(cells) => cells.len(),
            Self::I64s(cells) | Self::Timestamps(cells) | Self::Timespans(cells) => cells.len(),
            Self::F64s(cells) => cells.len(),
            Self::Syms(cells) => cells.len(),
            Self::Strs(cells) => cells.len(),
            Self::Col(column) => column.rows(),
            Self::List(values) => values.len(),
            Self::Table(columns) => columns.first().map_or(0, |column| column.data.length()),
            Self::Dict(keys, _) => keys.length(),
            _ => 1,
        }
    }

    /// Rows in a table.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Protocol`] if this is not a table.
    pub fn nrows(&self) -> Result<usize> {
        match self {
            Self::Table(columns) => Ok(columns.first().map_or(0, |column| column.data.length())),
            other => Err(Error::protocol(format!(
                "nrows: not a table ({})",
                other.summary()
            ))),
        }
    }

    /// Columns in a table.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Protocol`] if this is not a table.
    pub fn ncols(&self) -> Result<usize> {
        match self {
            Self::Table(columns) => Ok(columns.len()),
            other => Err(Error::protocol(format!(
                "ncols: not a table ({})",
                other.summary()
            ))),
        }
    }

    /// Look a column up by name.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Protocol`] if this is not a table.
    pub fn column(&self, name: &str) -> Result<Option<&Self>> {
        match self {
            Self::Table(columns) => Ok(columns
                .iter()
                .find(|column| column.name.name().as_ref() == name)
                .map(|column| &column.data)),
            other => Err(Error::protocol(format!(
                "column: not a table ({})",
                other.summary()
            ))),
        }
    }

    /// A one-line description, for error messages and assertions.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Null => "Null",
            Self::AtomFlags(..) => "AtomFlags",
            Self::Bool(_) => "Bool",
            Self::U8(_) => "U8",
            Self::I16(_) => "I16",
            Self::I32(_) => "I32",
            Self::I64(_) => "I64",
            Self::F64(_) => "F64",
            Self::F32(_) => "F32",
            Self::Time(_) => "Time",
            Self::Guid(_) => "Guid",
            Self::Timespan(_) => "Timespan",
            Self::Timestamp(_) => "Timestamp",
            Self::Date(_) => "Date",
            Self::Sym(_) => "Sym",
            Self::Str(_) => "Str",
            Self::Bools(_) => "Bools",
            Self::Bytes(_) => "Bytes",
            Self::I16s(_) => "I16s",
            Self::I32s(_) => "I32s",
            Self::Dates(_) => "Dates",
            Self::Times(_) => "Times",
            Self::F32s(_) => "F32s",
            Self::Guids(_) => "Guids",
            Self::I64s(_) => "I64s",
            Self::F64s(_) => "F64s",
            Self::Timespans(_) => "Timespans",
            Self::Timestamps(_) => "Timestamps",
            Self::Syms(_) => "Syms",
            Self::Strs(_) => "Strs",
            Self::Col(_) => "Col",
            Self::List(_) => "List",
            Self::Table(_) => "Table",
            Self::Dict(..) => "Dict",
            Self::Lambda { .. } => "Lambda",
            Self::Unary(_) => "Unary",
            Self::Binary(_) => "Binary",
            Self::Variadic(_) => "Variadic",
            Self::Error { .. } => "Error",
        }
    }

    /// The kind and the rows, for an assertion that fails.
    fn summary(&self) -> String {
        format!("{} of {}", self.kind(), self.length())
    }

    // ===== Size =====

    /// Bytes [`Value::fill`] will write.
    ///
    /// An atom is `type(1) + flags(1) + value`; a vector is `type(1) + attrs(1) + len(8) +
    /// payload`.
    #[must_use]
    pub fn size(&self) -> usize {
        match self {
            Self::Null => 1,
            Self::AtomFlags(_, value) => value.size(),
            Self::Error { message, .. } => 1 + 8 + 1 + error_message_len(message),
            Self::Bool(_) | Self::U8(_) => atom(1),
            Self::I16(_) => atom(2),
            Self::Guid(_) => atom(16),
            Self::I32(_) | Self::Date(_) | Self::Time(_) | Self::F32(_) => atom(4),
            Self::I64(_) | Self::F64(_) | Self::Timestamp(_) | Self::Timespan(_) => atom(8),
            Self::Sym(sym) => atom(cstring(sym.name().len())),
            Self::Str(text) => atom(8 + text.len()),
            Self::Bools(cells) => vector(cells.len()),
            Self::Bytes(cells) => vector(cells.len()),
            Self::I16s(cells) => vector(cells.len() * 2),
            Self::I32s(cells) | Self::Dates(cells) | Self::Times(cells) => vector(cells.len() * 4),
            Self::F32s(cells) => vector(cells.len() * 4),
            Self::Guids(cells) => vector(cells.len() * 16),
            Self::I64s(cells) | Self::Timestamps(cells) | Self::Timespans(cells) => {
                vector(cells.len() * 8)
            }
            Self::F64s(cells) => vector(cells.len() * 8),
            Self::Syms(cells) => vector(cells.iter().map(|sym| sym.wire_len()).sum()),
            Self::Strs(cells) => vector(cells.iter().map(|text| 8 + text.len()).sum()),
            Self::Col(column) => vector(column.wire_size()),
            Self::List(values) => vector(values.iter().map(Self::size).sum()),
            Self::Table(columns) => {
                2 + vector(columns.iter().map(|column| column.name.wire_len()).sum())
                    + vector(columns.iter().map(|column| column.data.size()).sum())
            }
            Self::Dict(keys, values) => 2 + keys.size() + values.size(),
            Self::Lambda {
                params,
                body,
                closure,
            } => {
                2 + params.size()
                    + body.size()
                    + closure.as_ref().map_or(0, |closure| closure.size())
            }
            Self::Unary(name) | Self::Binary(name) | Self::Variadic(name) => {
                1 + cstring(name.len())
            }
        }
    }

    // ===== Encode =====

    /// Write this value into `out`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Protocol`] if the value cannot be spelled: a GUID that is not 16 bytes,
    /// a flagged value that is not an atom, or flags outside a byte.
    pub fn fill(&self, out: &mut Vec<u8>) -> Result<()> {
        match self {
            Self::Null => out.push(tag::NULL),
            Self::AtomFlags(flags, value) => {
                // Re-encode the atom to find its tag, then overwrite the flags byte it wrote.
                let mut buf = Vec::with_capacity(value.size());
                value.fill(&mut buf)?;
                let mut encoded = Wire::new(&buf);
                let raw = encoded.i8()?;
                if raw >= 0 {
                    return Err(Error::protocol("atom flags require an atom"));
                }
                out.push(raw as u8);
                out.push(*flags);
                out.extend_from_slice(&buf[2..]);
            }
            Self::Error { code, message } => {
                out.push(tag::ERROR);
                let mut padded = [0_u8; 8];
                for (slot, byte) in padded.iter_mut().zip(code.bytes()) {
                    *slot = byte;
                }
                out.extend_from_slice(&padded);
                let len = error_message_len(message);
                out.push(u8::try_from(len).expect("capped at 255"));
                out.extend_from_slice(&message.as_bytes()[..len]);
            }
            Self::Bool(value) => {
                atom_head(out, tag::BOOL);
                out.push(u8::from(*value));
            }
            Self::U8(value) => {
                atom_head(out, tag::U8);
                out.push(*value);
            }
            Self::I16(value) => {
                atom_head(out, tag::I16);
                out.extend_from_slice(&value.to_le_bytes());
            }
            Self::I32(value) | Self::Date(value) | Self::Time(value) => {
                atom_head(
                    out,
                    match self {
                        Self::I32(_) => tag::I32,
                        Self::Date(_) => tag::DATE,
                        _ => tag::TIME,
                    },
                );
                out.extend_from_slice(&value.to_le_bytes());
            }
            Self::F32(bits) => {
                atom_head(out, tag::F32);
                out.extend_from_slice(&bits.to_le_bytes());
            }
            Self::Guid(bytes) => {
                atom_head(out, tag::GUID);
                out.extend_from_slice(bytes);
            }
            Self::I64(value) | Self::Timespan(value) | Self::Timestamp(value) => {
                atom_head(
                    out,
                    match self {
                        Self::I64(_) => tag::I64,
                        Self::Timespan(_) => tag::TIMESPAN,
                        _ => tag::TIMESTAMP,
                    },
                );
                out.extend_from_slice(&value.to_le_bytes());
            }
            Self::F64(value) => {
                atom_head(out, tag::F64);
                out.extend_from_slice(&value.to_le_bytes());
            }
            Self::Sym(sym) => {
                atom_head(out, tag::SYM);
                out.extend_from_slice(&sym.wire());
            }
            Self::Str(text) => {
                atom_head(out, tag::STR);
                out.extend_from_slice(&len_bytes(text.len()));
                out.extend_from_slice(text.as_bytes());
            }
            Self::Bools(cells) => {
                vector_head(out, tag::BOOL, cells.len(), 0);
                out.extend(cells.iter().map(|cell| u8::from(*cell)));
            }
            Self::Bytes(cells) => {
                vector_head(out, tag::U8, cells.len(), 0);
                out.extend_from_slice(cells);
            }
            Self::I16s(cells) => {
                vector_head(out, tag::I16, cells.len(), 0);
                for cell in cells {
                    out.extend_from_slice(&cell.to_le_bytes());
                }
            }
            Self::I32s(cells) | Self::Dates(cells) | Self::Times(cells) => {
                vector_head(
                    out,
                    match self {
                        Self::I32s(_) => tag::I32,
                        Self::Dates(_) => tag::DATE,
                        _ => tag::TIME,
                    },
                    cells.len(),
                    0,
                );
                for cell in cells {
                    out.extend_from_slice(&cell.to_le_bytes());
                }
            }
            Self::F32s(cells) => {
                vector_head(out, tag::F32, cells.len(), 0);
                for cell in cells {
                    out.extend_from_slice(&cell.to_le_bytes());
                }
            }
            Self::Guids(cells) => {
                vector_head(out, tag::GUID, cells.len(), 0);
                for cell in cells {
                    out.extend_from_slice(cell);
                }
            }
            Self::I64s(cells) | Self::Timespans(cells) | Self::Timestamps(cells) => {
                vector_head(
                    out,
                    match self {
                        Self::I64s(_) => tag::I64,
                        Self::Timespans(_) => tag::TIMESPAN,
                        _ => tag::TIMESTAMP,
                    },
                    cells.len(),
                    0,
                );
                for cell in cells {
                    out.extend_from_slice(&cell.to_le_bytes());
                }
            }
            Self::F64s(cells) => {
                vector_head(out, tag::F64, cells.len(), 0);
                for cell in cells {
                    out.extend_from_slice(&cell.to_le_bytes());
                }
            }
            Self::Syms(cells) => {
                sym_vector(out, cells, 0);
            }
            Self::Strs(cells) => {
                vector_head(out, tag::STR, cells.len(), 0);
                for cell in cells {
                    out.extend_from_slice(&len_bytes(cell.len()));
                    out.extend_from_slice(cell.as_bytes());
                }
            }
            Self::Col(column) => {
                vector_head(out, column.tag(), column.rows(), 0);
                column.fill(out);
            }
            Self::List(values) => {
                vector_head(out, tag::LIST, values.len(), 0);
                for value in values {
                    value.fill(out)?;
                }
            }
            Self::Table(columns) => {
                out.push(tag::TABLE);
                out.push(0);
                let names: Vec<Sym> = columns.iter().map(|column| column.name).collect();
                sym_vector(out, &names, SYM_W64);
                vector_head(out, tag::LIST, columns.len(), 0);
                for column in columns {
                    column.data.fill(out)?;
                }
            }
            Self::Dict(keys, values) => {
                out.push(tag::DICT);
                out.push(0);
                keys.fill(out)?;
                values.fill(out)?;
            }
            Self::Lambda {
                params,
                body,
                closure,
            } => {
                out.push(tag::LAMBDA);
                out.push(u8::from(closure.is_some()));
                params.fill(out)?;
                body.fill(out)?;
                if let Some(closure) = closure {
                    closure.fill(out)?;
                }
            }
            Self::Unary(name) | Self::Binary(name) | Self::Variadic(name) => {
                out.push(match self {
                    Self::Unary(_) => tag::UNARY,
                    Self::Binary(_) => tag::BINARY,
                    _ => tag::VARIADIC,
                });
                out.extend_from_slice(name.as_bytes());
                out.push(0);
            }
        }
        Ok(())
    }

    // ===== Decode =====

    /// Read one value from `buf`, advancing `at` past it.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Protocol`] on anything malformed, and on a type Rayforce does not
    /// serialize (such as an index).
    pub fn consume(buf: &[u8], at: &mut usize) -> Result<Self> {
        let mut wire = Wire::new_at(buf, *at);
        let value = wire.value()?;
        *at = wire.at();
        Ok(value)
    }
}

/// `type(1) + flags(1) + payload`.
const fn atom(payload: usize) -> usize {
    2 + payload
}

/// `type(1) + attrs(1) + len(8) + payload`.
const fn vector(payload: usize) -> usize {
    10 + payload
}

/// A NUL-terminated name.
const fn cstring(name: usize) -> usize {
    name + 1
}

fn len_bytes(len: usize) -> [u8; 8] {
    i64::try_from(len)
        .expect("length fits in i64")
        .to_le_bytes()
}

fn atom_head(out: &mut Vec<u8>, tag: u8) {
    out.push((tag as i8).wrapping_neg() as u8);
    out.push(0);
}

fn vector_head(out: &mut Vec<u8>, tag: u8, len: usize, attrs: u8) {
    out.push(tag);
    out.push(attrs);
    out.extend_from_slice(&len_bytes(len));
}

fn sym_vector(out: &mut Vec<u8>, syms: &[Sym], attrs: u8) {
    vector_head(out, tag::SYM, syms.len(), attrs);
    for sym in syms {
        out.extend_from_slice(&sym.wire());
    }
}

/// A cursor over one frame's payload.
struct Wire<'a> {
    buf: &'a [u8],
    at: usize,
}

impl<'a> Wire<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self::new_at(buf, 0)
    }

    fn new_at(buf: &'a [u8], at: usize) -> Self {
        Self { buf, at }
    }

    fn at(&self) -> usize {
        self.at
    }

    fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.at)
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self
            .at
            .checked_add(len)
            .filter(|end| *end <= self.buf.len())
            .ok_or_else(|| {
                Error::protocol(format!(
                    "frame ends after {} bytes, another {} were asked for",
                    self.remaining(),
                    len
                ))
            })?;
        let slice = &self.buf[self.at..end];
        self.at = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn i8(&mut self) -> Result<i8> {
        Ok(self.u8()? as i8)
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().expect("4 bytes"),
        ))
    }

    fn i16(&mut self) -> Result<i16> {
        Ok(i16::from_le_bytes(
            self.take(2)?.try_into().expect("2 bytes"),
        ))
    }

    fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(
            self.take(4)?.try_into().expect("4 bytes"),
        ))
    }

    fn i64(&mut self) -> Result<i64> {
        Ok(i64::from_le_bytes(
            self.take(8)?.try_into().expect("8 bytes"),
        ))
    }

    fn f64(&mut self) -> Result<f64> {
        Ok(f64::from_le_bytes(
            self.take(8)?.try_into().expect("8 bytes"),
        ))
    }

    /// A length, refused when it cannot possibly fit in what is left: every element costs at
    /// least one byte, so a count past the end of the frame is a malformed frame, not a large
    /// one. Refusing before allocating is the point.
    fn length(&mut self, what: &str) -> Result<usize> {
        let len = self.i64()?;
        let remaining = i64::try_from(self.remaining()).unwrap_or(i64::MAX);
        if len < 0 || len > remaining {
            return Err(Error::protocol(format!(
                "{what}: length {len} is not possible in {remaining} remaining bytes"
            )));
        }
        Ok(usize::try_from(len).expect("non-negative"))
    }

    fn cstring(&mut self) -> Result<String> {
        let rest = &self.buf[self.at..];
        let Some(len) = rest.iter().position(|byte| *byte == 0) else {
            return Err(Error::protocol(format!(
                "symbol with no terminator in {} remaining bytes",
                rest.len()
            )));
        };
        let name = std::str::from_utf8(&rest[..len])
            .map_err(|error| Error::protocol(format!("symbol is not UTF-8: {error}")))?
            .to_owned();
        self.at += len + 1;
        Ok(name)
    }

    fn value(&mut self) -> Result<Value> {
        // An atom tells us its type by negating the tag; a vector writes it as is.
        let raw = self.i8()?;
        let byte = raw as u8;
        if byte == tag::NULL {
            return Ok(Value::Null);
        }
        if byte == tag::ERROR {
            let code = self.take(8)?;
            let code = code.split(|byte| *byte == 0).next().unwrap_or_default();
            let code = String::from_utf8_lossy(code).into_owned();
            let len = usize::from(self.u8()?);
            let message = String::from_utf8_lossy(self.take(len)?).into_owned();
            return Ok(Value::Error { code, message });
        }
        if raw < 0 {
            return self.atom(raw.wrapping_neg() as u8);
        }
        self.vector(byte)
    }

    fn atom(&mut self, tag: u8) -> Result<Value> {
        // Preserve atom flags as well as the payload: typed nulls and quoted symbols are only
        // visible here.
        let flags = self.u8()?;
        let value = match tag {
            tag::BOOL => Value::Bool(self.u8()? != 0),
            tag::U8 => Value::U8(self.u8()?),
            tag::I16 => Value::I16(self.i16()?),
            tag::I32 => Value::I32(self.i32()?),
            tag::DATE => Value::Date(self.i32()?),
            tag::TIME => Value::Time(self.i32()?),
            tag::F32 => Value::F32(self.u32()?),
            tag::GUID => Value::Guid(self.take(16)?.try_into().expect("16 bytes")),
            tag::I64 => Value::I64(self.i64()?),
            tag::TIMESPAN => Value::Timespan(self.i64()?),
            tag::TIMESTAMP => Value::Timestamp(self.i64()?),
            tag::F64 => Value::F64(self.f64()?),
            tag::SYM => Value::Sym(Sym::intern(&self.cstring()?)),
            tag::STR => {
                let len = self.length("string atom")?;
                Value::Str(String::from_utf8_lossy(self.take(len)?).into_owned())
            }
            undecoded => {
                if let Some((_, name)) = tag::UNDECODED.iter().find(|(tag, _)| *tag == undecoded) {
                    return Err(Error::protocol(format!(
                        "a {name} atom is not a value this library decodes"
                    )));
                }
                return Err(Error::protocol(format!(
                    "atom type {tag} is not a Rayforce type"
                )));
            }
        };
        if flags == 0 {
            Ok(value)
        } else {
            Ok(Value::AtomFlags(flags, Box::new(value)))
        }
    }

    fn vector(&mut self, tag: u8) -> Result<Value> {
        if let Some((_, name)) = tag::UNDECODED
            .iter()
            .find(|(undecoded, _)| *undecoded == tag)
        {
            return Err(Error::protocol(format!(
                "a {name} is not a value this library decodes"
            )));
        }
        match tag {
            // Named builtins carry their name and no attrs byte.
            tag::UNARY => return Ok(Value::Unary(self.cstring()?)),
            tag::BINARY => return Ok(Value::Binary(self.cstring()?)),
            tag::VARIADIC => return Ok(Value::Variadic(self.cstring()?)),
            _ => {}
        }
        let attrs = self.u8()?;
        match tag {
            tag::LAMBDA => {
                let params = self.value()?;
                let body = self.value()?;
                let closure = (attrs & 1 != 0).then(|| self.value()).transpose()?;
                Ok(Value::Lambda {
                    params: Box::new(params),
                    body: Box::new(body),
                    closure: closure.map(Box::new),
                })
            }
            tag::TABLE => {
                // A table is its schema -- a symbol vector of names -- followed by its columns
                // as a list. The two are read back together: neither is a table on its own.
                let names = match self.value()? {
                    Value::Syms(names) => names,
                    other => {
                        return Err(Error::protocol(format!(
                            "table schema is a {}, not a symbol vector",
                            other.kind()
                        )))
                    }
                };
                let columns = match self.value()? {
                    Value::List(columns) => columns,
                    other => {
                        return Err(Error::protocol(format!(
                            "table columns are a {}, not a list",
                            other.kind()
                        )))
                    }
                };
                if names.len() != columns.len() {
                    return Err(Error::protocol(format!(
                        "table has {} column names and {} columns",
                        names.len(),
                        columns.len()
                    )));
                }
                Ok(Value::Table(
                    names
                        .into_iter()
                        .zip(columns)
                        .map(|(name, data)| TableColumn { name, data })
                        .collect(),
                ))
            }
            tag::DICT => {
                let keys = self.value()?;
                let values = self.value()?;
                Ok(Value::Dict(Box::new(keys), Box::new(values)))
            }
            tag::BOOL => {
                let len = self.length("vector")?;
                Ok(Value::Bools(
                    self.take(len)?.iter().map(|byte| *byte != 0).collect(),
                ))
            }
            tag::U8 => {
                let len = self.length("vector")?;
                Ok(Value::Bytes(self.take(len)?.to_vec()))
            }
            tag::I16 => {
                let len = self.length("vector")?;
                self.take_mapped(len, 2, |cell| {
                    i16::from_le_bytes(cell.try_into().expect("2"))
                })
                .map(Value::I16s)
            }
            tag::I32 => {
                let len = self.length("vector")?;
                self.take_mapped(len, 4, |cell| {
                    i32::from_le_bytes(cell.try_into().expect("4"))
                })
                .map(Value::I32s)
            }
            tag::DATE => {
                let len = self.length("vector")?;
                self.take_mapped(len, 4, |cell| {
                    i32::from_le_bytes(cell.try_into().expect("4"))
                })
                .map(Value::Dates)
            }
            tag::TIME => {
                let len = self.length("vector")?;
                self.take_mapped(len, 4, |cell| {
                    i32::from_le_bytes(cell.try_into().expect("4"))
                })
                .map(Value::Times)
            }
            tag::F32 => {
                let len = self.length("vector")?;
                self.take_mapped(len, 4, |cell| {
                    u32::from_le_bytes(cell.try_into().expect("4"))
                })
                .map(Value::F32s)
            }
            tag::GUID => {
                let len = self.length("vector")?;
                self.take_mapped(len, 16, |cell| cell.try_into().expect("16"))
                    .map(Value::Guids)
            }
            tag::I64 => {
                let len = self.length("vector")?;
                self.take_mapped(len, 8, |cell| {
                    i64::from_le_bytes(cell.try_into().expect("8"))
                })
                .map(Value::I64s)
            }
            tag::TIMESPAN => {
                let len = self.length("vector")?;
                self.take_mapped(len, 8, |cell| {
                    i64::from_le_bytes(cell.try_into().expect("8"))
                })
                .map(Value::Timespans)
            }
            tag::TIMESTAMP => {
                let len = self.length("vector")?;
                self.take_mapped(len, 8, |cell| {
                    i64::from_le_bytes(cell.try_into().expect("8"))
                })
                .map(Value::Timestamps)
            }
            tag::F64 => {
                let len = self.length("vector")?;
                self.take_mapped(len, 8, |cell| {
                    f64::from_le_bytes(cell.try_into().expect("8"))
                })
                .map(Value::F64s)
            }
            tag::SYM => {
                let len = self.length("vector")?;
                let mut syms = Vec::with_capacity(len.min(1_024));
                for _ in 0..len {
                    syms.push(Sym::intern(&self.cstring()?));
                }
                Ok(Value::Syms(syms))
            }
            tag::STR => {
                let len = self.length("vector")?;
                let mut strings = Vec::with_capacity(len.min(1_024));
                for _ in 0..len {
                    let len = self.length("string element")?;
                    strings.push(String::from_utf8_lossy(self.take(len)?).into_owned());
                }
                Ok(Value::Strs(strings))
            }
            tag::LIST => {
                let len = self.length("vector")?;
                let mut values = Vec::with_capacity(len.min(1_024));
                for _ in 0..len {
                    values.push(self.value()?);
                }
                Ok(Value::List(values))
            }
            other => Err(Error::protocol(format!(
                "vector type {other} is not a Rayforce type"
            ))),
        }
    }

    /// Read `len` cells of `width` bytes each, mapping each one.
    fn take_mapped<T>(
        &mut self,
        len: usize,
        width: usize,
        mut cell: impl FnMut(&[u8]) -> T,
    ) -> Result<Vec<T>> {
        let bytes = len
            .checked_mul(width)
            .ok_or_else(|| Error::protocol(format!("{len} cells of {width} bytes overflows")))?;
        let slice = self.take(bytes)?;
        Ok(slice.chunks_exact(width).map(&mut cell).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(value: &Value) -> Value {
        let mut buf = Vec::new();
        value.fill(&mut buf).expect("encode");
        assert_eq!(
            buf.len(),
            value.size(),
            "size disagrees with what was written for {value:?}"
        );
        let mut at = 0;
        let decoded = Value::consume(&buf, &mut at).expect("decode");
        assert_eq!(
            at,
            buf.len(),
            "decode did not consume the whole frame for {value:?}"
        );
        decoded
    }

    fn assert_round_trips(value: Value) {
        assert_eq!(round_trip(&value), value);
    }

    #[test]
    fn atoms_round_trip() {
        assert_round_trips(Value::Null);
        assert_round_trips(Value::Bool(true));
        assert_round_trips(Value::Bool(false));
        assert_round_trips(Value::U8(255));
        assert_round_trips(Value::I16(-1_234));
        assert_round_trips(Value::I32(-123_456));
        assert_round_trips(Value::I64(i64::MIN + 1));
        assert_round_trips(Value::F64(-1.5));
        assert_round_trips(Value::F32(0x3f80_0000));
        assert_round_trips(Value::Time(1_000));
        assert_round_trips(Value::Date(9_000));
        assert_round_trips(Value::Timespan(-42));
        assert_round_trips(Value::Timestamp(1_789_732_800_500_000_000));
        assert_round_trips(Value::Guid([7; 16]));
        assert_round_trips(Value::Sym(Sym::intern("BTCUSDT")));
        assert_round_trips(Value::Str("a string".into()));
        assert_round_trips(Value::Str(String::new()));
    }

    #[test]
    fn vectors_round_trip() {
        assert_round_trips(Value::Bools(vec![true, false, true]));
        assert_round_trips(Value::Bytes(vec![0, 1, 2, 255]));
        assert_round_trips(Value::I16s(vec![-1, 0, 1]));
        assert_round_trips(Value::I32s(vec![-1, 0, 1]));
        assert_round_trips(Value::Dates(vec![0, 9_000]));
        assert_round_trips(Value::Times(vec![0, 86_399_999]));
        assert_round_trips(Value::F32s(vec![0x3f80_0000, 0xbf80_0000]));
        assert_round_trips(Value::Guids(vec![[1; 16], [2; 16]]));
        assert_round_trips(Value::I64s(vec![i64::MIN, 0, i64::MAX]));
        assert_round_trips(Value::F64s(vec![1.5, -2.5]));
        assert_round_trips(Value::Timespans(vec![0, -1, i64::MIN]));
        assert_round_trips(Value::Timestamps(vec![0, i64::MIN]));
        assert_round_trips(Value::Syms(vec![Sym::intern("a"), Sym::intern("bc")]));
        assert_round_trips(Value::Strs(vec!["one".into(), String::new()]));
        assert_round_trips(Value::Bools(Vec::new()));
        assert_round_trips(Value::Syms(Vec::new()));
    }

    #[test]
    fn composites_round_trip() {
        assert_round_trips(Value::List(vec![
            Value::Sym(Sym::intern("upd")),
            Value::Sym(Sym::intern("depth")),
            Value::I64s(vec![1, 2, 3]),
        ]));
        assert_round_trips(Value::Dict(
            Box::new(Value::Syms(vec![Sym::intern("k")])),
            Box::new(Value::I64s(vec![1])),
        ));
        assert_round_trips(Value::Lambda {
            params: Box::new(Value::Syms(vec![Sym::intern("x")])),
            body: Box::new(Value::Sym(Sym::intern("x"))),
            closure: None,
        });
        assert_round_trips(Value::Lambda {
            params: Box::new(Value::Syms(Vec::new())),
            body: Box::new(Value::Null),
            closure: Some(Box::new(Value::I64(1))),
        });
        assert_round_trips(Value::Unary("count".into()));
        assert_round_trips(Value::Binary("take".into()));
        assert_round_trips(Value::Variadic("list".into()));
        assert_round_trips(Value::Error {
            code: "type".into(),
            message: "bad type".into(),
        });
        assert_round_trips(Value::Error {
            code: String::new(),
            message: String::new(),
        });
    }

    #[test]
    fn atom_flags_are_preserved_through_the_negated_tag() {
        let typed_null = Value::AtomFlags(1, Box::new(Value::I64(i64::MIN)));
        let mut buf = Vec::new();
        typed_null.fill(&mut buf).unwrap();
        assert_eq!(buf[0], (tag::I64 as i8).wrapping_neg() as u8);
        assert_eq!(buf[1], 1);
        assert_eq!(round_trip(&typed_null), typed_null);

        let quoted = Value::AtomFlags(32, Box::new(Value::Sym(Sym::intern("quote"))));
        assert_eq!(round_trip(&quoted), quoted);
    }

    #[test]
    fn a_table_round_trips_column_for_column() {
        let table = Value::table([
            ("ts_evt", Value::Timestamps(vec![1, 2, 3])),
            (
                "instrument",
                Value::Syms(vec![
                    Sym::intern("BTCUSDT"),
                    Sym::intern("ETHUSDT"),
                    Sym::intern("SOLUSDT"),
                ]),
            ),
            ("px", Value::F64s(vec![1.5, 2.5, 3.5])),
            ("live", Value::Bools(vec![true, false, true])),
        ])
        .unwrap();
        assert_eq!(table.nrows().unwrap(), 3);
        assert_eq!(table.ncols().unwrap(), 4);
        assert_eq!(round_trip(&table), table);
        assert_eq!(
            table.column("instrument").unwrap(),
            Some(&Value::Syms(vec![
                Sym::intern("BTCUSDT"),
                Sym::intern("ETHUSDT"),
                Sym::intern("SOLUSDT"),
            ]))
        );
        assert_eq!(table.column("absent").unwrap(), None);
    }

    #[test]
    fn a_built_column_travels_as_its_vector() {
        let mut column = Column::i64(4);
        for value in [10_i64, 20, 30] {
            column.add_i64(value);
        }
        let value = Value::Col(column);
        assert_eq!(round_trip(&value), Value::I64s(vec![10, 20, 30]));
        assert_eq!(value.length(), 3);
    }

    #[test]
    fn a_ragged_table_is_refused() {
        let error = Value::table([("a", Value::I64s(vec![1, 2])), ("b", Value::I64s(vec![1]))])
            .unwrap_err();
        assert_eq!(error.to_string(), "table: column b has 1 rows, a has 2");
    }

    #[test]
    fn empty_tables_are_allowed() {
        let table = Value::table(Vec::<(String, Value)>::new()).unwrap();
        assert_eq!(table.nrows().unwrap(), 0);
        assert_eq!(table.ncols().unwrap(), 0);
        assert_eq!(round_trip(&table), table);
    }

    #[test]
    fn a_short_guid_is_refused_by_the_decoder() {
        // An atom tells us it is a GUID, then the frame ends.
        let buf = [(tag::GUID as i8).wrapping_neg() as u8, 0, 1, 2, 3];
        let mut at = 0;
        let error = Value::consume(&buf, &mut at).unwrap_err();
        assert!(
            error.to_string().contains("another 16 were asked for"),
            "{error}"
        );
    }

    #[test]
    fn a_symbol_with_no_terminator_is_refused() {
        let buf = [(tag::SYM as i8).wrapping_neg() as u8, 0, b'a', b'b'];
        let mut at = 0;
        let error = Value::consume(&buf, &mut at).unwrap_err();
        assert_eq!(
            error.to_string(),
            "symbol with no terminator in 2 remaining bytes"
        );
    }

    #[test]
    fn an_impossible_length_is_refused_before_allocating() {
        let mut buf = vec![tag::I64, 0];
        buf.extend_from_slice(&i64::MAX.to_le_bytes());
        let mut at = 0;
        let error = Value::consume(&buf, &mut at).unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("vector: length 9223372036854775807"),
            "{error}"
        );
    }

    #[test]
    fn an_index_is_named_rather_than_misread() {
        let buf = [97_u8, 0];
        let mut at = 0;
        let error = Value::consume(&buf, &mut at).unwrap_err();
        assert_eq!(
            error.to_string(),
            "a index is not a value this library decodes"
        );
    }

    #[test]
    fn an_unknown_atom_type_is_reported() {
        let buf = [(-42_i8) as u8, 0];
        let mut at = 0;
        let error = Value::consume(&buf, &mut at).unwrap_err();
        assert_eq!(error.to_string(), "atom type 42 is not a Rayforce type");
    }

    #[test]
    fn an_error_cuts_its_message_at_255_bytes() {
        let message = "x".repeat(300);
        let value = Value::Error {
            code: "overflow".into(),
            message: message.clone(),
        };
        let mut buf = Vec::new();
        value.fill(&mut buf).unwrap();
        assert_eq!(buf.len(), 1 + 8 + 1 + 255);
        let mut at = 0;
        match Value::consume(&buf, &mut at).unwrap() {
            Value::Error { code, message: got } => {
                assert_eq!(code, "overflow");
                assert_eq!(got.len(), 255);
            }
            other => panic!("decoded a {}", other.kind()),
        }
    }
}
