//! Callable values a body names statically: which registers hold a known
//! function or closure.
//!
//! The elaborator folds a function value held in such a register into the
//! callable binder it is passed to, and resolves a call through one of a
//! capture-free function value, as upstream folds a constant function
//! value; the omitted-argument fill reads the same dataflow to fill such a
//! call's omitted arguments. A call of a nested `def` by name is a direct
//! call and needs none of this.

use crate::mir::{
    Const, MirBlock, MirFunction, MirInstr, MirPlace, OperandSlot, Reg, call_operands,
    instruction_regions, operand_addresses,
};
use mojito_types::origin::Mutability;
use mojito_types::types::TyKind;
use std::collections::{HashMap, HashSet};

/// The function each register of `function` statically holds, beside
/// whether its value captures nothing.
///
/// It is resolved through closures, function constants, copies, and
/// variables, whether used, loaded, or addressed whole, and through a
/// reference to one. A variable names its callable
/// only while nothing writes it but its definitions (see
/// [`rebound_variables`]) and every definition holds the same callable
/// (`var g = use; g = use`). The walk descends into every region.
pub fn callable_targets(function: &MirFunction) -> HashMap<u32, (String, bool)> {
    let mut rebound = rebound_variables(function);
    loop {
        let mut registers = HashMap::new();
        let mut variables = HashMap::new();
        let mut conflicting = HashSet::new();
        while visit_callables(
            &function.body,
            &rebound,
            &mut registers,
            &mut variables,
            &mut conflicting,
        ) {}
        if conflicting.is_empty() {
            return registers;
        }
        // A register resolved through a variable before its definitions
        // disagreed is stale: resolve again with the variable rebound.
        rebound.extend(conflicting);
    }
}

/// Every instruction of `block` in order, each region op followed by its
/// regions' instructions.
pub fn nested_instructions(block: &MirBlock) -> Vec<&MirInstr> {
    let mut instructions = Vec::new();
    for instruction in &block.instrs {
        instructions.push(instruction);
        for region in instruction_regions(instruction) {
            instructions.extend(nested_instructions(region));
        }
    }
    instructions
}

/// One round of [`callable_targets`]' fixed point; returns whether it
/// resolved anything new. A variable whose definitions hold different
/// callables joins `conflicting` and resolves nothing.
fn visit_callables(
    block: &MirBlock,
    rebound: &HashSet<u32>,
    registers: &mut HashMap<u32, (String, bool)>,
    variables: &mut HashMap<u32, (String, bool)>,
    conflicting: &mut HashSet<u32>,
) -> bool {
    let mut changed = false;
    for instruction in &block.instrs {
        let resolved = match instruction {
            MirInstr::MakeClosure {
                dest,
                function,
                captures,
                ..
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
            MirInstr::LoadPlace { dest, place }
                if place.through.is_none() && place.proj.is_empty() =>
            {
                variables
                    .get(&place.root)
                    .cloned()
                    .map(|value| (dest.0, value))
            }
            // A reference to a callable's whole slot, as a `def(...)`
            // bound's call passes its callee, and the value read through it.
            MirInstr::MakeRef { dest, place }
                if place.through.is_none() && place.proj.is_empty() =>
            {
                variables
                    .get(&place.root)
                    .cloned()
                    .map(|value| (dest.0, value))
            }
            MirInstr::ReadRef { dest, reference } => registers
                .get(&reference.0)
                .cloned()
                .map(|value| (dest.0, value)),
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
            && !conflicting.contains(var)
            && let Some(value) = registers.get(&src.0).cloned()
        {
            match variables.get(var) {
                Some(existing) if *existing != value => {
                    variables.remove(var);
                    conflicting.insert(*var);
                    changed = true;
                }
                Some(_) => {}
                None => {
                    variables.insert(*var, value);
                    changed = true;
                }
            }
        }
        for region in instruction_regions(instruction) {
            changed |= visit_callables(region, rebound, registers, variables, conflicting);
        }
    }
    changed
}

/// The variables whose value is not fixed by their definitions: written
/// whole through a store, a reference, a mutable call operand's address,
/// or a callable environment, as upstream treats a mutable `!lit.ref` a
/// call takes as an escape whichever operand it is. A `read` operand's
/// immutable reference cannot rebind its referent.
fn rebound_variables(function: &MirFunction) -> HashSet<u32> {
    let instructions = nested_instructions(&function.body);
    let addresses = operand_addresses(&function.body, &function.reg_types);
    let written_references: HashSet<u32> = instructions
        .iter()
        .filter_map(|instruction| match instruction {
            MirInstr::WriteRef { reference, .. } => Some(reference.0),
            _ => None,
        })
        .collect();
    let whole = |place: &MirPlace| place.is_whole_root().then_some(place.root);
    let mut rebound = HashSet::new();
    for instruction in instructions {
        match instruction {
            MirInstr::Store { place, .. } => rebound.extend(whole(place)),
            MirInstr::MakeRef { dest, place } if written_references.contains(&dest.0) => {
                rebound.extend(whole(place));
            }
            MirInstr::Call {
                capture_accesses, ..
            }
            | MirInstr::CallIndirect {
                capture_accesses, ..
            }
            | MirInstr::MethodCall {
                capture_accesses, ..
            } => {
                rebound.extend(
                    call_operands(instruction)
                        .iter()
                        .filter(|operand| operand.slot != OperandSlot::Callee)
                        .filter(|operand| !immutable_reference(function, operand.reg))
                        .filter_map(|operand| addresses.made(operand.reg))
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

/// Whether `register` of `function` holds an immutable reference.
fn immutable_reference(function: &MirFunction, register: Reg) -> bool {
    function
        .reg_types
        .get(&register.0)
        .is_some_and(|ty| match ty.kind() {
            TyKind::Ref(reference) => reference.mutability == Mutability::Immutable,
            _ => false,
        })
}
