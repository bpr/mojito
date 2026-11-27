//! Overload scoring, call-boundary snapshots, and selected-method
//! conversion recording.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_checked::checked::NodeFacts;
use mojito_types::types::TyKind;

impl Checker {
    /// Rebase a symbolic `origin_of(self)` pointer origin in a method result
    /// onto the concrete receiver: the declared interior projection is
    /// appended to the receiver's place, so the returned pointer carries the
    /// receiver's interior-generation loan. Non-pointer results and
    /// unresolvable receivers pass through unchanged, as does a result
    /// whose `declared` type names no `origin_of(self)`: its symbolic
    /// receiver origin is the caller's, bound through the bracket
    /// (`p.unsafe_origin_cast[origin_of(self)]()` in a method's body).
    pub(super) fn rebase_self_place_pointer(
        &self,
        ty: Ty,
        receiver: &Expr,
        declared: Option<&Ty>,
    ) -> Ty {
        use mojito_types::origin::{Mutability, Origin, RefTy};
        // The projection below `origin_of(self)` a pointer's origin argument
        // names, when that is the receiver's.
        let self_place_path = |ty: &Ty| {
            ty.pointer_lit_ref()
                .and_then(|pointer| match pointer.origin.uncast() {
                    Origin::SelfParam => Some(Vec::new()),
                    Origin::Projected(base, path) if **base == Origin::SelfParam => {
                        Some(path.clone())
                    }
                    _ => None,
                })
        };
        if declared.is_some_and(|declared| self_place_path(declared).is_none()) {
            return ty;
        }
        let (Some(pointer), Some(path)) = (ty.pointer_lit_ref(), self_place_path(&ty)) else {
            return ty;
        };
        let rebased = |origin: Origin, mutability| {
            Ty::pointer_from_lit_ref(&RefTy {
                origin: origin.reprojected(&path),
                mutability,
                ..pointer.clone()
            })
        };
        // A `ref[Self.o]` parameter receiver reads as the enclosing struct's
        // binder, as `Pointer(to=xs)` does: `array.unsafe_ptr()` in `Span`'s
        // constructor is a `Pointer[T, Self.origin._get_owned_interior[...]]`.
        if let ExprKind::Identifier(name) = &receiver.kind
            && let Some((id, mutability)) = self.lookup_reference_parameter_binder(name)
        {
            return rebased(Origin::Param(id), mutability);
        }
        let Ok(reference) = self.reference_actual(receiver) else {
            return ty;
        };
        match reference.origin {
            // The receiver's own storage in a bare `ref self` body is its
            // symbolic origin at its implicit binder's mutability, upstream's
            // one `self_is_origin`, as `origin_of(self)` reads there.
            Origin::Place(place)
                if place.path.is_empty()
                    && matches!(reference.mutability, Mutability::Param(id) if id.is_reference_binder())
                    && self.lookup_owner("self") == Some(place.root) =>
            {
                rebased(Origin::SelfParam, reference.mutability)
            }
            // A place below a bare `ref` argument keeps its binder's
            // mutability, as the argument's own origin does.
            place @ Origin::Place(_) if matches!(reference.mutability, Mutability::Param(id) if id.is_reference_binder()) => {
                rebased(place, reference.mutability)
            }
            place @ Origin::Place(_) => rebased(
                place,
                Mutability::of(reference.mutability == Mutability::Mutable),
            ),
            binder @ Origin::Param(_) => rebased(binder, reference.mutability),
            _ => ty,
        }
    }

