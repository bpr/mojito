//! Register-loan dataflow: which registers carry loans of owned
//! storage across instructions.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_mir::mir::TraitRequirement;
use mojito_symbol::symbol::is_trait_dispatch_symbol;
use mojito_types::types::{Ty, TyKind};

#[derive(Clone, Default, PartialEq, Eq)]
pub(super) struct RegisterLoanState {
    owners: BTreeMap<u32, BTreeSet<VarId>>,
    /// Roots a scalar `LoadPlace` result keeps alive through its consumers
    /// only: reported as uses at every instruction reading the register, but
    /// never inherited by that instruction's own results (`t.n + 1` borrows
    /// `t` for `__add__` alone; the sum owns its bits).
    single_hop: BTreeMap<u32, BTreeSet<VarId>>,
}

pub(super) fn join_register_loan_states(
    mut left: RegisterLoanState,
    right: &RegisterLoanState,
) -> RegisterLoanState {
    for (register, owners) in &right.owners {
        left.owners.entry(*register).or_default().extend(owners);
    }
    for (register, owners) in &right.single_hop {
        left.single_hop.entry(*register).or_default().extend(owners);
    }
    left
}

pub(super) fn active_register_loan_roots(
    references: impl IntoIterator<Item = VarId>,
    state: &LoanGenerationState,
    generations: &BTreeMap<u32, DropLoanGeneration>,
) -> BTreeSet<VarId> {
    let mut roots = BTreeSet::new();
    let mut pending: Vec<VarId> = references.into_iter().collect();
    let mut visited = BTreeSet::new();
    while let Some(reference) = pending.pop() {
        if !visited.insert(reference) {
            continue;
        }
        let Some(active) = state.active.get(&reference) else {
            continue;
        };
        for generation in active {
            let Some(generation) = generations.get(generation) else {
                continue;
            };
            if !generation.propagate_through_registers {
                continue;
            }
            for root in &generation.roots {
                if roots.insert(*root) {
                    pending.push(*root);
                }
            }
        }
    }
    roots
}

/// Whether a shallowly read value of this type can alias heap storage owned by
/// its source place — an aggregate whose fields may hold owning pointers (the
/// self-hosted `String`/`List` shape), or any binder, a requirement's
/// abstract `Self` among them. Scalars and compile-time values own their
/// bits; a bare `Pointer` read aliases by design (the `unsafe_*`
/// vocabulary makes its lifetime the user's obligation).
pub(super) fn may_alias_owned_storage(ty: &Ty) -> bool {
    matches!(
        ty.kind(),
        TyKind::Struct(..)
            | TyKind::Tuple(_)
            | TyKind::RuntimePack(_)
            | TyKind::Variant(_)
            | TyKind::ComptimeList(_)
            | TyKind::Param { .. }
            | TyKind::Assoc { .. }
            | TyKind::SelfType
    )
}

/// The roots one instruction consumes through its operand registers.
pub(super) struct RegisterUses {
    /// Roots the instruction keeps alive through itself: owner provenance,
    /// and the owners of scalars it receives by address.
    pub(super) retained: Vec<VarId>,
    /// The owners of scalars the instruction receives by value, in the order
    /// it loads them (parameter-slot order): each read is a load at call
    /// setup, so an owner whose last use it is dies before the instruction.
    pub(super) loaded: Vec<VarId>,
}

/// Which operand registers an instruction receives by value, as the callee's
/// declared contract says: a concrete register-passable `read` or `var`
/// parameter is loaded at call setup, while a memory-only one — a generic
/// `T`, a variadic collector, a callee with no declaration (`print`) — takes
/// the argument's address for the whole call.
pub(super) struct ArgumentPassing<'a> {
    declarations: &'a MirDeclarations,
}

impl<'a> ArgumentPassing<'a> {
    pub(super) const fn new(declarations: &'a MirDeclarations) -> Self {
        Self { declarations }
    }

