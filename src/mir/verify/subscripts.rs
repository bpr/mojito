//! Subscript receiver/call verification and parameter-argument
//! checks.

use super::*;

/// Runtime frame/slot capabilities are represented either by a source-level
/// `ref T` or by an origin-bearing `UnsafePointer[T, origin]`. Raw/static/
/// untracked pointer values use allocation arithmetic and are not valid
/// operands for `ReadRef`/`WriteRef`.
pub(super) fn reference_capability(ty: &Ty) -> Option<ReferenceCapability<'_>> {
    match ty {
        Ty::Ref(reference) => Some(ReferenceCapability {
            target: &reference.referent,
            permission: ReferencePermission::from_mutability(reference.mutability),
        }),
        Ty::Pointer { element, origin } => {
            let permission = match origin {
                PointerOrigin::Place { mutable, .. } => {
                    if *mutable {
                        ReferencePermission::Mutable
                    } else {
                        ReferencePermission::Immutable
                    }
                }
                PointerOrigin::Param { mutability, .. }
                | PointerOrigin::SelfPlace { mutability, .. } => {
                    ReferencePermission::from_mutability(*mutability)
                }
                PointerOrigin::Static
                | PointerOrigin::Untracked { .. }
                | PointerOrigin::UnsafeAny { .. } => return None,
            };
            Some(ReferenceCapability {
                target: element,
                permission,
            })
        }
        _ => None,
    }
}

pub(super) fn verify_subscript_receiver_place(
    prefix: &str,
    receiver_ty: Option<&Ty>,
    receiver_place: Option<&MirPlace>,
    errors: &mut Vec<String>,
) {
    let (Some(found), Some(storage)) = (
        receiver_ty,
        receiver_place.and_then(|place| place.ty.as_ref()),
    ) else {
        return;
    };
    let receiver = match storage {
        Ty::Ref(reference) => reference.referent.as_ref(),
        other => other,
    };
    // The loaded base register may legitimately stay reference-typed one
    // level above its place (the VM's LoadPlace second dereference resolves
    // it at runtime); peel exactly one level, symmetric with the storage
    // peel above, so genuine mismatches still fail.
    let found = match found {
        Ty::Ref(reference) => reference.referent.as_ref(),
        other => other,
    };
    if !types_compatible(found, receiver) {
        errors.push(format!(
            "{prefix}: subscript receiver place type {receiver} does not match base value type {found}"
        ));
    }
}

