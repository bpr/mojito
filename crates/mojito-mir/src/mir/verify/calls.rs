//! Block, direct-call, function, and runtime-pack ABI verification.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
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

pub(super) fn verify_call_instruction(
    cx: &InstrCx<'_>,
    instruction: &MirInstr,
    errors: &mut Vec<String>,
) {
    let prefix = cx.prefix;
    match instruction {
        MirInstr::Call {
            func: FuncRef(callee),
            args,
            kwargs,
            arg_places,
            kwarg_places,
            capture_accesses,
            param_arg_regs,
            spread,
            ..
        } => {
            verify_capture_accesses(prefix, cx.function, capture_accesses, errors);
            if let Some(position) = spread {
                verify_pack_spread(
                    prefix,
                    cx.function,
                    *position,
                    args,
                    declared(cx.declarations, callee),
                    errors,
                );
            }
            if let Some(declaration) = declared(cx.declarations, callee) {
                verify_direct_call(
                    prefix,
                    cx.function,
                    declaration,
                    args,
                    kwargs,
                    arg_places,
                    errors,
                );
                verify_param_arguments(
                    prefix,
                    cx.function,
                    &declaration.param_decls,
                    param_arg_regs,
                    errors,
                );
            }
            if arg_places.len() != args.len() || kwarg_places.len() != kwargs.len() {
                errors.push(format!(
                    "{prefix}: call place metadata is not aligned with its arguments"
                ));
            }
        }
        MirInstr::MethodCall {
            dest,
            method,
            resolved: Some(callee),
            reference_result,
            result_adapter,
            args,
            kwargs,
            arg_places,
            kwarg_places,
            capture_accesses,
            param_arg_regs,
            param_decls,
            spread,
            ..
        } => {
            verify_capture_accesses(prefix, cx.function, capture_accesses, errors);
            if let Some(position) = spread {
                verify_pack_spread(
                    prefix,
                    cx.function,
                    *position,
                    args,
                    declared(cx.declarations, callee),
                    errors,
                );
            }
            let abstract_value_next = callee.starts_with("__trait_dispatch.")
                && method == "__next__"
                && reference_result.is_none();
            if let Some(reference) = reference_result {
                let expected = Ty::Ref(reference.clone());
                if cx.function.reg_types.get(&dest.0) != Some(&expected) {
                    errors.push(format!(
                        "{prefix}: method-call reference ABI {expected} does not match its destination type"
                    ));
                }
            }
            // The adapter copies the element out of the reference a concrete
            // `__next__` returns, as it does for the abstract dispatch.
            let adapted_reference_next = method == "__next__"
                && reference_result.is_none()
                && declared(cx.declarations, callee)
                    .is_some_and(|declaration| declaration.returns_reference);
            match result_adapter {
                Some(mojito_checked::checked::CheckedResultAdapter::CopyIteratorReference) => {
                    if !abstract_value_next && !adapted_reference_next {
                        errors.push(format!(
                            "{prefix}: copy-reference result adapter is not attached to a value-yielding __next__ call"
                        ));
                    }
                }
                None if abstract_value_next => errors.push(format!(
                    "{prefix}: abstract value-returning __next__ call lacks its copy-reference adapter"
                )),
                None => {}
            }
            if let Some(declaration) = declared(cx.declarations, callee) {
                verify_direct_call(
                    prefix,
                    cx.function,
                    declaration,
                    args,
                    kwargs,
                    arg_places,
                    errors,
                );
                let result_abi_matches =
                    match reference_result {
                        Some(reference) => {
                            declaration.returns_reference
                                && types_compatible(&reference.referent, &declaration.ret_ty)
                        }
                        None => {
                            declaration.returns_reference == result_adapter.is_some()
                                && cx.function.reg_types.get(&dest.0).is_some_and(|result| {
                                    types_compatible(result, &declaration.ret_ty)
                                })
                        }
                    };
                if !result_abi_matches {
                    errors.push(format!(
                        "{prefix}: method-call result ABI does not match '{}'",
                        declaration.lowered_name
                    ));
                }
                if &declaration.param_decls != param_decls {
                    errors.push(format!(
                        "{prefix}: method-call compile-time parameter metadata does not match '{}'",
                        declaration.lowered_name
                    ));
                }
                verify_param_arguments(
                    prefix,
                    cx.function,
                    &declaration.param_decls,
                    param_arg_regs,
                    errors,
                );
            }
            if arg_places.len() != args.len() || kwarg_places.len() != kwargs.len() {
                errors.push(format!(
                    "{prefix}: method-call place metadata is not aligned with its arguments"
                ));
            }
        }
        // A spread names the collector of the method the checker selected;
        // a dispatch-resolved call has none to check it against.
        MirInstr::MethodCall {
            resolved: None,
            spread: Some(position),
            ..
        } => errors.push(format!(
            "{prefix}: unresolved method call spreads argument {position}"
        )),
        MirInstr::CallIndirect { .. } => verify_indirect_call(cx, instruction, errors),
        _ => {}
    }
}

