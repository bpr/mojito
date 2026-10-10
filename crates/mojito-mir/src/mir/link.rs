//! Attaching a compiled package's MIR declarations to an importer's.
//!
//! An importer lowers only its own declarations; the declarations a package
//! compiled come from the package, so a call into it, a type it declares,
//! and a conformance it states read exactly what the package's own bodies
//! were lowered against. The importer copies none of them and adds nothing
//! to them: its program holds the package, and a lookup that misses the
//! program's own declarations resolves the name there, decoding the
//! declaration the first time, as upstream's importer resolves a
//! declaration from the package's bytecode on first lookup. A package
//! struct's conformance to a trait the importer asks is decided from the
//! struct's declared conformances when the clause asks it.

use super::text::package::PackageDeclarations;
use super::{MirDeclarations, MirFunctionDeclaration, MirStructDeclaration};
use std::collections::BTreeSet;
use std::sync::Arc;

/// A package's declarations as one importer sees them: the package's own,
/// shared by every importer in the process.
#[derive(Debug, Clone)]
pub struct ImportedDeclarations {
    package: Arc<PackageDeclarations>,
}

impl ImportedDeclarations {
    /// The function declaration `name`, decoded on first use.
    pub fn function(&self, name: &str) -> Option<&MirFunctionDeclaration> {
        self.package.function(name)
    }

    /// The struct declaration `name`, decoded on first use.
    pub fn structure(&self, name: &str) -> Option<&MirStructDeclaration> {
        self.package.structure(name)
    }

    /// The package's declarations.
    pub const fn package(&self) -> &Arc<PackageDeclarations> {
        &self.package
    }

    /// Append every package declaration `declarations` does not declare
    /// itself, in package order, its traits sorted by name.
    pub(super) fn materialize_into(&self, declarations: &mut MirDeclarations) {
        let own: BTreeSet<String> = declarations
            .structs
            .iter()
            .map(|declaration| declaration.name.clone())
            .collect();
        for (position, name) in self.package.struct_names().enumerate() {
            if !own.contains(name)
                && let Some(declaration) = self.package.structure_at(position)
            {
                declarations.structs.push(declaration.clone());
            }
        }
        let lowered: BTreeSet<String> = declarations
            .functions
            .iter()
            .map(|declaration| declaration.lowered_name.clone())
            .collect();
        for (position, name) in self.package.function_names().enumerate() {
            if !lowered.contains(name)
                && let Some(declaration) = self.package.function_at(position)
            {
                declarations.functions.push(declaration.clone());
            }
        }
        for name in self.package.traits() {
            if let Err(position) = declarations
                .traits
                .binary_search_by(|declaration| declaration.name.as_str().cmp(name))
                && let Some(declaration) = self.package.trait_declaration(name)
            {
                declarations.traits.insert(position, declaration.clone());
            }
        }
    }
}

impl MirDeclarations {
    /// A package's own declarations, which its bodies were lowered against.
    pub fn of_package(package: Arc<PackageDeclarations>) -> Self {
        Self {
            imported: Some(ImportedDeclarations { package }),
            ..Self::default()
        }
    }
}

/// Attach `package`'s declarations to `declarations`, an importer's own.
pub fn link_declarations(declarations: &mut MirDeclarations, package: &Arc<PackageDeclarations>) {
    declarations.imported = Some(ImportedDeclarations {
        package: Arc::clone(package),
    });
}
