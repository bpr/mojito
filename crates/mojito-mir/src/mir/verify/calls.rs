//! Block, direct-call, function, and runtime-pack ABI verification.

use super::*;

pub(super) fn verify_blocks(
    name: &str,
    function: &MirFunction,
    declarations: &MirDeclarations,
    blocks: &[MirBlock],
    context: &RegionContext,
    errors: &mut Vec<String>,
) {
    for (block_index, block) in blocks.iter().enumerate() {
        for instruction in &block.instrs {
            verify_instruction(
                name,
                function,
                declarations,
                block_index,
                instruction,
                context,
                errors,
            );
        }
        verify_terminator(name, function, block_index, &block.term, context, errors);
    }
}

/// Arity, argument-type, and write-back checks against a declaration. Only the
/// plain positional shape is compared — defaulted, keyword, and variadic calls
/// are bound by the runtime matcher, whose slotting the verifier does not
/// replicate.
pub(super) fn verify_direct_call(
    prefix: &str,
    function: &MirFunction,
    declaration: &MirFunctionDeclaration,
    args: &[Reg],
    kwargs: &[(String, Reg)],
    arg_places: &[Option<MirPlace>],
    errors: &mut Vec<String>,
) {
    let plain = kwargs.is_empty()
        && declaration.variadic.is_none()
        && declaration.kw_variadic.is_none()
        && args.len() == declaration.param_types.len();
    if !plain {
        return;
    }
    for (index, (argument, expected)) in args.iter().zip(&declaration.param_types).enumerate() {
        if let Some(found) = function.reg_types.get(&argument.0)
            && !types_compatible(found, expected)
        {
            errors.push(format!(
                "{prefix}: argument {index} of '{}' has type {found}, declared {expected}",
                declaration.lowered_name
            ));
        }
        if declaration.ref_params.get(index).copied().unwrap_or(false)
            && arg_places
                .get(index)
                .map(Option::as_ref)
                .unwrap_or(None)
                .is_none()
        {
            errors.push(format!(
                "{prefix}: write-back parameter {index} of '{}' has no caller place",
                declaration.lowered_name
            ));
        }
    }
}

pub(super) fn subscript_arg_regs(argument: &crate::mir::MirSubscriptArg, out: &mut Vec<Reg>) {
    match argument {
        crate::mir::MirSubscriptArg::Index(register) => out.push(*register),
        crate::mir::MirSubscriptArg::Slice {
            lower, upper, step, ..
        } => out.extend([lower, upper, step].into_iter().flatten().copied()),
    }
}

pub(super) fn simd_element_type(dtype: mojito_ast::ast::Dtype) -> Ty {
    match dtype {
        mojito_ast::ast::Dtype::Int => Ty::Int,
        mojito_ast::ast::Dtype::Float64 => Ty::Float64,
        dtype => Ty::Simd { dtype, width: 1 },
    }
}

pub(super) fn slice_descriptor_ty(kind: mojito_types::types::SliceKind) -> Ty {
    Ty::Struct(kind.type_name().to_string(), Vec::new())
}

pub(super) fn subscript_argument_ty(
    function: &MirFunction,
    argument: &crate::mir::MirSubscriptArg,
) -> Option<Ty> {
    match argument {
        crate::mir::MirSubscriptArg::Index(register) => {
            function.reg_types.get(&register.0).cloned()
        }
        crate::mir::MirSubscriptArg::Slice { kind, .. } => Some(slice_descriptor_ty(*kind)),
    }
}

pub(super) fn effective_call_convention_matches(
    declared: Option<mojito_ast::ast::ArgConvention>,
    effective: Option<mojito_ast::ast::ArgConvention>,
) -> bool {
    declared == effective
        || matches!(
            (declared, effective),
            (
                Some(mojito_ast::ast::ArgConvention::Ref),
                Some(mojito_ast::ast::ArgConvention::Imm)
            )
        )
}

pub(super) fn generic_callable_decls(ty: &Ty) -> Option<&[mojito_types::types::ParamDecl]> {
    match ty {
        Ty::GenericFunc { decls, .. } => Some(decls),
        Ty::Param {
            callable_bound: Some(bound),
            ..
        } => generic_callable_decls(bound),
        _ => None,
    }
}

