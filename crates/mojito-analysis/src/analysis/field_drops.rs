//! Per-field last-use destruction for `deinit` parameters: after the
//! variable-granular drop elaboration, each direct field of a `deinit
//! self`/`deinit existing` receiver dies at that field's own last use (an
//! unused one at entry), as Mojo destroys a destructor body's fields. The
//! receiver's `ConsumeVar` teardown stays where the variable pass put it and
//! destroys only the fields that survive to that point.
//!
//! The fields ride the region walks drop elaboration rides: one forward walk
//! ([`walk_forward`]) finds the fields each instruction consumes through its
//! operand registers, one backward walk ([`walk_backward`]) where each field
//! is live, a raise keeping live only what its handler reads, and a splice
//! places each field's drop after its last use, at the entry of an `if` or
//! `comptime if` arm that leaves out a field a sibling arm keeps (upstream's
//! `destroyValuesAtEntryIfNeeded`), and at a handler's entry for what the
//! body's raise points hold that the handler does not read. Upstream tracks
//! these fields in the one field-sensitive walk of `DestructorInsertion`;
//! here they are a walk of their own after drop elaboration.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_mir::mir::verify::{
    instruction_operand_regs, instruction_places, instruction_result_regs,
};
use mojito_mir::mir::{instruction_regions, instruction_regions_mut};
use mojito_types::types::Ty;
use mojito_types::types::TyKind;

/// Splice `DropPlace`s for every refinable field of every `deinit`
/// parameter of struct type with a known declaration.
pub(super) fn refine_deinit_fields(f: &mut MirFunction, declarations: &MirDeclarations) {
    let roots: Vec<VarId> = (0..f.n_params)
        .filter(|&i| f.deinit_params.get(i).copied().unwrap_or(false))
        .map(|i| i as VarId)
        .collect();
    for var in roots {
        let Some(root) = deinit_root(&f.blocks, &f.var_tys, &f.reg_types, var, declarations) else {
            continue;
        };
        let facts = FieldFacts::new(&f.blocks, &root);
        let liveness = FieldLiveness {
            root: &root,
            facts: &facts,
        };
        let mut plan = FieldPlan::default();
        walk_backward(&liveness, &f.blocks, &mut plan);
        // Fields present on entry that no path reads die there.
        let entry_dead: Vec<usize> = f
            .blocks
            .first()
            .and_then(|entry| plan.blocks.get(&std::ptr::from_ref(entry)))
            .map(|entry| root.all.difference(&entry.live_in).copied().collect())
            .unwrap_or_default();
        // Taking the blocks moves no block: the plan's addresses stay valid.
        let mut blocks = std::mem::take(&mut f.blocks);
        let splice = FieldSplice {
            root: &root,
            liveness: &liveness,
            plan: &plan,
        };
        splice.region(&mut blocks);
        if let Some(entry) = blocks.first_mut() {
            prepend_field_drops(&mut entry.instrs, &root, entry_dead);
        }
        f.blocks = blocks;
    }
}

/// Declaration positions of a receiver's refinable fields.
type FieldSet = BTreeSet<usize>;

struct DeinitRoot<'a> {
    var: VarId,
    root_ty: Ty,
    /// Every field in declaration order (name, type).
    fields: Vec<(String, Ty)>,
    /// The positions the pass may destroy individually: droppable, and never
    /// borrowed or moved below depth one anywhere in the body.
    all: FieldSet,
    /// Which operands each instruction receives by value, over the
    /// function's register types.
    passing: ArgumentPassing<'a>,
    register_types: &'a HashMap<u32, Ty>,
}

impl DeinitRoot<'_> {
    fn position(&self, name: &str) -> Option<usize> {
        self.fields
            .iter()
            .position(|(field, _)| field == name)
            .filter(|position| self.all.contains(position))
    }

    fn place(&self, position: usize) -> MirPlace {
        let (name, ty) = &self.fields[position];
        let mut place = MirPlace::root(self.var, Some(self.root_ty.clone()));
        place.project(Proj::Field(name.clone()), ty.clone());
        place
    }
}

