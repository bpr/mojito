//! Drop elaboration: placing `DropVar`s in the structured body, as
//! upstream's `DestructorInsertion` places destructors, and the cleanup
//! lists of a `try`'s raise edge and of a `return` crossing a `finally`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_mir::mir::{instruction_regions, instruction_regions_mut};

/// Insert `DropVar`s at each variable's death, on the structured body, as
/// upstream's `DestructorInsertion` does. One backward walk
/// ([`walk_backward`]) finds where each variable dies, and a splice pass
/// places its drop:
///
/// - right after the instruction that is its last use (touched, then dead),
///   skipping variables moved out with `^` (their new owner drops them);
/// - at the entry of an `if` or `comptime if` arm that leaves out a value a
///   sibling arm keeps (`destroyValuesAtEntryIfNeeded`);
/// - before a block's terminator, when the values the block yields were the
///   last use.
///
/// At a shared death point, variables drop in reverse declaration order
/// (descending `VarId`), except that an `in_place` binding joins its
/// producer's death point and its slot drops first (see [`InPlaceDeaths`]).
/// No block is added: a `try` keeps cleanup lists only where a region has
/// no place for the drop, its raise edge and a return crossing a `finally`.
pub(super) fn elaborate_drops(
    mut function: MirFunction,
    value_parameters: &HashSet<VarId>,
    declarations: &MirDeclarations,
) -> MirFunction {
    let passing = ArgumentPassing::new(declarations);
    let loan_roots = drop_loan_generations(&function);
    let generation_dests = loan_generation_dests(&function);
    let facts = DropFacts::new(&function, &loan_roots, &generation_dests, &passing);
    let mut plan = DropPlan::default();
    let liveness = DropLiveness {
        facts: &facts,
        loan_roots: &loan_roots,
    };
    walk_backward(&liveness, &function.blocks, &mut plan);
    // Taking the blocks moves no block: the plan's addresses stay valid.
    let mut blocks = std::mem::take(&mut function.blocks);
    let splice = Splice {
        f: &function,
        parameter_slots: value_parameters,
        loan_roots: &loan_roots,
        facts: &facts,
        plan: &plan,
    };
    // A consuming parameter the body never uses is dead on arrival: Mojo
    // destroys an unused `deinit self`'s fields before the first statement
    // runs, so it drops at the function's entry.
    let entry_dead: Vec<VarId> = blocks
        .first()
        .and_then(|entry| {
            let live = splice.plan.blocks.get(&std::ptr::from_ref(entry))?;
            let live = splice.effective(&live.live_in, facts.entry(entry));
            Some(
                (0..function.n_params)
                    .map(|v| v as VarId)
                    .filter(|v| splice.droppable(*v) && !live.contains(v))
                    .collect(),
            )
        })
        .unwrap_or_default();
    splice.region(&mut blocks, &PartCx::default());
    if let Some(entry) = blocks.first_mut() {
        prepend_drops(&mut entry.instrs, entry_dead);
    }

    // A `deinit` parameter is *consumed*, not destroyed: the spliced teardown
    // for it must skip the value's whole-value `__deinit__` (its resources already
    // moved into the receiver) while still destroying any residual fields. Drop
    // elaboration emits an ordinary `DropVar`; rewrite those to `ConsumeVar` for
    // deinit parameters. Their `^`-transferred fields are `Value::Moved` and a
    // no-op to drop, so this cannot double-free.
    if function.deinit_params.iter().any(|&d| d) {
        let is_deinit_param = |var: VarId| {
            (var as usize) < function.n_params
                && function
                    .deinit_params
                    .get(var as usize)
                    .copied()
                    .unwrap_or(false)
        };
        for_each_instr_deep_mut(&mut blocks, &mut |instr| {
            if let MirInstr::DropVar { var, .. } = instr
                && is_deinit_param(*var)
            {
                *instr = MirInstr::ConsumeVar { var: *var };
            }
        });
    }

    function.blocks = blocks;
    function
}

