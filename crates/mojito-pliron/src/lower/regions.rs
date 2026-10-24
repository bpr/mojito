//! Region-op lowering, as upstream's `LowerControlFlow` lowers `hlcf` to
//! LLVM blocks: each region's single block becomes an LLVM block, an op's
//! results become arguments of the block after it, and each region
//! terminator becomes a branch carrying its values to the block it enters.
//! `if` enters its `then` region with a conditional branch, `loop` and
//! `match` with a plain one, and `lit.try` (`try_flow.rs`) lowers through
//! the same block walk.
//!
//! A scalar value is a block argument. An aggregate lives in memory, so the
//! edge copies it into the entered block's staging storage and the block
//! copies it out into fresh storage of its own, which keeps a `continue`
//! that permutes a loop's arguments a parallel copy; pliron's mem2reg and
//! LLVM remove the copies.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_mir::mir::{LoopLabel, MirCase, MirElif, region_exit};
use mojito_types::types::TyKind;

/// How the block being lowered ends when its own terminator ends it.
#[derive(Clone, Copy)]
pub(super) enum RegionEnd {
    /// The function body: only a return ends it.
    Function,
    /// An `if` arm or a `match`'s else region: `yield` enters the op's exit,
    /// carrier `0`, with the op's results.
    Arm(usize),
    /// An elif condition: `if.elifcond.yield` branches to its arm, carrier
    /// `then`, or on to carrier `next`, carrying its values to both.
    ElifCond { then: usize, next: usize },
    /// A `try` part: `lit.try.yield` enters carrier `0`, the `else`'s
    /// arguments or the op's results, with its values.
    TryPart(usize),
}

/// A region op a `break`, `continue`, `match.next`, or `match.complete`
/// may leave: a loop, or the `match` case being lowered.
pub(super) struct RegionTarget {
    frame: RegionFrame,
    /// Where a resuming terminator goes: the loop body, or the next case
    /// (the else region after the last).
    resume: usize,
    /// Where a leaving terminator goes: the block after the op.
    exit: usize,
}

/// A block entered with values: the registers it defines from them, and
/// how each travels.
pub(super) struct Carrier {
    block: Ptr<BasicBlock>,
    regs: Vec<Reg>,
    slots: Vec<CarrySlot>,
    edges: Vec<EdgeFacts>,
}

