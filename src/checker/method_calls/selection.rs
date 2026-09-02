//! Overload scoring, call-boundary snapshots, and selected-method
//! conversion recording.

use super::*;

impl Checker {
    /// Rebase a symbolic `origin_of(self)` pointer origin in a method result
    /// onto the concrete receiver: the declared interior projection is
    /// appended to the receiver's place, so the returned pointer carries the
    /// receiver's interior-generation loan. Non-pointer results and
    /// unresolvable receivers pass through unchanged.
    pub(super) fn rebase_self_place_pointer(&self, ty: Ty, receiver: &Expr) -> Ty {
        use crate::origin::{Mutability, Origin, OriginSeg, PointerOrigin};
        let Ty::Pointer {
            element,
            origin: PointerOrigin::SelfPlace {
                interior, subtree, ..
            },
        } = &ty
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
        Ty::Pointer {
            element: element.clone(),
            origin,
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
        kwargs: &[crate::ast::KwArg],
    ) -> Result<(), TypeError> {
        for (index, slot) in resolved.slots.iter().enumerate() {
            let expression = match slot {
                ArgSlot::Positional(position) => &args[*position],
                ArgSlot::Keyword(position) => &kwargs[*position].value,
                ArgSlot::Default => continue,
            };
            if let Some(expected) = resolved.param_types.get(index) {
                let actual = self.infer_with_expected(expression, expected, true)?;
                if !self.has_index_normalization(expression, expected)
                    && !self.record_implicit_conversion(expression, &actual, expected)?
                {
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
                let expected = match element {
                    Ty::RuntimePack(elements) => elements.get(pack_index).unwrap_or(element),
                    _ => element,
                };
                let expression = &args[position];
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
        kwargs: &[crate::ast::KwArg],
    ) -> CallBoundarySnapshot {
        let invalidations = self.interior_invalidations.borrow();
        let mut before = HashMap::new();
        for source in std::iter::once(span.clone())
            .chain(args.iter().map(Expr::source_span))
            .chain(kwargs.iter().map(|argument| argument.value.source_span()))
        {
            before
                .entry(source.clone())
                .or_insert_with(|| invalidations.get(&source).cloned().unwrap_or_default());
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
        kwargs: &[crate::ast::KwArg],
        before: &CallBoundarySnapshot,
    ) -> crate::checked::CheckedCallBoundary {
        use crate::checked::{
            CheckedCallArgumentBoundary, CheckedCallArgumentSource, CheckedCallBoundary,
            CheckedCallValueAdjustment,
        };

        let overloads = self.overload_targets.borrow();
        let implicit = self.implicit_conversions.borrow();
        let operations = self.operation_adjustments.borrow();
        let expression_types = self.expression_types.borrow();
        let invalidations = self.interior_invalidations.borrow();
        let argument =
            |source: CheckedCallArgumentSource, expression: &Expr| -> CheckedCallArgumentBoundary {
                let value_source = expression.source_span();
                let adjustments =
                    if matches!(expression_types.get(&value_source), Some(Ty::Overload(_)))
                        && let Some(target) = overloads.get(&value_source)
                    {
                        vec![CheckedCallValueAdjustment::ResolveCallable {
                            target: target.clone(),
                        }]
                    } else if let Some(target) = implicit.get(&value_source) {
                        if crate::symbol::is_index_normalization_symbol(target) {
                            vec![CheckedCallValueAdjustment::IndexNormalization {
                                target: target.clone(),
                            }]
                        } else {
                            vec![CheckedCallValueAdjustment::ImplicitConversion {
                                target: target.clone(),
                            }]
                        }
                    } else {
                        operations
                            .get(&value_source)
                            .and_then(|adjustment| match adjustment {
                                crate::checked::SemanticAdjustment::MaterializeLiteral(target) => {
                                    Some(vec![CheckedCallValueAdjustment::MaterializeLiteral {
                                        target: Box::new(target.clone()),
                                    }])
                                }
                                _ => None,
                            })
                            .unwrap_or_default()
                    };
                let prior = before
                    .invalidations
                    .get(&value_source)
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                let call_invalidations = invalidations
                    .get(&value_source)
                    .into_iter()
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
        let call_invalidations = invalidations
            .get(span)
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
        let overloads = self.overload_targets.borrow();
        let implicit = self.implicit_conversions.borrow();
        let operations = self.operation_adjustments.borrow();
        sources
            .iter()
            .map(|source| ValueAdjustmentSnapshot {
                source: source.clone(),
                overload_target: overloads.get(source).cloned(),
                implicit_conversion: implicit.get(source).cloned(),
                operation: operations.get(source).cloned(),
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
        let mut overloads = self.overload_targets.borrow_mut();
        let mut implicit = self.implicit_conversions.borrow_mut();
        let mut operations = self.operation_adjustments.borrow_mut();
        for snapshot in snapshots {
            match &snapshot.overload_target {
                Some(target) => {
                    overloads.insert(snapshot.source.clone(), target.clone());
                }
                None => {
                    overloads.remove(&snapshot.source);
                }
            }
            match &snapshot.implicit_conversion {
                Some(target) => {
                    implicit.insert(snapshot.source.clone(), target.clone());
                }
                None => {
                    implicit.remove(&snapshot.source);
                }
            }
            match &snapshot.operation {
                Some(adjustment) => {
                    operations.insert(snapshot.source.clone(), adjustment.clone());
                }
                None => {
                    operations.remove(&snapshot.source);
                }
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
        boundary: &crate::checked::CheckedCallBoundary,
    ) {
        let mut invalidations = self.interior_invalidations.borrow_mut();
        let mut remove = |source: &SourceSpan, facts: &[crate::checked::InteriorInvalidation]| {
            let empty = if let Some(current) = invalidations.get_mut(source) {
                current.retain(|fact| !facts.contains(fact));
                current.is_empty()
            } else {
                false
            };
            if empty {
                invalidations.remove(source);
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
        kwargs: &[crate::ast::KwArg],
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
        let (slots, overflow) = (matched.slots, matched.positional_overflow);
        let mut score = 0;
        for (index, slot) in slots.iter().enumerate() {
            let expression = match slot {
                ArgSlot::Positional(position) => &args[*position],
                ArgSlot::Keyword(position) => &kwargs[*position].value,
                ArgSlot::Default => continue,
            };
            let actual = self.infer_with_expected(expression, &params[index], false)?;
            if !self.has_index_normalization(expression, &params[index])
                && !self.value_coerces(&actual, &params[index])
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
        }
        if let Some(element) = variadic {
            // A specialized heterogeneous pack (`Ty::RuntimePack`) checks each overflow
            // argument against its per-index element type with exact arity; an
            // ordinary variadic checks every argument against one element type.
            for (pack_index, &position) in overflow.iter().enumerate() {
                let expected = match element {
                    Ty::RuntimePack(elements) => {
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
            if let Ty::RuntimePack(elements) = element
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
        Ok(MethodCallScore {
            rank: overload_rank(score, variadic.is_some() || kw_variadic.is_some(), 0, false),
            slots,
            positional_overflow: overflow,
            keyword_overflow,
        })
    }
}
