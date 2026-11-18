//! `dead-argument-elimination`: upstream's `DeadArgumentElimination`, which
//! its optimizing pipeline runs over the concrete functions once they are
//! inlined and folded, before its last `EliminateDeadSymbols`.
//!
//! A function no root names, and that nothing but its direct calls names,
//! loses each argument its body no longer uses
//! ([`mojito_mir::mir::unused_arguments`]) and keeps its name, and every
//! call of it passes no operand for one, the instructions that only
//! computed that operand erased
//! ([`mojito_mir::mir::without_removed_argument_forwards`]). So an
//! instance whose argument only an inlined callee read takes none once
//! that callee's body stops reading it. A function a value, a member
//! table, a declaration, or the module initializer names keeps its
//! signature. The pass repeats until no argument goes, since a caller that
//! only passed an argument on to a removed one no longer uses it.

use super::pre_elaboration::ProgramPass;
use mojito_common::timing;
use mojito_hir::hir::VarId;
use mojito_mir::mir::{
    MODULE_INITIALIZER, MirBlock, MirProgram, SymbolTable, direct_call_target, entry_roots,
    instruction_regions, unused_arguments, without_removed_argument_forwards,
    without_removed_arguments,
};
use std::collections::{HashMap, HashSet};

/// The pass, under upstream's name.
pub const DEAD_ARGUMENT_ELIMINATION: ProgramPass = ProgramPass {
    name: "dead-argument-elimination",
    run: eliminate_dead_arguments,
};

/// Remove the arguments its body does not use from each function of the
/// concrete `program` that only direct calls name.
///
/// No entry root and no symbol in `exports` loses one, and every call of
/// a reduced function passes no operand for them.
///
/// A method keeps its receiver, which a method call, an operator, and a
/// subscript pass as their own operand rather than as an argument.
pub fn eliminate_dead_arguments(program: &mut MirProgram, exports: &[String]) {
    let candidates = only_called(program, exports);
    let mut removed_count = 0;
    while !candidates.is_empty() {
        let removals: HashMap<String, Vec<VarId>> = program
            .functions
            .iter()
            .filter(|(name, _)| candidates.contains(name))
            .filter_map(|(name, function)| {
                let receiver = program
                    .declarations
                    .function(name)
                    .is_some_and(|declaration| declaration.has_receiver);
                let slots: Vec<VarId> = unused_arguments(name, function, &program.declarations)
                    .into_iter()
                    .filter(|&slot| !(receiver && slot == 0))
                    .collect();
                (!slots.is_empty()).then(|| (name.clone(), slots))
            })
            .collect();
        if removals.is_empty() {
            break;
        }
        // Each call binds its operands against the callee's declaration as
        // it stands, so the callers shed theirs before the callees change.
        let removed = |callee: &str| removals.get(callee).map(Vec::as_slice);
        for (_, function) in &mut program.functions {
            if let Some((view, _)) =
                without_removed_argument_forwards(function, &program.declarations, &removed)
            {
                *function = view;
            }
        }
        for (name, function) in &mut program.functions {
            let Some(slots) = removals.get(name) else {
                continue;
            };
            let Some(declaration) = program
                .declarations
                .functions
                .iter_mut()
                .find(|declaration| declaration.lowered_name == *name)
            else {
                continue;
            };
            let (body, reduced) = without_removed_arguments(function, declaration, slots);
            *function = body;
            *declaration = reduced;
            removed_count += slots.len();
        }
    }
    timing::count("dead_arguments", removed_count as u64);
}

/// The functions of `program` with an own declaration that no root is and
/// whose every use is a direct call outside the module initializer.
fn only_called(program: &MirProgram, exports: &[String]) -> HashSet<String> {
    let declared: HashSet<&str> = program
        .declarations
        .functions
        .iter()
        .map(|declaration| declaration.lowered_name.as_str())
        .collect();
    let mut table = SymbolTable::new(
        program
            .functions
            .iter()
            .map(|(name, _)| name.as_str())
            .filter(|name| declared.contains(name)),
    );
    let mut named = Vec::new();
    for (_, function) in &program.functions {
        named.extend(table.function_uses(function));
    }
    for declaration in &program.declarations.functions {
        named.extend(table.uses(declaration));
    }
    for declaration in &program.declarations.structs {
        named.extend(table.struct_uses(declaration));
    }
    named.extend(table.uses(&program.declarations.traits));
    let mut uses: HashMap<&str, usize> = HashMap::new();
    for name in named {
        *uses.entry(name).or_default() += 1;
    }
    let mut calls: HashMap<&str, usize> = HashMap::new();
    for (name, function) in &program.functions {
        if name != MODULE_INITIALIZER {
            count_direct_calls(&function.body, &mut calls);
        }
    }
    let roots: HashSet<String> = entry_roots(program)
        .into_iter()
        .chain(exports.iter().cloned())
        .collect();
    uses.into_iter()
        .filter(|&(name, used)| !roots.contains(name) && calls.get(name) == Some(&used))
        .map(|(name, _)| name.to_string())
        .collect()
}

/// Count into `calls` each direct call in `block` and its regions, by
/// callee ([`direct_call_target`]).
fn count_direct_calls<'p>(block: &'p MirBlock, calls: &mut HashMap<&'p str, usize>) {
    for instruction in &block.instrs {
        if let Some(target) = direct_call_target(instruction) {
            *calls.entry(target).or_default() += 1;
        }
        for region in instruction_regions(instruction) {
            count_direct_calls(region, calls);
        }
    }
}