    /// The operand registers `instruction` loads by value, each with its
    /// load rank.
    pub(super) fn by_value(
        &self,
        instruction: &MirInstr,
        register_types: &HashMap<u32, mojito_types::types::Ty>,
    ) -> Vec<(u32, usize)> {
        match instruction {
            MirInstr::Call {
                func,
                args,
                kwargs,
                arg_places,
                kwarg_places,
                spread,
                ..
            } => self
                .callee(&func.0)
                .map(|declaration| {
                    let mut loaded = Vec::new();
                    let arguments = CallArguments {
                        args,
                        kwargs,
                        arg_places,
                        kwarg_places,
                        spread: *spread,
                    };
                    arguments.by_value(&declaration.into(), 0, &mut loaded);
                    loaded
                })
                .unwrap_or_default(),
            MirInstr::MethodCall {
                recv,
                resolved,
                recv_place,
                args,
                kwargs,
                arg_places,
                kwarg_places,
                spread,
                ..
            } => resolved
                .as_deref()
                .and_then(|name| {
                    self.resolve(
                        name,
                        receiver_ty(recv_place.as_deref(), *recv, register_types),
                    )
                })
                .map(|callee| {
                    let mut loaded = Vec::new();
                    if recv_place.is_none() {
                        callee.receiver_by_value(*recv, register_types, &mut loaded);
                    }
                    let arguments = CallArguments {
                        args,
                        kwargs,
                        arg_places,
                        kwarg_places,
                        spread: *spread,
                    };
                    arguments.by_value(&callee.contract(), 1, &mut loaded);
                    loaded
                })
                .unwrap_or_default(),
            MirInstr::BinOp {
                a,
                b,
                resolved: Some(name),
                ..
            } => self
                .resolve(name, receiver_ty(None, *a, register_types))
                .map(|callee| {
                    let mut loaded = Vec::new();
                    callee.receiver_by_value(*a, register_types, &mut loaded);
                    let arguments = CallArguments {
                        args: std::slice::from_ref(b),
                        ..CallArguments::default()
                    };
                    arguments.by_value(&callee.contract(), 1, &mut loaded);
                    loaded
                })
                .unwrap_or_default(),
            MirInstr::CallIndirect {
                callee,
                resolved,
                args,
                kwargs,
                arg_places,
                kwarg_places,
                instantiated_contract,
                spread,
                ..
            } => {
                let arguments = CallArguments {
                    args,
                    kwargs,
                    arg_places,
                    kwarg_places,
                    spread: *spread,
                };
                let mut loaded = Vec::new();
                let contract = instantiated_contract
                    .as_ref()
                    .or_else(|| register_types.get(&callee.0));
                if let Some(mojito_types::types::TyKind::Func(function)) =
                    contract.map(mojito_types::types::Ty::kind)
                {
                    arguments.by_value(&function.as_ref().into(), 0, &mut loaded);
                } else if let Some(declaration) = resolved
                    .as_deref()
                    .and_then(|name| self.declarations.function(name))
                {
                    arguments.by_value(&declaration.into(), 1, &mut loaded);
                }
                loaded
            }
            MirInstr::Index {
                base,
                index,
                base_place,
                index_place,
                call: Some(call),
                ..
            } => self
                .resolve(
                    &call.target,
                    receiver_ty(base_place.as_deref(), *base, register_types),
                )
                .map(|callee| {
                    let mut loaded = Vec::new();
                    if base_place.is_none() {
                        callee.receiver_by_value(*base, register_types, &mut loaded);
                    }
                    if index_place.is_none() && callee.contract().loads_positional(0) {
                        loaded.push((index.0, 1));
                    }
                    loaded
                })
                .unwrap_or_default(),
            MirInstr::Slice {
                object,
                lower,
                upper,
                step,
                object_place,
                call: Some(call),
                ..
            } => self
                .resolve(
                    &call.target,
                    receiver_ty(object_place.as_deref(), *object, register_types),
                )
                .map(|callee| {
                    let mut loaded = Vec::new();
                    slice_bounds_loaded(*lower, *upper, *step, 0, &mut loaded);
                    if object_place.is_none() {
                        callee.receiver_by_value(*object, register_types, &mut loaded);
                    }
                    loaded
                })
                .unwrap_or_default(),
            MirInstr::MultiIndex {
                object,
                args,
                object_place,
                arg_places,
                call: Some(call),
                ..
            } => self
                .resolve(
                    &call.target,
                    receiver_ty(object_place.as_deref(), *object, register_types),
                )
                .map(|callee| {
                    let mut loaded = Vec::new();
                    subscript_slices_loaded(args, &mut loaded);
                    if object_place.is_none() {
                        callee.receiver_by_value(*object, register_types, &mut loaded);
                    }
                    subscript_indices_loaded(&callee.contract(), args, arg_places, &mut loaded);
                    loaded
                })
                .unwrap_or_default(),
            // A setter loads its plain indices like a getter; the receiver
            // is its `mut self` place, and the assigned value is evaluated
            // after every index.
            MirInstr::MultiSet {
                receiver,
                receiver_place,
                args,
                arg_places,
                call,
                ..
            } => self
                .resolve(
                    &call.target,
                    receiver_ty(receiver_place.as_deref(), *receiver, register_types),
                )
                .map(|callee| {
                    let mut loaded = Vec::new();
                    subscript_slices_loaded(args, &mut loaded);
                    subscript_indices_loaded(&callee.contract(), args, arg_places, &mut loaded);
                    loaded
                })
                .unwrap_or_default(),
            // The rest run callee code or consume a whole value: none loads a
            // scalar operand.
            MirInstr::Try { .. }
            | MirInstr::Raise { .. }
            | MirInstr::GetIter { .. }
            | MirInstr::HasNext { .. }
            | MirInstr::Next { .. }
            | MirInstr::TryNext { .. } => Vec::new(),
            _ => {
                let mut operands = Vec::new();
                mojito_mir::mir::verify::instruction_operand_regs(instruction, &mut operands);
                operands
                    .into_iter()
                    .enumerate()
                    .map(|(rank, operand)| (operand.0, rank))
                    .collect()
            }
        }
    }

