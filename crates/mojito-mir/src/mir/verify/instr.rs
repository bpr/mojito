//! The per-instruction verifier.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use crate::mir::{MirCaptureMode, MirClosureCapture};
use mojito_ast::ast::ArgConvention;
use mojito_types::types::TyKind;
use mojito_types::types::{held_simd_ty, held_ty};

pub(super) fn verify_instruction(
    name: &str,
    function: &MirFunction,
    declarations: &MirDeclarations,
    block_index: usize,
    instruction: &MirInstr,
    context: &RegionContext,
    errors: &mut Vec<String>,
) {
    let prefix = format!("MIR function '{name}' block {block_index}");
    // Loan places are analytical origin paths and may name a nominal
    // collection element (`owner[index]`) even though executable collection
    // access retained its checked method call. Every other instruction place
    // is VM navigation and must name concrete indexed storage.
    let executable_place = !matches!(instruction, MirInstr::EstablishLoans { .. });
    for place in instruction_places(instruction) {
        verify_place(
            name,
            block_index,
            function,
            declarations,
            place,
            executable_place,
            errors,
        );
    }
    // Register bounds and type completeness.
    let mut regs = Vec::new();
    instruction_result_regs(instruction, &mut regs);
    instruction_operand_regs(instruction, &mut regs);
    for register in &regs {
        if register.0 >= function.n_regs {
            errors.push(format!("{prefix}: invalid register r{}", register.0));
        } else if !function.reg_types.contains_key(&register.0) {
            errors.push(format!("{prefix}: untyped register r{}", register.0));
        }
    }
    // A slot an instruction declares, reads, or drops has the type its
    // declaring op gave it.
    if let MirInstr::UseVar { var, .. }
    | MirInstr::DeclareVar { var }
    | MirInstr::DropVar { var, .. } = instruction
        && (*var as usize) < function.n_vars
        && !function.var_tys.contains_key(var)
    {
        let slot = function
            .var_names
            .get(*var as usize)
            .map_or("", String::as_str);
        errors.push(format!("{prefix}: untyped variable slot ${var} ('{slot}')"));
    }
    let cx = InstrCx {
        name,
        prefix: &prefix,
        function,
        declarations,
        context,
    };
    match instruction {
        MirInstr::MakeSimd { .. }
        | MirInstr::SimdCast { .. }
        | MirInstr::SimdBitcast { .. }
        | MirInstr::SimdShuffle { .. } => verify_simd_instruction(&cx, instruction, errors),
        MirInstr::SimdSelect { .. }
        | MirInstr::SimdExtract { .. }
        | MirInstr::SimdInsert { .. }
        | MirInstr::SimdReduce { .. } => verify_simd_lane_instruction(&cx, instruction, errors),
        MirInstr::EstablishLoans { .. } | MirInstr::InvalidateInteriors { .. } => {
            verify_loan_instruction(&cx, instruction, errors);
        }
        MirInstr::MakeRef { .. }
        | MirInstr::ReadRef { .. }
        | MirInstr::WriteRef { .. }
        | MirInstr::Store { .. }
        | MirInstr::StoreRef { .. } => verify_reference_instruction(&cx, instruction, errors),
        MirInstr::ScalarCast {
            dest,
            value,
            target,
        } => verify_scalar_cast(&cx, *dest, *value, *target, errors),
        MirInstr::BuiltinCast { dest, value } => verify_builtin_cast(&cx, *dest, *value, errors),
        MirInstr::PointerConvert { dest, value } => {
            verify_pointer_convert(&cx, *dest, *value, errors);
        }
        MirInstr::Select {
            dest,
            condition,
            on_true,
            on_false,
        } => verify_select(&cx, *dest, *condition, [*on_true, *on_false], errors),
        MirInstr::Pop { dest, op, operands } => verify_pop(&cx, *dest, *op, operands, errors),
        MirInstr::ArrayRepeat { .. }
        | MirInstr::ArrayGet { .. }
        | MirInstr::ArrayReplace { .. } => {
            verify_array_op(&cx, instruction, errors);
        }
        MirInstr::CallLlvmIntrinsic { args, spread, .. } => {
            verify_llvm_intrinsic(&cx, args, *spread, errors);
        }
        MirInstr::MaterializeLiteral { .. }
        | MirInstr::ConstructTypeParam { .. }
        | MirInstr::ConstructType { .. }
        | MirInstr::SizeOf { .. }
        | MirInstr::ParamListAddress { .. }
        | MirInstr::StoreToMem { .. }
        | MirInstr::TypeName { .. }
        | MirInstr::CopyValue { .. }
        | MirInstr::Rebind { .. }
        | MirInstr::DefVar { .. }
        | MirInstr::MakeStruct { .. }
        | MirInstr::MakeVariant { .. }
        | MirInstr::MakeClosure { .. } => verify_value_instruction(&cx, instruction, errors),
        MirInstr::GetIter { .. } | MirInstr::Next { .. } => {
            verify_iteration_instruction(&cx, instruction, errors);
        }
        MirInstr::Index { .. } | MirInstr::MultiIndex { .. } | MirInstr::MultiSet { .. } => {
            verify_subscript_instruction(&cx, instruction, errors);
        }
        MirInstr::PointerStorageTake { .. }
        | MirInstr::PointerStorageDestroy { .. }
        | MirInstr::UninitStorage { .. }
        | MirInstr::UninitStorageTake { .. }
        | MirInstr::UninitStorageDestroy { .. } => {
            verify_storage_instruction(&cx, instruction, errors);
        }
        MirInstr::Call { .. } | MirInstr::CallIndirect { .. } => {
            verify_call_instruction(&cx, instruction, errors);
        }
        MirInstr::MethodCall {
            dest,
            result_adapter: Some(adapter),
            ..
        } => {
            verify_call_instruction(&cx, instruction, errors);
            verify_lifecycle_symbol(
                &cx,
                "method result adapter",
                cx.reg_ty(*dest),
                Some("__copyinit__"),
                adapter.constructor.as_deref(),
                errors,
            );
        }
        MirInstr::MethodCall { .. } => verify_call_instruction(&cx, instruction, errors),
        MirInstr::DropVar { var, destructor } => verify_lifecycle_symbol(
            &cx,
            "drop",
            function.var_tys.get(var),
            Some("__deinit__"),
            destructor.as_deref(),
            errors,
        ),
        MirInstr::Drop { reg, destructor } => verify_lifecycle_symbol(
            &cx,
            "drop",
            function.reg_types.get(&reg.0),
            Some("__deinit__"),
            destructor.as_deref(),
            errors,
        ),
        MirInstr::DropPlace { place, destructor } => verify_lifecycle_symbol(
            &cx,
            "drop",
            place.ty.as_ref(),
            Some("__deinit__"),
            destructor.as_deref(),
            errors,
        ),
        MirInstr::UseVar {
            var,
            mode,
            constructor,
            ..
        } => verify_lifecycle_symbol(
            &cx,
            "use",
            function.var_tys.get(var),
            crate::mir::use_constructor_member(*mode),
            constructor.as_deref(),
            errors,
        ),
        MirInstr::VariantSet {
            place,
            index,
            constructor,
            destructors,
            ..
        } => {
            verify_lifecycle_symbol(
                &cx,
                "variant payload move",
                crate::mir::variant_place_payload_ty(place, index, &function.var_tys),
                Some("__moveinit__"),
                constructor.as_deref(),
                errors,
            );
            verify_variant_destructors(&cx, place, destructors, errors);
        }
        MirInstr::VariantSetInitWith {
            place, destructors, ..
        } => verify_variant_destructors(&cx, place, destructors, errors),
        MirInstr::VariantReplace {
            place,
            input_index: index,
            constructor,
            ..
        } => verify_lifecycle_symbol(
            &cx,
            "variant payload move",
            crate::mir::variant_place_payload_ty(place, index, &function.var_tys),
            Some("__moveinit__"),
            constructor.as_deref(),
            errors,
        ),
        // A parameter value constant is a closed vector, a struct or tuple
        // of such values, scalars, and strings, a string read at the
        // nominal `String`, a `UInt` read at `UInt`, or an `index` read at
        // `index`.
        MirInstr::Const {
            k: crate::mir::Const::Value(CtValue::UInt(_)),
            dest,
        } => {
            if let Some(ty) = cx.reg_ty(*dest)
                && !matches!(ty.kind(), mojito_types::types::TyKind::UINT)
            {
                errors.push(format!(
                    "{}: `UInt` value constant is read at `{ty}`",
                    cx.prefix
                ));
            }
        }
        MirInstr::Const {
            k: crate::mir::Const::Value(CtValue::Index(_)),
            dest,
        } => {
            if let Some(ty) = cx.reg_ty(*dest)
                && !matches!(ty.kind(), mojito_types::types::TyKind::Index)
            {
                errors.push(format!(
                    "{}: `index` value constant is read at `{ty}`",
                    cx.prefix
                ));
            }
        }
        MirInstr::Const {
            k: crate::mir::Const::Value(CtValue::Str(_)),
            dest,
        } => {
            if let Some(ty) = cx.reg_ty(*dest)
                && !mojito_types::types::is_nominal_string(ty)
            {
                errors.push(format!(
                    "{}: string parameter value constant is read at `{ty}`, not `String`",
                    cx.prefix
                ));
            }
        }
        MirInstr::Const {
            k: crate::mir::Const::Value(value @ (CtValue::MlirAttr(_) | CtValue::KgenTarget(_))),
            dest,
        } => {
            if let Some(ty) = cx.reg_ty(*dest)
                && !mojito_types::types::has_no_llvm_type(ty.kind())
            {
                errors.push(format!(
                    "{}: attribute constant `{value}` is read at `{ty}`, which has an LLVM type",
                    cx.prefix
                ));
            }
        }
        MirInstr::Const {
            k: crate::mir::Const::Value(CtValue::KgenString(_)),
            dest,
        } => {
            if let Some(ty) = cx.reg_ty(*dest)
                && !matches!(ty.kind(), TyKind::KgenString)
            {
                errors.push(format!(
                    "{}: KGEN string constant is read at `{ty}`, not `!kgen.string`",
                    cx.prefix
                ));
            }
        }
        MirInstr::Const {
            k: crate::mir::Const::Value(value),
            ..
        } if !value.is_materialized_constant() => errors.push(format!(
            "{}: parameter value constant `{value}` is not a closed vector, struct, or tuple",
            cx.prefix
        )),
        _ => {}
    }
    verify_effect_instruction(&cx, instruction, errors);
}

