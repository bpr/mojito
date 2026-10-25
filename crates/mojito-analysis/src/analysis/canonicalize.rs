//! `canonicalize`: upstream's `Canonicalizer`, MLIR's `canonicalize` pass,
//! which its pre-elaboration pipeline runs after `EliminateDeadSymbols`.
//!
//! This is its folding, region simplification, and dead-op erasure: an
//! instruction [`MirInstr::folded_constant`] folds over operands defined
//! once by a constant becomes that `const`, as MLIR's greedy driver applies
//! a `fold` that returns an attribute (through the replacement `sccp`
//! shares, `fold::replace_folded`); an instruction that folds to
//! one of its operands ([`MirInstr::forwarded_operand`], an identity
//! `value.copy`, or a copy of a closure of reference captures, a struct of
//! pointers) is replaced by that operand in every reader, as MLIR's greedy
//! driver applies a `fold` that returns an operand; and an instruction
//! whose results nothing reads goes when it has no effect but reading
//! ([`MirInstr::would_be_trivially_dead`]), repeated until nothing changes.
//! A `call.indirect` whose callee is defined once by a function constant,
//! or by a closure of no captures, becomes a direct `call` of that
//! function, as KGEN's `CallParamOp` and `CallIndirectOp` canonicalizers
//! fold a known callee.
//! A `drop.reg` of a register whose type an instantiation made one whose
//! destruction runs no code goes first, as upstream tracks no
//! register-passable trivial value: a fold may then forward the register
//! past where it died.
//!
//! A structured body's region ops take upstream's `hlcf` canonicalization
//! patterns (`hlcf`): an `if` on a constant keeps the arm it selects, exits
//! every arm shares are hoisted after the `if`, arms that only yield fold
//! into the results, a loop that only breaks goes, and a `comptime if` on a
//! constant keeps its live arm. Folding and the patterns alternate until
//! the patterns stop, as the greedy driver's worklist does. An erased raise
//! point erases the destructors on its edge: each `try`'s raise-edge
//! cleanup is cut to what a remaining raise path holds.

mod hlcf;

pub(super) use hlcf::erased_results;

use super::fold;
use super::pre_elaboration::BodyPass;
use mojito_common::timing;
use mojito_mir::mir::verify::instruction_result_regs;
use mojito_mir::mir::{
    Const, FuncRef, MirBlock, MirDeclarations, MirFunction, MirInstr, Reg, count_register_reads,
    instruction_regions_mut, instruction_regs_mut, terminator_regs_mut,
};
use mojito_types::types::{Ty, TyKind};
use std::collections::{HashMap, HashSet};

/// The pass, under MLIR's flag spelling.
pub const CANONICALIZE: BodyPass = BodyPass {
    name: "canonicalize",
    run: canonicalize,
};

/// Fold constants, simplify the region ops, and erase what `function` no
/// longer reaches or reads.
///
/// Each instruction over constant operands becomes a `const` and each
/// region op the `hlcf` patterns match is rewritten; identity copies are
/// forwarded, a call through a known callee becomes a direct call, and
/// dead instructions are erased.
/// Constants fold first, so a comparison folded to a `const bool` decides
/// its `if` in the same run. Erasure and the patterns repeat until neither
/// changes anything: an erased instruction can empty an arm. A removed
/// instruction's results lose their types and spans; registers are not
/// renumbered. Returns whether any of these rewrites applied.
pub fn canonicalize(
    _name: &str,
    function: &mut MirFunction,
    declarations: &MirDeclarations,
) -> bool {
    let mut erased = HashSet::new();
    let mut folded_constants = 0;
    let mut patterns = erase_trivial_drops(&mut function.blocks, &function.reg_types, declarations);
    // A fold can expose another, as a struct copy whose fields were copies
    // does once they are forwarded, and a forwarded copy can expose a fold,
    // as a comparison of two copies of one value does: the greedy driver
    // folds to a fixpoint.
    let mut forwarded_copies = 0;
    loop {
        loop {
            folded_constants += fold_constants(function, declarations);
            let rewritten = hlcf::hlcf_patterns(function, &mut erased);
            if rewritten == 0 {
                break;
            }
            patterns += rewritten;
        }
        let before = forwarded_copies;
        loop {
            let forwarded = forward_copies(function, declarations);
            if forwarded == 0 {
                break;
            }
            forwarded_copies += forwarded;
        }
        if forwarded_copies == before {
            break;
        }
    }
    timing::count("folded_constants", folded_constants as u64);
    timing::count("forwarded_copies", forwarded_copies as u64);
    let folded_callees = fold_known_callees(function, declarations);
    timing::count("folded_callees", folded_callees as u64);
    patterns += folded_callees;
    loop {
        let before = erased.len();
        erase_dead_until_none(function, &mut erased);
        if erased.len() == before {
            break;
        }
        let rewritten = hlcf::hlcf_patterns(function, &mut erased);
        if rewritten == 0 {
            break;
        }
        patterns += rewritten;
    }
    function.reg_types.retain(|reg, _| !erased.contains(reg));
    function.spans.0.retain(|reg, _| !erased.contains(reg));
    timing::count("erased_instructions", erased.len() as u64);
    folded_constants + patterns + forwarded_copies + erased.len() > 0
}