pub(super) fn verify_subscript_call(
    prefix: &str,
    function: &MirFunction,
    declarations: &MirDeclarations,
    call: &crate::mir::MirSubscriptCall,
    sources: SubscriptSources<'_>,
    errors: &mut Vec<String>,
) {
    let SubscriptSources {
        receiver_ty,
        method,
        receiver_place,
        positional_places,
        keyword_places,
        positional_types,
        keyword_types,
        dest,
    } = sources;
    verify_capture_accesses(prefix, function, &call.capture_accesses, errors);
    let abstract_trait_dispatch = call.target.starts_with("__trait_dispatch.");
    let target_family = call.target.rsplit_once('.').is_some_and(|(_, symbol)| {
        symbol == method
            || symbol
                .strip_prefix(method)
                .is_some_and(|suffix| suffix.starts_with('$'))
            || method == "__getitem__"
                && (symbol == "__getitem_param__"
                    || symbol.starts_with("__getitem_param__$")
                    || symbol.starts_with("__getitem_param_value__$"))
    });
    if !target_family {
        errors.push(format!(
            "{prefix}: selected subscript target '{}' is not in the {method} method family",
            call.target
        ));
    }
    let concrete_receiver = match receiver_ty {
        Some(Ty::Struct(name, _)) => Some(name.as_str()),
        Some(Ty::Ref(reference)) => match reference.referent.as_ref() {
            Ty::Struct(name, _) => Some(name.as_str()),
            _ => None,
        },
        _ => None,
    };
    if !abstract_trait_dispatch
        && let Some(receiver) = concrete_receiver
        && call
            .target
            .rsplit_once('.')
            .is_some_and(|(owner, _)| owner != receiver)
    {
        errors.push(format!(
            "{prefix}: selected subscript target '{}' does not belong to receiver type {receiver}",
            call.target
        ));
    }
    if call.receiver_requires_place && receiver_place.is_none() {
        errors.push(format!(
            "{prefix}: selected subscript reference receiver has no retained caller place"
        ));
    }
    if positional_places.len() != positional_types.len()
        || keyword_places.len() != keyword_types.len()
    {
        errors.push(format!(
            "{prefix}: subscript argument place/type metadata is not aligned"
        ));
    }
    let mut positional_uses = vec![0usize; positional_places.len()];
    let mut keyword_uses = vec![0usize; keyword_places.len()];
    for argument in &call.arguments {
        let (retained, actual_ty) = match argument.source {
            crate::checked::CheckedCallArgumentSource::Positional(index) => {
                if let Some(uses) = positional_uses.get_mut(index) {
                    *uses += 1;
                } else {
                    errors.push(format!(
                        "{prefix}: selected subscript positional source {index} is out of range"
                    ));
                }
                (
                    positional_places.get(index).and_then(Option::as_ref),
                    positional_types.get(index).and_then(Option::as_ref),
                )
            }
            crate::checked::CheckedCallArgumentSource::Keyword(index) => {
                if let Some(uses) = keyword_uses.get_mut(index) {
                    *uses += 1;
                } else {
                    errors.push(format!(
                        "{prefix}: selected subscript keyword source {index} is out of range"
                    ));
                }
                (
                    keyword_places.get(index).and_then(Option::as_ref),
                    keyword_types.get(index).and_then(Option::as_ref),
                )
            }
            crate::checked::CheckedCallArgumentSource::Default => (None, None),
        };
        if argument.requires_place && retained.is_none() {
            errors.push(format!(
                "{prefix}: selected subscript mut/ref argument has no retained caller place"
            ));
        }
        if let Some(actual_ty) = actual_ty
            && !types_compatible(actual_ty, &argument.parameter_ty)
        {
            errors.push(format!(
                "{prefix}: subscript source has type {actual_ty}, selected parameter expects {}",
                argument.parameter_ty
            ));
        }
        if matches!(
            argument.convention,
            Some(crate::ast::ArgConvention::Mut | crate::ast::ArgConvention::Ref)
        ) && !argument.requires_place
        {
            errors.push(format!(
                "{prefix}: selected subscript mut/ref convention does not require a caller place"
            ));
        }
    }
    for (index, uses) in positional_uses.iter().enumerate() {
        if *uses != 1 {
            errors.push(format!(
                "{prefix}: subscript positional source {index} is represented {uses} times"
            ));
        }
    }
    for (index, uses) in keyword_uses.iter().enumerate() {
        if *uses != 1 {
            errors.push(format!(
                "{prefix}: subscript keyword source {index} is represented {uses} times"
            ));
        }
    }
    let retained_reference_ty = call
        .reference_result
        .as_ref()
        .map(|reference| Ty::Ref(reference.clone()));
    match &retained_reference_ty {
        Some(expected) if &call.result_ty != expected => errors.push(format!(
            "{prefix}: subscript reference-result metadata {expected} does not match selected result type {}",
            call.result_ty
        )),
        None if matches!(call.result_ty, Ty::Ref(_)) => errors.push(format!(
            "{prefix}: subscript selected result type {} lacks reference-result metadata",
            call.result_ty
        )),
        _ => {}
    }
    if let Some(dest) = dest
        && let Some(found) = function.reg_types.get(&dest.0)
    {
        let exact_reference_mismatch = matches!(
            (found, &call.result_ty),
            (Ty::Ref(found), Ty::Ref(expected)) if found != expected
        );
        if exact_reference_mismatch || !types_compatible(found, &call.result_ty) {
            errors.push(format!(
                "{prefix}: subscript result has type {found}, selected contract returns {}",
                call.result_ty
            ));
        }
    }
    verify_param_arguments(
        prefix,
        function,
        &call.param_decls,
        &call.param_arg_regs,
        errors,
    );
    match declared(declarations, &call.target) {
        // Trait-bound dispatch is intentionally abstract in checked MIR. Its
        // complete selected argument/result contract is still verified here;
        // the VM retargets the symbol to the concrete receiver declaration.
        None if abstract_trait_dispatch => {}
        None => errors.push(format!(
            "{prefix}: subscript refers to undeclared method '{}'",
            call.target
        )),
        Some(declaration) => {
            if !declaration.has_receiver {
                errors.push(format!(
                    "{prefix}: selected subscript target '{}' has no declared receiver",
                    call.target
                ));
            }
            if !effective_call_convention_matches(
                declaration.receiver_convention,
                call.receiver_convention,
            ) {
                errors.push(format!(
                    "{prefix}: subscript receiver convention {:?} does not match '{}' declaration {:?}",
                    call.receiver_convention, call.target, declaration.receiver_convention
                ));
            }
            let declared_receiver_place = matches!(
                declaration.receiver_convention,
                Some(crate::ast::ArgConvention::Mut | crate::ast::ArgConvention::Ref)
            );
            if call.receiver_requires_place != declared_receiver_place {
                errors.push(format!(
                    "{prefix}: subscript receiver place requirement does not match '{}' declaration",
                    call.target
                ));
            }
            if declaration.param_decls != call.param_decls {
                errors.push(format!(
                    "{prefix}: subscript generic declaration metadata does not match '{}'",
                    call.target
                ));
            }
            if declaration.raises != call.raises.is_some() {
                errors.push(format!(
                    "{prefix}: subscript raising metadata does not match '{}'",
                    call.target
                ));
            } else if let (Some(found), Some(expected)) =
                (&call.raises, declaration.error_ty.as_ref())
                && !types_compatible(found, expected)
            {
                errors.push(format!(
                    "{prefix}: subscript error type {found} does not match '{}' contract {expected}",
                    call.target
                ));
            }
            if declaration.returns_reference != call.reference_result.is_some() {
                errors.push(format!(
                    "{prefix}: subscript reference-result ABI does not match '{}' declaration",
                    call.target
                ));
            }
            if let Some(reference) = &call.reference_result
                && !types_compatible(reference.referent.as_ref(), &declaration.ret_ty)
            {
                errors.push(format!(
                    "{prefix}: subscript reference-result referent {} does not match '{}' declaration {}",
                    reference.referent, call.target, declaration.ret_ty
                ));
            }
            if call.reference_result.is_none()
                && !types_compatible(&call.result_ty, &declaration.ret_ty)
            {
                errors.push(format!(
                    "{prefix}: subscript selected result type {} does not match '{}' declaration {}",
                    call.result_ty, call.target, declaration.ret_ty
                ));
            }
            if call.arguments.len() < declaration.param_types.len() {
                errors.push(format!(
                    "{prefix}: selected subscript has {} argument contract(s), but '{}' declares {} fixed parameter(s)",
                    call.arguments.len(),
                    call.target,
                    declaration.param_types.len()
                ));
            }
            for (index, expected) in declaration.param_types.iter().enumerate() {
                let Some(argument) = call.arguments.get(index) else {
                    continue;
                };
                let declared_convention =
                    declaration.param_conventions.get(index).copied().flatten();
                if !effective_call_convention_matches(declared_convention, argument.convention) {
                    errors.push(format!(
                        "{prefix}: selected subscript parameter {index} convention {:?} does not match '{}' declaration {:?}",
                        argument.convention, call.target, declared_convention
                    ));
                }
                if !types_compatible(&argument.parameter_ty, expected) {
                    errors.push(format!(
                        "{prefix}: selected subscript parameter type {} does not match '{}' declaration {expected}",
                        argument.parameter_ty, call.target
                    ));
                }
                let requires_place = declaration.ref_params.get(index).copied().unwrap_or(false);
                if argument.requires_place != requires_place {
                    errors.push(format!(
                        "{prefix}: selected subscript parameter {index} place requirement does not match '{}' declaration",
                        call.target
                    ));
                }
                if matches!(
                    argument.source,
                    crate::checked::CheckedCallArgumentSource::Default
                ) && declaration.required.get(index).copied().unwrap_or(true)
                {
                    errors.push(format!(
                        "{prefix}: required subscript parameter {index} of '{}' is bound to a default",
                        call.target
                    ));
                }
            }
            let overflow = call
                .arguments
                .get(declaration.param_types.len()..)
                .unwrap_or_default();
            let positional_overflow = overflow
                .iter()
                .filter(|argument| {
                    matches!(
                        argument.source,
                        crate::checked::CheckedCallArgumentSource::Positional(_)
                    )
                })
                .collect::<Vec<_>>();
            let expected_positional = match declaration.variadic.as_ref() {
                Some(Ty::RuntimePack(elements)) => {
                    if elements.len() != positional_overflow.len() {
                        errors.push(format!(
                            "{prefix}: selected subscript has {} positional overflow argument(s), but '{}' requires RuntimePack arity {}",
                            positional_overflow.len(),
                            call.target,
                            elements.len()
                        ));
                    }
                    Some(elements.as_slice())
                }
                _ => None,
            };
            let mut positional_index = 0;
            for argument in overflow {
                let expected = match argument.source {
                    crate::checked::CheckedCallArgumentSource::Positional(_) => {
                        let expected = match (&declaration.variadic, expected_positional) {
                            (Some(Ty::RuntimePack(_)), Some(elements)) => {
                                elements.get(positional_index)
                            }
                            (Some(element), _) => Some(element),
                            (None, _) => None,
                        };
                        positional_index += 1;
                        expected
                    }
                    crate::checked::CheckedCallArgumentSource::Keyword(_) => {
                        declaration.kw_variadic.as_ref()
                    }
                    crate::checked::CheckedCallArgumentSource::Default => {
                        errors.push(format!(
                            "{prefix}: selected subscript overflow argument cannot use a default"
                        ));
                        None
                    }
                };
                let Some(expected) = expected else {
                    if !matches!(
                        argument.source,
                        crate::checked::CheckedCallArgumentSource::Default
                    ) {
                        errors.push(format!(
                            "{prefix}: selected subscript overflow argument has no matching variadic collector in '{}'",
                            call.target
                        ));
                    }
                    continue;
                };
                if !types_compatible(&argument.parameter_ty, expected) {
                    errors.push(format!(
                        "{prefix}: selected subscript overflow type {} does not match '{}' collector {expected}",
                        argument.parameter_ty, call.target
                    ));
                }
            }
        }
    }
}