fn deinit_root<'a>(
    blocks: &[MirBlock],
    var_tys: &HashMap<VarId, Ty>,
    register_types: &'a HashMap<u32, Ty>,
    var: VarId,
    declarations: &'a MirDeclarations,
) -> Option<DeinitRoot<'a>> {
    let root_ty = var_tys.get(&var)?.clone();
    let TyKind::Struct(name, _) = root_ty.kind() else {
        return None;
    };
    let decl = declarations.structure(name)?;
    let mut pinned: HashSet<String> = HashSet::new();
    for_each_instr_deep(blocks, &mut |instr| {
        let borrowing = matches!(
            instr,
            MirInstr::MakeRef { .. }
                | MirInstr::EstablishLoans { .. }
                | MirInstr::MakeClosure { .. }
        );
        for place in instruction_places(instr) {
            if place.root != var {
                continue;
            }
            let Some(Proj::Field(field)) = place.steps().next() else {
                continue;
            };
            let deep_move = place.steps().nth(1).is_some()
                && matches!(
                    instr,
                    MirInstr::MovePlace { .. } | MirInstr::ConsumePlace { .. }
                );
            if borrowing || deep_move || place.through.is_some() {
                pinned.insert(field.clone());
            }
        }
    });
    let all: FieldSet = decl
        .fields
        .iter()
        .enumerate()
        .filter(|(_, (name, ty))| field_needs_drop(ty) && !pinned.contains(name))
        .map(|(position, _)| position)
        .collect();
    if all.is_empty() {
        return None;
    }
    Some(DeinitRoot {
        var,
        root_ty,
        fields: decl.fields.clone(),
        all,
        passing: ArgumentPassing::new(declarations),
        register_types,
    })
}

/// The receiver fields an instruction reads through its places: a depth-one
/// field projection names that field; a bare receiver touch (a whole-variable
/// use, an unprojected place, a `through` reference) touches every field.
fn direct_field_uses(instr: &MirInstr, root: &DeinitRoot) -> FieldSet {
    let mut set = FieldSet::new();
    let mut whole = false;
    match instr {
        MirInstr::UseVar { var, .. } | MirInstr::KeepAlive { var } if *var == root.var => {
            whole = true;
        }
        MirInstr::HasNext { iter, .. }
        | MirInstr::Next { iter, .. }
        | MirInstr::TryNext { iter, .. }
            if *iter == root.var =>
        {
            whole = true;
        }
        _ => {}
    }
    for place in instruction_places(instr) {
        if place.through == Some(root.var) {
            whole = true;
        }
        if place.root != root.var {
            continue;
        }
        match place.steps().next() {
            Some(Proj::Field(field)) => {
                if let Some(position) = root.position(field) {
                    set.insert(position);
                }
            }
            _ => whole = true,
        }
    }
    if whole {
        set.extend(root.all.iter().copied());
    }
    set
}

/// The receiver fields an instruction transfers out (its new owner drops
/// them, so no `DropPlace` follows).
fn moved_fields(instr: &MirInstr, root: &DeinitRoot) -> FieldSet {
    let mut set = FieldSet::new();
    if let MirInstr::MovePlace { place, .. }
    | MirInstr::ConsumePlace { place, .. }
    | MirInstr::MarkDestroyed { place } = instr
        && place.root == root.var
        && let Some(Proj::Field(field)) = place.steps().next()
        && let Some(position) = root.position(field)
    {
        set.insert(position);
    }
    if let MirInstr::MakeClosure { captures, .. } = instr {
        for capture in captures {
            if capture.mode == MirCaptureMode::Move
                && capture.place.root == root.var
                && let Some(Proj::Field(field)) = capture.place.steps().next()
                && let Some(position) = root.position(field)
            {
                set.insert(position);
            }
        }
    }
    set
}

