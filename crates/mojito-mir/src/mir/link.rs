//! Attaching a compiled package's MIR declarations to an importer's.
//!
//! An importer lowers only its own declarations; the declarations a package
//! compiled come from the package, so a call into it, a type it declares,
//! and a conformance it states read exactly what the package's own bodies
//! were lowered against. The importer copies none of them: its program
//! holds the package, and a lookup that misses the program's own
//! declarations resolves the name there, decoding the declaration the first
//! time, as upstream's importer resolves a declaration from the package's
//! bytecode on first lookup.

use super::text::package::PackageDeclarations;
use super::{MirDeclarations, MirFunctionDeclaration, MirStructDeclaration, StructConformance};
use mojito_types::types::is_uninit_storage_name;
use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, OnceLock};

/// A package's declarations as one importer sees them: the package's own,
/// shared by every importer in the process, and the conformance rows this
/// importer adds to the package's structs.
#[derive(Debug, Clone)]
pub struct ImportedDeclarations {
    package: Arc<PackageDeclarations>,
    /// The importer's row for each trait a package struct never asked
    /// about, by struct.
    rows: Arc<HashMap<String, Vec<StructConformance>>>,
    /// The traits the importer declares that no package module declares,
    /// which every package struct answers with no conformance.
    foreign: Arc<[String]>,
    /// Each package struct that gains a row, joined with its rows on first
    /// lookup, by package position.
    joined: Arc<[OnceLock<Option<MirStructDeclaration>>]>,
}

impl ImportedDeclarations {
    /// The function declaration `name`, decoded on first use.
    pub fn function(&self, name: &str) -> Option<&MirFunctionDeclaration> {
        self.package.function(name)
    }

    /// The struct declaration `name`, decoded on first use and joined with
    /// the importer's rows.
    pub fn structure(&self, name: &str) -> Option<&MirStructDeclaration> {
        let position = self.package.struct_position(name)?;
        self.structure_at(position)
    }

    /// The package's declarations.
    pub const fn package(&self) -> &Arc<PackageDeclarations> {
        &self.package
    }

    fn structure_at(&self, position: usize) -> Option<&MirStructDeclaration> {
        let declaration = self.package.structure_at(position)?;
        let rows = self.rows.get(&declaration.name);
        let foreign: &[String] = if is_uninit_storage_name(&declaration.name) {
            &[]
        } else {
            &self.foreign
        };
        if rows.is_none() && foreign.is_empty() {
            return Some(declaration);
        }
        self.joined[position]
            .get_or_init(|| {
                Some(joined(
                    declaration,
                    rows.map_or(&[], Vec::as_slice),
                    foreign,
                ))
            })
            .as_ref()
    }

    /// Append every package declaration `declarations` does not declare
    /// itself, in package order.
    pub(super) fn materialize_into(&self, declarations: &mut MirDeclarations) {
        let own: BTreeSet<String> = declarations
            .structs
            .iter()
            .map(|declaration| declaration.name.clone())
            .collect();
        for (position, name) in self.package.struct_names().enumerate() {
            if !own.contains(name)
                && let Some(declaration) = self.structure_at(position)
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
    }
}

impl MirDeclarations {
    /// A package's own declarations, which its bodies were lowered against.
    pub fn of_package(package: Arc<PackageDeclarations>) -> Self {
        Self {
            traits: package.traits().to_vec(),
            imported: Some(ImportedDeclarations {
                rows: Arc::default(),
                foreign: Arc::from([]),
                joined: Arc::from([]),
                package,
            }),
            ..Self::default()
        }
    }
}

/// Attach `package`'s declarations to `declarations`, an importer's own.
///
/// A package struct keeps its own conformance rows and gains the importer's
/// row for every trait the package never asked about: `rows`, the
/// importer's answers, which decide a trait only the importer's clauses
/// name, and an empty row for each of the importer's `foreign` traits,
/// which no package struct can conform to. The trait list is the union of
/// both programs', sorted.
pub fn link_declarations<S: std::hash::BuildHasher>(
    declarations: &mut MirDeclarations,
    package: &Arc<PackageDeclarations>,
    rows: &HashMap<String, Vec<StructConformance>, S>,
    foreign: &[String],
) {
    let own: BTreeSet<&str> = declarations
        .structs
        .iter()
        .map(|declaration| declaration.name.as_str())
        .collect();
    let rows: HashMap<String, Vec<StructConformance>> = rows
        .iter()
        .filter(|(name, rows)| {
            !rows.is_empty()
                && !own.contains(name.as_str())
                && package.struct_position(name).is_some()
        })
        .map(|(name, rows)| (name.clone(), rows.clone()))
        .collect();
    let traits: BTreeSet<String> = declarations
        .traits
        .iter()
        .chain(package.traits())
        .cloned()
        .collect();
    declarations.traits = traits.into_iter().collect();
    declarations.imported = Some(ImportedDeclarations {
        joined: package.struct_names().map(|_| OnceLock::new()).collect(),
        rows: Arc::new(rows),
        foreign: Arc::from(foreign),
        package: Arc::clone(package),
    });
}

/// `declaration` with a row from `rows`, or an empty row for a `foreign`
/// trait, for each trait it does not answer, its rows sorted by trait.
fn joined(
    declaration: &MirStructDeclaration,
    rows: &[StructConformance],
    foreign: &[String],
) -> MirStructDeclaration {
    let mut declaration = declaration.clone();
    let foreign = foreign.iter().map(|trait_name| StructConformance {
        trait_name: trait_name.clone(),
        conditions: Vec::new(),
    });
    for row in rows.iter().cloned().chain(foreign) {
        if !declaration
            .conformances
            .iter()
            .any(|known| known.trait_name == row.trait_name)
        {
            declaration.conformances.push(row);
        }
    }
    declaration
        .conformances
        .sort_by(|left, right| left.trait_name.cmp(&right.trait_name));
    declaration
}
