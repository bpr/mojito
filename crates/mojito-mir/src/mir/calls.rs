//! Call-site lowering: argument/keyword/receiver lowering, checked-call value
//! adjustments and boundaries, reference-result places, and interior-origin
//! invalidation emission.
//! Extracted from `mir.rs`; see `docs/symbol-map.md`.

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
    /// Where the read begins once the handle is materialized: its block and
    /// the index of the first instruction after the handle's loans.
    read_start: Option<(MirBlockId, usize)>,
}

/// Whether a value of this type owns storage that destruction must release:
/// an aggregate (a struct with a destructor or owning fields, tuple/pack
/// storage, a variant, or a still-abstract parameter type). Kept textually
/// aligned with the drop analysis' `may_alias_owned_storage`.
pub(super) fn owns_droppable_storage(ty: &Ty) -> bool {
    matches!(
        ty.kind(),
        TyKind::Struct(..)
            | TyKind::Tuple(_)
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
        self.lower_call_argument_with(expression, false, CheckedCallArgumentSource::Positional(0))
    }

    /// Lower one operand of a struct operator: a named place the checker
    /// marked `BorrowReadArgument` (the dunder reads it) is read where it
    /// lies, as a read-convention call argument is; a temporary it marked
    /// `ReadTemporaryArgument` binds a `$tmp_arg_r` slot, as a read call
    /// argument's does, so it dies when the dunder returns; anything else is
    /// an ordinary value. Inside an operator's setup frame
    /// ([`Self::open_call_setup`]) a simple place's read is deferred to the
    /// frame's setup.
    pub(super) fn lower_operator_operand(&mut self, expression: Node<'a>) -> Reg {
        self.lower_setup_operand(expression, None, false, |this| {
            let borrowed = expression.adjustments().iter().any(|adjustment| {
                matches!(
                    adjustment,
                    mojito_checked::checked::SemanticAdjustment::BorrowReadArgument
                )
            });
            borrowed
                .then(|| this.lower_borrowed_read_argument(expression))
                .flatten()
                .unwrap_or_else(|| {
                    let value = this.expr(expression);
                    this.bind_read_temporary(expression, value)
                })
        })
    }

    /// Bind `value`, the lowered `expression`, through
    /// [`Self::bind_temporary_argument`] when the checker marked it
    /// `ReadTemporaryArgument`: an owned temporary a read parameter or a
    /// read dunder operand takes, which the caller destroys once the call
    /// returns. Anything else is `value` unchanged.
    pub(super) fn bind_read_temporary(&mut self, expression: Node<'a>, value: Reg) -> Reg {
        let adjustments = expression.adjustments();
        if !adjustments.iter().any(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::ReadTemporaryArgument
            )
        }) {
            return value;
        }
        let converted = expression.implicit_conversion().is_some();
        self.bind_temporary_argument(expression, value, adjustments, converted)
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
            pending.read_start = Some((self.cur, self.f.blocks[self.cur].instrs.len()));
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
        });
    }

    /// Close the innermost setup frame, emitting its deferred reads: the
    /// receiver and operator operands first, then the arguments in the
    /// parameter order `call`'s checked contract binds them in, as Mojo's
    /// call setup reads them; a call without a contract keeps source order.
    pub(super) fn close_call_setup(&mut self, call: Option<Node<'a>>) {
        let Some(setup) = self.call_setups.pop() else {
            return;
        };
        let mut reads = setup.reads;
        if reads.len() > 1
            && let Some(contract) = call.and_then(Node::call_contract)
        {
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
        }
        for instr in reads.into_iter().flat_map(|(_, instrs)| instrs) {
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
    /// ([`Self::display_elements`]). A read that split the block stays where
    /// written.
    fn defer_to_setup<R>(
        &mut self,
        expression: Node<'a>,
        source: Option<CheckedCallArgumentSource>,
        bound: bool,
        lower: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let block = self.cur;
        let start = self.f.blocks[block].instrs.len();
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
            Some((block, start))
        } else {
            reference.and_then(|reference| reference.read_start)
        };
        if let Some((block, start)) = read_start
            && self.cur == block
            && let Some(setup) = self.call_setups.last_mut()
        {
            let instrs = self.f.blocks[block].instrs.drain(start..).collect();
            setup.reads.push((source, instrs));
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

    /// Whether `expression` is a by-value operand whose reference result
    /// belongs to call setup: an accessor subscript or a `ref`-returning call
    /// read as a value, not a borrow, a materialized temporary, a module
    /// constant, a pointer handle, a spread, or a transfer. An accessor chain
    /// over a temporary (`first(Span(ys))[0]`) is read where written: the
    /// temporary's slot dies at its last use, and a view's referent is
    /// reached through it.
    fn setup_reference(&mut self, expression: Node<'a>) -> bool {
        let rooted = std::iter::successors(expression.object(), |node| node.object())
            .last()
            .is_none_or(|root| self.simple_place(root).is_some());
        rooted
            && expression.reference_result().is_some()
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
            && mojito_checked::checked::materialized_borrow_owner(expression.adjustments())
                .is_none()
    }

    /// Whether `expression` is a by-value place whose read belongs to call
    /// setup: a direct root or field chain with no lvalue work, no reference
    /// alias between it and its owner, and none of the identifier lowerings
    /// that are values rather than reads (callables, closures, pointer
    /// handles, folded constants, materialized borrow slots).
    fn setup_read(&mut self, expression: Node<'a>) -> bool {
        if expression.resolved_callable().is_some()
            || self.nested_info(expression).is_some()
            || expression.is_origin_bearing_pointer()
            || expression.constant_param_value()
            || mojito_checked::checked::materialized_borrow_owner(expression.adjustments())
                .is_some()
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
            )
        {
            return false;
        }
        self.simple_place(expression).is_some_and(|place| {
            place.through.is_none()
                && !self.aliases.contains_key(&place.root)
                && !self.runtime_aliases.contains(&place.root)
        })
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

    /// `lends_reads`: the enclosing call's result borrows its read
    /// arguments, so a shared read of a place argument retains that place
    /// (see [`Self::lends_read_arguments`]). `source` names the argument
    /// for its setup read's parameter order.
    fn lower_call_argument_with(
        &mut self,
        expression: Node<'a>,
        lends_reads: bool,
        source: CheckedCallArgumentSource,
    ) -> (Reg, Option<MirPlace>) {
        let owns_setup = self
            .call_setups
            .last()
            .is_some_and(|setup| setup.depth == self.argument_depth);
        self.argument_depth += 1;
        let lowered =
            self.lower_call_operand(expression, lends_reads, owns_setup.then_some(source));
        self.argument_depth -= 1;
        lowered
    }

    /// [`Self::lower_call_argument_with`] one argument depth down. `setup` is
    /// the argument's source when the innermost setup frame is its call's,
    /// so a copied-value read of a simple place defers to setup.
    fn lower_call_operand(
        &mut self,
        expression: Node<'a>,
        lends_reads: bool,
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
            return self.lower_call_operand(inner, lends_reads, setup);
        }
        // A temporary bound to a `ref [origin]` parameter: store the value in
        // a hidden slot registered under the checker-minted owner identity and
        // hand the call that slot's place — the VM/native ref binding then
        // borrows real frame storage, and the slot's loans give the temporary
        // its borrower's lifetime.
        // An implicitly converted temporary (`f(String("abc"))` at a view
        // parameter) materializes inside the conversion lowering instead: the
        // slot holds the unconverted source and the callee receives the view.
        let converted = expression.implicit_conversion().is_some();
        if let Some(owner) = mojito_checked::checked::materialized_borrow_owner(adjustments)
            && !converted
        {
            return self.materialize_borrow_source(expression, owner);
        }
        let retains_place = adjustments.iter().any(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::RetainCallPlace
            ) || (lends_reads
                && !converted
                && matches!(
                    adjustment,
                    mojito_checked::checked::SemanticAdjustment::BorrowReadArgument
                ))
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
            }) && expression.implicit_conversion().is_none()
                && let Some(register) = match setup.filter(|_| !lends_reads) {
                    Some(source) => self.defer_to_setup(expression, Some(source), true, |this| {
                        this.lower_borrowed_read_argument(expression)
                    }),
                    None => self.lower_borrowed_read_argument(expression),
                }
            {
                // A call whose result borrows its reads retains the place as
                // a SHARED read:
                // the ownership analysis classifies a retained place by the
                // callee's declared convention (a non-`mut`/`ref` slot reads),
                // and the VM binds it as a caller-place handle so the view's
                // reference fields root in caller storage. Every other call
                // retains no place: owner liveness through the call comes from
                // the register's place provenance.
                if lends_reads {
                    return (register, self.simple_place(expression));
                }
                return (register, None);
            }
            if let Some(source) = setup
                && self.setup_read(expression)
            {
                return (
                    self.defer_to_setup(expression, Some(source), true, |this| {
                        this.expr(expression)
                    }),
                    None,
                );
            }
            // A reference result's read, and the temporary binding of its
            // copy, are setup work too.
            let read = |this: &mut Self| {
                let value = this.expr(expression);
                this.anchor_temporary_argument(expression, value, adjustments, converted);
                this.bind_read_temporary(expression, value)
            };
            let value = match setup {
                Some(source) => self.defer_to_setup(expression, Some(source), true, read),
                None => read(self),
            };
            return (value, None);
        }

        // A pure root/field place needs no emitted projection state, so keep
        // the existing expression lowering (notably its reference-field
        // handling) and attach the place afterward. A bare aggregate variable
        // is read shallowly instead: the VM rebinds a `mut`/`ref` parameter to
        // the caller place, so a `UseVar` lifecycle copy here would run a user
        // `__copyinit__` only to discard the result.
        if let Some(place) = self.simple_place(expression) {
            if let Some(register) = self.lower_borrowed_read_argument(expression) {
                return (register, Some(place));
            }
            return (self.expr(expression), Some(place));
        }

        if expression.reference_result().is_some() {
            return self.lower_call_receiver(expression);
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

        // A temporary at a place-retaining slot (the builtin `String(x)`
        // conversion retains its argument's place) has no place to retain,
        // so it anchors like any other temporary argument. The checker
        // rejects a non-place actual for a place-requiring parameter; lowering
        // stays total so the verifier can diagnose corrupt checked input
        // without manufacturing a caller place.
        let value = self.expr(expression);
        self.anchor_temporary_argument(expression, value, adjustments, converted);
        (value, None)
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

    #[allow(clippy::ref_option, reason = "TODO: take Option<&T>")]
    pub(super) fn checked_call_source_place(
        contract: &mojito_checked::checked::CheckedCallContract,
        source: mojito_checked::checked::CheckedCallArgumentSource,
        place: &Option<MirPlace>,
    ) -> Option<MirPlace> {
        Self::checked_call_source_requires_place(contract, source)
            .then(|| place.clone())
            .flatten()
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
                } => {
                    let dest = self.fresh_typed(
                        site.clone(),
                        None,
                        parameter_ty.clone().unwrap_or_else(Ty::error),
                    );
                    self.emit(MirInstr::Call {
                        dest,
                        func: FuncRef::named(target),
                        raises: None,
                        args: vec![value],
                        kwargs: Vec::new(),
                        arg_places: vec![None],
                        kwarg_places: Vec::new(),
                        capture_accesses: Vec::new(),
                        param_arg_regs: Vec::new(),
                        receiver: None,
                        instantiated_args: Vec::new(),
                        spread: None,
                    });
                    dest
                }
                mojito_checked::checked::CheckedCallValueAdjustment::IndexNormalization {
                    target,
                } => {
                    let dest = self.fresh_typed(
                        site.clone(),
                        None,
                        parameter_ty.clone().unwrap_or_else(Ty::int),
                    );
                    self.emit(MirInstr::MethodCall {
                        dest,
                        recv: value,
                        method: "__mlir_index__".to_string(),
                        resolved: Some(target.clone()),
                        raises: None,
                        reference_result: None,
                        result_adapter: None,
                        args: Vec::new(),
                        kwargs: Vec::new(),
                        recv_place: None,
                        recv_writes: false,
                        arg_places: Vec::new(),
                        kwarg_places: Vec::new(),
                        capture_accesses: Vec::new(),
                        param_arg_regs: Vec::new(),
                        param_decls: Vec::new(),
                        instantiated_args: Vec::new(),
                        spread: None,
                    });
                    dest
                }
                mojito_checked::checked::CheckedCallValueAdjustment::MaterializeLiteral {
                    target,
                } => self.materialize_literal(value, target, site.clone()),
                mojito_checked::checked::CheckedCallValueAdjustment::SplatScalar { target } => {
                    self.splat_scalar(value, target, site.clone())
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
    /// `show(*args^)`): the caller's collector, read in place and retained
    /// as the argument's place, or moved out of its slot when transferred.
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
            matches!(
                place.ty.as_ref().map(Ty::kind),
                Some(TyKind::VariadicPack(_))
            )
        }) else {
            return (self.expr(expression), None);
        };
        let value = self.fresh_typed(
            operand.location(),
            Some(place.root),
            place.ty.clone().unwrap_or_else(Ty::error),
        );
        self.emit(MirInstr::LoadPlace {
            dest: value,
            place: place.clone(),
        });
        (value, Some(place))
    }

    /// `lends_reads`: see [`Self::lends_read_arguments`].
    pub(super) fn lower_call_arguments(
        &mut self,
        arguments: &[Node<'a>],
        lends_reads: bool,
    ) -> (Vec<Reg>, Vec<Option<MirPlace>>) {
        let mut registers = Vec::with_capacity(arguments.len());
        let mut places = Vec::with_capacity(arguments.len());
        for (index, &argument) in arguments.iter().enumerate() {
            let (register, place) = self.lower_call_argument_with(
                argument,
                lends_reads,
                CheckedCallArgumentSource::Positional(index),
            );
            registers.push(register);
            places.push(place);
        }
        (registers, places)
    }

    /// `lends_reads`: see [`Self::lends_read_arguments`].
    pub(super) fn lower_call_keywords(
        &mut self,
        arguments: &[(&str, Node<'a>)],
        lends_reads: bool,
    ) -> LoweredKeywords {
        let mut registers = Vec::with_capacity(arguments.len());
        let mut places = Vec::with_capacity(arguments.len());
        for (index, &(name, value)) in arguments.iter().enumerate() {
            let (register, place) = self.lower_call_argument_with(
                value,
                lends_reads,
                CheckedCallArgumentSource::Keyword(index),
            );
            registers.push((name.to_string(), register));
            places.push(place);
        }
        (registers, places)
    }

    /// The keyword of a copy construction (`Name(copy=place)`,
    /// `T(copy=place)`), which binds the copy initializer's borrowed
    /// `copy: Self` parameter: a place source is read shallowly and retained,
    /// so the copy initializer runs on the live source exactly once, where an
    /// ordinary value read would run a user's copy initializer a second time
    /// for the argument itself — observable through its side effects. `None`
    /// for any other keyword list or a source that is not a simple place.
    pub(super) fn copy_construction_keywords(
        &mut self,
        arguments: &[(&str, Node<'a>)],
    ) -> Option<LoweredKeywords> {
        let [("copy", value)] = arguments else {
            return None;
        };
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
        Some((vec![("copy".to_string(), source)], vec![Some(place)]))
    }

    /// Fold a keyword `@fieldwise_init` construction into its positional form:
    /// the arguments were evaluated in source order, and the call lists their
    /// registers in field order, so no backend binds keywords to fields. A
    /// call on any other target is returned unchanged.
    pub(super) fn fieldwise_keywords_positional(
        &self,
        constructed: &str,
        call: FieldwiseCallArguments,
    ) -> FieldwiseCallArguments {
        let (regs, places, keywords, keyword_places) = call;
        if keywords.is_empty() {
            return (regs, places, keywords, keyword_places);
        }
        let keyword_names: Vec<&str> = keywords.iter().map(|(name, _)| name.as_str()).collect();
        let Some(slots) = self
            .overloads
            .fieldwise_parameters(constructed)
            .and_then(|names| {
                mojito_ast::call::match_fieldwise_slots(&names, regs.len(), &keyword_names).ok()
            })
        else {
            return (regs, places, keywords, keyword_places);
        };
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
        (ordered_regs, ordered_places, Vec::new(), Vec::new())
    }

    /// Store an accessor-produced reference in a hidden local and establish its
    /// checked owner loans.  This turns the handle into the same persistent,
    /// analyzable call-place representation as an explicit `ref` binding while
    /// evaluating the accessor exactly once. A checked owner with no MIR slot
    /// of its own lends through `fallback`, the storage the accessor reads.
    pub(super) fn materialize_call_reference_place(
        &mut self,
        expression: Node<'a>,
        handle: Reg,
        reference: &mojito_types::origin::RefTy,
        fallback: Option<VarId>,
        setup: bool,
    ) -> MirPlace {
        let variable = self.var(&format!("$call_ref_r{}", handle.0));
        let storage_ty = Ty::new(TyKind::Ref(reference.clone()));
        self.var_types.insert(variable, storage_ty.clone());
        self.runtime_aliases.insert(variable);
        self.emit(MirInstr::DefVar {
            var: variable,
            src: handle,
            binding_ty: Some(storage_ty.clone()),
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
                mutable: reference.mutability == mojito_types::origin::Mutability::Mutable,
                interior,
                shared: false,
                setup,
            });
        }
        // A setup read also keeps the accessor chain's root receiver live:
        // a view's referent (`Span.__getitem__`) is reached through it.
        if setup
            && let Some(receiver) = std::iter::successors(expression.object(), |node| node.object())
                .find_map(|node| self.simple_place(node))
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
        let place = self.materialize_call_reference_place(
            expression,
            handle,
            &materialized_reference,
            None,
            setup,
        );
        if setup && let Some(pending) = self.setup_reference.as_mut() {
            pending.read_start = Some((self.cur, self.f.blocks[self.cur].instrs.len()));
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
                if let Some(ty) = expression.place_ty().or_else(|| expression.ty()) {
                    place.project(Proj::Field(field.clone()), ty);
                } else {
                    place.proj.push(Proj::Field(field.clone()));
                }
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
                                | MirIntrinsicSubscript::Simd
                                | MirIntrinsicSubscript::Pointer
                        )
                    ) =>
            {
                let mut place = base_place(self, object)?;
                let projection = match object.ty().as_ref().map(Ty::kind) {
                    Some(TyKind::Tuple(_)) => index
                        .exact_nonnegative_index()
                        .map_or_else(|| Proj::Index(self.expr(index)), Proj::ConstIndex),
                    _ => Proj::Index(self.expr(index)),
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
            && matches!(
                expression.ty().as_ref().map(Ty::kind),
                Some(TyKind::Struct(..))
            )
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
        // any shadow spelling, a module constant, or, in a lifted thunk, a
        // local `comptime` binding of a parameter expression (`lane` of
        // `comptime lane = dt`), is a value the elaborator folds, not a
        // place: the receiver is read as such and no place is retained.
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
                }))
        {
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

    /// [`Self::lower_call_receiver`] for the receiver of method call `call`,
    /// giving a fresh owned temporary receiver (`B(3).show()`) frame storage:
    /// the value is bound to a hidden `$tmp_recv_r` slot and the call receives
    /// that slot's place, exactly as a named receiver retains its own. The
    /// call is the slot's last use, so drop elaboration destroys the
    /// temporary right after it returns — upstream's ASAP destruction. A
    /// `var`/`deinit` receiver convention binds no slot: the callee owns the
    /// temporary and tears it down itself. A value that owns no droppable
    /// storage, or one the receiver paths already gave a place, is returned
    /// unchanged.
    pub(super) fn lower_method_receiver(
        &mut self,
        call: Node<'a>,
        receiver: Node<'a>,
    ) -> (Reg, Option<MirPlace>) {
        let (value, place) = self.lower_call_receiver(receiver);
        if place.is_some()
            || !receiver.is_call()
            || call.call_contract().is_some_and(|contract| {
                matches!(
                    contract.receiver_convention,
                    Some(
                        mojito_ast::ast::ArgConvention::Var
                            | mojito_ast::ast::ArgConvention::Deinit
                    )
                )
            })
        {
            return (value, place);
        }
        let Some(ty) = self
            .f
            .reg_types
            .get(&value.0)
            .cloned()
            .or_else(|| receiver.ty())
            .filter(owns_droppable_storage)
        else {
            return (value, None);
        };
        let variable = self.var(&format!("$tmp_recv_r{}", value.0));
        self.var_types.insert(variable, ty.clone());
        self.emit(MirInstr::DefVar {
            var: variable,
            src: value,
            binding_ty: Some(ty.clone()),
        });
        (value, Some(MirPlace::root(variable, Some(ty))))
    }

    /// Lower method call `call`'s receiver as [`Self::lower_method_receiver`]
    /// does, copying a consuming receiver when `copied`, and open the call's
    /// setup frame ([`Self::open_call_setup`]). A receiver the method reads
    /// or owns by value is read at setup, after the arguments, as Mojo's
    /// call setup reads it; a `mut`/`ref`/`deinit` receiver binds its place
    /// and stays where written.
    pub(super) fn lower_setup_receiver(
        &mut self,
        call: Node<'a>,
        receiver: Node<'a>,
        copied: bool,
    ) -> (Reg, Option<MirPlace>) {
        self.lower_setup_receiver_with(call, receiver, |this| {
            let (value, place) = this.lower_method_receiver(call, receiver);
            let value = if copied {
                this.copy_consuming_receiver(receiver, value, place.as_ref())
            } else {
                value
            };
            (value, place)
        })
    }

    /// [`Self::lower_setup_receiver`] with the receiver lowered by `lower`:
    /// a subscript's accessor receiver goes through
    /// [`Self::lower_call_receiver`] alone.
    pub(super) fn lower_setup_receiver_with(
        &mut self,
        call: Node<'a>,
        receiver: Node<'a>,
        lower: impl FnOnce(&mut Self) -> (Reg, Option<MirPlace>),
    ) -> (Reg, Option<MirPlace>) {
        let by_value = call.call_contract().is_none_or(|contract| {
            matches!(
                contract.receiver_convention,
                None | Some(
                    mojito_ast::ast::ArgConvention::Imm | mojito_ast::ast::ArgConvention::Var
                )
            )
        });
        if by_value {
            self.open_call_setup();
            return self.lower_setup_operand(receiver, None, false, lower);
        }
        let lowered = lower(self);
        self.open_call_setup();
        lowered
    }

    /// Read a field (`B(1).x`) off a fresh owned temporary through frame
    /// storage: bind the temporary to a hidden `$tmp_field_r` slot and load
    /// the projected place, exactly as `p.x` reads off a named local. The
    /// load keeps the slot alive through the instructions that consume it
    /// (`print(B(1).x)` destroys the temporary after `print`), so drop
    /// elaboration runs the temporary's destructor where the pinned Mojo
    /// does; a consuming context's checked `CopyPlaceValue` copies an owning
    /// field out before the slot dies, as it does for a named local's. `None`
    /// — leaving the register-based `GetField` — when the base is not a
    /// loan-free call or operator temporary with droppable storage, when the base is a
    /// slice descriptor, or when the field is a reference.
    pub(super) fn load_temporary_field(
        &mut self,
        expression: Node<'a>,
        object: Node<'a>,
        field: &str,
    ) -> Option<Reg> {
        if !object.is_fresh_temporary()
            || object.reference_result().is_some()
            || mojito_checked::checked::materialized_borrow_owner(object.adjustments()).is_some()
        {
            return None;
        }
        let field_ty = expression.ty()?;
        if matches!(field_ty.kind(), TyKind::Ref(_)) {
            return None;
        }
        // A slice descriptor's `start`/`end`/`step` are intrinsic bounds on a
        // `Slice` value, not struct fields: only the register-based `GetField`
        // mints their nominal `Optional`, so a place read of one has nothing
        // to navigate. The `Member` place branch in `lower_expr::expr` carves
        // out the same set, and the two must agree.
        if object.is_slice_descriptor() {
            return None;
        }
        let object_ty = object.ty().filter(owns_droppable_storage)?;
        if !self.aggregate_borrows(object).is_empty() {
            return None;
        }
        let value = self.expr(object);
        let variable = self.var(&format!("$tmp_field_r{}", value.0));
        self.var_types.insert(variable, object_ty.clone());
        self.emit(MirInstr::DefVar {
            var: variable,
            src: value,
            binding_ty: Some(object_ty.clone()),
        });
        let mut place = MirPlace::root(variable, Some(object_ty));
        place.project(Proj::Field(field.to_string()), field_ty.clone());
        let loaded = self.fresh_typed(expression.location(), Some(variable), field_ty);
        self.emit(MirInstr::LoadPlace {
            dest: loaded,
            place,
        });
        Some(loaded)
    }

    /// Give a loan-free owned temporary bound to a read parameter
    /// (`take(B(2))`, or the `String` an implicit conversion builds for
    /// `take("abc")`) frame storage, as [`Self::lower_method_receiver`] does
    /// for a receiver: bind it to a hidden `$tmp_arg_r` slot and hand the call
    /// a shallow read of that slot — the shape a named local takes at a read
    /// parameter. The loaded register retains the slot through the call, so
    /// drop elaboration destroys the temporary once the callee returns. A
    /// temporary that borrows other storage already owns its `$arg_loan_r`
    /// anchor, a materialized borrow source its `$mat_r` slot, and a value
    /// without droppable storage needs none: those return `value` unchanged.
    fn bind_temporary_argument(
        &mut self,
        expression: Node<'a>,
        value: Reg,
        adjustments: &[mojito_checked::checked::SemanticAdjustment],
        converted: bool,
    ) -> Reg {
        let fresh_temporary = converted || expression.is_fresh_temporary();
        if !fresh_temporary
            || mojito_checked::checked::materialized_borrow_owner(adjustments).is_some()
            || !self.aggregate_borrows(expression).is_empty()
        {
            return value;
        }
        // A view over other storage (`opt.value()` of a span) names an
        // origin: it owns nothing to destroy, and its register's provenance
        // keeps the viewed storage alive through the call.
        let Some(ty) = self
            .f
            .reg_types
            .get(&value.0)
            .cloned()
            .filter(owns_droppable_storage)
            .filter(|ty| mojito_types::types::erase_origin_arguments(ty) == *ty)
        else {
            return value;
        };
        let variable = self.var(&format!("$tmp_arg_r{}", value.0));
        self.var_types.insert(variable, ty.clone());
        self.emit(MirInstr::DefVar {
            var: variable,
            src: value,
            binding_ty: Some(ty.clone()),
        });
        let read = self.fresh_typed(expression.location(), Some(variable), ty.clone());
        self.emit(MirInstr::LoadPlace {
            dest: read,
            place: MirPlace::root(variable, Some(ty)),
        });
        read
    }

    /// The place of a compile-time binding's read the checker materialized
    /// (a tuple or struct value parameter holding a `String`): its hidden
    /// `$mat_r` slot, which the read stores a copy of the binding into.
    pub(super) fn materialized_parameter_place(
        &mut self,
        identifier: Node<'a>,
    ) -> Option<MirPlace> {
        let owner = mojito_checked::checked::materialized_borrow_owner(identifier.adjustments())?;
        self.materialize_borrow_source(identifier, owner).1
    }

    /// Store a materialized borrow-source temporary in its hidden slot
    /// (`$mat_r`, registered under the checker-minted owner) and return the
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
        self.materialize_borrow_slot(expression, owner, value)
    }

    /// Bind an already-lowered temporary `value` into its owner's hidden
    /// `$mat_r` slot (once per owner) and return the value plus the slot's
    /// place. The conversion lowering uses this with the *unconverted* source
    /// so the slot holds what the view constructor borrows, not the view.
    pub(super) fn materialize_borrow_slot(
        &mut self,
        expression: Node<'a>,
        owner: mojito_types::origin::OwnerId,
        value: Reg,
    ) -> (Reg, Option<MirPlace>) {
        let variable = if let Some(variable) = self.owner_vars.get(&owner).copied() {
            variable
        } else {
            let ty = self
                .f
                .reg_types
                .get(&value.0)
                .cloned()
                .or_else(|| expression.ty());
            let variable = self.var(&format!("$mat_r{}", value.0));
            if let Some(ty) = ty.clone() {
                self.var_types.insert(variable, ty);
            }
            self.emit(MirInstr::DefVar {
                var: variable,
                src: value,
                binding_ty: ty,
            });
            self.owner_vars.insert(owner, variable);
            // The slot owns the temporary, and the temporary may itself view
            // other storage (`StringSpan(s)`, a view-returning call): the
            // slot carries those loans, so the viewed storage outlives every
            // borrower of the slot rather than dying at the temporary's call.
            let loans = self.aggregate_borrows_unmaterialized(expression);
            if let Some(first) = loans.first() {
                let marker =
                    self.fresh_typed(expression.location(), Some(first.place.root), Ty::none());
                self.aggregate_loans.insert(variable, loans.clone());
                self.emit(MirInstr::EstablishLoans {
                    reference: variable,
                    loans,
                    marker,
                    dest_interior: None,
                });
            }
            variable
        };
        let place = MirPlace::root(variable, self.var_types.get(&variable).cloned());
        // A reference-context lowering already handed back a handle to the
        // slot, which the call binds as it stands.
        if matches!(
            self.f.reg_types.get(&value.0).map(Ty::kind),
            Some(TyKind::Ref(_))
        ) {
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

    /// Bind the result of a call whose value no expression consumes (an
    /// expression statement; the unbound result of a non-consuming
    /// `__enter__`) to a hidden `$discard_r` slot. Mojo destroys such a
    /// temporary before the next statement: the slot is dead at its
    /// definition, so drop elaboration splices its `DropVar` right after the
    /// call. Only a fresh owned aggregate is bound — a reference result,
    /// scalar, or pointer owns no storage, and a place read is never re-owned.
    pub(super) fn bind_discarded_result(
        &mut self,
        expression: &'a mojito_hir::hir::HirExpr,
        value: Reg,
    ) {
        use mojito_checked::checked::{CheckedOp, SemanticAdjustment};
        let Some(node) = self.checked.expression(expression.node) else {
            return;
        };
        if !matches!(
            node.op,
            CheckedOp::Call { .. } | CheckedOp::MethodCall { .. } | CheckedOp::Invoke { .. }
        ) {
            return;
        }
        if node.adjustments.iter().any(|adjustment| {
            matches!(
                adjustment,
                SemanticAdjustment::SelectedCall(contract) if contract.reference_result.is_some()
            )
        }) {
            return;
        }
        let Some(ty) = self
            .f
            .reg_types
            .get(&value.0)
            .cloned()
            .or_else(|| node.ty.clone())
        else {
            return;
        };
        if !owns_droppable_storage(&ty) {
            return;
        }
        let variable = self.var(&format!("$discard_r{}", value.0));
        self.var_types.insert(variable, ty.clone());
        self.emit(MirInstr::DefVar {
            var: variable,
            src: value,
            binding_ty: Some(ty),
        });
    }

    /// Anchor a loan-carrying temporary *argument* in a hidden retained slot
    /// (`$arg_loan_r`) whose loans keep its borrowed sources alive through the
    /// consuming call. Unlike the receiver anchor below, the value is NOT read
    /// back out of the slot — a native re-read would run the aggregate's
    /// lifecycle copy where the VM's shallow read does not. Instead the
    /// original register keeps carrying the value (`DefVar` clones on both
    /// backends), and the statement-end `KeepAlive` flush (the temporary's
    /// upstream lifetime is the full statement) extends the slot's — and so
    /// the loans' — liveness across the call.
    /// Anchor a temporary argument that borrows caller storage (a
    /// constructor or call result holding references/pointers into live
    /// places) in a hidden slot, as a chained view receiver is, or its
    /// sources are dropped before the consuming call runs. Bare
    /// reference/pointer handles stay unanchored: a `LoadPlace` read out of
    /// the hidden slot would dereference the handle.
    /// Scope: only a plain `Call` or method-call temporary (a view result
    /// such as `s.strip()`) in a call's argument list anchors (see
    /// `allow_argument_anchors`) — every other consumer carries the
    /// temporary's loans through its own channel, and an extra anchor is a
    /// conflicting duplicate borrow.
    /// A subscript view temporary (`s[byte=a:b]`, a `Span` slice) borrows its
    /// source through the subscript's own borrow fact and no consumer channel
    /// retains it — a plain call, a method call, and a construction alike
    /// would drop the source before the call runs when the argument is the
    /// source's last use — so it anchors in every argument list: one loan on
    /// the source, never a duplicate of a channel that does not exist.
    pub(super) fn anchor_temporary_argument(
        &mut self,
        expression: Node<'a>,
        value: Reg,
        adjustments: &[mojito_checked::checked::SemanticAdjustment],
        converted: bool,
    ) {
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
            && matches!(
                expression.ty().as_ref().map(Ty::kind),
                Some(TyKind::Struct(..))
            )
        {
            let loans = self.aggregate_borrows(expression);
            if !loans.is_empty() {
                self.anchor_borrowing_argument(expression, value, loans);
            }
        }
    }

    fn anchor_borrowing_argument(&mut self, expression: Node<'a>, value: Reg, loans: Vec<MirLoan>) {
        let view_ty = self
            .f
            .reg_types
            .get(&value.0)
            .cloned()
            .or_else(|| expression.ty());
        let variable = self.var(&format!("$arg_loan_r{}", value.0));
        if let Some(ty) = view_ty.clone() {
            self.var_types.insert(variable, ty);
        }
        self.emit(MirInstr::DefVar {
            var: variable,
            src: value,
            binding_ty: view_ty,
        });
        let marker = self.fresh_typed(expression.location(), Some(loans[0].place.root), Ty::none());
        self.emit(MirInstr::EstablishLoans {
            reference: variable,
            loans: loans.clone(),
            marker,
            dest_interior: None,
        });
        self.aggregate_loans.insert(variable, loans);
        self.pending_argument_anchors.push(variable);
    }

    /// Bind a loan-carrying temporary into a hidden retained slot whose loans
    /// keep its borrowed sources alive and conflict-checked for the slot's
    /// lifetime, and read the value back out of that slot: the chained view
    /// receiver anchor (`$view_recv_r`). Without the anchor, ownership sees
    /// the borrowed source dead immediately after the producing expression and
    /// drop elaboration frees it before the consuming call runs.
    fn anchor_borrowing_temporary(
        &mut self,
        expression: Node<'a>,
        value: Reg,
        loans: Vec<MirLoan>,
        prefix: &str,
    ) -> (Reg, Option<MirPlace>) {
        let view_ty = self
            .f
            .reg_types
            .get(&value.0)
            .cloned()
            .or_else(|| expression.ty());
        let variable = self.var(&format!("{prefix}{}", value.0));
        if let Some(ty) = view_ty.clone() {
            self.var_types.insert(variable, ty);
        }
        self.emit(MirInstr::DefVar {
            var: variable,
            src: value,
            binding_ty: view_ty.clone(),
        });
        let marker = self.fresh_typed(expression.location(), Some(loans[0].place.root), Ty::none());
        self.emit(MirInstr::EstablishLoans {
            reference: variable,
            loans: loans.clone(),
            marker,
            dest_interior: None,
        });
        self.aggregate_loans.insert(variable, loans);
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
        if !base.is_call() {
            return None;
        }
        let owner = mojito_checked::checked::materialized_borrow_owner(base.adjustments())?;
        self.materialize_borrow_source(base, owner).1
    }

    /// Retain storage for any callable place. Nominal callable receivers use it
    /// for `mut self`; declaration-owned closure environments use it so their
    /// copy/move capture slots are borrowed in place across repeated calls.
    pub(super) fn callable_receiver_place(&mut self, expression: Node<'a>) -> Option<MirPlace> {
        let place = self.simple_place(expression)?;
        place.is_typed().then_some(place)
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
    fn lower_borrowed_read_argument(&mut self, expression: Node<'a>) -> Option<Reg> {
        if let Some((object, _)) = expression.index_operands() {
            return self.lower_borrowed_element_read(expression, object);
        }
        let name = self.name_spelled(expression)?;
        // A module constant read is a value with no slot of its own.
        if expression.resolved_callable().is_some()
            || self.nested_info(expression).is_some()
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
        if !matches!(
            ty.kind(),
            TyKind::Struct(..)
                | TyKind::Tuple(_)
                | TyKind::RuntimePack(_)
                | TyKind::VariadicPack(_)
                | TyKind::Variant(_)
                | TyKind::Param { .. }
                | TyKind::Assoc { .. }
        ) {
            return None;
        }
        let var = self.expression_var(&name, expression);
        if self.aliases.contains_key(&var) || self.runtime_aliases.contains(&var) {
            return None;
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
    /// for the pass. A reference-returning accessor of an owning container
    /// lends its referent through the receiver path. Tuple and pack storage
    /// loads the element place, whatever the element: the ordinary read
    /// would copy the whole storage to index it.
    fn lower_borrowed_element_read(
        &mut self,
        expression: Node<'a>,
        object: Node<'a>,
    ) -> Option<Reg> {
        if expression.adjustments().iter().any(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::CopyPlaceValue
            )
        }) {
            return None;
        }
        if let Some(reference) = expression.reference_result() {
            // A view (`Span[T, origin]`) lends storage it does not own; its
            // element keeps the ordinary read.
            let view = matches!(
                object.ty().as_ref().map(Ty::kind),
                Some(TyKind::Struct(_, arguments))
                    if arguments.iter().any(|argument| matches!(argument, TyArg::Origin(_)))
            );
            return (!view && owns_droppable_storage(&reference.referent))
                .then(|| self.lower_call_receiver(expression).0);
        }
        if expression.call_contract().is_some()
            || !matches!(
                object.intrinsic_index_dispatch(),
                Some(MirIntrinsicSubscript::TupleStorage | MirIntrinsicSubscript::VariadicStorage)
            )
        {
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
        Some(dest)
    }
}