/// A raising site in a nonraising function must sit under a handler; a
/// region op verifies each of its regions under that region's protection.
fn verify_effect_instruction(cx: &InstrCx<'_>, instruction: &MirInstr, errors: &mut Vec<String>) {
    let prefix = cx.prefix;
    verify_regions(cx, instruction, errors);
    // A call carrying a checked error contract raises unless handled.
    if instruction.call_error().is_some() && !cx.function.raises && !cx.context.protected {
        errors.push(format!(
            "{prefix}: unprotected raising call in nonraising function"
        ));
    }
}

/// A variant store names, per alternative, the destructor the previous
/// payload runs under that tag ([`verify_lifecycle_symbol`]); a list that
/// is not one entry per alternative is an error.
fn verify_variant_destructors(
    cx: &InstrCx<'_>,
    place: &MirPlace,
    destructors: &[Option<String>],
    errors: &mut Vec<String>,
) {
    let Some(alternatives) = crate::mir::variant_place_alternatives(place, &cx.function.var_tys)
    else {
        return;
    };
    if !destructors.is_empty() && destructors.len() != alternatives.len() {
        errors.push(format!(
            "{}: variant store names {} destructors for {} alternatives",
            cx.prefix,
            destructors.len(),
            alternatives.len()
        ));
        return;
    }
    for (position, alternative) in alternatives.iter().enumerate() {
        verify_lifecycle_symbol(
            cx,
            "variant payload drop",
            Some(alternative),
            Some("__deinit__"),
            destructors.get(position).and_then(Option::as_deref),
            errors,
        );
    }
}

/// A drop names the destructor its value's struct records, and a copy or
/// move the constructor it runs, the template member in a parametric body
/// and its instance in concrete MIR, and only a declared function: a pass
/// that makes a drop, a copy, or a move names it, as upstream's destructor
/// and copy/move insertion name the member at each site. `member` is the
/// lifecycle member the site runs, `None` for a use that runs none.
pub(super) fn verify_lifecycle_symbol(
    cx: &InstrCx<'_>,
    site: &str,
    ty: Option<&Ty>,
    member: Option<&str>,
    named: Option<&str>,
    errors: &mut Vec<String>,
) {
    let prefix = cx.prefix;
    let Some(member) = member else {
        if let Some(named) = named {
            errors.push(format!(
                "{prefix}: {site} runs no member but names `{named}`"
            ));
        }
        return;
    };
    if let Some(named) = named
        && cx.declarations.function(named).is_none()
    {
        errors.push(format!(
            "{prefix}: {site} names the undeclared {member} `{named}`"
        ));
    }
    let Some(ty) = ty else {
        return;
    };
    if let Some(recorded) = cx.declarations.struct_member(ty, member)
        && named != Some(recorded)
    {
        errors.push(format!(
            "{prefix}: {site} of `{ty}` names {}, not its {member} `{recorded}`",
            named.map_or_else(|| format!("no {member}"), |named| format!("`{named}`"))
        ));
    }
}

