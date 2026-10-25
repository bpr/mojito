//! `cse`: MLIR's `CSE` pass, which upstream runs on each generator in its
//! last pre-elaboration round, after `RemoveUnusedParams` and
//! `EliminateDeadSymbols`, and which its `'kgen.func'` pipeline runs last,
//! after `Canonicalizer`, on every function `ElaborateGenerators` emits.
//!
//! As in MLIR, the pass walks each region's dominator tree, which for a
//! region's one block is that block, with a scoped table of the
//! instructions available at each block: an instruction free
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
//! value register, a SIMD construction, cast, or shuffle, or a built-in
//! operator (no operator method, every operand a built-in scalar or SIMD
//! type), as MLIR's arithmetic and comparisons are. A duplicate of a
//! trapping division is dominated by the identical division, which traps
//! first. A read of storage or through a reference (a `var.use` borrow,
//! `place.load`, `ref.make`, `ref.read`, a field read through a reference)
//! has only a read effect, as does a `var.use` copy of a slot of a
//! register-passable trivial type, which upstream reads with a load. As in
//! MLIR, a read matches only an instruction in its own block with no
//! instruction between that may write: anything but an instruction free of
//! effects, a read, a declaration, or a copy or drop of a register-passable
//! trivial value that runs no constructor or destructor (which upstream does
//! not emit), where an instruction holding regions writes when its own kind
//! does (a `try`) or what its regions hold does. A read that misses becomes the one later
//! reads match.
//!
//! Only a result of a register-passable trivial type
//! ([`MirDeclarations::copy_is_identity`]) or a reference handle is merged,
//! as upstream's parser holds only such a value in an SSA value. Its
//! readers may consume it: a register is a value no reader writes, so an
//! owned parameter's callee and an `UninitStorage` initialization copy it.
//! A value of any other type may own what a consuming reader takes. Each
//! merged result and every operand of
//! the matched instructions has one definition, which the walk has passed
//! on the dominating path: MIR does not verify that a definition dominates
//! its reads, and an operand redefined between the two would hold another
//! value.

use super::canonicalize::{definition_counts, forward};
use super::pre_elaboration::BodyPass;
use mojito_common::timing;
use mojito_hir::hir::VarId;
use mojito_mir::mir::verify::instruction_result_regs;
use mojito_mir::mir::{
    Const, MirBlock, MirDeclarations, MirFunction, MirInstr, Reg, UseMode, instruction_regions,
    instruction_regs_mut,
};
use mojito_types::types::{Ty, TyKind, trivially_register_passable_leaf};
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

/// The pass, under MLIR's flag spelling.
pub const CSE: BodyPass = BodyPass {
    name: "cse",
    run: cse,
};

/// Erase each duplicate instruction of `function`; returns whether any was.
///
/// A duplicate is an instruction free of effects that a dominating one
/// matches, or a read that one earlier in its block matches with no write
/// between; its result's reads are renamed to the matched one's result.
pub fn cse(_name: &str, function: &mut MirFunction, declarations: &MirDeclarations) -> bool {
    let definitions = definition_counts(&mut function.blocks, function.n_regs);
    let mut table = Table {
        declarations,
        reg_types: &function.reg_types,
        var_types: &function.var_tys,
        definitions,
        available: HashMap::new(),
        defined: HashSet::new(),
        scope: Vec::new(),
        replaced: HashMap::new(),
        blocks: 0,
        writes: 0,
    };
    table.block_list(&function.blocks);
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
    declarations: &'f MirDeclarations,
    reg_types: &'f HashMap<u32, Ty>,
    var_types: &'f HashMap<VarId, Ty>,
    /// How many times the function defines each register.
    definitions: Vec<u32>,
    /// The available instruction under each key.
    available: HashMap<String, Available>,
    /// The registers defined on the path from the entry to the walk's
    /// position in the dominator tree.
    defined: HashSet<u32>,
    /// What the walk added to `available` and `defined`, in order, so a
    /// scope's own are removed when the walk leaves it.
    scope: Vec<Added>,
    /// Each erased instruction's result, to the result its reads now name.
    replaced: HashMap<u32, Reg>,
    /// How many blocks the walk has entered, each block's number.
    blocks: usize,
    /// How many instructions that may write the walk has passed.
    writes: u64,
}

