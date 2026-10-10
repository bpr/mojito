//! A compiled package's modules, as its importer's link reads them.
//!
//! Upstream's `createBinaryPackageState` reads a package's module tree and
//! registers each module's declarations from the package, never reading
//! the library's source. A package records, for every module it compiled,
//! what linking that module from source would have produced: the modules
//! its link loaded, in order, its export tables, and its declarations as
//! the package's build prepared them, whose bodies are in the package
//! already. An importer's
//! link replays the record in place of reading, parsing, and resolving the
//! module.
//!
//! A record is encoded as binary bytecode (`postcard` over the AST's serde
//! form), one per module, and decoded only when a link reaches its module.

use mojito_ast::ast::Stmt;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

/// The records a package build's link made, each beside its module's
/// identity.
pub type ModuleRecords = Vec<(ModuleIdentity, PackageModule)>;

/// Which module a record is: its dotted name, by which the package's
/// module table finds it, and the source file it was compiled from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleIdentity {
    /// The module's dotted name (`std.io.file`).
    pub name: String,
    /// The canonical path of its source file, its identity in a link.
    pub canonical: String,
    /// The path its declarations' locations name.
    pub path: String,
}

/// A package's module table: each module's encoded record, by the
/// module's dotted name, and the library root the package was built from.
#[derive(Debug, Default)]
pub struct PackageModules {
    root: PathBuf,
    modules: HashMap<String, PackagedModule>,
}

impl PackageModules {
    /// The modules of the library under `root` whose encoded records
    /// `sections` holds, each beside its identity.
    pub fn new(
        root: impl Into<PathBuf>,
        sections: impl IntoIterator<Item = (ModuleIdentity, Vec<u8>)>,
    ) -> Self {
        Self {
            root: root.into(),
            modules: sections
                .into_iter()
                .map(|(identity, bytes)| {
                    let module = PackagedModule {
                        canonical: identity.canonical.into(),
                        path: identity.path.into(),
                        bytes,
                    };
                    (identity.name, module)
                })
                .collect(),
        }
    }

    /// How many modules the package records.
    pub fn len(&self) -> usize {
        self.modules.len()
    }

    /// Whether the package records no module.
    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }

    /// The library root the package's modules were compiled under.
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// The module the package records under the dotted name `name`, if
    /// any, reading no record.
    pub(crate) fn entry(&self, name: &str) -> Option<&PackagedModule> {
        self.modules.get(name)
    }

    /// The record of the module `name`, decoded, when the package holds one
    /// for the source file `canonical`; `None` otherwise, or when the record
    /// does not decode, and the module links from source.
    pub(crate) fn module(&self, name: &str, canonical: &Path) -> Option<PackageModule> {
        let module = self
            .entry(name)
            .filter(|module| module.canonical == canonical)?;
        let _decode = mojito_common::timing::span("package.module");
        mojito_common::timing::count("package_modules", 1);
        PackageModule::decode(&module.bytes).filter(|record| record.name == name)
    }
}

/// One module of a package's table.
#[derive(Debug)]
pub(crate) struct PackagedModule {
    /// The canonical path of its source file.
    pub(crate) canonical: PathBuf,
    /// The path its declarations' locations name.
    pub(crate) path: PathBuf,
    bytes: Vec<u8>,
}

/// What linking one module from source produced.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageModule {
    /// The module's dotted name (`std.io.file`), which its linked names
    /// spell.
    pub(crate) name: String,
    /// The dotted name of every module its link asked to load, whether
    /// loaded already or not, in order.
    pub(crate) loads: Vec<String>,
    /// The declarations it exposes by local name, and the linked name each
    /// binds.
    pub(crate) exports: BTreeMap<String, String>,
    /// The namespaces it exposes, by relative qualified path, and the
    /// declarations visible through each.
    pub(crate) namespaces: BTreeMap<String, BTreeMap<String, String>>,
    /// Its declarations, as the package build's `prepare` left them, in
    /// order, bodies emptied ([`mojito_ast::ast::clear_bodies`]). The link
    /// records them as linked; the build replaces them
    /// ([`Self::replace_declarations`]).
    pub(crate) declarations: Vec<Stmt>,
}

impl PackageModule {
    /// Record `declarations`, the module's declarations as the package
    /// build prepared them, in place of its linked ones; their bodies are
    /// emptied.
    pub fn replace_declarations(&mut self, mut declarations: Vec<Stmt>) {
        declarations
            .iter_mut()
            .for_each(mojito_ast::ast::clear_bodies);
        self.declarations = declarations;
    }

    /// This record as bytecode.
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        postcard::to_stdvec(self).map_err(|error| format!("module `{}`: {error}", self.name))
    }

    fn decode(bytes: &[u8]) -> Option<Self> {
        postcard::from_bytes(bytes).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_reads_back_only_whole_and_under_its_name() {
        let record = PackageModule {
            name: "std.probe".to_string(),
            loads: vec!["std.other".to_string()],
            exports: BTreeMap::from([("f".to_string(), "__module$std$probe$f".to_string())]),
            namespaces: BTreeMap::new(),
            declarations: mojito_parser::parser::Parser::new(mojito_lexer::lexer::Lexer::new(
                "def f(x: Int = 1) -> Int:\n    return x\n",
            ))
            .parse_program()
            .expect("parses"),
        };
        let bytes = record.encode().expect("encodes");
        let identity = |name: &str, file: &str| ModuleIdentity {
            name: name.to_string(),
            canonical: format!("/lib/std/{file}"),
            path: format!("/lib/./std/{file}"),
        };
        let package = PackageModules::new(
            "/lib",
            [
                (identity("std.probe", "probe.mojo"), bytes.clone()),
                (
                    identity("std.torn", "torn.mojo"),
                    bytes[..bytes.len() / 2].to_vec(),
                ),
                (identity("probe", "probe.mojo"), bytes.clone()),
            ],
        );
        let canonical = Path::new("/lib/std/probe.mojo");
        let read = package.module("std.probe", canonical).expect("decodes");
        assert_eq!(read.declarations, record.declarations);
        assert_eq!(read.loads, record.loads);
        assert_eq!(read.exports, record.exports);
        assert_eq!(
            package
                .entry("std.probe")
                .map(|module| module.path.as_path()),
            Some(Path::new("/lib/./std/probe.mojo"))
        );
        assert!(package.module("probe", canonical).is_none());
        assert!(
            package
                .module("std.probe", Path::new("/elsewhere/std/probe.mojo"))
                .is_none()
        );
        assert!(
            package
                .module("std.torn", Path::new("/lib/std/torn.mojo"))
                .is_none()
        );
    }
}