fn verify_reference_instruction(
    cx: &InstrCx<'_>,
    instruction: &MirInstr,
    errors: &mut Vec<String>,
) {
    let prefix = cx.prefix;
    match instruction {
        MirInstr::MakeRef { dest, place } => {
            if let Some(destination) = cx.reg_ty(*dest) {
                let Some(capability) = reference_capability(destination) else {
                    errors.push(format!(
                        "{prefix}: MakeRef destination has non-reference-capability type {destination}"
                    ));
                    return;
                };
                if let Some((target, source_permission)) = make_ref_target(place) {
                    // A place ending at a stored reference admits a second
                    // interpretation beside the storage borrow: forwarding the
                    // stored handle itself (`ref s = self.src` reborrows).
                    // The runtime chases such handles symmetrically, so a
                    // destination typed as the stored handle is accepted when
                    // the stored capability grants its permission.
                    let forwarded = match place.ty.as_ref().map(Ty::kind) {
                        Some(TyKind::Ref(stored))
                            if !types_compatible(capability.target, target)
                                && types_compatible(capability.target, &stored.referent) =>
                        {
                            Some(ReferencePermission::from_mutability(stored.mutability))
                        }
                        _ => None,
                    };
                    if let Some(stored_permission) = forwarded {
                        if !stored_permission.satisfies(capability.permission) {
                            errors.push(format!(
                                "{prefix}: MakeRef destination recovers permission unavailable through its source capability"
                            ));
                        }
                    } else {
                        if !types_compatible(capability.target, target) {
                            errors.push(format!(
                                "{prefix}: MakeRef destination targets {}, incompatible with place storage {target}",
                                capability.target
                            ));
                        }
                        if source_permission
                            .is_some_and(|permission| !permission.satisfies(capability.permission))
                        {
                            errors.push(format!(
                                "{prefix}: MakeRef destination recovers permission unavailable through its source capability"
                            ));
                        }
                    }
                }
            }
        }
        MirInstr::ReadRef { dest, reference } => {
            if let Some(source) = cx.reg_ty(*reference) {
                let Some(capability) = reference_capability(source) else {
                    errors.push(format!(
                        "{prefix}: ReadRef source has non-reference-capability type {source}"
                    ));
                    return;
                };
                if let Some(destination) = cx.reg_ty(*dest)
                    && !types_compatible(destination, capability.target)
                {
                    errors.push(format!(
                        "{prefix}: ReadRef result type {destination} is incompatible with referent {}",
                        capability.target
                    ));
                }
            }
        }
        MirInstr::WriteRef { reference, value } => {
            if let Some(source) = cx.reg_ty(*reference) {
                let Some(capability) = reference_capability(source) else {
                    errors.push(format!(
                        "{prefix}: WriteRef source has non-reference-capability type {source}"
                    ));
                    return;
                };
                if !capability.permission.allows_write() {
                    errors.push(format!(
                        "{prefix}: WriteRef source capability of type {source} is immutable"
                    ));
                }
                if let Some(value) = cx.reg_ty(*value)
                    && !types_compatible(value, capability.target)
                {
                    errors.push(format!(
                        "{prefix}: WriteRef value type {value} is incompatible with referent {}",
                        capability.target
                    ));
                }
            }
        }
        MirInstr::Store { place, src } => {
            if let (Some(expected), Some(found)) = (place.ty.as_ref(), cx.reg_ty(*src)) {
                let target = match expected.kind() {
                    TyKind::Ref(reference) => &reference.referent,
                    _ => expected,
                };
                // Storage of a reference type (a `List[ref T]` element) takes
                // a reference; any other store writes through to the referent.
                if !types_compatible(found, target) && !types_compatible(found, expected) {
                    errors.push(format!(
                        "{prefix}: store of {found} into storage of type {target}"
                    ));
                }
            }
        }
        MirInstr::StoreRef { place, reference } => {
            if let Some(storage) = place.ty.as_ref() {
                let TyKind::Ref(storage_reference) = storage.kind() else {
                    errors.push(format!(
                        "{prefix}: StoreRef into non-reference storage of type {storage}"
                    ));
                    return;
                };
                if let Some(source) = cx.reg_ty(*reference) {
                    let TyKind::Ref(source_reference) = source.kind() else {
                        errors.push(format!(
                            "{prefix}: StoreRef source has non-reference type {source}"
                        ));
                        return;
                    };
                    if !types_compatible(&source_reference.referent, &storage_reference.referent) {
                        errors.push(format!(
                            "{prefix}: StoreRef source referent {} is incompatible with storage referent {}",
                            source_reference.referent, storage_reference.referent
                        ));
                    }
                    let source_permission =
                        ReferencePermission::from_mutability(source_reference.mutability);
                    let storage_permission =
                        ReferencePermission::from_mutability(storage_reference.mutability);
                    if !source_permission.satisfies(storage_permission) {
                        errors.push(format!(
                            "{prefix}: StoreRef source permission cannot initialize storage of type {storage}"
                        ));
                    }
                }
            }
        }
        _ => {}
    }
}

