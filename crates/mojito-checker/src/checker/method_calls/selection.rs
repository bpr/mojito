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
    /// unresolvable receivers pass through unchanged.
    pub(super) fn rebase_self_place_pointer(&self, ty: Ty, receiver: &Expr) -> Ty {
        use mojito_types::origin::{Mutability, Origin, OriginSeg, PointerOrigin};
        let TyKind::Pointer {
            element,
            origin: PointerOrigin::SelfPlace {
                interior, subtree, ..
            },
        } = ty.kind()
        else {
            return ty;
        };
        let Ok(reference) = self.reference_actual(receiver) else {
            return ty;
        };
        let origin = match reference.origin {
            Origin::Place(mut place) => {
                for tag in interior {
                    place.path.push(OriginSeg::Interior(tag.clone()));
                }
                if *subtree {
                    place.path.push(OriginSeg::Subtree);
                }
                PointerOrigin::Place {
                    place,
                    mutable: matches!(reference.mutability, Mutability::Mutable),
                }
            }
            Origin::Param(id) => PointerOrigin::Param {
                id,
                mutability: reference.mutability,
                interior: interior.clone(),
                subtree: *subtree,
            },
            _ => return ty,
        };
        TyKind::Pointer {
            element: element.clone(),
            origin,
        }
        .into()
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
            if let Some(expected) = resolved.param_types.get(index) {
                let actual = self.infer_with_expected(expression, expected, true)?;
                let binds = self.has_index_normalization(expression, expected)
                    || self.binds_slice_literal(expression, &actual, expected)
                    || self.record_implicit_conversion(expression, &actual, expected)?;
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
        let records = self.node_facts.borrow();
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
            CheckedCallValueAdjustment,
        };

        let records = self.node_facts.borrow();
        let argument = |source: CheckedCallArgumentSource,
                        expression: &Expr|
         -> CheckedCallArgumentBoundary {
            let value_source = expression.source_span();
            let facts = records.get(&value_source).unwrap_or(&NodeFacts::EMPTY);
            let adjustments =
                if matches!(facts.ty.as_ref().map(Ty::kind), Some(TyKind::Overload(_)))
                    && let Some(target) = &facts.overload_target
                {
                    vec![CheckedCallValueAdjustment::ResolveCallable {
                        target: target.clone(),
                    }]
                } else if let Some(target) = &facts.implicit_conversion {
                    if target == mojito_symbol::symbol::SCALAR_SPLAT_CONVERSION {
                        facts
                            .implicit_conversion_type
                            .as_ref()
                            .map(|ty| {
                                vec![CheckedCallValueAdjustment::SplatScalar { target: ty.clone() }]
                            })
                            .unwrap_or_default()
                    } else if mojito_symbol::symbol::is_index_normalization_symbol(target) {
                        vec![CheckedCallValueAdjustment::IndexNormalization {
                            target: target.clone(),
                        }]
                    } else {
                        vec![CheckedCallValueAdjustment::ImplicitConversion {
                            target: target.clone(),
                        }]
                    }
                } else {
                    facts
                        .adjustment
                        .as_deref()
                        .and_then(|adjustment| match adjustment {
                            mojito_checked::checked::SemanticAdjustment::MaterializeLiteral(
                                target,
                            ) => Some(vec![CheckedCallValueAdjustment::MaterializeLiteral {
                                target: target.clone(),
                            }]),

                            _ => None,
                        })
                        .unwrap_or_default()
                };
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
        let records = self.node_facts.borrow();
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
        let mut records = self.node_facts.borrow_mut();
        for snapshot in snapshots {
            let restores_any = snapshot.overload_target.is_some()
                || snapshot.implicit_conversion.is_some()
                || snapshot.operation.is_some();
            let facts = if restores_any {
                Some(records.entry(snapshot.source.clone()).or_default())
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
        let mut records = self.node_facts.borrow_mut();
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
            let binds = self.has_index_normalization(expression, &params[index])
                || self.value_coerces(&actual, &params[index])
                || self.binds_slice_literal(expression, &actual, &params[index]);
            if !binds
                && (self.is_synthetic_slice_descriptor(expression)
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
            score += conversion_count(&actual, &params[index]);
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
                if !coerces(&actual, expected)
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
        // not make the candidate generic.
        let baked = baked_decl_count(&signature.decls);
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