/// Ensure each symbolic dependent index is owned by an explicit enclosing
/// value-parameter binder. This permits generic MIR to remain symbolic while
/// rejecting a misspelled or escaped index name before wildcard compatibility
/// could hide it.
pub(super) fn validate_dependent_bindings(ty: &Ty) -> Result<(), String> {
    pub(super) fn walk(ty: &Ty, bound: &HashSet<String>) -> Result<(), String> {
        match ty {
            Ty::Dependent(DependentType::Indexed { elements, index }) => {
                let mut referenced = HashSet::new();
                index.referenced_parameters(&mut referenced);
                let mut unbound: Vec<_> = referenced.difference(bound).cloned().collect();
                unbound.sort();
                if !unbound.is_empty() {
                    return Err(format!(
                        "dependent index references unbound parameter(s): {}",
                        unbound.join(", ")
                    ));
                }
                for element in elements {
                    walk(element, bound)?;
                }
            }
            Ty::GenericFunc {
                decls,
                params,
                ret,
                variadic,
                kw_variadic,
                error,
                ..
            } => {
                let mut signature_scope = bound.clone();
                for declaration in decls {
                    match declaration {
                        ParamDecl::Type {
                            callable_bound,
                            default,
                            ..
                        } => {
                            if let Some(callable) = callable_bound {
                                walk(callable, &signature_scope)?;
                            }
                            if let Some(default) = default {
                                walk(default, &signature_scope)?;
                            }
                        }
                        ParamDecl::Value { name, ty, .. } => {
                            walk(ty, &signature_scope)?;
                            signature_scope.insert(name.trim_start_matches('*').to_string());
                        }
                    }
                }
                for parameter in params {
                    walk(parameter, &signature_scope)?;
                }
                walk(ret, &signature_scope)?;
                if let Some(parameter) = variadic {
                    walk(parameter, &signature_scope)?;
                }
                if let Some(parameter) = kw_variadic {
                    walk(parameter, &signature_scope)?;
                }
                if let Some(error) = error {
                    walk(error, &signature_scope)?;
                }
            }
            Ty::Func {
                params,
                ret,
                variadic,
                kw_variadic,
                error,
                ..
            } => {
                for parameter in params {
                    walk(parameter, bound)?;
                }
                walk(ret, bound)?;
                if let Some(parameter) = variadic {
                    walk(parameter, bound)?;
                }
                if let Some(parameter) = kw_variadic {
                    walk(parameter, bound)?;
                }
                if let Some(error) = error {
                    walk(error, bound)?;
                }
            }
            Ty::Param {
                callable_bound: Some(callable),
                ..
            } => walk(callable, bound)?,
            Ty::Struct(_, arguments) => {
                for argument in arguments {
                    if let TyArg::Ty(ty) = argument {
                        walk(ty, bound)?;
                    }
                }
            }
            Ty::ComptimeList(element) | Ty::VariadicPack(element) | Ty::Pointer { element, .. } => {
                walk(element, bound)?
            }
            Ty::Tuple(elements)
            | Ty::RuntimePack(elements)
            | Ty::Variant(elements)
            | Ty::Overload(elements) => {
                for element in elements {
                    walk(element, bound)?;
                }
            }
            Ty::Assoc { base, .. } => walk(base, bound)?,
            Ty::Ref(reference) => walk(&reference.referent, bound)?,
            _ => {}
        }
        Ok(())
    }

    walk(ty, &HashSet::new())
}

/// The storage a `MakeRef` handle designates. A capability-typed root already
/// contains a runtime frame/slot handle, so `MakeRef` forwards that handle and
/// extends its projection instead of borrowing the capability slot itself.
/// An ordinary root, including a struct field whose value happens to be a
/// reference, is borrowed as storage and therefore retains `place.ty`.
pub(super) fn make_ref_target(place: &MirPlace) -> Option<(&Ty, Option<ReferencePermission>)> {
    let root_capability = place.root_ty.as_ref().and_then(reference_capability);
    match (root_capability, place.proj.is_empty()) {
        (Some(capability), true) => Some((capability.target, Some(capability.permission))),
        (Some(capability), false) => place
            .ty
            .as_ref()
            .map(|target| (target, Some(capability.permission))),
        (None, _) => place.ty.as_ref().map(|target| (target, None)),
    }
}