/// The owner roots carried by each `EstablishLoans` generation. A later
/// establishment for the same aggregate replaces the old marker; keeping the
/// marker as the key prevents historical owners from becoming one permanent
/// dependency of the variable slot.
pub(super) struct DropLoanGeneration {
    pub(super) roots: Vec<VarId>,
    /// `ref` values, reference-bearing aggregates, and tracked pointers can
    /// leave a handle in an SSA register (a pointer argument is read inside
    /// its callee) and therefore participate in transient register
    /// provenance.
    pub(super) propagate_through_registers: bool,
}

/// Collected deep through `try` regions: region interiors get their own drop
/// elaboration, so their generations participate in owner retention too.
/// Markers are function-wide unique registers, so the extra entries are inert
/// for any walk that never activates them.
pub(super) fn drop_loan_generations(f: &MirFunction) -> BTreeMap<u32, DropLoanGeneration> {
    let mut roots_by_generation: BTreeMap<u32, DropLoanGeneration> = BTreeMap::new();
    for_each_instr_deep(&f.blocks, &mut |instr| {
        if let MirInstr::EstablishLoans { loans, marker, .. } = instr {
            // Every generation propagates, pointer variables included: a
            // pointer register handed to a call reads its pointee inside the
            // callee, after the variable's own last use.
            let generation =
                roots_by_generation
                    .entry(marker.0)
                    .or_insert_with(|| DropLoanGeneration {
                        roots: Vec::new(),
                        propagate_through_registers: true,
                    });
            for loan in loans {
                if !generation.roots.contains(&loan.place.root) {
                    generation.roots.push(loan.place.root);
                }
            }
        }
    });
    roots_by_generation
}

pub(super) fn active_drop_loan_roots(
    state: &LoanGenerationState,
    roots_by_generation: &BTreeMap<u32, DropLoanGeneration>,
) -> BTreeSet<VarId> {
    state
        .active
        .values()
        .flatten()
        .filter_map(|generation| roots_by_generation.get(generation))
        .flat_map(|generation| generation.roots.iter().copied())
        .collect()
}

/// What the backward walk reports for the splice, keyed by node address
/// (stable while the body is borrowed, and through the splice: no block is
/// added or moved, and an instruction is looked up before its block is
/// rebuilt).
#[derive(Default)]
struct DropPlan {
    /// The live-in of each region op.
    ops: HashMap<*const MirInstr, DenseSet>,
    blocks: HashMap<*const MirBlock, BlockPlan>,
    tries: HashMap<*const MirInstr, TryChannels<DenseSet>>,
}

/// The liveness around one block.
struct BlockPlan {
    live_in: DenseSet,
    live_out: DenseSet,
    /// What a raise in the block leaves live: its handler's (or its
    /// `finally`'s) live-in. `None` outside every `try`.
    raise: Option<DenseSet>,
}

impl BackwardRecorder<DenseSet> for DropPlan {
    fn instr(&mut self, instr: &MirInstr, live: &DenseSet) {
        if region_op(instr) {
            self.ops.insert(std::ptr::from_ref(instr), live.clone());
        }
    }

    fn block(&mut self, block: &MirBlock, live: BlockLiveness<'_, DenseSet>) {
        self.blocks.insert(
            std::ptr::from_ref(block),
            BlockPlan {
                live_in: live.live_in.clone(),
                live_out: live.live_out.clone(),
                raise: live.raise.cloned(),
            },
        );
    }

    fn try_op(&mut self, op: &MirInstr, channels: &TryChannels<DenseSet>) {
        self.tries.insert(std::ptr::from_ref(op), channels.clone());
    }
}

/// Backward drop liveness: a variable is live from a use to its definition.
/// A use is a direct read, a pending teardown, or a root an instruction
/// consumes through its operand registers ([`DropFacts::uses`]), counted at
/// its exact consuming point, so a register loaded in one region and
/// consumed in another keeps its owner live between them. Loan-generation
/// owners are not folded in, since generations join where paths meet and
/// would keep every historical owner of a rebound aggregate live; the
/// splice adds them per point, and a `try` part's live-in as the `try` sees
/// it.
struct DropLiveness<'a> {
    facts: &'a DropFacts,
    loan_roots: &'a BTreeMap<u32, DropLoanGeneration>,
}