/// Erase each `drop.reg` of `blocks`, regions included, whose register
/// holds no owned value ([`mojito_mir::mir::owned_register`]); returns how
/// many went.
fn erase_trivial_drops(
    blocks: &mut [MirBlock],
    reg_types: &HashMap<u32, Ty>,
    declarations: &MirDeclarations,
) -> usize {
    let mut erased = 0;
    visit_blocks(blocks, &mut |block| {
        let before = block.instrs.len();
        block.instrs.retain(|instruction| {
            !matches!(instruction, MirInstr::Drop { reg, .. }
                if reg_types
                    .get(&reg.0)
                    .is_some_and(|ty| !mojito_mir::mir::owned_register(ty, declarations)))
        });
        erased += before - block.instrs.len();
    });
    erased
}

/// How many times `blocks` and their regions define each of `n_regs`
/// registers.
pub(super) fn definition_counts(blocks: &mut [MirBlock], n_regs: u32) -> Vec<u32> {
    let mut definitions = vec![0_u32; n_regs as usize];
    visit_blocks(blocks, &mut |block| {
        for instruction in &block.instrs {
            let mut results = Vec::new();
            instruction_result_regs(instruction, &mut results);
            for reg in &results {
                if let Some(count) = definitions.get_mut(reg.0 as usize) {
                    *count += 1;
                }
            }
        }
    });
    definitions
}

/// Replace each instruction of `function` that [`MirInstr::folded_constant`]
/// folds over operands defined once by a folded `const`
/// ([`Const::is_folded`]) by that `const`, until none folds; returns how
/// many were replaced.
///
/// One definition makes every read of an operand see its constant; a folded
/// instruction's own result may have several, as replacing one definition
/// by the constant it computes is exact. Each round folds one level of a
/// chain, the fixpoint MLIR's greedy driver reaches through its worklist.
fn fold_constants(function: &mut MirFunction, declarations: &MirDeclarations) -> usize {
    let mut folded = 0;
    loop {
        let definitions = definition_counts(&mut function.blocks, function.n_regs);
        let mut constants = HashMap::new();
        visit_blocks(&mut function.blocks, &mut |block| {
            for instruction in &block.instrs {
                if let MirInstr::Const { dest, k } = instruction
                    && k.is_folded()
                    && definitions.get(dest.0 as usize) == Some(&1)
                {
                    constants.insert(dest.0, k.clone());
                }
            }
        });
        let constant = |reg: Reg| constants.get(&reg.0).cloned();
        let round = fold::replace_folded(
            &mut function.blocks,
            &constant,
            &function.reg_types,
            declarations,
        );
        if round == 0 {
            return folded;
        }
        folded += round;
    }
}

/// Call `visit` on every block of `blocks`, then on the blocks of the `try`
/// regions each holds.
fn visit_blocks(blocks: &mut [MirBlock], visit: &mut impl FnMut(&mut MirBlock)) {
    for block in blocks {
        visit(block);
        for region in block.instrs.iter_mut().flat_map(instruction_regions_mut) {
            visit_blocks(region, visit);
        }
    }
}

