//! Subscript receiver/call verification and parameter-argument
//! checks.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::TyKind;

pub(super) fn verify_subscript_instruction(
    cx: &InstrCx<'_>,
    instruction: &MirInstr,
    errors: &mut Vec<String>,
) {
    let prefix = cx.prefix;
    match instruction {
        MirInstr::Index {
            dest,
            base,
            index,
            call,
            intrinsic,
            ..
        } => {
            if call.is_some() == intrinsic.is_some() {
                errors.push(format!(
                    "{prefix}: index operation must carry exactly one checked-call or intrinsic dispatch"
                ));
            }
            if let Some(call) = call {
                // `__getitem_param__` reuses the syntactic index register as a
                // compile-time value argument; it has no ordinary runtime
                // positional argument. The empty checked argument list is the
                // explicit ABI discriminator used by lowering and execution.
                let positional_types = if call.arguments.is_empty() {
                    Vec::new()
                } else {
                    vec![cx.reg_ty(*index).cloned()]
                };
                verify_subscript_call(
                    prefix,
                    cx.function,
                    cx.declarations,
                    call,
                    SubscriptSources {
                        receiver_ty: cx.reg_ty(*base),
                        method: "__getitem__",
                        positional_types: &positional_types,
                        keyword_types: &[],
                        dest: Some(*dest),
                    },
                    errors,
                );
                verify_accessor_operands(cx, instruction, call, *base, errors);
            } else if let Some(intrinsic) = intrinsic {
                verify_intrinsic_index(
                    prefix,
                    *intrinsic,
                    cx.reg_ty(*base),
                    cx.reg_ty(*index),
                    cx.reg_ty(*dest),
                    errors,
                );
            }
        }
        MirInstr::MultiIndex {
            dest,
            object,
            args,
            kwargs,
            call,
        } => {
            let positional_types = args
                .iter()
                .map(|argument| subscript_argument_ty(cx.function, argument))
                .collect::<Vec<_>>();
            let keyword_types = kwargs
                .iter()
                .map(|(_, argument)| subscript_argument_ty(cx.function, argument))
                .collect::<Vec<_>>();
            if let Some(call) = call {
                verify_subscript_call(
                    prefix,
                    cx.function,
                    cx.declarations,
                    call,
                    SubscriptSources {
                        receiver_ty: cx.reg_ty(*object),
                        method: "__getitem__",
                        positional_types: &positional_types,
                        keyword_types: &keyword_types,
                        dest: Some(*dest),
                    },
                    errors,
                );
                verify_accessor_operands(cx, instruction, call, *object, errors);
            } else {
                errors.push(format!(
                    "{prefix}: multi-index operation lacks a checked call contract"
                ));
            }
        }
        MirInstr::MultiSet {
            receiver,
            args,
            value,
            call,
            ..
        } => {
            let positional_types = args
                .iter()
                .map(|argument| subscript_argument_ty(cx.function, argument))
                .collect::<Vec<_>>();
            let keyword_types = vec![cx.reg_ty(*value).cloned()];
            verify_subscript_call(
                prefix,
                cx.function,
                cx.declarations,
                call,
                SubscriptSources {
                    receiver_ty: cx.reg_ty(*receiver),
                    method: "__setitem__",
                    positional_types: &positional_types,
                    keyword_types: &keyword_types,
                    dest: None,
                },
                errors,
            );
            verify_accessor_operands(cx, instruction, call, *receiver, errors);
        }
        _ => {}
    }
}