/// The forward facts of one receiver's fields, from one walk of the
/// structured body entering every region op, keyed by node address: the
/// fields each instruction consumes through its operand registers, and
/// those each yielding block's terminator hands its op.
struct FieldFacts {
    uses: HashMap<*const MirInstr, RegisterFields>,
    yielded: HashMap<*const MirBlock, FieldSet>,
    unused: RegisterFields,
}

/// The receiver fields one instruction consumes through its operand
/// registers: every one, and, in load order, those of the scalars it loads
/// by value.
#[derive(Default)]
struct RegisterFields {
    used: FieldSet,
    loaded: Vec<usize>,
}

impl FieldFacts {
    fn new(blocks: &[MirBlock], root: &DeinitRoot) -> Self {
        let scan = FieldCarryScan { root };
        let mut observer = FieldFactsObserver {
            scan: &scan,
            facts: Self {
                uses: HashMap::new(),
                yielded: HashMap::new(),
                unused: RegisterFields::default(),
            },
        };
        // The observer never refuses an instruction.
        let _ = walk_forward(&scan, Carry::new(), blocks, &mut observer);
        observer.facts
    }

    fn uses(&self, instr: &MirInstr) -> &RegisterFields {
        self.uses
            .get(&std::ptr::from_ref(instr))
            .unwrap_or(&self.unused)
    }

    /// The fields `block`'s `yield` or `comptime.yield` hands its op
    /// through the registers it yields: the op's results carry none, so
    /// the terminator is their last use.
    fn yielded(&self, block: &MirBlock) -> Option<&FieldSet> {
        self.yielded.get(&std::ptr::from_ref(block))
    }
}

/// The receiver fields each register carries: a load or handle of a
/// receiver field carries that field to every consumer (results of
/// non-call instructions inherit it; a call result is a fresh value; a
/// scalar load borrows for one hop only), mirroring the register-loan
/// transfer [`DropFacts`] records. Per register, the carried fields and the
/// single-hop ones.
type Carry = BTreeMap<u32, (FieldSet, FieldSet)>;

/// The forward walk [`FieldFacts`] records.
struct FieldCarryScan<'a> {
    root: &'a DeinitRoot<'a>,
}

impl FieldCarryScan<'_> {
    /// The fields `instr` consumes through its operand registers in `state`.
    fn register_fields(&self, state: &Carry, instr: &MirInstr) -> RegisterFields {
        let mut operands = Vec::new();
        instruction_operand_regs(instr, &mut operands);
        let mut used = FieldSet::new();
        let mut single_hop = Vec::new();
        for operand in operands {
            if let Some((carry, single)) = state.get(&operand.0) {
                used.extend(carry.iter().copied());
                if !single.is_empty() {
                    single_hop.push((operand.0, single.clone()));
                }
            }
        }
        let loaded = split_single_hop(&mut used, single_hop, || {
            self.root.passing.by_value(instr, self.root.register_types)
        });
        used.extend(loaded.iter().copied());
        RegisterFields { used, loaded }
    }
}