    /// Rebase a method result's struct instances over the receiver's
    /// origin (`origin_of(self)`) onto the concrete receiver, as upstream's
    /// call substitutes the callee's `self_is_origin` and `self_is_mut`
    /// together: each such origin argument becomes the receiver's, an
    /// interior projection re-applied, and the `mut=` slot it governs
    /// takes that origin's mutability. A pointer result is
    /// [`Self::rebase_self_place_pointer`]'s, and a receiver with no
    /// reference passes the result through.
    pub(super) fn rebase_self_struct_result(&self, ty: Ty, receiver: &Expr) -> Ty {
        if ty.pointer_lit_ref().is_some() {
            return ty;
        }
        let origin = match &receiver.kind {
            ExprKind::Identifier(name)
                if let Some((id, _)) = self.lookup_reference_parameter_binder(name) =>
            {
                mojito_types::origin::Origin::Param(id)
            }
            _ => match self.reference_actual(receiver) {
                Ok(reference) => reference.origin,
                Err(_) => return ty,
            },
        };
        // A module binding's owner is one check's numbering, which no
        // package declaration carries: its origin stays the callee's, and
        // only the `mut=` slots it governs take the receiver's.
        let module_root = matches!(
            origin.uncast().projection_base(),
            mojito_types::origin::Origin::Place(place) if self.owner_scope(place.root) == Some(0)
        );
        self.rebase_self_origins(&ty, &origin, !module_root)
    }

    /// `ty` with each struct instance's receiver-rooted origin argument
    /// (`origin_of(self)`, projected or cast) read as `receiver` when
    /// `rebase_origins` says so, and the
    /// `mut=` slot that origin governs, when the receiver's convention left
    /// it open (a bare `ref self`'s binder read), holding the rebased
    /// origin's mutability ([`Self::origin_mutability_slot`]); an associated
    /// application's arguments rebase as
    /// [`crate::checker::origins::bind_self_origin_arguments`] rebases them.
    pub(in crate::checker) fn rebase_self_origins(
        &self,
        ty: &Ty,
        receiver: &mojito_types::origin::Origin,
        rebase_origins: bool,
    ) -> Ty {
        use mojito_types::origin::Origin;
        let rebased = mojito_types::types::map_struct_arguments(ty, &|name, mut arguments| {
            let Some(info) = self.structs.get(name) else {
                return arguments;
            };
            if arguments.len() != info.params.len() {
                return arguments;
            }
            for (index, decl) in info.params.iter().enumerate() {
                let (ParamDecl::Origin { origin: id, .. }, TyArg::Origin(origin)) =
                    (decl, &arguments[index])
                else {
                    continue;
                };
                if !matches!(origin.uncast().projection_base(), Origin::SelfParam) {
                    continue;
                }
                let bound = crate::checker::origins::rebase_self_origin(origin, receiver);
                if let Some(binder) = ParamDecl::governing_mutability_binder(&info.params, *id)
                    && let Some(slot) = info
                        .params
                        .iter()
                        .position(|candidate| candidate.id() == binder.id())
                    && matches!(arguments[slot], TyArg::Val(CtValue::Deferred(_)))
                {
                    arguments[slot] = TyArg::Val(self.origin_mutability_slot(binder, &bound, None));
                }
                if rebase_origins {
                    arguments[index] = TyArg::Origin(bound);
                }
            }
            arguments
        });
        if rebase_origins {
            crate::checker::origins::bind_self_origin_arguments(&rebased, receiver)
        } else {
            rebased
        }
    }

