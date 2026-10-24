//! Drop elaboration: placing `DropVar`s and `DropPlace`s in the structured
//! body, as upstream's `DestructorInsertion` places destructors.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_mir::mir::{instruction_regions, instruction_regions_mut};

/// Insert a drop at each value's death, on the structured body, as
/// upstream's `DestructorInsertion` does: a `DropVar` of a variable, or a
/// `DropPlace` of a part of a root `values` lays out, which dies alone
/// where its whole-object bit stays live (a part a store redefines). One
/// backward walk ([`walk_backward`]) finds where each value dies, and a
/// splice pass places its drop:
///
/// - right after the instruction that is its last use (touched, then dead),
///   skipping variables moved out with `^` (their new owner drops them);
/// - at the entry of an `if` or `comptime if` arm that leaves out a value a
///   sibling arm keeps (`destroyValuesAtEntryIfNeeded`);
/// - before a block's terminator, when the values the block yields were the
///   last use.
///
/// An initializer's `out self` `receiver` is a root too, which every
/// `return` reads: it dies only where a path leaves without returning, a
/// raise's, and [`refine_receiver_drops`] destroys there what it holds.
/// A `deinit` parameter laid out per field is marked destroyed at every
/// exit, as upstream marks a `DeinitMem` argument: each field dies at its
/// own last use, and a `ConsumeVar` before the exit ends the parameter
/// unless every path there already has.
///
/// At a shared death point, variables drop in reverse declaration order
/// (descending `VarId`), the receiver first, as the pin destroys a partly
/// built `self` before the values dying with it, except that an `in_place`
/// binding joins its producer's death point and its slot drops first (see
/// [`InPlaceDeaths`]). No block is added: MIR lowering copied each
/// `finally` before the exits crossing it, so every death has an
/// instruction to follow or a region entry to lead, a raise's at its arm's
/// entry or before it.
pub(super) fn elaborate_drops(
    mut function: MirFunction,
    value_parameters: &HashSet<VarId>,
    receiver: Option<VarId>,
    values: &DropValues,
    declarations: &MirDeclarations,
) -> MirFunction {
    let passing = ArgumentPassing::new(declarations);
    let loan_roots = drop_loan_generations(&function, values);
    let generation_dests = loan_generation_dests(&function);
    let split_parameters = |flags: &[bool]| -> Vec<VarId> {
        (0..function.n_params)
            .map(|v| v as VarId)
            .filter(|v| values.is_split(*v) && flags.get(*v as usize).copied() == Some(true))
            .collect()
    };
    // A `mut` or `ref` parameter laid out in parts must be whole again
    // wherever the function leaves, as upstream's `checkTerminatorOp`
    // demands a value live on exit; a `deinit` parameter is marked
    // destroyed there.
    let demands = ExitDemands {
        receiver,
        parameters: split_parameters(&function.ref_params),
        deinit: split_parameters(&function.deinit_params),
    };
    let facts = DropFacts::new(
        &function,
        &loan_roots,
        &generation_dests,
        &passing,
        values,
        &demands.deinit,
    );
    let mut plan = DropPlan::default();
    let liveness = DropLiveness {
        facts: &facts,
        loan_roots: &loan_roots,
        values,
        demands: &demands,
    };
    walk_backward(&liveness, &function.blocks, &mut plan);
    // Taking the blocks moves no block: the plan's addresses stay valid.
    let mut blocks = std::mem::take(&mut function.blocks);
    let splice = Splice {
        f: &function,
        parameter_slots: value_parameters,
        receiver,
        demands: &demands,
        loan_roots: &loan_roots,
        values,
        facts: &facts,
        plan: &plan,
    };
    // A consuming parameter the body never uses is dead on arrival: Mojo
    // destroys an unused `deinit self`'s fields before the first statement
    // runs, so it drops at the function's entry; so does a parameter's part
    // the body redefines before reading it (upstream's `scanFunction`).
    let entry_dead: Vec<Death> = blocks
        .first()
        .and_then(|entry| {
            let live = splice.plan.blocks.get(&std::ptr::from_ref(entry))?;
            let live = splice.effective(&live.live_in, facts.entry(entry));
            let parameters = (0..function.n_params).map(|v| v as VarId);
            let dying: DenseSet = parameters
                .clone()
                .flat_map(|v| values.range(v))
                .filter(|bit| !live.contains(*bit))
                .collect();
            let mut deaths = Vec::new();
            for v in parameters {
                splice.deaths(v, &dying, &live, &mut deaths);
            }
            Some(deaths)
        })
        .unwrap_or_default();
    splice.region(&mut blocks);
    if let Some(entry) = blocks.first_mut() {
        prepend_drops(&mut entry.instrs, entry_dead, receiver);
    }

    // A `deinit` parameter is *consumed*, not destroyed: the spliced teardown
    // for it must skip the value's whole-value `__deinit__` (its resources already
    // moved into the receiver) while still destroying any residual fields. Where
    // one dies whole — one not laid out per field, at its last use, or one whose
    // whole-object bit dies on a path reaching no exit — drop elaboration emits
    // an ordinary `DropVar`; rewrite those to `ConsumeVar`. Their
    // `^`-transferred fields are `Value::Moved` and a no-op to drop, so this
    // cannot double-free.
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

/// The owner parts carried by each `EstablishLoans` generation. A later
/// establishment for the same aggregate replaces the old marker; keeping the
/// marker as the key prevents historical owners from becoming one permanent
/// dependency of the variable slot.
pub(super) struct DropLoanGeneration {
    /// The part each loan's place reaches ([`DropValues::access`]).
    pub(super) roots: Vec<ValueRef>,
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
pub(super) fn drop_loan_generations(
    f: &MirFunction,
    values: &DropValues,
) -> BTreeMap<u32, DropLoanGeneration> {
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
                let part = values.access(&loan.place);
                if !generation.roots.contains(&part) {
                    generation.roots.push(part);
                }
            }
        }
    });
    roots_by_generation
}

