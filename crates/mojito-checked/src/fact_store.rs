//! Fact stores that log every key written, so a body's entries can be told
//! apart from every other body's without walking its syntax.
//!
//! A checker pass writes its facts into these stores; a body site notes the
//! log position before and after its check, and the next pass copies exactly
//! the logged range when the body's inputs are unchanged (the checker's
//! `body_carry` module). Reads go through `Deref` to the inner collection;
//! every writer is a method here, so nothing escapes the log.

use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::ops::{Deref, Range};

/// A `HashMap` whose writes are logged by key.
#[derive(Debug, Clone)]
pub struct FactMap<K, V> {
    map: HashMap<K, V>,
    log: Vec<K>,
}

impl<K, V> Default for FactMap<K, V> {
    fn default() -> Self {
        Self {
            map: HashMap::new(),
            log: Vec::new(),
        }
    }
}

impl<K, V> Deref for FactMap<K, V> {
    type Target = HashMap<K, V>;

    fn deref(&self) -> &HashMap<K, V> {
        &self.map
    }
}

impl<K, V> From<HashMap<K, V>> for FactMap<K, V> {
    /// Seed entries are not logged: no body of this pass wrote them.
    fn from(map: HashMap<K, V>) -> Self {
        Self {
            map,
            log: Vec::new(),
        }
    }
}

impl<K: Eq + Hash + Clone, V> FactMap<K, V> {
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        self.log.push(key.clone());
        self.map.insert(key, value)
    }

    /// The entry is logged whether or not the caller fills it: an empty
    /// logged key is skipped when copied.
    pub fn entry(&mut self, key: K) -> std::collections::hash_map::Entry<'_, K, V> {
        self.log.push(key.clone());
        self.map.entry(key)
    }

    /// A value handed out for mutation counts as written.
    pub fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        let value = self.map.get_mut(key)?;
        self.log.push(key.clone());
        Some(value)
    }

    pub fn remove(&mut self, key: &K) -> Option<V> {
        self.map.remove(key)
    }

    pub fn extend(&mut self, entries: impl IntoIterator<Item = (K, V)>) {
        for (key, value) in entries {
            self.insert(key, value);
        }
    }

    pub fn retain(&mut self, keep: impl FnMut(&K, &mut V) -> bool) {
        self.map.retain(keep);
    }

    pub fn clear(&mut self) {
        self.map.clear();
    }

    /// The log position: keys written from here on are `logged(mark..)`.
    pub const fn mark(&self) -> usize {
        self.log.len()
    }

    /// The keys written in `range` of the log, repeats included.
    pub fn logged(&self, range: Range<usize>) -> &[K] {
        &self.log[range]
    }

    pub fn into_inner(self) -> HashMap<K, V> {
        self.map
    }
}

/// A `HashSet` whose insertions are logged.
#[derive(Debug, Clone)]
pub struct FactSet<K> {
    set: HashSet<K>,
    log: Vec<K>,
}

impl<K> Default for FactSet<K> {
    fn default() -> Self {
        Self {
            set: HashSet::new(),
            log: Vec::new(),
        }
    }
}

impl<K> Deref for FactSet<K> {
    type Target = HashSet<K>;

    fn deref(&self) -> &HashSet<K> {
        &self.set
    }
}

impl<K> From<HashSet<K>> for FactSet<K> {
    fn from(set: HashSet<K>) -> Self {
        Self {
            set,
            log: Vec::new(),
        }
    }
}

impl<K: Eq + Hash + Clone> FactSet<K> {
    pub fn insert(&mut self, key: K) -> bool {
        self.log.push(key.clone());
        self.set.insert(key)
    }

    pub fn remove(&mut self, key: &K) -> bool {
        self.set.remove(key)
    }

    pub fn extend(&mut self, keys: impl IntoIterator<Item = K>) {
        for key in keys {
            self.insert(key);
        }
    }

    pub fn retain(&mut self, keep: impl FnMut(&K) -> bool) {
        self.set.retain(keep);
    }

    pub fn clear(&mut self) {
        self.set.clear();
    }

    pub const fn mark(&self) -> usize {
        self.log.len()
    }

    pub fn logged(&self, range: Range<usize>) -> &[K] {
        &self.log[range]
    }

    pub fn into_inner(self) -> HashSet<K> {
        self.set
    }
}

/// An append-only `Vec` store: the log is the store itself.
#[derive(Debug, Clone)]
pub struct FactVec<T> {
    items: Vec<T>,
}

impl<T> Default for FactVec<T> {
    fn default() -> Self {
        Self { items: Vec::new() }
    }
}

impl<T> Deref for FactVec<T> {
    type Target = Vec<T>;

    fn deref(&self) -> &Vec<T> {
        &self.items
    }
}

impl<T> FactVec<T> {
    pub fn push(&mut self, item: T) {
        self.items.push(item);
    }

    pub fn extend(&mut self, items: impl IntoIterator<Item = T>) {
        self.items.extend(items);
    }

    pub const fn mark(&self) -> usize {
        self.items.len()
    }

    pub fn logged(&self, range: Range<usize>) -> &[T] {
        &self.items[range]
    }

    pub fn into_inner(self) -> Vec<T> {
        self.items
    }
}
