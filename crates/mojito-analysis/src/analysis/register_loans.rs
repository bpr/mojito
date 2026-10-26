//! Register-loan dataflow: which registers carry loans of owned
//! storage across instructions.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_mir::mir::{
    ArgumentBinding, CallArguments, Callee, ParameterContract, RegisterEnds, receiver_ty,
};
use mojito_types::types::{Ty, TyKind};

#[derive(Clone, Default, PartialEq, Eq)]
pub(super) struct RegisterLoanState {
    owners: BTreeMap<u32, BTreeSet<ValueRef>>,
    /// Parts a scalar `LoadPlace` result keeps alive through its consumers
    /// only: reported as uses at every instruction reading the register, but
    /// never inherited by that instruction's own results (`t.n + 1` borrows
    /// `t` for `__add__` alone; the sum owns its bits).
    single_hop: BTreeMap<u32, BTreeSet<ValueRef>>,
    /// Parts an aggregate `LoadPlace` result aliases: its shallow read shares
    /// their storage, so they live through every consumer and pass to each
    /// derived register until a copy initializer gives the copy its own
    /// storage. The loans the value carries stay in `owners`.
    aliased: BTreeMap<u32, BTreeSet<ValueRef>>,
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
    for (register, owners) in &right.aliased {
        left.aliased.entry(*register).or_default().extend(owners);
    }
    left
}

pub(super) fn active_register_loan_roots(
    references: impl IntoIterator<Item = VarId>,
    state: &LoanGenerationState,
    generations: &BTreeMap<u32, DropLoanGeneration>,
) -> BTreeSet<ValueRef> {
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
            for part in &generation.roots {
                if roots.insert(*part) {
                    pending.push(part.root);
                }
            }
        }
    }
    roots
}

/// Whether a shallowly read value of this type can alias heap storage owned by
/// its source place — an aggregate whose fields may hold owning pointers (the
/// self-hosted `String`/`List` shape, and the built-in `Error`, upstream's
/// memory-only library struct), or any binder, a requirement's
/// abstract `Self` among them. Scalars and compile-time values own their
/// bits; a bare `Pointer` read aliases by design (the `unsafe_*`
/// vocabulary makes its lifetime the user's obligation).
pub(super) fn may_alias_owned_storage(ty: &Ty) -> bool {
    matches!(
        ty.kind(),
        TyKind::Struct(..)
            | TyKind::Error
            | TyKind::Tuple(_)
            | TyKind::RuntimePack(_)
            | TyKind::Variant(_)
            | TyKind::ComptimeList(_)
            | TyKind::Param { .. }
            | TyKind::Assoc { .. }
            | TyKind::SelfType
    )
}

/// The parts of roots one instruction consumes through its operand
/// registers, each the part the register's source place reached
/// ([`DropValues::access`]).
pub(super) struct RegisterUses {
    /// Parts the instruction keeps alive through itself: owner provenance,
    /// and the owners of scalars it receives by address.
    pub(super) retained: Vec<ValueRef>,
    /// The owners of scalars the instruction receives by value, in the order
    /// it loads them (parameter-slot order): each read is a load at call
    /// setup, so an owner whose last use it is dies before the instruction.
    pub(super) loaded: Vec<ValueRef>,
    /// The owned registers it defines, upstream's owned SSA results; none
    /// for a raising instruction, whose results its raise-edge `if`
    /// defines ([`DropFacts::op_defined`]).
    pub(super) defined: Vec<ValueRef>,
    /// The roots of the owned registers it takes ownership of
    /// ([`RegisterEnds::consumed`]), which their new owner ends.
    pub(super) ended: Vec<VarId>,
}

