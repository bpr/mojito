//! A struct's method signatures by name, which records a demand for a
//! method its struct declares but has not registered yet.
//!
//! Upstream emits a declaration's signature when a use first needs it,
//! whatever the source order. The checker registers a struct's method
//! signatures one name at a time: while it does, every declared name not yet
//! registered is *pending*, and a lookup that misses a pending name records
//! that name as demanded, so the registration can resolve that name's
//! signatures first and type the demanding signature again.

use super::MethodSig;
use std::collections::hash_map::{Entry, Iter, Keys};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Mutex;

/// The method table of a struct: each method name's overloads, in source
/// order, plus the registration's pending names and recorded demands. The
/// demands sit behind a `Mutex` since an installed package's tables are
/// shared across threads.
#[derive(Default)]
pub(super) struct MethodTable {
    methods: HashMap<String, Vec<MethodSig>>,
    pending: HashSet<String>,
    demanded: Mutex<BTreeSet<String>>,
}

impl MethodTable {
    /// The overloads of `name`, recording a demand when `name` is pending.
    pub(super) fn get(&self, name: &str) -> Option<&Vec<MethodSig>> {
        let found = self.methods.get(name);
        if found.is_none() {
            self.demand(name);
        }
        found
    }

    /// Whether `name` has a registered signature, recording a demand when
    /// it is pending.
    pub(super) fn contains_key(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    pub(super) fn get_mut(&mut self, name: &str) -> Option<&mut Vec<MethodSig>> {
        self.methods.get_mut(name)
    }

    pub(super) fn entry(&mut self, name: String) -> Entry<'_, String, Vec<MethodSig>> {
        self.methods.entry(name)
    }

    pub(super) fn iter(&self) -> Iter<'_, String, Vec<MethodSig>> {
        self.methods.iter()
    }

    pub(super) fn keys(&self) -> Keys<'_, String, Vec<MethodSig>> {
        self.methods.keys()
    }

    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.methods.is_empty()
    }

    /// Marks `names` declared but not yet registered.
    pub(super) fn begin_pending(&mut self, names: impl IntoIterator<Item = String>) {
        self.pending.extend(names);
    }

    /// Whether `name` is declared but not yet registered.
    pub(super) fn is_pending(&self, name: &str) -> bool {
        self.pending.contains(name)
    }

    /// Marks `name`'s signatures registered.
    pub(super) fn settle(&mut self, name: &str) {
        self.pending.remove(name);
    }

    /// Ends the registration: no name is pending any more.
    pub(super) fn settle_all(&mut self) {
        self.pending.clear();
        self.demands().clear();
    }

    /// The pending names looked up since the last call, in name order.
    pub(super) fn take_demands(&self) -> BTreeSet<String> {
        std::mem::take(&mut *self.demands())
    }

    fn demand(&self, name: &str) {
        if self.pending.contains(name) {
            self.demands().insert(name.to_string());
        }
    }

    fn demands(&self) -> std::sync::MutexGuard<'_, BTreeSet<String>> {
        self.demanded
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Clone for MethodTable {
    fn clone(&self) -> Self {
        Self {
            methods: self.methods.clone(),
            pending: self.pending.clone(),
            demanded: Mutex::new(self.demands().clone()),
        }
    }
}

impl std::ops::Index<&str> for MethodTable {
    type Output = Vec<MethodSig>;

    fn index(&self, name: &str) -> &Vec<MethodSig> {
        &self.methods[name]
    }
}

impl<'a> IntoIterator for &'a MethodTable {
    type Item = (&'a String, &'a Vec<MethodSig>);
    type IntoIter = Iter<'a, String, Vec<MethodSig>>;

    fn into_iter(self) -> Self::IntoIter {
        self.methods.iter()
    }
}

impl IntoIterator for MethodTable {
    type Item = (String, Vec<MethodSig>);
    type IntoIter = std::collections::hash_map::IntoIter<String, Vec<MethodSig>>;

    fn into_iter(self) -> Self::IntoIter {
        self.methods.into_iter()
    }
}

impl From<HashMap<String, Vec<MethodSig>>> for MethodTable {
    fn from(methods: HashMap<String, Vec<MethodSig>>) -> Self {
        Self {
            methods,
            ..Self::default()
        }
    }
}

impl serde::Serialize for MethodTable {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        super::interface::sorted(&self.methods, serializer)
    }
}

impl<'de> serde::Deserialize<'de> for MethodTable {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        HashMap::deserialize(deserializer).map(Self::from)
    }
}
