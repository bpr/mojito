//! `hoist-trivial-invariants`: upstream's `HoistTrivialInvariants`
//! (`lib/Transforms/HoistTrivialInvariants.cpp`), which the function
//! pipeline `AutomaticInline` nests runs after its first `CSEPass`.
//!
//! The walk visits a function's regions outside in. For each region of a
//! `loop`, an `if` (its `then`, each `elif`'s condition and arm, and its
//! `else`), or a `try` (its body, handler, and `else`), each instruction
//! directly in the region that is pure and holds no region, and whose
//! operands are all defined outside the instruction holding the region, is
//! moved to the earliest point its operands allow: just after the
//! definition of the operand every other operand's definition dominates,
//! or to the start of the region whose argument that operand is, or to the
//! function's entry when it has no operand. An instruction moved out of an
//! inner region is not visited again for the region it lands in, as
//! upstream's pre-order walk has passed it; one moved out first is an
//! operand defined outside for those after it, so a chain of invariants
//! moves together.
//!
//! An instruction is pure when it is free of effects, as `cse` reads
//! MLIR's `MemoryEffectOpInterface` ([`free_of_effects`]), and cannot trap,
//! as `isPure` also asks an op to be speculatable: a division or
//! remainder, an LLVM intrinsic, an element or lane access the backend
//! bounds-checks, or a conversion to an integer that a compile-time run
//! checks is not moved, since moving it out of an arm not taken, or out of
//! a loop that never runs, would trap where the program does not.
//!
//! MIR is not SSA: an instruction moves only when its result and each of
//! its operands has one definition, the result of a register-passable
//! trivial type or a reference handle, as `cse` merges only such a result,
//! since every iteration of a loop then reads the one value.

use super::canonicalize::definition_counts;
use super::cse::{free_of_effects, region_arguments};
use super::pre_elaboration::BodyPass;
use mojito_ast::ast::InfixOp;
use mojito_common::timing;
use mojito_mir::mir::verify::instruction_result_regs;
use mojito_mir::mir::{
    MirBlock, MirDeclarations, MirFunction, MirInstr, Reg, instruction_regions_mut,
    instruction_regs_mut,
};
use mojito_types::ct::ScalarConversion;
use mojito_types::mlir::PopOp;
use mojito_types::types::{Ty, TyKind};
use std::collections::HashMap;

/// The pass, under upstream's flag spelling.
pub const HOIST_TRIVIAL_INVARIANTS: BodyPass = BodyPass {
    name: "hoist-trivial-invariants",
    run: hoist_trivial_invariants,
};

/// Move each trivially invariant instruction of `function` out of the
/// `loop`, `if`, and `try` regions holding it; returns whether any moved.
pub fn hoist_trivial_invariants(
    _name: &str,
    function: &mut MirFunction,
    declarations: &MirDeclarations,
) -> bool {
    let definitions = definition_counts(&mut function.blocks, function.n_regs);
    let mut walk = Walk {
        declarations,
        reg_types: &function.reg_types,
        definitions,
        points: HashMap::new(),
        scope: Vec::new(),
        fresh: 0,
        hoisted: 0,
    };
    if let Some(entry) = function.blocks.first_mut() {
        walk.block(entry, 0, false, &[]);
    }
    timing::count("hoisted_instructions", walk.hoisted);
    walk.hoisted > 0
}

/// Where a register is defined on the walk's path: the depth of the region
/// holding its definition (the function's body is 0), then the definition's
/// order within that region's block.
///
/// Every definition on the path dominates the walk's position, and of two,
/// the deeper one, or the later at one depth, is dominated by the other, so
/// the greatest point of an instruction's operands is the earliest one it
/// may move to.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Point {
    depth: usize,
    /// `[0]` is the block's start, where its region's arguments are
    /// defined; an instruction of the block in its original order is
    /// `[n]` from 1; one moved there just after the point `key` is `key`
    /// followed by a number greater than any before it, so it orders after
    /// that point, after what moved there before it, and before the next
    /// instruction.
    key: Vec<u32>,
}

/// An instruction moved out of its region, on its way to the block at the
/// point it lands after.
struct Hoisted {
    point: Point,
    instruction: MirInstr,
}

/// The walk's state.
struct Walk<'f> {
    declarations: &'f MirDeclarations,
    reg_types: &'f HashMap<u32, Ty>,
    /// How many times the function defines each register.
    definitions: Vec<u32>,
    /// Where each register defined on the walk's path is defined.
    points: HashMap<u32, Point>,
    /// The registers added to `points`, in order, so a region's own are
    /// removed when the walk leaves it.
    scope: Vec<u32>,
    /// The last number a moved instruction's point was given.
    fresh: u32,
    /// How many instructions moved.
    hoisted: u64,
}

