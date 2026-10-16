//! Callable values a body names statically: which registers hold a known
//! function or closure, and which nested bodies a body only ever calls.
//!
//! Upstream lifts a nested `def` to a closure generator and calls it
//! directly, passing its captures as arguments. Mojito lowers a call of one
//! to `closure.make` and `call.indirect`, and the elaborator recovers the
//! direct call by this dataflow; `remove-unused-params` reads the same
//! dataflow to know which uses of a nested body take the generator with its
//! unused arguments removed.

use crate::mir::verify::instruction_operand_regs;
use crate::mir::{
    Const, MirBlock, MirCaptureMode, MirFunction, MirInstr, MirPlace, instruction_regions,
    instruction_slots, terminator_regs_mut,
};
use std::collections::{BTreeSet, HashMap, HashSet};

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

/// The nested bodies `function` names by a closure or a function value that
/// some use other than a call reaches, so they escape the direct call the
/// elaborator rewrites each call of them to.
///
/// A body stays direct while each register [`callable_targets`] maps to it
/// is read only as a `call.indirect`'s callee, a copy's source, or the
/// value stored into a variable that is not rebound; each variable holding
/// it only by a `var.use` whose result is so mapped, a `drop.var`, or as
/// the root of a `call.indirect`'s callee place; and each of its closures
/// captures by reference alone, the environment a direct call passes as
/// leading reference arguments. Any other read escapes it, as does a
/// closure or a value the target walk does not reach, in a region it skips. `nested(name)` answers whether `name` is a nested
/// body.
pub fn escaping_nested_bodies(
    function: &MirFunction,
    nested: &dyn Fn(&str) -> bool,
) -> BTreeSet<String> {
    let registers = callable_targets(function);
    let rebound = rebound_variables(function);
    let mut variables: HashMap<u32, &str> = HashMap::new();
    let mut escaping = BTreeSet::new();
    holding_variables(&function.blocks, &registers, &mut variables);
    let mut reads = Escapes {
        registers: &registers,
        variables: &variables,
        rebound: &rebound,
        escaping: &mut escaping,
    };
    reads.blocks(&function.blocks);
    escaping.retain(|body| nested(body));
    escaping
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
                MirInstr::CopyValue { dest, value } => registers
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

/// Add to `variables` each variable of `blocks` and their regions that a
/// `var.store` of a register `registers` maps fills, with its body.
fn holding_variables<'a>(
    blocks: &[MirBlock],
    registers: &'a HashMap<u32, (String, bool)>,
    variables: &mut HashMap<u32, &'a str>,
) {
    for instruction in blocks.iter().flat_map(|block| &block.instrs) {
        if let MirInstr::DefVar { var, src, .. } = instruction
            && let Some((body, _)) = registers.get(&src.0)
        {
            variables.insert(*var, body);
        }
        for region in instruction_regions(instruction) {
            holding_variables(region, registers, variables);
        }
    }
}

/// The walk [`escaping_nested_bodies`] makes over a body's reads.
struct Escapes<'e> {
    registers: &'e HashMap<u32, (String, bool)>,
    variables: &'e HashMap<u32, &'e str>,
    rebound: &'e HashSet<u32>,
    escaping: &'e mut BTreeSet<String>,
}

impl Escapes<'_> {
    fn blocks(&mut self, blocks: &[MirBlock]) {
        for block in blocks {
            for instruction in &block.instrs {
                self.instruction(instruction);
                for region in instruction_regions(instruction) {
                    self.blocks(region);
                }
            }
            for reg in terminator_regs_mut(&mut block.term.clone()) {
                self.escape_register(reg.0);
            }
        }
    }

    fn instruction(&mut self, instruction: &MirInstr) {
        let mut operands = Vec::new();
        instruction_operand_regs(instruction, &mut operands);
        let mut slots = instruction_slots(instruction);
        match instruction {
            MirInstr::MakeClosure {
                dest,
                function,
                captures,
            } if !self.registers.contains_key(&dest.0)
                || captures
                    .iter()
                    .any(|capture| capture.mode != MirCaptureMode::Reference) =>
            {
                self.escaping.insert(function.clone());
            }
            MirInstr::Const {
                dest,
                k: Const::Function(function),
            } if !self.registers.contains_key(&dest.0) => {
                self.escaping.insert(function.clone());
            }
            MirInstr::CallIndirect {
                callee,
                callee_place,
                ..
            } => {
                if let Some(position) = operands.iter().position(|reg| reg == callee) {
                    operands.remove(position);
                }
                if let Some(place) = callee_place
                    && let Some(position) = slots.iter().position(|&slot| slot == place.root)
                {
                    slots.remove(position);
                }
            }
            MirInstr::CopyValue { .. } => operands.clear(),
            MirInstr::DefVar { var, src, .. } if !self.rebound.contains(var) => {
                operands.retain(|reg| reg != src);
                slots.retain(|slot| slot != var);
            }
            MirInstr::DefVar { var, .. } => slots.retain(|slot| slot != var),
            MirInstr::UseVar { dest, var, .. } if self.registers.contains_key(&dest.0) => {
                slots.retain(|slot| slot != var);
            }
            MirInstr::DropVar { var } => slots.retain(|slot| slot != var),
            _ => {}
        }
        for reg in operands {
            self.escape_register(reg.0);
        }
        for slot in slots {
            if let Some(body) = self.variables.get(&slot) {
                self.escaping.insert((*body).to_string());
            }
        }
    }

    fn escape_register(&mut self, reg: u32) {
        if let Some((body, _)) = self.registers.get(&reg) {
            self.escaping.insert(body.clone());
        }
    }
}
