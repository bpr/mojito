//! Type-level verification rules: compatibility, checked-type
//! instantiation, and iterator-result adapters.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::param_expr::{ParamBindings, ParamContext};
use mojito_types::types::TySubst;

/// Compatibility for verification purposes: either direction of the checker's
/// coercion predicate. Lowering emits checker-approved conversions before
/// values flow, so remaining differences are representational (literal
/// materialization, generic instantiation), not errors to re-litigate. A type
/// mentioning an unsubstituted parameter is not compared — instantiation is
/// the checker's domain and the verifier never re-derives it.
pub(super) fn types_compatible(found: &Ty, expected: &Ty) -> bool {
    pub(super) const fn callable_environment(
        ty: &Ty,
    ) -> Option<&mojito_types::origin::CallableEnvironment> {
        match ty {
            Ty::Func { environment, .. } | Ty::GenericFunc { environment, .. } => Some(environment),
            _ => None,
        }
    }
    // Some semantic-only sum types (notably unresolved overload sets) are not
    // ordinary value-coercion sources or destinations. Identity is still the
    // strongest possible compatibility proof and must precede those structural
    // special cases.
    if found == expected {
        return true;
    }
    if let (Some(found), Some(expected)) =
        (callable_environment(found), callable_environment(expected))
        && !mojito_types::types::callable_environment_coerces(found, expected)
    {
        // Environment differences are semantic, not a representational detail
        // that lowering may erase. In particular, an inference/default contract
        // is not a general MIR-level wildcard for a concrete capture set.
        // This is deliberately the permissive bound-channel predicate: the
        // checker's strict value-coercion rule (no capturing closure into an
        // unqualified `def(...)` value) has already run, and comptime callable
        // bounds legitimately ground `Capturing` values against `Default`
        // contracts here.
        return false;
    }
    // Two residual sizes over the same binders are one type only when they
    // are one canonical expression: a parameter occurring in both does not
    // make `Buf[n + 1]` and `Buf[n + 2]` interchangeable.
    if residual_arguments_conflict(found, expected) {
        return false;
    }
    if contains_type_param(found) || contains_type_param(expected) {
        return true;
    }
    // Pointer provenance erases from the runtime ABI: `unsafe_origin_cast` retypes
    // a pointer without any runtime operation (lowering forwards the
    // receiver register), so ABI compatibility compares elements only. The
    // checker and ownership analysis own origin discipline.
    if let (
        Ty::Pointer {
            element: found_element,
            ..
        },
        Ty::Pointer {
            element: expected_element,
            ..
        },
    ) = (found, expected)
    {
        return types_compatible(found_element, expected_element);
    }
    // A bare `Struct(name, [])` is the established erased spelling for a
    // receiver or synthesized construction of any instantiation of `name`.
    // Otherwise the origin tail erases from the runtime ABI like a pointer
    // origin: the checker owns origin identity, so two instantiations that
    // differ only in their origin arguments are one type here.
    if let (Ty::Struct(found_name, found_args), Ty::Struct(expected_name, expected_args)) =
        (found, expected)
        && found_name == expected_name
    {
        if found_args.is_empty() || expected_args.is_empty() {
            return true;
        }
        if found_args.len() == expected_args.len()
            && found_args
                .iter()
                .zip(expected_args)
                .all(|(found, expected)| match (found, expected) {
                    (TyArg::Ty(found), TyArg::Ty(expected)) => types_compatible(found, expected),
                    (TyArg::Val(found), TyArg::Val(expected)) => found == expected,
                    (TyArg::Origin(_), TyArg::Origin(_)) => true,
                    _ => false,
                })
        {
            return true;
        }
    }
    // A contextual selection narrows an overload set to one member.
    if let Ty::Overload(members) = found {
        return members
            .iter()
            .any(|member| types_compatible(member, expected));
    }
    // A struct may nominally conform to a `def(...)` callable trait; the
    // conformance is checker-verified and not yet recorded in MIR
    // declarations, so the verifier does not re-check it here.
    if matches!(found, Ty::Struct(..)) && matches!(expected, Ty::Func { .. }) {
        return true;
    }
    mojito_types::types::value_coerces(found, expected)
        || mojito_types::types::value_coerces(expected, found)
}

