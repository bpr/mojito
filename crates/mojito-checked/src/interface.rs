//! What a compiled package lends the check of a program that imports it,
//! beside its declarations.
//!
//! A package's bodies were checked when it was built. Its importer installs
//! the package's declarations from their records and checks no package body
//! again: what a call site reads from a callee's body, its transfer and
//! call-through effect summaries, rides in the callee's declaration record
//! (`crate::package`); what the package says of itself as a whole comes
//! from here.

use std::collections::BTreeSet;

/// The checked facts of a package as a whole its importer reads.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PackageInterface {
    /// The source module (its stamped provenance) of every declaration the
    /// package compiled: a declaration from one of them has its body in the
    /// package.
    pub modules: BTreeSet<String>,
    /// The linked name of every module-level declaration those modules
    /// make: an importer declaring one again cannot import the package.
    pub names: BTreeSet<String>,
    /// Every trait the package's declarations and bodies ask a type's
    /// conformance to: an importer's struct answers each, since a package
    /// body may ask it of the importer's types.
    pub asked_traits: BTreeSet<String>,
}

impl PackageInterface {
    /// Whether a declaration stamped with `module` has its body in the
    /// package.
    pub fn compiles(&self, module: Option<&str>) -> bool {
        module.is_some_and(|module| self.modules.contains(module))
    }
}
