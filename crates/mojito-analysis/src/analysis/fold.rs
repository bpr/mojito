//! The one place a folded instruction becomes a `const` and a `const` is
//! materialized at its region's entry, for `sccp` and `canonicalize` alike,
//! as MLIR's `OperationFolder` (`FoldUtils`) is for `SCCP` and the greedy
//! pattern driver.
//!
//! The folder materializes each constant once per value at the entry of
//! its insertion region and erases every other `const` of that value: the
//! body, or for a constant a `comptime` region's binders or dead arms may
//! leave meaningless (a parameter expression, a function instance, or any
//! constant of a type that is not closed), the innermost `comptime if` arm
//! or `comptime for` region holding it, as a fold interface's
//! `shouldMaterializeInto` claims a region. A constant is keyed by its
//! value, its result type, and a parameter expression's identity
//! ([`parameter_identity`]), as the folder uniques by dialect, attribute,
//! and type. Only a `const` whose register has one definition and holds a
//! register-passable trivial value ([`MirDeclarations::copy_is_identity`])
//! takes part, as upstream's parser holds only such a value in an SSA
//! value: any other stays where it is.

use super::canonicalize::{definition_counts, forward};
use mojito_mir::mir::verify::instruction_result_regs;
use mojito_mir::mir::{
    Const, MirBlock, MirDeclarations, MirFunction, MirInstr, Reg, instruction_regions,
    instruction_regions_mut,
};
use mojito_types::types::Ty;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

/// Replace each instruction of `block` and its regions that folds
/// ([`MirInstr::folded_constant`]) over `constant`'s operands by a `const`
/// defining the same register, and return the registers so defined; the
/// caller's [`materialize_constants`] moves each to its region's entry, as
/// MLIR materializes the constant there and erases the op. A folded
/// producer's in-place binding is no longer in place, as a `const` is no
/// call-family producer. Terminators are left alone.
pub(super) fn replace_folded(
    block: &mut MirBlock,
    constant: &impl Fn(Reg) -> Option<Const>,
    reg_types: &HashMap<u32, Ty>,
    declarations: &MirDeclarations,
) -> Vec<Reg> {
    let mut folded = Vec::new();
    let mut folded_here = Vec::new();
    for instruction in &mut block.instrs {
        let regions = instruction_regions_mut(instruction);
        if !regions.is_empty() {
            for region in regions {
                folded.extend(replace_folded(region, constant, reg_types, declarations));
            }
            continue;
        }
        let mut results = Vec::new();
        instruction_result_regs(instruction, &mut results);
        if let (Some(k), [dest]) = (
            instruction.folded_constant(constant, reg_types, declarations),
            results.as_slice(),
        ) {
            folded_here.push(*dest);
            *instruction = MirInstr::Const { dest: *dest, k };
        }
    }
    clear_in_place(block, &folded_here.iter().map(|reg| reg.0).collect());
    folded.extend(folded_here);
    folded
}