pub(super) fn active_drop_loan_roots(
    state: &LoanGenerationState,
    roots_by_generation: &BTreeMap<u32, DropLoanGeneration>,
) -> BTreeSet<ValueRef> {
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
            },
        );
    }

    fn try_op(&mut self, op: &MirInstr, channels: &TryChannels<DenseSet>) {
        self.tries.insert(std::ptr::from_ref(op), channels.clone());
    }
}

/// Backward drop liveness over the bits of [`DropValues`]: a value is live
/// from a use to its definition, a part laid out in a split root from a use
/// to the store defining it. A use is a direct read, a pending teardown, or
/// a root an instruction consumes through its operand registers
/// ([`DropFacts::uses`]), counted at its exact consuming point, so a
/// register loaded in one region and consumed in another keeps its owner
/// live between them. Loan-generation owners are not folded in, since
/// generations join where paths meet and would keep every historical owner
/// of a rebound aggregate live; the splice adds them per point, and a `try`
/// part's live-in as the `try` sees it. A `return` reads an initializer's
/// receiver, the value it returns, and every exit a split `mut` parameter
/// and a split `deinit` parameter's whole-object bit.
struct DropLiveness<'a> {
    facts: &'a DropFacts,
    loan_roots: &'a BTreeMap<u32, DropLoanGeneration>,
    values: &'a DropValues,
    demands: &'a ExitDemands,
}

impl BackwardScan for DropLiveness<'_> {
    type State = DenseSet;

    fn join(&self, state: &mut DenseSet, other: &DenseSet) {
        self.values.unify(state, other);
    }

    fn transfer(&self, live: &mut DenseSet, instr: &MirInstr) {
        effects(self.values, live, instr, self.facts.uses(instr));
    }

    fn enters(&self, _instr: &MirInstr) -> bool {
        true
    }

    fn define(&self, live: &mut DenseSet, var: VarId) {
        self.values.clear_all(live, var);
    }

    fn terminate(&self, live: &mut DenseSet, block: &MirBlock) {
        for part in &self.facts.yielded(block).retained {
            self.values.mark_ref(live, *part);
        }
        self.demands.at_exit(self.values, live, &block.term);
    }

    fn loop_entry(&self, live: &mut DenseSet) {
        self.values.mark_reached(live);
    }

    fn enter(&self, live: &mut DenseSet, part: &[MirBlock]) {
        let extras = loan_extras(
            live,
            self.facts.region_entry(part),
            self.loan_roots,
            self.values,
        );
        live.union(&extras);
    }
}