impl ForwardScan for FieldCarryScan<'_> {
    type State = Carry;

    fn join(&self, state: &Carry, other: &Carry) -> Carry {
        let mut joined = state.clone();
        for (register, (carry, single)) in other {
            let entry = joined.entry(*register).or_default();
            entry.0.extend(carry.iter().copied());
            entry.1.extend(single.iter().copied());
        }
        joined
    }

    fn transfer(&self, state: &mut Carry, instr: &MirInstr) {
        let root = self.root;
        let mut operands = Vec::new();
        instruction_operand_regs(instr, &mut operands);
        let mut carried: FieldSet = operands
            .iter()
            .filter_map(|operand| state.get(&operand.0))
            .flat_map(|(carry, _)| carry.iter().copied())
            .collect();
        let mut single_hop = FieldSet::new();
        if let MirInstr::MakeRef { place, .. } | MirInstr::LoadPlace { place, .. } = instr
            && place.root == root.var
            && let Some(Proj::Field(field)) = place.steps().next()
            && let Some(position) = root.position(field)
        {
            let scalar = matches!(instr, MirInstr::LoadPlace { .. })
                && place
                    .ty
                    .as_ref()
                    .is_some_and(|ty| !may_alias_owned_storage(ty));
            if scalar {
                single_hop.insert(position);
            } else {
                carried.insert(position);
            }
        }
        let call_result = matches!(
            instr,
            MirInstr::Call { .. }
                | MirInstr::CallIndirect { .. }
                | MirInstr::MethodCall { .. }
                | MirInstr::Index { .. }
                | MirInstr::Slice { .. }
                | MirInstr::MultiIndex { .. }
                | MirInstr::BinOp {
                    resolved: Some(_),
                    ..
                }
        );
        let mut results = Vec::new();
        instruction_result_regs(instr, &mut results);
        for result in results {
            if call_result || (carried.is_empty() && single_hop.is_empty()) {
                state.remove(&result.0);
            } else {
                state.insert(result.0, (carried.clone(), single_hop.clone()));
            }
        }
    }

    fn raise_seed(&self, state: &Carry, _next: Option<&MirInstr>) -> Carry {
        state.clone()
    }

    fn enters(&self, _instr: &MirInstr) -> bool {
        true
    }
}

/// The observer filling [`FieldFacts`].
struct FieldFactsObserver<'a> {
    scan: &'a FieldCarryScan<'a>,
    facts: FieldFacts,
}

impl ForwardObserver<Carry> for FieldFactsObserver<'_> {
    fn instr(&mut self, state: &Carry, instr: &MirInstr) -> Result<(), OwnershipError> {
        let fields = self.scan.register_fields(state, instr);
        if !fields.used.is_empty() {
            self.facts.uses.insert(std::ptr::from_ref(instr), fields);
        }
        Ok(())
    }

    fn block(&mut self, _entry: &Carry, end: Option<&Carry>, block: &MirBlock) {
        let (Some(state), MirTerm::Yield(values) | MirTerm::ComptimeYield(values)) =
            (end, &block.term)
        else {
            return;
        };
        let fields: FieldSet = values
            .iter()
            .filter_map(|value| state.get(&value.0))
            .flat_map(|(carry, single)| carry.iter().chain(single).copied())
            .collect();
        if !fields.is_empty() {
            self.facts.yielded.insert(std::ptr::from_ref(block), fields);
        }
    }
}

/// Backward field liveness: a field is live from a use, through its places
/// or through the registers an instruction consumes, back to the entry. A
/// `yield` reads what its values carry, and a `return` whose cleanup
/// consumes the receiver every field.
struct FieldLiveness<'a> {
    root: &'a DeinitRoot<'a>,
    facts: &'a FieldFacts,
}

impl FieldLiveness<'_> {
    /// The fields `block`'s terminator destroys with the receiver: every
    /// one at an exit whose cleanup consumes it.
    fn cleaned(&self, block: &MirBlock) -> Option<&FieldSet> {
        match &block.term {
            MirTerm::ReturnWithCleanup { cleanup, .. } | MirTerm::EscapeJump { cleanup, .. }
                if cleanup_drops(cleanup, self.root.var) =>
            {
                Some(&self.root.all)
            }
            _ => None,
        }
    }
}

impl BackwardScan for FieldLiveness<'_> {
    type State = FieldSet;

    fn join(&self, state: &mut FieldSet, other: &FieldSet) {
        state.extend(other.iter().copied());
    }

    fn transfer(&self, live: &mut FieldSet, instr: &MirInstr) {
        live.extend(direct_field_uses(instr, self.root));
        live.extend(self.facts.uses(instr).used.iter().copied());
    }

    fn enters(&self, _instr: &MirInstr) -> bool {
        true
    }

    fn define(&self, _live: &mut FieldSet, _var: VarId) {}

    fn terminate(&self, live: &mut FieldSet, block: &MirBlock) {
        let read = self
            .facts
            .yielded(block)
            .into_iter()
            .chain(self.cleaned(block));
        live.extend(read.flatten().copied());
    }
}