impl Table<'_> {
    /// Walk `blocks`, a region's one block, in a scope of its own: the
    /// dominator tree of a one-block region is its block.
    fn block_list(&mut self, blocks: &[MirBlock]) {
        if let Some(block) = blocks.first() {
            let mark = self.scope.len();
            self.block(block);
            self.leave(mark);
        }
    }

    /// Match or record each instruction of `block` in order, walking each
    /// region in a scope of its own that its arguments are defined in, and
    /// count each instruction or terminator that may write.
    fn block(&mut self, block: &MirBlock) {
        let number = self.blocks;
        self.blocks += 1;
        for instruction in &block.instrs {
            let arguments = region_arguments(instruction);
            for (region, args) in instruction_regions(instruction).into_iter().zip(&arguments) {
                let mark = self.scope.len();
                self.define(args.iter().copied());
                self.block_list(region);
                self.leave(mark);
            }
            match self.effect(instruction) {
                Effect::Free => self.match_or_record(instruction, None),
                Effect::Read => self.match_or_record(instruction, Some((number, self.writes))),
                Effect::Write => self.writes += 1,
                Effect::Nested => {}
            }
            let mut results = Vec::new();
            instruction_result_regs(instruction, &mut results);
            results.retain(|reg| !arguments.iter().any(|args| args.contains(reg)));
            self.define(results);
        }
    }

    /// Erase `instruction` when an available instruction matches it, or make
    /// it the available one under its key; `read` is the block number and
    /// write count a read is recorded at, which a read it matches must share.
    fn match_or_record(&mut self, instruction: &MirInstr, read: Option<(usize, u64)>) {
        let Some((dest, key)) = self.key(instruction) else {
            return;
        };
        match self.available.get(&key) {
            Some(available) if available.read == read => {
                self.replaced.insert(dest.0, available.result);
            }
            _ => {
                let shadowed = self
                    .available
                    .insert(key.clone(), Available { result: dest, read });
                self.scope.push(Added::Key(key, shadowed));
            }
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
                Added::Key(key, shadowed) => {
                    match shadowed {
                        Some(shadowed) => self.available.insert(key, shadowed),
                        None => self.available.remove(&key),
                    };
                }
                Added::Defined(reg) => {
                    self.defined.remove(&reg);
                }
            }
        }
    }

    /// The result of `instruction` and the key it matches under, when its
    /// result and operands qualify.
    ///
    /// The key spells the instruction with its result blanked and each
    /// operand an erased instruction defined renamed, then the result's
    /// type, as MLIR's `OperationEquivalence` compares the name, the
    /// attributes, the operands, and the result types.
    fn key(&self, instruction: &MirInstr) -> Option<(Reg, String)> {
        let mut results = Vec::new();
        instruction_result_regs(instruction, &mut results);
        let [dest] = results[..] else {
            return None;
        };
        let ty = self.reg_types.get(&dest.0)?;
        if !(self.declarations.copy_is_identity(ty) || matches!(ty.kind(), TyKind::Ref(_)))
            || self.definitions(dest) != 1
        {
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
        let identity = match instruction {
            MirInstr::Const { k, .. } | MirInstr::ParamListAddress { values: k, .. } => {
                parameter_identity(k)
            }
            _ => 0,
        };
        Some((dest, format!("{spelled:?} : {ty:?} #{identity}")))
    }

    /// What `instruction` may do to memory, as MLIR's
    /// `MemoryEffectOpInterface` declares it.
    fn effect(&self, instruction: &MirInstr) -> Effect {
        let trivial = |ty: Option<&Ty>| ty.is_some_and(|ty| self.declarations.copy_is_identity(ty));
        if self.free_of_effects(instruction) {
            return Effect::Free;
        }
        match instruction {
            MirInstr::If { .. }
            | MirInstr::Loop { .. }
            | MirInstr::ComptimeIf { .. }
            | MirInstr::ComptimeFor { .. }
            | MirInstr::Match { .. }
            | MirInstr::DeclareVar { .. } => Effect::Nested,
            MirInstr::CopyValue {
                dest,
                constructor: None,
                ..
            } if trivial(self.reg_types.get(&dest.0)) => Effect::Nested,
            MirInstr::DropVar {
                var,
                destructor: None,
            } if trivial(self.var_types.get(var)) => Effect::Nested,
            MirInstr::UseVar {
                var,
                mode: UseMode::Copy,
                constructor: None,
                ..
            } if trivial(self.var_types.get(var)) => Effect::Read,
            _ if instruction.would_be_trivially_dead() => Effect::Read,
            _ => Effect::Write,
        }
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
            | MirInstr::VariantTag { .. }
            | MirInstr::MakeSimd { .. }
            | MirInstr::SimdCast { .. }
            | MirInstr::SimdBitcast { .. }
            | MirInstr::SimdShuffle { .. } => true,
            MirInstr::GetField { base, .. } => self
                .reg_types
                .get(&base.0)
                .is_some_and(|ty| !matches!(ty.kind(), TyKind::Ref(_))),
            MirInstr::BinOp {
                a,
                b,
                resolved: None,
                raises: None,
                ..
            } => self.built_in(&[*a, *b]),
            MirInstr::UnOp {
                a, raises: None, ..
            } => self.built_in(&[*a]),
            _ => false,
        }
    }

    /// Whether every one of `regs` has a built-in scalar or SIMD type, so an
    /// operator over them is the backend's own arithmetic or comparison
    /// rather than a call of an operator method.
    fn built_in(&self, regs: &[Reg]) -> bool {
        regs.iter().all(|reg| {
            self.reg_types
                .get(&reg.0)
                .is_some_and(|ty| trivially_register_passable_leaf(ty.kind()))
        })
    }

    /// How many times the function defines `reg`.
    fn definitions(&self, reg: Reg) -> u32 {
        self.definitions.get(reg.0 as usize).copied().unwrap_or(0)
    }
}