impl RegisterUses {
    const fn none() -> Self {
        Self {
            retained: Vec::new(),
            loaded: Vec::new(),
            defined: Vec::new(),
            ended: Vec::new(),
        }
    }
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
            } => Callee::direct(self.declarations, &func.0)
                .map(|callee| {
                    let mut loaded = Vec::new();
                    let arguments = CallArguments {
                        args,
                        kwargs,
                        arg_places,
                        kwarg_places,
                        spread: *spread,
                    };
                    arguments_by_value(&arguments, &callee.contract(), 0, &mut loaded);
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
                    Callee::resolve(
                        self.declarations,
                        name,
                        receiver_ty(recv_place.as_deref(), *recv, register_types),
                    )
                })
                .map(|callee| {
                    let mut loaded = Vec::new();
                    if recv_place.is_none() {
                        callee_receiver_by_value(callee, *recv, register_types, &mut loaded);
                    }
                    let arguments = CallArguments {
                        args,
                        kwargs,
                        arg_places,
                        kwarg_places,
                        spread: *spread,
                    };
                    arguments_by_value(&arguments, &callee.contract(), 1, &mut loaded);
                    loaded
                })
                .unwrap_or_default(),
            MirInstr::BinOp {
                a,
                b,
                resolved: Some(name),
                ..
            } => Callee::resolve(
                self.declarations,
                name,
                receiver_ty(None, *a, register_types),
            )
            .map(|callee| {
                let mut loaded = Vec::new();
                callee_receiver_by_value(callee, *a, register_types, &mut loaded);
                let arguments = CallArguments {
                    args: std::slice::from_ref(b),
                    ..CallArguments::default()
                };
                arguments_by_value(&arguments, &callee.contract(), 1, &mut loaded);
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
                    arguments_by_value(&arguments, &function.as_ref().into(), 0, &mut loaded);
                } else if let Some(declaration) = resolved
                    .as_deref()
                    .and_then(|name| self.declarations.function(name))
                {
                    arguments_by_value(&arguments, &declaration.into(), 1, &mut loaded);
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
            } => Callee::resolve(
                self.declarations,
                &call.target,
                receiver_ty(base_place.as_deref(), *base, register_types),
            )
            .map(|callee| {
                let mut loaded = Vec::new();
                if base_place.is_none() {
                    callee_receiver_by_value(callee, *base, register_types, &mut loaded);
                }
                if index_place.is_none() && loads_positional(&callee.contract(), 0) {
                    loaded.push((index.0, 1));
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
            } => Callee::resolve(
                self.declarations,
                &call.target,
                receiver_ty(object_place.as_deref(), *object, register_types),
            )
            .map(|callee| {
                let mut loaded = Vec::new();
                if object_place.is_none() {
                    callee_receiver_by_value(callee, *object, register_types, &mut loaded);
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
            } => Callee::resolve(
                self.declarations,
                &call.target,
                receiver_ty(receiver_place.as_deref(), *receiver, register_types),
            )
            .map(|callee| {
                let mut loaded = Vec::new();
                subscript_indices_loaded(&callee.contract(), args, arg_places, &mut loaded);
                loaded
            })
            .unwrap_or_default(),
            // The rest run callee code or consume a whole value: none loads a
            // scalar operand.
            MirInstr::Try { .. }
            | MirInstr::GetIter { .. }
            | MirInstr::HasNext { .. }
            | MirInstr::Next { .. } => Vec::new(),
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

/// The forward facts drop elaboration reads, from one walk of the
/// structured body (`walk_forward`, entering every region op): the loan
/// generations reaching each instruction and each block's ends, the roots
/// each instruction consumes through its operand registers, and the owned
/// registers each instruction, region op, and terminator defines or ends.
/// Keyed by node address, which is stable while the body is borrowed; an
/// instruction the walk never reaches has no generations and consumes
/// nothing.
pub(super) struct DropFacts {
    /// The distinct generation states the walk passed through.
    states: Vec<LoanGenerationState>,
    /// The state reaching each instruction, region ops included.
    before: HashMap<*const MirInstr, usize>,
    uses: HashMap<*const MirInstr, RegisterUses>,
    blocks: HashMap<*const MirBlock, BlockFacts>,
    /// The owned registers each region op defines, and those it ends.
    ops: HashMap<*const MirInstr, OpRegisters>,
    unreached: LoanGenerationState,
    unused: RegisterUses,
    no_registers: Vec<ValueRef>,
}

/// The owned registers one region op defines and ends.
#[derive(Default)]
struct OpRegisters {
    /// Its owned results, and for a raising instruction's raise-edge `if`
    /// the instruction's owned results, which exist on its normal path
    /// alone.
    defined: Vec<ValueRef>,
    /// What it takes ownership of: a loop's operands.
    ended: Vec<ValueRef>,
}

impl DropFacts {
    /// Walk `f` forward from its entry, where no generation is active and
    /// no register carries a loan. A raise hands its handler the
    /// generations and no register: no SSA handle survives into a region
    /// entered by raise.
    pub(super) fn new(
        f: &MirFunction,
        declarations: &MirDeclarations,
        loan_roots: &BTreeMap<u32, DropLoanGeneration>,
        generation_dests: &BTreeMap<u32, Option<MirInteriorOrigin>>,
        passing: &ArgumentPassing,
        values: &DropValues,
        exits: &[VarId],
    ) -> Self {
        let ends = RegisterEnds::new(f, declarations);
        let scan = DropFactsScan {
            loan_roots,
            generation_dests,
            reg_types: &f.reg_types,
            values,
            ends: &ends,
            exits,
        };
        let mut observer = DropFactsObserver {
            scan: &scan,
            passing,
            raise_flags: HashMap::new(),
            facts: Self {
                states: Vec::new(),
                before: HashMap::new(),
                uses: HashMap::new(),
                blocks: HashMap::new(),
                ops: HashMap::new(),
                unreached: LoanGenerationState::default(),
                unused: RegisterUses::none(),
                no_registers: Vec::new(),
            },
        };
        // The observer never refuses an instruction.
        let _ = walk_forward(&scan, DropFactsState::default(), &f.blocks, &mut observer);
        observer.facts
    }

    /// The generations reaching `instr`.
    pub(super) fn before(&self, instr: &MirInstr) -> &LoanGenerationState {
        self.before
            .get(&std::ptr::from_ref(instr))
            .map_or(&self.unreached, |index| &self.states[*index])
    }

    /// The parts `instr` consumes through its operand registers.
    pub(super) fn uses(&self, instr: &MirInstr) -> &RegisterUses {
        self.uses
            .get(&std::ptr::from_ref(instr))
            .unwrap_or(&self.unused)
    }

    /// The generations entering `block`.
    pub(super) fn entry(&self, block: &MirBlock) -> &LoanGenerationState {
        self.blocks
            .get(&std::ptr::from_ref(block))
            .map_or(&self.unreached, |facts| &self.states[facts.entry])
    }

    /// The generations entering a region: its first block's.
    pub(super) fn region_entry(&self, region: &[MirBlock]) -> &LoanGenerationState {
        region
            .first()
            .map_or(&self.unreached, |block| self.entry(block))
    }

    /// The generations at `block`'s terminator; `None` when the walk never
    /// reaches it.
    pub(super) fn end(&self, block: &MirBlock) -> Option<&LoanGenerationState> {
        self.blocks
            .get(&std::ptr::from_ref(block))
            .and_then(|facts| facts.end)
            .map(|index| &self.states[index])
    }

    /// Which of the variables the walk tracks the ending of have ended on
    /// every path to `block`'s terminator, a whole transfer's or teardown's;
    /// `None` when the walk never reaches it.
    pub(super) fn ended(&self, block: &MirBlock) -> Option<&BTreeSet<VarId>> {
        self.blocks
            .get(&std::ptr::from_ref(block))
            .and_then(|facts| facts.end.map(|_| &facts.ended))
    }

    /// The parts `block`'s terminator consumes through the values it
    /// yields, as an op's result takes each value over.
    pub(super) fn yielded(&self, block: &MirBlock) -> &RegisterUses {
        self.blocks
            .get(&std::ptr::from_ref(block))
            .map_or(&self.unused, |facts| &facts.yielded)
    }

    /// The owned registers `block`'s terminator takes ownership of
    /// ([`RegisterEnds::consumed_by`]): what it carries out of its region,
    /// returns, or raises, a use at the terminator and no death.
    pub(super) fn term_ended(&self, block: &MirBlock) -> &[ValueRef] {
        self.blocks
            .get(&std::ptr::from_ref(block))
            .map_or(&self.no_registers, |facts| &facts.term_ended)
    }

    /// The owned registers the region op `op` defines: its owned results,
    /// and a raising instruction's when `op` is its raise-edge `if`.
    pub(super) fn op_defined(&self, op: &MirInstr) -> &[ValueRef] {
        self.ops
            .get(&std::ptr::from_ref(op))
            .map_or(&self.no_registers, |registers| &registers.defined)
    }

    /// The owned registers the region op `op` takes ownership of: a loop's
    /// operands.
    pub(super) fn op_ended(&self, op: &MirInstr) -> &[ValueRef] {
        self.ops
            .get(&std::ptr::from_ref(op))
            .map_or(&self.no_registers, |registers| &registers.ended)
    }

    fn intern(&mut self, state: &LoanGenerationState) -> usize {
        if self.states.last() != Some(state) {
            self.states.push(state.clone());
        }
        self.states.len() - 1
    }
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
    /// The parts the instruction uses whatever its operands carry: the
    /// active loan roots of the variables it reads, and the parts its
    /// `MakeRef` retains or its callee-produced handle seeds.
    owners: BTreeSet<ValueRef>,
    /// The parts a callee-produced handle result carries.
    seeded: BTreeSet<ValueRef>,
    /// Whether the results are fresh values that inherit only `seeded`.
    call_result: bool,
    /// The parts an aggregate place read's result aliases.
    aliased: BTreeSet<ValueRef>,
    /// Whether the instruction is a `value.copy` running a copy initializer,
    /// whose result inherits its operand's loans but none of its aliases.
    copies: bool,
    /// A scalar read's result register and the parts it borrows for one hop.
    single_hop: Option<(u32, BTreeSet<ValueRef>)>,
    /// The slots the instruction ends by moving them, and the roots of the
    /// owned registers it takes ownership of. An ended slot's storage backs
    /// no register: a by-value scalar already holds its bits, and a handle
    /// into the slot is dead (use after move is `moves`' error); an ended
    /// register's alias is dead likewise.
    ended: Vec<VarId>,
    /// The owned registers among the results, each its own root: a fresh
    /// owned value carries its own root and its operands' variable owners,
    /// never their registers, so its readers keep it, not them, live.
    owned: Vec<(Reg, ValueRef)>,
    /// The first register root: roots below it are variables'.
    first_register: VarId,
}

impl RegisterLoanStep {
    fn new(
        generations: &LoanGenerationState,
        instruction: &MirInstr,
        loan_roots: &BTreeMap<u32, DropLoanGeneration>,
        register_types: &HashMap<u32, mojito_types::types::Ty>,
        values: &DropValues,
        ends: &RegisterEnds,
    ) -> Self {
        // The part a place reaches, and the reference it is reached through.
        let reached = |place: &MirPlace| {
            std::iter::once(values.access(place)).chain(place.through.map(|v| values.whole(v)))
        };
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
        // an aggregate (or unknown-typed) result retains through every consumer
        // (`aliased`), as Mojo destroys `h` once `h.c`'s copy is made.
        // A scalar read owns its bits, but the read is still a borrow of its
        // owner that lasts through the instruction consuming the register (Mojo
        // passes `t.n` to `print` as a reference to `t`, which therefore outlives
        // the call): the root is retained for that single hop only.
        let mut aliased = BTreeSet::new();
        match instruction {
            MirInstr::MakeRef { place, .. } => owners.extend(reached(place)),
            MirInstr::LoadPlace { dest, place } => {
                if place
                    .ty
                    .as_ref()
                    .or_else(|| register_types.get(&dest.0))
                    .is_none_or(may_alias_owned_storage)
                {
                    aliased.extend(reached(place));
                } else {
                    single_hop = Some((dest.0, reached(place).collect()));
                }
            }
            _ => {}
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
            seeded.extend(reached(place));
        }
        // A free call returning a tracked pointer borrows through its retained
        // argument places.
        if let MirInstr::Call {
            dest, arg_places, ..
        } = instruction
            && tracked_pointer_result(register_types, dest)
        {
            for place in arg_places.iter().flatten() {
                seeded.extend(reached(place));
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
            if register_types
                .get(&dest.0)
                .is_none_or(may_alias_owned_storage)
            {
                seeded.extend(reached(place));
            } else {
                single_hop = Some((dest.0, reached(place).collect()));
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
                | MirInstr::MultiIndex { .. }
                | MirInstr::BinOp {
                    resolved: Some(_),
                    ..
                }
        );
        let mut results = Vec::new();
        mojito_mir::mir::verify::instruction_result_regs(instruction, &mut results);
        let owned = results
            .iter()
            .filter_map(|reg| {
                values
                    .register_root(*reg)
                    .map(|root| (*reg, values.whole(root)))
            })
            .collect();
        let mut consumed = Vec::new();
        ends.consumed(instruction, &mut consumed);
        let mut ended = super::scan::vars_moved(instruction);
        ended.extend(
            consumed
                .into_iter()
                .filter_map(|reg| values.register_root(reg)),
        );
        Self {
            ended,
            operands,
            results,
            owners,
            seeded,
            call_result,
            single_hop,
            aliased,
            copies: matches!(
                instruction,
                MirInstr::CopyValue {
                    constructor: Some(_),
                    ..
                }
            ),
            owned,
            first_register: values.first_register(),
        }
    }

    /// What the instruction does to owned registers: the ones it defines,
    /// on its normal path when it raises, and those it ends.
    fn register_effects(
        &self,
        instruction: &MirInstr,
        values: &DropValues,
    ) -> (Vec<ValueRef>, Vec<VarId>) {
        let defined = if instruction.raise_edge().is_some() {
            Vec::new()
        } else {
            self.owned.iter().map(|(_, root)| *root).collect()
        };
        let ended = self
            .ended
            .iter()
            .copied()
            .filter(|root| values.is_register_root(*root))
            .collect();
        (defined, ended)
    }

    /// The parts the instruction retains: its own owners and its operands'
    /// provenance. Each operand's single-hop parts, which a scalar read
    /// borrows for this one hop, are pushed to `single_hop`.
    fn retained(
        &self,
        registers: &RegisterLoanState,
        single_hop: &mut Vec<(u32, BTreeSet<ValueRef>)>,
    ) -> BTreeSet<ValueRef> {
        let mut owners = self.owners.clone();
        owners.extend(&self.aliased);
        for operand in &self.operands {
            for map in [&registers.owners, &registers.aliased] {
                owners.extend(map.get(&operand.0).into_iter().flatten());
            }
            if let Some(provenance) = registers.single_hop.get(&operand.0) {
                single_hop.push((operand.0, provenance.clone()));
            }
        }
        owners
    }

    /// Advance `registers` past the instruction.
    fn transfer(&self, registers: &mut RegisterLoanState) {
        let inherit = |own: &BTreeSet<ValueRef>, map: &BTreeMap<u32, BTreeSet<ValueRef>>| {
            let mut owners = own.clone();
            for operand in &self.operands {
                owners.extend(map.get(&operand.0).into_iter().flatten());
            }
            owners
        };
        let fresh = self.call_result || self.results.is_empty();
        let inherited = if fresh {
            BTreeSet::new()
        } else {
            inherit(&self.owners, &registers.owners)
        };
        let aliased = if fresh || self.copies {
            BTreeSet::new()
        } else {
            inherit(&self.aliased, &registers.aliased)
        };
        if let Some((dest, roots)) = &self.single_hop {
            registers.single_hop.insert(*dest, roots.clone());
        }
        let result_owners = if self.call_result {
            &self.seeded
        } else {
            &inherited
        };
        for result in &self.results {
            let owned = self.owned.iter().any(|(reg, _)| reg == result);
            let aliased: BTreeSet<ValueRef> = aliased
                .iter()
                .filter(|part| !owned || part.root < self.first_register)
                .copied()
                .collect();
            if aliased.is_empty() {
                registers.aliased.remove(&result.0);
            } else {
                registers.aliased.insert(result.0, aliased);
            }
            let owners = match self.owned.iter().find(|(reg, _)| reg == result) {
                Some((_, root)) => {
                    let mut owners: BTreeSet<ValueRef> = result_owners
                        .iter()
                        .filter(|part| part.root < self.first_register)
                        .copied()
                        .collect();
                    owners.insert(*root);
                    owners
                }
                None => result_owners.clone(),
            };
            if owners.is_empty() {
                registers.owners.remove(&result.0);
            } else {
                registers.owners.insert(result.0, owners);
            }
        }
        if !self.ended.is_empty() {
            for map in [
                &mut registers.owners,
                &mut registers.single_hop,
                &mut registers.aliased,
            ] {
                for parts in map.values_mut() {
                    parts.retain(|part| !self.ended.contains(&part.root));
                }
                map.retain(|_, roots| !roots.is_empty());
            }
        }
    }
}

/// An instruction's uses from the roots it retains and the per-operand roots
/// its scalar reads borrow: a by-value operand's roots load in rank order, an
/// addressed one's are retained, and a root also retained is not loaded.
fn split_register_uses(
    mut retained: BTreeSet<ValueRef>,
    single_hop: Vec<(u32, BTreeSet<ValueRef>)>,
    by_value: impl FnOnce() -> Vec<(u32, usize)>,
) -> RegisterUses {
    let loaded = split_single_hop(&mut retained, single_hop, by_value);
    RegisterUses {
        retained: retained.into_iter().collect(),
        loaded,
        ..RegisterUses::none()
    }
}

/// Push `receiver` at rank zero when `callee` takes it by value. A
/// requirement's receiver is a bound parameter, which is memory-only.
fn callee_receiver_by_value(
    callee: Callee,
    receiver: Reg,
    register_types: &HashMap<u32, Ty>,
    loaded: &mut Vec<(u32, usize)>,
) {
    if let Callee::Declared(declaration) = callee {
        receiver_by_value(declaration, receiver, register_types, loaded);
    }
}

/// Whether positional argument `index` binds a parameter of `contract`
/// that receives it by value, rather than the variadic collector.
fn loads_positional(contract: &ParameterContract, index: usize) -> bool {
    contract.collector.is_none_or(|collector| index < collector) && loads(contract, index)
}

/// Whether parameter `slot` of `contract` receives its argument by value: a
/// `read` or `var` convention over a concrete register-passable type.
fn loads(contract: &ParameterContract, slot: usize) -> bool {
    use mojito_ast::ast::ArgConvention;
    contract
        .types
        .get(slot)
        .is_some_and(|ty| !may_alias_owned_storage(ty))
        && matches!(
            contract.conventions.get(slot).copied().flatten(),
            None | Some(ArgConvention::Imm | ArgConvention::Var)
        )
}

/// Push each argument of `arguments` that `contract` loads by value,
/// ranked by its parameter slot after `offset` receiver slots: a retained
/// place, a spread pack, and a variadic element are addressed.
fn arguments_by_value(
    arguments: &CallArguments,
    contract: &ParameterContract,
    offset: usize,
    loaded: &mut Vec<(u32, usize)>,
) {
    for argument in arguments.bindings(contract) {
        if let (false, ArgumentBinding::Slot(slot)) = (argument.placed, argument.binding)
            && loads(contract, slot)
        {
            loaded.push((argument.reg.0, offset + slot));
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
            && loads_positional(contract, index)
        {
            loaded.push((register.0, index + 1));
        }
    }
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

/// What [`DropFacts`] keeps per block.
struct BlockFacts {
    entry: usize,
    end: Option<usize>,
    yielded: RegisterUses,
    ended: BTreeSet<VarId>,
    term_ended: Vec<ValueRef>,
}

/// One program point of the drop-facts walk.
#[derive(Clone, Default, PartialEq)]
struct DropFactsState {
    generations: LoanGenerationState,
    registers: RegisterLoanState,
    /// The tracked variables ended on every path here, upstream's
    /// `unusedMarkDestroyName` facts: an exit's `mark_destroyed` of one
    /// is no use.
    ended: BTreeSet<VarId>,
}

/// The forward walk [`DropFacts`] records.
struct DropFactsScan<'a> {
    loan_roots: &'a BTreeMap<u32, DropLoanGeneration>,
    generation_dests: &'a BTreeMap<u32, Option<MirInteriorOrigin>>,
    reg_types: &'a HashMap<u32, Ty>,
    values: &'a DropValues,
    ends: &'a RegisterEnds<'a>,
    /// The variables whose ending the walk tracks: the split `deinit`
    /// parameters, which every exit ends unless a path already did.
    exits: &'a [VarId],
}

impl DropFactsScan<'_> {
    fn step(&self, state: &DropFactsState, instr: &MirInstr) -> RegisterLoanStep {
        RegisterLoanStep::new(
            &state.generations,
            instr,
            self.loan_roots,
            self.reg_types,
            self.values,
            self.ends,
        )
    }
}

impl ForwardScan for DropFactsScan<'_> {
    type State = DropFactsState;

    fn join(&self, state: &DropFactsState, other: &DropFactsState) -> DropFactsState {
        DropFactsState {
            generations: join_loan_generation_states(state.generations.clone(), &other.generations),
            registers: join_register_loan_states(state.registers.clone(), &other.registers),
            ended: state.ended.intersection(&other.ended).copied().collect(),
        }
    }

    fn transfer(&self, state: &mut DropFactsState, instr: &MirInstr) {
        self.step(state, instr).transfer(&mut state.registers);
        transfer_loan_generation(&mut state.generations, instr, self.generation_dests);
        if !self.exits.is_empty() {
            if let Some(def) = var_def(instr) {
                state.ended.remove(&def);
            }
            let ended = vars_moved(instr).into_iter();
            state
                .ended
                .extend(ended.filter(|var| self.exits.contains(var)));
        }
    }

    fn raise_seed(&self, state: &DropFactsState) -> DropFactsState {
        DropFactsState {
            generations: state.generations.clone(),
            registers: RegisterLoanState::default(),
            ended: state.ended.clone(),
        }
    }

    fn enters(&self, _instr: &MirInstr) -> bool {
        true
    }
}

/// The observer filling [`DropFacts`].
struct DropFactsObserver<'a> {
    scan: &'a DropFactsScan<'a>,
    passing: &'a ArgumentPassing<'a>,
    /// Each raising instruction's error flag beside its owned results,
    /// which its raise-edge `if` defines.
    raise_flags: HashMap<u32, Vec<ValueRef>>,
    facts: DropFacts,
}

impl ForwardObserver<DropFactsState> for DropFactsObserver<'_> {
    fn instr(&mut self, state: &DropFactsState, instr: &MirInstr) -> Result<(), OwnershipError> {
        let index = self.facts.intern(&state.generations);
        self.facts.before.insert(std::ptr::from_ref(instr), index);
        let mut single_hop = Vec::new();
        let step = self.scan.step(state, instr);
        let retained = step.retained(&state.registers, &mut single_hop);
        let mut uses = split_register_uses(retained, single_hop, || {
            self.passing.by_value(instr, self.scan.reg_types)
        });
        (uses.defined, uses.ended) = step.register_effects(instr, self.scan.values);
        if let Some(raises) = instr.raise_edge() {
            let results: Vec<ValueRef> = step.owned.iter().map(|(_, root)| *root).collect();
            if !results.is_empty() {
                self.raise_flags.insert(raises.flag.0, results);
            }
        }
        self.facts.uses.insert(std::ptr::from_ref(instr), uses);
        Ok(())
    }

    fn op(&mut self, state: &DropFactsState, instr: &MirInstr) {
        let index = self.facts.intern(&state.generations);
        self.facts.before.insert(std::ptr::from_ref(instr), index);
        let values = self.scan.values;
        let mut results = Vec::new();
        mojito_mir::mir::owned_result_regs(instr, &mut results);
        let mut defined = values.register_refs(results);
        if let MirInstr::If { cond, .. } = instr
            && let Some(raised) = self.raise_flags.get(&cond.0)
        {
            defined.extend(raised.iter().copied());
        }
        let mut consumed = Vec::new();
        self.scan.ends.consumed(instr, &mut consumed);
        let ended = values.register_refs(consumed);
        if !defined.is_empty() || !ended.is_empty() {
            self.facts
                .ops
                .insert(std::ptr::from_ref(instr), OpRegisters { defined, ended });
        }
    }

    fn block(&mut self, entry: &DropFactsState, end: Option<&DropFactsState>, block: &MirBlock) {
        let entry = self.facts.intern(&entry.generations);
        let mut yielded = RegisterUses::none();
        let term_ended = self
            .scan
            .values
            .register_refs(self.scan.ends.consumed_by(&block.term));
        let ended = end.map(|state| state.ended.clone()).unwrap_or_default();
        let end = end.map(|state| {
            let mut owners = BTreeSet::new();
            for value in handed_values(&block.term) {
                let provenance = [
                    &state.registers.owners,
                    &state.registers.single_hop,
                    &state.registers.aliased,
                ];
                for map in provenance {
                    owners.extend(map.get(&value.0).into_iter().flatten());
                }
            }
            yielded.retained = owners.into_iter().collect();
            self.facts.intern(&state.generations)
        });
        self.facts.blocks.insert(
            std::ptr::from_ref(block),
            BlockFacts {
                entry,
                end,
                yielded,
                ended,
                term_ended,
            },
        );
    }
}