/// What the backward walk reports for the splice, keyed by node address
/// (stable while the body is borrowed, and through the splice: no block is
/// added or moved, and an instruction is looked up before its block is
/// rebuilt).
#[derive(Default)]
struct FieldPlan {
    /// The fields live just before each instruction, region ops included.
    instrs: HashMap<*const MirInstr, FieldSet>,
    blocks: HashMap<*const MirBlock, FieldBlock>,
    /// The handler live-in of each `try` with a handler.
    handlers: HashMap<*const MirInstr, FieldSet>,
}

/// The field liveness around one block.
struct FieldBlock {
    live_in: FieldSet,
    live_out: FieldSet,
    /// What a raise in the block keeps live; `None` outside every `try`.
    raise: Option<FieldSet>,
}

impl BackwardRecorder<FieldSet> for FieldPlan {
    fn instr(&mut self, instr: &MirInstr, live: &FieldSet) {
        self.instrs.insert(std::ptr::from_ref(instr), live.clone());
    }

    fn block(&mut self, block: &MirBlock, live: BlockLiveness<'_, FieldSet>) {
        self.blocks.insert(
            std::ptr::from_ref(block),
            FieldBlock {
                live_in: live.live_in.clone(),
                live_out: live.live_out.clone(),
                raise: live.raise.cloned(),
            },
        );
    }

    fn try_op(&mut self, op: &MirInstr, channels: &TryChannels<FieldSet>) {
        if let Some(handler) = &channels.handler {
            self.handlers
                .insert(std::ptr::from_ref(op), handler.clone());
        }
    }
}

/// The splice pass: the plan's field drops placed in the body, nested
/// regions before the blocks holding them.
struct FieldSplice<'a> {
    root: &'a DeinitRoot<'a>,
    liveness: &'a FieldLiveness<'a>,
    plan: &'a FieldPlan,
}

