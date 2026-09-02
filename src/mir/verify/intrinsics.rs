//! Intrinsic index/slice verification and generic callable-contract
//! instantiation.

use super::*;

pub(super) fn verify_intrinsic_index(
    prefix: &str,
    intrinsic: MirIntrinsicSubscript,
    base: Option<&Ty>,
    index: Option<&Ty>,
    dest: Option<&Ty>,
    errors: &mut Vec<String>,
) {
    if let Some(index) = index
        && !types_compatible(index, &Ty::Int)
        && !matches!(index, Ty::UInt)
    {
        errors.push(format!(
            "{prefix}: intrinsic index has non-Indexer register type {index}"
        ));
    }

    let result_candidates = match (intrinsic, base) {
        (MirIntrinsicSubscript::TupleStorage, Some(Ty::Tuple(elements)))
        | (MirIntrinsicSubscript::TupleStorage, Some(Ty::RuntimePack(elements))) => {
            Some(elements.iter().collect::<Vec<_>>())
        }
        (MirIntrinsicSubscript::TupleStorage, Some(base)) => tuple_elements(base),
        (MirIntrinsicSubscript::VariadicStorage, Some(Ty::VariadicPack(element))) => {
            Some(vec![element.as_ref()])
        }
        (MirIntrinsicSubscript::Simd, Some(Ty::Simd { dtype, .. })) => {
            let scalar = simd_element_type(*dtype);
            if let Some(dest) = dest
                && !types_compatible(dest, &scalar)
            {
                errors.push(format!(
                    "{prefix}: SIMD intrinsic result has type {dest}, expected {scalar}"
                ));
            }
            return;
        }
        (MirIntrinsicSubscript::Pointer, Some(Ty::Pointer { element, .. }))
        | (MirIntrinsicSubscript::ComptimeList, Some(Ty::ComptimeList(element))) => {
            Some(vec![element.as_ref()])
        }
        (_, Some(base)) => {
            errors.push(format!(
                "{prefix}: intrinsic {intrinsic:?} is incompatible with checked base type {base}"
            ));
            return;
        }
        (_, None) => return,
    };

    if let (Some(dest), Some(candidates)) = (dest, result_candidates)
        && !candidates.iter().any(|candidate| {
            types_compatible(dest, candidate)
                || matches!(candidate, Ty::Ref(reference) if types_compatible(dest, &reference.referent))
        })
    {
        errors.push(format!(
            "{prefix}: intrinsic {intrinsic:?} result type {dest} is incompatible with its checked element type"
        ));
    }
}

/// Element types a raw MIR place projection may select. Nominal collection
/// indexing is absent by construction: it must retain a checked call contract
/// instead of becoming storage navigation. Public Tuple spellings are accepted
/// only for the narrow compiler-owned storage bridge already represented by a
/// projection place.
pub(super) fn indexed_place_element_types(base: &Ty) -> Option<Vec<Ty>> {
    match base {
        Ty::Tuple(elements) | Ty::RuntimePack(elements) => Some(elements.clone()),
        Ty::VariadicPack(element) | Ty::ComptimeList(element) | Ty::Pointer { element, .. } => {
            Some(vec![(**element).clone()])
        }
        Ty::Simd { dtype, .. } => Some(vec![simd_element_type(*dtype)]),
        other => tuple_elements(other).map(|elements| elements.into_iter().cloned().collect()),
    }
}

pub(super) fn verify_intrinsic_slice(
    prefix: &str,
    intrinsic: MirIntrinsicSubscript,
    _base: Option<&Ty>,
    _dest: Option<&Ty>,
    errors: &mut Vec<String>,
) {
    // No intrinsic slice dispatch remains (StringLiteral positional slicing
    // was removed at the audited head); any recorded kind is a verifier error.
    errors.push(format!(
        "{prefix}: intrinsic {intrinsic:?} is not a slice dispatch"
    ));
}