/// Runtime frame/slot capabilities are represented by a source-level
/// `ref T`, an origin-bearing `Pointer[T, origin]`, or the `!lit.ref`
/// a `Pointer` is built from. Raw/static/
/// untracked pointer values use allocation arithmetic and are not valid
/// operands for `ReadRef`/`WriteRef`.
pub(super) fn reference_capability(ty: &Ty) -> Option<ReferenceCapability<'_>> {
    let (element, origin) = match ty.kind() {
        TyKind::Ref(reference) => {
            return Some(ReferenceCapability {
                target: &reference.referent,
                permission: ReferencePermission::from_mutability(reference.mutability),
            });
        }
        TyKind::LitRef {
            element, origin, ..
        } => (element, origin.clone()),
        pointer => (pointer.pointer_element()?, pointer.pointer_origin()?),
    };
    let permission = match origin {
        PointerOrigin::Place { mutable, .. } => {
            if mutable {
                ReferencePermission::Mutable
            } else {
                ReferencePermission::Immutable
            }
        }
        PointerOrigin::Param { mutability, .. }
        | PointerOrigin::SelfPlace { mutability, .. }
        | PointerOrigin::Union { mutability, .. } => {
            ReferencePermission::from_mutability(mutability)
        }
        // An untracked pointer addresses its pointee with the
        // permission it is declared with, a `VariadicList`'s element
        // pointer among them.
        PointerOrigin::Untracked { mutable } => {
            if mutable {
                ReferencePermission::Mutable
            } else {
                ReferencePermission::Immutable
            }
        }
        PointerOrigin::Static { .. } | PointerOrigin::UnsafeAny { .. } => return None,
    };
    Some(ReferenceCapability {
        target: element,
        permission,
    })
}