/// The values a function's exits read: an initializer's receiver, which a
/// `return` returns, the split `mut` and `ref` parameters, which every
/// `return` and every raise out of the function hands back whole, and the
/// whole-object bit of each split `deinit` parameter, which every such exit
/// marks destroyed, as upstream's `mark_destroyed` of a `DeinitMem`
/// argument before each `return` and raise does: what a field holds then
/// died at the field's own last use.
struct ExitDemands {
    receiver: Option<VarId>,
    parameters: Vec<VarId>,
    deinit: Vec<VarId>,
}

impl ExitDemands {
    /// Add to `live` what the terminator `term` demands.
    fn at_exit(&self, values: &DropValues, live: &mut DenseSet, term: &MirTerm) {
        let returns = matches!(term, MirTerm::Return(_));
        if let (Some(receiver), true) = (self.receiver, returns) {
            values.mark_all(live, receiver);
        }
        if leaves(term) {
            values.mark_reached(live);
            for parameter in &self.parameters {
                values.mark_all(live, *parameter);
            }
            for parameter in &self.deinit {
                values.mark_whole_bit(live, *parameter);
            }
        }
    }

    /// The `ConsumeVar`s before the exit `term` that end the `deinit`
    /// parameters no path to it has ended already (`ended`).
    fn consumed_at_exit(&self, term: &MirTerm, ended: &BTreeSet<VarId>) -> Vec<MirInstr> {
        if !leaves(term) {
            return Vec::new();
        }
        self.deinit
            .iter()
            .filter(|parameter| !ended.contains(parameter))
            .map(|parameter| MirInstr::ConsumeVar { var: *parameter })
            .collect()
    }
}

/// Whether `term` leaves the function: a `return`, or a raise out of it.
const fn leaves(term: &MirTerm) -> bool {
    matches!(
        term,
        MirTerm::Return(_) | MirTerm::Raise { label: None, .. }
    )
}

/// What one instruction does to the live bits, walked backward.
#[derive(Default)]
struct Effects {
    /// The variable it defines whole.
    def: Option<VarId>,
    /// The bits it defines: its variable's, or a laid-out part a store
    /// fills.
    defined: Vec<std::ops::Range<u32>>,
    /// The bits its uses make live.
    marked: Vec<std::ops::Range<u32>>,
    /// The laid-out parts it moves out or ends, which their new owner
    /// destroys.
    consumed: Vec<std::ops::Range<u32>>,
    /// The bits its own places and variables make live, beside those its
    /// operand registers do.
    direct: Vec<std::ops::Range<u32>>,
    /// The variables it moves out or tears down whole.
    moved: Vec<VarId>,
}