/// Replace each register an instruction folds to an operand
/// ([`MirInstr::forwarded_operand`]) by that operand, chains resolved to
/// their root, and remove the folding instructions; returns how many went.
///
/// Only a result with one definition whose operand has at most one is
/// forwarded, so every other occurrence of the result is a read the
/// operand's definition dominates. A forwarded register-passable trivial
/// value ([`MirDeclarations::copy_is_identity`]) gains the copy's readers,
/// which copy it where they consume it, as `cse` relies on too. Any other
/// forwarded value (a struct field read out of a `struct.create`) is
/// forwarded only when the fold is its operand's sole reader: a reader may
/// consume such a value, so it keeps exactly the fold's readers.
fn forward_copies(function: &mut MirFunction, declarations: &MirDeclarations) -> usize {
    let MirFunction {
        blocks,
        reg_types,
        n_regs,
        ..
    } = function;
    let definitions = definition_counts(blocks, *n_regs);
    let mut reads = vec![0_u32; *n_regs as usize];
    count_register_reads(blocks, &mut reads);
    let mut closures = HashSet::new();
    let mut creates = HashMap::new();
    let mut extracts = HashMap::new();
    visit_blocks(blocks, &mut |block| {
        for instruction in &block.instrs {
            match instruction {
                MirInstr::MakeClosure { dest, .. } if instruction.would_be_trivially_dead() => {
                    closures.insert(dest.0);
                }
                MirInstr::MakeStruct { dest, fields } => {
                    creates.insert(dest.0, fields.clone());
                }
                MirInstr::GetField {
                    dest,
                    base,
                    field,
                    constructors,
                } if constructors.is_empty() => {
                    extracts.insert(dest.0, (*base, field.clone()));
                }
                _ => {}
            }
        }
    });
    let structs = StructFolds {
        reg_types,
        declarations,
        creates,
        extracts,
    };
    // A copy of a closure of reference captures copies its pointers.
    let copied_closure = |instruction: &MirInstr| match instruction {
        MirInstr::CopyValue { dest, value, .. }
            if closures.contains(&value.0) && reg_types.get(&dest.0) == reg_types.get(&value.0) =>
        {
            Some(*value)
        }
        _ => None,
    };
    let mut folds = HashMap::new();
    let mut copies = HashMap::new();
    visit_blocks(blocks, &mut |block| {
        for instruction in &block.instrs {
            let mut results = Vec::new();
            instruction_result_regs(instruction, &mut results);
            if let (Some(operand), [dest]) = (
                instruction
                    .forwarded_operand(reg_types, declarations)
                    .or_else(|| copied_closure(instruction))
                    .or_else(|| structs.extracted(instruction)),
                results.as_slice(),
            ) {
                folds.insert(dest.0, operand);
            }
            if let MirInstr::MakeStruct { dest, fields } = instruction
                && let Some(container) = structs.trivial_copy(*dest, fields, &reads)
            {
                copies.insert(dest.0, container);
            }
        }
    });
    let single = |reg: u32, most: u32| definitions.get(reg as usize).is_some_and(|n| *n <= most);
    let in_register = |reg: &Reg| {
        closures.contains(&reg.0)
            || reg_types
                .get(&reg.0)
                .is_some_and(|ty| declarations.copy_is_identity(ty))
    };
    let sole_reader = |reg: &Reg| reads.get(reg.0 as usize) == Some(&1);
    folds.retain(|dest: &u32, operand: &mut Reg| {
        single(*dest, 1) && single(operand.0, 1) && (in_register(operand) || sole_reader(operand))
    });
    // A trivial struct copy's container keeps the copy's readers alone: the
    // extracts that were its only other readers go with the copy.
    folds.extend(
        copies
            .into_iter()
            .filter(|(dest, container)| single(*dest, 1) && single(container.0, 1)),
    );
    let roots: HashMap<u32, Reg> = folds
        .keys()
        .filter_map(|dest| {
            let mut root = folds[dest];
            for _ in 0..folds.len() {
                match folds.get(&root.0) {
                    Some(next) => root = *next,
                    None => return Some((*dest, root)),
                }
            }
            None
        })
        .collect();
    forward(blocks, &roots);
    reg_types.retain(|reg, _| !roots.contains_key(reg));
    function.spans.0.retain(|reg, _| !roots.contains_key(reg));
    roots.len()
}