pub(super) fn verify_function(
    name: &str,
    function: &MirFunction,
    declarations: &MirDeclarations,
    errors: &mut Vec<String>,
) {
    if function.var_names.len() != function.n_vars {
        errors.push(format!(
            "MIR function '{name}' has {} variable names for {} slots",
            function.var_names.len(),
            function.n_vars
        ));
    }
    for (index, ty) in function.param_types.iter().enumerate() {
        if contains_runtime_pack(ty) {
            errors.push(format!(
                "MIR function '{name}' parameter slot {index} retains ABI-only RuntimePack type {ty}"
            ));
        }
    }
    for (slot, ty) in &function.var_tys {
        if contains_runtime_pack(ty) {
            errors.push(format!(
                "MIR function '{name}' variable slot {slot} retains ABI-only RuntimePack type {ty}"
            ));
        }
    }
    for (register, ty) in &function.reg_types {
        if contains_runtime_pack(ty) {
            errors.push(format!(
                "MIR function '{name}' register r{register} retains ABI-only RuntimePack type {ty}"
            ));
        }
    }
    for (description, ty) in [
        ("return", function.ret_ty.as_ref()),
        ("error", function.error_ty.as_ref()),
    ] {
        if ty.is_some_and(contains_runtime_pack) {
            errors.push(format!(
                "MIR function '{name}' {description} contract retains ABI-only RuntimePack type"
            ));
        }
    }
    let context = RegionContext {
        region_len: function.blocks.len(),
        function_len: function.blocks.len(),
        in_try_region: false,
        protected: false,
    };
    verify_blocks(
        name,
        function,
        declarations,
        &function.blocks,
        &context,
        errors,
    );
}

/// `RuntimePack[T0, ...]` distinguishes a specialized heterogeneous `*args`
/// declaration from an ordinary homogeneous `*args: Tuple[...]`. It may occur
/// only as the top-level positional-variadic ABI type. Once arguments have
/// been matched, the body sees an ordinary native `Tuple[T0, ...]` slot.
pub(super) fn verify_runtime_pack_abi(declarations: &MirDeclarations, errors: &mut Vec<String>) {
    for declaration in &declarations.structs {
        for (field, ty) in &declaration.fields {
            if contains_runtime_pack(ty) {
                errors.push(format!(
                    "MIR struct '{}.{}' contains ABI-only RuntimePack type {ty}",
                    declaration.name, field
                ));
            }
        }
    }
    for declaration in &declarations.functions {
        let name = &declaration.lowered_name;
        let parameter_count = declaration.param_types.len();
        for (description, found) in [
            ("names", declaration.param_names.len()),
            ("defaults", declaration.defaults.len()),
            ("required mask", declaration.required.len()),
            ("conventions", declaration.param_conventions.len()),
            ("reference mask", declaration.ref_params.len()),
        ] {
            if found != parameter_count {
                errors.push(format!(
                    "MIR declaration '{name}' has {found} parameter {description}, expected {parameter_count}"
                ));
            }
        }
        if !declaration.has_receiver && declaration.receiver_convention.is_some() {
            errors.push(format!(
                "MIR declaration '{name}' has a receiver convention but no receiver"
            ));
        }
        if declaration.variadic.is_none() && declaration.variadic_convention.is_some() {
            errors.push(format!(
                "MIR declaration '{name}' has a positional variadic convention but no positional variadic parameter"
            ));
        }
        if declaration.kw_variadic.is_none() && declaration.kw_variadic_convention.is_some() {
            errors.push(format!(
                "MIR declaration '{name}' has a keyword variadic convention but no keyword variadic parameter"
            ));
        }
        for (index, convention) in declaration.param_conventions.iter().enumerate() {
            let expected_reference = matches!(
                convention,
                Some(mojito_ast::ast::ArgConvention::Mut | mojito_ast::ast::ArgConvention::Ref)
            );
            if declaration.ref_params.get(index).copied() != Some(expected_reference) {
                errors.push(format!(
                    "MIR declaration '{name}' parameter {index} convention/reference mask disagree"
                ));
            }
        }
        for (index, ty) in declaration.param_types.iter().enumerate() {
            if contains_runtime_pack(ty) {
                errors.push(format!(
                    "MIR declaration '{name}' regular parameter {index} contains ABI-only RuntimePack type {ty}"
                ));
            }
        }
        if let Some(variadic) = &declaration.variadic {
            match variadic {
                Ty::RuntimePack(elements) => {
                    if elements.iter().any(contains_runtime_pack) {
                        errors.push(format!(
                            "MIR declaration '{name}' has a nested RuntimePack variadic ABI"
                        ));
                    }
                }
                other if contains_runtime_pack(other) => errors.push(format!(
                    "MIR declaration '{name}' embeds RuntimePack below its variadic ABI root"
                )),
                _ => {}
            }
        }
        for (description, ty) in [
            ("keyword variadic", declaration.kw_variadic.as_ref()),
            ("return", Some(&declaration.ret_ty)),
            ("error", declaration.error_ty.as_ref()),
        ] {
            if ty.is_some_and(contains_runtime_pack) {
                errors.push(format!(
                    "MIR declaration '{name}' {description} type contains ABI-only RuntimePack"
                ));
            }
        }
    }
}