impl BackwardScan for DropLiveness<'_> {
    type State = DenseSet;

    fn join(&self, state: &mut DenseSet, other: &DenseSet) {
        state.union(other);
    }

    fn transfer(&self, live: &mut DenseSet, instr: &MirInstr) {
        if let Some(d) = var_def(instr) {
            live.remove(d);
        }
        live.extend(var_uses(instr).into_iter().map(|(v, _)| v));
        // A pending teardown keeps its variable live up to that point so no
        // earlier death splices a competing `DropVar`.
        live.extend(teardown(instr));
        let uses = self.facts.uses(instr);
        live.extend(uses.retained.iter().copied());
        live.extend(uses.loaded.iter().copied());
    }

    fn enters(&self, _instr: &MirInstr) -> bool {
        true
    }

    fn define(&self, live: &mut DenseSet, var: VarId) {
        live.remove(var);
    }

    fn terminate(&self, live: &mut DenseSet, block: &MirBlock) {
        live.extend(self.facts.yielded(block).retained.iter().copied());
    }

    fn enter(&self, live: &mut DenseSet, part: &[MirBlock]) {
        let extras = loan_extras(live, self.facts.region_entry(part), self.loan_roots);
        live.union(&extras);
    }
}

/// The splice pass: the plan's drops placed in the body, nested regions
/// before the blocks holding them.
struct Splice<'a> {
    f: &'a MirFunction,
    /// The function's compile-time parameter slots: its materialized value
    /// parameters (`materialized_parameter_slots`), which it never drops.
    parameter_slots: &'a HashSet<VarId>,
    loan_roots: &'a BTreeMap<u32, DropLoanGeneration>,
    facts: &'a DropFacts,
    plan: &'a DropPlan,
}

/// The channels of the `try` part a region sits in, as its `return`s and
/// nested `try`s see them; empty outside every `try`.
#[derive(Default)]
struct PartCx {
    /// What a `return` here leaves live: the live-in of the `finally`s it
    /// crosses, and of those beyond them.
    return_live: HashSet<VarId>,
    /// The slots of `return_live` those `finally`s end.
    return_ended: HashSet<VarId>,
}

impl PartCx {
    /// What a `return` here destroys once the `finally`s it crosses have
    /// run, in reverse declaration order.
    fn return_cleanup(&self, droppable: impl Fn(VarId) -> bool) -> Vec<CleanupDrop> {
        let mut held: Vec<VarId> = self
            .return_live
            .iter()
            .copied()
            .filter(|v| droppable(*v) && !self.return_ended.contains(v))
            .collect();
        held.sort_unstable_by(|a, b| b.cmp(a));
        held.into_iter().map(CleanupDrop::unnamed).collect()
    }
}