/// The struct values a body builds and takes apart, for upstream's
/// `kgen.struct` folds.
struct StructFolds<'a> {
    reg_types: &'a HashMap<u32, Ty>,
    declarations: &'a MirDeclarations,
    /// Each `struct.create`'s field operands, by its result.
    creates: HashMap<u32, Vec<Reg>>,
    /// Each `field.get`'s container and field, by its result.
    extracts: HashMap<u32, (Reg, String)>,
}

impl StructFolds<'_> {
    /// The operand an extract of a created struct's field reads: upstream's
    /// `StructExtractOp::fold`.
    fn extracted(&self, instruction: &MirInstr) -> Option<Reg> {
        let MirInstr::GetField {
            base,
            field,
            constructors,
            ..
        } = instruction
        else {
            return None;
        };
        if !constructors.is_empty() {
            return None;
        }
        let position = self.position(*base, field)?;
        self.creates.get(&base.0)?.get(position).copied()
    }

    /// The container a `struct.create` of every field extracted from it in
    /// order copies, when the extracts, each read once, are its only
    /// readers: upstream's `foldTrivialStructCopy`.
    fn trivial_copy(&self, dest: Reg, fields: &[Reg], reads: &[u32]) -> Option<Reg> {
        let (container, _) = self.extracts.get(&fields.first()?.0)?;
        let read_once =
            |reg: &Reg, times: usize| reads.get(reg.0 as usize).map(|&n| n as usize) == Some(times);
        (self.reg_types.get(&container.0) == self.reg_types.get(&dest.0)
            && read_once(container, fields.len())
            && fields.iter().enumerate().all(|(index, field)| {
                read_once(field, 1)
                    && self.extracts.get(&field.0).is_some_and(|(base, name)| {
                        base.0 == container.0 && self.position(*base, name) == Some(index)
                    })
            }))
        .then_some(*container)
    }

    /// The declaration position of the field `field` of the struct `base`
    /// holds.
    fn position(&self, base: Reg, field: &str) -> Option<usize> {
        let TyKind::Struct(name, _) = self.reg_types.get(&base.0)?.kind() else {
            return None;
        };
        self.declarations
            .structure(name)?
            .fields
            .iter()
            .position(|(candidate, _)| candidate == field)
    }
}

/// Erase the trivially dead instructions of `function` until none is left,
/// adding their results to `erased`.
fn erase_dead_until_none(function: &mut MirFunction, erased: &mut HashSet<u32>) {
    loop {
        let mut reads = vec![0_u32; function.n_regs as usize];
        count_register_reads(&mut function.blocks, &mut reads);
        let before = erased.len();
        erase_dead(&mut function.blocks, &reads, erased);
        if erased.len() == before {
            break;
        }
    }
}

/// Remove each instruction of `blocks` and their regions that would be
/// trivially dead and defines only unread results, adding them to `erased`.
fn erase_dead(blocks: &mut [MirBlock], reads: &[u32], erased: &mut HashSet<u32>) {
    for block in blocks {
        block.instrs.retain_mut(|instruction| {
            for region in instruction_regions_mut(instruction) {
                erase_dead(region, reads, erased);
            }
            let mut results = Vec::new();
            instruction_result_regs(instruction, &mut results);
            let dead = instruction.would_be_trivially_dead()
                && !results.is_empty()
                && results
                    .iter()
                    .all(|reg| reads.get(reg.0 as usize) == Some(&0));
            if dead {
                erased.extend(results.iter().map(|reg| reg.0));
            }
            !dead
        });
    }
}

