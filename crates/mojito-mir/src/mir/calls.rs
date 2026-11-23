//! Call-site lowering: argument/keyword/receiver lowering, checked-call value
//! adjustments and boundaries, reference-result places, and interior-origin
//! invalidation emission.
//! Extracted from `mir.rs`; see `docs/symbol-map.md`.

use super::call_results::{CValue, OperandBinding};
#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_checked::checked::CheckedCallArgumentSource;
use mojito_types::types::TyKind;

/// A call's positional registers and places, then its keyword registers and
/// places.
pub(super) type FieldwiseCallArguments = (
    Vec<Reg>,
    Vec<Option<MirPlace>>,
    Vec<(String, Reg)>,
    Vec<Option<MirPlace>>,
);

/// A call's keyword registers and, aligned with them, the retained caller
/// place each reads.
pub(super) type LoweredKeywords = (Vec<(String, Reg)>, Vec<Option<MirPlace>>);

/// One open call's setup: the reads of its immediate by-value operands, held
/// until its last operand is evaluated. Mojo evaluates every argument to an
/// lvalue in source order and reads each at call setup, so a later
/// argument's write to the place is seen by an earlier one. A simple place
/// has no lvalue work, so its whole read sequence (`UseVar`/`LoadPlace`, a
/// `var` copy, an implicit conversion) is setup work; a reference result (an
/// accessor or a `ref`-returning call) is lvalue work where written and a
/// read at setup ([`SetupReference`]), as is a bound collection display,
/// whose elements are its lvalue work.
pub(super) struct CallSetup {
    /// The [`Flatten::argument_depth`] the call's own operands lower at.
    depth: usize,
    /// Each deferred read sequence with the operand it reads: `None` for a
    /// receiver or an operator operand, which precede every argument.
    reads: Vec<(Option<CheckedCallArgumentSource>, Vec<MirInstr>)>,
    /// Each owned operand's transfer (its `UseVar` forward) with the operand
    /// it transfers, emitted after every read: the call consumes its owned
    /// operands, so a raise in a later operand finds them still in their
    /// slots and an earlier operand may still read a value a later `^`
    /// transfers.
    forwards: Vec<(Option<CheckedCallArgumentSource>, Vec<MirInstr>)>,
}

/// A by-value operand whose reference result is read at its call's setup.
/// Its lvalue work (receiver, index, accessor or call) runs where written and
/// leaves the reference in a hidden handle slot whose setup loans keep its
/// owners live without excluding the later operands' accesses
/// (`MirLoan::setup`); everything after the handle, the read and any
/// conversion of it, moves to the setup frame. A bound collection display
/// ([`Flatten::setup_display`]) is the other such operand: its elements'
/// lvalue work runs where written, and its element reads, its construction,
/// and the argument's binding of it move to the setup frame.
pub(super) struct SetupReference {
    /// The operand's [`Node::address`].
    node: usize,
    /// Where the read begins once the handle is materialized: the index of
    /// the first instruction after the handle's loans.
    read_start: Option<usize>,
}

/// Whether a literal register of type `found` materializes at the checked
/// parameter type `target` as a written literal argument does: a numeric
/// literal at a scalar, or at a multi-lane vector it splats to.
pub(super) fn literal_materializes_to(found: &Ty, target: &Ty) -> bool {
    let scalar = match found.kind() {
        kind if kind.is_int_literal() => {
            target.is_int()
                || target.is_uint()
                || target.is_float64()
                || mojito_types::types::is_scalar_simd(target)
        }
        kind if kind.is_float_literal() => {
            target.is_float64()
                || mojito_types::types::scalar_simd_dtype(target)
                    .is_some_and(mojito_ast::ast::Dtype::is_float)
        }
        _ => false,
    };
    scalar
        || mojito_types::types::simd_shape(target).is_some_and(|(dtype, width)| {
            width > 1
                && found.kind().is_numeric_literal()
                && mojito_types::types::splats_to(
                    found,
                    &mojito_types::types::SimdDtype::Known(dtype),
                )
        })
}

/// Whether a value of this type owns storage that destruction must release:
/// an aggregate (a struct with a destructor or owning fields, the built-in
/// tuple/pack storage, a variant, or a still-abstract parameter
/// type). A struct whose copy is its operand (a numeric literal, upstream's
/// `TrivialRegisterPassable` `IntLiteral[v]`/`FloatLiteral[v]`) owns none.
/// Kept textually aligned with the drop analysis'
/// `may_alias_owned_storage`, which drop elaboration refines by
/// `MirDeclarations::copy_is_identity`.
pub(super) fn owns_droppable_storage(ty: &Ty) -> bool {
    (ty.is_non_simd_struct() && !mojito_types::types::copy_is_identity(ty.kind()))
        || matches!(
            ty.kind(),
            TyKind::Tuple(_)
                | TyKind::RuntimePack(_)
                | TyKind::Variant(_)
                | TyKind::ComptimeList(_)
                | TyKind::Param { .. }
                | TyKind::Assoc { .. }
        )
}

impl<'a> Flatten<'a> {
    /// Lower one ordinary call argument together with the caller storage that
    /// checking selected for a `mut`/`ref` parameter. Dynamic indexed places
    /// cannot be reconstructed after evaluating their value: doing so either
    /// evaluates the index twice or loses the write-back place altogether.
    /// Flatten those actuals once and retain the resulting typed place. A
    /// nominal accessor-produced reference uses the same hidden handle slot as
    /// a chained method receiver.
    pub(super) fn lower_call_argument(&mut self, expression: Node<'a>) -> (Reg, Option<MirPlace>) {
        self.lower_call_argument_with(expression, CheckedCallArgumentSource::Positional(0))
    }

    /// The operand of truthiness test `operand`, the receiver of its
    /// `__bool__` call: a `read self` of a type the check proved
    /// register-passable (`RegisterPassableRead`) is read as a value, and
    /// any other is the immutable address of its place, or of the storage
    /// a temporary is materialized in, as upstream's `lit.call` takes the
    /// `!lit.ref` of a memory-only `x`; a `mut self` receiver is its
    /// place's mutable address. A result slot an `and`/`or` or a ternary
    /// forwards is read where it lies: the test borrows the temporary
    /// rather than taking it.
    pub(super) fn truthiness_operand(&mut self, operand: Node<'a>) -> Reg {
        use mojito_types::origin::Mutability;
        let call = operand
            .truthiness_call()
            .map(|truthiness| &*truthiness.call);
        let register_passable = operand.register_passable_read();
        let by_address = call.map_or_else(
            || (!register_passable).then_some(Mutability::Immutable),
            |call| match call.receiver_convention {
                Some(mojito_ast::ast::ArgConvention::Mut | mojito_ast::ast::ArgConvention::Ref) => {
                    Some(Mutability::Mutable)
                }
                _ => receiver_by_address(call, register_passable),
            },
        );
        if by_address == Some(Mutability::Mutable) {
            return self.mutable_receiver_operand(operand).0;
        }
        let binding = match call.and_then(OperandBinding::of_receiver) {
            Some(OperandBinding::Owned) => OperandBinding::Owned,
            _ => OperandBinding::Read {
                by_address: by_address.is_some(),
            },
        };
        self.bind_operand(operand, Some(binding));
        if !self.read_by_address_binding(operand) {
            return self.read_operand(operand);
        }
        let (value, place) = self.lower_call_operand(operand, None);
        if place.is_some() {
            return value;
        }
        let Some(&MirInstr::UseVar {
            dest,
            var,
            mode: UseMode::Forward,
            ..
        }) = self.f.body.instrs.last()
        else {
            return value;
        };
        if dest != value {
            return value;
        }
        let Some(ty) = self
            .var_types
            .get(&var)
            .or_else(|| self.f.reg_types.get(&value.0))
            .cloned()
            .or_else(|| operand.ty())
        else {
            return value;
        };
        self.f.body.instrs.pop();
        self.var_types.entry(var).or_insert_with(|| ty.clone());
        let place = MirPlace::root(var, Some(ty));
        self.emit(MirInstr::LoadPlace {
            dest: value,
            place: place.clone(),
        });
        self.address_lowered_operand(
            operand.location(),
            (value, Some(place)),
            Mutability::Immutable,
        )
        .0
    }

    /// The mutable address of `operand`'s place, the receiver of a call
    /// whose `mut` or `ref self` takes it where it lies, as upstream's
    /// `lit.call` takes its `!lit.ref`, with the place it addresses.
    pub(super) fn mutable_receiver_operand(
        &mut self,
        operand: Node<'a>,
    ) -> (Reg, Option<MirPlace>) {
        self.bind_operand(operand, None);
        let lowered = self.lower_call_receiver(operand);
        self.address_lowered_operand(
            operand.location(),
            lowered,
            mojito_types::origin::Mutability::Mutable,
        )
    }

    /// Lower one operand of a struct operator, prefix or infix: a place the checker
    /// marked `BorrowReadArgument` (the dunder reads it) is read where it
    /// lies, as a read-convention call argument is; a temporary returned
    /// through memory is used in its call-result slot by the operand's
    /// convention ([`Self::operand_c`]), as a call argument's is, so a read
    /// one dies when the dunder returns; anything else is an ordinary
    /// value. Inside an operator's setup frame
    /// ([`Self::open_call_setup`]) a simple place's read is deferred to the
    /// frame's setup. A temporary view operand anchors its loans as a call
    /// argument does, under the anchor permission of `operator`, the
    /// operator's own checked call, so its source outlives the dunder.
    pub(super) fn lower_operator_operand(
        &mut self,
        operator: Node<'a>,
        expression: Node<'a>,
    ) -> Reg {
        let saved_anchor_permission = self.allow_argument_anchors;
        self.allow_argument_anchors = operator.call_anchors_arguments();
        let value = self.lower_setup_operand(expression, None, false, |this| {
            let borrowed = expression.adjustments().iter().any(|adjustment| {
                matches!(
                    adjustment,
                    mojito_checked::checked::SemanticAdjustment::BorrowReadArgument
                )
            });
            borrowed
                .then(|| this.lower_borrowed_read_argument(expression))
                .flatten()
                .unwrap_or_else(|| this.temporary_operand(expression, false, false).0)
        });
        self.allow_argument_anchors = saved_anchor_permission;
        value
    }

    /// Record the call writing `dest` for
    /// [`crate::mir::fill_omitted_arguments`]: the callee the checked call
    /// selected, whose regular parameter slots begin after `leading`
    /// operands its declaration does not list.
    pub(super) fn record_call(&mut self, dest: Reg, callee: Option<String>, leading: usize) {
        self.f.lowered_calls.push(crate::mir::LoweredCall {
            dest,
            callee,
            leading,
        });
    }