impl Splice<'_> {
    /// Whether the function drops the value in `v` ([`is_droppable_root`]);
    /// a value parameter's slot never.
    fn droppable(&self, v: VarId) -> bool {
        is_droppable_root(self.f, v) && !self.parameter_slots.contains(&v)
    }

    /// `live` with the owners the loan generations of `state` keep alive
    /// through it.
    fn effective(&self, live: &DenseSet, state: &LoanGenerationState) -> HashSet<VarId> {
        live.iter()
            .chain(loan_extras(live, state, self.loan_roots).iter())
            .collect()
    }

    /// A region's effective live-in, its generations those at `state`.
    fn region_live_in(&self, region: &[MirBlock], state: &LoanGenerationState) -> HashSet<VarId> {
        region
            .first()
            .and_then(|block| self.plan.blocks.get(&std::ptr::from_ref(block)))
            .map(|plan| self.effective(&plan.live_in, state))
            .unwrap_or_default()
    }

    /// Place the drops of every block of a region, returning the slots a
    /// raise leaving it still holds.
    fn region(&self, blocks: &mut [MirBlock], cx: &PartCx) -> DenseSet {
        let mut held = DenseSet::default();
        for block in blocks {
            held.union(&self.block(block, cx));
        }
        held
    }

    /// Place the drops of one block: its region ops' first, then its own,
    /// and list on a `return` what it holds past the `finally`s it crosses.
    /// Returns the slots a raise leaving the block still holds.
    fn block(&self, block: &mut MirBlock, cx: &PartCx) -> DenseSet {
        let Some(plan) = self.plan.blocks.get(&std::ptr::from_ref(&*block)) else {
            return DenseSet::default();
        };
        let mut steps: Vec<Step<'_>> = block
            .instrs
            .iter()
            .map(|instr| Step {
                generations: self.facts.before(instr),
                uses: self.facts.uses(instr),
                op: region_op(instr)
                    .then(|| self.plan.ops.get(&std::ptr::from_ref(instr)))
                    .flatten()
                    .map(|live_in| (live_in, DenseSet::default())),
            })
            .collect();
        let end = self.facts.end(block);
        for (index, instr) in block.instrs.iter_mut().enumerate() {
            if steps[index].op.is_none() {
                continue;
            }
            let held = self.op(instr, cx);
            if let Some((_, nested)) = &mut steps[index].op {
                *nested = held;
            }
        }
        let replay = BlockReplay {
            instrs: std::mem::take(&mut block.instrs),
            steps,
            end,
            yielded: self.facts.yielded(block),
            raise: plan.raise.as_ref(),
            deinit_params: &self.f.deinit_params,
            loan_roots: self.loan_roots,
        };
        let (instrs, held) = replay.elaborate(&plan.live_out, |v| self.droppable(v));
        block.instrs = instrs;
        if let MirTerm::Return(value) | MirTerm::ReturnWithCleanup { value, .. } = &mut block.term {
            let value = value.take();
            let cleanup = cx.return_cleanup(|v| self.droppable(v));
            block.term = if cleanup.is_empty() {
                MirTerm::Return(value)
            } else {
                MirTerm::ReturnWithCleanup { value, cleanup }
            };
        }
        held
    }

    /// Place the drops of one region op's regions. An `if` or `comptime if`
    /// destroys at each arm's entry what a sibling arm keeps and it does
    /// not, as `destroyValuesAtEntryIfNeeded` does; a `try` sets its
    /// raise-edge cleanup. Returns the slots a raise leaving the op still
    /// holds.
    fn op(&self, instr: &mut MirInstr, cx: &PartCx) -> DenseSet {
        if matches!(instr, MirInstr::Try { .. }) {
            return self.try_op(instr, cx);
        }
        let mut held = DenseSet::default();
        for region in instruction_regions_mut(instr) {
            held.union(&self.region(region, cx));
        }
        let regions = instruction_regions(instr);
        let entries = match instr {
            MirInstr::If { .. } => self.if_entry_drops(&regions),
            MirInstr::ComptimeIf { .. } => self.comptime_if_entry_drops(&regions),
            _ => Vec::new(),
        };
        if !entries.is_empty() {
            for (region, drops) in instruction_regions_mut(instr).into_iter().zip(entries) {
                if let Some(entry) = region.first_mut() {
                    entry.instrs.splice(0..0, drops.into_iter().map(drop_var));
                }
            }
        }
        held
    }

    /// The entry drops of an `if`'s regions, listed as `then`, each elif's
    /// condition and arm, and `else`: as `checkIfOp` unifies them, each
    /// step's arm against the next condition (or the `else`).
    fn if_entry_drops(&self, regions: &[&Vec<MirBlock>]) -> Vec<Vec<VarId>> {
        let mut drops = vec![Vec::new(); regions.len()];
        // Step `k` takes the branch to arm `regions[2k]` or on to
        // `regions[2k + 1]`.
        for arm in (0..regions.len() - 1).step_by(2) {
            let state = self.facts.region_entry(regions[arm]);
            let taken = self.region_live_in(regions[arm], state);
            let next = self.region_live_in(regions[arm + 1], state);
            drops[arm] = self.dying(&taken, &next);
            drops[arm + 1] = self.dying(&next, &taken);
        }
        drops
    }

    /// The entry drops of a `comptime if`'s arms and `else`: each arm is
    /// unified against the arms after it and the `else` in turn, and what
    /// a step destroys on the way on runs before each later arm's own.
    fn comptime_if_entry_drops(&self, regions: &[&Vec<MirBlock>]) -> Vec<Vec<VarId>> {
        let mut drops: Vec<Vec<VarId>> = vec![Vec::new(); regions.len()];
        for arm in 0..regions.len() - 1 {
            let state = self.facts.region_entry(regions[arm]);
            let taken = self.region_live_in(regions[arm], state);
            let mut rest = HashSet::new();
            for region in &regions[arm + 1..] {
                rest.extend(self.region_live_in(region, state));
            }
            drops[arm].extend(self.dying(&taken, &rest));
            let on = self.dying(&rest, &taken);
            for later in &mut drops[arm + 1..] {
                later.extend(on.iter().copied());
            }
        }
        drops
    }

    /// What dies entering a region whose effective live-in is `live` when
    /// its sibling's is `sibling`, in reverse declaration order.
    fn dying(&self, live: &HashSet<VarId>, sibling: &HashSet<VarId>) -> Vec<VarId> {
        let mut dying: Vec<VarId> = sibling
            .iter()
            .copied()
            .filter(|v| !live.contains(v) && self.droppable(*v))
            .collect();
        dying.sort_unstable_by(|a, b| b.cmp(a));
        dying
    }

    /// Place the drops of a `try`'s four parts and set its raise-edge
    /// cleanup: the `finally` under the enclosing part's channels, the
    /// others under the `try`'s own. A return crossing the `finally` holds
    /// what the `finally` reads on the return path, less what the `finally`
    /// ends on the way through. The handler's unread error binding dies on
    /// arrival, and the raise edge destroys what a raise in the body holds
    /// that the raise's observers (the handler, else the `finally` and the
    /// enclosing raise's) do not read, as the entry destructors of
    /// upstream's error arm do. Returns the slots a raise leaving the `try`
    /// still holds: an uncaught body raise past the raise edge, a raise out
    /// of the handler or `else`, each past what the `finally` ends, and a
    /// raise out of the `finally` itself.
    fn try_op(&self, instr: &mut MirInstr, cx: &PartCx) -> DenseSet {
        let Some(channels) = self.plan.tries.get(&std::ptr::from_ref(&*instr)) else {
            return DenseSet::default();
        };
        let MirInstr::Try {
            body,
            handler,
            orelse,
            finalbody,
            cleanup,
        } = instr
        else {
            return DenseSet::default();
        };
        let after_live: HashSet<VarId> = channels.after.iter().collect();
        // The `finally` ends, on every path through it, what it holds live
        // that its fall-off does not need.
        let fin_ended: HashSet<VarId> = if finalbody.is_some() {
            channels
                .fin
                .iter()
                .filter(|v| !after_live.contains(v) && self.droppable(*v))
                .collect()
        } else {
            HashSet::new()
        };
        let fin_held = finalbody.as_mut().map(|fb| self.region(fb, cx));
        let inner = PartCx {
            return_live: channels.returns.iter().collect(),
            return_ended: cx.return_ended.union(&fin_ended).copied().collect(),
        };
        let handler_held = handler.as_mut().map(|(binding, h)| {
            let held = self.region(h, &inner);
            // The caught error arrives as the handler entry's block argument,
            // so the binding's pre-raise content is never observable, and an
            // unread binding dies on arrival.
            if let Some(bound) = binding
                && self.droppable(*bound)
                && !channels
                    .handler
                    .as_ref()
                    .is_some_and(|live| live.contains(*bound))
                && let Some(entry) = h.first_mut()
            {
                entry.instrs.insert(0, drop_var(*bound));
            }
            held
        });
        let orelse_held = orelse.as_mut().map(|e| self.region(e, &inner));
        let observed = body
            .first()
            .and_then(|block| self.plan.blocks.get(&std::ptr::from_ref(block)))
            .and_then(|plan| plan.raise.clone())
            .unwrap_or_default();
        let body_held = self.region(body, &inner);

        // A slot consumed in place (a `ConsumeVar`, a whole-root move
        // capture) stays the body's to end: an explicit destructor that
        // raises leaves its receiver live for the handler's fallback.
        let consumed = consumed_in_place(body);
        let mut listed: Vec<VarId> = body_held
            .iter()
            .filter(|v| !observed.contains(*v) && !consumed.contains(v) && self.droppable(*v))
            .collect();
        listed.sort_unstable_by(|a, b| b.cmp(a));
        *cleanup = listed.into_iter().map(CleanupDrop::unnamed).collect();

        let mut raise_held = DenseSet::default();
        if handler_held.is_none() {
            raise_held.extend(body_held.iter().filter(|v| !cleanup_drops(cleanup, *v)));
        }
        for held in [handler_held, orelse_held].into_iter().flatten() {
            raise_held.union(&held);
        }
        for v in &fin_ended {
            raise_held.remove(*v);
        }
        if let Some(held) = fin_held {
            raise_held.union(&held);
        }
        raise_held
    }
}

