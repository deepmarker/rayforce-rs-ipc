//! Symbols, interned locally.
//!
//! A rayforce symbol travels on the wire as a NUL-terminated string: the integer id a server
//! hands out indexes its own intern table and means nothing to any other process. So this table
//! is ours alone. It exists for the reason rayforce's does — a column stores one small int per
//! row, and the string is written once per cell at serialization time — not because the peer
//! needs the id.
//!
//! The table never shrinks: symbols are venue codes, instrument names and column names, a
//! bounded set. Unlike the OCaml original it is thread-safe, so a symbol may be interned from
//! any thread, and one interned on one thread is usable on another.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, OnceLock, RwLock};

/// A symbol, identified by its row in this process's intern table.
///
/// Ids are dense, handed out in interning order, and valid for the life of the process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Sym(u32);

struct Entry {
    /// The name, shared so reading it costs a refcount rather than a copy.
    name: Arc<str>,
    /// The bytes that go on the wire: the name and its terminator. Worked out when the symbol
    /// is interned, so sizing a column is a read and writing a cell is one blit.
    wire_len: usize,
}

struct Interner {
    by_name: HashMap<Arc<str>, u32>,
    entries: Vec<Entry>,
}

impl Interner {
    fn new() -> Self {
        Self {
            by_name: HashMap::new(),
            entries: Vec::new(),
        }
    }
}

fn table() -> &'static RwLock<Interner> {
    static TABLE: OnceLock<RwLock<Interner>> = OnceLock::new();
    TABLE.get_or_init(|| RwLock::new(Interner::new()))
}

impl Sym {
    /// Intern `name`, returning the same symbol for the same name every time.
    ///
    /// # Panics
    ///
    /// Panics if `name` contains a NUL: the wire format terminates a symbol with one, so such a
    /// name cannot be spelled.
    #[must_use]
    pub fn intern(name: &str) -> Self {
        assert!(
            !name.contains('\0'),
            "a symbol cannot contain NUL: {name:?}"
        );
        if let Some(id) = table().read().expect("symbol table").by_name.get(name) {
            return Self(*id);
        }
        let mut table = table().write().expect("symbol table");
        // Another thread may have interned it between the two locks.
        if let Some(id) = table.by_name.get(name) {
            return Self(*id);
        }
        let id = u32::try_from(table.entries.len()).expect("symbol table overflow");
        let name: Arc<str> = Arc::from(name);
        let wire_len = name.len() + 1;
        table.by_name.insert(Arc::clone(&name), id);
        table.entries.push(Entry { name, wire_len });
        Self(id)
    }

    /// The symbol with this id, if one was ever interned.
    #[must_use]
    pub fn from_id(id: u32) -> Option<Self> {
        let table = table().read().expect("symbol table");
        (usize::try_from(id).is_ok_and(|id| id < table.entries.len())).then_some(Self(id))
    }

    /// The id, an index into this process's intern table.
    #[must_use]
    pub fn id(self) -> u32 {
        self.0
    }

    /// The name.
    ///
    /// # Panics
    ///
    /// Panics if the id was made up rather than handed out by [`Sym::intern`]. Ids are dense and
    /// monotonic, so this only happens through [`Sym::from_raw`].
    #[must_use]
    pub fn name(self) -> Arc<str> {
        let table = table().read().expect("symbol table");
        Arc::clone(&table.entries[self.0 as usize].name)
    }

    /// Bytes this symbol occupies on the wire: its name plus the NUL.
    #[must_use]
    pub fn wire_len(self) -> usize {
        let table = table().read().expect("symbol table");
        table.entries[self.0 as usize].wire_len
    }

    /// The bytes themselves, terminator included.
    #[must_use]
    pub fn wire(self) -> Vec<u8> {
        let table = table().read().expect("symbol table");
        let entry = &table.entries[self.0 as usize];
        let mut out = Vec::with_capacity(entry.wire_len);
        out.extend_from_slice(entry.name.as_bytes());
        out.push(0);
        out
    }

    /// Take an id on trust, for an id this process just read out of one of its own columns.
    ///
    /// # Safety
    ///
    /// Safe as a function, but the id is unchecked: a wrong one makes the next
    /// [`Sym::name`] or [`Sym::wire_len`] panic. The serializer uses it on symbols that came out
    /// of a column it filled itself, where the id is not in question.
    #[must_use]
    pub fn from_raw(id: u32) -> Self {
        Self(id)
    }
}

impl fmt::Display for Sym {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let table = table().read().expect("symbol table");
        f.write_str(&table.entries[self.0 as usize].name)
    }
}

impl From<&str> for Sym {
    fn from(name: &str) -> Self {
        Self::intern(name)
    }
}

impl From<String> for Sym {
    fn from(name: String) -> Self {
        Self::intern(&name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interning_is_idempotent() {
        let a = Sym::intern("BTCUSDT");
        let b = Sym::intern("BTCUSDT");
        assert_eq!(a, b);
        assert_eq!(a.name().as_ref(), "BTCUSDT");
        assert_eq!(Sym::from_id(a.id()), Some(a));
        assert_eq!(a.id(), b.id());
    }

    #[test]
    fn distinct_names_get_distinct_ids() {
        assert_ne!(Sym::intern("EURUSD"), Sym::intern("GBPUSD"));
    }

    #[test]
    fn wire_form_is_the_name_and_a_nul() {
        let sym = Sym::intern("xyz:AAPL");
        let wire = sym.wire();
        assert_eq!(wire.len(), sym.wire_len());
        assert_eq!(wire, b"xyz:AAPL\0");
    }

    #[test]
    #[should_panic(expected = "cannot contain NUL")]
    fn names_with_nul_are_refused() {
        let _ = Sym::intern("a\0b");
    }

    #[test]
    fn display_is_the_name() {
        assert_eq!(Sym::intern("tel").to_string(), "tel");
    }
}
