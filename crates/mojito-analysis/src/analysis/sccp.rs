//! `sccp`: upstream's `SCCP`, MLIR's sparse conditional constant
//! propagation, which its pre-elaboration pipeline runs between `Mem2Reg`
//! and a `Canonicalizer` on each generator.
//!
//! Each register takes a lattice value: unknown, one constant, or
//! overdefined. Blocks become executable from the entry, a `branch` making
//! only its taken arm executable while its condition is one constant. A
//! structured body is walked region by region, as `SCCP` walks `hlcf`: an
//! `if` without elifs enters only the arm a constant condition takes, its
//! results meeting the values its executable arms yield, and a loop's
//! results meet the values its executable `break`s carry, while a loop's
//! body, every region of an `if` with elifs, a `comptime if`, a `match`,
//! and a `try` are executable, the other ops' results overdefined. The
//! instructions of executable blocks are evaluated until nothing
//! changes: a scalar or SIMD `const` ([`Const::is_folded`], a `UInt` or
//! SIMD `Const::Value` included) is its constant, an instruction
//! that folds ([`MirInstr::folded_constant`]) over constant operands is the
//! folded constant, one with an unknown operand stays unknown, and every
//! other result is overdefined. A literal materialized to `UInt` or a scalar
//! SIMD type, a `simd.make` at any known width of one literal (the splat)
//! or of one constant element per lane (`ct::simd_construction_lanes`), and
//! an operator over `UInt` or SIMD constants fold to a
//! `Const::Value`, lanes by the VM's own lane rule. A built-in scalar
//! conversion (`UInt(x)`, `Float64(x)`, `Bool(x)`) over a constant folds by
//! the VM's conversion rule, as `SCCP` folds the inlined `pop.cast`. A parameter read (`Const::Param`) and
//! an aggregate `Const::Value` are overdefined: the first is the
//! elaborator's to fold, per instance.
//!
//! Each instruction that folds over the final lattice becomes a `const`
//! through the replacement `canonicalize` shares (`fold::replace_folded`).
//! The first `canonicalize` has already folded what straight-line constants
//! decide, so `sccp` adds the folds that need its executable-block reasoning
//! or a lattice meet. Terminators are left alone: the `Canonicalizer` after
//! it folds a `branch` on a constant and decides an `if` on one.

use super::fold;
use super::pre_elaboration::BodyPass;
use mojito_common::timing;
use mojito_mir::mir::verify::instruction_result_regs;
use mojito_mir::mir::{
    Const, LoopLabel, MirBlock, MirBlockId, MirDeclarations, MirFunction, MirInstr, MirTerm, Reg,
    RegionFrame, instruction_regions,
};
use mojito_types::types::Ty;
use std::collections::HashMap;

/// The pass, under MLIR's flag spelling.
pub const SCCP: BodyPass = BodyPass {
    name: "sccp",
    run: sccp,
};

/// Propagate `function`'s constants over its executable blocks and replace
/// each instruction they decide by its constant; returns whether any was.
pub fn sccp(_name: &str, function: &mut MirFunction, declarations: &MirDeclarations) -> bool {
    let mut solver = Solver {
        lattice: vec![Lattice::Unknown; function.n_regs as usize],
        reg_types: &function.reg_types,
        declarations,
        changed: true,
        loops: Vec::new(),
    };
    while std::mem::take(&mut solver.changed) {
        let mut escapes = Vec::new();
        let mut executable = vec![false; function.blocks.len()];
        let mut pending = vec![0];
        while let Some(block) = pending.pop() {
            if block >= function.blocks.len() || std::mem::replace(&mut executable[block], true) {
                continue;
            }
            pending.extend(solver.visit(&function.blocks[block], &mut escapes));
            pending.append(&mut escapes);
        }
    }
    let lattice = solver.lattice;
    let constant = |reg: Reg| match lattice.get(reg.0 as usize) {
        Some(Lattice::Constant(k)) => Some(k.clone()),
        _ => None,
    };
    let folded = fold::replace_folded(
        &mut function.blocks,
        &constant,
        &function.reg_types,
        declarations,
    );
    timing::count("folded_constants", folded as u64);
    folded > 0
}

/// A register's value over the executable blocks.
#[derive(Clone)]
enum Lattice {
    Unknown,
    Constant(Const),
    Overdefined,
}

