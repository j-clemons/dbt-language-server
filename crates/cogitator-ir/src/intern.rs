//! Per-plan string interner. The map is lookup-only; everything that
//! reaches output goes through `strings` in insertion order.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Serialize, Serializer};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct Symbol(u32);

impl Symbol {
    pub const fn raw(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Default)]
pub struct Interner {
    map: HashMap<Arc<str>, Symbol>,
    strings: Vec<Arc<str>>,
}

impl Interner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn intern(&mut self, text: &str) -> Symbol {
        if let Some(&sym) = self.map.get(text) {
            return sym;
        }
        let sym = Symbol(self.strings.len() as u32);
        // boundary: the interner is the owner of plan strings (08 §1.5)
        let owned: Arc<str> = Arc::from(text);
        self.strings.push(Arc::clone(&owned));
        self.map.insert(owned, sym);
        sym
    }

    pub fn get(&self, text: &str) -> Option<Symbol> {
        self.map.get(text).copied()
    }

    pub fn resolve(&self, sym: Symbol) -> Option<&str> {
        self.strings.get(sym.0 as usize).map(|s| &**s)
    }

    /// `resolve` for display: a foreign symbol prints as `<?>`.
    pub fn text(&self, sym: Symbol) -> &str {
        self.resolve(sym).unwrap_or("<?>")
    }

    pub fn len(&self) -> usize {
        self.strings.len()
    }

    pub fn is_empty(&self) -> bool {
        self.strings.is_empty()
    }
}

impl Serialize for Interner {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.strings.serialize(serializer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interning_is_idempotent_and_ordered() {
        let mut syms = Interner::new();
        let a = syms.intern("orders");
        let b = syms.intern("items");
        assert_eq!(syms.intern("orders"), a);
        assert_eq!(syms.len(), 2);
        assert_eq!(syms.resolve(b), Some("items"));
        assert_eq!(syms.get("nope"), None);
        assert!(a < b);
    }

    #[test]
    fn foreign_symbols_do_not_panic() {
        let mut other = Interner::new();
        let sym = other.intern("x");
        let empty = Interner::new();
        assert_eq!(empty.resolve(sym), None);
        assert_eq!(empty.text(sym), "<?>");
    }
}
