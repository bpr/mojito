//! Call-site lowering: argument/keyword/receiver lowering, checked-call value
//! adjustments and boundaries, reference-result places, and interior-origin
//! invalidation emission.
//! Extracted from `mir.rs`; see `docs/symbol-map.md`.

use super::*;

impl Flatten<'_> {
    /// Lower one ordinary call argument together with the caller storage that
    /// checking selected for a `mut`/`ref` parameter. Dynamic indexed places
    /// cannot be reconstructed after evaluating their value: doing so either
    /// evaluates the index twice or loses the write-back place altogether.
    /// Flatten those actuals once and retain the resulting typed place. A
    /// nominal accessor-produced reference uses the same hidden handle slot as
    /// a chained method receiver.
    pub(super) fn lower_call_argument(&mut self, expression: &Expr) -> (Reg, Option<MirPlace>) {
        self.lower_call_argument_with(expression, false)
    }

    /// Whether a call expression's checked result is a borrowing view of its
    /// arguments (`BorrowViewResult`): the caller-side loans lend the place
    /// arguments to the result, so a read-convention place argument must
    /// reach the callee as the caller's storage rather than a copy — a view
    /// built over a callee-local copy would dangle once the frame unwinds.
    pub(super) fn borrows_view_result(&self, expression: &Expr) -> bool {
        self.checked_adjustments(expression)
            .iter()
            .any(|adjustment| {
                matches!(
                    adjustment,
                    mojito_checked::checked::SemanticAdjustment::BorrowViewResult
                )
            })
    }

    /// Whether a callable-value (indirect) or method call's argument list
    /// may anchor loan-carrying temporaries — the same rule as a plain
    /// function call (see `allow_argument_anchors`): a callee borrowing
    /// through checker-selected `ref` arguments or recorded transfer effects
    /// carries a temporary's loans through that channel already, so anchoring
    /// there adds a conflicting duplicate borrow. (A plain call additionally
    /// excludes constructions; a method or callable value is never one.)
    pub(super) fn call_anchors_arguments(&self, expression: &Expr) -> bool {
        !(self
            .checked_adjustments(expression)
            .iter()
            .any(|adjustment| {
                matches!(
                    adjustment,
                    mojito_checked::checked::SemanticAdjustment::BorrowRefArguments { .. }
                )
            })
            || self.call_transfers.contains_key(&expression.source_span()))
    }

    /// `view_result`: the enclosing call carries `BorrowViewResult`, so a
    /// shared read of a place argument retains that place (see
    /// [`Self::borrows_view_result`]).
    fn lower_call_argument_with(
        &mut self,
        expression: &Expr,
        view_result: bool,
    ) -> (Reg, Option<MirPlace>) {
        let adjustments = self.checked_adjustments(expression);
        // A temporary bound to a `ref [origin]` parameter: store the value in
        // a hidden slot registered under the checker-minted owner identity and
        // hand the call that slot's place — the VM/native ref binding then
        // borrows real frame storage, and the slot's loans give the temporary
        // its borrower's lifetime.
        if let Some(mojito_checked::checked::SemanticAdjustment::MaterializeBorrowSource {
            owner,
        }) = adjustments.iter().find(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::MaterializeBorrowSource { .. }
            )
        }) {
            let owner = *owner;
            let value = self.expr(expression);
            // The expression lowering may already have materialized the slot
            // (reference-context paths route through `reference_handle`);
            // reuse it rather than storing a second copy.
            let variable = match self.owner_vars.get(&owner).copied() {
                Some(variable) => variable,
                None => {
                    let ty = self
                        .f
                        .reg_types
                        .get(&value.0)
                        .cloned()
                        .or_else(|| self.checked_ty(expression));
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
                    variable
                }
            };
            return (
                value,
                Some(MirPlace::root(
                    variable,
                    self.var_types.get(&variable).cloned(),
                )),
            );
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
            }) && self.implicit_conversion(expression).is_none()
                && let Some(register) = self.lower_borrowed_read_argument(expression)
            {
                // A borrowing-view call retains the place as a SHARED read:
                // the ownership analysis classifies a retained place by the
                // callee's declared convention (a non-`mut`/`ref` slot reads),
                // and the VM binds it as a caller-place handle so the view's
                // reference fields root in caller storage. Every other call
                // retains no place: owner liveness through the call comes from
                // the register's place provenance.
                if view_result {
                    return (register, self.simple_place(expression));
                }
                return (register, None);
            }
            let value = self.expr(expression);
            // A temporary aggregate argument that borrows caller storage (a
            // constructor or call result holding references/pointers into live
            // places) needs the same hidden anchor as a chained view receiver,
            // or its sources are dropped before the consuming call runs. Bare
            // reference/pointer handles stay unanchored: a `LoadPlace` read
            // out of the hidden slot would dereference the handle.
            // Scope: only a plain `Call` or method-call temporary (a view
            // result such as `s.strip()`) in a call's argument list anchors
            // (see `allow_argument_anchors`) — every other consumer carries
            // the temporary's loans through its own channel, and an extra
            // anchor is a conflicting duplicate borrow.
            if self.allow_argument_anchors
                && matches!(
                    expression.kind,
                    ExprKind::Call { .. } | ExprKind::MethodCall { .. }
                )
                && matches!(self.checked_ty(expression), Some(Ty::Struct(..)))
            {
                let loans = self.aggregate_borrows(expression);
                if !loans.is_empty() {
                    self.anchor_borrowing_argument(expression, value, loans);
                }
            }
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

        if self.reference_result(expression).is_some() {
            return self.lower_call_receiver(expression);
        }

        // `container[index].field` is not a raw place when the selected
        // `__getitem__` returns a reference. Evaluate that accessor once into
        // its hidden caller-handle slot, then retain the ordinary projections
        // below the returned referent. Falling through to `try_place` would
        // manufacture `container[Index].field` and bypass the selected call.
        if let Some(place) = self.lower_projected_reference_place(expression) {
            let value = self.fresh_typed(
                expression.source_span(),
                Some(place.root),
                place
                    .ty
                    .clone()
                    .or_else(|| self.checked_ty(expression))
                    .unwrap_or(Ty::Error),
            );
            self.emit(MirInstr::LoadPlace {
                dest: value,
                place: place.clone(),
            });
            return (value, Some(place));
        }

        if let Some(place) = self.try_place(expression) {
            let value = self.fresh_typed(
                expression.source_span(),
                Some(place.root),
                place
                    .ty
                    .clone()
                    .or_else(|| self.checked_ty(expression))
                    .unwrap_or(Ty::Error),
            );
            self.emit(MirInstr::LoadPlace {
                dest: value,
                place: place.clone(),
            });
            return (value, Some(place));
        }

        // The checker rejects a non-place actual for a place-requiring
        // parameter. Keep lowering total so the verifier can diagnose corrupt
        // checked input without manufacturing a caller place.
        (self.expr(expression), None)
    }

    /// Evaluate an augmented-subscript argument before either accessor call,
    /// without applying a conversion selected for one particular accessor.
    /// Getter and setter contracts may adapt the same source expression to
    /// different parameter types, but Mojo still evaluates that expression
    /// exactly once.
    pub(super) fn expr_without_call_value_adjustments(&mut self, expression: &Expr) -> Reg {
        if let Some(reference) = self.reference_result(expression) {
            let handle = self.reference_handle(expression);
            let value_ty = (*reference.referent).clone();
            let read = self.fresh_typed(expression.source_span(), None, value_ty.clone());
            self.emit(MirInstr::ReadRef {
                dest: read,
                reference: handle,
            });
            let copied = self.fresh_typed(expression.source_span(), None, value_ty);
            self.emit(MirInstr::CopyValue {
                dest: copied,
                value: read,
            });
            return copied;
        }

        let value = self.expr_unconverted(expression);
        if let Some(ty) = self.checked_ty(expression) {
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
        expression: &Expr,
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
        if self.reference_result(expression).is_some() {
            return self.lower_call_receiver(expression);
        }
        if let Some(place) = self.lower_projected_reference_place(expression) {
            let value = self.fresh_typed(
                expression.source_span(),
                Some(place.root),
                place
                    .ty
                    .clone()
                    .or_else(|| self.checked_ty(expression))
                    .unwrap_or(Ty::Error),
            );
            self.emit(MirInstr::LoadPlace {
                dest: value,
                place: place.clone(),
            });
            return (value, Some(place));
        }
        if let Some(place) = self.try_place(expression) {
            let value = self.fresh_typed(
                expression.source_span(),
                Some(place.root),
                place
                    .ty
                    .clone()
                    .or_else(|| self.checked_ty(expression))
                    .unwrap_or(Ty::Error),
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
                        parameter_ty.clone().unwrap_or(Ty::Error),
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
                        parameter_ty.clone().unwrap_or(Ty::Error),
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
                    });
                    dest
                }
                mojito_checked::checked::CheckedCallValueAdjustment::IndexNormalization {
                    target,
                } => {
                    let dest = self.fresh_typed(
                        site.clone(),
                        None,
                        parameter_ty.clone().unwrap_or(Ty::Int),
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
                    });
                    dest
                }
                mojito_checked::checked::CheckedCallValueAdjustment::MaterializeLiteral {
                    target,
                } => {
                    let target = target.as_ref().clone();
                    let dest = self.fresh_typed(site.clone(), None, target.clone());
                    self.emit(MirInstr::MaterializeLiteral {
                        dest,
                        value,
                        target,
                    });
                    dest
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
        site: SourceSpan,
    ) {
        for argument in &contract.boundary.arguments {
            self.emit_interior_invalidation_facts(
                &argument.invalidations,
                argument.value_source.clone(),
                None,
            );
        }
        self.emit_interior_invalidation_facts(&contract.boundary.invalidations, site, None);
    }

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
            place.ty.clone().unwrap_or(Ty::Error),
        );
        self.emit(MirInstr::LoadPlace {
            dest: value,
            place: place.clone(),
        });
        value
    }

    /// `view_result`: see [`Self::borrows_view_result`].
    pub(super) fn lower_call_arguments(
        &mut self,
        arguments: &[Expr],
        view_result: bool,
    ) -> (Vec<Reg>, Vec<Option<MirPlace>>) {
        let mut registers = Vec::with_capacity(arguments.len());
        let mut places = Vec::with_capacity(arguments.len());
        for argument in arguments {
            let (register, place) = self.lower_call_argument_with(argument, view_result);
            registers.push(register);
            places.push(place);
        }
        (registers, places)
    }

    /// `view_result`: see [`Self::borrows_view_result`].
    pub(super) fn lower_call_keywords(
        &mut self,
        arguments: &[mojito_ast::ast::KwArg],
        view_result: bool,
    ) -> (Vec<(String, Reg)>, Vec<Option<MirPlace>>) {
        let mut registers = Vec::with_capacity(arguments.len());
        let mut places = Vec::with_capacity(arguments.len());
        for argument in arguments {
            let (register, place) = self.lower_call_argument_with(&argument.value, view_result);
            registers.push((argument.name.clone(), register));
            places.push(place);
        }
        (registers, places)
    }

    /// Store an accessor-produced reference in a hidden local and establish its
    /// checked owner loans.  This turns the handle into the same persistent,
    /// analyzable call-place representation as an explicit `ref` binding while
    /// evaluating the accessor exactly once.
    pub(super) fn materialize_call_reference_place(
        &mut self,
        expression: &Expr,
        handle: Reg,
        reference: mojito_types::origin::RefTy,
    ) -> MirPlace {
        let variable = self.var(&format!("$call_ref_r{}", handle.0));
        let storage_ty = Ty::Ref(reference.clone());
        self.var_types.insert(variable, storage_ty.clone());
        self.runtime_aliases.insert(variable);
        self.emit(MirInstr::DefVar {
            var: variable,
            src: handle,
            binding_ty: Some(storage_ty.clone()),
        });

        let mut loans = Vec::new();
        for origin in self.checked_reference_places(expression) {
            let Some(canonical) = self.mir_interior_origin(&origin, None) else {
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
            });
        }
        if !loans.is_empty() {
            let marker = self.fresh_typed(
                expression.source_span(),
                Some(loans[0].place.root),
                Ty::None,
            );
            self.emit(MirInstr::EstablishLoans {
                reference: variable,
                loans: loans.clone(),
                marker,
                dest_interior: None,
            });
            self.aggregate_loans.insert(variable, loans);
        }

        let mut place = MirPlace::root(variable, Some(storage_ty));
        place.ty = Some((*reference.referent).clone());
        place.through = Some(variable);
        place
    }

    /// Materialize one checker-selected reference-returning expression as a
    /// stable hidden caller place without reading its referent. Projection
    /// chains reuse this handle, so the selected accessor is evaluated exactly
    /// once and the VM never has to reinterpret a nominal index as raw storage.
    pub(super) fn materialize_reference_result_place(
        &mut self,
        expression: &Expr,
    ) -> Option<MirPlace> {
        let reference = self.reference_result(expression)?;
        let handle = self.reference_handle(expression);
        // `reference_handle` may peel an outer reference-valued aggregate layer.
        // Materialize the handle's actual type rather than recreating `ref ref T`.
        let materialized_reference = match self.f.reg_types.get(&handle.0) {
            Some(Ty::Ref(reference)) => reference.clone(),
            _ => reference,
        };
        Some(self.materialize_call_reference_place(expression, handle, materialized_reference))
    }

    /// Lower ordinary field/intrinsic-index projections whose base is produced
    /// by a reference-returning call. Nominal index steps keep their own checked
    /// call path and are deliberately not generalized into raw projections.
    pub(super) fn lower_projected_reference_place(
        &mut self,
        expression: &Expr,
    ) -> Option<MirPlace> {
        let base_place = |this: &mut Self, base: &Expr| {
            if this.reference_result(base).is_some() {
                this.materialize_reference_result_place(base)
            } else {
                this.lower_projected_reference_place(base)
            }
        };
        match &expression.kind {
            ExprKind::Member { object, field } => {
                let mut place = base_place(self, object)?;
                if let Some(ty) = self
                    .checked_place_ty(expression)
                    .or_else(|| self.checked_ty(expression))
                {
                    place.project(Proj::Field(field.clone()), ty);
                } else {
                    place.proj.push(Proj::Field(field.clone()));
                }
                Some(place)
            }
            ExprKind::Index { object, index }
                if self.checked_call_contract(expression).is_none()
                    && matches!(
                        self.intrinsic_index_dispatch(object),
                        Some(
                            MirIntrinsicSubscript::TupleStorage
                                | MirIntrinsicSubscript::VariadicStorage
                                | MirIntrinsicSubscript::Simd
                                | MirIntrinsicSubscript::Pointer
                        )
                    ) =>
            {
                let mut place = base_place(self, object)?;
                let projection = match self.checked_ty(object) {
                    Some(Ty::Tuple(_)) => exact_nonnegative_index(index)
                        .map(Proj::ConstIndex)
                        .unwrap_or_else(|| Proj::Index(self.expr(index))),
                    _ => Proj::Index(self.expr(index)),
                };
                if let Some(ty) = self
                    .checked_place_ty(expression)
                    .or_else(|| self.checked_ty(expression))
                {
                    place.project(projection, ty);
                } else {
                    place.proj.push(projection);
                }
                Some(place)
            }
            _ => None,
        }
    }

    /// Whether a checked method call writes through its receiver place. The
    /// checker's effective receiver convention already collapses a `ref self`
    /// reached through an immutable reference to `Imm`; a call without a
    /// checked contract keeps the conservative exclusive classification.
    pub(super) fn receiver_writes(&self, expression: &Expr) -> bool {
        self.checked_call_contract(expression)
            .is_none_or(|contract| {
                matches!(
                    contract.receiver_convention,
                    Some(
                        mojito_ast::ast::ArgConvention::Mut
                            | mojito_ast::ast::ArgConvention::Ref
                            | mojito_ast::ast::ArgConvention::Var
                            | mojito_ast::ast::ArgConvention::Deinit
                    )
                )
            })
    }

    /// Evaluate a call receiver and retain its executable place when checking
    /// selected reference/write-back semantics. Accessor-produced references
    /// become hidden reference locals; value-returning accessors remain values
    /// and are never reconstructed as raw index projections.
    pub(super) fn lower_call_receiver(&mut self, expression: &Expr) -> (Reg, Option<MirPlace>) {
        if let Some(place) = self.materialize_reference_result_place(expression) {
            let value = self.fresh_typed(
                expression.source_span(),
                Some(place.root),
                place.ty.clone().unwrap_or(Ty::Error),
            );
            self.emit(MirInstr::LoadPlace {
                dest: value,
                place: place.clone(),
            });
            return (value, Some(place));
        }
        if let Some(place) = self.lower_projected_reference_place(expression) {
            let value = self.fresh_typed(
                expression.source_span(),
                Some(place.root),
                place
                    .ty
                    .clone()
                    .or_else(|| self.checked_ty(expression))
                    .unwrap_or(Ty::Error),
            );
            self.emit(MirInstr::LoadPlace {
                dest: value,
                place: place.clone(),
            });
            return (value, Some(place));
        }
        if self.checked_call_contract(expression).is_some() {
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
        match self.try_place(expression) {
            Some(place) => {
                let value = self.fresh(expression.source_span(), Some(place.root));
                self.emit(MirInstr::LoadPlace {
                    dest: value,
                    place: place.clone(),
                });
                (value, Some(place))
            }
            None => (self.expr(expression), None),
        }
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
    fn anchor_borrowing_argument(&mut self, expression: &Expr, value: Reg, loans: Vec<MirLoan>) {
        let view_ty = self
            .f
            .reg_types
            .get(&value.0)
            .cloned()
            .or_else(|| self.checked_ty(expression));
        let variable = self.var(&format!("$arg_loan_r{}", value.0));
        if let Some(ty) = view_ty.clone() {
            self.var_types.insert(variable, ty);
        }
        self.emit(MirInstr::DefVar {
            var: variable,
            src: value,
            binding_ty: view_ty,
        });
        let marker = self.fresh_typed(
            expression.source_span(),
            Some(loans[0].place.root),
            Ty::None,
        );
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
        expression: &Expr,
        value: Reg,
        loans: Vec<MirLoan>,
        prefix: &str,
    ) -> (Reg, Option<MirPlace>) {
        let view_ty = self
            .f
            .reg_types
            .get(&value.0)
            .cloned()
            .or_else(|| self.checked_ty(expression));
        let variable = self.var(&format!("{prefix}{}", value.0));
        if let Some(ty) = view_ty.clone() {
            self.var_types.insert(variable, ty);
        }
        self.emit(MirInstr::DefVar {
            var: variable,
            src: value,
            binding_ty: view_ty.clone(),
        });
        let marker = self.fresh_typed(
            expression.source_span(),
            Some(loans[0].place.root),
            Ty::None,
        );
        self.emit(MirInstr::EstablishLoans {
            reference: variable,
            loans: loans.clone(),
            marker,
            dest_interior: None,
        });
        self.aggregate_loans.insert(variable, loans);
        let place = MirPlace::root(variable, view_ty);
        let read = self.fresh_typed(
            expression.source_span(),
            Some(variable),
            place.ty.clone().unwrap_or(Ty::Error),
        );
        self.emit(MirInstr::LoadPlace {
            dest: read,
            place: place.clone(),
        });
        (read, Some(place))
    }

    /// Retain storage for any callable place. Nominal callable receivers use it
    /// for `mut self`; declaration-owned closure environments use it so their
    /// copy/move capture slots are borrowed in place across repeated calls.
    pub(super) fn callable_receiver_place(&mut self, expression: &Expr) -> Option<MirPlace> {
        let place = self.simple_place(expression)?;
        place.is_typed().then_some(place)
    }

    pub(super) fn checked_reference_places(
        &self,
        expression: &Expr,
    ) -> Vec<mojito_types::origin::OriginPlace> {
        fn collect(
            origin: &mojito_types::origin::Origin,
            places: &mut Vec<mojito_types::origin::OriginPlace>,
        ) {
            match origin {
                mojito_types::origin::Origin::Place(place) => places.push(place.clone()),
                mojito_types::origin::Origin::Union(members) => {
                    for member in members {
                        collect(member, places);
                    }
                }
                _ => {}
            }
        }

        let mut places = Vec::new();
        for adjustment in self.checked_adjustments(expression) {
            match adjustment {
                mojito_checked::checked::SemanticAdjustment::InteriorReference { origin } => {
                    places.push(origin)
                }
                mojito_checked::checked::SemanticAdjustment::ReferenceResult { reference } => {
                    collect(&reference.origin, &mut places);
                }
                _ => {}
            }
        }
        if let Some(Ty::Ref(reference)) = self.checked_ty(expression) {
            collect(&reference.origin, &mut places);
        }
        places.sort();
        places.dedup();
        places
    }

    pub(super) fn checked_interior_references(
        &self,
        expression: &Expr,
    ) -> Vec<mojito_types::origin::OriginPlace> {
        self.checked_reference_places(expression)
            .into_iter()
            .filter(|place| {
                place.path.iter().any(|segment| {
                    matches!(
                        segment,
                        mojito_types::origin::OriginSeg::Interior(_)
                            | mojito_types::origin::OriginSeg::Subtree
                    )
                })
            })
            .collect()
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

    pub(super) fn checked_interior_invalidations(
        &self,
        expression: &Expr,
    ) -> Vec<mojito_checked::checked::InteriorInvalidation> {
        self.checked_adjustments(expression)
            .into_iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::InvalidateInteriors {
                    invalidations,
                } => Some(invalidations),
                _ => None,
            })
            .unwrap_or_default()
    }

    /// Emit checker-selected invalidations at the precise operation boundary.
    /// `fallback` is used only for a whole-binding redefinition whose target
    /// owner has no expression occurrence before this instruction.
    pub(super) fn emit_interior_invalidations(
        &mut self,
        expression: &Expr,
        fallback: Option<VarId>,
    ) {
        let invalidations = self.checked_interior_invalidations(expression);
        self.emit_interior_invalidation_facts(&invalidations, expression.source_span(), fallback);
    }

    pub(super) fn emit_interior_invalidation_facts(
        &mut self,
        invalidations: &[mojito_checked::checked::InteriorInvalidation],
        site: SourceSpan,
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
            let marker = self.fresh_typed(site.clone(), Some(base.root), Ty::None);
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
        call: &Expr,
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) {
        for argument in args {
            self.emit_interior_invalidations(argument, None);
        }
        for argument in kwargs {
            self.emit_interior_invalidations(&argument.value, None);
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
    fn lower_borrowed_read_argument(&mut self, expression: &Expr) -> Option<Reg> {
        let ExprKind::Identifier(name) = &expression.kind else {
            return None;
        };
        if self.resolved_callable(expression).is_some()
            || self.nested_info(expression).is_some()
            || self.is_origin_bearing_pointer(expression)
            || (!self.vars.iter().any(|candidate| candidate == name)
                && self.overloads.is_function(name))
        {
            return None;
        }
        if self
            .checked_adjustments(expression)
            .iter()
            .any(|adjustment| {
                matches!(
                    adjustment,
                    mojito_checked::checked::SemanticAdjustment::CopyPlaceValue
                )
            })
        {
            return None;
        }
        let ty = self.checked_ty(expression)?;
        if !matches!(
            ty,
            Ty::Struct(..)
                | Ty::Tuple(_)
                | Ty::RuntimePack(_)
                | Ty::Variant(_)
                | Ty::Param { .. }
                | Ty::Assoc { .. }
        ) {
            return None;
        }
        let var = self.expression_var(name, expression);
        if self.aliases.contains_key(&var) || self.runtime_aliases.contains(&var) {
            return None;
        }
        let place = self.simple_place(expression)?;
        let dest = self.fresh_typed(
            expression.source_span(),
            Some(place.root),
            place.ty.clone().unwrap_or(ty),
        );
        self.emit(MirInstr::LoadPlace { dest, place });
        Some(dest)
    }
}