/// The fixpoint's state: the lattice, whether a walk raised it, and the
/// loops enclosing the block being walked, innermost last, with their
/// results.
struct Solver<'a> {
    lattice: Vec<Lattice>,
    reg_types: &'a HashMap<u32, Ty>,
    declarations: &'a MirDeclarations,
    changed: bool,
    loops: Vec<(Option<LoopLabel>, Vec<Reg>)>,
}

impl Solver<'_> {
    /// Evaluate the executable block `block`, with the executable regions
    /// of its region ops, and return the blocks of its list it makes
    /// executable. Escape targets, which name the function's blocks, go to
    /// `escapes`.
    fn visit(&mut self, block: &MirBlock, escapes: &mut Vec<MirBlockId>) -> Vec<MirBlockId> {
        for instruction in &block.instrs {
            if let MirInstr::If {
                results,
                cond,
                then,
                elifs,
                orelse,
            } = instruction
                && elifs.is_empty()
            {
                self.visit_if(results, *cond, [then, orelse], escapes);
                continue;
            }
            if let MirInstr::Loop {
                label,
                results,
                body,
            } = instruction
            {
                self.loops.push((*label, results.clone()));
                self.visit_region(body, escapes);
                self.loops.pop();
                continue;
            }
            self.transfer(instruction);
            for region in instruction_regions(instruction) {
                self.visit_region(region, escapes);
            }
        }
        match &block.term {
            MirTerm::Break { label, values } => {
                self.resolve_break(*label, values);
                Vec::new()
            }
            MirTerm::Jump(target) => vec![*target],
            MirTerm::Branch {
                cond,
                then_b,
                else_b,
            } => match self.lattice.get(cond.0 as usize) {
                Some(Lattice::Constant(Const::Bool(true))) => vec![*then_b],
                Some(Lattice::Constant(Const::Bool(false))) => vec![*else_b],
                Some(Lattice::Unknown) => Vec::new(),
                _ => vec![*then_b, *else_b],
            },
            MirTerm::ComptimeBranch { then_b, else_b, .. } => vec![*then_b, *else_b],
            MirTerm::ComptimeFor { body, exit, .. } => vec![*body, *exit],
            MirTerm::EscapeJump { target, .. } => {
                escapes.push(*target);
                Vec::new()
            }
            MirTerm::Return(_)
            | MirTerm::ReturnWithCleanup { .. }
            | MirTerm::TryYield
            | MirTerm::Yield(_)
            | MirTerm::ElifCond(_)
            | MirTerm::Continue { .. }
            | MirTerm::ComptimeYield(_)
            | MirTerm::ComptimeBreak
            | MirTerm::ComptimeContinue
            | MirTerm::MatchNext
            | MirTerm::MatchComplete
            | MirTerm::Unreachable => Vec::new(),
        }
    }

    /// Evaluate an `if` without elifs, as `IfOp::getEntryTargets` enters
    /// it: a constant condition makes only its arm executable, an unknown
    /// one neither yet, and any other both. Each result meets the values
    /// the executable arms yield to it.
    fn visit_if(
        &mut self,
        results: &[Reg],
        cond: Reg,
        [then, orelse]: [&[MirBlock]; 2],
        escapes: &mut Vec<MirBlockId>,
    ) {
        let arms = match self.lattice.get(cond.0 as usize) {
            Some(Lattice::Constant(Const::Bool(true))) => vec![then],
            Some(Lattice::Constant(Const::Bool(false))) => vec![orelse],
            Some(Lattice::Unknown) => Vec::new(),
            _ => vec![then, orelse],
        };
        for arm in arms {
            for values in self.visit_region(arm, escapes) {
                for (result, value) in results.iter().zip(values) {
                    let value = self
                        .lattice
                        .get(value.0 as usize)
                        .cloned()
                        .unwrap_or(Lattice::Overdefined);
                    self.meet(*result, value);
                }
            }
        }
    }

    /// Meet each result of the loop an executable `break` labelled `label`
    /// leaves with the value it carries, as `SCCP`'s `resolveBreak` does.
    fn resolve_break(&mut self, label: Option<LoopLabel>, values: &[Reg]) {
        let Some((_, results)) = self
            .loops
            .iter()
            .rev()
            .find(|(found, _)| RegionFrame::Loop(label).names(RegionFrame::Loop(*found)))
        else {
            return;
        };
        for (result, value) in results.clone().into_iter().zip(values) {
            let value = self
                .lattice
                .get(value.0 as usize)
                .cloned()
                .unwrap_or(Lattice::Overdefined);
            self.meet(result, value);
        }
    }

    /// Evaluate a region's executable blocks, from its entry, and return
    /// the values each of them yields to the op holding the region.
    fn visit_region(
        &mut self,
        blocks: &[MirBlock],
        escapes: &mut Vec<MirBlockId>,
    ) -> Vec<Vec<Reg>> {
        let mut executable = vec![false; blocks.len()];
        let mut pending = vec![0];
        let mut yields = Vec::new();
        while let Some(block) = pending.pop() {
            if block >= blocks.len() || std::mem::replace(&mut executable[block], true) {
                continue;
            }
            pending.extend(self.visit(&blocks[block], escapes));
            if let MirTerm::Yield(values) | MirTerm::ComptimeYield(values) = &blocks[block].term {
                yields.push(values.clone());
            }
        }
        yields
    }

    /// Meet each result of `instruction` with the value it computes.
    fn transfer(&mut self, instruction: &MirInstr) {
        let mut results = Vec::new();
        instruction_result_regs(instruction, &mut results);
        if let MirInstr::Const { dest, k } = instruction {
            let value = if k.is_folded() {
                Lattice::Constant(k.clone())
            } else {
                Lattice::Overdefined
            };
            return self.meet(*dest, value);
        }
        let lattice = &self.lattice;
        let constant = |reg: Reg| match lattice.get(reg.0 as usize) {
            Some(Lattice::Constant(k)) => Some(k.clone()),
            _ => None,
        };
        if let (Some(k), [dest]) = (
            instruction.folded_constant(constant, self.reg_types, self.declarations),
            results.as_slice(),
        ) {
            return self.meet(*dest, Lattice::Constant(k));
        }
        let unknown_operand = fold_operands(instruction, self.declarations)
            .iter()
            .any(|reg| matches!(self.lattice.get(reg.0 as usize), Some(Lattice::Unknown)));
        if !unknown_operand {
            for reg in results {
                self.meet(reg, Lattice::Overdefined);
            }
        }
    }

    /// Raise `reg`'s lattice value to its meet with `value`.
    fn meet(&mut self, reg: Reg, value: Lattice) {
        let Some(current) = self.lattice.get_mut(reg.0 as usize) else {
            return;
        };
        let met = match (&*current, value) {
            (Lattice::Overdefined, _) | (_, Lattice::Unknown) => return,
            (Lattice::Unknown, value) => value,
            (Lattice::Constant(held), Lattice::Constant(new)) if same_constant(held, &new) => {
                return;
            }
            (Lattice::Constant(_), _) => Lattice::Overdefined,
        };
        *current = met;
        self.changed = true;
    }
}

