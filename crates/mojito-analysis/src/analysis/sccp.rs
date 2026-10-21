//! `sccp`: upstream's `SCCP`, MLIR's sparse conditional constant
//! propagation, which its pre-elaboration pipeline runs between `Mem2Reg`
//! and a `Canonicalizer` on each generator.
//!
//! Each register takes a lattice value: unknown, one constant, or
//! overdefined. Blocks become executable from the entry, a `branch` making
//! only its taken arm executable while its condition is one constant. A
//! structured body is walked region by region, as `SCCP` walks `hlcf`
//! through `getEntryTargets` and `getBranchTargets`: an `if` enters only the
//! arm a constant condition takes, or the first elif's condition region
//! when it is false, and an `if.elifcond.yield` enters its arm, the next
//! region, or both alike, the arguments of each region it enters meeting
//! the values it carries; the `if`'s results meet the values its executable
//! arms yield. A `match` enters its first case, and each executable
//! `match.next` the next case (the else region after the last), its values
//! meeting that region's arguments; its results are overdefined, as
//! `SCCP`'s generic path leaves every node it does not special-case. A
//! loop's arguments meet its operands and the values its executable
//! `continue`s carry, and its results meet the values its executable
//! `break`s carry, while a loop's body, a `comptime if`, a `comptime for`,
//! and a `try` are executable, the other ops' results overdefined (a
//! `comptime for`'s arguments too, as that generic path leaves each region
//! argument at its entry state). The
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
    Const, LoopLabel, MirBlock, MirBlockId, MirCase, MirDeclarations, MirElif, MirFunction,
    MirInstr, MirTerm, Reg, RegionFrame, instruction_regions,
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
        cases: Vec::new(),
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
/// loops and `match` cases enclosing the block being walked, innermost
/// last.
struct Solver<'a> {
    lattice: Vec<Lattice>,
    reg_types: &'a HashMap<u32, Ty>,
    declarations: &'a MirDeclarations,
    changed: bool,
    loops: Vec<LoopFrame>,
    cases: Vec<CaseFrame>,
}

/// A loop enclosing the block being walked: what its `break`s and
/// `continue`s name and the registers their values define.
struct LoopFrame {
    label: Option<LoopLabel>,
    args: Vec<Reg>,
    results: Vec<Reg>,
}

