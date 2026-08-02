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
        let retains_place = self
            .checked_adjustments(expression)
            .iter()
            .any(|adjustment| matches!(adjustment, crate::SemanticAdjustment::RetainCallPlace));
        if !retains_place {
            return (self.expr(expression), None);
        }

        // A pure root/field place needs no emitted projection state, so keep
        // the existing expression lowering (notably its reference-field
        // handling) and attach the place afterward.
        if let Some(place) = self.simple_place(expression) {
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
        contract: &crate::checked::CheckedCallContract,
        source: crate::checked::CheckedCallArgumentSource,
    ) -> bool {
        contract
            .arguments
            .iter()
            .any(|argument| argument.source == source && argument.requires_place)
    }

    pub(super) fn checked_call_source_mutates(
        contract: &crate::checked::CheckedCallContract,
        source: crate::checked::CheckedCallArgumentSource,
    ) -> bool {
        contract.arguments.iter().any(|argument| {
            argument.source == source
                && matches!(
                    argument.convention,
                    Some(
                        crate::ast::ArgConvention::Mut
                            | crate::ast::ArgConvention::Ref
                            | crate::ast::ArgConvention::Out
                    )
                )
        })
    }

    pub(super) fn checked_call_source_place(
        contract: &crate::checked::CheckedCallContract,
        source: crate::checked::CheckedCallArgumentSource,
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
        contract: &crate::checked::CheckedCallContract,
        source: crate::checked::CheckedCallArgumentSource,
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
                crate::checked::CheckedCallValueAdjustment::ResolveCallable { target } => {
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
                crate::checked::CheckedCallValueAdjustment::ImplicitConversion { target } => {
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
                crate::checked::CheckedCallValueAdjustment::IndexNormalization { target } => {
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
                        arg_places: Vec::new(),
                        kwarg_places: Vec::new(),
                        capture_accesses: Vec::new(),
                        param_arg_regs: Vec::new(),
                        param_decls: Vec::new(),
                    });
                    dest
                }
                crate::checked::CheckedCallValueAdjustment::MaterializeLiteral { target } => {
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
        contract: &crate::checked::CheckedCallContract,
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

    pub(super) fn lower_call_arguments(
        &mut self,
        arguments: &[Expr],
    ) -> (Vec<Reg>, Vec<Option<MirPlace>>) {
        let mut registers = Vec::with_capacity(arguments.len());
        let mut places = Vec::with_capacity(arguments.len());
        for argument in arguments {
            let (register, place) = self.lower_call_argument(argument);
            registers.push(register);
            places.push(place);
        }
        (registers, places)
    }

    pub(super) fn lower_call_keywords(
        &mut self,
        arguments: &[crate::ast::KwArg],
    ) -> (Vec<(String, Reg)>, Vec<Option<MirPlace>>) {
        let mut registers = Vec::with_capacity(arguments.len());
        let mut places = Vec::with_capacity(arguments.len());
        for argument in arguments {
            let (register, place) = self.lower_call_argument(&argument.value);
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
        reference: crate::origin::RefTy,
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
                .any(|segment| matches!(segment, crate::origin::OriginSeg::Interior(_)))
                .then_some(canonical.clone());
            loans.push(MirLoan {
                place: MirPlace::root(canonical.root, self.var_types.get(&canonical.root).cloned()),
                mutable: reference.mutability == crate::origin::Mutability::Mutable,
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
            return (self.expr(expression), None);
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
    ) -> Vec<crate::origin::OriginPlace> {
        fn collect(origin: &crate::origin::Origin, places: &mut Vec<crate::origin::OriginPlace>) {
            match origin {
                crate::origin::Origin::Place(place) => places.push(place.clone()),
                crate::origin::Origin::Union(members) => {
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
                crate::SemanticAdjustment::InteriorReference { origin } => places.push(origin),
                crate::SemanticAdjustment::ReferenceResult { reference } => {
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
    ) -> Vec<crate::origin::OriginPlace> {
        self.checked_reference_places(expression)
            .into_iter()
            .filter(|place| {
                place
                    .path
                    .iter()
                    .any(|segment| matches!(segment, crate::origin::OriginSeg::Interior(_)))
            })
            .collect()
    }

    pub(super) fn mir_interior_origin(
        &mut self,
        origin: &crate::origin::OriginPlace,
        fallback: Option<VarId>,
    ) -> Option<MirInteriorOrigin> {
        let root = self.owner_vars.get(&origin.root).copied().or(fallback)?;
        self.owner_vars.entry(origin.root).or_insert(root);
        Some(MirInteriorOrigin {
            root,
            path: origin.path.clone(),
        })
    }

    pub(super) fn checked_interior_invalidations(
        &self,
        expression: &Expr,
    ) -> Vec<crate::checked::InteriorInvalidation> {
        self.checked_adjustments(expression)
            .into_iter()
            .find_map(|adjustment| match adjustment {
                crate::SemanticAdjustment::InvalidateInteriors { invalidations } => {
                    Some(invalidations)
                }
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
        invalidations: &[crate::checked::InteriorInvalidation],
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
        kwargs: &[crate::ast::KwArg],
    ) {
        for argument in args {
            self.emit_interior_invalidations(argument, None);
        }
        for argument in kwargs {
            self.emit_interior_invalidations(&argument.value, None);
        }
        self.emit_interior_invalidations(call, None);
    }
}