/// Move each eligible `const` of `function` (only those defining one of
/// `only`'s registers, when given) to its insertion region's entry, after
/// the constants already there, and erase each one whose value one before
/// it in walk order already holds, its reads renamed to that one's
/// register; returns how many constants moved or went.
///
/// The greedy driver's `insertKnownConstant` runs this over every constant;
/// `SCCP` uniques only the constants it creates, so it passes those.
/// Constants are kept in walk order, regions after the op holding them.
pub(super) fn materialize_constants(
    function: &mut MirFunction,
    declarations: &MirDeclarations,
    only: Option<&HashSet<u32>>,
) -> usize {
    let definitions = definition_counts(&mut function.body, function.n_regs);
    let mut plan = Plan {
        reg_types: &function.reg_types,
        declarations,
        definitions: &definitions,
        only,
        scopes: 1,
        canonical: HashMap::new(),
        targets: HashMap::new(),
        moved: HashSet::new(),
        duplicates: HashMap::new(),
    };
    plan.scope(&function.body, 0, None);
    let Plan {
        targets,
        moved,
        duplicates,
        ..
    } = plan;
    if moved.is_empty() && duplicates.is_empty() {
        return 0;
    }
    forward(&mut function.body, &duplicates);
    let mut taken: Vec<Vec<MirInstr>> = Vec::new();
    take_moved(&mut function.body, &moved, &mut taken);
    let mut materialized = HashMap::<usize, Vec<MirInstr>>::new();
    for instruction in taken.into_iter().flatten() {
        if let MirInstr::Const { dest, .. } = &instruction {
            materialized
                .entry(targets[&dest.0])
                .or_default()
                .push(instruction);
        }
    }
    let mut next = 0;
    place(&mut function.body, &targets, &mut materialized, &mut next);
    let entry: HashSet<u32> = targets.keys().copied().collect();
    clear_in_place_everywhere(&mut function.body, &entry);
    function
        .reg_types
        .retain(|reg, _| !duplicates.contains_key(reg));
    function
        .spans
        .0
        .retain(|reg, _| !duplicates.contains_key(reg));
    moved.len() + duplicates.len()
}

/// The identity hash of a parameter constant's expression, which its
/// spelling leaves out: two `comptime for` binders spelled alike are
/// distinct attributes, as upstream's parameter attributes are. Any other
/// constant is its spelling, `0`.
pub(super) fn parameter_identity(k: &Const) -> u64 {
    let mut state = std::collections::hash_map::DefaultHasher::new();
    match k {
        Const::Param(expr) => expr.hash(&mut state),
        Const::Value(value) => mojito_types::param_expr::identity_hash(value, &mut state),
        _ => return 0,
    }
    state.finish()
}

/// What [`materialize_constants`] decides in its walk: each insertion
/// region is numbered in walk order, the body `0`.
struct Plan<'a> {
    reg_types: &'a HashMap<u32, Ty>,
    declarations: &'a MirDeclarations,
    definitions: &'a [u32],
    only: Option<&'a HashSet<u32>>,
    /// How many insertion regions the walk has numbered.
    scopes: usize,
    /// The register holding each value in each insertion region.
    canonical: HashMap<(usize, String), Reg>,
    /// The insertion region of each kept constant.
    targets: HashMap<u32, usize>,
    /// The kept constants not already at their region's entry.
    moved: HashSet<u32>,
    /// Each erased constant's register, to the kept one of its value.
    duplicates: HashMap<u32, Reg>,
}

impl Plan<'_> {
    /// Walk `block`, the block of the insertion region `scope` (or a nested
    /// region, `usize::MAX`), whose innermost enclosing `comptime` region is
    /// `comptime`.
    fn scope(&mut self, block: &MirBlock, scope: usize, comptime: Option<usize>) {
        let mut at_entry = true;
        for instruction in &block.instrs {
            let target = self.target(instruction, comptime.unwrap_or(0));
            at_entry &= target == Some(scope);
            if let (Some(target), MirInstr::Const { dest, k }) = (target, instruction) {
                let key = (target, constant_key(k, &self.reg_types[&dest.0]));
                if let Some(kept) = self.canonical.get(&key) {
                    self.duplicates.insert(dest.0, *kept);
                } else {
                    self.canonical.insert(key, *dest);
                    self.targets.insert(dest.0, target);
                    if !at_entry {
                        self.moved.insert(dest.0);
                    }
                }
                continue;
            }
            let comptime_regions = matches!(
                instruction,
                MirInstr::ComptimeIf { .. } | MirInstr::ComptimeFor { .. }
            );
            for region in instruction_regions(instruction) {
                if comptime_regions {
                    let inner = self.scopes;
                    self.scopes += 1;
                    self.scope(region, inner, Some(inner));
                } else {
                    self.scope(region, usize::MAX, comptime);
                }
            }
        }
    }

    /// The insertion region `instruction` materializes in when it is an
    /// eligible `const`: the body, or `comptime`, the innermost `comptime`
    /// region holding it, for a constant that region may give meaning.
    fn target(&self, instruction: &MirInstr, comptime: usize) -> Option<usize> {
        let MirInstr::Const { dest, k } = instruction else {
            return None;
        };
        let ty = self.reg_types.get(&dest.0)?;
        let eligible = self.definitions.get(dest.0 as usize) == Some(&1)
            && self.only.is_none_or(|only| only.contains(&dest.0))
            && self.declarations.copy_is_identity(ty);
        let closed = ty.is_closed()
            && !matches!(
                k,
                Const::Param(_) | Const::FunctionInstance { .. } | Const::UninitMem
            );
        eligible.then_some(if closed { 0 } else { comptime })
    }
}

