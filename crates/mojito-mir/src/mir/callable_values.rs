//! Callable values a body names statically: which registers hold a known
//! function or closure.
//!
//! The elaborator folds a function value held in such a register into the
//! callable binder it is passed to, and resolves a call through one of a
//! capture-free function value, as upstream folds a constant function
//! value; the defaults filler reads the same dataflow to fill such a call's
//! omitted defaults. A call of a nested `def` by name is a direct call and
//! needs none of this.

use crate::mir::{Const, MirBlock, MirFunction, MirInstr, MirPlace};
use std::collections::{HashMap, HashSet};

/// The function each register of `function` statically holds, beside
/// whether its value captures nothing.
///
/// It is resolved through closures, function constants, copies, and
/// variables. A variable names its callable only while one definition is
/// its sole write (see [`rebound_variables`]). The walk descends into `try`
/// regions only.
pub fn callable_targets(function: &MirFunction) -> HashMap<u32, (String, bool)> {
    let rebound = rebound_variables(function);
    let mut registers = HashMap::new();
    let mut variables = HashMap::new();
    while visit_callables(&function.blocks, &rebound, &mut registers, &mut variables) {}
    registers
}

/// Every instruction of `blocks` in order, descending into `try` regions.
pub fn nested_instructions(blocks: &[MirBlock]) -> Vec<&MirInstr> {
    let mut instructions = Vec::new();
    for instruction in blocks.iter().flat_map(|block| &block.instrs) {
        instructions.push(instruction);
        if let MirInstr::Try {
            body,
            handler,
            orelse,
            finalbody,
            ..
        } = instruction
        {
            for blocks in [Some(body), handler.as_ref().map(|(_, blocks)| blocks)]
                .into_iter()
                .chain([orelse.as_ref(), finalbody.as_ref()])
                .flatten()
            {
                instructions.extend(nested_instructions(blocks));
            }
        }
    }
    instructions
}

/// One round of [`callable_targets`]' fixed point; returns whether it
/// resolved anything new.
fn visit_callables(
    blocks: &[MirBlock],
    rebound: &HashSet<u32>,
    registers: &mut HashMap<u32, (String, bool)>,
    variables: &mut HashMap<u32, (String, bool)>,
) -> bool {
    let mut changed = false;
    for block in blocks {
        for instruction in &block.instrs {
            let resolved = match instruction {
                MirInstr::MakeClosure {
                    dest,
                    function,
                    captures,
                } => Some((dest.0, (function.clone(), captures.is_empty()))),
                MirInstr::Const {
                    dest,
                    k: Const::Function(function),
                } => Some((dest.0, (function.clone(), true))),
                MirInstr::CopyValue { dest, value, .. } => registers
                    .get(&value.0)
                    .cloned()
                    .map(|value| (dest.0, value)),
                MirInstr::UseVar { dest, var, .. } => {
                    variables.get(var).cloned().map(|value| (dest.0, value))
                }
                _ => None,
            };
            if let Some((dest, value)) = resolved
                && registers.get(&dest) != Some(&value)
            {
                registers.insert(dest, value);
                changed = true;
            }
            if let MirInstr::DefVar { var, src, .. } = instruction
                && !rebound.contains(var)
                && let Some(value) = registers.get(&src.0).cloned()
                && variables.get(var) != Some(&value)
            {
                variables.insert(*var, value);
                changed = true;
            }
            if let MirInstr::Try {
                body,
                handler,
                orelse,
                finalbody,
                ..
            } = instruction
            {
                changed |= visit_callables(body, rebound, registers, variables);
                if let Some((_, blocks)) = handler {
                    changed |= visit_callables(blocks, rebound, registers, variables);
                }
                if let Some(blocks) = orelse {
                    changed |= visit_callables(blocks, rebound, registers, variables);
                }
                if let Some(blocks) = finalbody {
                    changed |= visit_callables(blocks, rebound, registers, variables);
                }
            }
        }
    }
    changed
}

/// The variables whose value is not fixed by a single definition: defined
/// more than once, or written whole through a store, a reference, a
/// `mut`/`ref` argument place, or a callable environment.
fn rebound_variables(function: &MirFunction) -> HashSet<u32> {
    let instructions = nested_instructions(&function.blocks);
    let written_references: HashSet<u32> = instructions
        .iter()
        .filter_map(|instruction| match instruction {
            MirInstr::WriteRef { reference, .. } => Some(reference.0),
            _ => None,
        })
        .collect();
    let whole = |place: &MirPlace| place.is_whole_root().then_some(place.root);
    let mut defined = HashSet::new();
    let mut rebound = HashSet::new();
    for instruction in instructions {
        match instruction {
            MirInstr::DefVar { var, .. } => {
                if !defined.insert(*var) {
                    rebound.insert(*var);
                }
            }
            MirInstr::Store { place, .. } => rebound.extend(whole(place)),
            MirInstr::MakeRef { dest, place } if written_references.contains(&dest.0) => {
                rebound.extend(whole(place));
            }
            MirInstr::Call {
                arg_places,
                kwarg_places,
                capture_accesses,
                ..
            }
            | MirInstr::CallIndirect {
                arg_places,
                kwarg_places,
                capture_accesses,
                ..
            }
            | MirInstr::MethodCall {
                arg_places,
                kwarg_places,
                capture_accesses,
                ..
            } => {
                rebound.extend(
                    arg_places
                        .iter()
                        .chain(kwarg_places)
                        .flatten()
                        .filter_map(whole),
                );
                rebound.extend(
                    capture_accesses
                        .iter()
                        .filter(|access| {
                            access.path.is_empty()
                                && access.access == mojito_types::origin::CaptureAccess::Write
                        })
                        .map(|access| access.root),
                );
            }
            _ => {}
        }
    }
    rebound
}