/// Undo `instr` on `live`, upstream's `scanBlock` operand effects: kill
/// what it defines, a store into a laid-out part that part alone, then add
/// what it uses, a use of the part a place reaches as
/// [`DropValues::mark_ref`] marks it.
fn effects(
    values: &DropValues,
    live: &mut DenseSet,
    instr: &MirInstr,
    uses: &RegisterUses,
) -> Effects {
    let mut effects = Effects {
        def: var_def(instr),
        ..Effects::default()
    };
    if let Some(def) = effects.def {
        values.clear_all(live, def);
        effects.defined.push(values.range(def));
    }
    let accesses = instruction_uses(instr);
    let stored = |used: &Used<'_>| match (instr, used) {
        (MirInstr::Store { .. }, Used::Place(place)) => place
            .owned_path()
            .and_then(|path| values.part(place.root, &path)),
        _ => None,
    };
    for (used, _) in &accesses {
        if let Some(part) = stored(used) {
            // As `checkDef`: what is dead after the store dies there, the
            // whole value when its whole-object bit is, and the part holds
            // no live value before it.
            if let Used::Place(place) = used {
                effects.mark(values.mark_ref(live, values.access(place)));
            }
            for bit in part.clone() {
                live.remove(bit);
            }
            effects.defined.push(part);
        }
    }
    let consuming = matches!(
        instr,
        MirInstr::MovePlace { .. }
            | MirInstr::ConsumePlace { .. }
            | MirInstr::DropPlace { .. }
            | MirInstr::MarkDestroyed { .. }
    );
    for (used, _) in &accesses {
        if stored(used).is_some() {
            continue;
        }
        match used {
            Used::Var(v) => effects.mark(values.mark_ref(live, values.whole(*v))),
            Used::Place(place) => {
                if let Some(through) = place.through {
                    effects.mark(values.mark_ref(live, values.whole(through)));
                }
                effects.mark(values.mark_ref(live, values.access(place)));
                if consuming
                    && let Some(part) = place
                        .owned_path()
                        .and_then(|path| values.part(place.root, &path))
                {
                    effects.consumed.push(part);
                }
            }
        }
    }
    // A pending teardown keeps its variable live up to that point so no
    // earlier death splices a competing `DropVar`.
    if let Some(v) = teardown(instr) {
        effects.mark(values.mark_ref(live, values.whole(v)));
        effects.moved.push(v);
    }
    effects.moved.extend(vars_moved(instr));
    for part in uses.retained.iter().chain(&uses.loaded) {
        effects.marked.push(values.mark_ref(live, *part));
    }
    effects
}

impl Effects {
    /// Record bits the instruction's own places or variables make live.
    fn mark(&mut self, bits: std::ops::Range<u32>) {
        self.marked.push(bits.clone());
        self.direct.push(bits);
    }
}

/// The splice pass: the plan's drops placed in the body, nested regions
/// before the blocks holding them.
struct Splice<'a> {
    f: &'a MirFunction,
    /// The function's compile-time value parameter slots
    /// (`mojito_mir::mir::parameter_slots`), which it never drops.
    parameter_slots: &'a HashSet<VarId>,
    /// An initializer's `out self`, which a path leaving without returning
    /// drops.
    receiver: Option<VarId>,
    demands: &'a ExitDemands,
    loan_roots: &'a BTreeMap<u32, DropLoanGeneration>,
    values: &'a DropValues,
    facts: &'a DropFacts,
    plan: &'a DropPlan,
}