fn verify_value_instruction(cx: &InstrCx<'_>, instruction: &MirInstr, errors: &mut Vec<String>) {
    let prefix = cx.prefix;
    match instruction {
        MirInstr::MaterializeLiteral { value, target, .. } => {
            let valid_target =
                matches!(target.kind(), TyKind::INT | TyKind::UINT | TyKind::FLOAT64)
                    || mojito_types::types::is_scalar_simd(target);
            if !valid_target {
                errors.push(format!(
                    "{prefix}: literal materialization has non-scalar target {target}"
                ));
            }
            // A target whose lane dtype is still a parameter expression takes
            // either literal; the instance's dtype decides the value.
            let symbolic_lane =
                matches!(target.kind(), TyKind::Simd { dtype, .. } if dtype.is_symbolic());
            if let Some(found) = cx.reg_ty(*value) {
                let valid_source = match found.kind() {
                    TyKind::IntLiteral => valid_target,
                    TyKind::FloatLiteral => {
                        matches!(target.kind(), TyKind::FLOAT64)
                            || symbolic_lane
                            || mojito_types::types::scalar_simd_dtype(target)
                                .is_some_and(mojito_ast::ast::Dtype::is_float)
                    }
                    _ => false,
                };
                // The pin's literal conversion is a constrained `__init__`
                // call, judged only in the instance that reaches it.
                if !valid_source && !cx.context.judged_per_instance() {
                    errors.push(format!("{prefix}: cannot materialize {found} as {target}"));
                }
            }
        }
        MirInstr::ConstructTypeParam {
            dest,
            param,
            kwargs,
            kwarg_places,
            element,
        } => {
            if let Some(element) = element {
                if element.value.is_none() && element.expr.is_none() {
                    errors.push(format!(
                        "{prefix}: element construction of pack '{}' names no index",
                        param.name
                    ));
                }
                if !kwargs.is_empty() {
                    errors.push(format!(
                        "{prefix}: element construction of pack '{}' passes keyword arguments",
                        param.name
                    ));
                }
                if let Some(found) = cx.reg_ty(*dest)
                    && !matches!(found.kind(), TyKind::Dependent(dependent) if dependent.pack_element().is_some())
                {
                    errors.push(format!(
                        "{prefix}: element construction of pack '{}' has result type {found}",
                        param.name
                    ));
                }
                return;
            }
            if kwarg_places.len() != kwargs.len() {
                errors.push(format!(
                    "{prefix}: type-parameter construction of '{}' has {} keyword places for {} keyword arguments",
                    param.name,
                    kwarg_places.len(),
                    kwargs.len()
                ));
            }
            if let Some(found) = cx.reg_ty(*dest)
                && !matches!(found.kind(), TyKind::Param { binder, .. } if binder == param)
                && !matches!(found.kind(), TyKind::Struct(..))
            {
                errors.push(format!(
                    "{prefix}: type-parameter construction of '{}' has result type {found}",
                    param.name
                ));
            }
        }
        MirInstr::ConstructType { dest, ty } => {
            if let Some(found) = cx.reg_ty(*dest)
                && found != ty
            {
                errors.push(format!(
                    "{prefix}: construction of the type {ty} has result type {found}"
                ));
            }
        }
        MirInstr::StoreToMem { dest, src } => match cx.reg_ty(*dest).map(Ty::kind) {
            None => {}
            Some(TyKind::Ref(reference)) => {
                if let Some(stored) = cx.reg_ty(*src)
                    && *stored != reference.referent
                {
                    errors.push(format!(
                        "{prefix}: store_to_mem of a {stored} yields a reference to {}",
                        reference.referent
                    ));
                }
            }
            Some(_) => errors.push(format!("{prefix}: store_to_mem must yield a reference")),
        },
        MirInstr::ParamListAddress { dest, values } => {
            if !matches!(
                cx.reg_ty(*dest).map(Ty::kind),
                None | Some(TyKind::Pointer {
                    origin: mojito_types::origin::PointerOrigin::Static,
                    ..
                })
            ) {
                errors.push(format!(
                    "{prefix}: a parameter list's address must be a static pointer"
                ));
            }
            if !matches!(
                values,
                crate::mir::Const::Param(_)
                    | crate::mir::Const::Value(mojito_types::ct::CtValue::Tuple(_))
            ) {
                errors.push(format!(
                    "{prefix}: a parameter list's address names no parameter list"
                ));
            }
        }
        MirInstr::SizeOf { dest, ty } => {
            if let Some(found) = cx.reg_ty(*dest)
                && found.kind() != TyKind::INT
            {
                errors.push(format!(
                    "{prefix}: size_of result register has type {found}, expected Int"
                ));
            }
            // A template's symbolic type has a layout only once an instance
            // substitutes it; the concrete mode rejects the query itself.
            if mojito_types::types::is_symbolic(ty) {
                return;
            }
            // Whether a type has a layout at all is target-independent
            // (pointer size is the one target fact, and every target has
            // one); the elaborator answers the size under the real target.
            let target = mojito_native_core::target::NativeTarget::new(
                mojito_native_core::target::Triple::X86_64UnknownLinuxGnu,
            );
            let structs = crate::mir::struct_field_index_reaching(cx.declarations, ty);
            if let Err(error) = (mojito_native_core::layout::LayoutCx {
                target: &target,
                structs: &structs,
            })
            .layout_of(ty)
            {
                errors.push(format!("{prefix}: size_of has no layout for {ty}: {error}"));
            }
        }
        MirInstr::TypeName { dest, ty } => {
            if let Some(found) = cx.reg_ty(*dest)
                && !found.is_string_literal()
            {
                errors.push(format!(
                    "{prefix}: type name result register has type {found}, expected StringLiteral"
                ));
            }
            if !mojito_types::types::is_symbolic(ty) {
                errors.push(format!(
                    "{prefix}: type name of the concrete type {ty} is not a constant"
                ));
            }
        }
        MirInstr::CopyValue {
            dest,
            value,
            constructor,
        } => {
            if let (Some(found), Some(expected)) = (cx.reg_ty(*value), cx.reg_ty(*dest))
                && !types_compatible(found, expected)
            {
                errors.push(format!(
                    "{prefix}: copied value has type {found}, destination has type {expected}"
                ));
            }
            verify_lifecycle_symbol(
                cx,
                "copy",
                cx.reg_ty(*dest),
                Some("__copyinit__"),
                constructor.as_deref(),
                errors,
            );
        }
        MirInstr::Rebind { dest, value } => verify_rebind(cx, *dest, *value, errors),
        MirInstr::DefVar {
            src,
            binding_ty: Some(expected),
            ..
        } => {
            if let Some(found) = cx.reg_ty(*src)
                && !types_compatible(found, expected)
            {
                errors.push(format!(
                    "{prefix}: binding of {found} to a slot of type {expected}"
                ));
            }
        }
        MirInstr::MakeStruct { dest, fields } => verify_struct_creation(cx, *dest, fields, errors),
        MirInstr::MakeVariant {
            alternatives,
            index,
            value,
            constructor,
            ..
        } => {
            verify_lifecycle_symbol(
                cx,
                "variant payload move",
                crate::mir::variant_payload_ty(alternatives, index),
                Some("__moveinit__"),
                constructor.as_deref(),
                errors,
            );
            if let Some(index) = index.known() {
                if index >= alternatives.len() {
                    errors.push(format!(
                        "{prefix}: variant construction index {index} out of {} alternatives",
                        alternatives.len()
                    ));
                } else if let Some(found) = cx.reg_ty(*value)
                    && !types_compatible(found, &alternatives[index])
                {
                    errors.push(format!(
                        "{prefix}: variant payload {found} does not fit alternative {}",
                        alternatives[index]
                    ));
                }
            }
        }
        MirInstr::MakeClosure {
            dest,
            function: target,
            captures,
        } => {
            for capture in captures {
                let ty = capture.place.storage_ty(&cx.function.var_tys);
                verify_lifecycle_symbol(
                    cx,
                    "closure capture",
                    ty,
                    capture.constructor_member(),
                    capture.constructor.as_deref(),
                    errors,
                );
                verify_lifecycle_symbol(
                    cx,
                    "closure capture release",
                    ty,
                    capture.destructor_member(),
                    capture.destructor.as_deref(),
                    errors,
                );
            }
            match declared(cx.declarations, target) {
                None => errors.push(format!(
                    "{prefix}: closure refers to undeclared lifted function '{target}'"
                )),
                Some(declaration) => {
                    let parameters =
                        declaration.param_types.len() + usize::from(declaration.has_receiver);
                    if captures.len() > parameters {
                        errors.push(format!(
                            "{prefix}: closure has {} captures for '{}' with only {} parameters",
                            captures.len(),
                            target,
                            parameters
                        ));
                    }
                    if !captures
                        .iter()
                        .enumerate()
                        .all(|(index, capture)| capture_binds(declaration, index, capture))
                    {
                        errors.push(format!(
                            "{prefix}: closure environment for '{target}' binds neither, by reference, a leading reference parameter nor, by reference or moved in, a leading `read` one of its type"
                        ));
                    }
                }
            }
            if let Some(found) = cx.reg_ty(*dest)
                && !matches!(found.kind(), TyKind::Func(_) | TyKind::GenericFunc(_))
            {
                errors.push(format!(
                    "{prefix}: closure result has non-callable type {found}"
                ));
            }
        }
        _ => {}
    }
}

