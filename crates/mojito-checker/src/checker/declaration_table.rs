//! The checker's declaration-keyed tables: the entries the check registers
//! from the program's own statements, over an imported package's, which a
//! lookup the own entries miss resolves from the package's declaration
//! records the first time one is asked for ([`InstalledInterfaces`]), as
//! upstream's importer resolves a package declaration on first lookup.
//!
//! Only the own entries are mutable: an installed declaration is the
//! package build's, and the check never changes one.

use super::interface::{DeclarationInterface, InstalledInterfaces, Table};
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};

/// A table of values keyed by a declaration's name, or by a key belonging
/// to one (an overload's signature-qualified name, a member's
/// `Owner.member`).
pub struct DeclarationTable<V: 'static> {
    own: HashMap<String, V>,
    installed: Option<InstalledRows<V>>,
}

impl<V: 'static> DeclarationTable<V> {
    /// Resolve the keys the own entries miss from `interfaces`' `table`,
    /// whose rows a record holds at `rows`.
    pub(super) fn attach(
        &mut self,
        interfaces: &InstalledInterfaces,
        table: Table,
        rows: fn(&DeclarationInterface) -> &[(String, V)],
    ) {
        self.installed = Some(InstalledRows {
            interfaces: interfaces.clone(),
            table,
            rows,
        });
    }

    pub fn get(&self, key: &str) -> Option<&V> {
        self.own
            .get(key)
            .or_else(|| self.installed.as_ref()?.get(key).map(|(_, value)| value))
    }

    pub fn contains_key(&self, key: &str) -> bool {
        self.own.contains_key(key)
            || self
                .installed
                .as_ref()
                .is_some_and(|installed| installed.interfaces.has_key(installed.table, key))
    }

    /// The own entry `key`; an installed entry is never mutated.
    pub(super) fn get_mut(&mut self, key: &str) -> Option<&mut V> {
        self.own.get_mut(key)
    }

    pub(super) fn insert(&mut self, key: String, value: V) -> Option<V> {
        self.own.insert(key, value)
    }

    /// Remove the own entry `key`.
    pub(super) fn remove(&mut self, key: &str) -> Option<V> {
        self.own.remove(key)
    }

    /// The own entry `key`, for in-place registration.
    pub(super) fn entry(&mut self, key: String) -> Entry<'_, String, V> {
        self.own.entry(key)
    }

    /// Every key, the installed ones read from the package's directory
    /// without resolving a record.
    pub(super) fn keys(&self) -> impl Iterator<Item = &String> {
        self.own.keys().chain(
            self.installed
                .iter()
                .flat_map(|installed| installed.interfaces.keys(installed.table)),
        )
    }

    /// Every entry, which resolves every installed record holding one.
    pub(super) fn iter(&self) -> impl Iterator<Item = (&String, &V)> {
        self.own
            .iter()
            .chain(self.installed.iter().flat_map(|installed| {
                installed
                    .interfaces
                    .keys(installed.table)
                    .filter_map(|key| installed.get(key))
            }))
    }

    /// Every own entry, and each installed entry under a key `select`
    /// names, resolving only those keys' records.
    pub(super) fn iter_selected<'a, I>(
        &'a self,
        select: impl FnOnce(&'a InstalledInterfaces) -> I,
    ) -> impl Iterator<Item = (&'a String, &'a V)>
    where
        I: Iterator<Item = &'a String> + 'a,
    {
        let installed = self.installed.as_ref().map(|installed| {
            select(&installed.interfaces).filter_map(move |key| installed.get(key))
        });
        self.own.iter().chain(installed.into_iter().flatten())
    }

    /// Whether the table has no entry, decoding nothing.
    pub fn is_empty(&self) -> bool {
        self.keys().next().is_none()
    }

    /// The own entries: those registered from the program's own
    /// statements.
    pub(super) const fn own(&self) -> &HashMap<String, V> {
        &self.own
    }
}

impl<V: 'static> Default for DeclarationTable<V> {
    fn default() -> Self {
        Self {
            own: HashMap::new(),
            installed: None,
        }
    }
}

impl<V: 'static> Extend<(String, V)> for DeclarationTable<V> {
    fn extend<I: IntoIterator<Item = (String, V)>>(&mut self, entries: I) {
        self.own.extend(entries);
    }
}

impl<V: 'static> std::ops::Index<&str> for DeclarationTable<V> {
    type Output = V;

    fn index(&self, key: &str) -> &V {
        self.get(key)
            .unwrap_or_else(|| panic!("no declaration table entry `{key}`"))
    }
}

/// A set of declaration names, the installed ones answered from the
/// package's directory without resolving a record.
#[derive(Default)]
pub(super) struct DeclarationSet {
    own: HashSet<String>,
    installed: Option<(InstalledInterfaces, Table)>,
}

impl DeclarationSet {
    /// Answer the names the own set misses from `interfaces`' `table`.
    pub(super) fn attach(&mut self, interfaces: &InstalledInterfaces, table: Table) {
        self.installed = Some((interfaces.clone(), table));
    }

    pub(super) fn contains(&self, name: &str) -> bool {
        self.own.contains(name)
            || self
                .installed
                .as_ref()
                .is_some_and(|(interfaces, table)| interfaces.has_key(*table, name))
    }

    pub(super) fn insert(&mut self, name: String) -> bool {
        self.own.insert(name)
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = &String> {
        self.own.iter().chain(
            self.installed
                .iter()
                .flat_map(|(interfaces, table)| interfaces.keys(*table)),
        )
    }
}

impl Extend<String> for DeclarationSet {
    fn extend<I: IntoIterator<Item = String>>(&mut self, names: I) {
        self.own.extend(names);
    }
}

/// Where a table's installed rows are: the interfaces, the table's
/// directory keys, and the field of a record holding its rows.
struct InstalledRows<V: 'static> {
    interfaces: InstalledInterfaces,
    table: Table,
    rows: fn(&DeclarationInterface) -> &[(String, V)],
}

impl<V: 'static> InstalledRows<V> {
    /// The row `key`, resolving the record of the declaration it belongs to.
    fn get(&self, key: &str) -> Option<(&String, &V)> {
        let rows = (self.rows)(self.interfaces.declaration(self.table, key)?);
        rows.binary_search_by(|(name, _)| name.as_str().cmp(key))
            .ok()
            .map(|position| (&rows[position].0, &rows[position].1))
    }
}