/// Rewrite each `call.indirect` of `function` whose callee register is
/// defined once by a function constant, or by a closure of no captures,
/// into a direct `call` of that function; returns how many were rewritten.
///
/// This is KGEN's `CallParamOp::canonicalize`, a call of a known callee
/// symbol, and its `CallIndirectOp::canonicalize` over a `create_closure`
/// with nothing to prepend. A copied callee was forwarded to its constant
/// already; one read out of a slot is not a known callee.
fn fold_known_callees(function: &mut MirFunction, declarations: &MirDeclarations) -> usize {
    let definitions = definition_counts(&mut function.blocks, function.n_regs);
    let mut callees = HashMap::new();
    visit_blocks(&mut function.blocks, &mut |block| {
        for instruction in &block.instrs {
            let (dest, name) = match instruction {
                MirInstr::Const {
                    dest,
                    k: Const::Function(name),
                } => (dest, name),
                MirInstr::MakeClosure {
                    dest,
                    function,
                    captures,
                } if captures.is_empty() => (dest, function),
                _ => continue,
            };
            if definitions.get(dest.0 as usize) == Some(&1) {
                callees.insert(dest.0, name.clone());
            }
        }
    });
    if callees.is_empty() {
        return 0;
    }
    let reg_types = &function.reg_types;
    let mut folded = 0;
    visit_blocks(&mut function.blocks, &mut |block| {
        for instruction in &mut block.instrs {
            if let Some(call) = direct_call(instruction, &callees, reg_types, declarations) {
                *instruction = call;
                folded += 1;
            }
        }
    });
    folded
}

/// The direct `call` a `call.indirect` through one of `callees` is, when
/// it binds its operands as the indirect call did: the target declares no
/// compile-time parameters or collectors, its signature is the callee's
/// function type (upstream's `verifyCallOp` agreement), and any keyword
/// names a parameter the declaration names alike.
fn direct_call(
    instruction: &mut MirInstr,
    callees: &HashMap<u32, String>,
    reg_types: &HashMap<u32, Ty>,
    declarations: &MirDeclarations,
) -> Option<MirInstr> {
    let MirInstr::CallIndirect {
        dest,
        callee,
        raises,
        args,
        kwargs,
        arg_places,
        kwarg_places,
        capture_accesses,
        param_arg_regs,
        instantiated_args,
        spread,
        ..
    } = instruction
    else {
        return None;
    };
    let name = callees.get(&callee.0)?;
    let declaration = declarations.function(name)?;
    let contract = reg_types.get(&callee.0)?.as_func()?;
    let binds = declaration.param_decls.is_empty()
        && param_arg_regs.is_empty()
        && instantiated_args.is_empty()
        && spread.is_none()
        && declaration.variadic.is_none()
        && declaration.kw_variadic.is_none()
        && contract.params == declaration.param_types
        && contract.conventions == declaration.param_conventions
        && contract.ret == declaration.ret_ty
        && contract.ref_return.is_some() == declaration.returns_reference
        && contract.raises == declaration.raises
        && (kwargs.is_empty() || contract.names == declaration.param_names);
    binds.then(|| MirInstr::Call {
        dest: *dest,
        func: FuncRef(name.clone()),
        raises: raises.take(),
        args: std::mem::take(args),
        kwargs: std::mem::take(kwargs),
        arg_places: std::mem::take(arg_places),
        kwarg_places: std::mem::take(kwarg_places),
        capture_accesses: std::mem::take(capture_accesses),
        param_arg_regs: Vec::new(),
        receiver: None,
        instantiated_args: Vec::new(),
        spread: None,
    })
}

/// Remove the instructions defining `roots`' keys from `blocks` and their
/// regions, and rewrite every read of a key to its root.
pub(super) fn forward(blocks: &mut [MirBlock], roots: &HashMap<u32, Reg>) {
    let rewrite = |reg: &mut Reg| {
        if let Some(root) = roots.get(&reg.0) {
            *reg = *root;
        }
    };
    for block in blocks {
        block.instrs.retain_mut(|instruction| {
            for region in instruction_regions_mut(instruction) {
                forward(region, roots);
            }
            let mut results = Vec::new();
            instruction_result_regs(instruction, &mut results);
            if results.iter().any(|reg| roots.contains_key(&reg.0)) {
                return false;
            }
            instruction_regs_mut(instruction)
                .into_iter()
                .for_each(rewrite);
            true
        });
        terminator_regs_mut(&mut block.term)
            .into_iter()
            .for_each(rewrite);
    }
}