/// Two closed types a rebind names must already agree; the elaborator
/// judges one over a parameter per instance, and one at a position judged
/// per instance (`RegionContext::judged_per_instance`): an inlined
/// `comptime if type == NoneType: return rebind[type](None)`, or an inlined
/// `select` over a non-`bool` mask after its `comptime assert`.
fn verify_rebind(cx: &InstrCx<'_>, dest: Reg, value: Reg, errors: &mut Vec<String>) {
    if let (Some(found), Some(expected)) = (cx.reg_ty(value), cx.reg_ty(dest))
        && !types_compatible(found, expected)
        && !cx.context.judged_per_instance()
    {
        errors.push(format!(
            "{}: rebinds {found} to {expected}, which never agree",
            cx.prefix
        ));
    }
}

fn verify_storage_instruction(cx: &InstrCx<'_>, instruction: &MirInstr, errors: &mut Vec<String>) {
    let prefix = cx.prefix;
    match instruction {
        MirInstr::PointerStorageTake {
            dest,
            pointer,
            index,
            element,
        }
        | MirInstr::PointerStorageDestroy {
            dest,
            pointer,
            index,
            element,
            ..
        } => {
            if let MirInstr::PointerStorageDestroy { destructor, .. } = instruction {
                verify_lifecycle_symbol(
                    cx,
                    "storage destroy",
                    Some(element),
                    Some("__deinit__"),
                    destructor.as_deref(),
                    errors,
                );
            }
            match cx.reg_ty(*pointer).map(Ty::kind) {
                Some(TyKind::Pointer {
                    element: actual,
                    origin,
                }) if origin.statically_mutable() != Some(false)
                    && types_compatible(actual, element) => {}
                Some(found) => errors.push(format!(
                    "{prefix}: compiler-private pointer storage operation expects a mutable Pointer[{element}], got {found}"
                )),
                None => {}
            }
            if let Some(found) = cx.reg_ty(*index)
                && !types_compatible(found, &Ty::int())
            {
                errors.push(format!(
                    "{prefix}: compiler-private pointer storage index has type {found}, expected Int"
                ));
            }
            let none = Ty::none();
            let expected = if matches!(instruction, MirInstr::PointerStorageTake { .. }) {
                element
            } else {
                &none
            };
            if let Some(found) = cx.reg_ty(*dest)
                && !types_compatible(found, expected)
            {
                errors.push(format!(
                    "{prefix}: compiler-private pointer storage result has type {found}, expected {expected}"
                ));
            }
        }
        MirInstr::UninitStorage { dest, init } => {
            if let Some(found) = cx.reg_ty(*dest)
                && mojito_types::types::uninit_storage_element(found).is_none()
            {
                errors.push(format!(
                    "{prefix}: inline uninit storage construction has type {found}, expected {}",
                    mojito_types::types::UNINIT_STORAGE_TYPE_NAME
                ));
            }
            if let Some(init) = init
                && let Some(element) = cx
                    .reg_ty(*dest)
                    .and_then(mojito_types::types::uninit_storage_element)
                && let Some(found) = cx.reg_ty(*init)
                && !types_compatible(found, element)
            {
                errors.push(format!(
                    "{prefix}: inline uninit storage payload has type {found}, expected {element}"
                ));
            }
        }
        MirInstr::UninitStorageTake {
            dest,
            storage,
            element,
        }
        | MirInstr::UninitStorageDestroy {
            dest,
            storage,
            element,
            ..
        } => {
            if let MirInstr::UninitStorageDestroy { destructor, .. } = instruction {
                verify_lifecycle_symbol(
                    cx,
                    "storage destroy",
                    Some(element),
                    Some("__deinit__"),
                    destructor.as_deref(),
                    errors,
                );
            }
            if let Some(found) = cx.reg_ty(*storage)
                && !mojito_types::types::uninit_storage_element(found)
                    .is_some_and(|actual| types_compatible(actual, element))
            {
                errors.push(format!(
                    "{prefix}: inline uninit storage operation expects {}[{element}], got {found}",
                    mojito_types::types::UNINIT_STORAGE_TYPE_NAME
                ));
            }
            let none = Ty::none();
            let expected = if matches!(instruction, MirInstr::UninitStorageTake { .. }) {
                element
            } else {
                &none
            };
            if let Some(found) = cx.reg_ty(*dest)
                && !types_compatible(found, expected)
            {
                errors.push(format!(
                    "{prefix}: inline uninit storage result has type {found}, expected {expected}"
                ));
            }
        }
        _ => {}
    }
}

