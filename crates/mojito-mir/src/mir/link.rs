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
//! struct's declared conformances when the clause asks it. A record a
//! lookup reaches that does not decode is a miss the view notes, and the
//! program verifier reports it against the program that reached it.

use super::text::package::PackageDeclarations;
use super::{MirDeclarations, MirFunctionDeclaration, MirStructDeclaration};
use mojito_checked::declaration::TraitDeclaration;
use mojito_checked::package::ReachedFailures;
use std::collections::BTreeSet;
use std::sync::Arc;

/// A package's declarations as one importer sees them: the package's own,
/// shared by every importer in the process, and the records this importer's
/// lookups reached that do not decode, its own.
#[derive(Debug, Clone)]
pub struct ImportedDeclarations {
    package: Arc<PackageDeclarations>,
    reached: ReachedFailures,
}

impl ImportedDeclarations {
    /// The function declaration `name`, decoded on first use.
    pub fn function(&self, name: &str) -> Option<&MirFunctionDeclaration> {
        let declaration = self.package.function(name);
        if declaration.is_none() {
            self.reached
                .note(&self.package, self.package.function_entry(name));
        }
        declaration
    }

    /// The struct declaration `name`, decoded on first use.
    pub fn structure(&self, name: &str) -> Option<&MirStructDeclaration> {
        let declaration = self.package.structure(name);
        if declaration.is_none() {
            self.reached
                .note(&self.package, self.package.struct_entry(name));
        }
        declaration
    }

    /// The trait declaration `name`, decoded on first use.
    pub fn trait_declaration(&self, name: &str) -> Option<&TraitDeclaration> {
        let declaration = self.package.trait_declaration(name);
        if declaration.is_none() {
            self.reached
                .note(&self.package, self.package.trait_entry(name));
        }
        declaration
    }

    /// The package's declarations.
    pub const fn package(&self) -> &Arc<PackageDeclarations> {
        &self.package
    }

    /// Why each record this view's lookups reached does not decode, in
    /// package order.
    pub fn failures(&self) -> Vec<String> {
        self.reached.messages(&self.package)
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
            if !own.contains(name) {
                match self.package.structure_at(position) {
                    Some(declaration) => declarations.structs.push(declaration.clone()),
                    None => self
                        .reached
                        .note(&self.package, self.package.struct_entry(name)),
                }
            }
        }
        let lowered: BTreeSet<String> = declarations
            .functions
            .iter()
            .map(|declaration| declaration.lowered_name.clone())
            .collect();
        for (position, name) in self.package.function_names().enumerate() {
            if !lowered.contains(name) {
                match self.package.function_at(position) {
                    Some(declaration) => declarations.functions.push(declaration.clone()),
                    None => self
                        .reached
                        .note(&self.package, self.package.function_entry(name)),
                }
            }
        }
        for name in self.package.traits() {
            if let Err(position) = declarations
                .traits
                .binary_search_by(|declaration| declaration.name.as_str().cmp(name))
                && let Some(declaration) = self.trait_declaration(name)
            {
                declarations.traits.insert(position, declaration.clone());
            }
        }
    }
}

impl MirDeclarations {
    /// A package's own declarations, which its bodies were lowered against.
    pub fn of_package(package: Arc<PackageDeclarations>) -> Self {
        Self::of_package_noting(package, ReachedFailures::default())
    }

    /// A package's own declarations, each record a lookup reaches that does
    /// not decode noted in `reached`.
    pub fn of_package_noting(package: Arc<PackageDeclarations>, reached: ReachedFailures) -> Self {
        Self {
            imported: Some(ImportedDeclarations { package, reached }),
            ..Self::default()
        }
    }
}

/// Attach `package`'s declarations to `declarations`, an importer's own.
pub fn link_declarations(declarations: &mut MirDeclarations, package: &Arc<PackageDeclarations>) {
    declarations.imported = Some(ImportedDeclarations {
        package: Arc::clone(package),
        reached: ReachedFailures::default(),
    });
}