impl FnLowering<'_> {
    /// Lower the function body's block into the current LLVM block.
    pub(super) fn lower_body_block(
        &mut self,
        ctx: &mut Context,
        block: &MirBlock,
    ) -> Result<(), PlironError> {
        self.lower_block(ctx, block, CarrierBinding::None, RegionEnd::Function)
    }

    /// `hlcf.if`: a conditional branch on `cond` to the `then` region or to
    /// the first elif condition (the else region without one); each region
    /// is a block of its own, and each `yield` enters the exit with the
    /// op's results.
    pub(super) fn lower_if(
        &mut self,
        ctx: &mut Context,
        results: &[Reg],
        cond: Reg,
        (then, elifs): (&[MirBlock], &[MirElif]),
        (else_args, orelse): (&[Reg], &[MirBlock]),
    ) -> Result<(), PlironError> {
        let exit = self.carrier(ctx, results)?;
        let then_entry = self.carrier(ctx, &[])?;
        let mut tests = Vec::with_capacity(elifs.len());
        for elif in elifs {
            tests.push((
                self.carrier(ctx, &elif.cond_args)?,
                self.carrier(ctx, &elif.then_args)?,
            ));
        }
        let else_entry = self.carrier(ctx, else_args)?;
        let first = tests.first().map_or(else_entry, |(test, _)| *test);
        let condition = self.reg_value(ctx, cond, ScalarTy::Bool)?;
        let branch = CondBrOp::new(
            ctx,
            condition,
            self.carriers[then_entry].block,
            vec![],
            self.carriers[first].block,
            vec![],
        );
        self.append(ctx, branch.get_operation(), Some(cond));
        self.lower_region(ctx, then, then_entry, RegionEnd::Arm(exit))?;
        for (index, (elif, (test, arm))) in elifs.iter().zip(&tests).enumerate() {
            let next = tests.get(index + 1).map_or(else_entry, |(test, _)| *test);
            let end = RegionEnd::ElifCond { then: *arm, next };
            self.lower_region(ctx, &elif.cond, *test, end)?;
            self.lower_region(ctx, &elif.then, *arm, RegionEnd::Arm(exit))?;
        }
        self.lower_region(ctx, orelse, else_entry, RegionEnd::Arm(exit))?;
        self.enter_exit(ctx, exit);
        Ok(())
    }

    /// `hlcf.loop`: a branch into the body with `operands` as its
    /// arguments; a `continue` branches back with the next iteration's, a
    /// `break` to the exit with the op's results.
    pub(super) fn lower_loop(
        &mut self,
        ctx: &mut Context,
        label: Option<LoopLabel>,
        (operands, args, results): (&[Reg], &[Reg], &[Reg]),
        body: &[MirBlock],
    ) -> Result<(), PlironError> {
        let exit = self.carrier(ctx, results)?;
        let entry = self.carrier(ctx, args)?;
        self.branch_carrying(ctx, entry, operands)?;
        let frame = RegionFrame::Loop(label);
        self.push_target(frame, entry, exit);
        let resumed = blocks_resume_loop(body, label, &mut Vec::new());
        let lowered = self.lower_region_with(ctx, body, entry, !resumed, RegionEnd::Function);
        self.region_targets.pop();
        lowered?;
        self.enter_exit(ctx, exit);
        Ok(())
    }

    /// `hlcf.match`: a branch into the first case; a `match.next` enters the
    /// next case (the else region after the last) with its arguments, and a
    /// `match.complete` or the else region's `yield` enters the exit with
    /// the op's results.
    pub(super) fn lower_match(
        &mut self,
        ctx: &mut Context,
        results: &[Reg],
        cases: &[MirCase],
        (else_args, orelse): (&[Reg], &[MirBlock]),
    ) -> Result<(), PlironError> {
        let exit = self.carrier(ctx, results)?;
        let mut entries = Vec::with_capacity(cases.len());
        for case in cases {
            entries.push(self.carrier(ctx, &case.args)?);
        }
        let else_entry = self.carrier(ctx, else_args)?;
        let first = entries.first().copied().unwrap_or(else_entry);
        self.branch_carrying(ctx, first, &[])?;
        for (index, (case, entry)) in cases.iter().zip(&entries).enumerate() {
            let next = entries.get(index + 1).copied().unwrap_or(else_entry);
            self.push_target(RegionFrame::MatchCase, next, exit);
            let lowered = self.lower_region(ctx, &case.body, *entry, RegionEnd::Function);
            self.region_targets.pop();
            lowered?;
        }
        self.lower_region(ctx, orelse, else_entry, RegionEnd::Arm(exit))?;
        self.enter_exit(ctx, exit);
        Ok(())
    }

    /// A region terminator ending the block it sits in (`yield`,
    /// `if.elifcond.yield`, `lit.try.yield`), as `end` says.
    pub(super) fn lower_region_end(
        &mut self,
        ctx: &mut Context,
        term: &MirTerm,
        end: RegionEnd,
    ) -> Result<(), PlironError> {
        match (term, end) {
            (MirTerm::Yield(values), RegionEnd::Arm(exit)) => {
                self.branch_carrying(ctx, exit, values)
            }
            (MirTerm::ElifCond { cond, values }, RegionEnd::ElifCond { then, next }) => {
                let condition = self.reg_value(ctx, *cond, ScalarTy::Bool)?;
                let then_args = self.carry(ctx, then, values)?;
                let next_args = self.carry(ctx, next, values)?;
                let branch = CondBrOp::new(
                    ctx,
                    condition,
                    self.carriers[then].block,
                    then_args,
                    self.carriers[next].block,
                    next_args,
                );
                self.append(ctx, branch.get_operation(), Some(*cond));
                Ok(())
            }
            (MirTerm::TryYield(values), RegionEnd::TryPart(completed)) => {
                self.branch_carrying(ctx, completed, values)
            }
            _ => Err(self.unsupported(
                format!(
                    "the region terminator `{}` outside the region it ends",
                    mojito_mir::mir::text::terminator_mnemonic(term)
                ),
                None,
            )),
        }
    }

    /// A `break`, `continue`, `match.next`, or `match.complete`: the
    /// innermost enclosing region op it names, entered with `values` at its
    /// exit, or at its body or next case when the terminator resumes it.
    pub(super) fn lower_region_exit(
        &mut self,
        ctx: &mut Context,
        term: &MirTerm,
        values: &[Reg],
    ) -> Result<(), PlironError> {
        let Some((frame, resume)) = region_exit(term) else {
            return Err(self.unsupported("a region exit without a frame".into(), None));
        };
        let Some(target) = self
            .region_targets
            .iter()
            .rposition(|target| frame.names(target.frame))
        else {
            return Err(self.unsupported(
                format!(
                    "`{}` outside the region op it leaves",
                    mojito_mir::mir::text::terminator_mnemonic(term)
                ),
                None,
            ));
        };
        let target = &self.region_targets[target];
        let carrier = if resume { target.resume } else { target.exit };
        self.branch_carrying(ctx, carrier, values)
    }

    /// A block of its own for the region `blocks`, entered from `entry`'s
    /// block; `self.current` is restored.
    pub(super) fn lower_region(
        &mut self,
        ctx: &mut Context,
        blocks: &[MirBlock],
        entry: usize,
        end: RegionEnd,
    ) -> Result<(), PlironError> {
        self.lower_region_with(ctx, blocks, entry, true, end)
    }

    /// A fresh block entered with no values, for a `try` part.
    pub(super) fn region_entry(&mut self, ctx: &mut Context) -> Result<usize, PlironError> {
        self.carrier(ctx, &[])
    }

    pub(super) fn carrier_block(&self, carrier: usize) -> Ptr<BasicBlock> {
        self.carriers[carrier].block
    }

    /// A fresh block entered with values for `regs`: one argument per
    /// scalar, staging storage per aggregate.
    pub(super) fn carrier(
        &mut self,
        ctx: &mut Context,
        regs: &[Reg],
    ) -> Result<usize, PlironError> {
        let mut slots = Vec::with_capacity(regs.len());
        let mut arguments = Vec::new();
        for reg in regs {
            let Some(ty) = self.func.reg_types.get(&reg.0).cloned() else {
                return Err(self.unsupported_reg(format!("untyped region value %r{}", reg.0), *reg));
            };
            let slot = match lower_ty(self.name, &ty, &self.layout, self.reg_span(*reg))? {
                LowerTy::Scalar(scalar) => {
                    arguments.push(scalar.handle(ctx));
                    CarrySlot::Scalar(scalar)
                }
                LowerTy::Aggregate { ty, layout } => {
                    let staging = self.value_storage(ctx, &ty, layout);
                    CarrySlot::Aggregate {
                        ty,
                        layout,
                        staging,
                    }
                }
                LowerTy::ZeroSized => CarrySlot::ZeroSized,
            };
            slots.push(slot);
        }
        let region = self.region.expect("lowering is inside a function");
        let block = BasicBlock::new(ctx, None, arguments);
        block.insert_at_back(region, ctx);
        self.carriers.push(Carrier {
            block,
            regs: regs.to_vec(),
            edges: vec![EdgeFacts::default(); slots.len()],
            slots,
        });
        Ok(self.carriers.len() - 1)
    }

    /// Branch to `carrier`'s block with `values`.
    fn branch_carrying(
        &mut self,
        ctx: &mut Context,
        carrier: usize,
        values: &[Reg],
    ) -> Result<(), PlironError> {
        let operands = self.carry(ctx, carrier, values)?;
        let jump = BrOp::new(ctx, self.carriers[carrier].block, operands);
        self.append(ctx, jump.get_operation(), None);
        Ok(())
    }

    /// The operands of an edge entering `carrier` with `values`: each
    /// scalar as its value at the target register's type, each aggregate
    /// copied into the target's staging storage. Ownership moves with the
    /// value; [`Self::transfer`] gives it up once every edge carrying it is
    /// lowered.
    pub(super) fn carry(
        &mut self,
        ctx: &mut Context,
        carrier: usize,
        values: &[Reg],
    ) -> Result<Vec<Value>, PlironError> {
        let regs = self.carriers[carrier].regs.clone();
        if regs.len() != values.len() {
            return Err(self.unsupported(
                format!(
                    "{} values entering a block that takes {}",
                    values.len(),
                    regs.len()
                ),
                None,
            ));
        }
        let mut operands = Vec::new();
        for (position, (src, dest)) in values.iter().zip(&regs).enumerate() {
            let facts = EdgeFacts {
                shared: self.storage_reads.contains(&src.0)
                    || self.aliased_load_regs.contains(&src.0),
                pointer_ref: self.pointer_slot_refs.contains(&src.0),
                constant: if self.pending_literals.contains_key(&src.0)
                    || self.str_consts.contains_key(&src.0)
                    || self.int_consts.contains_key(&src.0)
                {
                    Constant::Same(*src)
                } else {
                    Constant::Varies
                },
            };
            self.carriers[carrier].edges[position].join(&facts);
            match self.carriers[carrier].slots[position].clone() {
                CarrySlot::Scalar(expected) => {
                    operands.push(self.carried_scalar(ctx, *src, *dest, expected)?);
                }
                CarrySlot::Aggregate {
                    ty,
                    layout,
                    staging,
                } => {
                    let ptr = self.reg_ptr(ctx, *src)?;
                    self.copy_value(ctx, staging, ptr, &ty, layout, *src);
                }
                CarrySlot::ZeroSized => {}
            }
        }
        Ok(operands)
    }

    /// Continue in `exit`'s block after a region op, its results bound.
    pub(super) fn enter_exit(&mut self, ctx: &mut Context, exit: usize) {
        self.current = Some(self.carriers[exit].block);
        self.bind_carried(ctx, exit, true);
    }

    /// [`Self::lower_region`], where `edges_known` says whether every edge
    /// entering it is lowered before it, as only a loop body's `continue`s
    /// are not.
    fn lower_region_with(
        &mut self,
        ctx: &mut Context,
        blocks: &[MirBlock],
        entry: usize,
        edges_known: bool,
        end: RegionEnd,
    ) -> Result<(), PlironError> {
        let [block] = blocks else {
            return Err(self.unsupported(
                format!(
                    "a region of {} blocks reached native lowering",
                    blocks.len()
                ),
                None,
            ));
        };
        let saved = self.current.replace(self.carriers[entry].block);
        let binding = if edges_known {
            CarrierBinding::Complete(entry)
        } else {
            CarrierBinding::Partial(entry)
        };
        let result = self.lower_block(ctx, block, binding, end);
        self.current = saved;
        result
    }

    /// Lower `block`, a region's single block, into the current LLVM block:
    /// bind the values `entry` carries into it, lower its instructions, and
    /// end it as `end` says. The values it materialized on demand are
    /// forgotten afterwards, since its block dominates no sibling region.
    fn lower_block(
        &mut self,
        ctx: &mut Context,
        block: &MirBlock,
        entry: CarrierBinding,
        end: RegionEnd,
    ) -> Result<(), PlironError> {
        let lazy = self.lazy_values.len();
        let result = self.lower_block_body(ctx, block, entry, end);
        for (reg, previous) in self.lazy_values.split_off(lazy).into_iter().rev() {
            match previous {
                Some(value) => self.reg_values.insert(reg, value),
                None => self.reg_values.remove(&reg),
            };
        }
        result
    }

    fn push_target(&mut self, frame: RegionFrame, resume: usize, exit: usize) {
        self.region_targets.push(RegionTarget {
            frame,
            resume,
            exit,
        });
    }

    /// Define `carrier`'s registers in its block, the current one: a scalar
    /// is its block argument, an aggregate a copy of its staging storage.
    /// When `edges_known`
    /// and every edge carried the same compile-time constant, the register
    /// is that constant, as a block argument with one incoming value folds
    /// to it.
    fn bind_carried(&mut self, ctx: &mut Context, carrier: usize, edges_known: bool) {
        let block = self.carriers[carrier].block;
        let regs = self.carriers[carrier].regs.clone();
        let mut argument = 0;
        for (position, reg) in regs.into_iter().enumerate() {
            let facts = self.carriers[carrier].edges[position].clone();
            let constant = match facts.constant {
                Constant::Same(src) if edges_known => Some(src),
                _ => None,
            };
            if let Some(src) = constant {
                if let Some(value) = self.int_consts.get(&src.0).copied() {
                    self.int_consts.insert(reg.0, value);
                }
                if let Some(literal) = self.pending_literals.get(&src.0).cloned() {
                    self.pending_literals.insert(reg.0, literal);
                }
                if let Some(bytes) = self.str_consts.get(&src.0).cloned() {
                    self.str_consts.insert(reg.0, bytes);
                }
            }
            let folded = constant.is_some_and(|src| {
                self.pending_literals.contains_key(&src.0) || self.str_consts.contains_key(&src.0)
            });
            match self.carriers[carrier].slots[position].clone() {
                CarrySlot::Scalar(_) => {
                    let value = block.deref(ctx).get_argument(argument);
                    argument += 1;
                    if !folded {
                        self.reg_values.insert(reg.0, value);
                    }
                    if facts.pointer_ref {
                        self.pointer_slot_refs.insert(reg.0);
                    }
                }
                CarrySlot::Aggregate { .. } if folded => {}
                CarrySlot::Aggregate {
                    ty,
                    layout,
                    staging,
                } => {
                    let storage = self.value_storage(ctx, &ty, layout);
                    self.copy_value(ctx, storage, staging, &ty, layout, reg);
                    self.reg_values.insert(reg.0, storage);
                    if facts.shared {
                        self.storage_reads.insert(reg.0);
                    }
                }
                CarrySlot::ZeroSized => {
                    self.erased.insert(reg.0);
                }
            }
        }
    }

    fn lower_block_body(
        &mut self,
        ctx: &mut Context,
        block: &MirBlock,
        entry: CarrierBinding,
        end: RegionEnd,
    ) -> Result<(), PlironError> {
        match entry {
            CarrierBinding::None => {}
            CarrierBinding::Complete(carrier) => self.bind_carried(ctx, carrier, true),
            CarrierBinding::Partial(carrier) => self.bind_carried(ctx, carrier, false),
        }
        for instr in &block.instrs {
            self.lower_instr(ctx, instr)?;
        }
        self.lower_term(ctx, &block.term, end)
    }

    /// `src`'s value entering the scalar register `dest`: a pending literal
    /// converts exactly into literal-typed storage, as a store does.
    fn carried_scalar(
        &mut self,
        ctx: &mut Context,
        src: Reg,
        dest: Reg,
        expected: ScalarTy,
    ) -> Result<Value, PlironError> {
        if let Some(ty) = self.func.reg_types.get(&dest.0).cloned()
            && matches!(ty.kind(), TyKind::IntLiteral | TyKind::FloatLiteral)
            && let Some(literal) = self.pending_literals.get(&src.0).cloned()
        {
            return self.exact_literal_storage(ctx, &literal, &ty, src);
        }
        self.reg_value(ctx, src, expected)
    }
}