fn verify_simd_instruction(cx: &InstrCx<'_>, instruction: &MirInstr, errors: &mut Vec<String>) {
    let prefix = cx.prefix;
    let valid_simd_width = |width: usize| width >= 1 && width.is_power_of_two();
    match instruction {
        // Widths are validated during checked elaboration; this is the
        // phase-boundary backstop for assembled artifacts. A width that is
        // still a parameter expression is checked once an instance closes
        // it; `verify/scope.rs` checks its binders and kind here.
        MirInstr::MakeSimd { width, .. }
        | MirInstr::SimdCast { width, .. }
        | MirInstr::SimdBitcast { width, .. } => {
            if let Some(width) = width.known()
                && !usize::try_from(width).is_ok_and(valid_simd_width)
            {
                errors.push(format!(
                    "{prefix}: SIMD width {width} is not a positive power of two"
                ));
            }
        }
        // A template's mask is checked by the instance that closes it.
        MirInstr::SimdShuffle {
            dest,
            value,
            other,
            mask,
        } => {
            if let Some(mask) = mask.known() {
                if !valid_simd_width(mask.len()) {
                    errors.push(format!(
                        "{prefix}: SIMD shuffle mask length {} is not a positive power of two",
                        mask.len()
                    ));
                }
                if let Some((_, width)) = cx
                    .reg_ty(*dest)
                    .and_then(|ty| mojito_types::types::held_simd_slots(ty.kind()))
                    && let Some(width) = width.known()
                    && width != mask.len() as i64
                {
                    errors.push(format!(
                        "{prefix}: SIMD shuffle result has {width} lanes, its mask {}",
                        mask.len()
                    ));
                }
                if let Some((_, width)) = cx
                    .reg_ty(*value)
                    .and_then(|ty| mojito_types::types::held_simd_slots(ty.kind()))
                    && let Some(width) = width.known()
                {
                    let lanes = if other.is_some() { width * 2 } else { width };
                    if let Some(bad) = mask.iter().find(|lane| **lane as i64 >= lanes) {
                        errors.push(format!(
                            "{prefix}: SIMD shuffle lane {bad} is out of range for {lanes} source lanes"
                        ));
                    }
                }
            }
            if let Some(source) = cx.reg_ty(*value)
                && let Some(joined) = other.as_ref().and_then(|register| cx.reg_ty(*register))
                && held_simd_ty(joined) != held_simd_ty(source)
            {
                errors.push(format!(
                    "{prefix}: SIMD join operand {joined} does not match {source}"
                ));
            }
        }
        _ => {}
    }
}

/// The lane instructions, where their registers are typed: a `simd.select`
/// takes a `bool` mask and cases of its result type at the mask's width; a
/// `simd.extract` and a `simd.reduce` give one lane of the vector's dtype,
/// a `simd.insert` takes one (a `Bool` for `bool` lanes) and gives the
/// vector's type; and a lane index
/// is an `Int`, a `UInt`, or an `IntLiteral`.
fn verify_simd_lane_instruction(
    cx: &InstrCx<'_>,
    instruction: &MirInstr,
    errors: &mut Vec<String>,
) {
    let prefix = cx.prefix;
    let mnemonic = crate::mir::text::instruction_mnemonic(instruction);
    let slots = |register: Reg| {
        cx.reg_ty(register)
            .map(|ty| (ty, mojito_types::types::held_simd_slots(ty.kind())))
    };
    let mut expect_vector = |register: Reg, role: &str| match slots(register) {
        Some((ty, None)) => {
            errors.push(format!(
                "{prefix}: {mnemonic} {role} has the non-SIMD type {ty}"
            ));
            None
        }
        Some((_, Some(slots))) => Some(slots),
        None => None,
    };
    let lane_of = |dtype: &mojito_types::types::SimdDtype| {
        mojito_types::types::simd_ty_from_slots(
            dtype.clone(),
            mojito_types::types::SimdWidth::Known(1),
        )
        .ok()
    };
    let mut mismatches = Vec::new();
    let mut expect_type = |register: Reg, expected: Option<Ty>, role: &str| {
        if let (Some(found), Some(expected)) = (cx.reg_ty(register), expected)
            && held_simd_ty(found) != held_simd_ty(&expected)
        {
            mismatches.push(format!(
                "{prefix}: {mnemonic} {role} has type {found}, expected {expected}"
            ));
        }
    };
    let index_ok = |register: Reg| {
        cx.reg_ty(register).is_none_or(|ty| {
            types_compatible(ty, &Ty::int())
                || matches!(ty.kind(), TyKind::UINT | TyKind::IntLiteral | TyKind::Index)
        })
    };
    match *instruction {
        MirInstr::SimdSelect {
            dest,
            mask,
            on_true,
            on_false,
        } => {
            let mask_slots = expect_vector(mask, "mask");
            if let Some((dtype, _)) = &mask_slots
                && dtype
                    .known()
                    .is_some_and(|dtype| dtype != mojito_ast::ast::Dtype::Bool)
            {
                errors.push(format!(
                    "{prefix}: {mnemonic} mask has {dtype} lanes, not bool"
                ));
            }
            let result = cx.reg_ty(dest).cloned();
            if let (Some((_, mask_width)), Some(Some((_, width)))) = (
                &mask_slots,
                result
                    .as_ref()
                    .map(|ty| mojito_types::types::held_simd_slots(ty.kind())),
            ) && *mask_width != width
            {
                errors.push(format!(
                    "{prefix}: {mnemonic} mask of width {mask_width} selects a result of width {width}"
                ));
            }
            expect_type(on_true, result.clone(), "true case");
            expect_type(on_false, result, "false case");
        }
        MirInstr::SimdExtract { dest, value, index } => {
            let lane = expect_vector(value, "vector").and_then(|(dtype, _)| lane_of(&dtype));
            expect_type(dest, lane, "result");
            if !index_ok(index) {
                errors.push(format!("{prefix}: {mnemonic} index is not an Int"));
            }
        }
        MirInstr::SimdInsert {
            dest,
            value,
            element,
            index,
        } => {
            let lane = expect_vector(value, "vector").and_then(|(dtype, _)| lane_of(&dtype));
            // A `Bool` is a `bool` lane, as the checker coerces it.
            let bool_lane = lane.as_ref().and_then(|lane| {
                mojito_types::types::held_simd_slots(lane.kind())
                    .and_then(|(dtype, _)| dtype.known())
            }) == Some(mojito_ast::ast::Dtype::Bool)
                && cx.reg_ty(element) == Some(&Ty::bool());
            if !bool_lane {
                expect_type(element, lane, "element");
            }
            expect_type(dest, cx.reg_ty(value).cloned(), "result");
            if !index_ok(index) {
                errors.push(format!("{prefix}: {mnemonic} index is not an Int"));
            }
        }
        MirInstr::SimdReduce { dest, value, .. } => {
            let slots = expect_vector(value, "vector");
            if let Some((dtype, _)) = &slots
                && dtype.known().is_some_and(mojito_ast::ast::Dtype::is_float)
            {
                errors.push(format!("{prefix}: {mnemonic} reduces {dtype} lanes"));
            }
            expect_type(dest, slots.and_then(|(dtype, _)| lane_of(&dtype)), "result");
        }
        _ => {}
    }
    errors.extend(mismatches);
}

