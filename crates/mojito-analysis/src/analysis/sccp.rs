//! `sccp`: upstream's `SCCP`, MLIR's sparse conditional constant
//! propagation, which its pre-elaboration pipeline runs between `Mem2Reg`
//! and a `Canonicalizer` on each generator.
//!
//! Each register takes a lattice value: unknown, one constant, or
//! overdefined. Blocks become executable from the entry, a `branch` making
//! only its taken arm executable while its condition is one constant, and
//! the instructions of executable blocks are evaluated until nothing
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
//! it folds a `branch` on a constant.

use super::fold;
use super::pre_elaboration::BodyPass;
use mojito_common::timing;
use mojito_mir::mir::verify::instruction_result_regs;
use mojito_mir::mir::{
    Const, MirBlock, MirBlockId, MirDeclarations, MirFunction, MirInstr, MirTerm, Reg,
};
use mojito_types::types::Ty;
use std::collections::HashMap;

/// The pass, under MLIR's flag spelling.
pub const SCCP: BodyPass = BodyPass {
    name: "sccp",
    run: sccp,
};

/// Propagate `function`'s constants over its executable blocks and replace
/// each instruction they decide by its constant.
pub fn sccp(_name: &str, function: &mut MirFunction, declarations: &MirDeclarations) {
    let mut solver = Solver {
        lattice: vec![Lattice::Unknown; function.n_regs as usize],
        reg_types: &function.reg_types,
        declarations,
        changed: true,
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
}

/// A register's value over the executable blocks.
#[derive(Clone)]
enum Lattice {
    Unknown,
    Constant(Const),
    Overdefined,
}

/// The fixpoint's state: the lattice, and whether a walk raised it.
struct Solver<'a> {
    lattice: Vec<Lattice>,
    reg_types: &'a HashMap<u32, Ty>,
    declarations: &'a MirDeclarations,
    changed: bool,
}

impl Solver<'_> {
    /// Evaluate the executable block `block`, with every region its `try`
    /// instructions hold, and return the blocks of its list it makes
    /// executable. Escape targets, which name the function's blocks, go to
    /// `escapes`.
    fn visit(&mut self, block: &MirBlock, escapes: &mut Vec<MirBlockId>) -> Vec<MirBlockId> {
        for instruction in &block.instrs {
            self.transfer(instruction);
            for region in regions(instruction) {
                self.visit_region(region, escapes);
            }
        }
        match &block.term {
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
            MirTerm::Return(_) | MirTerm::ReturnWithCleanup { .. } | MirTerm::FallOff => Vec::new(),
        }
    }

    /// Evaluate a `try` region's executable blocks, from its entry.
    fn visit_region(&mut self, blocks: &[MirBlock], escapes: &mut Vec<MirBlockId>) {
        let mut executable = vec![false; blocks.len()];
        let mut pending = vec![0];
        while let Some(block) = pending.pop() {
            if block >= blocks.len() || std::mem::replace(&mut executable[block], true) {
                continue;
            }
            pending.extend(self.visit(&blocks[block], escapes));
        }
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

/// The block lists of a `try` instruction's regions.
fn regions(instruction: &MirInstr) -> Vec<&Vec<MirBlock>> {
    let MirInstr::Try {
        body,
        handler,
        orelse,
        finalbody,
        ..
    } = instruction
    else {
        return Vec::new();
    };
    std::iter::once(body)
        .chain(handler.as_ref().map(|(_, blocks)| blocks))
        .chain(orelse)
        .chain(finalbody)
        .collect()
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