impl Splice<'_> {
    /// Whether the function drops the value in `v` whole
    /// ([`is_droppable_root`]), or an initializer's receiver; a value
    /// parameter's slot never.
    fn droppable(&self, v: VarId) -> bool {
        self.receiver == Some(v)
            || (is_droppable_root(self.f, v) && !self.parameter_slots.contains(&v))
    }

    /// The deaths of `root` whose bits in `dying` die with `live` live
    /// after ([`DropValues::deaths`]), whole only where it is droppable.
    fn deaths(&self, root: VarId, dying: &DenseSet, live: &DenseSet, out: &mut Vec<Death>) {
        self.values
            .deaths(root, dying, live, self.droppable(root), out);
    }

    /// The deaths of every root with bits in `dying`, with `live` live
    /// after.
    fn all_deaths(&self, dying: &DenseSet, live: &DenseSet) -> Vec<Death> {
        let mut roots: Vec<VarId> = dying
            .iter()
            .filter(|bit| self.values.is_value_bit(*bit))
            .map(|bit| self.values.owner(bit))
            .collect();
        roots.dedup();
        let mut deaths = Vec::new();
        for root in roots {
            self.deaths(root, dying, live, &mut deaths);
        }
        deaths
    }

    /// `live` with the owners the loan generations of `state` keep alive
    /// through it.
    fn effective(&self, live: &DenseSet, state: &LoanGenerationState) -> DenseSet {
        let mut effective = live.clone();
        effective.union(&loan_extras(live, state, self.loan_roots, self.values));
        effective
    }

    /// A region's effective live-in, its generations those at `state`.
    fn region_live_in(&self, region: &[MirBlock], state: &LoanGenerationState) -> DenseSet {
        region
            .first()
            .and_then(|block| self.plan.blocks.get(&std::ptr::from_ref(block)))
            .map(|plan| self.effective(&plan.live_in, state))
            .unwrap_or_default()
    }

    /// Place the drops of every block of a region.
    fn region(&self, blocks: &mut [MirBlock]) {
        for block in blocks {
            self.block(block);
        }
    }

    /// Place the drops of one block: its region ops' first, then its own.
    fn block(&self, block: &mut MirBlock) {
        let Some(plan) = self.plan.blocks.get(&std::ptr::from_ref(&*block)) else {
            return;
        };
        let steps: Vec<Step<'_>> = block
            .instrs
            .iter()
            .map(|instr| Step {
                generations: self.facts.before(instr),
                uses: self.facts.uses(instr),
                op: region_op(instr)
                    .then(|| self.plan.ops.get(&std::ptr::from_ref(instr)))
                    .flatten(),
            })
            .collect();
        let end = self.facts.end(block);
        for (index, instr) in block.instrs.iter_mut().enumerate() {
            if steps[index].op.is_some() {
                self.op(instr);
            }
        }
        let replay = BlockReplay {
            instrs: std::mem::take(&mut block.instrs),
            steps,
            end,
            yielded: self.facts.yielded(block),
            receiver: self.receiver,
            loan_roots: self.loan_roots,
            splice: self,
        };
        let mut live_out = plan.live_out.clone();
        self.demands
            .at_exit(self.values, &mut live_out, &block.term);
        block.instrs = replay.elaborate(&live_out);
        if let Some(ended) = self.facts.ended(block) {
            let consumed = self.demands.consumed_at_exit(&block.term, ended);
            block.instrs.extend(consumed);
        }
    }

    /// Place the drops of one region op's regions. An `if` or `comptime if`
    /// destroys at each arm's entry what the arms' unified live-in keeps and
    /// it does not, as `destroyValuesAtEntryIfNeeded` does; a `try`'s
    /// handler destroys its unread error binding on arrival.
    fn op(&self, instr: &mut MirInstr) {
        if matches!(instr, MirInstr::Try { .. }) {
            self.try_op(instr);
            return;
        }
        for region in instruction_regions_mut(instr) {
            self.region(region);
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
                    entry
                        .instrs
                        .splice(0..0, drops.into_iter().map(Death::into_drop));
                }
            }
        }
    }

    /// The entry drops of an `if`'s regions, listed as `then`, each elif's
    /// condition and arm, and `else`: as `checkIfOp` unifies them, each
    /// step's arm against the next condition (or the `else`).
    fn if_entry_drops(&self, regions: &[&Vec<MirBlock>]) -> Vec<Vec<Death>> {
        let mut drops = vec![Vec::new(); regions.len()];
        // Step `k` takes the branch to arm `regions[2k]` or on to
        // `regions[2k + 1]`.
        for arm in (0..regions.len() - 1).step_by(2) {
            let state = self.facts.region_entry(regions[arm]);
            let taken = self.region_live_in(regions[arm], state);
            let next = self.region_live_in(regions[arm + 1], state);
            let mut unified = taken.clone();
            self.values.unify(&mut unified, &next);
            drops[arm] = self.dying(&taken, &unified);
            drops[arm + 1] = self.dying(&next, &unified);
        }
        drops
    }

    /// The entry drops of a `comptime if`'s arms and `else`: each arm is
    /// unified against the arms after it and the `else` in turn, and what
    /// a step destroys on the way on runs before each later arm's own.
    fn comptime_if_entry_drops(&self, regions: &[&Vec<MirBlock>]) -> Vec<Vec<Death>> {
        let mut drops: Vec<Vec<Death>> = vec![Vec::new(); regions.len()];
        for arm in 0..regions.len() - 1 {
            let state = self.facts.region_entry(regions[arm]);
            let taken = self.region_live_in(regions[arm], state);
            let mut rest = DenseSet::default();
            for region in &regions[arm + 1..] {
                self.values
                    .unify(&mut rest, &self.region_live_in(region, state));
            }
            let mut unified = taken.clone();
            self.values.unify(&mut unified, &rest);
            drops[arm].extend(self.dying(&taken, &unified));
            let on = self.dying(&rest, &unified);
            for later in &mut drops[arm + 1..] {
                later.extend(on.iter().cloned());
            }
        }
        drops
    }

    /// What dies entering a region whose effective live-in is `live` when
    /// the unified live-in of it and its siblings is `unified`, in drop
    /// order.
    fn dying(&self, live: &DenseSet, unified: &DenseSet) -> Vec<Death> {
        let dying: DenseSet = unified.iter().filter(|bit| !live.contains(*bit)).collect();
        let mut deaths = self.all_deaths(&dying, live);
        order_deaths(&mut deaths, self.receiver);
        deaths
    }

    /// Place the drops of a `try`'s parts. The caught error arrives as the
    /// handler entry's block argument, so the binding's pre-raise content
    /// is never observable, and an unread binding dies on arrival.
    fn try_op(&self, instr: &mut MirInstr) {
        let Some(channels) = self.plan.tries.get(&std::ptr::from_ref(&*instr)) else {
            return;
        };
        let MirInstr::Try {
            body,
            handler,
            orelse,
            ..
        } = instr
        else {
            return;
        };
        if let Some((binding, h)) = handler {
            self.region(h);
            if let Some(bound) = binding
                && self.droppable(*bound)
                && !channels
                    .handler
                    .as_ref()
                    .is_some_and(|live| self.values.any_live(live, *bound))
                && let Some(entry) = h.first_mut()
            {
                entry
                    .instrs
                    .insert(0, Death::var(*bound, self.values.range(*bound)).into_drop());
            }
        }
        if let Some(e) = orelse {
            self.region(e);
        }
        self.region(body);
    }
}