/// The operands of an instruction [`MirInstr::folded_constant`] may fold;
/// none for any other.
fn fold_operands(instruction: &MirInstr, declarations: &MirDeclarations) -> Vec<Reg> {
    match instruction {
        MirInstr::BinOp { a, b, .. } => vec![*a, *b],
        MirInstr::UnOp { a, .. } => vec![*a],
        MirInstr::MakeSimd { elems, .. } => elems.clone(),
        MirInstr::MaterializeLiteral { value, .. } | MirInstr::CopyValue { value, .. } => {
            vec![*value]
        }
        MirInstr::Call { .. } => instruction
            .scalar_conversion(declarations)
            .map(|(arg, _)| vec![arg])
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// Whether two lattice constants are one value; floats compare by bits.
fn same_constant(left: &Const, right: &Const) -> bool {
    match (left, right) {
        (Const::Int(a), Const::Int(b)) => a == b,
        (Const::Float(a), Const::Float(b)) => a.to_bits() == b.to_bits(),
        (Const::Bool(a), Const::Bool(b)) => a == b,
        (Const::IntLiteral(a), Const::IntLiteral(b)) => a == b,
        (Const::FloatLiteral(a), Const::FloatLiteral(b)) => a == b,
        (Const::Value(a), Const::Value(b)) => a == b,
        _ => false,
    }
}