/// Rebuild the executable callable shape from the symbolic generic contract.
/// The full dependent/type substitution is performed here rather than by
/// inspecting parameter-materialization instructions; `arguments` is the
/// checker-retained declaration-order witness.
pub(super) fn instantiate_generic_callable_contract(
    contract: &Ty,
    arguments: &[TyArg],
) -> Result<Ty, String> {
    let Ty::GenericFunc {
        environment,
        decls,
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
    } = contract
    else {
        return Err("retained contract is not generic".to_string());
    };
    if decls.len() != arguments.len() {
        return Err(format!(
            "{} retained argument(s) for {} declaration(s)",
            arguments.len(),
            decls.len()
        ));
    }
    let argument_maps = generic_argument_maps(decls, arguments)?;
    let bound = HashSet::new();
    let mut instantiate =
        |ty: &Ty| instantiate_checked_type(ty, &argument_maps.types, &argument_maps.values, &bound);
    let instantiated = Ty::Func {
        environment: environment.clone(),
        params: params
            .iter()
            .map(&mut instantiate)
            .collect::<Result<Vec<_>, _>>()?,
        names: names.clone(),
        ret: Box::new(instantiate(ret)?),
        required: required.clone(),
        variadic: variadic
            .as_ref()
            .map(|ty| instantiate(ty).map(Box::new))
            .transpose()?,
        kw_variadic: kw_variadic
            .as_ref()
            .map(|ty| instantiate(ty).map(Box::new))
            .transpose()?,
        positional_only: *positional_only,
        keyword_only: *keyword_only,
        raises: *raises,
        error: error
            .as_ref()
            .map(|ty| instantiate(ty).map(Box::new))
            .transpose()?,
        conventions: conventions.clone(),
        ref_params: ref_params.clone(),
        ref_return: ref_return.clone(),
        transfers: transfers.clone(),
    };
    validate_dependent_bindings(&instantiated)?;
    Ok(instantiated)
}

pub(super) fn generic_argument_maps(
    declarations: &[ParamDecl],
    arguments: &[TyArg],
) -> Result<GenericArgumentMaps, String> {
    let mut types = HashMap::new();
    let mut values = HashMap::new();
    for (declaration, argument) in declarations.iter().zip(arguments) {
        let name = declaration.name().trim_start_matches('*').to_string();
        match (declaration, argument) {
            (ParamDecl::Type { .. }, TyArg::Ty(ty)) => {
                types.insert(name, ty.clone());
            }
            (ParamDecl::Value { .. }, TyArg::Val(value))
            | (ParamDecl::Type { variadic: true, .. }, TyArg::Val(value)) => {
                values.insert(name, value.clone());
            }
            (ParamDecl::Type { .. }, TyArg::Val(_)) => {
                return Err(format!(
                    "type parameter '{}' retains a value argument",
                    declaration.name()
                ));
            }
            (ParamDecl::Value { .. }, TyArg::Ty(_)) => {
                return Err(format!(
                    "value parameter '{}' retains a type argument",
                    declaration.name()
                ));
            }
            // Origins erase from the runtime ABI, so they contribute no entry to
            // the type/value argument maps used for generic instantiation.
            (_, TyArg::Origin(_)) => {}
        }
    }
    Ok(GenericArgumentMaps { types, values })
}