impl FieldSplice<'_> {
    /// The recorded live-in of a region; nothing for an unreached one.
    fn region_live_in(&self, region: &[MirBlock]) -> FieldSet {
        region
            .first()
            .and_then(|block| self.plan.blocks.get(&std::ptr::from_ref(block)))
            .map(|block| block.live_in.clone())
            .unwrap_or_default()
    }

    fn region(&self, blocks: &mut [MirBlock]) {
        for block in blocks {
            self.block(block);
        }
    }

    /// Place the field drops of one block: its region ops' first, then its
    /// own, each field after the instruction that is its last use; a
    /// scalar's field at its by-value load; a field the terminator's
    /// yielded values last read before the terminator.
    fn block(&self, block: &mut MirBlock) {
        let Some(plan) = self.plan.blocks.get(&std::ptr::from_ref(&*block)) else {
            return;
        };
        let live_before: Vec<FieldSet> = block
            .instrs
            .iter()
            .map(|instr| {
                self.plan
                    .instrs
                    .get(&std::ptr::from_ref(instr))
                    .cloned()
                    .unwrap_or_default()
            })
            .collect();
        let steps: Vec<Option<(FieldSet, Vec<usize>)>> = block
            .instrs
            .iter()
            .map(|instr| {
                (!region_op(instr)).then(|| {
                    let uses = self.liveness.facts.uses(instr);
                    (direct_field_uses(instr, self.root), uses.loaded.clone())
                })
            })
            .collect();
        for instr in &mut block.instrs {
            if region_op(instr) {
                self.op(instr);
            }
        }
        let mut end = plan.live_out.clone();
        end.extend(self.liveness.cleaned(block).into_iter().flatten().copied());
        let mut live_after_last = end.clone();
        self.liveness.terminate(&mut live_after_last, block);

        let instrs = std::mem::take(&mut block.instrs);
        let n = instrs.len();
        let mut rebuilt = Vec::with_capacity(n);
        let mut instrs = instrs.into_iter().enumerate().peekable();
        while let Some((i, instr)) = instrs.next() {
            let after = live_before.get(i + 1).unwrap_or(&live_after_last);
            let Some((direct, loaded)) = &steps[i] else {
                // A region op is one step: what dies across it died inside it.
                rebuilt.push(instr);
                continue;
            };
            let moved = moved_fields(&instr, self.root);
            let mut dying: Vec<usize> = live_before[i]
                .difference(after)
                .copied()
                .filter(|position| !moved.contains(position))
                .collect();
            // A field whose last use is a scalar the instruction loads by
            // value dies at that load, before the instruction runs, unless
            // the raise leaving it still observes the field.
            let raise = plan.raise.as_ref().filter(|_| raises_to_handler(&instr));
            let before: Vec<usize> = loaded
                .iter()
                .copied()
                .filter(|position| {
                    dying.contains(position)
                        && !direct.contains(position)
                        && !raise.is_some_and(|raise| raise.contains(position))
                })
                .collect();
            dying.retain(|position| !before.contains(position));
            let at = rebuilt.len();
            insert_field_drops(&mut rebuilt, at, self.root, before);
            rebuilt.push(instr);
            // Keep the variable pass's trailing drop group together; a
            // `ConsumeVar` of the receiver in it destroys these fields at
            // this point, each by its own named drop just before it.
            let group = rebuilt.len();
            while let Some((_, drop)) = instrs.next_if(|(_, next)| is_drop_instr(next)) {
                rebuilt.push(drop);
            }
            let consume = rebuilt[group..]
                .iter()
                .position(
                    |instr| matches!(instr, MirInstr::ConsumeVar { var } if *var == self.root.var),
                )
                .map_or(rebuilt.len(), |offset| group + offset);
            insert_field_drops(&mut rebuilt, consume, self.root, dying);
        }
        // A field the terminator's yielded values carry dies before it.
        let yielded: Vec<usize> = live_after_last.difference(&end).copied().collect();
        let at = rebuilt.len();
        insert_field_drops(&mut rebuilt, at, self.root, yielded);
        block.instrs = rebuilt;
    }

    /// Place the field drops of one region op's regions. An `if` or
    /// `comptime if` destroys at each arm's entry what a sibling arm keeps
    /// and it does not, as `destroyValuesAtEntryIfNeeded` does; a `try`'s
    /// handler destroys on entry what the body's raise points hold that it
    /// does not read.
    fn op(&self, instr: &mut MirInstr) {
        let entries = {
            let regions = instruction_regions(instr);
            match &*instr {
                MirInstr::If { .. } => self.if_entry_drops(&regions),
                MirInstr::ComptimeIf { .. } => self.comptime_if_entry_drops(&regions),
                MirInstr::Try { body, .. } => self.handler_entry_drops(instr, body),
                _ => Vec::new(),
            }
        };
        for region in instruction_regions_mut(instr) {
            self.region(region);
        }
        if entries.is_empty() {
            return;
        }
        let regions = instruction_regions_mut(instr);
        for (region, drops) in regions.into_iter().zip(entries) {
            if let Some(entry) = region.first_mut() {
                prepend_field_drops(&mut entry.instrs, self.root, drops);
            }
        }
    }

    /// The entry drops of an `if`'s regions, listed as `then`, each elif's
    /// condition and arm, and `else`: as `checkIfOp` unifies them, each
    /// step's arm against the next condition (or the `else`).
    fn if_entry_drops(&self, regions: &[&Vec<MirBlock>]) -> Vec<Vec<usize>> {
        let mut drops = vec![Vec::new(); regions.len()];
        // Step `k` takes the branch to arm `regions[2k]` or on to
        // `regions[2k + 1]`.
        for arm in (0..regions.len().saturating_sub(1)).step_by(2) {
            let taken = self.region_live_in(regions[arm]);
            let next = self.region_live_in(regions[arm + 1]);
            drops[arm] = next.difference(&taken).copied().collect();
            drops[arm + 1] = taken.difference(&next).copied().collect();
        }
        drops
    }

    /// The entry drops of a `comptime if`'s arms and `else`: each arm is
    /// unified against the arms after it and the `else` in turn, and what
    /// a step destroys on the way on runs before each later arm's own.
    fn comptime_if_entry_drops(&self, regions: &[&Vec<MirBlock>]) -> Vec<Vec<usize>> {
        let mut drops: Vec<Vec<usize>> = vec![Vec::new(); regions.len()];
        for arm in 0..regions.len().saturating_sub(1) {
            let taken = self.region_live_in(regions[arm]);
            let rest: FieldSet = regions[arm + 1..]
                .iter()
                .flat_map(|region| self.region_live_in(region))
                .collect();
            drops[arm].extend(rest.difference(&taken).copied());
            for later in &mut drops[arm + 1..] {
                later.extend(taken.difference(&rest).copied());
            }
        }
        drops
    }

    /// The entry drops of a `try`'s regions: its handler's are the fields
    /// some raise point of the body holds, at any depth, a nested `try`
    /// counted by its live-in, that the handler does not read.
    fn handler_entry_drops(&self, op: &MirInstr, body: &[MirBlock]) -> Vec<Vec<usize>> {
        let Some(handler) = self.plan.handlers.get(&std::ptr::from_ref(op)) else {
            return Vec::new();
        };
        let mut present = FieldSet::new();
        self.raise_points(body, &mut present);
        vec![Vec::new(), present.difference(handler).copied().collect()]
    }

    /// Collect into `present` the fields live at each raise point of
    /// `blocks`, entering every region op but a `try`.
    fn raise_points(&self, blocks: &[MirBlock], present: &mut FieldSet) {
        for instr in blocks.iter().flat_map(|block| &block.instrs) {
            if region_op(instr) && !matches!(instr, MirInstr::Try { .. }) {
                for region in instruction_regions(instr) {
                    self.raise_points(region, present);
                }
            } else if raises_to_handler(instr)
                && let Some(live) = self.plan.instrs.get(&std::ptr::from_ref(instr))
            {
                present.extend(live.iter().copied());
            }
        }
    }
}

