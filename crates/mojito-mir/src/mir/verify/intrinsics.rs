//! Intrinsic index/slice verification and generic callable-contract
//! instantiation.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::TyKind;
use mojito_types::types::{FuncTy, GenericFuncTy};

pub(super) fn verify_intrinsic_index(
    prefix: &str,
    intrinsic: MirIntrinsicSubscript,
    base: Option<&Ty>,
    index: Option<&Ty>,
    dest: Option<&Ty>,
    errors: &mut Vec<String>,
) {
    if let Some(index) = index
        && index.is_int_literal()
    {
        errors.push(format!(
            "{prefix}: intrinsic index is the literal {index}, which no conversion \
             materialized at Int"
        ));
    } else if let Some(index) = index
        && !types_compatible(index, &Ty::int())
        && !index.is_uint()
    {
        errors.push(format!(
            "{prefix}: intrinsic index has non-Indexer register type {index}"
        ));
    }

    let references: Vec<Ty>;
    let result_candidates = match (intrinsic, base.map(Ty::kind)) {
        (MirIntrinsicSubscript::TupleStorage, Some(TyKind::LitRefPack { elements, origin })) => {
            references = elements
                .iter()
                .map(|element| {
                    Ty::lit_ref(
                        element.clone(),
                        origin.clone(),
                        mojito_types::types::AddressSpaceSlot::GENERIC,
                    )
                })
                .collect();
            Some(references.iter().collect::<Vec<_>>())
        }
        (
            MirIntrinsicSubscript::TupleStorage,
            Some(TyKind::Tuple(elements) | TyKind::RuntimePack(elements)),
        ) => Some(elements.iter().collect::<Vec<_>>()),
        (MirIntrinsicSubscript::TupleStorage, Some(base)) => tuple_elements(base),
        (MirIntrinsicSubscript::VariadicStorage, Some(TyKind::VariadicPack(element))) => {
            Some(vec![element])
        }
        // An elaborated pack is the tuple its call site collected: the
        // elaborator rewrites the pack's type at the instance's arity and
        // keeps the subscript. An empty instance keeps no element type to
        // check against; its every read is out of range and traps.
        (
            MirIntrinsicSubscript::VariadicStorage,
            Some(TyKind::Tuple(elements) | TyKind::RuntimePack(elements)),
        ) => (!elements.is_empty()).then(|| elements.iter().collect::<Vec<_>>()),
        (MirIntrinsicSubscript::Pointer, Some(pointer))
            if let Some(element) = pointer.pointer_element() =>
        {
            Some(vec![element])
        }
        (MirIntrinsicSubscript::ComptimeList, Some(TyKind::ComptimeList(element))) => {
            Some(vec![element])
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
                || matches!(candidate.kind(), TyKind::Ref(reference) if types_compatible(dest, &reference.referent))
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
    match base.kind() {
        TyKind::Tuple(elements) | TyKind::RuntimePack(elements) => Some(elements.clone()),
        TyKind::VariadicPack(element) | TyKind::ComptimeList(element) => {
            Some(vec![element.clone()])
        }
        pointer if let Some(element) = pointer.pointer_element() => Some(vec![element.clone()]),
        other => tuple_elements(other).map(|elements| elements.into_iter().cloned().collect()),
    }
}

/// Rebuild the executable callable shape from the symbolic generic contract.
/// The full dependent/type substitution is performed here rather than by
/// inspecting parameter-materialization instructions; `arguments` is the
/// checker-retained declaration-order witness. The caller's binders the
/// contract or its arguments name (`T._union_case_types[id]`, a `comptime
/// for`'s `$iter`, an enclosing pack `Ts`) stay bound in the result, as
/// `#kgen.bind_params` keeps them; only the contract's own binders must be
/// substituted.
pub(in crate::mir) fn instantiate_generic_callable_contract(
    contract: &Ty,
    arguments: &[TyArg],
) -> Result<Ty, String> {
    let Some(GenericFuncTy {
        decls: binders,
        func:
            FuncTy {
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
            },
    }) = contract.as_generic_func()
    else {
        return Err("retained contract is not generic".to_string());
    };
    let decls = binders.as_slice();
    if decls.len() != arguments.len() {
        return Err(format!(
            "{} retained argument(s) for {} declaration(s)",
            arguments.len(),
            decls.len()
        ));
    }
    let argument_maps = generic_argument_maps(decls, arguments)?;
    let packs = solved_packs(decls, arguments);
    let bound = caller_binders(contract, arguments);
    let mut instantiate = |ty: &Ty| {
        // A collector typed by a solved pack (`*args: *Ts`) collects its
        // elements; a spread of it elsewhere (`Tuple[*Ts]`) lists them.
        let expanded = match ty.kind() {
            TyKind::Param { binder, .. } => packs
                .iter()
                .find(|(pack, _)| *pack == binder.name.trim_start_matches('*'))
                .map_or_else(
                    || ty.clone(),
                    |(_, elements)| TyKind::RuntimePack(elements.clone()).into(),
                ),
            _ => packs.iter().fold(ty.clone(), |ty, (pack, elements)| {
                mojito_types::types::expand_pack_spread(&ty, pack, elements)
            }),
        };
        instantiate_checked_type(
            &expanded,
            &argument_maps.types,
            &argument_maps.values,
            &bound,
        )
    };
    let instantiated = Ty::new(TyKind::Func(Box::new(FuncTy {
        environment: environment.clone(),
        params: params
            .iter()
            .map(&mut instantiate)
            .collect::<Result<Vec<_>, _>>()?,
        names: names.clone(),
        ret: instantiate(ret)?,
        required: required.clone(),
        variadic: variadic.as_ref().map(&instantiate).transpose()?,
        kw_variadic: kw_variadic.as_ref().map(&instantiate).transpose()?,
        positional_only: *positional_only,
        keyword_only: *keyword_only,
        raises: *raises,
        error: error.as_ref().map(instantiate).transpose()?,
        conventions: conventions.clone(),
        ref_params: ref_params.clone(),
        ref_return: ref_return.clone(),
        transfers: transfers.clone(),
    })));
    validate_dependent_bindings_in(&instantiated, &bound)?;
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
            (ParamDecl::Type { id, .. }, TyArg::Ty(ty)) => {
                types.insert(id.clone(), ty.clone());
            }
            // A value the call leaves to the elaborated instance (an
            // origin's `mut=` the caller's binders decide) binds nothing
            // here.
            (_, TyArg::Val(CtValue::Deferred(_))) => {}
            (
                ParamDecl::Value { .. } | ParamDecl::Type { variadic: true, .. },
                TyArg::Val(value),
            ) => {
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
            (ParamDecl::Origin { .. }, _) => {
                return Err(format!(
                    "origin parameter '{}' retains a type or value argument",
                    declaration.name()
                ));
            }
        }
    }
    Ok(GenericArgumentMaps { types, values })
}

/// Verify an indirect call whose callee register carries a checked anonymous
/// callable contract. Unlike nominal dispatch, there is no concrete MIR
/// declaration to consult: the `TyKind::Func` retained on the bounded parameter is
/// the declaration and must be checked directly.
#[allow(
    clippy::too_many_arguments,
    clippy::ref_option,
    reason = "TODO: take Option<&T>"
)]
pub(super) fn verify_callable_contract_call(
    cx: &InstrCx<'_>,
    contract: &Ty,
    call_raises: Option<&Ty>,
    dest: Reg,
    args: &[Reg],
    kwargs: &[(String, Reg)],
    spread: Option<usize>,
    errors: &mut Vec<String>,
) {
    let (prefix, function) = (cx.prefix, cx.function);
    let Some(
        signature @ FuncTy {
            params,
            ret,
            required,
            conventions,
            ref_return,
            raises,
            error,
            ..
        },
    ) = contract.callable()
    else {
        return;
    };
    if *raises != call_raises.is_some() {
        errors.push(format!(
            "{prefix}: indirect-call raising metadata does not match its callable contract"
        ));
    } else if let (Some(found), Some(expected)) = (call_raises, error.as_ref())
        && !types_compatible(found, expected)
    {
        errors.push(format!(
            "{prefix}: indirect-call error type {found} does not match callable contract {expected}"
        ));
    }
    if required.len() != params.len() {
        errors.push(format!(
            "{prefix}: callable contract required-mask length does not match its parameters"
        ));
    }
    match crate::mir::signature_operand_parameters(signature, args.len(), spread, kwargs) {
        Some(parameters) => {
            let operands: Vec<_> = args
                .iter()
                .map(|argument| (*argument, None))
                .chain(
                    kwargs
                        .iter()
                        .map(|(name, argument)| (*argument, Some(name.as_str()))),
                )
                .zip(parameters)
                .map(
                    |((register, keyword), parameter)| crate::mir::BoundOperand {
                        register,
                        keyword,
                        parameter,
                    },
                )
                .collect();
            super::calls::verify_operand_types(
                cx,
                "indirect callable",
                params,
                conventions,
                &operands,
                errors,
            );
            let addressed = crate::mir::signature_by_address_operands(
                signature,
                args.len(),
                spread,
                kwargs,
                cx.declarations,
            )
            .unwrap_or_default();
            for (index, (operand, by_address)) in operands.iter().zip(addressed).enumerate() {
                if by_address.is_some()
                    && let Some(found) = function.reg_types.get(&operand.register.0)
                    && !matches!(found.kind(), TyKind::Ref(_))
                {
                    errors.push(format!(
                        "{prefix}: argument {index} of indirect callable is received by address but passes a {found} value"
                    ));
                }
            }
        }
        None => errors.push(format!(
            "{prefix}: indirect call does not bind its callable contract"
        )),
    }
    if let Some(found) = function.reg_types.get(&dest.0) {
        let found_value = match (ref_return.is_some(), found.kind()) {
            (true, TyKind::Ref(reference)) => {
                if let Some(signature) = ref_return.as_deref() {
                    let mutability_matches = match signature.mutability {
                        mojito_types::origin::SigMutability::Immutable => {
                            reference.mutability == mojito_types::origin::Mutability::Immutable
                        }
                        mojito_types::origin::SigMutability::Mutable => {
                            reference.mutability == mojito_types::origin::Mutability::Mutable
                        }
                        mojito_types::origin::SigMutability::BoolParam(_)
                        | mojito_types::origin::SigMutability::Infer => true,
                    };
                    if !mutability_matches {
                        errors.push(format!(
                            "{prefix}: indirect-call reference result mutability does not match its callable contract"
                        ));
                    }
                    let origin_matches = match &signature.origin {
                        mojito_types::origin::SigOrigin::Bound(expected) => {
                            &reference.origin == expected
                        }
                        mojito_types::origin::SigOrigin::Static => {
                            reference.origin == mojito_types::origin::Origin::Static
                        }
                        mojito_types::origin::SigOrigin::Untracked { mutable } => {
                            reference.origin
                                == mojito_types::origin::Origin::Untracked { mutable: *mutable }
                        }
                        mojito_types::origin::SigOrigin::UnsafeAny { .. }
                        | mojito_types::origin::SigOrigin::Self_
                        | mojito_types::origin::SigOrigin::Param(_)
                        | mojito_types::origin::SigOrigin::Projected(_, _)
                        | mojito_types::origin::SigOrigin::Union(_)
                        | mojito_types::origin::SigOrigin::Infer
                        | mojito_types::origin::SigOrigin::Carried(_) => true,
                    };
                    if !origin_matches {
                        errors.push(format!(
                            "{prefix}: indirect-call reference result origin does not match its callable contract"
                        ));
                    }
                }
                &reference.referent
            }
            (true, _) => {
                errors.push(format!(
                    "{prefix}: indirect-call result has type {found}, contract returns a reference to {ret}"
                ));
                return;
            }
            (false, TyKind::Ref(_)) => {
                errors.push(format!(
                    "{prefix}: indirect-call result is a reference, contract returns {ret} by value"
                ));
                return;
            }
            (false, _) => &found.clone(),
        };
        if !types_compatible(found_value, ret) {
            errors.push(format!(
                "{prefix}: indirect-call result has type {found}, contract returns {ret}"
            ));
        }
    }
}