/// One instruction's facts as its block's replay reads them.
struct Step<'a> {
    /// The loan generations reaching it.
    generations: &'a LoanGenerationState,
    /// The parts it consumes through its operand registers.
    uses: &'a RegisterUses,
    /// For a region op: its live-in.
    op: Option<&'a DenseSet>,
}

/// One block's instructions with the analysis facts their drop elaboration
/// reads.
struct BlockReplay<'a> {
    instrs: Vec<MirInstr>,
    steps: Vec<Step<'a>>,
    /// The generations at the terminator; `None` when it is unreachable.
    end: Option<&'a LoanGenerationState>,
    /// The parts the values the terminator hands on consume.
    yielded: &'a RegisterUses,
    /// An initializer's receiver, first at a shared death point.
    receiver: Option<VarId>,
    loan_roots: &'a BTreeMap<u32, DropLoanGeneration>,
    splice: &'a Splice<'a>,
}

impl BlockReplay<'_> {
    /// The block's instructions with a drop after each one for every value
    /// or part that dies there, in one backward walk from `live_out`.
    ///
    /// A value dies at an instruction when it is live (loan-aware) before
    /// it, or the instruction touches or defines it, or a loan generation
    /// keeping it alive retires there, and it is dead after it. A raising
    /// instruction raises through its error region, an `if` whose arm's
    /// entry destroys what the raise does not read. A region op is no death
    /// point: its regions end what it uses, and its live-in is the walk's.
    fn elaborate(self, live_out: &DenseSet) -> Vec<MirInstr> {
        let values = self.splice.values;
        let mut live = live_out.clone();
        let mut reversed = Vec::with_capacity(self.instrs.len());
        // The terminator reads the values it yields: an owner whose last use
        // that is dies before it.
        if let Some(end) = self.end
            && !self.yielded.retained.is_empty()
        {
            let extra_after = loan_extras(&live, end, self.loan_roots, values);
            let mut live_after = live.clone();
            live_after.union(&extra_after);
            let marked: Vec<std::ops::Range<u32>> = self
                .yielded
                .retained
                .iter()
                .map(|part| values.mark_ref(&mut live, *part))
                .collect();
            let extra_before = loan_extras(&live, end, self.loan_roots, values);
            let dying: DenseSet = marked
                .into_iter()
                .flatten()
                .chain(extra_before.iter())
                .filter(|bit| !live_after.contains(*bit))
                .collect();
            push_drops_reversed(
                &mut reversed,
                self.splice.all_deaths(&dying, &live_after),
                self.receiver,
            );
        }
        // An in-place binding's deaths, held until its producer is reached so
        // the producer's deaths join them after the binding.
        let mut pending: Option<InPlaceDeaths> = None;
        let count = self.instrs.len();
        for (i, instr) in self.instrs.into_iter().enumerate().rev() {
            let step = &self.steps[i];
            let generation_before = step.generations;
            if let Some(live_in) = step.op {
                if let Some(binding) = pending.take() {
                    binding.splice(&mut reversed, Vec::new(), self.receiver);
                }
                live = live_in.clone();
                reversed.push(instr);
                continue;
            }
            let generation_after = if i + 1 < count {
                self.steps[i + 1].generations
            } else {
                self.end.unwrap_or(generation_before)
            };
            let extra_after = loan_extras(&live, generation_after, self.loan_roots, values);
            let mut live_after = live.clone();
            live_after.union(&extra_after);

            // The backward step.
            let register_uses = step.uses;
            let effects = effects(values, &mut live, &instr, register_uses);
            let extra_before = loan_extras(&live, generation_before, self.loan_roots, values);

            let retired: Vec<ValueRef> = if std::ptr::eq(generation_before, generation_after)
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
            let mut ended = DenseSet::default();
            for range in &effects.consumed {
                ended.extend(range.clone());
            }
            for v in &effects.moved {
                ended.extend(values.range(*v));
            }
            let dying: DenseSet = effects
                .defined
                .iter()
                .chain(&effects.marked)
                .flat_map(Clone::clone)
                .chain(extra_before.iter())
                .chain(
                    retired
                        .iter()
                        .flat_map(|part| values.ref_bits(&live, *part)),
                )
                .filter(|bit| !live_after.contains(*bit) && !ended.contains(*bit))
                .collect();
            let mut dying = self.splice.all_deaths(&dying, &live_after);
            // An owner whose last use is a scalar the instruction loads by
            // value dies at that load, before the instruction runs: the
            // first loaded part its bits hold ranks it.
            let load_rank = |death: &Death| {
                let bits = death.bits();
                if effects.def == Some(death.root)
                    || effects.direct.iter().any(|marked| overlap(marked, &bits))
                    || retired.iter().any(|part| part.overlaps(&bits))
                    || bits.clone().any(|bit| extra_before.contains(bit))
                {
                    return None;
                }
                register_uses
                    .loaded
                    .iter()
                    .position(|part| part.overlaps(&bits))
            };
            let (mut loaded_deaths, kept): (Vec<Death>, Vec<Death>) = dying
                .into_iter()
                .partition(|death| load_rank(death).is_some());
            dying = kept;
            if let Some(held) = pending.take() {
                // The slot's previous value dying at the producer must drop
                // before the binding overwrites it, so that death point stays.
                if held.produced_by(&instr) && !dying.iter().any(|death| death.root == held.var) {
                    held.splice(&mut reversed, std::mem::take(&mut dying), self.receiver);
                } else {
                    held.splice(&mut reversed, Vec::new(), self.receiver);
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
                _ => push_drops_reversed(&mut reversed, dying, self.receiver),
            }
            // In the order the instruction loads them.
            loaded_deaths.sort_by_key(|death| load_rank(death).unwrap_or(usize::MAX));
            reversed.push(instr);
            reversed.extend(loaded_deaths.into_iter().rev().map(Death::into_drop));
        }
        if let Some(held) = pending {
            held.splice(&mut reversed, Vec::new(), self.receiver);
        }
        reversed.reverse();
        reversed
    }
}

