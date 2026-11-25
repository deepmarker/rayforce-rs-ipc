//! A column being built, in the layout it goes on the wire in.
//!
//! The point is what it does *not* do. A producer accumulating rows in a `Vec<i64>` per batch
//! pays for a container, a memcpy into the frame, and the collector's copy; here a row is
//! written into a flat buffer as it arrives, and the whole column reaches the frame as one
//! `extend_from_slice`. Adding an `i64`, a `bool` or a [`Sym`] allocates nothing at all once the
//! buffer has grown.
//!
//! The buffer is kept across flushes: [`Column::clear`] resets the row count and keeps the
//! capacity, so a steady feed stops allocating entirely once its batches reach their usual size.

use crate::sym::Sym;
use crate::tag;
use crate::value::Value;
use crate::Error;

/// A column of one type, filled a row at a time.
///
/// Every cell is `width` bytes wide in `buf`, which is exactly the vector payload the wire
/// format asks for — except for a symbol column, whose cells hold ids here and strings on the
/// wire.
#[derive(Debug, Clone)]
pub struct Column {
    buf: Vec<u8>,
    rows: usize,
    /// Wire bytes of the symbols added so far, so sizing the column is a read.
    sym_bytes: usize,
    width: usize,
    tag: u8,
}

impl Column {
    /// An empty column of `width`-byte cells carrying `tag`.
    #[must_use]
    pub fn with_capacity(tag: u8, width: usize, capacity: usize) -> Self {
        Self {
            buf: Vec::with_capacity(capacity * width),
            rows: 0,
            sym_bytes: 0,
            width,
            tag,
        }
    }

    /// A column of 64-bit integers.
    #[must_use]
    pub fn i64(capacity: usize) -> Self {
        Self::with_capacity(tag::I64, 8, capacity)
    }

    /// A column of 64-bit floats.
    #[must_use]
    pub fn f64(capacity: usize) -> Self {
        Self::with_capacity(tag::F64, 8, capacity)
    }

    /// A column of timespans: signed nanoseconds with no epoch.
    #[must_use]
    pub fn timespan(capacity: usize) -> Self {
        Self::with_capacity(tag::TIMESPAN, 8, capacity)
    }

    /// A column of timestamps: nanoseconds since 2000-01-01, see [`crate::temporal`].
    #[must_use]
    pub fn timestamp(capacity: usize) -> Self {
        Self::with_capacity(tag::TIMESTAMP, 8, capacity)
    }

    /// A column of booleans.
    #[must_use]
    pub fn bool(capacity: usize) -> Self {
        Self::with_capacity(tag::BOOL, 1, capacity)
    }

    /// A column of symbols, held as ids and written as their names.
    #[must_use]
    pub fn sym(capacity: usize) -> Self {
        Self::with_capacity(tag::SYM, 8, capacity)
    }

