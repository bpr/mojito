//! What a compiled package says of itself as a whole, beside its
//! declarations: what its importer reads to decide it can import it.
//!
//! A package's bodies were checked when it was built. Its importer installs
//! the package's declarations from their records and checks no package body
//! again: what a call site reads from a callee's body, its transfer and
//! call-through effect summaries, rides in the callee's declaration record
//! (`crate::package`); which modules it compiled is its module table's to
//! say (`crate::package::PackageDeclarations::compiles`); what the package
//! says of itself as a whole comes from here.

use std::collections::BTreeSet;

/// The checked facts of a package as a whole its importer reads.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PackageInterface {
    /// The linked name of every module-level declaration the package's
    /// modules make: an importer declaring one again cannot import the
    /// package.
    pub names: BTreeSet<String>,
}
