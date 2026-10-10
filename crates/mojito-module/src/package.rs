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
use std::path::Path;

/// The records a package build's link made, each beside the canonical path
/// of its module's source file.
pub type ModuleRecords = Vec<(String, PackageModule)>;

/// A package's module records, by the canonical path of each module's
/// source file.
#[derive(Debug, Default)]
pub struct PackageModules {
    modules: HashMap<String, Vec<u8>>,
}

impl PackageModules {
    /// The modules whose encoded records `sections` holds, each beside the
    /// canonical path of its source file.
    pub fn new(sections: impl IntoIterator<Item = (String, Vec<u8>)>) -> Self {
        Self {
            modules: sections.into_iter().collect(),
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

    /// The record of the module at `canonical`, decoded, when the package
    /// holds one under the dotted name `name`; `None` otherwise, or when
    /// the record does not decode, and the module links from source.
    pub(crate) fn module(&self, canonical: &Path, name: &str) -> Option<PackageModule> {
        let bytes = self.modules.get(canonical.to_str()?)?;
        let _decode = mojito_common::timing::span("package.module");
        mojito_common::timing::count("package_modules", 1);
        PackageModule::decode(bytes).filter(|record| record.name == name)
    }
}

/// What linking one module from source produced.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageModule {
    /// The module's dotted name (`std.io.file`), which its linked names
    /// spell.
    pub(crate) name: String,
    /// Every module its link asked to load, whether loaded already or not,
    /// in order: the path it was asked by, and the dotted name it asked
    /// for.
    pub(crate) loads: Vec<(String, String)>,
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
            loads: vec![("/lib/std/other.mojo".to_string(), "std.other".to_string())],
            exports: BTreeMap::from([("f".to_string(), "__module$std$probe$f".to_string())]),
            namespaces: BTreeMap::new(),
            declarations: mojito_parser::parser::Parser::new(mojito_lexer::lexer::Lexer::new(
                "def f(x: Int = 1) -> Int:\n    return x\n",
            ))
            .parse_program()
            .expect("parses"),
        };
        let bytes = record.encode().expect("encodes");
        let package = PackageModules::new([
            ("/lib/std/probe.mojo".to_string(), bytes.clone()),
            (
                "/lib/std/torn.mojo".to_string(),
                bytes[..bytes.len() / 2].to_vec(),
            ),
        ]);
        let read = package
            .module(Path::new("/lib/std/probe.mojo"), "std.probe")
            .expect("decodes");
        assert_eq!(read.declarations, record.declarations);
        assert_eq!(read.loads, record.loads);
        assert_eq!(read.exports, record.exports);
        assert!(
            package
                .module(Path::new("/lib/std/probe.mojo"), "probe")
                .is_none()
        );
        assert!(
            package
                .module(Path::new("/lib/std/torn.mojo"), "std.probe")
                .is_none()
        );
    }
}