/// How one region value travels into its block.
#[derive(Clone)]
enum CarrySlot {
    Scalar(ScalarTy),
    Aggregate {
        ty: Ty,
        layout: Layout,
        staging: Value,
    },
    ZeroSized,
}

/// What the edges lowered so far carried into one region value.
#[derive(Clone, Default)]
struct EdgeFacts {
    /// Some edge carried bytes sharing another owner's buffers.
    shared: bool,
    /// Some edge carried a handle to pointer-typed storage.
    pointer_ref: bool,
    constant: Constant,
}

impl EdgeFacts {
    fn join(&mut self, edge: &Self) {
        self.shared |= edge.shared;
        self.pointer_ref |= edge.pointer_ref;
        self.constant = match (self.constant, edge.constant) {
            (Constant::Unknown, constant) => constant,
            (Constant::Same(a), Constant::Same(b)) if a == b => Constant::Same(a),
            _ => Constant::Varies,
        };
    }
}

/// Whether every edge carried one compile-time constant register.
#[derive(Clone, Copy, Default)]
enum Constant {
    /// No edge yet.
    #[default]
    Unknown,
    Same(Reg),
    Varies,
}

/// Which values a region block binds on entry, and whether every edge
/// entering it is known by then.
#[derive(Clone, Copy)]
enum CarrierBinding {
    None,
    Complete(usize),
    Partial(usize),
}

/// Whether a `continue` in `blocks` resumes the loop labelled `label`:
/// `inner` holds the labels of the loops between them, the innermost of
/// which a `continue` naming it resumes instead.
fn blocks_resume_loop(
    blocks: &[MirBlock],
    label: Option<LoopLabel>,
    inner: &mut Vec<Option<LoopLabel>>,
) -> bool {
    blocks.iter().any(|block| {
        let resumes = matches!(region_exit(&block.term), Some((frame, true))
            if !inner.iter().any(|found| frame.names(RegionFrame::Loop(*found)))
                && frame.names(RegionFrame::Loop(label)));
        resumes
            || block.instrs.iter().any(|instr| {
                let nested = match instr {
                    MirInstr::Loop { label, .. } => Some(*label),
                    _ => None,
                };
                if let Some(nested) = nested {
                    inner.push(nested);
                }
                let found = mojito_mir::mir::instruction_regions(instr)
                    .into_iter()
                    .any(|region| blocks_resume_loop(region, label, inner));
                if nested.is_some() {
                    inner.pop();
                }
                found
            })
    })
}