/// Verify an indirect call whose callee register carries a checked anonymous
/// callable contract. Unlike nominal dispatch, there is no concrete MIR
/// declaration to consult: the `Ty::Func` retained on the bounded parameter is
/// the declaration and must be checked directly.
#[allow(clippy::too_many_arguments)]
pub(super) fn verify_callable_contract_call(
    prefix: &str,
    function: &MirFunction,
    contract: &Ty,
    call_raises: &Option<Ty>,
    dest: Reg,
    args: &[Reg],
    kwargs: &[(String, Reg)],
    arg_places: &[Option<MirPlace>],
    errors: &mut Vec<String>,
) {
    let (params, ret, required, variadic, kw_variadic, conventions, ref_return, raises, error) =
        match contract {
            Ty::Func {
                params,
                ret,
                required,
                variadic,
                kw_variadic,
                conventions,
                ref_return,
                raises,
                error,
                ..
            }
            | Ty::GenericFunc {
                params,
                ret,
                required,
                variadic,
                kw_variadic,
                conventions,
                ref_return,
                raises,
                error,
                ..
            } => (
                params,
                ret,
                required,
                variadic,
                kw_variadic,
                conventions,
                ref_return,
                raises,
                error,
            ),
            _ => return,
        };
    if *raises != call_raises.is_some() {
        errors.push(format!(
            "{prefix}: indirect-call raising metadata does not match its callable contract"
        ));
    } else if let (Some(found), Some(expected)) = (call_raises, error.as_deref())
        && !types_compatible(found, expected)
    {
        errors.push(format!(
            "{prefix}: indirect-call error type {found} does not match callable contract {expected}"
        ));
    }
    if kwargs.is_empty() && variadic.is_none() && kw_variadic.is_none() {
        let omitted_required = required
            .iter()
            .skip(args.len())
            .copied()
            .any(|is_required| is_required);
        if args.len() > params.len() || omitted_required {
            errors.push(format!(
                "{prefix}: indirect call has {} positional arguments, callable contract requires {}",
                args.len(),
                required.iter().filter(|required| **required).count()
            ));
        } else {
            for (index, (argument, expected)) in args.iter().zip(params).enumerate() {
                if let Some(found) = function.reg_types.get(&argument.0)
                    && !types_compatible(found, expected)
                {
                    errors.push(format!(
                        "{prefix}: argument {index} of indirect callable has type {found}, contract declares {expected}"
                    ));
                }
                if matches!(
                    conventions.get(index).copied().flatten(),
                    Some(crate::ast::ArgConvention::Mut | crate::ast::ArgConvention::Ref)
                ) && arg_places.get(index).and_then(Option::as_ref).is_none()
                {
                    errors.push(format!(
                        "{prefix}: mutable/reference parameter {index} of indirect callable has no caller place"
                    ));
                }
            }
        }
        if required.len() != params.len() {
            errors.push(format!(
                "{prefix}: callable contract required-mask length does not match its parameters"
            ));
        }
    }
    if let Some(found) = function.reg_types.get(&dest.0) {
        let found_value = match (ref_return.is_some(), found) {
            (true, Ty::Ref(reference)) => {
                if let Some(signature) = ref_return.as_deref() {
                    let mutability_matches = match signature.mutability {
                        crate::origin::SigMutability::Immutable => {
                            reference.mutability == crate::origin::Mutability::Immutable
                        }
                        crate::origin::SigMutability::Mutable => {
                            reference.mutability == crate::origin::Mutability::Mutable
                        }
                        crate::origin::SigMutability::BoolParam(_)
                        | crate::origin::SigMutability::Infer => true,
                    };
                    if !mutability_matches {
                        errors.push(format!(
                            "{prefix}: indirect-call reference result mutability does not match its callable contract"
                        ));
                    }
                    let origin_matches = match &signature.origin {
                        crate::origin::SigOrigin::Bound(expected) => &reference.origin == expected,
                        crate::origin::SigOrigin::Static => {
                            reference.origin == crate::origin::Origin::Static
                        }
                        crate::origin::SigOrigin::Untracked { mutable } => {
                            reference.origin
                                == crate::origin::Origin::Untracked { mutable: *mutable }
                        }
                        crate::origin::SigOrigin::Self_
                        | crate::origin::SigOrigin::Param(_)
                        | crate::origin::SigOrigin::Projected(_, _)
                        | crate::origin::SigOrigin::Union(_)
                        | crate::origin::SigOrigin::Infer => true,
                    };
                    if !origin_matches {
                        errors.push(format!(
                            "{prefix}: indirect-call reference result origin does not match its callable contract"
                        ));
                    }
                }
                reference.referent.as_ref()
            }
            (true, other) => {
                errors.push(format!(
                    "{prefix}: indirect-call result has type {other}, contract returns a reference to {ret}"
                ));
                return;
            }
            (false, Ty::Ref(_)) => {
                errors.push(format!(
                    "{prefix}: indirect-call result is a reference, contract returns {ret} by value"
                ));
                return;
            }
            (false, other) => other,
        };
        if !types_compatible(found_value, ret) {
            errors.push(format!(
                "{prefix}: indirect-call result has type {found}, contract returns {ret}"
            ));
        }
    }
}