/// A `match` case enclosing the block being walked: the arguments of the
/// region its `match.next`s enter, and whether an executable one does.
struct CaseFrame {
    next_args: Vec<Reg>,
    next: bool,
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
                else_args,
                orelse,
            } = instruction
            {
                self.visit_if(results, *cond, then, elifs, (else_args, orelse), escapes);
                continue;
            }
            if let MirInstr::Match {
                results,
                cases,
                else_args,
                orelse,
            } = instruction
            {
                self.visit_match(cases, (else_args, orelse), escapes);
                for result in results {
                    self.meet(*result, Lattice::Overdefined);
                }
                continue;
            }
            if let MirInstr::Loop {
                label,
                operands,
                args,
                results,
                body,
            } = instruction
            {
                self.meet_values(args, operands);
                self.loops.push(LoopFrame {
                    label: *label,
                    args: args.clone(),
                    results: results.clone(),
                });
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
                self.resolve_exit(*label, values, false);
                Vec::new()
            }
            MirTerm::Continue { label, values } => {
                self.resolve_exit(*label, values, true);
                Vec::new()
            }
            MirTerm::MatchNext(values) => {
                if let Some(frame) = self.cases.last_mut() {
                    frame.next = true;
                    let next_args = frame.next_args.clone();
                    self.meet_values(&next_args, values);
                }
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
            | MirTerm::ElifCond { .. }
            | MirTerm::ComptimeYield(_)
            | MirTerm::ComptimeBreak(_)
            | MirTerm::ComptimeContinue(_)
            | MirTerm::MatchComplete(_)
            | MirTerm::Unreachable => Vec::new(),
        }
    }

    /// Evaluate an `if`, as `IfOp::getEntryTargets` enters it and
    /// `IfElifCondYieldOp::getBranchTargets` goes on: the op and each
    /// executable `if.elifcond.yield` take the arm their condition selects,
    /// the next region (an elif's condition, or the else region), or both
    /// ([`Self::targets`]), each `if.elifcond.yield` meeting the arguments
    /// of every region it enters with its values. Each result meets the
    /// values the executable arms yield to it.
    fn visit_if(
        &mut self,
        results: &[Reg],
        cond: Reg,
        then: &[MirBlock],
        elifs: &[MirElif],
        (else_args, orelse): (&[Reg], &[MirBlock]),
        escapes: &mut Vec<MirBlockId>,
    ) {
        let (taken, mut next) = self.targets(cond);
        if taken {
            self.visit_arm(results, then, escapes);
        }
        for (index, elif) in elifs.iter().enumerate() {
            if !next {
                return;
            }
            let next_args = elifs
                .get(index + 1)
                .map_or(else_args, |rest| &rest.cond_args);
            next = false;
            let mut taken = false;
            for term in self.visit_region(&elif.cond, escapes) {
                if let MirTerm::ElifCond { cond, values } = term {
                    let (then, on) = self.targets(*cond);
                    if then {
                        self.meet_values(&elif.then_args, values);
                    }
                    if on {
                        self.meet_values(next_args, values);
                    }
                    taken |= then;
                    next |= on;
                }
            }
            if taken {
                self.visit_arm(results, &elif.then, escapes);
            }
        }
        if next {
            self.visit_arm(results, orelse, escapes);
        }
    }

    /// Evaluate an `if` arm, meeting each of `results` with the values its
    /// executable blocks yield.
    fn visit_arm(&mut self, results: &[Reg], arm: &[MirBlock], escapes: &mut Vec<MirBlockId>) {
        for term in self.visit_region(arm, escapes) {
            if let MirTerm::Yield(values) = term {
                self.meet_values(results, values);
            }
        }
    }

    /// The regions a `Bool` condition `cond` enters, as the arm it selects
    /// and the region after: a constant one only its own, an unknown one
    /// neither yet, and any other both.
    fn targets(&self, cond: Reg) -> (bool, bool) {
        match self.lattice.get(cond.0 as usize) {
            Some(Lattice::Constant(Const::Bool(true))) => (true, false),
            Some(Lattice::Constant(Const::Bool(false))) => (false, true),
            Some(Lattice::Unknown) => (false, false),
            _ => (true, true),
        }
    }

    /// Evaluate a `match`, as `MatchOp::getEntryTargets` enters its first
    /// case and `MatchNextOp::getBranchTargets` the next: a case, or the
    /// else region after the last, is executable once an executable
    /// `match.next` of the case before it enters it.
    fn visit_match(
        &mut self,
        cases: &[MirCase],
        (else_args, orelse): (&[Reg], &[MirBlock]),
        escapes: &mut Vec<MirBlockId>,
    ) {
        for (index, case) in cases.iter().enumerate() {
            let next_args = cases.get(index + 1).map_or(else_args, |next| &next.args);
            self.cases.push(CaseFrame {
                next_args: next_args.to_vec(),
                next: false,
            });
            self.visit_region(&case.body, escapes);
            if !self.cases.pop().is_some_and(|frame| frame.next) {
                return;
            }
        }
        self.visit_region(orelse, escapes);
    }

    /// Meet each register an executable `break` or `continue` labelled
    /// `label` defines with the value it carries: a result of the loop it
    /// leaves, as `SCCP`'s `resolveBreak` does, or, when it `resume`s, an
    /// argument of the loop's next iteration, as
    /// `processControlFlowTerminator` does.
    fn resolve_exit(&mut self, label: Option<LoopLabel>, values: &[Reg], resume: bool) {
        let Some(frame) = self
            .loops
            .iter()
            .rev()
            .find(|frame| RegionFrame::Loop(label).names(RegionFrame::Loop(frame.label)))
        else {
            return;
        };
        let defined = if resume {
            frame.args.clone()
        } else {
            frame.results.clone()
        };
        self.meet_values(&defined, values);
    }

    /// Meet each of `defined` with the lattice value of the register of
    /// `values` that defines it.
    fn meet_values(&mut self, defined: &[Reg], values: &[Reg]) {
        for (reg, value) in defined.iter().zip(values) {
            let value = self
                .lattice
                .get(value.0 as usize)
                .cloned()
                .unwrap_or(Lattice::Overdefined);
            self.meet(*reg, value);
        }
    }

    /// Evaluate a region's executable blocks, from its entry, and return
    /// their terminators that end the region for the op holding it: each
    /// `yield`, `comptime.yield`, and `if.elifcond.yield`.
    fn visit_region<'b>(
        &mut self,
        blocks: &'b [MirBlock],
        escapes: &mut Vec<MirBlockId>,
    ) -> Vec<&'b MirTerm> {
        let mut executable = vec![false; blocks.len()];
        let mut pending = vec![0];
        let mut ends = Vec::new();
        while let Some(block) = pending.pop() {
            if block >= blocks.len() || std::mem::replace(&mut executable[block], true) {
                continue;
            }
            pending.extend(self.visit(&blocks[block], escapes));
            let term = &blocks[block].term;
            if matches!(
                term,
                MirTerm::Yield(_) | MirTerm::ComptimeYield(_) | MirTerm::ElifCond { .. }
            ) {
                ends.push(term);
            }
        }
        ends
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