/// Check the source-order MIR representation against a declaration-order
/// generic ABI. Keyword names select their declaration directly; positional
/// entries skip infer-only declarations. A type argument occupies a slot but
/// has no register, while every supplied value argument must carry a register
/// compatible with its declared checked type.
pub(super) fn verify_param_arguments(
    prefix: &str,
    function: &MirFunction,
    declarations: &[crate::types::ParamDecl],
    arguments: &[crate::mir::MirParamArg],
    errors: &mut Vec<String>,
) {
    if declarations.is_empty() {
        // A specialized nominal constructor/method may retain erased source
        // type applications even though its selected MIR declaration is
        // monomorphic. They deliberately carry no register. A value argument,
        // however, would imply a runtime compile-time slot absent from the ABI.
        if arguments.iter().any(|argument| argument.value.is_some()) {
            errors.push(format!(
                "{prefix}: nongeneric call carries a compile-time value argument"
            ));
        }
        return;
    }
    let mut occupied = vec![false; declarations.len()];
    let mut next_positional = 0;
    for argument in arguments {
        let index = if let Some(name) = &argument.name {
            declarations
                .iter()
                .position(|declaration| declaration.name().trim_start_matches('*') == name.as_str())
        } else {
            while declarations
                .get(next_positional)
                .is_some_and(|declaration| {
                    occupied[next_positional]
                        || match declaration {
                            crate::types::ParamDecl::Type { infer_only, .. }
                            | crate::types::ParamDecl::Value { infer_only, .. } => *infer_only,
                        }
                })
            {
                next_positional += 1;
            }
            let index = (next_positional < declarations.len()).then_some(next_positional);
            next_positional += usize::from(index.is_some());
            index
        };
        let Some(index) = index else {
            errors.push(format!(
                "{prefix}: compile-time argument does not match a parameter declaration"
            ));
            continue;
        };
        if occupied[index] {
            errors.push(format!(
                "{prefix}: compile-time parameter '{}' is supplied more than once",
                declarations[index].name()
            ));
            continue;
        }
        occupied[index] = true;
        match (&declarations[index], argument.value) {
            (crate::types::ParamDecl::Type { name, .. }, Some(_)) => errors.push(format!(
                "{prefix}: type parameter '{name}' unexpectedly carries a runtime register"
            )),
            (crate::types::ParamDecl::Value { name, .. }, None) => errors.push(format!(
                "{prefix}: value parameter '{name}' has no runtime register"
            )),
            (crate::types::ParamDecl::Value { name, ty, .. }, Some(register)) => {
                if let Some(found) = function.reg_types.get(&register.0)
                    && !(matches!(ty.as_ref(), Ty::Func { .. } | Ty::GenericFunc { .. })
                        && crate::checker::callable_bound_accepts(found, ty))
                    && !types_compatible(found, ty)
                {
                    errors.push(format!(
                        "{prefix}: compile-time value parameter '{name}' has register type {found}, declared {ty}"
                    ));
                }
            }
            (crate::types::ParamDecl::Type { .. }, None) => {}
        }
    }
    for (index, declaration) in declarations.iter().enumerate() {
        if occupied[index] {
            continue;
        }
        if let crate::types::ParamDecl::Value {
            name,
            default,
            callable_default,
            infer_only,
            variadic,
            ..
        } = declaration
            && default.is_none()
            && callable_default.is_none()
            && !infer_only
            && !variadic
        {
            errors.push(format!(
                "{prefix}: required compile-time value parameter '{name}' is missing"
            ));
        }
    }
}

pub(super) fn verify_capture_accesses(
    prefix: &str,
    function: &MirFunction,
    accesses: &[crate::mir::MirCaptureAccess],
    errors: &mut Vec<String>,
) {
    for access in accesses {
        if access.root as usize >= function.var_names.len() {
            errors.push(format!(
                "{prefix}: callable capture access uses unknown owner slot {}",
                access.root
            ));
        }
    }
}
