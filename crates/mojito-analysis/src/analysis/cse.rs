//! `cse`: MLIR's `CSE` pass, which upstream runs on each generator in its
//! last pre-elaboration round, after `RemoveUnusedParams` and
//! `EliminateDeadSymbols`, and which its `'kgen.func'` pipeline runs last,
//! after `Canonicalizer`, on every function `ElaborateGenerators` emits.
//!
//! As in MLIR, the pass walks each block list's dominator tree with a scoped
//! table of the instructions available at each block: an instruction free
//! of effects that matches one in the table, with the same mnemonic,
//! attributes, operands, and result type, is erased and its result's reads
//! name the dominating instruction's result. A region's blocks see what is
//! available at its instruction and the region's own arguments, and what
//! they define does not leave it, so a structured body's sibling regions
//! never share an instruction.
//!
//! An instruction is free of effects when it reads nothing but its
//! operands: a constant, a literal materialization, a size or type-name
//! query, a parameter list's address, a field or variant-tag read of a
//! value register, or a SIMD construction, cast, or shuffle. A read of
//! storage or through a reference (`var.use` borrows, `place.load`,
//! `ref.read`, a field read through a reference) has a read effect, which
//! MLIR merges only with no write between; those stay.
//!
//! Only a result held whole in one register
//! ([`mojito_types::types::copy_is_identity`]) is merged, since a reader may
//! consume a value held in storage. Each merged result and every operand of
//! the matched instructions has one definition, which the walk has passed
//! on the dominating path: MIR does not verify that a definition dominates
//! its reads, and an operand redefined between the two would hold another
//! value.

use super::canonicalize::{definition_counts, forward};
use super::pre_elaboration::BodyPass;
use mojito_common::timing;
use mojito_mir::mir::verify::instruction_result_regs;
use mojito_mir::mir::{
    MirBlock, MirBlockId, MirDeclarations, MirFunction, MirInstr, Reg, immediate_dominators,
    instruction_regions, instruction_regs_mut,
};
use mojito_types::types::{Ty, TyKind, copy_is_identity};
use std::collections::{HashMap, HashSet};

/// The pass, under MLIR's flag spelling.
pub const CSE: BodyPass = BodyPass {
    name: "cse",
    run: cse,
};

/// Erase each instruction of `function` free of effects that a dominating
/// instruction matches, its result's reads renamed to that instruction's
/// result; returns whether any was.
pub fn cse(_name: &str, function: &mut MirFunction, _declarations: &MirDeclarations) -> bool {
    let definitions = definition_counts(&mut function.blocks, function.n_regs);
    let mut table = Table {
        reg_types: &function.reg_types,
        definitions,
        available: HashMap::new(),
        defined: HashSet::new(),
        scope: Vec::new(),
        replaced: HashMap::new(),
    };
    table.block_list(&function.blocks, true);
    let replaced = table.replaced;
    timing::count("erased_instructions", replaced.len() as u64);
    if replaced.is_empty() {
        return false;
    }
    forward(&mut function.blocks, &replaced);
    function
        .reg_types
        .retain(|reg, _| !replaced.contains_key(reg));
    function
        .spans
        .0
        .retain(|reg, _| !replaced.contains_key(reg));
    true
}

/// The scoped table of the instructions available on a dominator-tree walk.
struct Table<'f> {
    reg_types: &'f HashMap<u32, Ty>,
    /// How many times the function defines each register.
    definitions: Vec<u32>,
    /// The result of the available instruction under each key.
    available: HashMap<String, Reg>,
    /// The registers defined on the path from the entry to the walk's
    /// position in the dominator tree.
    defined: HashSet<u32>,
    /// What the walk added to `available` and `defined`, in order, so a
    /// scope's own are removed when the walk leaves it.
    scope: Vec<Added>,
    /// Each erased instruction's result, to the result its reads now name.
    replaced: HashMap<u32, Reg>,
}

