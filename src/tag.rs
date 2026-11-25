//! Type tags, from rayforce's `include/rayforce.h`.
//!
//! A vector carries the tag itself, an atom of the same type carries its negation, and both
//! [`NULL`] and [`ERROR`] are wire-only markers with no vector form.

/// A list of values (`10 + payload`).
pub const LIST: u8 = 0;
/// Boolean.
pub const BOOL: u8 = 1;
/// Unsigned byte.
pub const U8: u8 = 2;
/// 16-bit signed integer.
pub const I16: u8 = 3;
/// 32-bit signed integer.
pub const I32: u8 = 4;
/// 64-bit signed integer.
pub const I64: u8 = 5;
/// 32-bit float, carried as its IEEE-754 bits.
pub const F32: u8 = 6;
/// 64-bit float.
pub const F64: u8 = 7;
/// Days since 2000-01-01.
pub const DATE: u8 = 8;
/// Milliseconds since midnight.
pub const TIME: u8 = 9;
/// Nanoseconds since 2000-01-01.
pub const TIMESTAMP: u8 = 10;
/// 16 bytes of GUID.
pub const GUID: u8 = 11;
/// A symbol, carried as a NUL-terminated string.
pub const SYM: u8 = 12;
/// A string.
pub const STR: u8 = 13;
/// Signed nanoseconds, with no epoch.
pub const TIMESPAN: u8 = 16;
/// A table: a symbol vector of column names and a list of columns.
pub const TABLE: u8 = 98;
/// A dictionary: keys then values.
pub const DICT: u8 = 99;
/// A lambda: parameters, body, and an optional closure.
pub const LAMBDA: u8 = 100;
/// A named unary builtin.
pub const UNARY: u8 = 101;
/// A named binary builtin.
pub const BINARY: u8 = 102;
/// A named variadic builtin.
pub const VARIADIC: u8 = 103;

/// Types this library does not decode. They are named so that meeting one reports what it is
/// instead of reading its first bytes as a length.
pub const UNDECODED: [(u8, &str); 1] = [(97, "index")];

/// The typed value is absent; the whole value is this one byte.
pub const NULL: u8 = 126;
/// An error: a code and a message.
pub const ERROR: u8 = 127;