    /// Lower `display`'s elements in order. A display is its constructor's
    /// call, so each element is evaluated where written and read at that
    /// call's setup, after the last element. When the display is itself a
    /// bound operand awaiting its enclosing call's setup, its element reads
    /// and the construction after them are that setup's work.
    pub(super) fn display_elements(
        &mut self,
        display: Node<'a>,
        elements: &[Node<'a>],
    ) -> Vec<Reg> {
        self.open_call_setup();
        let regs = elements
            .iter()
            .map(|&element| {
                self.lower_setup_operand(element, None, true, |this| this.expr(element))
            })
            .collect();
        if self.awaits_setup_read(display)
            && let Some(pending) = self.setup_reference.as_mut()
        {
            pending.read_start = Some(self.f.body.instrs.len());
        }
        self.close_call_setup(None);
        regs
    }

    /// Open a setup frame for the call whose operands are lowered next. Every
    /// opened frame is closed by [`Self::close_call_setup`] after the call's
    /// last operand and before anything consumes the operands' registers.
    pub(super) fn open_call_setup(&mut self) {
        self.call_setups.push(CallSetup {
            depth: self.argument_depth,
            reads: Vec::new(),
            forwards: Vec::new(),
        });
    }

    /// Close the innermost setup frame, emitting its deferred reads and then
    /// its owned operands' transfers, each ranked with the receiver and
    /// operator operands first, then the arguments in the parameter order
    /// `call`'s checked contract binds them in, as Mojo's call setup reads
    /// them and its call consumes them; a call without a contract keeps
    /// source order.
    pub(super) fn close_call_setup(&mut self, call: Option<Node<'a>>) {
        let Some(setup) = self.call_setups.pop() else {
            return;
        };
        let mut reads = setup.reads;
        let mut forwards = setup.forwards;
        if let Some(contract) = call.and_then(Node::call_contract) {
            let rank = |source: &Option<CheckedCallArgumentSource>| {
                source.map_or(0, |source| {
                    contract
                        .arguments
                        .iter()
                        .position(|argument| argument.source == source)
                        .map_or(usize::MAX, |slot| slot + 1)
                })
            };
            reads.sort_by_key(|(source, _)| rank(source));
            forwards.sort_by_key(|(source, _)| rank(source));
        }
        for instr in reads
            .into_iter()
            .chain(forwards)
            .flat_map(|(_, instrs)| instrs)
        {
            self.emit(instr);
        }
    }

    /// Lower one operand of the innermost open call with `lower`, one
    /// argument depth deeper so a nested call's operands never reach the
    /// outer frame. When the frame is this operand's own, its read defers
    /// to setup ([`Self::defer_to_setup`]). `bound`: the operand converts to
    /// a type its consumer declares (a call argument or a display element),
    /// rather than keeping its natural type (a receiver or operator operand).
    fn lower_setup_operand<R>(
        &mut self,
        expression: Node<'a>,
        source: Option<CheckedCallArgumentSource>,
        bound: bool,
        lower: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let owns_setup = self
            .call_setups
            .last()
            .is_some_and(|setup| setup.depth == self.argument_depth);
        self.argument_depth += 1;
        let lowered = if owns_setup {
            self.defer_to_setup(expression, source, bound, lower)
        } else {
            lower(self)
        };
        self.argument_depth -= 1;
        lowered
    }

    /// Run `lower` one argument depth deeper: lowering that is not one of
    /// the open call's operands (its compile-time arguments) never reaches
    /// its setup frame.
    pub(super) fn outside_call_setup<R>(&mut self, lower: impl FnOnce(&mut Self) -> R) -> R {
        self.argument_depth += 1;
        let lowered = lower(self);
        self.argument_depth -= 1;
        lowered
    }

    /// Run `lower`, and move the read it emitted to the innermost setup
    /// frame: everything when `expression` is a [`Self::setup_read`], and
    /// everything after the reference handle when it is a
    /// [`Self::setup_reference`] whose lowering materialized one
    /// ([`Self::materialize_reference_result_place`]), or everything after
    /// the elements of a `bound` [`Self::setup_display`]
    /// ([`Self::display_elements`]); an operand's region ops stay where
    /// written while its read defers. Otherwise an operand whose lowering ends in a `UseVar`
    /// forward (a `^` transfer or a taken call-result slot) defers that forward to the frame's transfers; the slot it
    /// empties is defined where written, which dominates the call.
    fn defer_to_setup<R>(
        &mut self,
        expression: Node<'a>,
        source: Option<CheckedCallArgumentSource>,
        bound: bool,
        lower: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let start = self.f.body.instrs.len();
        let read = self.setup_read(expression);
        let pending = (!read
            && (bound && Self::setup_display(expression) || self.setup_reference(expression)))
        .then(|| SetupReference {
            node: expression.address(),
            read_start: None,
        });
        let enclosing = std::mem::replace(&mut self.setup_reference, pending);
        let lowered = lower(self);
        let reference = std::mem::replace(&mut self.setup_reference, enclosing);
        let read_start = if read {
            Some(start)
        } else {
            reference.and_then(|reference| reference.read_start)
        };
        if let Some(start) = read_start
            && let Some(setup) = self.call_setups.last_mut()
        {
            let instrs = self.f.body.instrs.drain(start..).collect();
            setup.reads.push((source, instrs));
        } else if self.f.body.instrs.len() > start
            && matches!(
                self.f.body.instrs.last(),
                Some(MirInstr::UseVar {
                    mode: UseMode::Forward,
                    ..
                })
            )
            && let Some(setup) = self.call_setups.last_mut()
        {
            let forward = self.f.body.instrs.pop().into_iter().collect();
            setup.forwards.push((source, forward));
        }
        lowered
    }

    /// Whether `expression` is the operand [`Self::defer_to_setup`] is
    /// lowering for a setup read of its reference result or display
    /// elements, its handle or elements not yet materialized.
    pub(super) fn awaits_setup_read(&self, expression: Node<'a>) -> bool {
        self.setup_reference.as_ref().is_some_and(|reference| {
            reference.node == expression.address() && reference.read_start.is_none()
        })
    }

    /// Begin `expression`'s setup read here when its call's setup awaits it
    /// ([`Self::awaits_setup_read`]): what is emitted from now on moves to
    /// that setup.
    pub(super) fn begin_awaited_setup_read(&mut self, expression: Node<'a>) {
        if self.awaits_setup_read(expression)
            && let Some(pending) = self.setup_reference.as_mut()
        {
            pending.read_start = Some(self.f.body.instrs.len());
        }
    }

    /// Whether `expression` is a by-value operand whose reference result
    /// belongs to call setup: an accessor subscript or a `ref`-returning call
    /// read as a value, not a borrow, a materialized temporary, a module
    /// constant, a pointer handle, a spread, or a transfer. An accessor chain
    /// over a temporary (`first(Span(ys))[0]`) is read where written: the
    /// temporary's slot dies at its last use, and a view's referent is
    /// reached through it.
    fn setup_reference(&mut self, expression: Node<'a>) -> bool {
        // The root's place is probed last: interning it allocates a slot,
        // and an operand that is no reference (`DType.float32`) names none.
        if expression.subscript_write_back().is_some() {
            return true;
        }
        expression.reference_result().is_some()
            && expression.spread_operand().is_none()
            && expression.transfer_operand().is_none()
            && !expression.constant_param_value()
            && !expression.is_origin_bearing_pointer()
            && !expression.adjustments().iter().any(|adjustment| {
                matches!(
                    adjustment,
                    mojito_checked::checked::SemanticAdjustment::BorrowShared
                        | mojito_checked::checked::SemanticAdjustment::BorrowMutable
                )
            })
            && !self.reads_materialized_temporary(expression)
            && self
                .place_chain(expression)
                .last()
                .is_none_or(|root| self.simple_place(*root).is_some())
    }

    /// The objects `expression`'s accessor chain reads through, innermost
    /// last. A static call's receiver is a type, never a place, so the call
    /// ends its chain as a free call does; a parameter value read through a
    /// member (`Self.vs`) is a value, so it ends the chain too.
    fn place_chain(&self, expression: Node<'a>) -> Vec<Node<'a>> {
        std::iter::successors(Some(expression), |node| {
            node.object().filter(|object| {
                (!matches!(node.op(), CheckedOp::MethodCall { .. })
                    || self.type_receiver_name(*object).is_none())
                    && (node.recorded_value().is_none()
                        || !matches!(node.op(), CheckedOp::Member { .. }))
            })
        })
        .skip(1)
        .collect()
    }

    /// Whether `expression` is a temporary the checker materialized into a
    /// hidden slot, or reads through a compile-time constant so
    /// materialized: a struct value the check could not fold a field out
    /// of, since an initializer of its own built it (`FLAG.n` over
    /// `comptime FLAG: W = True`, through `W`'s `@implicit` `__init__`).
    /// Such a chain is read
    /// where written, and its place is never probed: probing a materialized
    /// constant root stores it in its slot, so a probe whose instructions
    /// the setup then moves would leave later reads of the slot ahead of
    /// its definition.
    fn reads_materialized_temporary(&self, expression: Node<'a>) -> bool {
        let materialized = |node: Node<'a>| {
            mojito_checked::checked::materialized_borrow_owner(node.adjustments()).is_some()
        };
        materialized(expression)
            || self.place_chain(expression).into_iter().any(|node| {
                materialized(node)
                    && (node.constant_param_value() || node.recorded_value().is_some())
            })
    }

    /// Whether `expression` is a by-value place whose read belongs to call
    /// setup: a direct root or field chain with no lvalue work, no reference
    /// alias between it and its owner, and none of the identifier lowerings
    /// that are values rather than reads (callables, closures, pointer
    /// handles, folded constants, materialized borrow slots).
    fn setup_read(&mut self, expression: Node<'a>) -> bool {
        self.reads_value(expression)
            && self.simple_place(expression).is_some_and(|place| {
                place.through.is_none()
                    && !self.aliases.contains_key(&place.root)
                    && !self.runtime_aliases.contains(&place.root)
            })
    }

    /// Whether `expression` is none of the identifier lowerings that are
    /// values rather than reads of its place: a callable, a closure, a
    /// pointer handle, a folded constant, or a materialized borrow slot.
    pub(super) fn reads_value(&self, expression: Node<'a>) -> bool {
        !(expression.resolved_callable().is_some()
            || self.nested_info(expression).is_some()
            || expression.is_origin_bearing_pointer()
            || expression.constant_param_value()
            || self.reads_materialized_temporary(expression)
            || matches!(
                expression.ty().as_ref().map(Ty::kind),
                Some(
                    TyKind::Func(_)
                        | TyKind::GenericFunc(_)
                        | TyKind::Overload(_)
                        | TyKind::Param {
                            callable_bound: Some(_),
                            ..
                        }
                )
            ))
    }

    /// Whether `expression` is a collection display, which a bound position
    /// types by its consumer's parameter: a list, set, dictionary, or array
    /// display is built as its argument's conversion, at setup, while a tuple
    /// display or a comprehension has a natural type and is built where
    /// written.
    const fn setup_display(expression: Node<'a>) -> bool {
        matches!(
            expression.op(),
            CheckedOp::ListLit(_) | CheckedOp::BraceLit(_)
        )
    }

    /// `source` names the argument for its setup read's parameter order.
    fn lower_call_argument_with(
        &mut self,
        expression: Node<'a>,
        source: CheckedCallArgumentSource,
    ) -> (Reg, Option<MirPlace>) {
        let owns_setup = self
            .call_setups
            .last()
            .is_some_and(|setup| setup.depth == self.argument_depth);
        self.argument_depth += 1;
        let lowered = self.lower_call_operand(expression, owns_setup.then_some(source));
        self.argument_depth -= 1;
        lowered
    }

    /// [`Self::lower_call_argument_with`] one argument depth down. `setup` is
    /// the argument's source when the innermost setup frame is its call's,
    /// so a copied-value read of a simple place defers to setup.
    ///
    /// An argument the checker bound to a `mut` or `ref` parameter
    /// (`RetainCallPlace`) is the mutable address of its place, a
    /// `ref.make`, and one bound to a `read` parameter the check judged by
    /// address ([`OperandBinding::Read`]) the immutable address of the
    /// place it retains, as upstream emits the `!lit.ref` while lowering
    /// the operand ([`Self::address_operand`]).
    fn lower_call_operand(
        &mut self,
        expression: Node<'a>,
        setup: Option<CheckedCallArgumentSource>,
    ) -> (Reg, Option<MirPlace>) {
        use mojito_types::origin::Mutability;
        let lowered = self.lower_call_operand_value(expression, setup);
        if expression.spread_operand().is_some() {
            return lowered;
        }
        let by_reference = expression.adjustments().iter().any(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::RetainCallPlace
            )
        });
        if by_reference {
            return self.address_operand(expression, lowered, Mutability::Mutable);
        }
        if self.read_by_address_binding(expression) {
            let lowered = match lowered {
                (value, None) => self.materialize_operand(expression, value),
                lowered => lowered,
            };
            return self.address_operand(expression, lowered, Mutability::Immutable);
        }
        lowered
    }

    /// The storage of `value`, by-address `read` operand `expression` that
    /// lowering returned with no place: the place its load or slot copy just
    /// read (a dereference, `p[]`, or a materialized constant's slot), or
    /// else, for a temporary the operand owns, a hidden slot it is
    /// materialized in ([`Self::materialize_in_slot`]) holding the
    /// operand's loans, as upstream passes the `!lit.ref` of an anonymous
    /// temporary, a function constant or closure among them. A value read
    /// through a reference stays a value.
    fn materialize_operand(&mut self, expression: Node<'a>, value: Reg) -> (Reg, Option<MirPlace>) {
        match self.f.body.instrs.last() {
            Some(MirInstr::LoadPlace { dest, place }) if *dest == value => {
                return (value, Some(place.clone()));
            }
            Some(MirInstr::UseVar {
                dest,
                var,
                mode: UseMode::Copy,
                ..
            }) if *dest == value => {
                let var = *var;
                if !self.f.binds_reference(var) {
                    let ty = self.var_types.get(&var).cloned();
                    return (value, Some(MirPlace::root(var, ty)));
                }
            }
            _ => {}
        }
        let Some(ty) = self
            .f
            .reg_types
            .get(&value.0)
            .cloned()
            .or_else(|| expression.ty())
        else {
            return (value, None);
        };
        let addressed = self.passes_address(value, expression);
        if addressed || matches!(ty.kind(), TyKind::Overload(_)) || !self.owned_temporary(value) {
            return (value, None);
        }
        let loans = self.aggregate_borrows(expression);
        let materialized = self.materialize_in_slot(expression.location(), value, ty);
        if let Some(place) = &materialized.1 {
            self.establish_storage_loans(expression, place.root, loans);
        }
        materialized
    }

    /// Whether the body defines `value` as a fresh value of its own: a call
    /// or getter result not returned by reference, a copy, a construction,
    /// a pointer formed from a place, or a constant. A slot's forward or move is not: its loans stay with
    /// the slot.
    fn owned_temporary(&self, value: Reg) -> bool {
        self.f
            .body
            .instrs
            .iter()
            .rev()
            .any(|instruction| match instruction {
                MirInstr::Call { dest, .. }
                | MirInstr::CallIndirect { dest, .. }
                | MirInstr::CopyValue { dest, .. }
                | MirInstr::MakeClosure { dest, .. }
                | MirInstr::Const { dest, .. }
                | MirInstr::ConstructType { dest, .. }
                | MirInstr::ConstructTypeParam { dest, .. }
                | MirInstr::MaterializeLiteral { dest, .. }
                | MirInstr::MakeStruct { dest, .. }
                | MirInstr::MakeTuple { dest, .. }
                | MirInstr::MakeArray { dest, .. }
                | MirInstr::MakeVariant { dest, .. } => *dest == value,
                MirInstr::MethodCall {
                    dest,
                    reference_result,
                    ..
                } => *dest == value && reference_result.is_none(),
                MirInstr::Index { dest, call, .. } => {
                    *dest == value
                        && call
                            .as_ref()
                            .is_some_and(|call| call.reference_result.is_none())
                }
                MirInstr::UseVar { dest, mode, .. } => *dest == value && *mode == UseMode::Copy,
                // `Pointer(to=x)` is a pointer value formed from a place,
                // not a reference to one.
                MirInstr::MakeRef { dest, .. } => {
                    *dest == value && self.f.reg_types.get(&value.0).is_some_and(Ty::is_pointer)
                }
                _ => false,
            })
    }

    /// Pass the operand `lowered`, a value read from its place, as the
    /// place's address instead: the instruction that read it becomes a
    /// `ref.make` of the place, or, where the read is not the operand's own
    /// load, a fresh `ref.make` follows it. The reference is as mutable as
    /// `wanted` and the place allow: a `read` parameter's storage, or a
    /// place reached through an immutable reference, lends an immutable
    /// one, as upstream's `ref` binds its operand's own mutability. A place
    /// holding a reference (a `ref` field, a `ref` binding's slot) passes
    /// the reference it holds instead ([`Self::forward_stored_handle`]).
    pub(super) fn address_operand(
        &mut self,
        expression: Node<'a>,
        lowered: (Reg, Option<MirPlace>),
        wanted: mojito_types::origin::Mutability,
    ) -> (Reg, Option<MirPlace>) {
        self.address_lowered_operand(expression.location(), lowered, wanted)
    }

    /// [`Self::address_operand`] of an operand lowering synthesized at
    /// `location`, which no checked node spells.
    pub(super) fn address_lowered_operand(
        &mut self,
        location: SourceSpan,
        lowered: (Reg, Option<MirPlace>),
        wanted: mojito_types::origin::Mutability,
    ) -> (Reg, Option<MirPlace>) {
        let (register, Some(place)) = &lowered else {
            return lowered;
        };
        let Some(value) = self.place_value_ty(place) else {
            return lowered;
        };
        let addressed = self
            .f
            .reg_types
            .get(&register.0)
            .is_some_and(|operand| addressed_operand(operand, value).is_some());
        if addressed {
            return lowered;
        }
        if self.place_holds_reference(place) {
            let (register, place) = (*register, place.clone());
            let address = self.forward_stored_handle(location, register, &place, wanted);
            return (address, Some(place));
        }
        let handle = self.place_handle_ty(place, wanted);
        let (register, place) = (*register, place.clone());
        if self.address_definition(register, &place) {
            self.f.reg_types.insert(register.0, handle);
            return (register, Some(place));
        }
        let address = self.place_address(location, &place, wanted);
        (address, Some(place))
    }

    /// Emit a `ref.make` of `place` for a callee that wants a reference as
    /// mutable as `wanted` ([`Self::place_handle_ty`]).
    pub(super) fn place_address(
        &mut self,
        location: SourceSpan,
        place: &MirPlace,
        wanted: mojito_types::origin::Mutability,
    ) -> Reg {
        let handle = self.place_handle_ty(place, wanted);
        self.emit_make_ref(location, place.clone(), handle)
    }

    /// The type of a reference to `place` as mutable as `wanted` and the
    /// place allow: a `read` parameter's storage, or a place reached
    /// through an immutable reference, lends an immutable one, and a place
    /// rooted at the frame's own bare `ref` parameter lends that
    /// parameter's inferred mutability, which a call binds, as upstream's
    /// `ref` binds its operand's own mutability.
    fn place_handle_ty(&self, place: &MirPlace, wanted: mojito_types::origin::Mutability) -> Ty {
        use mojito_types::origin::{Mutability, OriginParamId};
        let binds_root = self.f.binds_reference(place.root);
        let immutable_root = binds_root
            && matches!(
                place.root_ty.as_ref().map(Ty::kind),
                Some(TyKind::Ref(reference)) if reference.mutability == Mutability::Immutable
            );
        let mutability = if self.read_parameters.contains(&place.root)
            || immutable_root
            || wanted == Mutability::Immutable
        {
            Mutability::Immutable
        } else if self.reference_binders.contains(&place.root) {
            Mutability::Param(OriginParamId::reference_binder(place.root))
        } else {
            wanted
        };
        mir_place_handle_ty(place, binds_root, Some(mutability)).unwrap_or_else(Ty::error)
    }

    /// Pass the reference `place` holds, rather than the value read through
    /// it into `register`, as upstream passes a `ref` field's or a `ref`
    /// binding's `!lit.ref` as the operand: the read defining `register`
    /// becomes a `ref.make` of the stored handle ([`Self::stored_handle`]),
    /// or, where no read of it is found, a fresh one follows. The retained
    /// place is unchanged.
    fn forward_stored_handle(
        &mut self,
        location: SourceSpan,
        register: Reg,
        place: &MirPlace,
        wanted: mojito_types::origin::Mutability,
    ) -> Reg {
        let (handle_place, handle) = self.stored_handle(place, wanted);
        if self.forward_definition(register, place, &handle_place) {
            self.f.reg_types.insert(register.0, handle);
            return register;
        }
        self.emit_make_ref(location, handle_place, handle)
    }

    /// The place whose `ref.make` forwards the reference `place` holds,
    /// with that reference's type
    /// ([`crate::mir::call_addresses::stored_handle`]).
    fn stored_handle(
        &self,
        place: &MirPlace,
        wanted: mojito_types::origin::Mutability,
    ) -> (MirPlace, Ty) {
        crate::mir::call_addresses::stored_handle(
            place,
            self.f.binds_reference(place.root),
            |base, field| self.declares_ref_field(base, field),
            wanted,
        )
        .unwrap_or_else(|| (place.clone(), Ty::error()))
    }

    /// Emit a `ref.make` of `place` typed `handle`.
    fn emit_make_ref(&mut self, location: SourceSpan, place: MirPlace, handle: Ty) -> Reg {
        let address = self.fresh_typed(location, Some(place.root), handle);
        self.emit(MirInstr::MakeRef {
            dest: address,
            place,
        });
        address
    }

    /// Turn the read defining `register`, emitted for an operand of the
    /// call being lowered (in the body or deferred to an open call's
    /// setup), into a `ref.make` of `place` when it reads that storage
    /// whole: a `place.load` of it, or a slot use of a whole root.
    fn address_definition(&mut self, register: Reg, place: &MirPlace) -> bool {
        let found = self.operand_definition(|instruction| match instruction {
            MirInstr::LoadPlace { dest, place: read } => {
                *dest == register
                    && crate::mir::call_addresses::same_storage(read, place)
                    && read.through == place.through
            }
            MirInstr::UseVar { dest, var, .. } => {
                *dest == register
                    && place.is_whole_root()
                    && place.root == *var
                    && place.through.is_none()
            }
            _ => false,
        });
        let Some((instructions, index)) = found else {
            return false;
        };
        instructions[index] = MirInstr::MakeRef {
            dest: register,
            place: place.clone(),
        };
        true
    }

    /// Turn the read defining `register` through the reference `place`
    /// holds into a `ref.make` of `handle_place`, the stored handle: a
    /// `place.load` of that storage, or a runtime alias's `ref.read` through
    /// a `ref.make` of it, which is left unread.
    fn forward_definition(
        &mut self,
        register: Reg,
        place: &MirPlace,
        handle_place: &MirPlace,
    ) -> bool {
        let found = self.operand_definition(|instruction| match instruction {
            MirInstr::LoadPlace { dest, place: read } => {
                *dest == register
                    && crate::mir::call_addresses::same_storage(read, place)
                    && read.through == place.through
            }
            MirInstr::ReadRef { dest, .. } => *dest == register,
            _ => false,
        });
        let Some((instructions, index)) = found else {
            return false;
        };
        if let MirInstr::ReadRef { reference, .. } = instructions[index] {
            let reads_alias = instructions[..index].iter().rev().any(|instruction| {
                matches!(
                    instruction,
                    MirInstr::MakeRef { dest, place: read }
                        if *dest == reference
                            && crate::mir::call_addresses::same_storage(read, handle_place)
                )
            });
            if !reads_alias {
                return false;
            }
        }
        instructions[index] = MirInstr::MakeRef {
            dest: register,
            place: handle_place.clone(),
        };
        true
    }

    /// The instruction list and index of the read `defines` picks among
    /// those emitted for the operands of the call being lowered: the
    /// body's, latest first, then each open call setup's deferred reads
    /// and forwards.
    fn operand_definition(
        &mut self,
        defines: impl Fn(&MirInstr) -> bool,
    ) -> Option<(&mut Vec<MirInstr>, usize)> {
        let setups = self.call_setups.iter_mut().flat_map(|setup| {
            setup
                .reads
                .iter_mut()
                .chain(setup.forwards.iter_mut())
                .map(|(_, instructions)| instructions)
        });
        std::iter::once(&mut self.f.body.instrs)
            .chain(setups)
            .find_map(|instructions| {
                let index = instructions.iter().rposition(&defines)?;
                Some((instructions, index))
            })
    }

    /// The value [`Self::lower_call_operand`] passes, before a by-address
    /// operand becomes its place's address.
    fn lower_call_operand_value(
        &mut self,
        expression: Node<'a>,
        setup: Option<CheckedCallArgumentSource>,
    ) -> (Reg, Option<MirPlace>) {
        if let Some(operand) = expression.spread_operand() {
            return self.lower_pack_spread_argument(expression, operand);
        }
        let adjustments = expression.adjustments();
        // A `^` the checker bound to a read parameter lends its place like a
        // plain read argument.
        if let Some(inner) = expression.transfer_operand()
            && adjustments.iter().any(|adjustment| {
                matches!(
                    adjustment,
                    mojito_checked::checked::SemanticAdjustment::BorrowReadArgument
                )
            })
        {
            return self.lower_call_operand_value(inner, setup);
        }
        // A temporary bound to a `ref [origin]` parameter: store the value in
        // a hidden slot registered under the checker-minted owner identity and
        // hand the call that slot's place — the VM/native ref binding then
        // borrows real frame storage, and the slot's loans give the temporary
        // its borrower's lifetime.
        // An implicitly converted temporary (`f(String("abc"))` at a view
        // parameter) materializes inside the conversion lowering instead: the
        // slot holds the unconverted source and the callee receives the view.
        // An `Indexer` normalized to `Int` likewise passes its result.
        let converted = expression.implicit_conversion().is_some()
            || expression.index_normalization().is_some();
        // A register-passable `read` operand passes by value with no
        // storage, as upstream passes it (a compile-time value as its
        // constant): nothing borrows a slot for it.
        if let Some(owner) = mojito_checked::checked::materialized_borrow_owner(adjustments)
            && !converted
        {
            if self.register_passable_read_binding(expression) {
                return (self.expr(expression), None);
            }
            return self.materialize_borrow_source(expression, owner);
        }
        let retains_place = adjustments.iter().any(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::RetainCallPlace
            )
        });
        if !retains_place {
            // An implicit conversion supersedes the shallow-read shortcut:
            // the ordinary lowering emits the converting constructor (which
            // performs its own source borrow), and the callee receives the
            // constructed value rather than the raw source aggregate.
            if adjustments.iter().any(|adjustment| {
                matches!(
                    adjustment,
                    mojito_checked::checked::SemanticAdjustment::BorrowReadArgument
                )
            }) && !converted
                && let Some((register, reference_place)) = match setup {
                    Some(source) => self.defer_to_setup(expression, Some(source), true, |this| {
                        this.lower_borrowed_read_operand(expression)
                    }),
                    None => self.lower_borrowed_read_operand(expression),
                }
            {
                // A memory-only read is passed by address, as upstream
                // passes it, so its place is retained as a SHARED read: the
                // ownership analysis classifies a retained place by the
                // callee's declared convention (a non-`mut`/`ref` slot
                // reads), and the VM binds it as a caller-place handle. A
                // register-passable read is passed by value and retains no
                // place: a result's loans come from the origin its type
                // names, never from the argument's storage.
                if self.read_by_address_binding(expression) || !expression.register_passable_read()
                {
                    return (
                        register,
                        reference_place.or_else(|| self.simple_place(expression)),
                    );
                }
                return (register, None);
            }
            // A conversion its constructor returns through memory is a
            // temporary, not the place it reads.
            if let Some(source) = setup
                && !(converted && expression.indirect_result())
                && self.setup_read(expression)
            {
                let read = self
                    .defer_to_setup(expression, Some(source), true, |this| this.expr(expression));
                let place = self
                    .read_by_address_binding(expression)
                    .then(|| self.simple_place(expression))
                    .flatten();
                return (read, place);
            }
            // A by-address `read` of a place no setup frame defers (one
            // through a reference, or an operand of a call with no frame)
            // is the place's address where written.
            if !converted
                && self.read_by_address_binding(expression)
                && self.reads_value(expression)
                && let Some(place) = self.simple_place(expression)
            {
                return (self.expr(expression), Some(place));
            }
            // A by-address `read` of a field read through a reference (an
            // accessor's returned reference or a pointer's pointee) is the
            // projected place's address, as upstream's `lit.call` takes the
            // `!lit.ref` of `p[].items`.
            if !converted
                && self.read_by_address_binding(expression)
                && self.reads_value(expression)
                && let Some(place) = self.projected_read_place(expression)
            {
                let value = self.fresh_typed(
                    expression.location(),
                    Some(place.root),
                    place
                        .ty
                        .clone()
                        .or_else(|| expression.ty())
                        .unwrap_or_else(Ty::error),
                );
                self.emit(MirInstr::LoadPlace {
                    dest: value,
                    place: place.clone(),
                });
                return (value, Some(place));
            }
            // A reference result's read, and the temporary binding of its
            // copy, are setup work too.
            let read = |this: &mut Self| this.temporary_operand(expression, converted, false);
            return match setup {
                Some(source) => self.defer_to_setup(expression, Some(source), true, read),
                None => read(self),
            };
        }

        // A written-back subscript's getter runs at the call's setup, after
        // its sibling arguments; its operands are evaluated where written.
        if expression.subscript_write_back().is_some() {
            let lower = |this: &mut Self| this.lower_call_receiver(expression);
            return match setup {
                Some(source) => self.defer_to_setup(expression, Some(source), true, lower),
                None => lower(self),
            };
        }
        // A pure root/field place needs no emitted projection state, so keep
        // the existing expression lowering (notably its reference-field
        // handling) and attach the place afterward. A bare aggregate variable
        // is read shallowly instead: the VM rebinds a `mut`/`ref` parameter to
        // the caller place, so a `UseVar` lifecycle copy here would run a user
        // `__copyinit__` only to discard the result.
        if let Some(place) = self.simple_place(expression) {
            // A `ref` field's place binds the parameter to the field's
            // referent, which a load of the field reads through to; a
            // by-address operand forwards the stored handle in its stead
            // (`Self::forward_stored_handle`).
            if self.place_holds_reference(&place)
                && let Some(TyKind::Ref(reference)) = place.ty.as_ref().map(Ty::kind)
            {
                let value = self.fresh_typed(
                    expression.location(),
                    Some(place.root),
                    reference.referent.clone(),
                );
                self.emit(MirInstr::LoadPlace {
                    dest: value,
                    place: place.clone(),
                });
                return (value, Some(place));
            }
            if let Some(register) = self.lower_borrowed_read_argument(expression) {
                return (register, Some(place));
            }
            return (self.expr(expression), Some(place));
        }

        // A reference result (`String(span[0])`) is read at its call's setup,
        // as a receiver is, so its handle lends through setup loans rather
        // than an exclusive borrow of the view's referent.
        if expression.reference_result().is_some() {
            return match setup {
                Some(source) => self.defer_to_setup(expression, Some(source), true, |this| {
                    this.lower_call_receiver(expression)
                }),
                None => self.lower_call_receiver(expression),
            };
        }

        // `container[index].field` is not a raw place when the selected
        // `__getitem__` returns a reference. Evaluate that accessor once into
        // its hidden caller-handle slot, then retain the ordinary projections
        // below the returned referent. Falling through to `try_place` would
        // manufacture `container[Index].field` and bypass the selected call.
        if let Some(place) = self.lower_projected_reference_place(expression) {
            let value = self.fresh_typed(
                expression.location(),
                Some(place.root),
                place
                    .ty
                    .clone()
                    .or_else(|| expression.ty())
                    .unwrap_or_else(Ty::error),
            );
            self.emit(MirInstr::LoadPlace {
                dest: value,
                place: place.clone(),
            });
            return (value, Some(place));
        }

        if let Some(place) = self.try_place(expression) {
            let value = self.fresh_typed(
                expression.location(),
                Some(place.root),
                place
                    .ty
                    .clone()
                    .or_else(|| expression.ty())
                    .unwrap_or_else(Ty::error),
            );
            self.emit(MirInstr::LoadPlace {
                dest: value,
                place: place.clone(),
            });
            return (value, Some(place));
        }

        // A temporary at a place-retaining slot has no place to retain, so
        // it anchors like any other temporary argument. The checker
        // rejects a non-place actual for a place-requiring parameter; lowering
        // stays total so the verifier can diagnose corrupt checked input
        // without manufacturing a caller place.
        self.temporary_operand(expression, converted, true)
    }

    /// Evaluate an augmented-subscript argument before either accessor call,
    /// without applying a conversion selected for one particular accessor.
    /// Getter and setter contracts may adapt the same source expression to
    /// different parameter types, but Mojo still evaluates that expression
    /// exactly once.
    pub(super) fn expr_without_call_value_adjustments(&mut self, expression: Node<'a>) -> Reg {
        if let Some(reference) = expression.reference_result() {
            let handle = self.reference_handle(expression);
            let value_ty = reference.referent;
            let read = self.fresh_typed(expression.location(), None, value_ty.clone());
            self.emit(MirInstr::ReadRef {
                dest: read,
                reference: handle,
            });
            let copied = self.fresh_typed(expression.location(), None, value_ty);
            self.emit(MirInstr::CopyValue {
                dest: copied,
                value: read,
                constructor: None,
            });
            return copied;
        }

        let value = self.expr_unconverted(expression);
        if let Some(ty) = expression.ty() {
            self.f.reg_types.entry(value.0).or_insert(ty);
        }
        value
    }

    /// Lower one source operand shared by the getter and setter of an
    /// augmented subscript. `retain_place` is the union of both call contracts:
    /// it lets a mutating getter write back now and lets lowering reload that
    /// updated value before the setter, without re-evaluating the source.
    pub(super) fn lower_augmented_argument_source(
        &mut self,
        expression: Node<'a>,
        retain_place: bool,
    ) -> (Reg, Option<MirPlace>) {
        if !retain_place {
            return (self.expr_without_call_value_adjustments(expression), None);
        }
        if let Some(place) = self.simple_place(expression) {
            return (
                self.expr_without_call_value_adjustments(expression),
                Some(place),
            );
        }
        if expression.reference_result().is_some() {
            return self.lower_call_receiver(expression);
        }
        if let Some(place) = self.lower_projected_reference_place(expression) {
            let value = self.fresh_typed(
                expression.location(),
                Some(place.root),
                place
                    .ty
                    .clone()
                    .or_else(|| expression.ty())
                    .unwrap_or_else(Ty::error),
            );
            self.emit(MirInstr::LoadPlace {
                dest: value,
                place: place.clone(),
            });
            return (value, Some(place));
        }
        if let Some(place) = self.try_place(expression) {
            let value = self.fresh_typed(
                expression.location(),
                Some(place.root),
                place
                    .ty
                    .clone()
                    .or_else(|| expression.ty())
                    .unwrap_or_else(Ty::error),
            );
            self.emit(MirInstr::LoadPlace {
                dest: value,
                place: place.clone(),
            });
            return (value, Some(place));
        }
        (self.expr_without_call_value_adjustments(expression), None)
    }

    pub(super) fn checked_call_source_requires_place(
        contract: &mojito_checked::checked::CheckedCallContract,
        source: mojito_checked::checked::CheckedCallArgumentSource,
    ) -> bool {
        contract
            .arguments
            .iter()
            .any(|argument| argument.source == source && argument.requires_place)
    }

    pub(super) fn checked_call_source_mutates(
        contract: &mojito_checked::checked::CheckedCallContract,
        source: mojito_checked::checked::CheckedCallArgumentSource,
    ) -> bool {
        contract.arguments.iter().any(|argument| {
            argument.source == source
                && matches!(
                    argument.convention,
                    Some(
                        mojito_ast::ast::ArgConvention::Mut
                            | mojito_ast::ast::ArgConvention::Ref
                            | mojito_ast::ast::ArgConvention::Out
                    )
                )
        })
    }

    /// Apply the adaptations frozen on one selected call to an already
    /// evaluated source register. This deliberately ignores the expression's
    /// compatibility adjustment table: getter and setter facts can share the
    /// same source span and must not overwrite each other.
    pub(super) fn apply_checked_call_value_adjustments(
        &mut self,
        contract: &mojito_checked::checked::CheckedCallContract,
        source: mojito_checked::checked::CheckedCallArgumentSource,
        raw: Reg,
        site: SourceSpan,
    ) -> Reg {
        let parameter_ty = contract
            .arguments
            .iter()
            .find(|argument| argument.source == source)
            .map(|argument| argument.parameter_ty.clone());
        let adjustments = contract
            .boundary
            .arguments
            .iter()
            .find(|argument| argument.source == source)
            .map(|argument| argument.adjustments.as_slice())
            .unwrap_or_default();
        self.apply_call_value_adjustments(adjustments, parameter_ty, raw, site)
    }

    /// Apply `adjustments`, adaptations the checker froze for one call, to
    /// `raw`, an already evaluated source register; a converting
    /// constructor's result is `parameter_ty`, which an unadapted value
    /// materializes to.
    pub(super) fn apply_call_value_adjustments(
        &mut self,
        adjustments: &[mojito_checked::checked::CheckedCallValueAdjustment],
        parameter_ty: Option<Ty>,
        raw: Reg,
        site: SourceSpan,
    ) -> Reg {
        let mut value = raw;
        for adjustment in adjustments {
            value = match adjustment {
                mojito_checked::checked::CheckedCallValueAdjustment::ResolveCallable { target } => {
                    let dest = self.fresh_typed(
                        site.clone(),
                        None,
                        parameter_ty.clone().unwrap_or_else(Ty::error),
                    );
                    self.emit(MirInstr::Const {
                        dest,
                        k: Const::Function(target.clone()),
                    });
                    dest
                }
                mojito_checked::checked::CheckedCallValueAdjustment::ImplicitConversion {
                    target,
                    instantiated_args,
                    ..
                } => {
                    let dest = self.fresh_typed(
                        site.clone(),
                        None,
                        parameter_ty.clone().unwrap_or_else(Ty::error),
                    );
                    self.emit(MirInstr::Call {
                        conventions: crate::mir::MirArgConventions::default(),
                        dest,
                        func: FuncRef::named(target),
                        raises: None,
                        args: vec![value],
                        kwargs: Vec::new(),
                        capture_accesses: Vec::new(),
                        param_arg_regs: Vec::new(),
                        receiver: None,
                        instantiated_args: instantiated_args.clone(),
                        spread: None,
                    });
                    dest
                }
                // Upstream's `index(idx)`: `Int(mlir_value=idx.__mlir_index__())`.
                mojito_checked::checked::CheckedCallValueAdjustment::IndexNormalization {
                    target,
                } => {
                    let dest = self.fresh_typed(site.clone(), None, Ty::index());
                    self.emit(MirInstr::MethodCall {
                        conventions: crate::mir::MirArgConventions::default(),
                        dest,
                        recv: value,
                        method: "__mlir_index__".to_string(),
                        resolved: Some(target.clone()),
                        raises: None,
                        reference_result: None,
                        result_adapter: None,
                        args: Vec::new(),
                        kwargs: Vec::new(),
                        recv_writes: false,
                        receiver_convention: None,
                        capture_accesses: Vec::new(),
                        param_arg_regs: Vec::new(),
                        param_decls: Vec::new(),
                        instantiated_args: Vec::new(),
                        spread: None,
                        refined_bound: None,
                    });
                    self.int_from_index(dest, site.clone())
                }
                mojito_checked::checked::CheckedCallValueAdjustment::MaterializeLiteral {
                    target,
                } => self.materialize_literal(value, target, site.clone()),
                mojito_checked::checked::CheckedCallValueAdjustment::MaterializeSource(
                    materialization,
                ) => {
                    self.materialize_conversion_source(value, materialization, site.clone())
                        .0
                }
            };
        }
        if adjustments.is_empty()
            && let Some(parameter_ty) = parameter_ty
        {
            value = self.materialize_register(value, &parameter_ty, site);
        }
        value
    }

    /// `value`, a `@__nonmaterializable` source, materialized to its
    /// target through that target's `@implicit` constructor, the first
    /// step of a conversion through a constructor of the target. A
    /// memory-only target the converting constructor reads is bound to a
    /// call-result slot, whose place the read is passed with, as upstream
    /// reads a `__call_result_tmp__`.
    pub(super) fn materialize_conversion_source(
        &mut self,
        value: Reg,
        materialization: &mojito_checked::checked::ConversionMaterialization,
        site: SourceSpan,
    ) -> (Reg, Option<MirPlace>) {
        let ty = materialization.ty.clone();
        let dest = self.fresh_typed(site.clone(), None, ty.clone());
        self.emit(MirInstr::Call {
            conventions: crate::mir::MirArgConventions::default(),
            dest,
            func: FuncRef::named(&materialization.constructor),
            raises: None,
            args: vec![value],
            kwargs: Vec::new(),
            capture_accesses: Vec::new(),
            param_arg_regs: Vec::new(),
            receiver: None,
            instantiated_args: materialization.instantiated_args.clone(),
            spread: None,
        });
        if !materialization.reads || !owns_droppable_storage(&ty) {
            return (dest, None);
        }
        let slot = self.call_result_slot(dest, ty.clone(), true);
        let place = MirPlace::root(slot, Some(ty.clone()));
        let read = self.fresh_typed(site, Some(slot), ty);
        self.emit(MirInstr::LoadPlace {
            dest: read,
            place: place.clone(),
        });
        (read, Some(place))
    }

    pub(super) fn emit_checked_call_boundary(
        &mut self,
        contract: &mojito_checked::checked::CheckedCallContract,
        site: &SourceSpan,
    ) {
        for argument in &contract.boundary.arguments {
            self.emit_interior_invalidation_facts(
                &argument.invalidations,
                &argument.value_source.clone(),
                None,
            );
        }
        self.emit_interior_invalidation_facts(&contract.boundary.invalidations, site, None);
    }

    #[allow(clippy::ref_option, reason = "TODO: take Option<&T>")]
    pub(super) fn reload_augmented_source(
        &mut self,
        raw: Reg,
        place: &Option<MirPlace>,
        mutated: bool,
        site: SourceSpan,
    ) -> Reg {
        if !mutated {
            return raw;
        }
        let Some(place) = place else {
            return raw;
        };
        let value = self.fresh_typed(
            site,
            Some(place.root),
            place.ty.clone().unwrap_or_else(Ty::error),
        );
        self.emit(MirInstr::LoadPlace {
            dest: value,
            place: place.clone(),
        });
        value
    }

    /// A whole pack spread into a callee's collector (`show(*args)`,
    /// `show(*args^)`): the caller's collector, loaded as its value, as
    /// upstream passes a register-passable `read` (its elements borrowed
    /// through the pack type's origin), or moved out of its slot when
    /// transferred.
    /// The call records the position (`MirInstr::Call::spread`) and the
    /// elaborator expands it into the bound pack's element places. Any other
    /// operand is the unexpanded spread the expression lowering rejects.
    fn lower_pack_spread_argument(
        &mut self,
        expression: Node<'a>,
        operand: Node<'a>,
    ) -> (Reg, Option<MirPlace>) {
        if let Some(moved) = operand.transfer_operand()
            && moved.is_name()
        {
            return (self.transfer(operand, moved), None);
        }
        let Some(place) = self.simple_place(operand).filter(|place| {
            place.ty.as_ref().is_some_and(|ty| {
                matches!(ty.kind(), TyKind::VariadicPack(_))
                    || mojito_types::types::variadic_pack_owned(ty.kind()).is_some()
            })
        }) else {
            return (self.expr(expression), None);
        };
        let value = self.fresh_typed(
            operand.location(),
            Some(place.root),
            place.ty.clone().unwrap_or_else(Ty::error),
        );
        self.emit(MirInstr::LoadPlace { dest: value, place });
        (value, None)
    }

    pub(super) fn lower_call_arguments(
        &mut self,
        arguments: &[Node<'a>],
    ) -> (Vec<Reg>, Vec<Option<MirPlace>>) {
        let mut registers = Vec::with_capacity(arguments.len());
        let mut places = Vec::with_capacity(arguments.len());
        for (index, &argument) in arguments.iter().enumerate() {
            let (register, place) = self
                .lower_call_argument_with(argument, CheckedCallArgumentSource::Positional(index));
            registers.push(register);
            places.push(place);
        }
        (registers, places)
    }

    pub(super) fn lower_call_keywords(
        &mut self,
        arguments: &[(&str, Node<'a>)],
    ) -> LoweredKeywords {
        let mut registers = Vec::with_capacity(arguments.len());
        let mut places = Vec::with_capacity(arguments.len());
        for (index, &(name, value)) in arguments.iter().enumerate() {
            let (register, place) =
                self.lower_call_argument_with(value, CheckedCallArgumentSource::Keyword(index));
            registers.push((name.to_string(), register));
            places.push(place);
        }
        (registers, places)
    }

    /// The keyword of a copy construction (`Name(copy=place)`,
    /// `T(copy=place)`), which binds the copy initializer's borrowed
    /// `copy: Self` parameter: a place source is read once and passed as
    /// the check bound it, as its address when memory-only and otherwise as
    /// the value, naming no storage, so the copy initializer runs on the
    /// live source exactly once, where an
    /// ordinary value read would run a user's copy initializer a second time
    /// for the argument itself — observable through its side effects. `None`
    /// for any other keyword list, a source that is not a simple place, or a
    /// source converted to `Self`, which is a value rather than the place.
    pub(super) fn copy_construction_keywords(
        &mut self,
        arguments: &[(&str, Node<'a>)],
    ) -> Option<LoweredKeywords> {
        let [("copy", value)] = arguments else {
            return None;
        };
        if value.implicit_conversion().is_some() {
            return None;
        }
        let place = self.simple_place(*value)?;
        let source = self.fresh_typed(
            value.location(),
            Some(place.root),
            place
                .ty
                .clone()
                .or_else(|| value.ty())
                .unwrap_or_else(Ty::error),
        );
        self.emit(MirInstr::LoadPlace {
            dest: source,
            place: place.clone(),
        });
        let (source, place) =
            if self.operand_binding(*value) == Some(OperandBinding::Read { by_address: true }) {
                self.address_operand(
                    *value,
                    (source, Some(place)),
                    mojito_types::origin::Mutability::Immutable,
                )
            } else {
                (source, None)
            };
        Some((vec![("copy".to_string(), source)], vec![place]))
    }

    /// Fold a keyword `@fieldwise_init` construction into its positional form:
    /// the arguments were evaluated in source order, and the call lists their
    /// registers in field order, so no backend binds keywords to fields;
    /// `conventions`, the call's checked record, is renumbered to the
    /// operands' new positions. A call on any other target is returned
    /// unchanged.
    pub(super) fn fieldwise_keywords_positional(
        &self,
        constructed: &str,
        call: FieldwiseCallArguments,
        conventions: crate::mir::MirArgConventions,
    ) -> (FieldwiseCallArguments, crate::mir::MirArgConventions) {
        let (regs, places, keywords, keyword_places) = call;
        if keywords.is_empty() {
            return ((regs, places, keywords, keyword_places), conventions);
        }
        let keyword_names: Vec<&str> = keywords.iter().map(|(name, _)| name.as_str()).collect();
        let Some(slots) = self
            .overloads
            .fieldwise_parameters(constructed)
            .and_then(|names| {
                mojito_ast::call::match_fieldwise_slots(&names, regs.len(), &keyword_names).ok()
            })
        else {
            return ((regs, places, keywords, keyword_places), conventions);
        };
        let conventions = conventions.remap(|source| {
            use mojito_ast::call::ArgSlot;
            slots.iter().position(|slot| match (slot, source) {
                (ArgSlot::Positional(index), CheckedCallArgumentSource::Positional(at))
                | (ArgSlot::Keyword(index), CheckedCallArgumentSource::Keyword(at)) => *index == at,
                _ => false,
            })
        });
        let (ordered_regs, ordered_places) = slots
            .into_iter()
            .filter_map(|slot| match slot {
                mojito_ast::call::ArgSlot::Positional(index) => {
                    Some((regs[index], places[index].clone()))
                }
                mojito_ast::call::ArgSlot::Keyword(index) => {
                    Some((keywords[index].1, keyword_places[index].clone()))
                }
                mojito_ast::call::ArgSlot::Default => None,
            })
            .unzip();
        (
            (ordered_regs, ordered_places, Vec::new(), Vec::new()),
            conventions,
        )
    }

    /// Anchor `src`, a value of type `ty` a lifted compile-time evaluation
    /// borrows, in compile-time memory, and make `var` a runtime alias of the
    /// reference to it: upstream's `store_to_mem`, whose memory is never
    /// destroyed, so a borrow of it outlives the evaluation's result (a
    /// `d.keys()` iterator). A binding is borrowed immutably (upstream's
    /// `PMBValue`), a temporary mutably (`PMRValue`). The returned place
    /// reads through `var`.
    pub(super) fn bind_comptime_memory(
        &mut self,
        location: SourceSpan,
        var: VarId,
        src: Reg,
        ty: Ty,
        mutability: mojito_types::origin::Mutability,
    ) -> MirPlace {
        let reference = Ty::new(TyKind::Ref(mojito_types::origin::RefTy {
            referent: ty.clone(),
            origin: mojito_types::origin::Origin::Static,
            mutability,
            address_space: mojito_types::types::AddressSpaceSlot::GENERIC,
        }));
        let handle = self.fresh_typed(location, Some(var), reference.clone());
        self.emit(MirInstr::StoreToMem { dest: handle, src });
        self.var_types.insert(var, reference.clone());
        self.bind_reference(var);
        self.emit(MirInstr::DefVar {
            var,
            src: handle,
            binding_ty: Some(reference.clone()),
            in_place: false,
        });
        let mut place = MirPlace::root(var, Some(reference));
        place.ty = Some(ty);
        place.through = Some(var);
        place
    }

    /// Store an accessor-produced reference in a hidden local and establish its
    /// checked owner loans.  This turns the handle into the same persistent,
    /// analyzable call-place representation as an explicit `ref` binding while
    /// evaluating the accessor exactly once. A checked owner with no MIR slot
    /// of its own lends through `fallback`, the storage the accessor reads.
    /// A reference the caller writes through (`written`) lends mutably unless
    /// its mutability is `Immutable`: a parametric one (`Origin[mut=m]`) is
    /// the instance's to judge, as the checker left it.
    pub(super) fn materialize_call_reference_place(
        &mut self,
        expression: Node<'a>,
        handle: Reg,
        reference: &mojito_types::origin::RefTy,
        fallback: Option<VarId>,
        (setup, written): (bool, bool),
    ) -> MirPlace {
        let variable = self.var(&format!("$call_ref_r{}", handle.0));
        let storage_ty = Ty::new(TyKind::Ref(reference.clone()));
        self.var_types.insert(variable, storage_ty.clone());
        self.bind_reference(variable);
        self.emit(MirInstr::DefVar {
            var: variable,
            src: handle,
            binding_ty: Some(storage_ty.clone()),
            in_place: false,
        });

        // The materialized reference's own origin names the storage it
        // designates even when the expression's checked type is the call's
        // result rather than the reference.
        let mut origins = expression.reference_places();
        let own = match &reference.origin {
            mojito_types::origin::Origin::Place(place) => vec![place],
            mojito_types::origin::Origin::Union(members) => members
                .iter()
                .filter_map(|member| match member {
                    mojito_types::origin::Origin::Place(place) => Some(place),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        for place in own {
            if !origins.contains(place) {
                origins.push(place.clone());
            }
        }
        // A pointer's pointee (`p[]`, `p.__getitem__()`) is a shared pointer
        // alias of the storage, as the pointer's own loan is.
        let shared = expression
            .object()
            .and_then(Node::ty)
            .is_some_and(|ty| ty.is_pointer());
        let mut loans = Vec::new();
        for origin in origins {
            let Some(canonical) = self.mir_interior_origin(&origin, fallback) else {
                continue;
            };
            let interior = canonical
                .path
                .iter()
                .any(|segment| matches!(segment, mojito_types::origin::OriginSeg::Interior(_)))
                .then_some(canonical.clone());
            loans.push(MirLoan {
                place: MirPlace::root(canonical.root, self.var_types.get(&canonical.root).cloned()),
                mutable: match reference.mutability {
                    mojito_types::origin::Mutability::Mutable => true,
                    mojito_types::origin::Mutability::Param(_) => written,
                    mojito_types::origin::Mutability::Immutable => false,
                },
                interior,
                shared,
                setup,
            });
        }
        let receiver = self
            .place_chain(expression)
            .into_iter()
            .find_map(|node| self.simple_place(node));
        // A referent a view's accessor reaches (`span[0]`) carries the view's
        // own origin: like a `ref` binding, it names that origin rather than
        // borrowing the owner again beside the view's own loan.
        if let Some(receiver) = &receiver
            && let Some(view_loans) = self.aggregate_loans.get(&receiver.root)
        {
            for loan in &mut loans {
                let mut same_root = view_loans
                    .iter()
                    .filter(|view_loan| view_loan.place.root == loan.place.root);
                if let Some(view_loan) = same_root.next() {
                    loan.setup = true;
                    // The view's own loan names the storage exactly (a
                    // pointer to `self.a` lends `self.a`, not `self`).
                    if same_root.next().is_none()
                        && view_loan.place.through.is_none()
                        && !view_loan.place.proj.is_empty()
                    {
                        loan.place = view_loan.place.clone();
                    }
                }
            }
        }
        // A setup read also keeps the accessor chain's root receiver live:
        // a view's referent (`Span.__getitem__`) is reached through it.
        if setup
            && let Some(receiver) = receiver
            && !loans.iter().any(|loan| loan.place.root == receiver.root)
        {
            loans.push(MirLoan {
                place: MirPlace::root(receiver.root, self.var_types.get(&receiver.root).cloned()),
                mutable: false,
                interior: None,
                shared: false,
                setup,
            });
        }
        if !loans.is_empty() {
            let marker =
                self.fresh_typed(expression.location(), Some(loans[0].place.root), Ty::none());
            self.emit(MirInstr::EstablishLoans {
                reference: variable,
                loans: loans.clone(),
                marker,
                dest_interior: None,
            });
            self.aggregate_loans.insert(variable, loans);
        }

        let mut place = MirPlace::root(variable, Some(storage_ty));
        place.ty = Some(reference.referent.clone());
        place.through = Some(variable);
        place
    }

    /// Materialize one checker-selected reference-returning expression as a
    /// stable hidden caller place without reading its referent. Projection
    /// chains reuse this handle, so the selected accessor is evaluated exactly
    /// once and the VM never has to reinterpret a nominal index as raw storage.
    pub(super) fn materialize_reference_result_place(
        &mut self,
        expression: Node<'a>,
    ) -> Option<MirPlace> {
        let reference = expression.reference_result()?;
        let handle = self.reference_handle(expression);
        // `reference_handle` may peel an outer reference-valued aggregate layer.
        // Materialize the handle's actual type rather than recreating `ref ref T`.
        let materialized_reference = match self.f.reg_types.get(&handle.0).map(Ty::kind) {
            Some(TyKind::Ref(reference)) => reference.clone(),
            _ => reference,
        };
        // An operand read at its call's setup lends through setup loans, and
        // its read begins after them.
        let setup = self.awaits_setup_read(expression);
        let written = expression.borrow_mutability() == Some(true);
        let place = self.materialize_call_reference_place(
            expression,
            handle,
            &materialized_reference,
            None,
            (setup, written),
        );
        if setup && let Some(pending) = self.setup_reference.as_mut() {
            pending.read_start = Some(self.f.body.instrs.len());
        }
        Some(place)
    }

    /// Lower ordinary field/intrinsic-index projections whose base is produced
    /// by a reference-returning call. Nominal index steps keep their own checked
    /// call path and are deliberately not generalized into raw projections.
    pub(super) fn lower_projected_reference_place(
        &mut self,
        expression: Node<'a>,
    ) -> Option<MirPlace> {
        if let Some(place) = self.materialize_write_back_place(expression) {
            return Some(place);
        }
        if let Some(step) = expression.reflected_field() {
            return Some(self.reflected_field_place(expression, step));
        }
        let base_place = |this: &mut Self, base: Node<'a>| {
            if base.reference_result().is_some() {
                this.materialize_reference_result_place(base)
            } else if let Some(place) = this.lower_projected_reference_place(base) {
                Some(place)
            } else {
                this.materialized_temporary_base_place(base)
            }
        };
        match (expression.op(), expression.object()) {
            (CheckedOp::Member { field, .. }, Some(object)) => {
                let mut place = base_place(self, object)?;
                expression.project_member(&mut place, field);
                Some(place)
            }
            (CheckedOp::Index { .. }, Some(object))
                if let Some(index) = expression.index()
                    && expression.call_contract().is_none()
                    && matches!(
                        object.intrinsic_index_dispatch(),
                        Some(
                            MirIntrinsicSubscript::TupleStorage
                                | MirIntrinsicSubscript::VariadicStorage
                        )
                    ) =>
            {
                let mut place = base_place(self, object)?;
                let projection = match object.ty().as_ref().map(Ty::kind) {
                    Some(TyKind::Tuple(_)) => index
                        .exact_nonnegative_index()
                        .map_or_else(|| Proj::Index(self.index_operand(index)), Proj::ConstIndex),
                    _ => Proj::Index(self.index_operand(index)),
                };
                if let Some(ty) = expression.place_ty().or_else(|| expression.ty()) {
                    place.project(projection, ty);
                } else {
                    place.proj.push(projection);
                }
                Some(place)
            }
            _ => None,
        }
    }

    /// The place a `reflect[T].field_ref[idx](s)` reference designates: the
    /// argument's place, a temporary's in its hidden slot, plus the field
    /// step upstream's `lit.ref.struct.ger` takes.
    fn reflected_field_place(
        &mut self,
        expression: Node<'a>,
        step: &mojito_checked::checked::ReflectedFieldStep,
    ) -> MirPlace {
        let argument = *expression
            .args()
            .first()
            .expect("a checked field_ref call has its one argument");
        let mut place =
            match mojito_checked::checked::materialized_borrow_owner(argument.adjustments()) {
                Some(owner) if !self.owner_vars.contains_key(&owner) => self
                    .materialize_borrow_source(argument, owner)
                    .1
                    .unwrap_or_else(|| self.place(argument)),
                _ => self.place(argument),
            };
        let projection = match step {
            mojito_checked::checked::ReflectedFieldStep::Named(name) => Proj::Field(name.clone()),
            mojito_checked::checked::ReflectedFieldStep::At(index) => Proj::FieldAt(index.clone()),
        };
        let ty = expression
            .ty()
            .expect("a checked field_ref call has its field's type");
        place.project(projection, ty);
        place
    }

    /// Evaluate a call receiver and retain its executable place when checking
    /// selected reference/write-back semantics. Accessor-produced references
    /// become hidden reference locals; value-returning accessors remain values
    /// and are never reconstructed as raw index projections.
    pub(super) fn lower_call_receiver(&mut self, expression: Node<'a>) -> (Reg, Option<MirPlace>) {
        // A `^` receiver of a read method lends its place (checker-marked).
        if let Some(inner) = expression.transfer_operand()
            && expression.adjustments().iter().any(|adjustment| {
                matches!(
                    adjustment,
                    mojito_checked::checked::SemanticAdjustment::BorrowReadArgument
                )
            })
        {
            return self.lower_call_receiver(inner);
        }
        // A temporary receiver the checker materialized (the result borrows
        // it: `ref[self]`) lives in its hidden slot for the statement.
        if let Some(owner) =
            mojito_checked::checked::materialized_borrow_owner(expression.adjustments())
        {
            return self.materialize_borrow_source(expression, owner);
        }
        if let Some(place) = self.materialize_reference_result_place(expression) {
            let value = self.fresh_typed(
                expression.location(),
                Some(place.root),
                place.ty.clone().unwrap_or_else(Ty::error),
            );
            self.emit(MirInstr::LoadPlace {
                dest: value,
                place: place.clone(),
            });
            return (value, Some(place));
        }
        if let Some(place) = self.lower_projected_reference_place(expression) {
            let value = self.fresh_typed(
                expression.location(),
                Some(place.root),
                place
                    .ty
                    .clone()
                    .or_else(|| expression.ty())
                    .unwrap_or_else(Ty::error),
            );
            self.emit(MirInstr::LoadPlace {
                dest: value,
                place: place.clone(),
            });
            return (value, Some(place));
        }
        if expression.call_contract().is_some() {
            let value = self.expr(expression);
            // A borrowing view produced by the receiver call (BorrowViewResult
            // carries loans on the ultimate owner) must outlive the chained
            // call: bind it into a hidden retained slot whose loans keep the
            // source alive and conflict-checked, exactly as the implicit
            // view-conversion binding does. Loan-free call results stay plain
            // temporaries.
            let loans = self.aggregate_borrows(expression);
            if loans.is_empty() {
                return (value, None);
            }
            return self.anchor_borrowing_temporary(expression, value, loans, "$view_recv_r");
        }
        // A constructed view temporary (`StringSpan(text).split(",")`) borrows
        // caller storage through its `ref [origin]` argument; the method body
        // reads through the view, so the source must outlive the call even at
        // its last use — the same hidden anchor a chained view result gets.
        if matches!(expression.op(), CheckedOp::Call { .. })
            && expression
                .ty()
                .as_ref()
                .is_some_and(|ty| ty.kind().is_non_simd_struct())
        {
            let value = self.expr(expression);
            let loans = self.aggregate_borrows(expression);
            if loans.is_empty() {
                return (value, None);
            }
            return self.anchor_borrowing_temporary(expression, value, loans, "$view_recv_r");
        }
        // A value binder of the enclosing declaration (`dt.is_integral()` in
        // a `DType`-keyed body), an enclosing `comptime for` variable under
        // any shadow spelling, a module constant, in a lifted thunk a local
        // `comptime` binding of a parameter expression (`lane` of `comptime
        // lane = dt`), or a slotless binding every read materializes
        // (`comptime n: Int = w` over a `SIMDLength`), is a value the
        // elaborator folds, not a place: the receiver is read as such and
        // no place is retained.
        if let Some(name) = self.name_spelled(expression)
            && (expression.constant_param_value()
                || self
                    .enclosing_binders
                    .values
                    .iter()
                    .any(|(binder, _)| binder.name.as_ref() == name.as_str())
                || expression.owner().is_some_and(|owner| {
                    self.enclosing_binders
                        .loop_bindings
                        .iter()
                        .any(|(binding, _)| *binding == owner)
                        || self.enclosing_binders.comptime_value(owner).is_some()
                        || self.comptime_thunks.bound_value(owner).is_some()
                }))
        {
            return (self.expr(expression), None);
        }
        // A literal receiver materialized to its default type (`K * n` over
        // `comptime K = 8`, `Int`'s `__mul__`) is the converted value, not
        // the literal's place.
        if expression.literal_materialization().is_some() {
            return (self.expr(expression), None);
        }
        match self.try_place(expression) {
            Some(place) => {
                let value = self.fresh(expression.location(), Some(place.root));
                self.emit(MirInstr::LoadPlace {
                    dest: value,
                    place: place.clone(),
                });
                (value, Some(place))
            }
            None => (self.expr(expression), None),
        }
    }

    /// [`Self::lower_call_receiver`] for the receiver of method call `call`.
    /// A temporary receiver returned through memory is used in its
    /// call-result slot: a `var` receiver takes it, a `deinit` one reads it
    /// with the slot's place, which [`Self::consume_receiver`] passes as
    /// the storage the call consumes, a declared
    /// `read` receiver reads it, with the slot's place when it is
    /// memory-only, whose address [`Self::lower_receiver_value`]
    /// passes as upstream's `lit.call` takes a `!lit.ref` of the rvalue it
    /// materialized, and any other (a `ref self`) reads it and
    /// binds the slot's place, exactly as a named
    /// receiver retains its own; such a receiver returned in registers is
    /// materialized ([`Self::materialize_receiver`]). A slot's last use is
    /// the call, so drop elaboration destroys the temporary right after it
    /// returns — upstream's ASAP destruction. A receiver the receiver paths
    /// already gave a place is returned unchanged.
    pub(super) fn lower_method_receiver(
        &mut self,
        call: Node<'a>,
        receiver: Node<'a>,
    ) -> (Reg, Option<MirPlace>) {
        let binding = call
            .call_contract()
            .and_then(|contract| OperandBinding::of_receiver(&contract));
        self.bind_operand(receiver, binding);
        let owned = binding == Some(OperandBinding::Owned);
        let deinit = call.call_contract().is_some_and(|contract| {
            contract.receiver_convention == Some(mojito_ast::ast::ArgConvention::Deinit)
        });
        // `^` over a temporary transfers nothing: the owned receiver is the
        // temporary itself (`pk(x^)^.release()`).
        if owned
            && let Some(inner) = receiver.transfer_operand()
            && inner.is_fresh_temporary()
        {
            return self.lower_method_receiver(call, inner);
        }
        // A `deinit` receiver transferred out of a named slot (`x^.f()`)
        // is that slot, whose address the call consumes.
        if deinit
            && let Some(inner) = receiver.transfer_operand()
            && matches!(inner.checked.op, CheckedOp::Identifier(_))
        {
            return self.lower_call_receiver(inner);
        }
        // A conditional expression is the result slot its arms move into,
        // and a capturing lambda its storage's slot, which a reading
        // receiver reads in place and an owning one takes.
        let lowered = if matches!(
            receiver.op(),
            CheckedOp::IfExpr { .. } | CheckedOp::Lambda { .. }
        ) {
            self.operand_expr_c(receiver)
        } else {
            let (value, place) = self.lower_call_receiver(receiver);
            if place.is_some() {
                return (value, place);
            }
            self.call_result(receiver, value)
        };
        let read =
            matches!(binding, Some(OperandBinding::Read { .. })) && receiver.binds_temporary();
        match lowered {
            CValue::Value(value) if owned || read || !receiver.is_fresh_temporary() => {
                (value, None)
            }
            CValue::Value(value) => self.materialize_receiver(receiver, value),
            slot @ CValue::Slot(_) if owned && !deinit => (self.take_c(receiver, slot), None),
            slot @ CValue::Slot(_) if read => {
                let (value, place) = self.read_c(receiver, slot);
                (value, place.filter(|_| receiver.indirect_result()))
            }
            slot @ CValue::Slot(_) => self.read_c(receiver, slot),
        }
    }

    /// Method call `call`'s receiver value and retained place, copying a
    /// consuming receiver when `copied`; a subscript's accessor and an
    /// `in`'s `__contains__` lower their receiver here too, so a
    /// register-passable `read self` one is passed by value as any method
    /// call's is. A copied named receiver is read
    /// as an owned argument is, by its slot's copying use, as upstream
    /// copies argument 0 from the name's memory. A copied receiver's place
    /// is its copy's call-result slot, which the call consumes
    /// ([`Self::copy_consuming_receiver`]), never the copied original's.
    pub(super) fn lower_receiver_value(
        &mut self,
        call: Node<'a>,
        receiver: Node<'a>,
        copied: bool,
    ) -> (Reg, Option<MirPlace>) {
        let deinit = call.call_contract().is_some_and(|contract| {
            contract.receiver_convention == Some(mojito_ast::ast::ArgConvention::Deinit)
        });
        if copied
            && matches!(receiver.checked.op, CheckedOp::Identifier(_))
            && !receiver.is_fresh_temporary()
        {
            let value = self.expr(receiver);
            // A copying slot read, or a compile-time parameter's value,
            // is already a fresh owned value the call may consume.
            let read_copied = matches!(
                self.f.body.instrs.last(),
                Some(
                    MirInstr::UseVar { dest, mode: UseMode::Copy, .. }
                        | MirInstr::Const { dest, .. }
                ) if *dest == value
            );
            if read_copied {
                let ty = self.f.reg_types.get(&value.0).cloned();
                return match ty {
                    Some(ty) if ty.kind().is_non_simd_struct() => {
                        self.consumed_copy(receiver, value, ty, deinit)
                    }
                    _ => (value, None),
                };
            }
            return self.copy_consuming_receiver(receiver, value, None, deinit);
        }
        let (value, place) = self.lower_method_receiver(call, receiver);
        if copied {
            return self.copy_consuming_receiver(receiver, value, place.as_ref(), deinit);
        }
        if deinit {
            return self.consume_receiver(
                receiver.location(),
                (value, place),
                receiver.register_passable_consume(),
            );
        }
        // A register-passable receiver the method reads is passed by
        // value, as upstream passes a `read self` whose type is not
        // memory-only, a contract-less builtin's included: it retains no
        // place, so its owner may die at the
        // read, before the call runs, even when the call's result borrows
        // it — a result's loans come from the origin its type names, as for
        // an argument. The register then carries the receiver's type, which
        // the call's binding reads its struct's binders from.
        if place.is_some()
            && call.call_contract().is_none_or(|contract| {
                matches!(
                    contract.receiver_convention,
                    None | Some(mojito_ast::ast::ArgConvention::Imm)
                )
            })
            && receiver.register_passable_read()
        {
            if let Some(ty) = receiver.ty() {
                self.f
                    .reg_types
                    .entry(value.0)
                    .or_insert_with(|| ty.clone());
            }
            return (value, None);
        }
        let Some(contract) = call.call_contract() else {
            return (value, place);
        };
        // A by-address receiver lowering returned with no place reaches the
        // storage its value came from, as a by-address `read` argument does
        // ([`Self::materialize_operand`]).
        let lowered = match place {
            None if receiver_by_address(&contract, receiver.register_passable_read()).is_some() => {
                self.materialize_operand(receiver, value)
            }
            place => (value, place),
        };
        self.address_receiver(&contract, receiver, lowered)
    }

    /// Pass a `deinit` receiver, lowered to `lowered`, as upstream's call
    /// takes it: a memory-only one as a mutable `ref.make` of the storage
    /// it consumes, its `DeinitMem` operand, and a `register_passable` one
    /// as the owned value moved out of that storage, the read defining it
    /// becoming the move ([`MirInstr::take_place`]). A receiver with no
    /// place is already the owned value the call takes.
    pub(super) fn consume_receiver(
        &mut self,
        location: SourceSpan,
        lowered: (Reg, Option<MirPlace>),
        register_passable: bool,
    ) -> (Reg, Option<MirPlace>) {
        let (register, Some(place)) = lowered else {
            return lowered;
        };
        if !register_passable {
            let (address, place) = self.address_lowered_operand(
                location,
                (register, Some(place)),
                mojito_types::origin::Mutability::Mutable,
            );
            let addressed = place.as_ref().is_some_and(|place| {
                self.f
                    .reg_types
                    .get(&address.0)
                    .zip(self.place_value_ty(place))
                    .is_some_and(|(operand, value)| addressed_operand(operand, value).is_some())
            });
            return (address, place.filter(|_| !addressed));
        }
        let found = self.operand_definition(|instruction| {
            matches!(
                instruction,
                MirInstr::LoadPlace { dest, place: read }
                    if *dest == register
                        && crate::mir::call_addresses::same_storage(read, &place)
                        && read.through == place.through
            )
        });
        if let Some((instructions, index)) = found {
            instructions[index] = MirInstr::take_place(register, place);
            return (register, None);
        }
        let ty = self.f.reg_types.get(&register.0).cloned();
        let taken = match ty {
            Some(ty) => self.fresh_typed(location, Some(place.root), ty),
            None => self.fresh(location, Some(place.root)),
        };
        self.emit(MirInstr::take_place(taken, place));
        (taken, None)
    }

    /// Pass `receiver`, lowered to `lowered`, as the call bound by
    /// `contract` takes it: by address when [`receiver_by_address`] says
    /// so, as upstream's `lit.call` takes a `!lit.ref` of the receiver's
    /// storage. A subscript's getter, a write-back's getter, and an
    /// element call's getter address their receiver here, keyed by the
    /// accessor's own contract rather than the subscript node's.
    pub(super) fn address_receiver(
        &mut self,
        contract: &mojito_checked::checked::CheckedCallContract,
        receiver: Node<'a>,
        lowered: (Reg, Option<MirPlace>),
    ) -> (Reg, Option<MirPlace>) {
        self.address_receiver_with(
            contract,
            receiver.register_passable_read(),
            receiver.location(),
            lowered,
        )
    }

    /// [`Self::address_receiver`] of a receiver no checked node spells, as
    /// a named local's element call (`objs[0](3)`) reads it, the check's
    /// verdict on its register-passability given as
    /// `register_passable_read`.
    pub(super) fn address_receiver_with(
        &mut self,
        contract: &mojito_checked::checked::CheckedCallContract,
        register_passable_read: bool,
        location: SourceSpan,
        lowered: (Reg, Option<MirPlace>),
    ) -> (Reg, Option<MirPlace>) {
        match receiver_by_address(contract, register_passable_read) {
            Some(wanted) => self.address_lowered_operand(location, lowered, wanted),
            None => lowered,
        }
    }

    /// The receiver of a call through a `def(...)` bound: upstream reads the
    /// bound as a trait whose `__call__` requirement takes `read self`, so
    /// the call passes an immutable `!lit.ref` of the callee's storage. A
    /// callee of type `callee_ty` that is a callable-bound parameter and
    /// kept a typed `callee_place` is addressed; any other callee, such as a
    /// callable binder's compile-time function, which has no place, stays
    /// the value it is.
    pub(super) fn address_bound_callee(
        &mut self,
        location: SourceSpan,
        callee: Reg,
        callee_place: Option<&MirPlace>,
        callee_ty: Option<&Ty>,
    ) -> Reg {
        let bound = callee_ty.is_some_and(crate::mir::bound_calls::is_callable_bound);
        match callee_place.filter(|place| bound && place.is_typed()) {
            Some(place) => {
                self.address_lowered_operand(
                    location,
                    (callee, Some(place.clone())),
                    mojito_types::origin::Mutability::Immutable,
                )
                .0
            }
            None => callee,
        }
    }

    /// The receiver a subscript store's `setter` takes after its getter
    /// read `raw` from `place`: a fresh `ref.make` of the base when the
    /// setter takes it by address, as upstream's `SubscriptDLValue` store
    /// forms its own `!lit.ref` of the base (the reference a base holding
    /// one stores, [`Self::stored_handle`]); otherwise the base read again
    /// when the getter may have written it (`getter_mutates`) or took it by
    /// address, so a getter's reference never reaches a by-value setter.
    pub(super) fn setter_receiver(
        &mut self,
        setter: &mojito_checked::checked::CheckedCallContract,
        receiver: Node<'a>,
        raw: Reg,
        place: Option<&MirPlace>,
        getter_mutates: bool,
    ) -> Reg {
        if let Some(place) = place
            && place.ty.is_some()
            && let Some(wanted) = receiver_by_address(setter, receiver.register_passable_read())
        {
            return if self.place_holds_reference(place) {
                let (handle_place, handle) = self.stored_handle(place, wanted);
                self.emit_make_ref(receiver.location(), handle_place, handle)
            } else {
                self.place_address(receiver.location(), place, wanted)
            };
        }
        let reload = getter_mutates
            || self
                .f
                .reg_types
                .get(&raw.0)
                .and_then(addressed_receiver)
                .is_some();
        self.reload_augmented_source(raw, &place.cloned(), reload, receiver.location())
    }

    /// Lower method call `call`'s receiver as [`Self::lower_method_receiver`]
    /// does, copying a consuming receiver when `copied`, and open the call's
    /// setup frame ([`Self::open_call_setup`]). A receiver the method reads
    /// or owns by value is read at setup, after the arguments, as Mojo's
    /// call setup reads it; a `mut`/`ref`/`deinit` receiver binds its place
    /// and stays where written. A copied named receiver is read as an owned
    /// argument is, by its slot's copying use, as upstream copies argument
    /// 0 from the name's memory.
    pub(super) fn lower_setup_receiver(
        &mut self,
        call: Node<'a>,
        receiver: Node<'a>,
        copied: bool,
    ) -> (Reg, Option<MirPlace>) {
        self.lower_setup_receiver_with(call, receiver, |this| {
            this.lower_receiver_value(call, receiver, copied)
        })
    }

    /// [`Self::lower_setup_receiver`] with the receiver lowered by `lower`,
    /// as a subscript lowers its accessor's receiver.
    pub(super) fn lower_setup_receiver_with(
        &mut self,
        call: Node<'a>,
        receiver: Node<'a>,
        lower: impl FnOnce(&mut Self) -> (Reg, Option<MirPlace>),
    ) -> (Reg, Option<MirPlace>) {
        // A written-back subscript receiver is a place whose getter runs at
        // setup, after the arguments, as upstream loads a `SubscriptDLValue`
        // where the call uses it.
        if reads_receiver_by_value(call) || receiver.subscript_write_back().is_some() {
            self.open_call_setup();
            return self.lower_setup_operand(receiver, None, false, lower);
        }
        let lowered = lower(self);
        self.open_call_setup();
        lowered
    }

    /// [`Self::lower_setup_receiver`] for a call whose setup frame its
    /// arguments already opened: a reflected operator's receiver, the right
    /// operand, follows its argument, the left one, in source order. A
    /// receiver read by value is read at that setup; any other stays where
    /// written.
    pub(super) fn lower_open_setup_receiver(
        &mut self,
        call: Node<'a>,
        receiver: Node<'a>,
        copied: bool,
    ) -> (Reg, Option<MirPlace>) {
        let lower = |this: &mut Self| this.lower_receiver_value(call, receiver, copied);
        if reads_receiver_by_value(call) {
            return self.lower_setup_operand(receiver, None, false, lower);
        }
        self.outside_call_setup(lower)
    }

    /// Read a field (`B(1).x`) off an expression temporary by projecting its
    /// storage, exactly as `p.x` reads off a named local: its call-result
    /// slot, or a hidden `$mat_r` slot for a register value, as upstream
    /// materializes every expression temporary. The load is the
    /// temporary's last use, so drop elaboration runs its destructor where
    /// the pinned Mojo does: after a call that lends the field
    /// (`print(B(1).x)`), and before one whose register-passable `read`
    /// parameter loads it at call setup (`show(R(5).x)`). A consuming
    /// context's checked `CopyPlaceValue` copies an owning field out before
    /// the slot dies, as it does for a named local's.
    /// A chain (`W(R(1)).r.x`) projects the temporary's storage through
    /// each field ([`Self::temporary_field_place`]). `None`, leaving the
    /// base to the caller, when the chain's root is not a fresh temporary,
    /// is a reference result, or a materialized borrow source (which the
    /// caller projects through [`Self::lower_projected_reference_place`]),
    /// or when the field is a reference.
    pub(super) fn load_temporary_field(
        &mut self,
        expression: Node<'a>,
        object: Node<'a>,
        field: &str,
    ) -> Option<Reg> {
        let field_ty = expression.ty()?;
        if matches!(field_ty.kind(), TyKind::Ref(_)) {
            return None;
        }
        let mut place = if object.is_fresh_temporary() {
            self.temporary_root_place(object)?
        } else {
            self.temporary_field_place(object)?
        };
        place.project(Proj::Field(field.to_string()), field_ty.clone());
        let loaded = self.fresh_typed(expression.location(), Some(place.root), field_ty);
        self.emit(MirInstr::LoadPlace {
            dest: loaded,
            place,
        });
        Some(loaded)
    }

    /// The place of field chain `e` (`mk().s`, `mk().a.b`) over an owned
    /// expression temporary, with the temporary lowered into storage: its
    /// call-result slot, or a hidden `$mat_r` slot for a register value,
    /// as upstream materializes every expression temporary. A transfer out
    /// of the place leaves a hole in that storage, which the move analysis
    /// sees when the temporary is destroyed. `None`, emitting nothing, when
    /// the chain's root is no owned temporary.
    pub(super) fn temporary_field_place(&mut self, e: Node<'a>) -> Option<MirPlace> {
        let (CheckedOp::Member { field, .. }, Some(object)) = (e.op(), e.member_object()) else {
            return None;
        };
        if e.is_dtype_constant()
            || e.simd_length().is_some()
            || e.recorded_value().is_some()
            || e.is_pointer_mlir_value_read()
            || e.ty().is_some_and(|ty| matches!(ty.kind(), TyKind::Ref(_)))
        {
            return None;
        }
        let mut place = if object.is_fresh_temporary() {
            self.temporary_root_place(object)?
        } else {
            self.temporary_field_place(object)?
        };
        e.project_member(&mut place, field);
        Some(place)
    }

    /// The storage of `object`, a fresh expression temporary, lowered into
    /// it: its call-result slot, or a hidden `$mat_r` slot for a register
    /// value. `None`, emitting nothing, for a reference result or a
    /// materialized borrow source.
    fn temporary_root_place(&mut self, object: Node<'a>) -> Option<MirPlace> {
        if object.reference_result().is_some()
            || mojito_checked::checked::materialized_borrow_owner(object.adjustments()).is_some()
        {
            return None;
        }
        let slot = match self.expr_c(object) {
            CValue::Slot(slot) => slot,
            CValue::Value(value) => self.temporary_storage(object, value, false, "$mat_r"),
        };
        Some(MirPlace::root(slot, self.var_types.get(&slot).cloned()))
    }

    /// The place of a compile-time binding's read the checker materialized
    /// (a tuple or struct value parameter holding a `String`): its hidden
    /// `$mat_r` slot, which the read stores a copy of the binding into.
    /// Once the read has filled the slot (a conversion of the read stores
    /// its unconverted source there), the place is that slot's.
    pub(super) fn materialized_parameter_place(
        &mut self,
        identifier: Node<'a>,
    ) -> Option<MirPlace> {
        let owner = mojito_checked::checked::materialized_borrow_owner(identifier.adjustments())?;
        if let Some(variable) = self.owner_vars.get(&owner).copied() {
            return Some(self.hidden_slot_place(variable));
        }
        self.materialize_borrow_source(identifier, owner).1
    }

    /// Store a materialized borrow-source temporary in its storage
    /// ([`Self::materialize_borrow_slot`], registered under the
    /// checker-minted owner) and return the
    /// value plus the slot's place: a temporary bound to a `ref [origin]`
    /// parameter, or the temporary receiver of a `ref[self]`-returning method
    /// (`FormatStruct(writer, "P").params(...)`), borrows real frame storage
    /// this way. The expression lowering may already have materialized the
    /// slot (reference-context paths route through `reference_handle`); reuse
    /// it rather than storing a second copy.
    fn materialize_borrow_source(
        &mut self,
        expression: Node<'a>,
        owner: mojito_types::origin::OwnerId,
    ) -> (Reg, Option<MirPlace>) {
        let value = self.expr(expression);
        self.materialize_borrow_slot(expression, owner, value, expression.indirect_result())
    }

    /// Bind an already-lowered temporary `value` into its owner's storage
    /// (once per owner) and return the value plus the slot's place: its
    /// call-result slot when its producer returned it through memory
    /// (`indirect`), else a `$mat_r` slot ([`Self::temporary_storage`]). The conversion lowering uses this with the *unconverted* source
    /// so the slot holds what the view constructor borrows, not the view.
    pub(super) fn materialize_borrow_slot(
        &mut self,
        expression: Node<'a>,
        owner: mojito_types::origin::OwnerId,
        value: Reg,
        indirect: bool,
    ) -> (Reg, Option<MirPlace>) {
        let variable = if let Some(variable) = self.owner_vars.get(&owner).copied() {
            variable
        } else {
            if self.enclosing_binders.lifted
                && let Some(ty) = self
                    .f
                    .reg_types
                    .get(&value.0)
                    .cloned()
                    .or_else(|| expression.ty())
            {
                let variable = self.var(&format!("$mat_r{}", value.0));
                self.bind_comptime_memory(
                    expression.location(),
                    variable,
                    value,
                    ty,
                    mojito_types::origin::Mutability::Mutable,
                );
                self.owner_vars.insert(owner, variable);
                return self.read_borrow_slot(expression, value, variable);
            }
            let variable = self.temporary_storage(expression, value, indirect, "$mat_r");
            self.owner_vars.insert(owner, variable);
            // The slot owns the temporary, and the temporary may itself view
            // other storage (`StringSpan(s)`, a view-returning call): the
            // slot carries those loans, so the viewed storage outlives every
            // borrower of the slot rather than dying at the temporary's call.
            let loans = self.aggregate_borrows_unmaterialized(expression);
            self.establish_storage_loans(expression, variable, loans);
            variable
        };
        self.read_borrow_slot(expression, value, variable)
    }

    /// The place of `variable`, a hidden owned slot: through the reference
    /// it holds when [`Self::bind_comptime_memory`] anchored it in
    /// compile-time memory.
    pub(super) fn hidden_slot_place(&self, variable: VarId) -> MirPlace {
        let ty = self.var_types.get(&variable).cloned();
        let mut place = MirPlace::root(variable, ty.clone());
        if self.runtime_aliases.contains(&variable)
            && let Some(TyKind::Ref(reference)) = ty.as_ref().map(Ty::kind)
        {
            place.ty = Some(reference.referent.clone());
            place.through = Some(variable);
        }
        place
    }

    /// The value and place of `variable`, the borrow slot `value` was
    /// stored in, as [`Self::materialize_borrow_slot`] hands them back.
    fn read_borrow_slot(
        &mut self,
        expression: Node<'a>,
        value: Reg,
        variable: VarId,
    ) -> (Reg, Option<MirPlace>) {
        let place = self.hidden_slot_place(variable);
        // A reference-context lowering already handed back a handle to the
        // slot, which the call binds as it stands.
        let addressed = self
            .f
            .reg_types
            .get(&value.0)
            .zip(self.place_value_ty(&place))
            .is_some_and(|(operand, value)| addressed_operand(operand, value).is_some());
        if addressed {
            return (value, Some(place));
        }
        // The slot now owns the temporary, so a use that copies it (a
        // consuming receiver or argument) reads the slot, keeping it live
        // until the copy, not the register the store moved from.
        let loaded = self.fresh_typed(
            expression.location(),
            Some(variable),
            place
                .ty
                .clone()
                .or_else(|| self.f.reg_types.get(&value.0).cloned())
                .unwrap_or_else(Ty::error),
        );
        self.emit(MirInstr::LoadPlace {
            dest: loaded,
            place: place.clone(),
        });
        (loaded, Some(place))
    }

    /// Lower expression statement `expression`, whose value nothing
    /// consumes. Mojo destroys such a temporary before the next statement:
    /// a result returned through memory lands in its call-result slot
    /// ([`Self::expr_c`]), dead at its definition, so drop elaboration
    /// splices its `DropVar` right after the call. A non-raising call's slot
    /// is its result storage (`in_place`), so it drops ahead of the call's
    /// argument temporaries; a raising call's drops after them. A `^`
    /// transfer's storage is forwarded to a slot of its own (the check
    /// records the discard as its consumer), so the slot destroys the source
    /// where it lies, before the next statement. A trivial register value's
    /// `^` is a copy the check decided, which stays in its register.
    pub(super) fn discard_result(&mut self, expression: &'a mojito_hir::hir::HirExpr) {
        let handle = self.handle(expression);
        if handle.transfer_operand().is_none()
            || handle.transfer_verdict()
                == Some(mojito_checked::checked::TransferVerdict::TrivialCopy)
        {
            self.with_statement_loans(|this| {
                this.expr_c(handle);
            });
            return;
        }
        // The discard takes the transferred storage and destroys it in
        // place, as an owned parameter takes it.
        self.bind_operand(handle, Some(OperandBinding::Owned));
        let value = self.expr_hir(expression);
        let Some(ty) = self
            .f
            .reg_types
            .get(&value.0)
            .cloned()
            .or_else(|| handle.ty())
        else {
            return;
        };
        if let Some(binding) = self.discard_binding(value, ty, false) {
            self.emit(binding);
        }
    }

    /// The binding of `value`, an owned value of type `ty` nothing reads, to
    /// a hidden slot ([`crate::mir::CALL_RESULT_SLOT`]), dead at its
    /// definition, so drop elaboration destroys it there; `in_place` when it
    /// is its producer's result storage. `None` when `ty` owns no droppable
    /// storage.
    pub(super) fn discard_binding(
        &mut self,
        value: Reg,
        ty: Ty,
        in_place: bool,
    ) -> Option<MirInstr> {
        if !owns_droppable_storage(&ty) {
            return None;
        }
        let variable = self.var(&format!("{}{}", crate::mir::CALL_RESULT_SLOT, value.0));
        self.var_types.insert(variable, ty.clone());
        Some(MirInstr::DefVar {
            var: variable,
            src: value,
            binding_ty: Some(ty),
            in_place,
        })
    }

    /// Whether the current block's last instruction is a non-raising call
    /// producing `value`, so a binding emitted next is that call's result
    /// storage (`DefVar::in_place`). A dunder operator carries no raising
    /// flag of its own; `operator_non_raising` is its checked node's answer.
    /// A prefix dunder's raising is not recorded, so its result is never
    /// bound in place.
    pub(super) fn produced_in_place(&self, value: Reg, operator_non_raising: bool) -> bool {
        let Some(producer) = self.f.body.instrs.last() else {
            return false;
        };
        match producer {
            MirInstr::Call { dest, raises, .. }
            | MirInstr::CallIndirect { dest, raises, .. }
            | MirInstr::MethodCall { dest, raises, .. } => *dest == value && raises.is_none(),
            MirInstr::BinOp { dest, .. } => *dest == value && operator_non_raising,
            _ => false,
        }
    }

    /// Anchor a loan-carrying temporary *argument* in storage whose loans
    /// keep its borrowed sources alive through the consuming call. A
    /// temporary returned through memory is anchored in its call-result
    /// slot, which the call reads or takes by the operand's convention
    /// ([`Self::operand_c`]), as upstream's call borrows or consumes its
    /// `__call_result_tmp__`: the slot dies when the call returns, or is the
    /// callee's. A register-passable one is materialized in a `$arg_loan_r`
    /// slot and read back out of it, the argument a shallow read of the
    /// slot, whose statement-end `KeepAlive` flush extends the slot's — and
    /// so the loans' — liveness across the call. Returns the operand, or
    /// `None` when the temporary needs no anchor.
    /// Anchor a temporary argument that borrows caller storage (a
    /// constructor or call result holding references/pointers into live
    /// places) in a hidden slot, as a chained view receiver is, or its
    /// sources are dropped before the consuming call runs. Bare
    /// reference/pointer handles stay unanchored: a `LoadPlace` read out of
    /// the hidden slot would dereference the handle.
    /// Scope: only a plain `Call` or method-call temporary (a view result
    /// such as `s.strip()`) in a call's argument list or an operator's
    /// operands anchors (see `allow_argument_anchors`) — every other consumer
    /// carries the temporary's loans through its own channel, and an extra
    /// anchor is a conflicting duplicate borrow.
    /// A subscript view temporary (`s[byte=a:b]`, a `Span` slice) borrows its
    /// source through the subscript's own borrow fact and no consumer channel
    /// retains it — a plain call, a method call, and a construction alike
    /// would drop the source before the call runs when the argument is the
    /// source's last use — so it anchors in every argument list and operand:
    /// one loan on the source, never a duplicate of a channel that does not
    /// exist.
    pub(super) fn anchor_temporary_argument(
        &mut self,
        expression: Node<'a>,
        value: Reg,
        adjustments: &[mojito_checked::checked::SemanticAdjustment],
        converted: bool,
        reads: bool,
    ) -> Option<(Reg, Option<MirPlace>)> {
        let subscript_view = matches!(
            expression.op(),
            CheckedOp::Index { .. } | CheckedOp::MultiIndex { .. } | CheckedOp::Slice { .. }
        );
        let call_temporary = self.allow_argument_anchors
            && matches!(
                expression.op(),
                CheckedOp::Call { .. } | CheckedOp::MethodCall { .. }
            );
        // A converted temporary's view is already bound to its
        // `$conv_view_r` slot by the conversion lowering; a second anchor
        // would duplicate that loan.
        let anchored_by_conversion = converted
            && (mojito_checked::checked::materialized_borrow_owner(adjustments).is_some()
                || self
                    .conversion_view_loans
                    .contains_key(&expression.address()));
        if (call_temporary || subscript_view)
            && !anchored_by_conversion
            && expression
                .ty()
                .as_ref()
                .is_some_and(|ty| ty.kind().is_non_pointer_struct())
        {
            let loans = self.aggregate_borrows(expression);
            if !loans.is_empty() {
                return Some(self.anchor_borrowing_argument(expression, value, loans, reads));
            }
        }
        None
    }

    fn anchor_borrowing_argument(
        &mut self,
        expression: Node<'a>,
        value: Reg,
        loans: Vec<MirLoan>,
        reads: bool,
    ) -> (Reg, Option<MirPlace>) {
        if expression.indirect_result() {
            let slot = self.temporary_storage(expression, value, true, "$arg_loan_r");
            self.establish_storage_loans(expression, slot, loans);
            return self.operand_c(expression, CValue::Slot(slot), reads);
        }
        let (read, place) =
            self.anchor_borrowing_temporary(expression, value, loans, "$arg_loan_r");
        self.pending_argument_anchors
            .extend(place.as_ref().map(|place| place.root));
        (
            read,
            place.filter(|_| self.operand_reads(expression, reads)),
        )
    }

    /// Bind a loan-carrying temporary into a hidden retained slot whose loans
    /// keep its borrowed sources alive and conflict-checked for the slot's
    /// lifetime, and read the value back out of that slot: the chained view
    /// receiver anchor, the temporary's call-result slot when returned
    /// through memory and otherwise `prefix`'s
    /// ([`Self::temporary_storage`]). Without the anchor, ownership sees
    /// the borrowed source dead immediately after the producing expression and
    /// drop elaboration frees it before the consuming call runs.
    fn anchor_borrowing_temporary(
        &mut self,
        expression: Node<'a>,
        value: Reg,
        loans: Vec<MirLoan>,
        prefix: &str,
    ) -> (Reg, Option<MirPlace>) {
        let variable =
            self.temporary_storage(expression, value, expression.indirect_result(), prefix);
        self.establish_storage_loans(expression, variable, loans);
        let view_ty = self.var_types.get(&variable).cloned();
        let place = MirPlace::root(variable, view_ty);
        let read = self.fresh_typed(
            expression.location(),
            Some(variable),
            place.ty.clone().unwrap_or_else(Ty::error),
        );
        self.emit(MirInstr::LoadPlace {
            dest: read,
            place: place.clone(),
        });
        (read, Some(place))
    }

    /// The place of a call result the checker materialized as the holder of
    /// a dereferenced pointer field (`make(xs).src[][0] = 9`,
    /// `h.view().src[]`): its hidden owned slot, whose loans keep the viewed
    /// storage alive while the projection chain reads or writes through it.
    /// Any other call result is a value, not a place.
    pub(super) fn materialized_temporary_base_place(&mut self, base: Node<'a>) -> Option<MirPlace> {
        if !base.is_fresh_temporary() {
            return None;
        }
        let owner = mojito_checked::checked::materialized_borrow_owner(base.adjustments())?;
        self.materialize_borrow_source(base, owner).1
    }

    /// Retain storage for any callable place. Nominal callable receivers use it
    /// for `mut self`; closure values use it so a lambda's moved storage is
    /// borrowed in place across repeated calls.
    pub(super) fn callable_receiver_place(&mut self, expression: Node<'a>) -> Option<MirPlace> {
        let place = self.simple_place(expression)?;
        place.is_typed().then_some(place)
    }

    /// The callable a `Variant` placement constructor or consuming teardown
    /// invokes: a local slot is borrowed where it lies, as a call borrows
    /// its callee and upstream passes the handler by reference, and any
    /// other expression is its value.
    pub(super) fn callable_operand(&mut self, expression: Node<'a>) -> Reg {
        if let CheckedOp::Identifier(name) = &expression.checked.op
            && let Some(variable) = self.local_callee(expression, name)
            && !self.runtime_aliases.contains(&variable)
            && !self.aliases.contains_key(&variable)
        {
            let dest = self.fresh(expression.location(), Some(variable));
            self.emit(MirInstr::UseVar {
                dest,
                var: variable,
                mode: UseMode::BorrowShared,
                constructor: None,
            });
            return dest;
        }
        self.expr(expression)
    }

    pub(super) fn mir_interior_origin(
        &mut self,
        origin: &mojito_types::origin::OriginPlace,
        fallback: Option<VarId>,
    ) -> Option<MirInteriorOrigin> {
        let root = self.owner_vars.get(&origin.root).copied().or(fallback)?;
        self.owner_vars.entry(origin.root).or_insert(root);
        Some(MirInteriorOrigin {
            root,
            path: origin.path.clone(),
        })
    }

    /// Canonical interior origin for a loan established directly through
    /// `place`. Loans and `InvalidateInteriors` facts must name interior
    /// origins in the same owner domain — `owner_vars` plus the checked
    /// owner-relative path — or the invalidation can never retire the loan.
    /// When the canonical owner slot differs from the executable root, the
    /// borrow reads through the reference held in that root (an iteration
    /// binding, or a rebound symbolic owner's current slot); recording the
    /// root as the place's `through` reference keeps the loan's executable
    /// place honest for the verifier's place/origin consistency rule.
    pub(super) fn direct_borrow_interior(
        &mut self,
        place: &mut MirPlace,
        origin: &mojito_types::origin::OriginPlace,
    ) -> MirInteriorOrigin {
        let canonical = self
            .mir_interior_origin(origin, Some(place.root))
            .expect("a direct borrow's place root is the interior-origin fallback");
        if canonical.root != place.root && place.through.is_none() {
            place.through = Some(place.root);
        }
        canonical
    }

    /// Emit checker-selected invalidations at the precise operation boundary.
    /// `fallback` is used only for a whole-binding redefinition whose target
    /// owner has no expression occurrence before this instruction.
    pub(super) fn emit_interior_invalidations(
        &mut self,
        expression: Node<'a>,
        fallback: Option<VarId>,
    ) {
        let invalidations = expression.interior_invalidations();
        self.emit_interior_invalidation_facts(&invalidations, &expression.location(), fallback);
    }

    pub(super) fn emit_interior_invalidation_facts(
        &mut self,
        invalidations: &[mojito_checked::checked::InteriorInvalidation],
        site: &SourceSpan,
        fallback: Option<VarId>,
    ) {
        for invalidation in invalidations {
            let Some(base) = self.mir_interior_origin(&invalidation.base, fallback) else {
                // Establishing an interior generation installs this checked
                // OwnerId's MIR slot. If no mapping exists, no earlier live
                // generation in this function can match the fact. Skipping also
                // keeps a same-span fact from another specialized clone inert.
                continue;
            };
            let except = invalidation
                .except
                .and_then(|owner| self.owner_vars.get(&owner).copied());
            let marker = self.fresh_typed(site.clone(), Some(base.root), Ty::none());
            self.emit(MirInstr::InvalidateInteriors {
                base,
                except,
                include_base_generation: invalidation.include_base_generation,
                marker,
            });
        }
    }

    /// Emit all invalidations whose semantic boundary is this call.  Argument
    /// facts are deliberately delayed until every argument has been evaluated:
    /// the callee, rather than evaluation of the place expression, performs the
    /// mutation.
    pub(super) fn emit_call_invalidations(
        &mut self,
        call: Node<'a>,
        args: &[Node<'a>],
        kwargs: &[(&str, Node<'a>)],
    ) {
        for &argument in args.iter().chain(kwargs.iter().map(|(_, value)| value)) {
            self.emit_interior_invalidations(argument, None);
        }
        self.emit_interior_invalidations(call, None);
    }

    /// Bind a borrowed actual as a shallow place read — the read
    /// method-receiver model — instead of the `UseVar` lifecycle copy, so a
    /// user `__copyinit__` never runs for the pass itself. Serves both
    /// checker-marked `BorrowReadArgument` reads and retained `mut`/`ref`
    /// places, whose bound value the VM replaces with a caller-place
    /// reference before the callee runs. Restricted
    /// to a bare aggregate-typed variable whose special identifier lowerings
    /// (callable references, closures, pointer handles, alias slots, checked
    /// value copies) don't apply; everything else falls back to ordinary
    /// expression lowering. Scalars also fall back: their register copy has no
    /// user-observable lifecycle, and keeping `UseVar` preserves their pinned
    /// ASAP destruction, which aggregate `LoadPlace` results deliberately
    /// trade for owner retention through the call.
    pub(super) fn lower_borrowed_read_argument(&mut self, expression: Node<'a>) -> Option<Reg> {
        self.lower_borrowed_read_operand(expression)
            .map(|(register, _)| register)
    }

    /// [`Self::lower_borrowed_read_argument`] with the place a reference
    /// result's referent is read through: its hidden handle slot, which a
    /// call passes as the returned reference itself, as upstream's
    /// `lit.call` passes the `!lit.ref` the accessor returned. Any other
    /// read has no place here; its caller derives the simple place.
    fn lower_borrowed_read_operand(
        &mut self,
        expression: Node<'a>,
    ) -> Option<(Reg, Option<MirPlace>)> {
        if let Some((object, _)) = expression.index_operands() {
            return self.lower_borrowed_element_read(expression, object);
        }
        if expression.call_contract().is_some() || expression.reference_result().is_some() {
            return self.lower_borrowed_reference_result(expression);
        }
        self.lower_borrowed_name_read(expression)
            .map(|register| (register, None))
    }

    /// A borrowed read of a bare aggregate-typed variable
    /// ([`Self::lower_borrowed_read_argument`]).
    fn lower_borrowed_name_read(&mut self, expression: Node<'a>) -> Option<Reg> {
        let name = self.name_spelled(expression)?;
        // A closure's storage value is the declaration's slot, lent as any
        // local is; a body without storage is a value with no slot to lend.
        let closure_storage = self
            .nested_info(expression)
            .map(|info| info.storage.is_some());
        // A module constant read is a value with no slot of its own, and a
        // literal its use materializes (`x // C` of `comptime C = 8`) is
        // upstream's implicit conversion: the callee receives the converted
        // value, never the literal's place.
        if expression.resolved_callable().is_some()
            || expression.param_literal_target().is_some()
            || expression.literal_materialization().is_some()
            || closure_storage == Some(false)
            || expression.is_origin_bearing_pointer()
            || expression.constant_param_value()
            || (!self.vars.contains(&name) && self.overloads.is_function(&name))
        {
            return None;
        }
        if expression.adjustments().iter().any(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::CopyPlaceValue
            )
        }) {
            return None;
        }
        let ty = expression.ty()?;
        if closure_storage != Some(true)
            && !ty.is_non_simd_struct()
            && !matches!(
                ty.kind(),
                TyKind::Tuple(_)
                    | TyKind::RuntimePack(_)
                    | TyKind::VariadicPack(_)
                    | TyKind::Variant(_)
                    | TyKind::Param { .. }
                    | TyKind::Assoc { .. }
                    | TyKind::Dependent(_)
            )
        {
            return None;
        }
        // A compile-time binding's read owns no slot: its place is the
        // hidden slot it materializes into, never an alias.
        if expression.recorded_value().is_none() {
            let var = self.expression_var(&name, expression);
            if self.aliases.contains_key(&var) || self.runtime_aliases.contains(&var) {
                return None;
            }
        }
        let place = self.simple_place(expression)?;
        let dest = self.fresh_typed(
            expression.location(),
            Some(place.root),
            place.ty.clone().unwrap_or(ty),
        );
        self.emit(MirInstr::LoadPlace { dest, place });
        Some(dest)
    }

    /// A borrowed element read (`f(a[0])` at a read slot): the element is
    /// read where it lies, as a field is, so its `__copyinit__` never runs
    /// for the pass. A reference-returning accessor, a container's or a
    /// view's, lends its referent through the receiver path. Tuple and pack
    /// storage loads the element place, whatever the element: the ordinary
    /// read would copy the whole storage to index it.
    fn lower_borrowed_element_read(
        &mut self,
        expression: Node<'a>,
        object: Node<'a>,
    ) -> Option<(Reg, Option<MirPlace>)> {
        if expression.adjustments().iter().any(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::CopyPlaceValue
            )
        }) {
            return None;
        }
        if expression.reference_result().is_some() {
            return self.lower_borrowed_reference_result(expression);
        }
        if expression.call_contract().is_some() {
            return None;
        }
        if !matches!(
            object.intrinsic_index_dispatch(),
            Some(MirIntrinsicSubscript::TupleStorage | MirIntrinsicSubscript::VariadicStorage)
        ) {
            return None;
        }
        let place = self.try_place(expression)?;
        let dest = self.fresh_typed(
            expression.location(),
            Some(place.root),
            place
                .ty
                .clone()
                .or_else(|| expression.ty())
                .unwrap_or_else(Ty::error),
        );
        self.emit(MirInstr::LoadPlace { dest, place });
        Some((dest, None))
    }

    /// A borrowed reference result (`show(o.value())`, `show(pick(a))`,
    /// `f(d[k])` at a read slot): the referent is read through the returned reference, as
    /// upstream binds the `read` parameter to it, so its `__copyinit__`
    /// never runs for the pass. A referent owning no droppable storage is
    /// an ordinary value read, unless the check judged the slot by address
    /// (a generic `T` or a pack element): its call then takes the returned
    /// reference whatever the referent.
    fn lower_borrowed_reference_result(
        &mut self,
        expression: Node<'a>,
    ) -> Option<(Reg, Option<MirPlace>)> {
        let reference = expression.reference_result()?;
        (owns_droppable_storage(&reference.referent)
            || matches!(reference.referent.kind(), TyKind::Dependent(_))
            || self.read_by_address_binding(expression))
        .then(|| self.lower_call_receiver(expression))
    }

    /// Lower call operand `expression`, a value no place holds, for a call
    /// that reads (`reads`) or takes it by its recorded convention
    /// ([`Self::operand_c`]): a result returned through memory is used in
    /// its call-result slot, and a loan-carrying temporary anchors its loans
    /// through the call ([`Self::anchor_temporary_argument`]). A memory-only
    /// `read` temporary ([`read_by_address`]) is passed by the address of
    /// that storage, its call-result, argument-anchor, or lambda storage
    /// slot, as upstream's `lit.call` takes a `!lit.ref` of the rvalue it
    /// materialized: the slot is returned as the operand's place, which
    /// [`Self::lower_call_operand`] addresses.
    fn temporary_operand(
        &mut self,
        expression: Node<'a>,
        converted: bool,
        reads: bool,
    ) -> (Reg, Option<MirPlace>) {
        let by_address = match self.operand_binding(expression) {
            Some(OperandBinding::Read { by_address }) => by_address,
            _ => self.read_by_address(expression),
        };
        let value = match self.operand_expr_c(expression) {
            CValue::Value(value) => {
                if let Some((read, place)) = self.anchor_temporary_argument(
                    expression,
                    value,
                    expression.adjustments(),
                    converted,
                    reads,
                ) {
                    return (read, place.filter(|_| by_address));
                }
                match self.loaned_call_result(expression, value) {
                    Some(slot) => CValue::Slot(slot),
                    None => return (value, None),
                }
            }
            slot @ CValue::Slot(_) => slot,
        };
        let (read, place) = self.operand_c(expression, value, reads);
        (read, place.filter(|_| by_address))
    }

    /// Whether call operand `expression` binds a `read` parameter the check
    /// judged by address on the callee's declared type
    /// ([`OperandBinding::Read`]), a template's `read x: T` included, as
    /// upstream's `lit.call` takes a `!lit.ref` of every such argument
    /// whatever the actual's own type.
    pub(super) fn read_by_address_binding(&self, expression: Node<'a>) -> bool {
        self.operand_binding(expression) == Some(OperandBinding::Read { by_address: true })
    }

    /// The place a field chain or intrinsic subscript over a reference
    /// names: the projection below an
    /// accessor's returned reference
    /// ([`Self::lower_projected_reference_place`]) or through a pointer's
    /// pointee ([`Self::try_place`]). A bare name, a temporary, a
    /// projection of one (`R(6).x`, which the call's temporary lowering
    /// keeps alive through it), and a subscript the checker resolved to a
    /// call name no such place.
    fn projected_read_place(&mut self, expression: Node<'a>) -> Option<MirPlace> {
        let projected = match expression.op() {
            CheckedOp::Member { .. } => true,
            CheckedOp::Index { .. } => expression.call_contract().is_none(),
            _ => false,
        };
        if !projected || expression.is_fresh_temporary() || rooted_in_temporary(expression) {
            return None;
        }
        self.lower_projected_reference_place(expression)
            .or_else(|| self.try_place(expression))
    }

    /// Whether call operand `expression` binds a `read` parameter by value
    /// and the check proved its type register-passable
    /// (`RegisterPassableRead`), as upstream passes it with no storage.
    fn register_passable_read_binding(&self, expression: Node<'a>) -> bool {
        expression.register_passable_read()
            && self.operand_binding(expression) == Some(OperandBinding::Read { by_address: false })
    }

    /// Whether call operand `expression` is a `read` temporary upstream
    /// passes by address: its call reads it ([`Self::operand_reads`]), and
    /// the check did not prove its type register-passable
    /// (`RegisterPassableRead`).
    fn read_by_address(&self, expression: Node<'a>) -> bool {
        expression.ty().is_some()
            && !expression.register_passable_read()
            && self.operand_reads(expression, false)
    }

    /// Materialize `value`, a fresh temporary its call returned in
    /// registers, in a hidden `$mat_r` slot whose place a `ref self` or
    /// other place receiver binds, as upstream's `getLValueForResult` gives
    /// an rvalue `anonymous` storage when an lvalue is required. A value
    /// owning no droppable storage stays in its register.
    fn materialize_receiver(&mut self, receiver: Node<'a>, value: Reg) -> (Reg, Option<MirPlace>) {
        match self
            .f
            .reg_types
            .get(&value.0)
            .cloned()
            .or_else(|| receiver.ty())
            .filter(owns_droppable_storage)
        {
            Some(ty) => self.materialize_in_slot(receiver.location(), value, ty),
            None => (value, None),
        }
    }

    /// Whether `value`, which operand `expression` produced, is a temporary
    /// a hidden slot may own and destroy: a fresh temporary, a `^`
    /// transfer, or a value the body defines fresh
    /// ([`Self::owned_temporary`]). A reference to the value
    /// ([`Self::passes_address`]), or a read out of a temporary that
    /// outlives it, owns nothing a slot may destroy.
    pub(in crate::mir) fn materializable(&self, expression: Node<'a>, value: Reg) -> bool {
        !self.passes_address(value, expression)
            && (expression.is_fresh_temporary()
                || matches!(expression.op(), CheckedOp::Transfer(_))
                || self.owned_temporary(value))
    }

    /// Whether `value`, the operand `expression` lowered to, passes a
    /// reference to the value the expression's checked type names, as
    /// upstream's `ref<P>` argument for a `P` ([`addressed_operand`]),
    /// rather than that value.
    pub(in crate::mir) fn passes_address(&self, value: Reg, expression: Node<'a>) -> bool {
        self.f
            .reg_types
            .get(&value.0)
            .zip(expression.ty())
            .is_some_and(|(operand, value)| addressed_operand(operand, &value).is_some())
    }

    /// [`Self::materialize_in_slot`] for the temporary operand `expression`
    /// lowered to `value`: the slot carries the loans the temporary holds
    /// (a view's of the storage it borrows), so that storage outlives every
    /// read through the slot, as [`Self::loaned_call_result`]'s slot does.
    pub(in crate::mir) fn materialize_operand_in_slot(
        &mut self,
        expression: Node<'a>,
        value: Reg,
        ty: Ty,
    ) -> (Reg, Option<MirPlace>) {
        let (read, place) = self.materialize_in_slot(expression.location(), value, ty);
        if let Some(place) = &place {
            let loans = self.aggregate_borrows(expression);
            self.establish_storage_loans(expression, place.root, loans);
        }
        (read, place)
    }

    /// Bind `value`, of type `ty`, a temporary no storage holds, to a
    /// hidden `$mat_r` slot and read it back with the slot's place, as
    /// upstream's IR generation gives an rvalue `anonymous` storage where a
    /// `!lit.ref` is wanted. The slot is destroyed at its last use.
    pub(in crate::mir) fn materialize_in_slot(
        &mut self,
        location: SourceSpan,
        value: Reg,
        ty: Ty,
    ) -> (Reg, Option<MirPlace>) {
        let variable = self.var(&format!("$mat_r{}", value.0));
        self.var_types.insert(variable, ty.clone());
        self.emit(MirInstr::DefVar {
            var: variable,
            src: value,
            binding_ty: Some(ty.clone()),
            in_place: false,
        });
        let place = MirPlace::root(variable, Some(ty.clone()));
        let loaded = self.fresh_typed(location, Some(variable), ty);
        self.emit(MirInstr::LoadPlace {
            dest: loaded,
            place: place.clone(),
        });
        (loaded, Some(place))
    }
}

/// Whether projection chain `expression` (fields and subscripts) is rooted
/// in a fresh temporary rather than in a place or a returned reference.
fn rooted_in_temporary(expression: Node<'_>) -> bool {
    let mut node = expression;
    while matches!(
        node.op(),
        CheckedOp::Member { .. } | CheckedOp::Index { .. } | CheckedOp::MultiIndex { .. }
    ) && node.reference_result().is_none()
    {
        match node.object() {
            Some(object) => node = object,
            None => return false,
        }
    }
    node.is_fresh_temporary() && node.reference_result().is_none()
}

/// How the method call (or subscript accessor) bound by `contract`
/// receives its receiver, which is register-passable as
/// `register_passable_read` records, by address: a declared `mut` or
/// `ref` receiver (`receiver_requires_place`, whatever its effective
/// access) as a mutable
/// reference, which [`Flatten::place_handle_ty`] narrows to the place's own
/// mutability, a `read` one the check did not prove register-passable as an
/// immutable one, as upstream's `lit.call` takes a `!lit.ref` of the
/// receiver's storage. An owned receiver and an elided one are `None`.
pub(super) const fn receiver_by_address(
    contract: &mojito_checked::checked::CheckedCallContract,
    register_passable_read: bool,
) -> Option<mojito_types::origin::Mutability> {
    use mojito_ast::ast::ArgConvention;
    use mojito_types::origin::Mutability;
    if contract.receiver_elided {
        return None;
    }
    if contract.receiver_requires_place {
        return Some(Mutability::Mutable);
    }
    match contract.receiver_convention {
        None | Some(ArgConvention::Imm) if !register_passable_read => Some(Mutability::Immutable),
        _ => None,
    }
}

/// Whether method call `call` reads or owns its receiver by value, so the
/// receiver is read at the call's setup; a `mut`/`ref`/`deinit` receiver
/// binds its place where written.
fn reads_receiver_by_value(call: Node<'_>) -> bool {
    call.call_contract().is_none_or(|contract| {
        matches!(
            contract.receiver_convention,
            None | Some(mojito_ast::ast::ArgConvention::Imm | mojito_ast::ast::ArgConvention::Var)
        )
    })
}