    /// Apply the implicit conversions selected while scoring one concrete method
    /// overload. Keyword-overflow arguments are materialized into the callee's
    /// `StringDict`, so their conversions must be recorded just like conversions
    /// for ordinary parameter slots.
    pub(in crate::checker) fn record_selected_method_conversions(
        &self,
        method: &str,
        resolved: &MethodCallResolution,
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Result<(), TypeError> {
        for (index, slot) in resolved.slots.iter().enumerate() {
            let expression = match slot {
                ArgSlot::Positional(position) => &args[*position],
                ArgSlot::Keyword(position) => &kwargs[*position].value,
                ArgSlot::Default => continue,
            };
            if let Some(declared) = resolved.param_types.get(index) {
                let actual = self.infer_with_expected(expression, declared, true)?;
                let expected = declared;
                let binds = if self.is_synthetic_slice_descriptor(expression) {
                    self.slice_literal_binding(expression, expected, true)?
                        .is_some()
                } else {
                    self.has_index_normalization(expression, expected)
                        || self.record_implicit_conversion(expression, &actual, expected)?
                };
                if !binds {
                    return Err(TypeError::TypeMismatch {
                        expected: expected.to_string(),
                        found: actual.to_string(),
                        context: format!("argument {} to method '{method}'", index + 1),
                    });
                }
            }
        }
        if let Some(element) = &resolved.variadic_element {
            // A specialized heterogeneous pack records each overflow argument
            // against its per-index element (mirroring the scoring pass), so
            // a literal converts where a nominal String element is expected.
            for (pack_index, &position) in resolved.positional_overflow.iter().enumerate() {
                let expected = match element.kind() {
                    TyKind::RuntimePack(elements) => elements.get(pack_index).unwrap_or(element),
                    _ => element,
                };
                let expression = &args[position];
                // A forwarded pack was bound whole while scoring; it has no
                // element conversion to record.
                if self.forwarded_pack(expression).is_some() {
                    continue;
                }
                let actual = self.infer_with_expected(expression, expected, true)?;
                if !self.record_implicit_conversion(expression, &actual, expected)? {
                    return Err(TypeError::TypeMismatch {
                        expected: expected.to_string(),
                        found: actual.to_string(),
                        context: format!("variadic argument to method '{method}'"),
                    });
                }
            }
        }
        if let Some(expected) = &resolved.keyword_element {
            for &position in &resolved.keyword_overflow {
                let expression = &kwargs[position].value;
                let actual = self.infer(expression)?;
                if !self.record_implicit_conversion(expression, &actual, expected)? {
                    return Err(TypeError::TypeMismatch {
                        expected: expected.to_string(),
                        found: actual.to_string(),
                        context: format!(
                            "keyword '{}' collected by method '{method}'",
                            kwargs[position].name
                        ),
                    });
                }
            }
        }
        Ok(())
    }

    pub(in crate::checker) fn call_boundary_snapshot(
        &self,
        span: &SourceSpan,
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> CallBoundarySnapshot {
        let records = self.emitter.borrow();
        let mut before = HashMap::new();
        for source in std::iter::once(span.clone())
            .chain(args.iter().map(Expr::source_span))
            .chain(kwargs.iter().map(|argument| argument.value.source_span()))
        {
            before.entry(source.clone()).or_insert_with(|| {
                records
                    .get(&source)
                    .and_then(|facts| facts.interior_invalidations.clone())
                    .unwrap_or_default()
            });
        }
        CallBoundarySnapshot {
            invalidations: before,
        }
    }

    /// Freeze the value adaptations and generation changes belonging to one
    /// selected call. A later call may reuse the same source occurrence (the
    /// getter/setter pair of augmented subscript assignment), so these facts must
    /// travel with the call contract rather than remain only in source-keyed maps.
    pub(in crate::checker) fn checked_call_boundary(
        &self,
        span: &SourceSpan,
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
        before: &CallBoundarySnapshot,
    ) -> mojito_checked::checked::CheckedCallBoundary {
        use mojito_checked::checked::{
            CheckedCallArgumentBoundary, CheckedCallArgumentSource, CheckedCallBoundary,
        };

        let records = self.emitter.borrow();
        let argument =
            |source: CheckedCallArgumentSource, expression: &Expr| -> CheckedCallArgumentBoundary {
                let value_source = expression.source_span();
                let facts = records.get(&value_source).unwrap_or(&NodeFacts::EMPTY);
                let adjustments = call_value_adjustments(facts);
                let prior = before
                    .invalidations
                    .get(&value_source)
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                let call_invalidations = facts
                    .interior_invalidations
                    .iter()
                    .flatten()
                    .filter(|fact| !prior.contains(fact))
                    .cloned()
                    .collect();
                CheckedCallArgumentBoundary {
                    source,
                    value_source,
                    adjustments,
                    invalidations: call_invalidations,
                }
            };

        let arguments = args
            .iter()
            .enumerate()
            .map(|(index, expression)| {
                argument(CheckedCallArgumentSource::Positional(index), expression)
            })
            .chain(kwargs.iter().enumerate().map(|(index, argument_value)| {
                argument(
                    CheckedCallArgumentSource::Keyword(index),
                    &argument_value.value,
                )
            }))
            .collect();
        let prior = before
            .invalidations
            .get(span)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let call_invalidations = records
            .get(span)
            .and_then(|facts| facts.interior_invalidations.as_ref())
            .into_iter()
            .flatten()
            .filter(|fact| !prior.contains(fact))
            .cloned()
            .collect();
        CheckedCallBoundary {
            arguments,
            invalidations: call_invalidations,
        }
    }

    pub(in crate::checker) fn snapshot_value_adjustments(
        &self,
        sources: &[SourceSpan],
    ) -> Vec<ValueAdjustmentSnapshot> {
        let records = self.emitter.borrow();
        sources
            .iter()
            .map(|source| {
                let facts = records.get(source).unwrap_or(&NodeFacts::EMPTY);
                ValueAdjustmentSnapshot {
                    source: source.clone(),
                    overload_target: facts.overload_target.clone(),
                    implicit_conversion: facts.implicit_conversion.clone(),
                    operation: facts.adjustment.clone(),
                }
            })
            .collect()
    }

    /// Put shared source operands back into their pre-call state after freezing a
    /// call boundary. Augmented subscripts then select the setter independently;
    /// neither call can overwrite the other's conversion or normalization.
    pub(in crate::checker) fn restore_value_adjustments(
        &self,
        snapshots: &[ValueAdjustmentSnapshot],
    ) {
        let mut records = self.emitter.borrow_mut();
        for snapshot in snapshots {
            let restores_any = snapshot.overload_target.is_some()
                || snapshot.implicit_conversion.is_some()
                || snapshot.operation.is_some();
            let facts = if restores_any {
                Some(records.record(snapshot.source.clone()))
            } else {
                records.get_mut(&snapshot.source)
            };
            if let Some(facts) = facts {
                facts.overload_target.clone_from(&snapshot.overload_target);
                facts
                    .implicit_conversion
                    .clone_from(&snapshot.implicit_conversion);
                facts.adjustment.clone_from(&snapshot.operation);
            }
        }
    }

    /// Remove call-local invalidations from the compatibility source tables once
    /// they have been frozen on a selected contract. Effects belonging to
    /// evaluation of the argument expression were present in the pre-call
    /// snapshot and therefore are not listed in `boundary` and remain untouched.
    pub(in crate::checker) fn remove_call_boundary_invalidations(
        &self,
        site: &SourceSpan,
        boundary: &mojito_checked::checked::CheckedCallBoundary,
    ) {
        let mut records = self.emitter.borrow_mut();
        let mut remove =
            |source: &SourceSpan, facts: &[mojito_checked::checked::InteriorInvalidation]| {
                if let Some(record) = records.get_mut(source)
                    && let Some(current) = record.interior_invalidations.as_mut()
                {
                    current.retain(|fact| !facts.contains(fact));
                    if current.is_empty() {
                        record.interior_invalidations = None;
                    }
                }
            };
        for argument in &boundary.arguments {
            remove(&argument.value_source, &argument.invalidations);
        }
        remove(site, &boundary.invalidations);
    }

    pub(in crate::checker) fn score_method_call(
        &self,
        signature: &MethodSig,
        params: &[Ty],
        variadic: Option<&Ty>,
        kw_variadic: Option<&Ty>,
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Result<MethodCallScore, TypeError> {
        let forwarded_element = self.forwarded_kwargs_element("method", kwargs)?;
        if forwarded_element.is_some() && kw_variadic.is_none() {
            return Err(TypeError::BadCall {
                func: "method".to_string(),
                reason: "`**kwargs^` requires a callee with a `**kwargs` collector".to_string(),
            });
        }
        let keyword_names: Vec<_> = kwargs
            .iter()
            .filter(|argument| !argument.is_forwarded())
            .map(|arg| arg.name.as_str())
            .collect();
        let forwarded_pack = self.forwarded_pack_argument("method", args, variadic.is_some())?;
        let matched = match_call_slots(
            &signature.names,
            &signature.required,
            signature.positional_only,
            signature.keyword_only,
            args.len(),
            &keyword_names,
            CallVariadics {
                positional: variadic.is_some(),
                keyword: kw_variadic.is_some(),
            },
        )
        .map_err(|error| error.into_type_error("method"))?;
        mojito_ast::call::bind_keyword_forward(&matched, forwarded_element.is_some())
            .map_err(|error| error.into_type_error("method"))?;
        if let Some((position, _)) = &forwarded_pack {
            mojito_ast::call::bind_spread(&matched, *position)
                .map_err(|error| error.into_type_error("method"))?;
        }
        let (slots, overflow) = (matched.slots, matched.positional_overflow);
        let mut score = 0;
        let mut simd_erasures = 0;
        for (index, slot) in slots.iter().enumerate() {
            let expression = match slot {
                ArgSlot::Positional(position) => &args[*position],
                ArgSlot::Keyword(position) => &kwargs[*position].value,
                ArgSlot::Default => continue,
            };
            let actual = self.infer_with_expected(expression, &params[index], false)?;
            // A slice literal binds through its parameter type's
            // `__slice_literal__` constructor, at one cost whichever type
            // that is.
            let literal = self.is_synthetic_slice_descriptor(expression);
            let binds = if literal {
                self.slice_literal_binding(expression, &params[index], false)?
                    .is_some()
            } else {
                self.has_index_normalization(expression, &params[index])
                    || self.value_coerces(&actual, &params[index])
            };
            if !binds
                && (literal
                    || self
                        .implicit_conversion_target(&actual, &params[index])?
                        .is_none())
            {
                return Err(TypeError::TypeMismatch {
                    expected: params[index].to_string(),
                    found: actual.to_string(),
                    context: "method overload candidate".to_string(),
                });
            }
            if !literal {
                score += conversion_count(&actual, &params[index]);
            }
            simd_erasures += usize::from(
                matches!(
                    signature.params.get(index).map(Ty::kind),
                    Some(TyKind::Param { .. })
                ) && mojito_types::types::simd_shape(&actual).is_some(),
            );
        }
        if let Some(element) = variadic {
            // A specialized heterogeneous pack (`TyKind::RuntimePack`) checks each overflow
            // argument against its per-index element type with exact arity; an
            // ordinary variadic checks every argument against one element type.
            for (pack_index, &position) in overflow.iter().enumerate() {
                let expected = match element.kind() {
                    TyKind::RuntimePack(elements) => {
                        elements
                            .get(pack_index)
                            .ok_or_else(|| TypeError::ArityMismatch {
                                name: "method".to_string(),
                                expected: elements.len(),
                                got: overflow.len(),
                            })?
                    }
                    _ => element,
                };
                if let Some((spread, forwarded)) = &forwarded_pack
                    && *spread == position
                {
                    // The whole pack is the collector's one argument; the
                    // conventions and bounds are its whole conversion.
                    self.bind_forwarded_pack(
                        "method",
                        &args[position],
                        forwarded,
                        expected,
                        Some(matches!(
                            signature.variadic_convention,
                            Some(ArgConvention::Var)
                        )),
                    )?;
                    continue;
                }
                let actual = self.infer_with_expected(&args[position], expected, false)?;
                if !self.coerces_by_origins(&actual, expected)
                    && self
                        .implicit_conversion_target(&actual, expected)?
                        .is_none()
                {
                    return Err(TypeError::TypeMismatch {
                        expected: expected.to_string(),
                        found: actual.to_string(),
                        context: "variadic method argument".to_string(),
                    });
                }
                score += conversion_count(&actual, expected);
            }
            if let TyKind::RuntimePack(elements) = element.kind()
                && elements.len() != overflow.len()
            {
                return Err(TypeError::ArityMismatch {
                    name: "method".to_string(),
                    expected: elements.len(),
                    got: overflow.len(),
                });
            }
        }
        let keyword_overflow = matched.keyword_overflow;
        if let Some(element) = kw_variadic {
            for &position in &keyword_overflow {
                let expression = &kwargs[position].value;
                let actual = self.infer(expression)?;
                if !self.value_coerces(&actual, element)
                    && self.implicit_conversion_target(&actual, element)?.is_none()
                {
                    return Err(TypeError::TypeMismatch {
                        expected: element.to_string(),
                        found: actual.to_string(),
                        context: "keyword variadic method argument".to_string(),
                    });
                }
                self.check_consuming(
                    expression,
                    &actual,
                    &format!("keyword '{}' collected by method", kwargs[position].name),
                )?;
                score += conversion_count(&actual, element);
            }
            if let Some(actual) = forwarded_element
                && actual != *element
            {
                return Err(TypeError::TypeMismatch {
                    expected: format!("StringDict[{element}]"),
                    found: format!("StringDict[{actual}]"),
                    context: "forwarded keyword arguments to method".to_string(),
                });
            }
        }
        // A method's own compile-time parameters rank it exactly as a free
        // function's do: `s.m(String("t"))` selects a concrete `m(self, a:
        // String)` over a generic `m[T: Writable](self, a: T)`. `decls` holds
        // the method's own bracket list only — a struct's parameters live in
        // `StructInfo::decls` — and a parameter no per-call clone bakes does
        // not make the candidate generic. Its origin binders count too,
        // the implicit one a `Pointer[T, _]` parameter declares among them,
        // so `p == q` over two pointers of one origin selects `__eq__(self,
        // rhs: Self)` beside `__eq__(self, rhs: Pointer[Self.T, _])`.
        let baked = baked_decl_count(&signature.decls())
            + signature
                .declared
                .iter()
                .filter(|decl| decl.is_origin())
                .count()
            + inferred_origin_count(&signature.params);
        Ok(MethodCallScore {
            rank: overload_rank(
                score,
                variadic.is_some() || kw_variadic.is_some(),
                baked,
                baked > 0,
            ) + self
                .argument_binding(
                    variadic.map(|_| overflow.len()),
                    &slots,
                    &signature.conventions,
                    params,
                    args,
                    kwargs,
                )
                .rank(),
            simd_erasures,
            slots,
            positional_overflow: overflow,
            keyword_overflow,
        })
    }
}

/// The adaptations `facts` record at one source operand, as a call's
/// boundary freezes them for that call alone. A conversion reads its
/// source by address when its constructor's declared parameter does, the
/// node's own `ConversionReadsSource`.
pub(in crate::checker) fn call_value_adjustments(
    facts: &NodeFacts,
) -> Vec<mojito_checked::checked::CheckedCallValueAdjustment> {
    use mojito_checked::checked::CheckedCallValueAdjustment;
    if matches!(facts.ty.as_ref().map(Ty::kind), Some(TyKind::Overload(_)))
        && let Some(target) = &facts.overload_target
    {
        vec![CheckedCallValueAdjustment::ResolveCallable {
            target: target.clone(),
        }]
    } else if let Some(target) = &facts.implicit_conversion {
        if mojito_symbol::symbol::is_index_normalization_symbol(target) {
            vec![CheckedCallValueAdjustment::IndexNormalization {
                target: target.clone(),
            }]
        } else {
            facts
                .conversion_materialization
                .clone()
                .map(CheckedCallValueAdjustment::MaterializeSource)
                .into_iter()
                .chain([CheckedCallValueAdjustment::ImplicitConversion {
                    target: target.clone(),
                    instantiated_args: facts.conversion_instantiation.clone().unwrap_or_default(),
                    source_by_address: facts.conversion_reads_source,
                }])
                .collect()
        }
    } else {
        facts
            .adjustment
            .as_deref()
            .and_then(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::MaterializeLiteral {
                    target, ..
                } => Some(vec![CheckedCallValueAdjustment::MaterializeLiteral {
                    target: target.clone(),
                }]),

                _ => None,
            })
            .unwrap_or_default()
    }
}