/// The packs `declarations` declares that `arguments` solve, each with its
/// element types.
fn solved_packs<'a>(declarations: &'a [ParamDecl], arguments: &[TyArg]) -> Vec<(&'a str, Vec<Ty>)> {
    declarations
        .iter()
        .zip(arguments)
        .filter_map(|(declaration, argument)| match (declaration, argument) {
            (ParamDecl::Type { variadic: true, .. }, TyArg::Val(CtValue::Tuple(values))) => values
                .iter()
                .map(|value| match value {
                    CtValue::Type(ty) => Some(ty.clone()),
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()
                .map(|elements| (declaration.name().trim_start_matches('*'), elements)),
            _ => None,
        })
        .collect()
}

/// The caller's binders a call through `contract` with `arguments` names:
/// those free in the symbolic contract and those its arguments reference.
fn caller_binders(contract: &Ty, arguments: &[TyArg]) -> HashSet<String> {
    let mut binders = HashSet::new();
    mojito_types::types::referenced_parameters(contract, &mut binders);
    for argument in arguments {
        match argument {
            TyArg::Ty(ty) | TyArg::Val(CtValue::Type(ty)) => {
                mojito_types::types::referenced_parameters(ty, &mut binders);
            }
            TyArg::Val(CtValue::Expr(expr)) => expr.referenced_parameters(&mut binders),
            _ => {}
        }
    }
    binders
}