impl Table<'_> {
    /// Walk the dominator tree of `blocks` from its entry; `function_level`
    /// says `blocks` is the function's list, whose regions' escapes are
    /// edges too.
    fn block_list(&mut self, blocks: &[MirBlock], function_level: bool) {
        if blocks.is_empty() {
            return;
        }
        let children = dominator_children(blocks, function_level);
        let mut work = vec![Visit::Enter(0)];
        while let Some(visit) = work.pop() {
            match visit {
                Visit::Enter(at) => {
                    work.push(Visit::Leave(self.scope.len()));
                    self.block(&blocks[at]);
                    work.extend(children[at].iter().rev().map(|&child| Visit::Enter(child)));
                }
                Visit::Leave(mark) => self.leave(mark),
            }
        }
    }

    /// Match or record each instruction of `block` in order, walking each
    /// region in a scope of its own that its arguments are defined in.
    fn block(&mut self, block: &MirBlock) {
        for instruction in &block.instrs {
            let arguments = region_arguments(instruction);
            for (region, args) in instruction_regions(instruction).into_iter().zip(&arguments) {
                let mark = self.scope.len();
                self.define(args.iter().copied());
                self.block_list(region, false);
                self.leave(mark);
            }
            if let Some((dest, key)) = self.key(instruction) {
                if let Some(&dominating) = self.available.get(&key) {
                    self.replaced.insert(dest.0, dominating);
                } else {
                    self.available.insert(key.clone(), dest);
                    self.scope.push(Added::Key(key));
                }
            }
            let mut results = Vec::new();
            instruction_result_regs(instruction, &mut results);
            results.retain(|reg| !arguments.iter().any(|args| args.contains(reg)));
            self.define(results);
        }
    }

    /// Add `regs` to the registers defined on the walk's path.
    fn define(&mut self, regs: impl IntoIterator<Item = Reg>) {
        for reg in regs {
            if self.defined.insert(reg.0) {
                self.scope.push(Added::Defined(reg.0));
            }
        }
    }

    /// Remove what the walk added since the scope length was `mark`.
    fn leave(&mut self, mark: usize) {
        for added in self.scope.drain(mark..) {
            match added {
                Added::Key(key) => {
                    self.available.remove(&key);
                }
                Added::Defined(reg) => {
                    self.defined.remove(&reg);
                }
            }
        }
    }

    /// The result of `instruction` and the key it matches under, when it is
    /// free of effects and its result and operands qualify.
    ///
    /// The key spells the instruction with its result blanked and each
    /// operand an erased instruction defined renamed, then the result's
    /// type, as MLIR's `OperationEquivalence` compares the name, the
    /// attributes, the operands, and the result types.
    fn key(&self, instruction: &MirInstr) -> Option<(Reg, String)> {
        if !self.free_of_effects(instruction) {
            return None;
        }
        let mut results = Vec::new();
        instruction_result_regs(instruction, &mut results);
        let [dest] = results[..] else {
            return None;
        };
        let ty = self.reg_types.get(&dest.0)?;
        if !copy_is_identity(ty.kind()) || self.definitions(dest) != 1 {
            return None;
        }
        let mut spelled = instruction.clone();
        for reg in instruction_regs_mut(&mut spelled) {
            if *reg == dest {
                *reg = Reg(u32::MAX);
            } else if self.definitions(*reg) != 1 || !self.defined.contains(&reg.0) {
                return None;
            } else if let Some(renamed) = self.replaced.get(&reg.0) {
                *reg = *renamed;
            }
        }
        Some((dest, format!("{spelled:?} : {ty:?}")))
    }

    /// Whether `instruction` reads nothing but its operands.
    fn free_of_effects(&self, instruction: &MirInstr) -> bool {
        match instruction {
            MirInstr::Const { .. }
            | MirInstr::MaterializeLiteral { .. }
            | MirInstr::SizeOf { .. }
            | MirInstr::TypeName { .. }
            | MirInstr::ParamListAddress { .. }
            | MirInstr::VariantIs { .. }
            | MirInstr::MakeSimd { .. }
            | MirInstr::SimdCast { .. }
            | MirInstr::SimdBitcast { .. }
            | MirInstr::SimdShuffle { .. } => true,
            MirInstr::GetField { base, .. } => self
                .reg_types
                .get(&base.0)
                .is_some_and(|ty| !matches!(ty.kind(), TyKind::Ref(_))),
            _ => false,
        }
    }

    /// How many times the function defines `reg`.
    fn definitions(&self, reg: Reg) -> u32 {
        self.definitions.get(reg.0 as usize).copied().unwrap_or(0)
    }
}

/// An entry of a scope of the walk: a key made available, or a register
/// defined.
enum Added {
    Key(String),
    Defined(u32),
}

/// A step of the dominator-tree walk: a block to enter, or the scope length
/// to restore on leaving one.
enum Visit {
    Enter(MirBlockId),
    Leave(usize),
}

/// The arguments of each region of `instruction`, in
/// [`instruction_regions`]' order.
fn region_arguments(instruction: &MirInstr) -> Vec<&[Reg]> {
    match instruction {
        MirInstr::If {
            elifs, else_args, ..
        } => std::iter::once(&[][..])
            .chain(
                elifs
                    .iter()
                    .flat_map(|elif| [&elif.cond_args[..], &elif.then_args[..]]),
            )
            .chain(std::iter::once(&else_args[..]))
            .collect(),
        MirInstr::Loop { args, .. } => vec![args],
        MirInstr::ComptimeFor { args, .. } => vec![args, &[]],
        MirInstr::Match {
            cases, else_args, ..
        } => cases
            .iter()
            .map(|case| &case.args[..])
            .chain(std::iter::once(&else_args[..]))
            .collect(),
        _ => vec![&[]; instruction_regions(instruction).len()],
    }
}

/// Each block's children in the dominator tree of `blocks`, rooted at the
/// entry; a block the entry does not reach is no one's child
/// ([`immediate_dominators`]).
fn dominator_children(blocks: &[MirBlock], function_level: bool) -> Vec<Vec<MirBlockId>> {
    let idom = immediate_dominators(blocks, function_level);
    let mut children = vec![Vec::new(); blocks.len()];
    for (block, dominator) in idom.iter().enumerate().skip(1) {
        if let Some(dominator) = dominator {
            children[*dominator].push(block);
        }
    }
    children
}
