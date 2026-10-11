//! Fact stores that log every key written, so a body's entries can be told
//! apart from every other body's without walking its syntax.
//!
//! A checker pass writes its facts into these stores; a body site notes the
//! log position before and after its check, and the next pass copies exactly
//! the logged range when the body's inputs are unchanged (the checker's
//! `body_carry` module). Reads go through `Deref` to the inner collection;
//! every writer is a method here, so nothing escapes the log.
//!
//! A store also undoes writes. Between `open_probe` and `roll_back` every
//! write journals what it overwrote, and rolling back restores it and
//! truncates the log, so a speculative check (an overload candidate) leaves
//! neither a fact nor a logged key behind. Probes nest: a mark is a position,
//! and rolling back pops only what was written after it.

use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::ops::{Deref, Range};

/// A store's log and journal positions when a probe opened.
#[derive(Debug, Clone, Copy)]
pub struct ProbeMark {
    log: usize,
    undo: usize,
}

/// A `HashMap` whose writes are logged by key.
#[derive(Debug, Clone)]
pub struct FactMap<K, V> {
    map: HashMap<K, V>,
    log: Vec<K>,
    /// Each probed write's key and the value it displaced, oldest first.
    undo: Vec<(K, Option<V>)>,
    probes: u32,
}

impl<K, V> Default for FactMap<K, V> {
    fn default() -> Self {
        Self {
            map: HashMap::new(),
            log: Vec::new(),
            undo: Vec::new(),
            probes: 0,
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
            ..Self::default()
        }
    }
}

impl<K, V> FactMap<K, V> {
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

impl<K: Eq + Hash + Clone, V: Clone> FactMap<K, V> {
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        self.log.push(key.clone());
        if self.probes == 0 {
            return self.map.insert(key, value);
        }
        let previous = self.map.insert(key.clone(), value);
        self.undo.push((key, previous.clone()));
        previous
    }

    /// The entry is logged whether or not the caller fills it: an empty
    /// logged key is skipped when copied.
    pub fn entry(&mut self, key: K) -> std::collections::hash_map::Entry<'_, K, V> {
        self.log.push(key.clone());
        self.journal(&key);
        self.map.entry(key)
    }

    /// A value handed out for mutation counts as written.
    pub fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        if !self.map.contains_key(key) {
            return None;
        }
        self.log.push(key.clone());
        self.journal(key);
        self.map.get_mut(key)
    }

    pub fn remove(&mut self, key: &K) -> Option<V> {
        let removed = self.map.remove(key);
        if self.probes > 0
            && let Some(value) = &removed
        {
            self.undo.push((key.clone(), Some(value.clone())));
        }
        removed
    }

    pub fn extend(&mut self, entries: impl IntoIterator<Item = (K, V)>) {
        for (key, value) in entries {
            self.insert(key, value);
        }
    }

    /// Under a probe every entry is journaled first, since `keep` may
    /// mutate the entries it keeps.
    pub fn retain(&mut self, keep: impl FnMut(&K, &mut V) -> bool) {
        if self.probes > 0 {
            self.undo.extend(
                self.map
                    .iter()
                    .map(|(key, value)| (key.clone(), Some(value.clone()))),
            );
        }
        self.map.retain(keep);
    }

    pub fn clear(&mut self) {
        if self.probes > 0 {
            self.undo
                .extend(self.map.drain().map(|(key, value)| (key, Some(value))));
        } else {
            self.map.clear();
        }
    }

    /// Start journaling writes; `roll_back(mark)` undoes them.
    pub const fn open_probe(&mut self) -> ProbeMark {
        self.probes += 1;
        ProbeMark {
            log: self.log.len(),
            undo: self.undo.len(),
        }
    }

    /// Undo every write since `mark` was opened, newest first, and drop
    /// the keys it logged.
    pub fn roll_back(&mut self, mark: ProbeMark) {
        for (key, previous) in self.undo.drain(mark.undo..).rev() {
            match previous {
                Some(value) => self.map.insert(key, value),
                None => self.map.remove(&key),
            };
        }
        self.log.truncate(mark.log);
        self.probes -= 1;
    }

    fn journal(&mut self, key: &K) {
        if self.probes > 0 {
            self.undo.push((key.clone(), self.map.get(key).cloned()));
        }
    }
}

/// A `HashSet` whose insertions are logged.
#[derive(Debug, Clone)]
pub struct FactSet<K> {
    set: HashSet<K>,
    log: Vec<K>,
    /// Each probed change's key and whether it was present before.
    undo: Vec<(K, bool)>,
    probes: u32,
}

impl<K> Default for FactSet<K> {
    fn default() -> Self {
        Self {
            set: HashSet::new(),
            log: Vec::new(),
            undo: Vec::new(),
            probes: 0,
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
            ..Self::default()
        }
    }
}

impl<K: Eq + Hash + Clone> FactSet<K> {
    pub fn insert(&mut self, key: K) -> bool {
        self.log.push(key.clone());
        if self.probes == 0 {
            return self.set.insert(key);
        }
        let added = self.set.insert(key.clone());
        if added {
            self.undo.push((key, false));
        }
        added
    }

    pub fn remove(&mut self, key: &K) -> bool {
        let removed = self.set.remove(key);
        if removed && self.probes > 0 {
            self.undo.push((key.clone(), true));
        }
        removed
    }

    pub fn extend(&mut self, keys: impl IntoIterator<Item = K>) {
        for key in keys {
            self.insert(key);
        }
    }

    pub fn retain(&mut self, mut keep: impl FnMut(&K) -> bool) {
        if self.probes > 0 {
            self.undo
                .extend(self.set.extract_if(|key| !keep(key)).map(|key| (key, true)));
        } else {
            self.set.retain(keep);
        }
    }

    pub fn clear(&mut self) {
        if self.probes > 0 {
            self.undo.extend(self.set.drain().map(|key| (key, true)));
        } else {
            self.set.clear();
        }
    }

    /// Start journaling changes; `roll_back(mark)` undoes them.
    pub const fn open_probe(&mut self) -> ProbeMark {
        self.probes += 1;
        ProbeMark {
            log: self.log.len(),
            undo: self.undo.len(),
        }
    }

    /// Undo every change since `mark` was opened and drop the keys it
    /// logged.
    pub fn roll_back(&mut self, mark: ProbeMark) {
        for (key, present) in self.undo.drain(mark.undo..).rev() {
            if present {
                self.set.insert(key);
            } else {
                self.set.remove(&key);
            }
        }
        self.log.truncate(mark.log);
        self.probes -= 1;
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

    /// A vector store journals nothing: rolling back truncates it.
    pub const fn open_probe(&self) -> ProbeMark {
        ProbeMark {
            log: self.items.len(),
            undo: 0,
        }
    }

    pub fn roll_back(&mut self, mark: ProbeMark) {
        self.items.truncate(mark.log);
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