/// The deaths at an in-place `DefVar`, held in the backward walk until its
/// producer: Mojo initializes the binding as the call's result storage, so
/// the call's own deaths (argument temporaries, owners last used by the
/// call) come after the binding, and the binding's slot, dead at its
/// definition, drops ahead of them.
struct InPlaceDeaths {
    /// Where the binding's trailing drops belong in the reversed output.
    mark: usize,
    var: VarId,
    src: Reg,
    dying: Vec<Death>,
}

impl InPlaceDeaths {
    fn produced_by(&self, instr: &MirInstr) -> bool {
        let mut results = Vec::new();
        mojito_mir::mir::verify::instruction_result_regs(instr, &mut results);
        results.contains(&self.src)
    }

    /// Splice the binding's deaths and its producer's into `reversed`: the
    /// binding's slot first, the rest in drop order ([`order_deaths`]).
    fn splice(self, reversed: &mut Vec<MirInstr>, producer: Vec<Death>, receiver: Option<VarId>) {
        let var = self.var;
        let (mut own, rest): (Vec<Death>, Vec<Death>) =
            self.dying.into_iter().partition(|death| death.root == var);
        let mut rest: Vec<Death> = rest.into_iter().chain(producer).collect();
        rest.sort_by_key(|death| death.order(receiver));
        rest.dedup_by(|a, b| a.root == b.root && a.order(receiver) == b.order(receiver));
        order_deaths(&mut own, receiver);
        let drops = rest
            .into_iter()
            .rev()
            .chain(own.into_iter().rev())
            .map(Death::into_drop);
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

/// The bits `live` keeps alive through the loan generations of `state`
/// beyond itself: every bit of the owner roots of every generation a live
/// reference (or a root kept alive this way) holds, closed transitively.
fn loan_extras(
    live: &DenseSet,
    state: &LoanGenerationState,
    roots_by_generation: &BTreeMap<u32, DropLoanGeneration>,
    values: &DropValues,
) -> DenseSet {
    let mut extras = DenseSet::default();
    loop {
        let mut changed = false;
        for (reference, generations) in &state.active {
            if !values.any_live(live, *reference) && !values.any_live(&extras, *reference) {
                continue;
            }
            for generation in generations {
                let Some(generation) = roots_by_generation.get(generation) else {
                    continue;
                };
                for part in &generation.roots {
                    for bit in values.ref_bits(live, *part) {
                        if !live.contains(bit) {
                            changed |= extras.insert(bit);
                        }
                    }
                }
            }
        }
        if !changed {
            return extras;
        }
    }
}

/// Push drops in drop order ([`order_deaths`]) onto a block being built
/// back to front.
fn push_drops_reversed(
    reversed: &mut Vec<MirInstr>,
    mut deaths: Vec<Death>,
    receiver: Option<VarId>,
) {
    order_deaths(&mut deaths, receiver);
    reversed.extend(deaths.into_iter().rev().map(Death::into_drop));
}

/// Prepend drops in drop order ([`order_deaths`]) to the front of a block.
fn prepend_drops(instrs: &mut Vec<MirInstr>, mut deaths: Vec<Death>, receiver: Option<VarId>) {
    order_deaths(&mut deaths, receiver);
    instrs.splice(0..0, deaths.into_iter().map(Death::into_drop));
}

/// Sort the deaths at one point into the order they drop: an initializer's
/// receiver first, then reverse declaration order, a variable's parts in
/// the order its fields are declared, as `destroyValueIfNeeded` walks them.
fn order_deaths(deaths: &mut [Death], receiver: Option<VarId>) {
    deaths.sort_by_key(|death| death.order(receiver));
}