/// One instruction's facts as its block's replay reads them.
struct Step<'a> {
    /// The loan generations reaching it.
    generations: &'a LoanGenerationState,
    /// The roots it consumes through its operand registers.
    uses: &'a RegisterUses,
    /// For a region op: its live-in, and the slots a raise leaving it holds.
    op: Option<(&'a DenseSet, DenseSet)>,
}

/// One block's instructions with the analysis facts their drop elaboration
/// reads.
struct BlockReplay<'a> {
    instrs: Vec<MirInstr>,
    steps: Vec<Step<'a>>,
    /// The generations at the terminator; `None` when it is unreachable.
    end: Option<&'a LoanGenerationState>,
    /// The roots the terminator's yielded values consume.
    yielded: &'a RegisterUses,
    /// What a raising instruction keeps live: the raise channel.
    raise: Option<&'a DenseSet>,
    /// The function's `deinit` parameters, whose fields
    /// [`refine_deinit_fields`] destroys one by one: the variable's own
    /// teardown stays after the instruction.
    deinit_params: &'a [bool],
    loan_roots: &'a BTreeMap<u32, DropLoanGeneration>,
}

impl BlockReplay<'_> {
    /// The block's instructions with a `DropVar` after each one for every
    /// variable that dies there, in one backward walk from `live_out`, and
    /// the slots a raise out of the block still holds: the effective
    /// liveness entering each raising instruction, which the elaboration
    /// leaves held, and what a raise leaving each region op holds.
    ///
    /// A variable dies at an instruction when it is live (loan-aware) before
    /// it, or the instruction touches it, or a loan generation keeping it
    /// alive retires there, and it is dead after it. A raising instruction
    /// keeps the raise channel live. A region op is no death point: its
    /// regions end what it uses, and its live-in is the walk's.
    fn elaborate(
        self,
        live_out: &DenseSet,
        droppable: impl Fn(VarId) -> bool,
    ) -> (Vec<MirInstr>, DenseSet) {
        let mut live = live_out.clone();
        let mut reversed = Vec::with_capacity(self.instrs.len());
        let mut raise_held = DenseSet::default();
        // The terminator reads the values it yields: an owner whose last use
        // that is dies before it.
        if let Some(end) = self.end
            && !self.yielded.retained.is_empty()
        {
            let extra_after = loan_extras(&live, end, self.loan_roots);
            let added: Vec<VarId> = self
                .yielded
                .retained
                .iter()
                .copied()
                .filter(|v| live.insert(*v))
                .collect();
            let extra_before = loan_extras(&live, end, self.loan_roots);
            let mut dying = Vec::new();
            for v in added.iter().copied().chain(extra_before.iter()) {
                if droppable(v) && !extra_after.contains(v) && !dying.contains(&v) {
                    dying.push(v);
                }
            }
            push_drops_reversed(&mut reversed, dying);
        }
        let mut entering = Vec::new();
        // An in-place binding's deaths, held until its producer is reached so
        // the producer's deaths join them after the binding.
        let mut pending: Option<InPlaceDeaths> = None;
        let count = self.instrs.len();
        for (i, instr) in self.instrs.into_iter().enumerate().rev() {
            let step = &self.steps[i];
            let generation_before = step.generations;
            if let Some((live_in, held)) = &step.op {
                if let Some(binding) = pending.take() {
                    binding.splice(&mut reversed, Vec::new());
                }
                raise_held.union(held);
                live = (*live_in).clone();
                reversed.push(instr);
                continue;
            }
            let generation_after = if i + 1 < count {
                self.steps[i + 1].generations
            } else {
                self.end.unwrap_or(generation_before)
            };
            let extra_after = loan_extras(&live, generation_after, self.loan_roots);

            // The backward step, remembering which variables it made live.
            let def = var_def(&instr);
            let uses: Vec<VarId> = var_uses(&instr).into_iter().map(|(v, _)| v).collect();
            let removed_def = def.is_some_and(|d| live.remove(d));
            entering.clear();
            entering.extend(&uses);
            if let Some(raise) = self.raise.filter(|_| raises_to_handler(&instr)) {
                entering.extend(raise.iter());
            }
            // A pending teardown keeps its variable live until then.
            entering.extend(teardown(&instr));
            // What the instruction itself, or a raise leaving it, still reads.
            let observed = entering.len();
            let register_uses = step.uses;
            entering.extend(&register_uses.retained);
            entering.extend(&register_uses.loaded);
            let mut added = Vec::new();
            for v in &entering {
                if live.insert(*v) && !(removed_def && def == Some(*v)) {
                    added.push(*v);
                }
            }
            let extra_before = loan_extras(&live, generation_before, self.loan_roots);
            let live_after = |v: VarId| {
                (live.contains(v) && !added.contains(&v))
                    || (removed_def && def == Some(v))
                    || extra_after.contains(v)
            };

            let mut moved = vars_moved(&instr);
            moved.extend(teardown(&instr));
            let retired: Vec<VarId> = if std::ptr::eq(generation_before, generation_after)
                || generation_before == generation_after
            {
                Vec::new()
            } else {
                let roots_after = active_drop_loan_roots(generation_after, self.loan_roots);
                active_drop_loan_roots(generation_before, self.loan_roots)
                    .difference(&roots_after)
                    .copied()
                    .collect()
            };
            let deaths = added
                .iter()
                .copied()
                .chain(extra_before.iter())
                .chain(uses)
                .chain(def)
                .chain(register_uses.retained.iter().copied())
                .chain(register_uses.loaded.iter().copied())
                .chain(retired.iter().copied());
            let mut dying: Vec<VarId> = Vec::new();
            for v in deaths {
                if droppable(v) && !moved.contains(&v) && !live_after(v) && !dying.contains(&v) {
                    dying.push(v);
                }
            }
            // An owner whose last use is a scalar the instruction loads by
            // value dies at that load, before the instruction runs.
            let deinit = |v: VarId| self.deinit_params.get(v as usize).copied() == Some(true);
            let loaded: Vec<VarId> = register_uses
                .loaded
                .iter()
                .copied()
                .filter(|v| {
                    dying.contains(v)
                        && !deinit(*v)
                        && !entering[..observed].contains(v)
                        && def != Some(*v)
                        && !retired.contains(v)
                        && !extra_before.contains(*v)
                })
                .collect();
            dying.retain(|v| !loaded.contains(v));
            if instr.is_raise_point() {
                let mut held = live.clone();
                held.union(&extra_before);
                for v in vars_moved(&instr).into_iter().chain(loaded.iter().copied()) {
                    held.remove(v);
                }
                raise_held.union(&held);
            }
            if let Some(held) = pending.take() {
                // The slot's previous value dying at the producer must drop
                // before the binding overwrites it, so that death point stays.
                if held.produced_by(&instr) && !dying.contains(&held.var) {
                    held.splice(&mut reversed, std::mem::take(&mut dying));
                } else {
                    held.splice(&mut reversed, Vec::new());
                }
            }
            match &instr {
                MirInstr::DefVar {
                    var,
                    src,
                    in_place: true,
                    ..
                } => {
                    pending = Some(InPlaceDeaths {
                        mark: reversed.len(),
                        var: *var,
                        src: *src,
                        dying,
                    });
                }
                _ => push_drops_reversed(&mut reversed, dying),
            }
            reversed.push(instr);
            reversed.extend(loaded.into_iter().rev().map(drop_var));
        }
        if let Some(held) = pending {
            held.splice(&mut reversed, Vec::new());
        }
        reversed.reverse();
        (reversed, raise_held)
    }
}