    /// The declaration a direct call of `callee` binds: a struct name calls
    /// its `__init__`.
    fn callee(&self, callee: &str) -> Option<&'a MirFunctionDeclaration> {
        if self.declarations.structure(callee).is_some() {
            self.declarations.function(&format!("{callee}.__init__"))
        } else {
            self.declarations.function(callee)
        }
    }

    /// The callee a method or operator call of `target` binds over a
    /// receiver of type `receiver`: its declaration, or for a call through
    /// a bound, the requirement the bound trait or a parent declares.
    fn resolve(&self, target: &str, receiver: Option<&Ty>) -> Option<Callee<'a>> {
        self.declarations
            .function(target)
            .map(Callee::Declared)
            .or_else(|| {
                let Some(TyKind::Param { bounds, .. }) = receiver.map(Ty::kind) else {
                    return None;
                };
                is_trait_dispatch_symbol(target)
                    .then(|| self.declarations.bound_requirement(bounds, target))
                    .flatten()
                    .map(Callee::Required)
            })
    }
}

/// Split per-operand single-hop provenance by how the instruction receives
/// each operand: an addressed operand's items join `retained`, and a by-value
/// one's are returned in load-rank order, less any also retained.
pub(super) fn split_single_hop<T: Ord + Copy>(
    retained: &mut BTreeSet<T>,
    single_hop: Vec<(u32, BTreeSet<T>)>,
    by_value: impl FnOnce() -> Vec<(u32, usize)>,
) -> Vec<T> {
    if single_hop.is_empty() {
        return Vec::new();
    }
    let by_value = by_value();
    let mut ranked = Vec::new();
    for (register, items) in single_hop {
        match by_value.iter().find(|(operand, _)| *operand == register) {
            Some((_, rank)) => ranked.extend(items.into_iter().map(|item| (*rank, item))),
            None => retained.extend(items),
        }
    }
    ranked.sort_by_key(|(rank, _)| *rank);
    let mut loaded = Vec::new();
    for (_, item) in ranked {
        if !retained.contains(&item) && !loaded.contains(&item) {
            loaded.push(item);
        }
    }
    loaded
}

/// One function body's (or `try` region's) register-loan facts: the roots
/// each instruction consumes, and the register-loan state entering each
/// `try`, which seeds its body region.
pub(super) struct RegisterLoans {
    /// Per block, per instruction, the roots the instruction consumes.
    pub(super) uses: Vec<Vec<RegisterUses>>,
    /// The state entering each `try`, keyed by block and instruction index.
    pub(super) before_try: HashMap<(usize, usize), RegisterLoanState>,
}

