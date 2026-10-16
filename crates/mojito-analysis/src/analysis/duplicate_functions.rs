//! `eliminate-duplicate-functions`: upstream's `EliminateDuplicateFunctions`,
//! which its elaborator's pipeline runs straight after `ElaborateGenerators`
//! and before `EliminateDeadSymbols`, over a concrete program.
//!
//! Instances of one generator often come out identical: `foo[1]` and
//! `foo[2]` when a `comptime if` picks the same arm, or when the parameter
//! only reaches a callee whose instances merged. Each set of functions
//! whose bodies, declarations, and source locations match, once each spells
//! its own name alike, keeps its first member, and every reference to the
//! others names it instead. Merging a set rewrites its callers, which may
//! make them match in turn, so the pass repeats until no set merges.

use super::pre_elaboration::ProgramPass;
use mojito_common::timing;
use mojito_mir::mir::text::package::{FunctionIdentity, function_identity};
use mojito_mir::mir::{MirFunction, MirProgram, SymbolTable, entry_roots, replace_symbol_uses};
use mojito_symbol::symbol::split_method_symbol;
use std::collections::{HashMap, HashSet};

/// The pass, under upstream's flag spelling.
pub const ELIMINATE_DUPLICATE_FUNCTIONS: ProgramPass = ProgramPass {
    name: "eliminate-duplicate-functions",
    run: eliminate_duplicate_functions,
};

/// Merge each set of `program`'s functions that are identical up to their
/// names into its first member, keeping the rest in order.
///
/// A function stays, whatever its duplicates, when something reaches it by
/// name other than a reference [`replace_symbol_uses`] rewrites: an entry
/// root, one of `exports`, a struct's method the backends reach from its
/// type (its special methods and its traits' requirements,
/// [`mojito_mir::mir::MirDeclarations::reached_from_type`]), a body the
/// instructions call implicitly, or a symbol a type, a compile-time value,
/// or a declaration names. Such a function can still absorb its
/// duplicates.
pub fn eliminate_duplicate_functions(program: &mut MirProgram, exports: &[String]) {
    let pinned = pinned(program, exports);
    let mut identities: HashMap<String, Option<FunctionIdentity>> = HashMap::new();
    let mut merged = 0;
    let mut instructions = 0;
    loop {
        for (name, function) in &program.functions {
            if !identities.contains_key(name) {
                let identity = identity(program, name, function);
                identities.insert(name.clone(), identity);
            }
        }
        let replaced = duplicates(program, &pinned, &identities);
        if replaced.is_empty() {
            break;
        }
        program.functions.retain(|(name, function)| {
            let keep = !replaced.contains_key(name);
            if !keep {
                merged += 1;
                instructions += function.instruction_count();
                identities.remove(name);
            }
            keep
        });
        program
            .declarations
            .functions
            .retain(|declaration| !replaced.contains_key(&declaration.lowered_name));
        for (name, function) in &mut program.functions {
            let mut changed = false;
            replace_symbol_uses(&mut function.blocks, &mut |symbol| {
                if let Some(survivor) = replaced.get(symbol.as_str()) {
                    symbol.clone_from(survivor);
                    changed = true;
                }
            });
            if changed {
                identities.remove(name);
            }
        }
    }
    timing::count("duplicate_functions", merged);
    timing::count("duplicate_instructions", instructions as u64);
}

/// The functions `program` must keep under their own names: those
/// something reaches other than through a reference
/// [`replace_symbol_uses`] rewrites.
///
/// The references it rewrites are counted against every use
/// [`SymbolTable`] finds, in the bodies (implicit callees included) and in
/// the declarations; a function with any use the rewrite does not reach is
/// kept.
fn pinned(program: &mut MirProgram, exports: &[String]) -> HashSet<String> {
    let rewritten = rewritten_uses(program);
    let mut table = SymbolTable::new(program.functions.iter().map(|(name, _)| name.as_str()));
    let mut uses: HashMap<&str, usize> = HashMap::new();
    let declarations = &program.declarations;
    for (_, function) in &program.functions {
        for name in table.function_uses(function) {
            *uses.entry(name).or_default() += 1;
        }
    }
    let declared = declarations
        .functions
        .iter()
        .map(|declaration| table.uses(declaration))
        .chain(
            declarations
                .structs
                .iter()
                .map(|declaration| table.uses(declaration)),
        )
        .chain(
            declarations
                .traits
                .iter()
                .map(|declaration| table.uses(declaration)),
        );
    for names in declared {
        for name in names {
            *uses.entry(name).or_default() += 1;
        }
    }
    // A struct's member table names its lifecycle members, which the
    // backends run from a value's type.
    let members: HashSet<&str> = declarations
        .structs
        .iter()
        .flat_map(|declaration| &declaration.members)
        .map(|entry| entry.symbol.as_str())
        .collect();
    let method = |name: &str| {
        members.contains(name)
            || split_method_symbol(name)
                .is_some_and(|(owner, method)| declarations.reached_from_type(owner, method))
    };
    let unreached = |name: &str| {
        uses.get(name).copied().unwrap_or_default()
            != rewritten.get(name).copied().unwrap_or_default()
    };
    entry_roots(program)
        .into_iter()
        .chain(exports.iter().cloned())
        .chain(
            program
                .functions
                .iter()
                .map(|(name, _)| name.as_str())
                .filter(|name| method(name) || unreached(name))
                .map(str::to_string),
        )
        .collect()
}

/// Each function a function of `program` identical to it stands for, from
/// `identities`: in each set of identical functions, its first pinned
/// member, else its first, stands for the unpinned others.
fn duplicates(
    program: &MirProgram,
    pinned: &HashSet<String>,
    identities: &HashMap<String, Option<FunctionIdentity>>,
) -> HashMap<String, String> {
    let identified = |keep: bool| {
        program
            .functions
            .iter()
            .filter(move |(name, _)| pinned.contains(name) == keep)
            .filter_map(|(name, _)| Some((name, identities.get(name)?.as_ref()?)))
    };
    let mut survivors: HashMap<&FunctionIdentity, &String> = HashMap::new();
    for (name, identity) in identified(true) {
        survivors.entry(identity).or_insert(name);
    }
    let mut replaced = HashMap::new();
    for (name, identity) in identified(false) {
        let survivor = *survivors.entry(identity).or_insert(name);
        if survivor != name {
            replaced.insert(name.clone(), survivor.clone());
        }
    }
    replaced
}

/// The identity of `program`'s function `name`, with its own name spelled
/// as no symbol is, so a recursive function matches its duplicate.
fn identity(program: &MirProgram, name: &str, function: &MirFunction) -> Option<FunctionIdentity> {
    let mut function = function.clone();
    replace_symbol_uses(&mut function.blocks, &mut |symbol| {
        if symbol == name {
            symbol.clear();
        }
    });
    let declaration = program.declarations.function(name);
    function_identity(&function, declaration)
}

/// How many references to each symbol [`replace_symbol_uses`] reaches in
/// `program`'s bodies, which it leaves as they are.
fn rewritten_uses(program: &mut MirProgram) -> HashMap<String, usize> {
    let mut rewritten: HashMap<String, usize> = HashMap::new();
    for (_, function) in &mut program.functions {
        replace_symbol_uses(&mut function.blocks, &mut |name| {
            *rewritten.entry(name.clone()).or_default() += 1;
        });
    }
    rewritten
}
