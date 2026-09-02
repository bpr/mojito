//! Type-level verification rules: compatibility, checked-type
//! instantiation, and iterator-result adapters.

use super::*;

/// Compatibility for verification purposes: either direction of the checker's
/// coercion predicate. Lowering emits checker-approved conversions before
/// values flow, so remaining differences are representational (literal
/// materialization, generic instantiation), not errors to re-litigate. A type
/// mentioning an unsubstituted parameter is not compared — instantiation is
/// the checker's domain and the verifier never re-derives it.
pub(super) fn types_compatible(found: &Ty, expected: &Ty) -> bool {
    pub(super) fn callable_environment(ty: &Ty) -> Option<&crate::origin::CallableEnvironment> {
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
        && !crate::checker::callable_environment_coerces(found, expected)
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
    if let (Ty::Struct(found_name, found_args), Ty::Struct(expected_name, expected_args)) =
        (found, expected)
        && found_name == expected_name
        && (found_args.is_empty() || expected_args.is_empty())
    {
        return true;
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
    crate::checker::value_coerces(found, expected) || crate::checker::value_coerces(expected, found)
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
    call: &crate::checked::CheckedIteratorCall,
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
    call: &crate::checked::CheckedIteratorCall,
    errors: &mut Vec<String>,
) -> bool {
    let abstract_dispatch = call.target == "__iterator_dispatch.__next__";
    match call.result_adapter {
        Some(crate::checked::CheckedResultAdapter::CopyIteratorReference) => {
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
            crate::types::TyArg::Ty(inner) => contains_runtime_pack(inner),
            crate::types::TyArg::Val(_) | crate::types::TyArg::Origin(_) => false,
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
    type_arguments: &HashMap<String, Ty>,
    value_arguments: &HashMap<String, CtValue>,
    bound_values: &HashSet<String>,
) -> Result<Ty, String> {
    Ok(match ty {
        Ty::Param {
            name,
            bounds,
            callable_bound,
        } => type_arguments
            .get(name)
            .or_else(|| type_arguments.get(name.trim_start_matches('*')))
            .cloned()
            .unwrap_or_else(|| Ty::Param {
                name: name.clone(),
                bounds: bounds.clone(),
                callable_bound: callable_bound.clone(),
            }),
        Ty::Dependent(DependentType::Indexed { elements, index }) => {
            let elements = elements
                .iter()
                .map(|element| {
                    instantiate_checked_type(element, type_arguments, value_arguments, bound_values)
                })
                .collect::<Result<Vec<_>, _>>()?;
            let Some(value) = index.evaluate(value_arguments) else {
                let mut referenced = HashSet::new();
                index.referenced_parameters(&mut referenced);
                if !referenced.is_empty()
                    && referenced.iter().all(|name| bound_values.contains(name))
                {
                    return Ok(Ty::Dependent(DependentType::Indexed {
                        elements,
                        index: index.clone(),
                    }));
                }
                let mut unbound: Vec<_> = referenced.difference(bound_values).cloned().collect();
                unbound.sort();
                return Err(if unbound.is_empty() {
                    "dependent index did not evaluate to a compile-time value".to_string()
                } else {
                    format!(
                        "dependent index references unsubstituted parameter(s): {}",
                        unbound.join(", ")
                    )
                });
            };
            let index_value = match value {
                CtValue::Int(value) => Some(value),
                CtValue::UInt(value) => i64::try_from(value).ok(),
                CtValue::IntLiteral(value) => value.to_i64(),
                _ => None,
            }
            .ok_or_else(|| "dependent index is not an Int".to_string())?;
            let position = usize::try_from(index_value)
                .map_err(|_| format!("dependent index {index_value} is negative"))?;
            elements.get(position).cloned().ok_or_else(|| {
                format!(
                    "dependent index {index_value} is out of range for {} element(s)",
                    elements.len()
                )
            })?
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
                    TyArg::Val(value) => Ok(TyArg::Val(value.clone())),
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

pub(super) fn contains_type_param(ty: &Ty) -> bool {
    match ty {
        Ty::Param { .. } | Ty::Assoc { .. } | Ty::Dependent(_) => true,
        Ty::ComptimeList(inner) | Ty::Pointer { element: inner, .. } => contains_type_param(inner),
        Ty::Tuple(elements) | Ty::RuntimePack(elements) | Ty::Variant(elements) => {
            elements.iter().any(contains_type_param)
        }
        Ty::Ref(reference) => contains_type_param(&reference.referent),
        Ty::Struct(_, arguments) => arguments.iter().any(|argument| match argument {
            crate::types::TyArg::Ty(inner) => contains_type_param(inner),
            crate::types::TyArg::Val(_) | crate::types::TyArg::Origin(_) => false,
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