/// A whole pack spread into the callee's collector (`show(*args)`,
/// `Sink().take(*args)`): the position names an argument whose register
/// holds a `VariadicPack` over a pack still a parameter, which only a
/// template holds, and a declared callee collects a positional pack.
/// `print` is undeclared and collects.
fn verify_pack_spread(
    prefix: &str,
    function: &MirFunction,
    position: usize,
    args: &[Reg],
    declaration: Option<&MirFunctionDeclaration>,
    errors: &mut Vec<String>,
) {
    let Some(argument) = args.get(position) else {
        errors.push(format!(
            "{prefix}: call spreads argument {position} but passes {} arguments",
            args.len()
        ));
        return;
    };
    match function.reg_types.get(&argument.0) {
        Some(Ty::VariadicPack(element))
            if mojito_types::types::pack_spread(std::slice::from_ref(&**element)).is_some() => {}
        Some(found) => errors.push(format!(
            "{prefix}: spread argument {position} has type {found}, not a pack collector"
        )),
        None => errors.push(format!("{prefix}: spread argument {position} has no type")),
    }
    if let Some(declaration) = declaration
        && declaration.variadic.is_none()
    {
        errors.push(format!(
            "{prefix}: a pack is spread into '{}', which collects no positional pack",
            declaration.lowered_name
        ));
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
            && arg_places.get(index).and_then(Option::as_ref).is_none()
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

pub(super) fn simd_element_type(dtype: &mojito_types::types::SimdDtype) -> Ty {
    mojito_types::types::simd_lane(dtype)
}

pub(super) fn slice_descriptor_ty(kind: mojito_types::types::SliceKind) -> Ty {
    Ty::Struct(kind.type_name().to_string(), Vec::new().into())
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
/// value-parameter binder, and that every parameter expression a type carries
/// may cross into MIR at all. This permits generic MIR to remain symbolic
/// while rejecting a misspelled or escaped index name, an out-of-range or
/// mistyped signature slot, and an unknown or unbound parameter before
/// wildcard compatibility could hide it.
pub(super) fn validate_dependent_bindings(ty: &Ty) -> Result<(), String> {
    /// `frames` holds the value-binder types of each enclosing generic
    /// signature, innermost last; a type parameter's slot holds `None`.
    fn check_expr(
        expr: &mojito_types::param_expr::ParamExpr,
        frames: &[Vec<Option<Ty>>],
    ) -> Result<(), String> {
        use mojito_types::param_expr::{MetaTy, ParamKind};
        // A tabulation's element names its index as the slot of a binder of
        // its own.
        if let ParamKind::ListTabulate { count, element } = expr.kind() {
            check_expr(count, frames)?;
            let mut inner = frames.to_vec();
            inner.push(vec![Some(Ty::Int)]);
            return check_expr(element, &inner);
        }
        let finding = match expr.kind() {
            ParamKind::Hole { .. } => {
                Some("an unknown or unbound parameter cannot cross into MIR".to_string())
            }
            ParamKind::IndexRef { depth, index } => {
                let slot = frames
                    .len()
                    .checked_sub(1 + *depth as usize)
                    .and_then(|frame| frames[frame].get(*index as usize));
                match slot {
                    None => Some(format!(
                        "signature slot {depth}.{index} names no enclosing binder"
                    )),
                    Some(None) => Some(format!(
                        "signature slot {depth}.{index} names a type parameter"
                    )),
                    Some(Some(declared)) if MetaTy::value(declared.clone()) != *expr.meta() => {
                        Some(format!(
                            "signature slot {depth}.{index} is declared `{declared}`, not `{}`",
                            expr.meta()
                        ))
                    }
                    Some(Some(_)) => None,
                }
            }
            _ => None,
        };
        match finding {
            Some(finding) => Err(finding),
            None => expr
                .operands()
                .into_iter()
                .try_for_each(|operand| check_expr(operand, frames)),
        }
    }

    fn walk(
        ty: &Ty,
        bound: &HashSet<String>,
        frames: &mut Vec<Vec<Option<Ty>>>,
    ) -> Result<(), String> {
        match ty {
            Ty::Dependent(dependent) => {
                let mut referenced = HashSet::new();
                dependent.expr().referenced_parameters(&mut referenced);
                let mut unbound: Vec<_> = referenced.difference(bound).cloned().collect();
                unbound.sort();
                if !unbound.is_empty() {
                    return Err(format!(
                        "dependent index references unbound parameter(s): {}",
                        unbound.join(", ")
                    ));
                }
                check_expr(dependent.expr(), frames)?;
                for element in dependent
                    .selection()
                    .map_or(&[][..], |(elements, _)| elements)
                {
                    walk(element, bound, frames)?;
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
                frames.push(
                    decls
                        .iter()
                        .map(|declaration| match declaration {
                            ParamDecl::Value { ty, .. } => Some((**ty).clone()),
                            ParamDecl::Type { .. } => None,
                        })
                        .collect(),
                );
                let checked = (|| -> Result<(), String> {
                    for declaration in decls {
                        match declaration {
                            ParamDecl::Type {
                                callable_bound,
                                default,
                                ..
                            } => {
                                if let Some(callable) = callable_bound {
                                    walk(callable, &signature_scope, frames)?;
                                }
                                if let Some(default) = default {
                                    walk(default, &signature_scope, frames)?;
                                }
                            }
                            ParamDecl::Value { name, ty, .. } => {
                                walk(ty, &signature_scope, frames)?;
                                signature_scope.insert(name.trim_start_matches('*').to_string());
                            }
                        }
                    }
                    for parameter in params {
                        walk(parameter, &signature_scope, frames)?;
                    }
                    walk(ret, &signature_scope, frames)?;
                    if let Some(parameter) = variadic {
                        walk(parameter, &signature_scope, frames)?;
                    }
                    if let Some(parameter) = kw_variadic {
                        walk(parameter, &signature_scope, frames)?;
                    }
                    if let Some(error) = error {
                        walk(error, &signature_scope, frames)?;
                    }
                    Ok(())
                })();
                frames.pop();
                checked?;
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
                    walk(parameter, bound, frames)?;
                }
                walk(ret, bound, frames)?;
                if let Some(parameter) = variadic {
                    walk(parameter, bound, frames)?;
                }
                if let Some(parameter) = kw_variadic {
                    walk(parameter, bound, frames)?;
                }
                if let Some(error) = error {
                    walk(error, bound, frames)?;
                }
            }
            Ty::Param {
                callable_bound: Some(callable),
                ..
            } => walk(callable, bound, frames)?,
            Ty::Struct(_, arguments) => {
                for argument in arguments {
                    match argument {
                        TyArg::Ty(ty) => walk(ty, bound, frames)?,
                        TyArg::Val(mojito_types::ct::CtValue::Expr(expr)) => {
                            check_expr(expr, frames)?;
                        }
                        TyArg::Val(_) | TyArg::Origin(_) => {}
                    }
                }
            }
            Ty::ComptimeList(element) | Ty::VariadicPack(element) | Ty::Pointer { element, .. } => {
                walk(element, bound, frames)?;
            }
            Ty::Tuple(elements)
            | Ty::RuntimePack(elements)
            | Ty::Variant(elements)
            | Ty::Overload(elements) => {
                for element in elements {
                    walk(element, bound, frames)?;
                }
            }
            Ty::Assoc { base, .. } => walk(base, bound, frames)?,
            Ty::Ref(reference) => walk(&reference.referent, bound, frames)?,
            // A symbolic lane or width is a parameter expression like a
            // struct's value argument; `verify/scope.rs` checks its binders
            // and kind, and the concrete mode rejects it.
            Ty::Simd { dtype, width } => {
                if let mojito_types::types::SimdDtype::Expr(expr) = dtype {
                    check_expr(expr, frames)?;
                }
                if let mojito_types::types::SimdWidth::Expr(expr) = width {
                    check_expr(expr, frames)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    walk(ty, &HashSet::new(), &mut Vec::new())
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
    verify_scope(name, function, declarations, errors);
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

fn verify_indirect_call(cx: &InstrCx<'_>, instruction: &MirInstr, errors: &mut Vec<String>) {
    let prefix = cx.prefix;
    let MirInstr::CallIndirect {
        dest,
        callee,
        resolved,
        raises,
        args,
        kwargs,
        arg_places,
        kwarg_places,
        capture_accesses,
        param_arg_regs,
        param_decls,
        instantiated_contract,
        instantiated_args,
        ..
    } = instruction
    else {
        return;
    };
    verify_capture_accesses(prefix, cx.function, capture_accesses, errors);
    if arg_places.len() != args.len() || kwarg_places.len() != kwargs.len() {
        errors.push(format!(
            "{prefix}: indirect-call place metadata is not aligned with its arguments"
        ));
    }
    let stored_contract = cx
        .reg_ty(*callee)
        .and_then(mojito_types::types::callable_contract_ty);
    let mut verified_instantiation = None;
    let contract = match stored_contract {
        Some(symbolic @ Ty::GenericFunc { .. }) => {
            if let Some(found) = instantiated_contract {
                match instantiate_generic_callable_contract(symbolic, instantiated_args) {
                    Ok(expected) => {
                        if found != &expected {
                            errors.push(format!(
                                        "{prefix}: checker-instantiated callable contract does not match its retained generic arguments"
                                    ));
                        }
                        verified_instantiation = Some(expected);
                    }
                    Err(reason) => errors.push(format!(
                        "{prefix}: invalid generic callable instantiation: {reason}"
                    )),
                }
                verified_instantiation.as_ref()
            } else {
                if !instantiated_args.is_empty() {
                    errors.push(format!(
                                "{prefix}: symbolic generic indirect call carries an orphaned instantiation witness"
                            ));
                }
                if let Err(reason) = validate_dependent_bindings(symbolic) {
                    errors.push(format!(
                        "{prefix}: invalid symbolic generic callable contract: {reason}"
                    ));
                }
                // Calls in an unspecialized generic body remain under
                // the callable contract's own explicit binders. Their
                // dependent references are scope-validated below; no
                // concrete substitution witness exists at this layer.
                Some(symbolic)
            }
        }
        Some(contract) => {
            if instantiated_contract.is_some() || !instantiated_args.is_empty() {
                errors.push(format!(
                    "{prefix}: nongeneric indirect call carries generic instantiation metadata"
                ));
            }
            Some(contract)
        }
        None => {
            if instantiated_contract.is_some() || !instantiated_args.is_empty() {
                errors.push(format!(
                    "{prefix}: non-callable indirect operand carries generic instantiation metadata"
                ));
            }
            None
        }
    };
    if let Some(contract) = contract {
        verify_callable_contract_call(
            prefix,
            cx.function,
            contract,
            raises,
            *dest,
            args,
            kwargs,
            arg_places,
            errors,
        );
    }
    let checked_decls = cx.reg_ty(*callee).and_then(generic_callable_decls);
    if let Some(checked_decls) = checked_decls {
        if checked_decls != param_decls {
            errors.push(format!(
                        "{prefix}: indirect-call compile-time parameter metadata does not match its callable contract"
                    ));
        }
        verify_param_arguments(prefix, cx.function, checked_decls, param_arg_regs, errors);
    } else if !param_decls.is_empty() {
        errors.push(format!(
            "{prefix}: nongeneric indirect call carries compile-time parameter metadata"
        ));
    }
    let nominal_name = match cx.reg_ty(*callee) {
        Some(Ty::Struct(name, _)) => Some(name.as_str()),
        _ => None,
    };
    if let Some(target) = resolved {
        if nominal_name.is_none()
            && let Some(expected) = cx
                .reg_ty(*callee)
                .and_then(mojito_symbol::symbol::callable_contract_target)
            && target != &expected
        {
            errors.push(format!(
                        "{prefix}: indirect-call target '{target}' does not match callable contract '{expected}'"
                    ));
        }
        let is_call_target =
            mojito_symbol::symbol::split_method_symbol(target).is_some_and(|(_, method)| {
                method == "__call__" || mojito_symbol::symbol::is_overload_of(method, "__call__")
            });
        if !is_call_target {
            errors.push(format!(
                "{prefix}: indirect-call target '{target}' is not a __call__ method"
            ));
        }
        let concrete = nominal_name
            .and_then(|name| mojito_symbol::symbol::retarget_method_symbol(target, name))
            .unwrap_or_else(|| target.clone());
        if let Some(declaration) = declared(cx.declarations, &concrete) {
            verify_direct_call(
                prefix,
                cx.function,
                declaration,
                args,
                kwargs,
                arg_places,
                errors,
            );
        } else if nominal_name.is_some() {
            errors.push(format!(
                "{prefix}: nominal indirect-call target '{concrete}' is undeclared"
            ));
        }
    } else if let Some(name) = nominal_name {
        errors.push(format!(
            "{prefix}: nominal callable '{name}' has no checker-selected __call__ target"
        ));
    }
}