pub(super) fn declared<'a>(
    declarations: &'a MirDeclarations,
    callee: &str,
) -> Option<&'a MirFunctionDeclaration> {
    declarations
        .functions
        .iter()
        .find(|declaration| declaration.lowered_name == callee)
}

pub(super) fn iterator_result_matches_declaration(
    call: &mojito_checked::checked::CheckedIteratorCall,
    declaration: &MirFunctionDeclaration,
) -> bool {
    if call.result_adapter.is_some() {
        return false;
    }
    match (&call.reference_result, &call.result_ty) {
        (Some(reference), Ty::Ref(result_reference)) => {
            declaration.returns_reference
                && reference == result_reference
                && types_compatible(&reference.referent, &declaration.ret_ty)
        }
        (None, result) => {
            !declaration.returns_reference && types_compatible(result, &declaration.ret_ty)
        }
        _ => false,
    }
}

pub(super) fn verify_iterator_result_adapter(
    prefix: &str,
    call: &mojito_checked::checked::CheckedIteratorCall,
    errors: &mut Vec<String>,
) -> bool {
    let abstract_dispatch = call.target == "__iterator_dispatch.__next__";
    match call.result_adapter {
        Some(mojito_checked::checked::CheckedResultAdapter::CopyIteratorReference) => {
            if !abstract_dispatch {
                errors.push(format!(
                    "{prefix}: iterator copy-reference adapter is attached to concrete target '{}'",
                    call.target
                ));
            }
            if call.reference_result.is_some() {
                errors.push(format!(
                    "{prefix}: adapted abstract iterator result also carries a concrete reference ABI"
                ));
            }
        }
        None if abstract_dispatch && call.reference_result.is_none() => errors.push(format!(
            "{prefix}: abstract value-returning iterator dispatch lacks its copy-reference adapter"
        )),
        None => {}
    }
    abstract_dispatch
}

pub(super) fn contains_runtime_pack(ty: &Ty) -> bool {
    match ty {
        Ty::RuntimePack(_) => true,
        Ty::ComptimeList(inner) | Ty::Pointer { element: inner, .. } => {
            contains_runtime_pack(inner)
        }
        Ty::Tuple(elements) | Ty::Variant(elements) | Ty::Overload(elements) => {
            elements.iter().any(contains_runtime_pack)
        }
        Ty::Ref(reference) => contains_runtime_pack(&reference.referent),
        Ty::Struct(_, arguments) => arguments.iter().any(|argument| match argument {
            mojito_types::types::TyArg::Ty(inner) => contains_runtime_pack(inner),
            mojito_types::types::TyArg::Val(_) | mojito_types::types::TyArg::Origin(_) => false,
        }),
        Ty::Assoc { base, .. } => contains_runtime_pack(base),
        Ty::Func {
            params,
            ret,
            variadic,
            kw_variadic,
            error,
            ..
        }
        | Ty::GenericFunc {
            params,
            ret,
            variadic,
            kw_variadic,
            error,
            ..
        } => {
            params.iter().any(contains_runtime_pack)
                || contains_runtime_pack(ret)
                || variadic.as_deref().is_some_and(contains_runtime_pack)
                || kw_variadic.as_deref().is_some_and(contains_runtime_pack)
                || error.as_deref().is_some_and(contains_runtime_pack)
        }
        _ => false,
    }
}