    /// Rows in the column.
    #[must_use]
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// Whether the column holds no rows.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows == 0
    }

    /// Drops the rows, keeps the buffer.
    pub fn clear(&mut self) {
        self.rows = 0;
        self.sym_bytes = 0;
        self.buf.clear();
    }

    /// The type tag this column writes.
    #[must_use]
    pub fn tag(&self) -> u8 {
        self.tag
    }

    /// Bytes the cells occupy in a frame.
    #[must_use]
    pub fn wire_size(&self) -> usize {
        if self.tag == tag::SYM {
            self.sym_bytes
        } else {
            self.rows * self.width
        }
    }

    fn push_cell(&mut self, cell: &[u8]) {
        debug_assert_eq!(cell.len(), self.width);
        self.buf.extend_from_slice(cell);
        self.rows += 1;
    }

    /// Append a raw 8-byte cell: the i64, timespan and timestamp columns all take one.
    pub fn add_i64(&mut self, value: i64) {
        self.push_cell(&value.to_le_bytes());
    }

    /// Append a float.
    pub fn add_f64(&mut self, value: f64) {
        self.push_cell(&value.to_le_bytes());
    }

    /// Append a boolean.
    pub fn add_bool(&mut self, value: bool) {
        self.push_cell(&[u8::from(value)]);
    }

    /// Append a symbol, recording what its name will cost on the wire.
    pub fn add_sym(&mut self, value: Sym) {
        self.sym_bytes += value.wire_len();
        self.push_cell(&i64::from(value.id()).to_le_bytes());
    }

    fn cell(&self, row: usize) -> &[u8] {
        let start = row * self.width;
        &self.buf[start..start + self.width]
    }

    /// The value this column writes.
    ///
    /// # Panics
    ///
    /// Panics if `row` is past the end: reading back is for tests and assertions, not for the
    /// hot path.
    #[must_use]
    pub fn get_i64(&self, row: usize) -> i64 {
        let cell = self.cell(row);
        i64::from_le_bytes(cell.try_into().expect("8-byte cell"))
    }

    /// The float in a row.
    ///
    /// # Panics
    ///
    /// Panics if the column does not hold floats or `row` is past the end.
    #[must_use]
    pub fn get_f64(&self, row: usize) -> f64 {
        self.expect(tag::F64, "floats").expect("column type");
        f64::from_le_bytes(self.cell(row).try_into().expect("8-byte cell"))
    }

    /// The boolean in a row.
    ///
    /// # Panics
    ///
    /// Panics if the column does not hold booleans or `row` is past the end.
    #[must_use]
    pub fn get_bool(&self, row: usize) -> bool {
        self.expect(tag::BOOL, "booleans").expect("column type");
        self.cell(row)[0] != 0
    }

    /// The symbol in a row.
    ///
    /// # Panics
    ///
    /// Panics if the column does not hold symbols or `row` is past the end.
    #[must_use]
    pub fn get_sym(&self, row: usize) -> Sym {
        self.expect(tag::SYM, "symbols").expect("column type");
        Sym::from_id(u32::try_from(self.get_i64(row)).expect("symbol id in range"))
            .expect("symbol id interned")
    }

    /// The same rows as an ordinary value, which allocates.
    #[must_use]
    pub fn to_value(&self) -> Value {
        match self.tag {
            tag::F64 => Value::F64s((0..self.rows).map(|row| self.get_f64(row)).collect()),
            tag::BOOL => Value::Bools((0..self.rows).map(|row| self.get_bool(row)).collect()),
            tag::SYM => Value::Syms((0..self.rows).map(|row| self.get_sym(row)).collect()),
            tag::TIMESPAN => {
                Value::Timespans((0..self.rows).map(|row| self.get_i64(row)).collect())
            }
            tag::TIMESTAMP => {
                Value::Timestamps((0..self.rows).map(|row| self.get_i64(row)).collect())
            }
            tag::I64 => Value::I64s((0..self.rows).map(|row| self.get_i64(row)).collect()),
            other => panic!("a column of tag {other} has no value form here"),
        }
    }

    /// Write the cells into `out`; the caller has already written the vector header.
    pub(crate) fn fill(&self, out: &mut Vec<u8>) {
        if self.tag != tag::SYM {
            out.extend_from_slice(&self.buf);
            return;
        }
        for row in 0..self.rows {
            let id = u32::try_from(self.get_i64(row)).expect("symbol id in range");
            let sym = Sym::from_id(id).expect("symbol id interned");
            out.extend_from_slice(&sym.wire());
        }
    }

    /// Check that a column can be read as `tag`.
    pub(crate) fn expect(&self, tag: u8, what: &str) -> Result<(), Error> {
        if self.tag == tag {
            Ok(())
        } else {
            Err(Error::protocol(format!(
                "column holds tag {}, not {what}",
                self.tag
            )))
        }
    }
}

impl PartialEq for Column {
    fn eq(&self, other: &Self) -> bool {
        self.tag == other.tag && self.rows == other.rows && self.buf == other.buf
    }
}

impl Eq for Column {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integers_grow_and_read_back() {
        let mut column = Column::i64(2);
        for i in 0..1_000 {
            column.add_i64(i);
        }
        assert_eq!(column.rows(), 1_000);
        assert_eq!(column.get_i64(0), 0);
        assert_eq!(column.get_i64(999), 999);
        assert_eq!(column.wire_size(), 8_000);
    }

    #[test]
    fn clearing_keeps_the_capacity() {
        let mut column = Column::f64(1_024);
        column.add_f64(1.5);
        let capacity = column.buf.capacity();
        column.clear();
        assert!(column.is_empty());
        assert_eq!(column.buf.capacity(), capacity);
        column.add_f64(2.5);
        assert_eq!(column.get_f64(0), 2.5);
    }

    #[test]
    fn symbols_cost_their_names_on_the_wire() {
        let mut column = Column::sym(4);
        for name in ["BTCUSDT", "ETHUSDT", "BTCUSDT"] {
            column.add_sym(Sym::intern(name));
        }
        assert_eq!(column.rows(), 3);
        assert_eq!(column.wire_size(), 3 * 8);
        assert_eq!(column.get_sym(1), Sym::intern("ETHUSDT"));
        assert_eq!(
            column.to_value(),
            Value::Syms(vec![
                Sym::intern("BTCUSDT"),
                Sym::intern("ETHUSDT"),
                Sym::intern("BTCUSDT"),
            ])
        );
    }

    #[test]
    fn a_wrong_tag_is_reported() {
        let column = Column::i64(1);
        assert!(column.expect(tag::I64, "integers").is_ok());
        let message = column.expect(tag::F64, "floats").unwrap_err().to_string();
        assert_eq!(
            message,
            format!("column holds tag {}, not floats", tag::I64)
        );
    }
}