const fn is_drop_instr(instr: &MirInstr) -> bool {
    matches!(
        instr,
        MirInstr::DropVar { .. } | MirInstr::ConsumeVar { .. } | MirInstr::DropPlace { .. }
    )
}

/// Prepend `DropPlace`s (declaration order) after any leading drop group,
/// or just before the `ConsumeVar` of the receiver in that group, which
/// destroys the fields at that point: each then dies by its own named drop.
fn prepend_field_drops(instrs: &mut Vec<MirInstr>, root: &DeinitRoot, positions: Vec<usize>) {
    let lead = instrs
        .iter()
        .take_while(|instr| is_drop_instr(instr))
        .count();
    let at = instrs[..lead]
        .iter()
        .position(|instr| matches!(instr, MirInstr::ConsumeVar { var } if *var == root.var))
        .unwrap_or(lead);
    insert_field_drops(instrs, at, root, positions);
}

/// Insert `DropPlace`s in declaration order at `at`.
fn insert_field_drops(
    instrs: &mut Vec<MirInstr>,
    at: usize,
    root: &DeinitRoot,
    mut positions: Vec<usize>,
) {
    positions.sort_unstable();
    positions.dedup();
    for (offset, position) in positions.into_iter().enumerate() {
        instrs.insert(
            at + offset,
            MirInstr::DropPlace {
                place: root.place(position),
                destructor: None,
            },
        );
    }
}