/// A `struct.create` builds a declared struct of as many fields as it
/// passes, each typed as declared. A generic struct's field types would
/// need substitution the verifier does not re-derive, as a field
/// projection's do.
fn verify_struct_creation(cx: &InstrCx<'_>, dest: Reg, fields: &[Reg], errors: &mut Vec<String>) {
    let prefix = cx.prefix;
    let declared = cx.reg_ty(dest).and_then(|ty| match ty.kind() {
        TyKind::Struct(name, arguments) => cx
            .declarations
            .structure(name)
            .map(|declaration| (declaration, arguments.is_empty())),
        _ => None,
    });
    let Some((declaration, concrete)) = declared else {
        errors.push(format!(
            "{prefix}: struct creation's result is no declared struct"
        ));
        return;
    };
    if declaration.fields.len() != fields.len() {
        errors.push(format!(
            "{prefix}: struct creation passes {} fields to a struct of {}",
            fields.len(),
            declaration.fields.len()
        ));
        return;
    }
    if !concrete {
        return;
    }
    for ((field, expected), value) in declaration.fields.iter().zip(fields) {
        if let Some(found) = cx.reg_ty(*value)
            && !types_compatible(found, expected)
        {
            errors.push(format!(
                "{prefix}: struct creation's field {field} is {found}, declared {expected}"
            ));
        }
    }
}

/// Whether the closure capture `capture` binds the leading parameter
/// `index` of its lifted body `declaration`: a reference parameter by
/// reference (a `@parameter` closure's), or a `read` one, a closure's
/// storage, by reference (a `def` used as a value) or moved in (a
/// lambda's) as a value of its type. A method's receiver leads its
/// parameters: a closure's storage is the `read` `self` of the method its
/// body is.
fn capture_binds(
    declaration: &MirFunctionDeclaration,
    index: usize,
    capture: &MirClosureCapture,
) -> bool {
    match index.checked_sub(usize::from(declaration.has_receiver)) {
        Some(index) => parameter_binds(declaration, index, capture),
        None => {
            declaration.receiver_convention.is_none()
                && match capture.mode {
                    MirCaptureMode::Reference => true,
                    MirCaptureMode::Move => capture.place.ty.as_ref().is_none_or(|ty| {
                        matches!(ty.kind(), TyKind::Struct(name, _)
                            if mojito_symbol::symbol::split_method_symbol(&declaration.lowered_name)
                                .is_some_and(|(owner, _)| owner == name))
                    }),
                }
        }
    }
}

/// [`capture_binds`] for the parameter `index`, its receiver apart, of
/// `declaration`.
fn parameter_binds(
    declaration: &MirFunctionDeclaration,
    index: usize,
    capture: &MirClosureCapture,
) -> bool {
    if declaration.ref_params.get(index).copied().unwrap_or(false) {
        return capture.mode == MirCaptureMode::Reference;
    }
    matches!(
        declaration.param_conventions.get(index),
        Some(None | Some(ArgConvention::Imm))
    ) && match capture.mode {
        MirCaptureMode::Reference => true,
        MirCaptureMode::Move => capture.place.ty.as_ref().is_none_or(|ty| {
            declaration
                .param_types
                .get(index)
                .is_some_and(|param| types_compatible(ty, param))
        }),
    }
}

/// A `scalar.cast` reads a value its target `casts_from` and defines a
/// register of the target type.
/// A `pop` lane op takes its arity's operands, each of the destination's
/// SIMD type when both are typed.
fn verify_pop(
    cx: &InstrCx<'_>,
    dest: Reg,
    op: mojito_types::mlir::PopOp,
    operands: &[Reg],
    errors: &mut Vec<String>,
) {
    let prefix = cx.prefix;
    if operands.len() != op.arity() {
        errors.push(format!(
            "{prefix}: pop.{} takes {} operand(s), got {}",
            op.mnemonic(),
            op.arity(),
            operands.len()
        ));
    }
    let Some(result) = cx.reg_ty(dest) else {
        return;
    };
    if op.is_string_read() {
        let expected = if op == mojito_types::mlir::PopOp::StringSize {
            Ty::index()
        } else {
            mojito_types::types::static_string_address_ty()
        };
        if *result != expected {
            errors.push(format!(
                "{prefix}: pop.{} defines a register of type {result}, not {expected}",
                op.mnemonic()
            ));
        }
        if let Some(found) = operands.first().and_then(|operand| cx.reg_ty(*operand))
            && !matches!(found.kind(), TyKind::KgenString)
        {
            errors.push(format!(
                "{prefix}: pop.{} reads an operand of type {found}, not {}",
                op.mnemonic(),
                mojito_types::types::KGEN_STRING_TYPE_SPELLING
            ));
        }
        return;
    }
    if op.is_compare() {
        if *result != Ty::i1() {
            errors.push(format!(
                "{prefix}: pop.{} defines a register of type {result}, not i1",
                op.mnemonic()
            ));
        }
        for operand in operands {
            if let Some(found) = cx.reg_ty(*operand)
                && *found != Ty::index()
            {
                errors.push(format!(
                    "{prefix}: pop.{} compares an operand of type {found}, not index",
                    op.mnemonic()
                ));
            }
        }
        return;
    }
    // `index.divs` divides `index` values.
    if mojito_types::types::held_simd_slots(result.kind()).is_none() && *result != Ty::index() {
        errors.push(format!(
            "{prefix}: pop.{} defines a register of the non-SIMD type {result}",
            op.mnemonic()
        ));
    }
    for operand in operands {
        if let Some(found) = cx.reg_ty(*operand)
            && held_simd_ty(found) != held_simd_ty(result)
        {
            errors.push(format!(
                "{prefix}: pop.{} operand of type {found} where its result is {result}",
                op.mnemonic()
            ));
        }
    }
}