/// The identity hash of a parameter constant's expression, which its
/// spelling leaves out: two `comptime for` binders spelled alike are
/// distinct attributes, as upstream's parameter attributes are. Any other
/// constant is its spelling, `0`.
fn parameter_identity(k: &Const) -> u64 {
    let mut state = std::collections::hash_map::DefaultHasher::new();
    match k {
        Const::Param(expr) => expr.hash(&mut state),
        Const::Value(value) => mojito_types::param_expr::identity_hash(value, &mut state),
        _ => return 0,
    }
    state.finish()
}

/// An entry of a scope of the walk: a key made available, with the entry it
/// shadows, or a register defined.
enum Added {
    Key(String, Option<Available>),
    Defined(u32),
}

/// An instruction available under its key: its result, and for a read the
/// number of its block and the write count when the walk passed it.
#[derive(Clone, Copy)]
struct Available {
    result: Reg,
    read: Option<(usize, u64)>,
}

/// What an instruction may do to memory.
enum Effect {
    /// Nothing: it reads only its operands.
    Free,
    /// It reads storage or through a reference, and writes nothing.
    Read,
    /// It may write.
    Write,
    /// Nothing of its own, besides what any region it holds does, which the
    /// walk counts: a structured control-flow op, a declaration, or a copy
    /// or drop that runs no code.
    Nested,
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
        MirInstr::Try {
            handler_args,
            handler,
            else_args,
            orelse,
            ..
        } => std::iter::once(&[][..])
            .chain(handler.as_ref().map(|_| &handler_args[..]))
            .chain(orelse.as_ref().map(|_| &else_args[..]))
            .collect(),
        _ => vec![&[]; instruction_regions(instruction).len()],
    }
}
