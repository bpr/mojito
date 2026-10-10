//! Linking a compiled package's MIR declarations into an importer's.
//!
//! An importer lowers only its own declarations; the declarations a package
//! compiled come from the package, so a call into it, a type it declares,
//! and a conformance it states read exactly what the package's own bodies
//! were lowered against.

use super::{MirDeclarations, StructConformance};
use std::collections::{BTreeSet, HashMap};

/// Join `package`'s declarations to `declarations`, an importer's own.
///
/// A package struct keeps its own conformance rows and gains the importer's
/// row for every trait the package never asked about (`rows`, the
/// importer's answers, which decide a trait only the importer's clauses
/// name). The trait list is the union of both, sorted.
pub fn link_declarations<S: std::hash::BuildHasher>(
    declarations: &mut MirDeclarations,
    package: &MirDeclarations,
    rows: &HashMap<String, Vec<StructConformance>, S>,
) {
    let own: BTreeSet<String> = declarations
        .structs
        .iter()
        .map(|declaration| declaration.name.clone())
        .collect();
    for declaration in &package.structs {
        if own.contains(&declaration.name) {
            continue;
        }
        let mut declaration = declaration.clone();
        for row in rows.get(&declaration.name).into_iter().flatten() {
            if !declaration
                .conformances
                .iter()
                .any(|known| known.trait_name == row.trait_name)
            {
                declaration.conformances.push(row.clone());
            }
        }
        declaration
            .conformances
            .sort_by(|left, right| left.trait_name.cmp(&right.trait_name));
        declarations.structs.push(declaration);
    }
    let lowered: BTreeSet<String> = declarations
        .functions
        .iter()
        .map(|declaration| declaration.lowered_name.clone())
        .collect();
    declarations.functions.extend(
        package
            .functions
            .iter()
            .filter(|declaration| !lowered.contains(&declaration.lowered_name))
            .cloned(),
    );
    let traits: BTreeSet<String> = declarations
        .traits
        .iter()
        .chain(&package.traits)
        .cloned()
        .collect();
    declarations.traits = traits.into_iter().collect();
}