/// Per-instruction transient owner uses over an arbitrary block vector (a
/// function body or a `try` region mini-CFG) with an explicit entry state.
///
/// A forward worklist dataflow in reverse postorder. Each state holds only
/// the registers some later instruction still reads (deep through `try`
/// regions), so joins and comparisons stay proportional to the registers
/// live across an edge rather than to every register the body defines.
pub(super) fn register_loan_uses_over(
    blocks: &[MirBlock],
    generation_entries: &[LoanGenerationState],
    entry_registers: &RegisterLoanState,
    loan_roots: &BTreeMap<u32, DropLoanGeneration>,
    generation_dests: &BTreeMap<u32, Option<MirInteriorOrigin>>,
    reg_types: &HashMap<u32, mojito_types::types::Ty>,
    passing: &ArgumentPassing,
) -> RegisterLoans {
    let nb = blocks.len();
    let steps: Vec<Vec<RegisterLoanStep>> = blocks
        .iter()
        .zip(generation_entries)
        .map(|(block, entry)| {
            let mut generations = entry.clone();
            block
                .instrs
                .iter()
                .map(|instruction| {
                    let step =
                        RegisterLoanStep::new(&generations, instruction, loan_roots, reg_types);
                    transfer_loan_generation(&mut generations, instruction, generation_dests);
                    step
                })
                .collect()
        })
        .collect();
    let predecessors = block_predecessors(blocks);
    let liveness = RegisterLiveness::new(blocks, &steps, &predecessors);

    let mut incoming: Vec<Option<RegisterLoanState>> = vec![None; nb];
    let mut outgoing: Vec<Option<RegisterLoanState>> = vec![None; nb];
    let mut worklist = Worklist::forward(blocks);
    while let Some(block) = worklist.pop() {
        let new_in = if block == 0 || predecessors[block].is_empty() {
            entry_registers.clone()
        } else {
            let mut states = predecessors[block]
                .iter()
                .filter_map(|predecessor| outgoing[*predecessor].as_ref());
            let Some(first) = states.next() else {
                continue;
            };
            states.fold(first.clone(), join_register_loan_states)
        };
        let new_in = liveness.entering(block, new_in);
        if incoming[block].as_ref() == Some(&new_in) {
            continue;
        }
        let mut new_out = new_in.clone();
        for (index, step) in steps[block].iter().enumerate() {
            step.transfer(&mut new_out, None);
            liveness.leaving(block, index, &mut new_out);
        }
        incoming[block] = Some(new_in);
        if outgoing[block].as_ref() != Some(&new_out) {
            outgoing[block] = Some(new_out);
            worklist.push_successors(&blocks[block].term);
        }
    }

    let mut before_try = HashMap::new();
    let uses = blocks
        .iter()
        .zip(incoming)
        .enumerate()
        .map(|(block, (body, entry))| {
            let mut registers = entry.unwrap_or_default();
            body.instrs
                .iter()
                .zip(&steps[block])
                .enumerate()
                .map(|(index, (instruction, step))| {
                    if matches!(instruction, MirInstr::Try { .. }) {
                        before_try.insert((block, index), registers.clone());
                    }
                    let mut single_hop = Vec::new();
                    let retained = step.transfer(&mut registers, Some(&mut single_hop));
                    let uses = split_register_uses(retained, single_hop, || {
                        passing.by_value(instruction, reg_types)
                    });
                    liveness.leaving(block, index, &mut registers);
                    uses
                })
                .collect()
        })
        .collect();
    RegisterLoans { uses, before_try }
}

/// Whether a call result register holds a pointer whose provenance designates
/// checked storage (a loan-tracked origin), as opposed to an untracked or
/// unsafe-any pointer.
#[allow(
    clippy::trivially_copy_pass_by_ref,
    reason = "TODO: take by value and update the call sites"
)]
fn tracked_pointer_result(
    register_types: &HashMap<u32, mojito_types::types::Ty>,
    dest: &Reg,
) -> bool {
    matches!(
        register_types.get(&dest.0).map(mojito_types::types::Ty::kind),
        Some(mojito_types::types::TyKind::Pointer { origin, .. }) if origin.as_origin().is_some()
    )
}

/// One SSA instruction's transfer of transient owner provenance, whose
/// register-state-independent half is computed once per instruction rather
/// than once per dataflow visit. Its transfer returns the roots the
/// instruction consumes, which become ordinary drop liveness uses at this
/// exact program point. Results inherit operand provenance until a terminal
/// instruction such as `DefVar` consumes them, allowing `MakeRef -> ReadRef
/// -> Call` to retain an owner through the outer call without extending the
/// source reference's generation to function exit.
struct RegisterLoanStep {
    operands: Vec<Reg>,
    results: Vec<Reg>,
    /// The roots the instruction uses whatever its operands carry: the active
    /// loan roots of the variables it reads, and the roots its place reads
    /// retain or its callee-produced handle seeds.
    owners: BTreeSet<VarId>,
    /// The roots a callee-produced handle result carries.
    seeded: BTreeSet<VarId>,
    /// Whether the results are fresh values that inherit only `seeded`.
    call_result: bool,
    /// A scalar read's result register and the roots it borrows for one hop.
    single_hop: Option<(u32, BTreeSet<VarId>)>,
    /// The slots the instruction ends by moving them. An ended slot's storage
    /// backs no register: a by-value scalar already holds its bits, and a
    /// handle into the slot is dead (use after move is `moves`' error).
    ended: Vec<VarId>,
}