/// The variables dying at an in-place `DefVar`, held in the backward walk
/// until its producer: Mojo initializes the binding as the call's result
/// storage, so the call's own deaths (argument temporaries, owners last used
/// by the call) come after the binding, and the binding's slot, dead at its
/// definition, drops ahead of them.
struct InPlaceDeaths {
    /// Where the binding's trailing drops belong in the reversed output.
    mark: usize,
    var: VarId,
    src: Reg,
    dying: Vec<VarId>,
}

impl InPlaceDeaths {
    fn produced_by(&self, instr: &MirInstr) -> bool {
        let mut results = Vec::new();
        mojito_mir::mir::verify::instruction_result_regs(instr, &mut results);
        results.contains(&self.src)
    }

    /// Splice the binding's deaths and its producer's into `reversed`: the
    /// binding's slot first, the rest in descending `VarId`.
    fn splice(self, reversed: &mut Vec<MirInstr>, producer: Vec<VarId>) {
        let own = self.dying.contains(&self.var);
        let mut rest: Vec<VarId> = self
            .dying
            .into_iter()
            .filter(|v| *v != self.var)
            .chain(producer)
            .collect();
        rest.sort_unstable();
        rest.dedup();
        let drops = rest
            .into_iter()
            .chain(own.then_some(self.var))
            .map(drop_var);
        reversed.splice(self.mark..self.mark, drops);
    }
}