impl Walk<'_> {
    /// Walk `block`, a region's one block at `depth` whose arguments are
    /// `arguments`: move each invariant instruction out when `hoists` (the
    /// region is one upstream hoists from), walk each instruction's
    /// regions, and put each instruction moved out of them that lands in
    /// this block in place. Returns the moved instructions that land in an
    /// enclosing block.
    fn block(
        &mut self,
        block: &mut MirBlock,
        depth: usize,
        hoists: bool,
        arguments: &[Reg],
    ) -> Vec<Hoisted> {
        let mark = self.scope.len();
        for &argument in arguments {
            self.define(
                argument,
                Point {
                    depth,
                    key: vec![0],
                },
            );
        }
        let mut keys: Vec<Vec<u32>> = Vec::new();
        let mut outward = Vec::new();
        let mut ordinal = 0;
        let mut index = 0;
        while index < block.instrs.len() {
            if hoists && let Some((dest, point)) = self.invariant(&block.instrs[index], depth) {
                let instruction = block.instrs.remove(index);
                self.fresh += 1;
                let mut key = point.key;
                key.push(self.fresh);
                let point = Point {
                    depth: point.depth,
                    key,
                };
                self.define(dest, point.clone());
                self.hoisted += 1;
                outward.push(Hoisted { point, instruction });
                continue;
            }
            ordinal += 1;
            let arrived = self.regions(&mut block.instrs[index], depth);
            let instruction = &block.instrs[index];
            let mut results = Vec::new();
            instruction_result_regs(instruction, &mut results);
            let arguments = region_arguments(instruction);
            results.retain(|reg| !arguments.iter().any(|args| args.contains(reg)));
            for result in results {
                self.define(
                    result,
                    Point {
                        depth,
                        key: vec![ordinal],
                    },
                );
            }
            keys.push(vec![ordinal]);
            for hoisted in arrived {
                if hoisted.point.depth == depth {
                    let at = keys.partition_point(|key| *key < hoisted.point.key);
                    keys.insert(at, hoisted.point.key);
                    block.instrs.insert(at, hoisted.instruction);
                    index += 1;
                } else {
                    outward.push(hoisted);
                }
            }
            index += 1;
        }
        for reg in self.scope.drain(mark..) {
            self.points.remove(&reg);
        }
        outward
    }

    /// Walk each region of `instruction`, at `depth`, one deeper; returns
    /// what moved out of them.
    fn regions(&mut self, instruction: &mut MirInstr, depth: usize) -> Vec<Hoisted> {
        let hoists = hoists_from(instruction);
        let arguments: Vec<Vec<Reg>> = region_arguments(instruction)
            .into_iter()
            .map(<[Reg]>::to_vec)
            .collect();
        let mut arrived = Vec::new();
        for (region, arguments) in instruction_regions_mut(instruction)
            .into_iter()
            .zip(&arguments)
        {
            if let Some(block) = region.first_mut() {
                arrived.extend(self.block(block, depth + 1, hoists, arguments));
            }
        }
        arrived
    }

    /// The result of `instruction`, directly in a region at `depth`, and
    /// the point it may move to, when it is invariant there: pure, its
    /// result one register-like value defined once, and each operand
    /// defined once on the walk's path outside the region's holder.
    fn invariant(&self, instruction: &MirInstr, depth: usize) -> Option<(Reg, Point)> {
        if !free_of_effects(instruction, self.reg_types) || may_trap(instruction) {
            return None;
        }
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
        let mut operands = instruction.clone();
        let mut point = Point {
            depth: 0,
            key: vec![0],
        };
        for reg in instruction_regs_mut(&mut operands) {
            if *reg == dest {
                continue;
            }
            if self.definitions(*reg) != 1 {
                return None;
            }
            let defined = self.points.get(&reg.0)?;
            if defined.depth >= depth {
                return None;
            }
            if *defined > point {
                point = defined.clone();
            }
        }
        Some((dest, point))
    }

    /// Record that `reg` is defined at `point` on the walk's path.
    fn define(&mut self, reg: Reg, point: Point) {
        if self.points.insert(reg.0, point).is_none() {
            self.scope.push(reg.0);
        }
    }

    /// How many times the function defines `reg`.
    fn definitions(&self, reg: Reg) -> u32 {
        self.definitions.get(reg.0 as usize).copied().unwrap_or(0)
    }
}

/// Whether upstream hoists out of the regions of `instruction`: an
/// `hlcf.loop`, an `hlcf.if`, or a `lit.try`.
const fn hoists_from(instruction: &MirInstr) -> bool {
    matches!(
        instruction,
        MirInstr::Loop { .. } | MirInstr::If { .. } | MirInstr::Try { .. }
    )
}

/// Whether `instruction`, free of effects, may still trap, and so is not
/// speculatable: a division or remainder, an operator the
/// backend does not define over its operands, an LLVM intrinsic, an
/// element or lane access the backend bounds-checks, or a conversion to an
/// integer a compile-time run checks.
const fn may_trap(instruction: &MirInstr) -> bool {
    match instruction {
        MirInstr::BinOp { op, .. } => !matches!(
            op,
            InfixOp::Add
                | InfixOp::Sub
                | InfixOp::Mul
                | InfixOp::Shl
                | InfixOp::Shr
                | InfixOp::BitAnd
                | InfixOp::BitOr
                | InfixOp::BitXor
                | InfixOp::Eq
                | InfixOp::Ne
                | InfixOp::Lt
                | InfixOp::Gt
                | InfixOp::Le
                | InfixOp::Ge
        ),
        MirInstr::Pop { op, .. } => matches!(op, PopOp::Div | PopOp::Rem),
        MirInstr::ScalarCast { target, .. } => {
            matches!(target, ScalarConversion::Int | ScalarConversion::UInt)
        }
        MirInstr::CallLlvmIntrinsic { .. }
        | MirInstr::ArrayGet { .. }
        | MirInstr::ArrayReplace { .. }
        | MirInstr::ArrayGep { .. }
        | MirInstr::SimdExtract { .. }
        | MirInstr::SimdInsert { .. }
        | MirInstr::SimdShuffle { .. } => true,
        _ => false,
    }
}
