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
//! or by a closure of reference captures, becomes a direct call of that
//! function, its captured places prepended to the arguments or its
//! storage the receiver, as KGEN's `CallParamOp` and `CallIndirectOp`
//! canonicalizers fold a known callee.
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
    Const, FuncRef, MirBlock, MirCaptureAccess, MirCaptureMode, MirDeclarations, MirFunction,
    MirFunctionDeclaration, MirInstr, MirPlace, Proj, Reg, count_register_reads, fresh_register,
    instruction_regions_mut, instruction_regs_mut, terminator_regs_mut,
};
use mojito_types::origin::{CaptureAccess, OriginSeg};
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
                MirInstr::GetField { dest, base, field } => {
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
    // A field read of a `struct.create` forwards the field's operand, which
    // the creation reads: an owned operand forwards only when that read is
    // the struct's last, so the creation dies with it.
    let sole_struct_reader = |dest: &u32| {
        structs
            .extracts
            .get(dest)
            .is_none_or(|(base, _)| sole_reader(base))
    };
    folds.retain(|dest: &u32, operand: &mut Reg| {
        single(*dest, 1)
            && single(operand.0, 1)
            && (in_register(operand) || (sole_reader(operand) && sole_struct_reader(dest)))
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
        let MirInstr::GetField { base, field, .. } = instruction else {
            return None;
        };
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
/// defined once by a function constant, or by a closure of reference
/// captures, into a direct call of that function; returns how many were
/// rewritten.
///
/// This is KGEN's `CallParamOp::canonicalize`, a call of a known callee
/// symbol, and its `CallIndirectOp::canonicalize` over a `create_closure`,
/// which prepends the closure's captures to the arguments: each captured
/// place is loaded before the call and passed as a direct call of a
/// capturing nested `def` passes it. A closure holding a lambda's moved
/// storage has no place to load it from and stays indirect. A copied
/// callee was forwarded to its constant already; one read out of a slot
/// is not a known callee.
fn fold_known_callees(function: &mut MirFunction, declarations: &MirDeclarations) -> usize {
    let definitions = definition_counts(&mut function.blocks, function.n_regs);
    let mut callees = HashMap::new();
    visit_blocks(&mut function.blocks, &mut |block| {
        for instruction in &block.instrs {
            let (dest, callee) = match instruction {
                MirInstr::Const {
                    dest,
                    k: Const::Function(name),
                } => (dest, KnownCallee::new(name, Vec::new())),
                MirInstr::MakeClosure {
                    dest,
                    function,
                    captures,
                } if captures
                    .iter()
                    .all(|capture| capture.mode == MirCaptureMode::Reference) =>
                {
                    let places = captures.iter().map(|capture| capture.place.clone());
                    (dest, KnownCallee::new(function, places.collect()))
                }
                _ => continue,
            };
            if definitions.get(dest.0 as usize) == Some(&1) {
                callees.insert(dest.0, callee);
            }
        }
    });
    if callees.is_empty() {
        return 0;
    }
    let mut blocks = std::mem::take(&mut function.blocks);
    let mut folded = 0;
    visit_blocks(&mut blocks, &mut |block| {
        let mut instructions = Vec::with_capacity(block.instrs.len());
        for mut instruction in std::mem::take(&mut block.instrs) {
            if let Some((callee, declaration)) =
                folded_callee(&instruction, &callees, function, declarations)
            {
                instructions.extend(direct_call(
                    &mut instruction,
                    callee,
                    declaration,
                    function,
                    declarations,
                ));
                folded += 1;
            }
            instructions.push(instruction);
        }
        block.instrs = instructions;
    });
    function.blocks = blocks;
    if folded > 0 {
        erase_closure_drops(function, &callees);
    }
    folded
}

/// Erase each `drop.reg` of a closure in `callees` that is its last
/// reader: a closure of references destroys nothing, so once its calls
/// are direct it goes with the erasure of dead instructions.
fn erase_closure_drops(function: &mut MirFunction, callees: &HashMap<u32, KnownCallee>) {
    let mut reads = vec![0; function.n_regs as usize];
    count_register_reads(&mut function.blocks, &mut reads);
    let mut drops: HashMap<u32, u32> = HashMap::new();
    visit_blocks(&mut function.blocks, &mut |block| {
        for instruction in &block.instrs {
            if let MirInstr::Drop { reg, .. } = instruction
                && callees.contains_key(&reg.0)
            {
                *drops.entry(reg.0).or_default() += 1;
            }
        }
    });
    drops.retain(|reg, count| reads.get(*reg as usize) == Some(count));
    if drops.is_empty() {
        return;
    }
    visit_blocks(&mut function.blocks, &mut |block| {
        block.instrs.retain(|instruction| match instruction {
            MirInstr::Drop { reg, .. } => !drops.contains_key(&reg.0),
            _ => true,
        });
    });
}

/// The callee a `call.indirect` is known to call: the function a constant
/// names, or a closure's with the places it captures by reference.
struct KnownCallee {
    name: String,
    captures: Vec<MirPlace>,
}

impl KnownCallee {
    fn new(name: &str, captures: Vec<MirPlace>) -> Self {
        Self {
            name: name.to_owned(),
            captures,
        }
    }
}

/// The known callee a `call.indirect` through one of `callees` folds to,
/// with its declaration, when a direct call binds its operands as the
/// indirect call did: the target declares no compile-time parameters or
/// collectors, the closure's captures are its storage, which a closure's
/// method reads as `self`, or its leading parameters, each a reference to
/// its captured place's type, the rest are the callee's function type
/// (upstream's `verifyCallOp` agreement), and any keyword names a
/// parameter the declaration names alike.
fn folded_callee<'c, 'd>(
    instruction: &MirInstr,
    callees: &'c HashMap<u32, KnownCallee>,
    function: &MirFunction,
    declarations: &'d MirDeclarations,
) -> Option<(&'c KnownCallee, &'d MirFunctionDeclaration)> {
    let MirInstr::CallIndirect {
        callee,
        kwargs,
        param_arg_regs,
        instantiated_args,
        spread,
        ..
    } = instruction
    else {
        return None;
    };
    let known = callees.get(&callee.0)?;
    let declaration = declarations.function(&known.name)?;
    let contract = function.reg_types.get(&callee.0)?.as_func()?;
    let (captures_bind, leading) = if declaration.is_closure_method() {
        let reads_storage = matches!(known.captures.as_slice(), [storage] if storage.ty.is_some())
            && declaration.receiver_convention.is_none()
            && !declaration.returns_reference;
        (reads_storage, 0)
    } else {
        let captures_bind = known.captures.iter().enumerate().all(|(index, place)| {
            declaration.ref_params.get(index) == Some(&true)
                && place
                    .ty
                    .as_ref()
                    .is_some_and(|ty| declaration.param_types.get(index) == Some(ty))
        });
        (captures_bind, known.captures.len())
    };
    let binds = captures_bind
        && declaration.param_decls.is_empty()
        && param_arg_regs.is_empty()
        && instantiated_args.is_empty()
        && spread.is_none()
        && declaration.variadic.is_none()
        && declaration.kw_variadic.is_none()
        && declaration.param_types.get(leading..) == Some(contract.params.as_slice())
        && declaration.param_conventions.get(leading..) == Some(contract.conventions.as_slice())
        && contract.ret == declaration.ret_ty
        && contract.ref_return.is_some() == declaration.returns_reference
        && contract.raises == declaration.raises
        && (kwargs.is_empty()
            || declaration.param_names.get(leading..) == Some(contract.names.as_slice()));
    binds.then_some((known, declaration))
}

/// Rewrite the `call.indirect` `instruction` into the direct call of
/// `callee`, declared as `declaration`, returning the loads of its
/// captured places the call now takes first.
///
/// A closure's method is called on its storage, passed at its place when
/// memory-only, as a capture-list `def` called by name is; any other
/// target takes each captured place as a leading reference argument, with
/// the access its parameter makes to it.
fn direct_call(
    instruction: &mut MirInstr,
    callee: &KnownCallee,
    declaration: &MirFunctionDeclaration,
    function: &mut MirFunction,
    declarations: &MirDeclarations,
) -> Vec<MirInstr> {
    let MirInstr::CallIndirect {
        dest,
        raises,
        args,
        kwargs,
        arg_places,
        kwarg_places,
        capture_accesses,
        ..
    } = instruction
    else {
        return Vec::new();
    };
    let span = function.spans.0.get(&dest.0).map(|(span, _)| span.clone());
    let mut loads = Vec::with_capacity(callee.captures.len());
    let mut operands = Vec::with_capacity(callee.captures.len());
    for place in &callee.captures {
        let Some(ty) = place.ty.clone() else {
            continue;
        };
        let load = fresh_register(function, ty, span.as_ref());
        loads.push(MirInstr::LoadPlace {
            dest: load,
            place: place.clone(),
        });
        operands.push((load, place));
    }
    if declaration.is_closure_method()
        && let [(recv, storage)] = operands.as_slice()
    {
        let method = mojito_symbol::symbol::split_method_symbol(&callee.name)
            .map_or(callee.name.as_str(), |(_, method)| method);
        let memory_only = storage
            .ty
            .as_ref()
            .is_some_and(|ty| !declarations.register_passable(ty));
        *instruction = MirInstr::MethodCall {
            dest: *dest,
            recv: *recv,
            method: method.to_owned(),
            resolved: Some(callee.name.clone()),
            raises: raises.take(),
            reference_result: None,
            result_adapter: None,
            args: std::mem::take(args),
            kwargs: std::mem::take(kwargs),
            recv_place: memory_only.then(|| Box::new((*storage).clone())),
            recv_writes: false,
            receiver_convention: None,
            arg_places: std::mem::take(arg_places),
            kwarg_places: std::mem::take(kwarg_places),
            capture_accesses: std::mem::take(capture_accesses),
            param_arg_regs: Vec::new(),
            param_decls: Vec::new(),
            instantiated_args: Vec::new(),
            spread: None,
            refined_bound: None,
        };
        return loads;
    }
    let mut accesses = std::mem::take(capture_accesses);
    let mut places = Vec::with_capacity(operands.len() + arg_places.len());
    for (index, (_, place)) in operands.iter().enumerate() {
        places.push(Some((*place).clone()));
        let access = MirCaptureAccess {
            root: place.root,
            path: capture_path(place),
            access: if declaration.param_writes.get(index) == Some(&true) {
                CaptureAccess::Write
            } else {
                CaptureAccess::Read
            },
        };
        if !accesses.contains(&access) {
            accesses.push(access);
        }
    }
    places.append(arg_places);
    *instruction = MirInstr::Call {
        dest: *dest,
        func: FuncRef(callee.name.clone()),
        raises: raises.take(),
        args: operands
            .iter()
            .map(|(load, _)| *load)
            .chain(args.drain(..))
            .collect(),
        kwargs: std::mem::take(kwargs),
        arg_places: places,
        kwarg_places: std::mem::take(kwarg_places),
        capture_accesses: accesses,
        param_arg_regs: Vec::new(),
        receiver: None,
        instantiated_args: Vec::new(),
        spread: None,
    };
    loads
}

/// The origin path a capture access names for `place`: its leading field
/// projections, the narrowest path that still covers every step after
/// them.
fn capture_path(place: &MirPlace) -> Vec<OriginSeg> {
    place
        .steps()
        .map_while(|step| match step {
            Proj::Field(name) => Some(OriginSeg::Field(name.clone())),
            _ => None,
        })
        .collect()
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