/// The variable an instruction tears down: a `ConsumeVar`, or a `DropVar`
/// already in the input. Its slot is live up to it and dead after it, so
/// elaboration splices no second drop of it.
const fn teardown(instr: &MirInstr) -> Option<VarId> {
    match instr {
        MirInstr::ConsumeVar { var } | MirInstr::DropVar { var, .. } => Some(*var),
        _ => None,
    }
}

/// The variables `live` keeps alive through the loan generations of `state`
/// beyond itself: the owner roots of every generation a live reference (or a
/// root kept alive this way) holds, closed transitively.
fn loan_extras(
    live: &DenseSet,
    state: &LoanGenerationState,
    roots_by_generation: &BTreeMap<u32, DropLoanGeneration>,
) -> DenseSet {
    let mut extras = DenseSet::default();
    loop {
        let mut changed = false;
        for (reference, generations) in &state.active {
            if !live.contains(*reference) && !extras.contains(*reference) {
                continue;
            }
            for generation in generations {
                let Some(generation) = roots_by_generation.get(generation) else {
                    continue;
                };
                for root in &generation.roots {
                    if !live.contains(*root) {
                        changed |= extras.insert(*root);
                    }
                }
            }
        }
        if !changed {
            return extras;
        }
    }
}

/// Push `DropVar`s in reverse declaration order onto a block being built
/// back to front.
fn push_drops_reversed(reversed: &mut Vec<MirInstr>, mut vars: Vec<VarId>) {
    vars.sort_unstable();
    reversed.extend(vars.into_iter().map(drop_var));
}

/// Prepend `DropVar`s (reverse declaration order) to the front of a block.
fn prepend_drops(instrs: &mut Vec<MirInstr>, mut vars: Vec<VarId>) {
    vars.sort_unstable_by(|a, b| b.cmp(a));
    instrs.splice(0..0, vars.into_iter().map(drop_var));
}

/// A `DropVar` of `var`, its destructor named later.
const fn drop_var(var: VarId) -> MirInstr {
    MirInstr::DropVar {
        var,
        destructor: None,
    }
}