/// An LLVM intrinsic call's spread names one of its operands, and that
/// operand is a heterogeneous collector (a tuple once the pack is bound).
fn verify_llvm_intrinsic(
    cx: &InstrCx<'_>,
    args: &[Reg],
    spread: Option<usize>,
    errors: &mut Vec<String>,
) {
    let prefix = cx.prefix;
    let Some(position) = spread else {
        return;
    };
    let Some(pack) = args.get(position) else {
        errors.push(format!(
            "{prefix}: llvm.intrinsic spreads operand {position} of {}",
            args.len()
        ));
        return;
    };
    if let Some(found) = cx.reg_ty(*pack)
        && !mojito_types::types::is_heterogeneous_collector(found)
        && !matches!(found.kind(), mojito_types::types::TyKind::Tuple(_))
    {
        errors.push(format!(
            "{prefix}: llvm.intrinsic spreads the non-pack {found}"
        ));
    }
}

fn verify_scalar_cast(
    cx: &InstrCx<'_>,
    dest: Reg,
    value: Reg,
    target: mojito_types::ct::ScalarConversion,
    errors: &mut Vec<String>,
) {
    let prefix = cx.prefix;
    if let Some(found) = cx.reg_ty(value)
        && !target.casts_from(&held_simd_ty(found))
    {
        errors.push(format!(
            "{prefix}: scalar cast to {} of the non-scalar {found}",
            target.name()
        ));
    }
    if let Some(found) = cx.reg_ty(dest)
        && *found != target.ty()
    {
        errors.push(format!(
            "{prefix}: scalar cast to {} defines a register of type {found}",
            target.name()
        ));
    }
}

/// A `builtin.cast` retypes an `index` as its `Int` storage or back, so
/// exactly one side is an `index` and the other an `Int`.
/// `select`'s condition is a one-lane `bool` vector, and both cases are of
/// the destination's type.
fn verify_select(
    cx: &InstrCx<'_>,
    dest: Reg,
    condition: Reg,
    cases: [Reg; 2],
    errors: &mut Vec<String>,
) {
    let prefix = cx.prefix;
    if let Some(found) = cx.reg_ty(condition)
        && held_simd_ty(found)
            != mojito_types::types::canonical_simd_ty(mojito_ast::ast::Dtype::Bool, 1)
    {
        errors.push(format!(
            "{prefix}: select condition has type {found}, not a one-lane bool vector"
        ));
    }
    let Some(result) = cx.reg_ty(dest) else {
        return;
    };
    for (role, case) in ["true", "false"].into_iter().zip(cases) {
        if let Some(found) = cx.reg_ty(case)
            && held_simd_ty(found) != held_simd_ty(result)
        {
            errors.push(format!(
                "{prefix}: select {role} case has type {found}, expected {result}"
            ));
        }
    }
}

/// Upstream's `pop.array` verifiers: the value is the array's element
/// type, `array.replace` keeps its array's type, and a closed index is below
/// a known size. A template's open index or size is checked by the
/// instance that closes it.
fn verify_array_op(cx: &InstrCx<'_>, instruction: &MirInstr, errors: &mut Vec<String>) {
    let prefix = cx.prefix;
    let (op, array, value, index) = match instruction {
        MirInstr::ArrayRepeat { dest, value } => ("repeat", *dest, *value, None),
        MirInstr::ArrayGet { dest, array, index } => ("get", *array, *dest, Some(index)),
        MirInstr::ArrayReplace {
            dest,
            array,
            value,
            index,
        } => {
            if let (Some(result), Some(operand)) = (cx.reg_ty(*dest), cx.reg_ty(*array))
                && !types_compatible(result, operand)
            {
                errors.push(format!(
                    "{prefix}: array.replace result {result} is not its array's type {operand}"
                ));
            }
            ("replace", *array, *value, Some(index))
        }
        _ => return,
    };
    let Some(TyKind::PopArray { size, element }) = cx.reg_ty(array).map(Ty::kind) else {
        errors.push(format!("{prefix}: array.{op} over a non-array register"));
        return;
    };
    if let Some(found) = cx.reg_ty(value)
        && !types_compatible(found, element)
    {
        errors.push(format!(
            "{prefix}: array.{op} value has type {found}, not the element type {element}"
        ));
    }
    if let (Some(index), Some(size)) = (
        index.and_then(crate::mir::Const::index_attribute),
        size.known(),
    ) && u64::try_from(index).map_or(true, |index| index >= size)
    {
        errors.push(format!(
            "{prefix}: array.{op} index {index} is out of bounds of {size} elements"
        ));
    }
}

/// A `pointer.convert` retypes an address among a `Pointer`, a
/// `!kgen.pointer`, and a `!lit.ref` of one element; origins erase at the
/// waist, as they do for a `Pointer`.
fn verify_pointer_convert(cx: &InstrCx<'_>, dest: Reg, value: Reg, errors: &mut Vec<String>) {
    let (Some(to), Some(from)) = (cx.reg_ty(dest), cx.reg_ty(value)) else {
        return;
    };
    let element = |ty: &Ty| match ty.kind() {
        TyKind::Pointer { element, .. }
        | TyKind::KgenPointer(element)
        | TyKind::LitRef { element, .. } => Some(element.clone()),
        _ => None,
    };
    let (Some(to_element), Some(from_element)) = (element(&held_ty(to)), element(&held_ty(from)))
    else {
        errors.push(format!(
            "{}: pointer conversion from {from} to {to} is not among `Pointer`, \
             `!kgen.pointer`, and `!lit.ref`",
            cx.prefix
        ));
        return;
    };
    if !super::types::types_compatible(&from_element, &to_element) {
        errors.push(format!(
            "{}: pointer conversion from {from} to {to} changes the element type",
            cx.prefix
        ));
    }
}

fn verify_builtin_cast(cx: &InstrCx<'_>, dest: Reg, value: Reg, errors: &mut Vec<String>) {
    let (Some(to), Some(from)) = (cx.reg_ty(dest), cx.reg_ty(value)) else {
        return;
    };
    let (to, from) = (&held_ty(to), &held_ty(from));
    let pairs = [
        (Ty::index(), Ty::int()),
        (
            Ty::bool(),
            mojito_types::types::canonical_simd_ty(mojito_ast::ast::Dtype::Bool, 1),
        ),
    ];
    if !pairs.iter().any(|(builtin, storage)| {
        (from == builtin && to == storage) || (from == storage && to == builtin)
    }) {
        errors.push(format!(
            "{}: builtin cast from {from} to {to} is not between `index` and `Int` \
             or `i1` and `Scalar[DType.bool]`",
            cx.prefix
        ));
    }
}