impl RegisterLoanStep {
    fn new(
        generations: &LoanGenerationState,
        instruction: &MirInstr,
        loan_roots: &BTreeMap<u32, DropLoanGeneration>,
        register_types: &HashMap<u32, mojito_types::types::Ty>,
    ) -> Self {
        let mut operands = Vec::new();
        mojito_mir::mir::verify::instruction_operand_regs(instruction, &mut operands);
        let mut owners = active_register_loan_roots(
            var_uses(instruction).into_iter().map(|(var, _)| var),
            generations,
            loan_roots,
        );
        let mut single_hop = None;

        // `MakeRef` creates a transient handle directly into its place root.  That
        // root is not necessarily an established reference generation itself (for
        // example `box.value` where `box` merely *contains* a stored reference), so
        // the generation closure above cannot discover it.  Retain the storage root
        // through every SSA consumer of the handle; `through` additionally keeps a
        // substituted reference binding alive when the place was reached via one.
        //
        // `LoadPlace` needs the same retention for a different reason: it reads the
        // place shallowly, so a pointer-owning (lifecycle) result register aliases
        // the root's storage until a `CopyValue` runs the copy lifecycle or a
        // consuming call finishes. Dropping the root between the load and that
        // consumer would free storage the pending register still references, so
        // an aggregate (or unknown-typed) result retains through every consumer.
        // A scalar read owns its bits, but the read is still a borrow of its
        // owner that lasts through the instruction consuming the register (Mojo
        // passes `t.n` to `print` as a reference to `t`, which therefore outlives
        // the call): the root is retained for that single hop only.
        let retained_place = match instruction {
            MirInstr::MakeRef { place, .. } => Some(place),
            MirInstr::LoadPlace { dest, place } => {
                if place
                    .ty
                    .as_ref()
                    .or_else(|| register_types.get(&dest.0))
                    .is_none_or(may_alias_owned_storage)
                {
                    Some(place)
                } else {
                    let roots = std::iter::once(place.root).chain(place.through);
                    single_hop = Some((dest.0, roots.collect()));
                    None
                }
            }
            _ => None,
        };
        if let Some(place) = retained_place {
            owners.insert(place.root);
            owners.extend(place.through);
        }

        // A nominal subscript returning `ref T` establishes a transient handle to
        // its retained receiver place. Unlike `MakeRef`, the handle is produced by
        // the callee, so no explicit loan generation exists at this instruction.
        // Seed the destination register with the receiver root directly; subsequent
        // `ReadRef`/call uses then keep that caller slot alive until the handle has
        // actually been consumed.
        let reference_subscript_place = match instruction {
            MirInstr::Index {
                dest,
                base_place: Some(place),
                ..
            }
            | MirInstr::Slice {
                dest,
                object_place: Some(place),
                ..
            }
            | MirInstr::MultiIndex {
                dest,
                object_place: Some(place),
                ..
            } if matches!(
                register_types
                    .get(&dest.0)
                    .map(mojito_types::types::Ty::kind),
                Some(mojito_types::types::TyKind::Ref(_))
            ) =>
            {
                Some(place)
            }
            // A direct method call returning `ref T` (no adapter) is the same
            // callee-produced transient handle as a reference subscript: the
            // result register borrows the receiver's storage, which must outlive
            // every consumer of the handle — not just the call instruction.
            MirInstr::MethodCall {
                reference_result: Some(_),
                result_adapter: None,
                recv_place: Some(place),
                ..
            } => Some(place),
            // A method call returning a tracked pointer (`xs.unsafe_ptr()`, a
            // `pointer_self`/`pointer_place` provenance) hands out a handle into
            // the receiver's storage the same way: the receiver must outlive
            // every consumer of the pointer register — a callee reading the
            // pointee included. Untracked and unsafe-any results carry no loan.
            MirInstr::MethodCall {
                dest,
                recv_place: Some(place),
                ..
            } if tracked_pointer_result(register_types, dest) => Some(place),
            _ => None,
        };
        let mut seeded = BTreeSet::new();
        if let Some(place) = reference_subscript_place {
            seeded.insert(place.root);
            seeded.extend(place.through);
        }
        // A free call returning a tracked pointer borrows through its retained
        // argument places.
        if let MirInstr::Call {
            dest, arg_places, ..
        } = instruction
            && tracked_pointer_result(register_types, dest)
        {
            for place in arg_places.iter().flatten() {
                seeded.insert(place.root);
                seeded.extend(place.through);
            }
        }
        // An element read out of pack storage is the same shallow read as a
        // `LoadPlace`: an aggregate element aliases the pack until its consumer
        // finishes, and a scalar one borrows the pack for that consumer alone.
        if let MirInstr::Index {
            dest,
            base_place: Some(place),
            intrinsic:
                Some(MirIntrinsicSubscript::TupleStorage | MirIntrinsicSubscript::VariadicStorage),
            ..
        } = instruction
        {
            let roots = std::iter::once(place.root).chain(place.through);
            if register_types
                .get(&dest.0)
                .is_none_or(may_alias_owned_storage)
            {
                seeded.extend(roots);
            } else {
                single_hop = Some((dest.0, roots.collect()));
            }
        }
        owners.extend(seeded.iter().copied());

        // A call-family result is a fresh independent value — a return runs the
        // copy/move lifecycle out of the callee frame — so operand provenance is
        // consumed at the call (the roots are uses at this exact point) and does
        // not flow into the result. Only the explicit reference-result seeding
        // above (a callee-produced handle into caller storage) carries through.
        let call_result = matches!(
            instruction,
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
        mojito_mir::mir::verify::instruction_result_regs(instruction, &mut results);
        Self {
            ended: super::scan::vars_moved(instruction),
            operands,
            results,
            owners,
            seeded,
            call_result,
            single_hop,
        }
    }

    /// Advance `registers` past the instruction. With `single_hop_uses`, also
    /// return the roots it retains and record, per operand register, the
    /// roots a scalar read borrows for this one hop; otherwise the returned
    /// set is meaningless.
    fn transfer(
        &self,
        registers: &mut RegisterLoanState,
        mut single_hop_uses: Option<&mut Vec<(u32, BTreeSet<VarId>)>>,
    ) -> BTreeSet<VarId> {
        let inherits = !self.call_result && !self.results.is_empty();
        let mut owners = BTreeSet::new();
        if single_hop_uses.is_some() || inherits {
            owners.clone_from(&self.owners);
            for operand in &self.operands {
                if let Some(provenance) = registers.owners.get(&operand.0) {
                    owners.extend(provenance);
                }
                if let Some(uses) = single_hop_uses.as_deref_mut()
                    && let Some(provenance) = registers.single_hop.get(&operand.0)
                {
                    uses.push((operand.0, provenance.clone()));
                }
            }
        }
        if let Some((dest, roots)) = &self.single_hop {
            registers.single_hop.insert(*dest, roots.clone());
        }
        let result_owners = if self.call_result {
            &self.seeded
        } else {
            &owners
        };
        for result in &self.results {
            if result_owners.is_empty() {
                registers.owners.remove(&result.0);
            } else {
                registers.owners.insert(result.0, result_owners.clone());
            }
        }
        if !self.ended.is_empty() {
            for map in [&mut registers.owners, &mut registers.single_hop] {
                for roots in map.values_mut() {
                    roots.retain(|root| !self.ended.contains(root));
                }
                map.retain(|_, roots| !roots.is_empty());
            }
        }
        owners
    }
}

/// An instruction's uses from the roots it retains and the per-operand roots
/// its scalar reads borrow: a by-value operand's roots load in rank order, an
/// addressed one's are retained, and a root also retained is not loaded.
fn split_register_uses(
    mut retained: BTreeSet<VarId>,
    single_hop: Vec<(u32, BTreeSet<VarId>)>,
    by_value: impl FnOnce() -> Vec<(u32, usize)>,
) -> RegisterUses {
    let loaded = split_single_hop(&mut retained, single_hop, by_value);
    RegisterUses {
        retained: retained.into_iter().collect(),
        loaded,
    }
}

/// What a method, operator, or subscript call binds: a declared function,
/// or a requirement a call through a trait bound selects.
#[derive(Clone, Copy)]
enum Callee<'a> {
    Declared(&'a MirFunctionDeclaration),
    Required(&'a TraitRequirement),
}

impl<'a> Callee<'a> {
    fn contract(self) -> ParameterContract<'a> {
        match self {
            Self::Declared(declaration) => declaration.into(),
            Self::Required(requirement) => requirement.into(),
        }
    }

    /// Push `receiver` at rank zero when the callee takes it by value. A
    /// requirement's receiver is a bound parameter, which is memory-only.
    fn receiver_by_value(
        self,
        receiver: Reg,
        register_types: &HashMap<u32, Ty>,
        loaded: &mut Vec<(u32, usize)>,
    ) {
        if let Self::Declared(declaration) = self {
            receiver_by_value(declaration, receiver, register_types, loaded);
        }
    }
}

/// The parameters a call binds its arguments to, as a declaration, a
/// requirement, or a function type spells them.
struct ParameterContract<'c> {
    names: &'c [String],
    types: &'c [mojito_types::types::Ty],
    conventions: &'c [Option<mojito_ast::ast::ArgConvention>],
    /// The first positional argument the variadic collector takes.
    collector: Option<usize>,
}

