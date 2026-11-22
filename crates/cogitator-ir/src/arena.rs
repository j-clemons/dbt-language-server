//! Typed index arenas (08 §1 "spans are indices" applied to IR nodes).
//!
//! `Idx<T>` is a `u32`; an `Arena<T>` is only ever appended to, so an
//! `Idx` handed out once stays valid for that arena's life.

use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::ops::{Index, IndexMut};

use serde::{Serialize, Serializer};

pub struct Idx<T> {
    raw: u32,
    _marker: PhantomData<fn() -> T>,
}

impl<T> Idx<T> {
    pub const fn from_raw(raw: u32) -> Self {
        Self {
            raw,
            _marker: PhantomData,
        }
    }

    pub const fn raw(self) -> u32 {
        self.raw
    }

    pub const fn index(self) -> usize {
        self.raw as usize
    }
}

impl<T> Clone for Idx<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for Idx<T> {}
impl<T> PartialEq for Idx<T> {
    fn eq(&self, other: &Self) -> bool {
        self.raw == other.raw
    }
}
impl<T> Eq for Idx<T> {}
impl<T> PartialOrd for Idx<T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl<T> Ord for Idx<T> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.raw.cmp(&other.raw)
    }
}
impl<T> Hash for Idx<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.raw.hash(state);
    }
}
impl<T> fmt::Debug for Idx<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.raw)
    }
}
impl<T> Serialize for Idx<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u32(self.raw)
    }
}

#[derive(Debug, Clone)]
pub struct Arena<T> {
    data: Vec<T>,
}

impl<T> Default for Arena<T> {
    fn default() -> Self {
        Self { data: Vec::new() }
    }
}

impl<T> Arena<T> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends `value`. The front end's node cap (specs/ir-pipeline/00)
    /// keeps arenas far below `u32::MAX`; on overflow the id saturates
    /// and the id-range verifier rejects the plan instead of panicking.
    pub fn alloc(&mut self, value: T) -> Idx<T> {
        let raw = u32::try_from(self.data.len()).unwrap_or(u32::MAX);
        self.data.push(value);
        Idx::from_raw(raw)
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn contains(&self, id: Idx<T>) -> bool {
        id.index() < self.data.len()
    }

    pub fn get(&self, id: Idx<T>) -> Option<&T> {
        self.data.get(id.index())
    }

    pub fn get_mut(&mut self, id: Idx<T>) -> Option<&mut T> {
        self.data.get_mut(id.index())
    }

    /// Entries in allocation order — the deterministic iteration order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = (Idx<T>, &T)> + '_ {
        self.data
            .iter()
            .enumerate()
            .map(|(i, v)| (Idx::from_raw(i as u32), v))
    }

    pub fn values(&self) -> impl ExactSizeIterator<Item = &T> + '_ {
        self.data.iter()
    }

    pub fn ids(&self) -> impl ExactSizeIterator<Item = Idx<T>> + '_ {
        (0..self.data.len() as u32).map(Idx::from_raw)
    }
}

/// Panics on an out-of-range id: IR that reaches indexing has passed
/// its verifier, whose first check is id range. Code that may see
/// unverified IR (the verifiers, the printers) uses [`Arena::get`].
impl<T> Index<Idx<T>> for Arena<T> {
    type Output = T;

    fn index(&self, id: Idx<T>) -> &T {
        &self.data[id.index()]
    }
}

impl<T> IndexMut<Idx<T>> for Arena<T> {
    fn index_mut(&mut self, id: Idx<T>) -> &mut T {
        &mut self.data[id.index()]
    }
}

impl<T: Serialize> Serialize for Arena<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.data.serialize(serializer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alloc_is_sequential_and_get_is_total() {
        let mut arena = Arena::new();
        let a = arena.alloc("a");
        let b = arena.alloc("b");
        assert_eq!((a.raw(), b.raw()), (0, 1));
        assert_eq!(arena[b], "b");
        assert_eq!(arena.get(Idx::from_raw(9)), None);
        assert!(!arena.contains(Idx::from_raw(2)));
        let order: Vec<_> = arena.iter().map(|(id, v)| (id.raw(), *v)).collect();
        assert_eq!(order, vec![(0, "a"), (1, "b")]);
    }
}