#[allow(
    clippy::cognitive_complexity,
    clippy::too_many_lines,
    clippy::needless_pass_by_value,
    reason = "TODO: split this pass; `sources` is a borrow bundle of `&mut` fields"
)]
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
        positional_types,
        keyword_types,
        dest,
    } = sources;
    verify_capture_accesses(prefix, function, &call.capture_accesses, errors);
    let abstract_trait_dispatch = call.target.starts_with("__trait_dispatch.");
    let target_family =
        mojito_symbol::symbol::split_method_symbol(&call.target).is_some_and(|(_, symbol)| {
            symbol == method
                || symbol
                    .strip_prefix(method)
                    .is_some_and(|suffix| suffix.starts_with('$'))
                || method == "__getitem__"
                    && (symbol == "__getitem_param__" || symbol.starts_with("__getitem_param__$"))
        });
    if !target_family {
        errors.push(format!(
            "{prefix}: selected subscript target '{}' is not in the {method} method family",
            call.target
        ));
    }
    let concrete_receiver = match receiver_ty.map(Ty::kind) {
        Some(struct_ty) if let Some((name, _)) = struct_ty.as_non_simd_struct() => {
            Some(name.as_str())
        }
        Some(TyKind::Ref(reference)) => match reference.referent.kind() {
            struct_ty if let Some((name, _)) = struct_ty.as_non_simd_struct() => {
                Some(name.as_str())
            }
            _ => None,
        },
        _ => None,
    };
    // A generator that removed one of its struct's parameters is named
    // after the struct's template rather than the receiver's instance.
    if !abstract_trait_dispatch
        && let Some(receiver) = concrete_receiver
        && mojito_symbol::symbol::split_method_symbol(&call.target).is_some_and(|(owner, _)| {
            owner != receiver
                && !receiver
                    .strip_prefix(owner)
                    .is_some_and(|instance| instance.starts_with("$mono$"))
        })
    {
        errors.push(format!(
            "{prefix}: selected subscript target '{}' does not belong to receiver type {receiver}",
            call.target
        ));
    }
    // A receiver or argument the accessor takes by address is a reference
    // to the caller's storage, which its operand is.
    let is_reference = |ty: Option<&Ty>| ty.is_some_and(|ty| matches!(ty.kind(), TyKind::Ref(_)));
    if call.receiver_requires_place && !is_reference(receiver_ty) {
        errors.push(format!(
            "{prefix}: selected subscript reference receiver passes no reference"
        ));
    }
    let mut positional_uses = vec![0usize; positional_types.len()];
    let mut keyword_uses = vec![0usize; keyword_types.len()];
    for argument in &call.arguments {
        let actual_ty = match argument.source {
            mojito_checked::checked::CheckedCallArgumentSource::Positional(index) => {
                if let Some(uses) = positional_uses.get_mut(index) {
                    *uses += 1;
                } else {
                    errors.push(format!(
                        "{prefix}: selected subscript positional source {index} is out of range"
                    ));
                }
                positional_types.get(index).and_then(Option::as_ref)
            }
            mojito_checked::checked::CheckedCallArgumentSource::Keyword(index) => {
                if let Some(uses) = keyword_uses.get_mut(index) {
                    *uses += 1;
                } else {
                    errors.push(format!(
                        "{prefix}: selected subscript keyword source {index} is out of range"
                    ));
                }
                keyword_types.get(index).and_then(Option::as_ref)
            }
            mojito_checked::checked::CheckedCallArgumentSource::Default => None,
        };
        if argument.requires_place && !is_reference(actual_ty) {
            errors.push(format!(
                "{prefix}: selected subscript mut/ref argument {:?} passes no reference",
                argument.source
            ));
        }
        // A parameter received by address takes a reference to the caller's
        // storage (`Flatten::address_operand`); an owned or
        // `deinit` one takes the value itself.
        let borrowed = !matches!(
            argument.convention,
            Some(
                mojito_ast::ast::ArgConvention::Var
                    | mojito_ast::ast::ArgConvention::Deinit
                    | mojito_ast::ast::ArgConvention::Out
            )
        ) && !matches!(argument.parameter_ty.kind(), TyKind::Ref(_));
        let actual_ty = actual_ty.map(|actual| {
            if borrowed {
                crate::mir::operand_value_ty(actual)
            } else {
                actual
            }
        });
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
            Some(mojito_ast::ast::ArgConvention::Mut | mojito_ast::ast::ArgConvention::Ref)
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
        .map(|reference| TyKind::Ref(reference.clone()));
    match &retained_reference_ty {
        Some(expected) if &call.result_ty != expected => errors.push(format!(
            "{prefix}: subscript reference-result metadata {expected} does not match selected result type {}",
            call.result_ty
        )),
        None if matches!(call.result_ty.kind(), TyKind::Ref(_)) => errors.push(format!(
            "{prefix}: subscript selected result type {} lacks reference-result metadata",
            call.result_ty
        )),
        _ => {}
    }
    if let Some(dest) = dest
        && let Some(found) = function.reg_types.get(&dest.0)
    {
        let exact_reference_mismatch = matches!(
            (found.kind(), call.result_ty.kind()),
            (TyKind::Ref(found), TyKind::Ref(expected)) if found != expected
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
        &[],
        None,
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
                Some(mojito_ast::ast::ArgConvention::Mut | mojito_ast::ast::ArgConvention::Ref)
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
            } else if let (Some(found), Some(expected)) = (
                call.raises.as_ref().map(|raises| &raises.ty),
                declaration.error_ty.as_ref(),
            ) && !types_compatible(found, expected)
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
                && !types_compatible(&reference.referent, &declaration.ret_ty)
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
                // An elaborated instance names its own `VariadicList`.
                let list_elements = (
                    mojito_types::types::variadic_list_element(&argument.parameter_ty),
                    mojito_types::types::variadic_list_element(expected),
                );
                if !types_compatible(&argument.parameter_ty, expected)
                    && !matches!(list_elements, (Some(found), Some(declared))
                        if types_compatible(found, declared))
                {
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
                    mojito_checked::checked::CheckedCallArgumentSource::Default
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
                        mojito_checked::checked::CheckedCallArgumentSource::Positional(_)
                    )
                })
                .collect::<Vec<_>>();
            // The overflow's positional part is the one collector the call
            // builds: the declared collector's slot type, a bound pack's
            // elements' tuple.
            if positional_overflow.len() > 1 {
                errors.push(format!(
                    "{prefix}: selected subscript passes {} positional overflow arguments to '{}', not one collector",
                    positional_overflow.len(),
                    call.target
                ));
            }
            let collector = declaration.variadic.clone().map(|variadic| {
                mojito_types::types::variadic_collector_type(
                    variadic,
                    declaration.variadic_convention,
                )
            });
            for argument in overflow {
                let expected = match argument.source {
                    mojito_checked::checked::CheckedCallArgumentSource::Positional(_) => {
                        collector.as_ref()
                    }
                    mojito_checked::checked::CheckedCallArgumentSource::Keyword(_) => {
                        declaration.kw_variadic.as_ref()
                    }
                    mojito_checked::checked::CheckedCallArgumentSource::Default => {
                        errors.push(format!(
                            "{prefix}: selected subscript overflow argument cannot use a default"
                        ));
                        None
                    }
                };
                let Some(expected) = expected else {
                    if !matches!(
                        argument.source,
                        mojito_checked::checked::CheckedCallArgumentSource::Default
                    ) {
                        errors.push(format!(
                            "{prefix}: selected subscript overflow argument has no matching variadic collector in '{}'",
                            call.target
                        ));
                    }
                    continue;
                };
                // An elaborated instance names its own `VariadicList`.
                let list_elements = (
                    mojito_types::types::variadic_list_element(&argument.parameter_ty),
                    mojito_types::types::variadic_list_element(expected),
                );
                if !types_compatible(&argument.parameter_ty, expected)
                    && !matches!(list_elements, (Some(found), Some(declared))
                        if types_compatible(found, declared))
                {
                    errors.push(format!(
                        "{prefix}: selected subscript overflow type {} does not match '{}' collector {expected}",
                        argument.parameter_ty, call.target
                    ));
                }
            }
        }
    }
}