impl<'c> From<&'c MirFunctionDeclaration> for ParameterContract<'c> {
    fn from(declaration: &'c MirFunctionDeclaration) -> Self {
        Self {
            names: &declaration.param_names,
            types: &declaration.param_types,
            conventions: &declaration.param_conventions,
            collector: declaration.variadic_index,
        }
    }
}

impl<'c> From<&'c TraitRequirement> for ParameterContract<'c> {
    fn from(requirement: &'c TraitRequirement) -> Self {
        Self {
            names: &requirement.param_names,
            types: &requirement.param_types,
            conventions: &requirement.param_conventions,
            collector: requirement.variadic_index,
        }
    }
}

impl<'c> From<&'c mojito_types::types::FuncTy> for ParameterContract<'c> {
    fn from(function: &'c mojito_types::types::FuncTy) -> Self {
        Self {
            names: &function.names,
            types: &function.params,
            conventions: &function.conventions,
            collector: function
                .variadic
                .as_ref()
                .map(|_| function.keyword_only.unwrap_or(function.params.len())),
        }
    }
}

impl ParameterContract<'_> {
    /// Whether positional argument `index` binds a parameter that receives
    /// it by value, rather than the variadic collector.
    fn loads_positional(&self, index: usize) -> bool {
        self.collector.is_none_or(|collector| index < collector) && self.loads(index)
    }

    /// Whether parameter `slot` receives its argument by value: a `read` or
    /// `var` convention over a concrete register-passable type.
    fn loads(&self, slot: usize) -> bool {
        use mojito_ast::ast::ArgConvention;
        self.types
            .get(slot)
            .is_some_and(|ty| !may_alias_owned_storage(ty))
            && matches!(
                self.conventions.get(slot).copied().flatten(),
                None | Some(ArgConvention::Imm | ArgConvention::Var)
            )
    }
}