pub(super) fn instantiate_checked_type(
    ty: &Ty,
    type_arguments: &TySubst,
    value_arguments: &HashMap<String, CtValue>,
    bound_values: &HashSet<String>,
) -> Result<Ty, String> {
    Ok(match ty {
        Ty::Param {
            binder,
            bounds,
            callable_bound,
        } => type_arguments
            .get(&binder.id)
            .cloned()
            .unwrap_or_else(|| Ty::Param {
                binder: binder.clone(),
                bounds: bounds.clone(),
                callable_bound: callable_bound.clone(),
            }),
        Ty::Dependent(dependent) => {
            let context = ParamContext::detached();
            let bindings = ParamBindings::from_named_values(&context, value_arguments);
            let expr = match dependent.selection() {
                Some((elements, index)) => {
                    let elements = elements
                        .iter()
                        .map(|element| {
                            instantiate_checked_type(
                                element,
                                type_arguments,
                                value_arguments,
                                bound_values,
                            )
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    context
                        .replace(index, &bindings)
                        .and_then(|index| context.fold(&index))
                        .and_then(|index| context.select(elements, &index))
                }
                None => context.replace(dependent.expr(), &bindings),
            }
            .map_err(|error| error.to_string())?;
            let instantiated = DependentType::resolve(expr);
            if let Ty::Dependent(residual) = &instantiated {
                let mut referenced = HashSet::new();
                residual.expr().referenced_parameters(&mut referenced);
                if referenced.is_empty() {
                    return Err(
                        "dependent index did not evaluate to a compile-time value".to_string()
                    );
                }
                let mut unbound: Vec<_> = referenced.difference(bound_values).cloned().collect();
                unbound.sort();
                if !unbound.is_empty() {
                    return Err(format!(
                        "dependent index references unsubstituted parameter(s): {}",
                        unbound.join(", ")
                    ));
                }
            }
            instantiated
        }
        Ty::Struct(name, arguments) => Ty::Struct(
            name.clone(),
            arguments
                .iter()
                .map(|argument| match argument {
                    TyArg::Ty(ty) => {
                        instantiate_checked_type(ty, type_arguments, value_arguments, bound_values)
                            .map(TyArg::Ty)
                    }
                    // A residual value argument binds through the same
                    // replacement the checker used, so an erased instance
                    // compares canonical expressions, not spellings.
                    TyArg::Val(value) => {
                        let context = ParamContext::detached();
                        let bindings = ParamBindings::from_named_values(&context, value_arguments);
                        mojito_types::types::replace_value_parameters(&context, value, &bindings)
                            .map(TyArg::Val)
                            .map_err(|error| error.to_string())
                    }
                    TyArg::Origin(origin) => Ok(TyArg::Origin(origin.clone())),
                })
                .collect::<Result<Vec<_>, String>>()?,
        ),
        Ty::ComptimeList(element) => Ty::ComptimeList(Box::new(instantiate_checked_type(
            element,
            type_arguments,
            value_arguments,
            bound_values,
        )?)),
        Ty::Tuple(elements) => Ty::Tuple(
            elements
                .iter()
                .map(|ty| {
                    instantiate_checked_type(ty, type_arguments, value_arguments, bound_values)
                })
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Ty::RuntimePack(elements) => Ty::RuntimePack(
            elements
                .iter()
                .map(|ty| {
                    instantiate_checked_type(ty, type_arguments, value_arguments, bound_values)
                })
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Ty::VariadicPack(element) => Ty::VariadicPack(Box::new(instantiate_checked_type(
            element,
            type_arguments,
            value_arguments,
            bound_values,
        )?)),
        Ty::Variant(elements) => Ty::Variant(
            elements
                .iter()
                .map(|ty| {
                    instantiate_checked_type(ty, type_arguments, value_arguments, bound_values)
                })
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Ty::Pointer { element, origin } => Ty::Pointer {
            element: Box::new(instantiate_checked_type(
                element,
                type_arguments,
                value_arguments,
                bound_values,
            )?),
            origin: origin.clone(),
        },
        Ty::Ref(reference) => {
            let mut reference = reference.clone();
            reference.referent = Box::new(instantiate_checked_type(
                &reference.referent,
                type_arguments,
                value_arguments,
                bound_values,
            )?);
            Ty::Ref(reference)
        }
        Ty::Assoc { base, name, args } => Ty::Assoc {
            base: Box::new(instantiate_checked_type(
                base,
                type_arguments,
                value_arguments,
                bound_values,
            )?),
            name: name.clone(),
            args: args
                .iter()
                .map(|argument| match argument {
                    TyArg::Ty(ty) => Ok(TyArg::Ty(instantiate_checked_type(
                        ty,
                        type_arguments,
                        value_arguments,
                        bound_values,
                    )?)),
                    TyArg::Val(value) => Ok(TyArg::Val(value.clone())),
                    TyArg::Origin(origin) => Ok(TyArg::Origin(origin.clone())),
                })
                .collect::<Result<Vec<_>, String>>()?,
        },
        Ty::Overload(candidates) => Ty::Overload(
            candidates
                .iter()
                .map(|ty| {
                    instantiate_checked_type(ty, type_arguments, value_arguments, bound_values)
                })
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Ty::Func {
            environment,
            params,
            names,
            ret,
            required,
            variadic,
            kw_variadic,
            positional_only,
            keyword_only,
            raises,
            error,
            conventions,
            ref_params,
            ref_return,
            transfers,
        } => Ty::Func {
            environment: environment.clone(),
            params: params
                .iter()
                .map(|ty| {
                    instantiate_checked_type(ty, type_arguments, value_arguments, bound_values)
                })
                .collect::<Result<Vec<_>, _>>()?,
            names: names.clone(),
            ret: Box::new(instantiate_checked_type(
                ret,
                type_arguments,
                value_arguments,
                bound_values,
            )?),
            required: required.clone(),
            variadic: variadic
                .as_ref()
                .map(|ty| {
                    instantiate_checked_type(ty, type_arguments, value_arguments, bound_values)
                        .map(Box::new)
                })
                .transpose()?,
            kw_variadic: kw_variadic
                .as_ref()
                .map(|ty| {
                    instantiate_checked_type(ty, type_arguments, value_arguments, bound_values)
                        .map(Box::new)
                })
                .transpose()?,
            positional_only: *positional_only,
            keyword_only: *keyword_only,
            raises: *raises,
            error: error
                .as_ref()
                .map(|ty| {
                    instantiate_checked_type(ty, type_arguments, value_arguments, bound_values)
                        .map(Box::new)
                })
                .transpose()?,
            conventions: conventions.clone(),
            ref_params: ref_params.clone(),
            ref_return: ref_return.clone(),
            transfers: transfers.clone(),
        },
        // Nested generic callable contracts own their own binder scope. The
        // outer verifier validates that scope recursively; retaining it is
        // sound and avoids capturing same-spelled outer substitution names.
        Ty::GenericFunc { .. } => ty.clone(),
        _ => ty.clone(),
    })
}

/// Whether two instantiations of one struct hold, in the same slot, distinct
/// residual expressions over the same set of parameters. An instance binding
/// maps both alike, so they can never become equal; residuals over different
/// binders (a caller's `n + 1` against a callee's `m + 1`) are not judged.
pub(super) fn residual_arguments_conflict(found: &Ty, expected: &Ty) -> bool {
    use mojito_types::ct::CtValue;
    use mojito_types::types::TyArg;
    let (Ty::Struct(found_name, found_args), Ty::Struct(expected_name, expected_args)) =
        (found, expected)
    else {
        return false;
    };
    found_name == expected_name
        && found_args.len() == expected_args.len()
        && found_args.iter().zip(expected_args).any(|pair| match pair {
            (TyArg::Val(CtValue::Expr(left)), TyArg::Val(CtValue::Expr(right))) => {
                left != right && left.free_parameters() == right.free_parameters()
            }
            (TyArg::Ty(left), TyArg::Ty(right)) => residual_arguments_conflict(left, right),
            _ => false,
        })
}

pub(super) fn contains_type_param(ty: &Ty) -> bool {
    match ty {
        Ty::Param { .. } | Ty::Assoc { .. } | Ty::Dependent(_) => true,
        Ty::ComptimeList(inner) | Ty::Pointer { element: inner, .. } => contains_type_param(inner),
        Ty::Tuple(elements) | Ty::RuntimePack(elements) | Ty::Variant(elements) => {
            elements.iter().any(contains_type_param)
        }
        Ty::Ref(reference) => contains_type_param(&reference.referent),
        Ty::Struct(_, arguments) => arguments.iter().any(|argument| match argument {
            mojito_types::types::TyArg::Ty(inner) => contains_type_param(inner),
            // A symbolic value argument (`Counter[Self.length]` in an erased
            // method's signature) is an ABI slot the instance binds.
            mojito_types::types::TyArg::Val(value) => {
                matches!(
                    value,
                    mojito_types::ct::CtValue::Expr(_) | mojito_types::ct::CtValue::Deferred(_)
                )
            }
            mojito_types::types::TyArg::Origin(_) => false,
        }),
        Ty::Func {
            params,
            ret,
            variadic,
            kw_variadic,
            error,
            ..
        }
        | Ty::GenericFunc {
            params,
            ret,
            variadic,
            kw_variadic,
            error,
            ..
        } => {
            params.iter().any(contains_type_param)
                || contains_type_param(ret)
                || variadic.as_deref().is_some_and(contains_type_param)
                || kw_variadic.as_deref().is_some_and(contains_type_param)
                || error.as_deref().is_some_and(contains_type_param)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mojito_ast::ast::InfixOp;
    use mojito_types::ct::CtValue;
    use mojito_types::param_expr::{MetaTy, ParamExpr, ParamId};
    use mojito_types::types::TyArg;

    fn sized(size: ParamExpr) -> Ty {
        Ty::Struct("Buf".into(), vec![TyArg::Val(size.into_value())])
    }

    fn plus(context: &ParamContext, parameter: &ParamExpr, offset: i64) -> ParamExpr {
        context
            .infix(
                InfixOp::Add,
                parameter,
                &context
                    .constant(CtValue::Int(offset))
                    .expect("an Int is a constant"),
            )
            .expect("Int + Int builds")
    }

    /// A parameter occurring on both sides is not a wildcard: two residual
    /// sizes over the same binder are compatible only as one canonical node.
    #[test]
    fn distinct_residual_sizes_are_not_compatible() {
        let context = ParamContext::new();
        let n = context.decl_ref(ParamId::new("f", 0), "n", MetaTy::int());
        let m = context.decl_ref(ParamId::new("g", 0), "m", MetaTy::int());
        assert!(types_compatible(
            &sized(plus(&context, &n, 1)),
            &sized(plus(&context, &n, 1))
        ));
        assert!(!types_compatible(
            &sized(plus(&context, &n, 1)),
            &sized(plus(&context, &n, 2))
        ));
        // Residuals over different binders are an instance binding's to
        // relate, so the verifier does not judge them.
        assert!(types_compatible(
            &sized(plus(&context, &n, 1)),
            &sized(plus(&context, &m, 1))
        ));
        // A bound residual re-folds and then compares as a constant.
        let bound = instantiate_checked_type(
            &sized(plus(&context, &n, 1)),
            &HashMap::new(),
            &HashMap::from([("n".to_string(), CtValue::Int(3))]),
            &HashSet::new(),
        )
        .expect("instantiation");
        assert_eq!(
            bound,
            Ty::Struct("Buf".into(), vec![TyArg::Val(CtValue::Int(4))])
        );
    }

    #[test]
    fn signature_slots_and_holes_are_checked_at_the_mir_boundary() {
        let context = ParamContext::new();
        let dangling = sized(context.index_ref(0, 0, MetaTy::int()));
        assert!(
            super::super::calls::validate_dependent_bindings(&dangling)
                .is_err_and(|finding| finding.contains("names no enclosing binder"))
        );
        let hole = sized(context.hole(mojito_types::param_expr::HoleKind::Unbound, MetaTy::int()));
        assert!(
            super::super::calls::validate_dependent_bindings(&hole)
                .is_err_and(|finding| finding.contains("cannot cross into MIR"))
        );
        // A lane dtype or width still symbolic belongs to a validated
        // template, never to a clone below the waist.
        let symbolic = Ty::Simd {
            dtype: mojito_types::types::SimdDtype::Known(mojito_ast::ast::Dtype::Int32),
            width: mojito_types::types::SimdWidth::Expr(context.decl_ref(
                mojito_types::param_expr::ParamId::new("f", 0),
                "width",
                MetaTy::int(),
            )),
        };
        assert!(
            super::super::calls::validate_dependent_bindings(&symbolic)
                .is_err_and(|finding| finding.contains("symbolic SIMD type"))
        );
    }
}