/// Check a call's compile-time arguments against its callee's whole
/// declared list, each argument binding the declaration it names
/// ([`crate::mir::MirParamArg::declaration_index`]), so a non-pack
/// declaration named twice is rejected. An origin argument names its
/// origin binder and is erased; `instantiated`, when the call carries it,
/// holds a value for every declared parameter, origins included, as
/// upstream's call does. A
/// type argument occupies a slot but has no register, while every supplied
/// value argument must carry a register compatible with its declared checked
/// type.
/// A value slot's register is absent only where the call's `instantiated`
/// arguments carry the closed value it was folded to, and a value parameter
/// the call leaves to inference has no slot at all: the call carries each
/// value as compile-time data, as Mojo's carries a parameter attribute.
/// Each supplied value is checked against its declared type closed under
/// the call's `instantiated` arguments and the receiver struct's `owner`
/// arguments (`h: def(SIMD[d, 2])` at `d = DType.int32`), as the pin
/// substitutes the whole parameter list before converting an argument.
pub(super) fn verify_param_arguments(
    prefix: &str,
    function: &MirFunction,
    declarations: &[mojito_types::types::ParamDecl],
    arguments: &[crate::mir::MirParamArg],
    instantiated: &[mojito_types::types::TyArg],
    owner: Option<(
        &[mojito_types::types::ParamDecl],
        &[mojito_types::types::TyArg],
    )>,
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
    // A type or value pack, whose arguments each bind one element.
    let pack = |index: usize| {
        matches!(
            declarations[index],
            mojito_types::types::ParamDecl::Type { variadic: true, .. }
                | mojito_types::types::ParamDecl::Value { variadic: true, .. }
        )
    };
    if !instantiated.is_empty() && instantiated.len() != declarations.len() {
        errors.push(format!(
            "{prefix}: the call's {} solved argument(s) do not bind its callee's {} declared parameter(s)",
            instantiated.len(),
            declarations.len()
        ));
    }
    let context = mojito_types::param_expr::ParamContext::detached();
    let mut bindings = mojito_types::param_expr::ParamBindings::new();
    if let Some((owner_decls, owner_arguments)) = owner {
        bindings.bind_value_arguments(&context, owner_decls, owner_arguments);
    }
    bindings.bind_value_arguments(&context, declarations, instantiated);
    let instantiated_at = |index: usize| instantiated.get(index);
    let mut occupied = vec![false; declarations.len()];
    for argument in arguments {
        let Some(index) = argument.declaration_index(declarations) else {
            // An erased argument naming no declaration is an origin
            // argument the check bound by no declaration.
            if argument.value.is_some() {
                errors.push(format!(
                    "{prefix}: compile-time argument does not match a parameter declaration"
                ));
            }
            continue;
        };
        if occupied[index] && !pack(index) {
            errors.push(format!(
                "{prefix}: compile-time parameter '{}' is supplied more than once",
                declarations[index].name()
            ));
            continue;
        }
        occupied[index] = true;
        match (&declarations[index], argument.value) {
            // A type argument is compile-time data on the call: it has no
            // run-time form.
            (mojito_types::types::ParamDecl::Type { name, .. }, Some(_)) => {
                errors.push(format!(
                    "{prefix}: type parameter '{name}' carries a runtime register"
                ));
            }
            (mojito_types::types::ParamDecl::Value { name, .. }, None) => {
                if !matches!(
                    instantiated_at(index),
                    Some(mojito_types::types::TyArg::Val(value)) if value.is_parameter_attribute()
                ) {
                    errors.push(format!(
                        "{prefix}: value parameter '{name}' has no runtime register"
                    ));
                }
            }
            (
                declaration @ mojito_types::types::ParamDecl::Value { name, ty, .. },
                Some(register),
            ) => {
                // Each argument to a pack binds one value of its element; a
                // value pack spread whole binds the pack of its element.
                let ty = declaration.value_element_ty().unwrap_or(ty);
                let ty = &mojito_types::types::replace_parameters(&context, ty, &bindings, 0)
                    .unwrap_or_else(|_| ty.clone());
                let spread = pack(index)
                    && matches!(function.reg_types.get(&register.0).map(Ty::kind),
                        Some(TyKind::VariadicPack(element)) if types_compatible(element, ty));
                if let Some(found) = function.reg_types.get(&register.0)
                    && !spread
                    && !(declaration.binds_callables()
                        && mojito_types::types::callable_bound_accepts(found, ty))
                    && !types_compatible(found, ty)
                {
                    errors.push(format!(
                        "{prefix}: compile-time value parameter '{name}' has register type {found}, declared {ty}"
                    ));
                }
            }
            (
                mojito_types::types::ParamDecl::Type { .. }
                | mojito_types::types::ParamDecl::Origin { .. },
                None,
            ) => {}
            (mojito_types::types::ParamDecl::Origin { name, .. }, Some(_)) => {
                errors.push(format!(
                    "{prefix}: origin parameter '{name}' carries a runtime register"
                ));
            }
        }
    }
    for (index, declaration) in declarations.iter().enumerate() {
        if occupied[index] {
            continue;
        }
        if let mojito_types::types::ParamDecl::Value {
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
            && declaration.is_argument_binder(declarations)
            && !matches!(
                instantiated_at(index),
                Some(mojito_types::types::TyArg::Val(value))
                    if !matches!(value, mojito_types::ct::CtValue::Deferred(_))
            )
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

/// A checked subscript accessor's receiver and arguments, where its
/// declaration, or the requirement a subscript through a trait bound
/// binds, takes them by address, are references
/// (`verify_by_address_receiver`, `verify_by_address_operands`,
/// `verify_requirement_call`), as
/// upstream's accessor `lit.call` takes a `!lit.ref` wherever the
/// accessor's signature does.
fn verify_accessor_operands(
    cx: &InstrCx<'_>,
    instruction: &MirInstr,
    call: &crate::mir::MirSubscriptCall,
    receiver: Reg,
    errors: &mut Vec<String>,
) {
    if let Some(declaration) = declared(cx.declarations, &call.target) {
        verify_by_address_receiver(cx, declaration, receiver, errors);
        if let Some(operands) = crate::mir::subscript_operand_parameters(instruction, declaration) {
            verify_by_address_operands(cx, declaration, &operands, errors);
        }
    } else if let Some(requirement) = cx.declarations.call_requirement(
        &call.target,
        cx.reg_ty(receiver),
        call.refined_bound.as_deref(),
    ) {
        let operands =
            crate::mir::subscript_requirement_operand_parameters(instruction, requirement);
        verify_requirement_call(
            cx,
            requirement,
            Some((receiver, call.receiver_convention)),
            operands.as_deref(),
            errors,
        );
    }
}