/// A call's argument registers and their retained caller places.
#[derive(Default)]
struct CallArguments<'c> {
    args: &'c [Reg],
    kwargs: &'c [(String, Reg)],
    arg_places: &'c [Option<MirPlace>],
    kwarg_places: &'c [Option<MirPlace>],
    spread: Option<usize>,
}

impl CallArguments<'_> {
    /// Push each argument `contract` loads by value, ranked by its parameter
    /// slot after `offset` receiver slots: a retained place, a spread pack,
    /// and a variadic element are addressed.
    fn by_value(
        &self,
        contract: &ParameterContract,
        offset: usize,
        loaded: &mut Vec<(u32, usize)>,
    ) {
        let placed = |places: &[Option<MirPlace>], index: usize| {
            places.get(index).is_some_and(Option::is_some)
        };
        for (index, argument) in self.args.iter().enumerate() {
            if self.spread != Some(index)
                && !placed(self.arg_places, index)
                && contract.loads_positional(index)
            {
                loaded.push((argument.0, offset + index));
            }
        }
        for (index, (name, argument)) in self.kwargs.iter().enumerate() {
            if placed(self.kwarg_places, index) {
                continue;
            }
            if let Some(slot) = contract
                .names
                .iter()
                .position(|parameter| parameter == name)
                && contract.loads(slot)
            {
                loaded.push((argument.0, offset + slot));
            }
        }
    }
}

/// The slice descriptors' bounds of subscript `args`: every descriptor is
/// built before the call loads its receiver and plain indices.
fn subscript_slices_loaded(args: &[MirSubscriptArg], loaded: &mut Vec<(u32, usize)>) {
    for argument in args {
        if let MirSubscriptArg::Slice {
            lower, upper, step, ..
        } = argument
        {
            slice_bounds_loaded(*lower, *upper, *step, 0, loaded);
        }
    }
}

/// The plain subscript indices `contract` loads by value, each at its
/// parameter rank after the receiver.
fn subscript_indices_loaded(
    contract: &ParameterContract,
    args: &[MirSubscriptArg],
    arg_places: &[Option<MirPlace>],
    loaded: &mut Vec<(u32, usize)>,
) {
    for (index, argument) in args.iter().enumerate() {
        let placed = arg_places.get(index).is_some_and(Option::is_some);
        if let MirSubscriptArg::Index(register) = argument
            && !placed
            && contract.loads_positional(index)
        {
            loaded.push((register.0, index + 1));
        }
    }
}

/// Push a slice's present bounds at `rank`: the slice descriptor the call
/// receives is built from them by value before the call loads anything else.
fn slice_bounds_loaded(
    lower: Option<Reg>,
    upper: Option<Reg>,
    step: Option<Reg>,
    rank: usize,
    loaded: &mut Vec<(u32, usize)>,
) {
    loaded.extend(
        [lower, upper, step]
            .into_iter()
            .flatten()
            .map(|bound| (bound.0, rank)),
    );
}

/// A call receiver's type: its retained place's, else its register's.
fn receiver_ty<'t>(
    place: Option<&'t MirPlace>,
    receiver: Reg,
    register_types: &'t HashMap<u32, Ty>,
) -> Option<&'t Ty> {
    place
        .and_then(|place| place.ty.as_ref())
        .or_else(|| register_types.get(&receiver.0))
}

/// Push `receiver` at rank zero when `declaration` takes its receiver by
/// value: a `read` or `var` `self` of a register-passable type.
fn receiver_by_value(
    declaration: &MirFunctionDeclaration,
    receiver: Reg,
    register_types: &HashMap<u32, mojito_types::types::Ty>,
    loaded: &mut Vec<(u32, usize)>,
) {
    use mojito_ast::ast::ArgConvention;
    if declaration.has_receiver
        && matches!(
            declaration.receiver_convention,
            None | Some(ArgConvention::Imm | ArgConvention::Var)
        )
        && register_types
            .get(&receiver.0)
            .is_some_and(|ty| !may_alias_owned_storage(ty))
    {
        loaded.push((receiver.0, 0));
    }
}