/// The key a constant is uniqued under: its value, its result type, and a
/// parameter expression's identity.
fn constant_key(k: &Const, ty: &Ty) -> String {
    format!("{k:?} : {ty:?} #{}", parameter_identity(k))
}

/// Remove each `const` of `block` and its regions defining one of `moved`,
/// pushing them in walk order.
fn take_moved(block: &mut MirBlock, moved: &HashSet<u32>, taken: &mut Vec<Vec<MirInstr>>) {
    let mut here = Vec::new();
    let mut kept = Vec::with_capacity(block.instrs.len());
    for mut instruction in std::mem::take(&mut block.instrs) {
        if matches!(&instruction, MirInstr::Const { dest, .. } if moved.contains(&dest.0)) {
            here.push(instruction);
            continue;
        }
        if !here.is_empty() {
            taken.push(std::mem::take(&mut here));
        }
        for region in instruction_regions_mut(&mut instruction) {
            take_moved(region, moved, taken);
        }
        kept.push(instruction);
    }
    taken.push(here);
    block.instrs = kept;
}

/// Insert each insertion region's materialized constants after the
/// constants already at its entry, numbering the regions as
/// [`Plan::scope`] does.
fn place(
    block: &mut MirBlock,
    targets: &HashMap<u32, usize>,
    materialized: &mut HashMap<usize, Vec<MirInstr>>,
    next: &mut usize,
) {
    let scope = *next;
    *next += 1;
    place_in(block, scope, targets, materialized, next);
}

/// Place the constants of the insertion region `scope` in its block
/// `block`, and walk its regions.
fn place_in(
    block: &mut MirBlock,
    scope: usize,
    targets: &HashMap<u32, usize>,
    materialized: &mut HashMap<usize, Vec<MirInstr>>,
    next: &mut usize,
) {
    for instruction in &mut block.instrs {
        let comptime_regions = matches!(
            instruction,
            MirInstr::ComptimeIf { .. } | MirInstr::ComptimeFor { .. }
        );
        for region in instruction_regions_mut(instruction) {
            if comptime_regions {
                place(region, targets, materialized, next);
            } else {
                place_in(region, usize::MAX, targets, materialized, next);
            }
        }
    }
    if let Some(constants) = materialized.remove(&scope) {
        let entry = block
            .instrs
            .iter()
            .take_while(|instruction| {
                matches!(instruction, MirInstr::Const { dest, .. }
                    if targets.get(&dest.0) == Some(&scope))
            })
            .count();
        block.instrs.splice(entry..entry, constants);
    }
}

/// Clear the in-place mark of each binding of `block` whose source is one
/// of `constants`.
fn clear_in_place(block: &mut MirBlock, constants: &HashSet<u32>) {
    for instruction in &mut block.instrs {
        if let MirInstr::DefVar { src, in_place, .. } = instruction
            && constants.contains(&src.0)
        {
            *in_place = false;
        }
    }
}

/// [`clear_in_place`] over `block` and its regions.
fn clear_in_place_everywhere(block: &mut MirBlock, constants: &HashSet<u32>) {
    clear_in_place(block, constants);
    for region in block.instrs.iter_mut().flat_map(instruction_regions_mut) {
        clear_in_place_everywhere(region, constants);
    }
}