/// Which registers a later instruction still reads, so a register-loan
/// state can forget the rest: per block the registers live on entry, and per
/// instruction the registers it reads or writes that are dead after it.
struct RegisterLiveness {
    live_in: Vec<DenseSet>,
    /// Per block, the dead registers of every instruction in order, and the
    /// end of each instruction's run of them.
    dead_after: Vec<(Vec<u32>, Vec<usize>)>,
}

impl RegisterLiveness {
    /// A `try` reads every register its regions read, since its body region
    /// starts from the state entering it.
    fn new(
        blocks: &[MirBlock],
        steps: &[Vec<RegisterLoanStep>],
        predecessors: &[Vec<usize>],
    ) -> Self {
        // Per block, every instruction's reads in order, and the end of each
        // instruction's run of them.
        let block_reads: Vec<(Vec<u32>, Vec<usize>)> = blocks
            .iter()
            .zip(steps)
            .map(|(body, steps)| {
                let mut reads = Vec::new();
                let mut ends = Vec::with_capacity(steps.len());
                for (instruction, step) in body.instrs.iter().zip(steps) {
                    reads.extend(step.operands.iter().map(|reg| reg.0));
                    region_operand_regs(instruction, &mut reads);
                    ends.push(reads.len());
                }
                (reads, ends)
            })
            .collect();
        let reads = |block: usize, index: usize| {
            let (reads, ends) = &block_reads[block];
            let start = index.checked_sub(1).map_or(0, |previous| ends[previous]);
            &reads[start..ends[index]]
        };
        let live_out = |live_in: &[DenseSet], block: usize| {
            let mut out = DenseSet::default();
            for successor in terminator_targets(&blocks[block].term) {
                if let Some(live) = live_in.get(successor) {
                    out.union(live);
                }
            }
            out
        };
        let mut live_in = vec![DenseSet::default(); blocks.len()];
        let mut worklist = Worklist::backward(blocks);
        while let Some(block) = worklist.pop() {
            let mut live = live_out(&live_in, block);
            for (index, step) in steps[block].iter().enumerate().rev() {
                for result in &step.results {
                    live.remove(result.0);
                }
                live.extend(reads(block, index).iter().copied());
            }
            if live != live_in[block] {
                live_in[block] = live;
                for predecessor in &predecessors[block] {
                    worklist.push(*predecessor);
                }
            }
        }
        let dead_after = (0..blocks.len())
            .map(|block| {
                let mut live = live_out(&live_in, block);
                let mut runs: Vec<Vec<u32>> = vec![Vec::new(); steps[block].len()];
                for (index, step) in steps[block].iter().enumerate().rev() {
                    let read = reads(block, index);
                    let touched = read.iter().copied().chain(step.results.iter().map(|r| r.0));
                    runs[index].extend(touched.filter(|reg| !live.contains(*reg)));
                    for result in &step.results {
                        live.remove(result.0);
                    }
                    live.extend(read.iter().copied());
                }
                let mut ends = Vec::with_capacity(runs.len());
                let mut dead = Vec::new();
                for run in runs {
                    dead.extend(run);
                    ends.push(dead.len());
                }
                (dead, ends)
            })
            .collect();
        Self {
            live_in,
            dead_after,
        }
    }

    /// `state` restricted to the registers live entering `block`.
    fn entering(&self, block: usize, mut state: RegisterLoanState) -> RegisterLoanState {
        let live = &self.live_in[block];
        state.owners.retain(|register, _| live.contains(*register));
        state
            .single_hop
            .retain(|register, _| live.contains(*register));
        state
    }

    /// Forget the registers dead after instruction `index` of `block`.
    fn leaving(&self, block: usize, index: usize, state: &mut RegisterLoanState) {
        let (dead, ends) = &self.dead_after[block];
        let start = index.checked_sub(1).map_or(0, |previous| ends[previous]);
        for register in &dead[start..ends[index]] {
            state.owners.remove(register);
            state.single_hop.remove(register);
        }
    }
}

/// Every register read anywhere in a `try`'s regions.
fn region_operand_regs(instruction: &MirInstr, out: &mut Vec<u32>) {
    let MirInstr::Try {
        body,
        handler,
        orelse,
        finalbody,
        ..
    } = instruction
    else {
        return;
    };
    let mut registers = Vec::new();
    let mut visit = |instruction: &MirInstr| {
        mojito_mir::mir::verify::instruction_operand_regs(instruction, &mut registers);
    };
    for_each_instr_deep(body, &mut visit);
    if let Some((_, handler)) = handler {
        for_each_instr_deep(handler, &mut visit);
    }
    if let Some(orelse) = orelse {
        for_each_instr_deep(orelse, &mut visit);
    }
    if let Some(finalbody) = finalbody {
        for_each_instr_deep(finalbody, &mut visit);
    }
    out.extend(registers.into_iter().map(|reg| reg.0));
}
