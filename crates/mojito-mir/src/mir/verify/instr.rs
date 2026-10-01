//! The per-instruction verifier.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

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
        MirInstr::EstablishLoans { .. } | MirInstr::InvalidateInteriors { .. } => {
            verify_loan_instruction(&cx, instruction, errors);
        }
        MirInstr::MakeRef { .. }
        | MirInstr::ReadRef { .. }
        | MirInstr::WriteRef { .. }
        | MirInstr::Store { .. }
        | MirInstr::StoreRef { .. } => verify_reference_instruction(&cx, instruction, errors),
        MirInstr::MaterializeLiteral { .. }
        | MirInstr::ConstructTypeParam { .. }
        | MirInstr::SizeOf { .. }
        | MirInstr::CopyValue { .. }
        | MirInstr::DefVar { .. }
        | MirInstr::MakeVariant { .. }
        | MirInstr::MakeClosure { .. } => verify_value_instruction(&cx, instruction, errors),
        MirInstr::GetIter { .. } | MirInstr::Next { .. } | MirInstr::TryNext { .. } => {
            verify_iteration_instruction(&cx, instruction, errors);
        }
        MirInstr::Index { .. }
        | MirInstr::Slice { .. }
        | MirInstr::MultiIndex { .. }
        | MirInstr::MultiSet { .. } => verify_subscript_instruction(&cx, instruction, errors),
        MirInstr::PointerStorageTake { .. }
        | MirInstr::PointerStorageDestroy { .. }
        | MirInstr::UninitStorage { .. }
        | MirInstr::UninitStorageTake { .. }
        | MirInstr::UninitStorageDestroy { .. } => {
            verify_storage_instruction(&cx, instruction, errors);
        }
        MirInstr::Call { .. } | MirInstr::MethodCall { .. } | MirInstr::CallIndirect { .. } => {
            verify_call_instruction(&cx, instruction, errors);
        }
        _ => {}
    }
    verify_effect_instruction(&cx, instruction, errors);
}

/// A raising site in a nonraising function must sit under a handler; a `try`
/// verifies each of its regions under that region's protection.
fn verify_effect_instruction(cx: &InstrCx<'_>, instruction: &MirInstr, errors: &mut Vec<String>) {
    let prefix = cx.prefix;
    match instruction {
        MirInstr::Raise { .. } => {
            if !cx.function.raises && !cx.context.protected {
                errors.push(format!(
                    "{prefix}: unprotected raise in nonraising function"
                ));
            }
        }
        MirInstr::Try {
            body,
            handler,
            orelse,
            finalbody,
            ..
        } => {
            let body_context = RegionContext {
                region_len: body.len(),
                function_len: cx.context.function_len,
                in_try_region: true,
                protected: handler.is_some() || cx.context.protected,
            };
            verify_blocks(
                cx.name,
                cx.function,
                cx.declarations,
                body,
                &body_context,
                errors,
            );
            for region in handler
                .iter()
                .map(|(_, blocks)| blocks)
                .chain(orelse.iter())
                .chain(finalbody.iter())
            {
                let region_context = RegionContext {
                    region_len: region.len(),
                    function_len: cx.context.function_len,
                    in_try_region: true,
                    protected: cx.context.protected,
                };
                verify_blocks(
                    cx.name,
                    cx.function,
                    cx.declarations,
                    region,
                    &region_context,
                    errors,
                );
            }
        }
        _ => {}
    }
    // A call carrying a checked error contract raises unless handled.
    if let MirInstr::Call {
        raises: Some(_), ..
    }
    | MirInstr::CallIndirect {
        raises: Some(_), ..
    }
    | MirInstr::MethodCall {
        raises: Some(_), ..
    }
    | MirInstr::Index {
        call: Some(crate::mir::MirSubscriptCall {
            raises: Some(_), ..
        }),
        ..
    }
    | MirInstr::Slice {
        call: Some(crate::mir::MirSubscriptCall {
            raises: Some(_), ..
        }),
        ..
    }
    | MirInstr::MultiIndex {
        call: Some(crate::mir::MirSubscriptCall {
            raises: Some(_), ..
        }),
        ..
    }
    | MirInstr::MultiSet {
        call: crate::mir::MirSubscriptCall {
            raises: Some(_), ..
        },
        ..
    } = instruction
        && !cx.function.raises
        && !cx.context.protected
    {
        errors.push(format!(
            "{prefix}: unprotected raising call in nonraising function"
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
                    let forwarded = match place.ty.as_ref() {
                        Some(Ty::Ref(stored))
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
                let target = match expected {
                    Ty::Ref(reference) => reference.referent.as_ref(),
                    other => other,
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
                let Ty::Ref(storage_reference) = storage else {
                    errors.push(format!(
                        "{prefix}: StoreRef into non-reference storage of type {storage}"
                    ));
                    return;
                };
                if let Some(source) = cx.reg_ty(*reference) {
                    let Ty::Ref(source_reference) = source else {
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
            let valid_target = matches!(target, Ty::Int | Ty::UInt | Ty::Float64)
                || mojito_types::types::is_scalar_simd(target);
            if !valid_target {
                errors.push(format!(
                    "{prefix}: literal materialization has non-scalar target {target}"
                ));
            }
            if let Some(found) = cx.reg_ty(*value) {
                let valid_source = match found {
                    Ty::IntLiteral => valid_target,
                    Ty::FloatLiteral => {
                        matches!(target, Ty::Float64)
                            || mojito_types::types::scalar_simd_dtype(target)
                                .is_some_and(mojito_ast::ast::Dtype::is_float)
                    }
                    _ => false,
                };
                if !valid_source {
                    errors.push(format!("{prefix}: cannot materialize {found} as {target}"));
                }
            }
        }
        MirInstr::ConstructTypeParam { dest, param } => {
            if let Some(found) = cx.reg_ty(*dest)
                && !matches!(found, Ty::Param { binder, .. } if binder == param)
                && !matches!(found, Ty::Struct(..))
            {
                errors.push(format!(
                    "{prefix}: type-parameter construction of '{}' has result type {found}",
                    param.name
                ));
            }
        }
        MirInstr::SizeOf { dest, ty } => {
            if let Some(found) = cx.reg_ty(*dest)
                && found != &Ty::Int
            {
                errors.push(format!(
                    "{prefix}: size_of result register has type {found}, expected Int"
                ));
            }
            let target = mojito_native_core::target::NativeTarget::new(
                mojito_native_core::target::Triple::X86_64UnknownLinuxGnu,
            );
            let structs = crate::mir::struct_field_index(cx.declarations);
            if let Err(error) = (mojito_native_core::layout::LayoutCx {
                target: &target,
                structs: &structs,
            })
            .layout_of(ty)
            {
                errors.push(format!("{prefix}: size_of has no layout for {ty}: {error}"));
            }
        }
        MirInstr::CopyValue { dest, value } => {
            if let (Some(found), Some(expected)) = (cx.reg_ty(*value), cx.reg_ty(*dest))
                && !types_compatible(found, expected)
            {
                errors.push(format!(
                    "{prefix}: copied value has type {found}, destination has type {expected}"
                ));
            }
        }
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
        MirInstr::MakeVariant {
            alternatives,
            index,
            value,
            ..
        } => {
            if *index >= alternatives.len() {
                errors.push(format!(
                    "{prefix}: variant construction index {index} out of {} alternatives",
                    alternatives.len()
                ));
            } else if let Some(found) = cx.reg_ty(*value)
                && !types_compatible(found, &alternatives[*index])
            {
                errors.push(format!(
                    "{prefix}: variant payload {found} does not fit alternative {}",
                    alternatives[*index]
                ));
            }
        }
        MirInstr::MakeClosure {
            dest,
            function: target,
            captures,
        } => {
            match declared(cx.declarations, target) {
                None => errors.push(format!(
                    "{prefix}: closure refers to undeclared lifted function '{target}'"
                )),
                Some(declaration) => {
                    if captures.len() > declaration.param_types.len() {
                        errors.push(format!(
                            "{prefix}: closure has {} captures for '{}' with only {} parameters",
                            captures.len(),
                            target,
                            declaration.param_types.len()
                        ));
                    }
                    if declaration
                        .ref_params
                        .iter()
                        .take(captures.len())
                        .any(|is_reference| !is_reference)
                    {
                        errors.push(format!(
                            "{prefix}: closure environment for '{target}' is not a reference-parameter prefix"
                        ));
                    }
                }
            }
            if let Some(found) = cx.reg_ty(*dest)
                && !matches!(found, Ty::Func { .. } | Ty::GenericFunc { .. })
            {
                errors.push(format!(
                    "{prefix}: closure result has non-callable type {found}"
                ));
            }
        }
        _ => {}
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
        } => {
            match cx.reg_ty(*pointer) {
                Some(Ty::Pointer {
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
                && !types_compatible(found, &Ty::Int)
            {
                errors.push(format!(
                    "{prefix}: compiler-private pointer storage index has type {found}, expected Int"
                ));
            }
            let expected = if matches!(instruction, MirInstr::PointerStorageTake { .. }) {
                element
            } else {
                &Ty::None
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
        } => {
            if let Some(found) = cx.reg_ty(*storage)
                && !mojito_types::types::uninit_storage_element(found)
                    .is_some_and(|actual| types_compatible(actual, element))
            {
                errors.push(format!(
                    "{prefix}: inline uninit storage operation expects {}[{element}], got {found}",
                    mojito_types::types::UNINIT_STORAGE_TYPE_NAME
                ));
            }
            let expected = if matches!(instruction, MirInstr::UninitStorageTake { .. }) {
                element
            } else {
                &Ty::None
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
        // phase-boundary backstop for assembled artifacts.
        MirInstr::MakeSimd { width, .. }
        | MirInstr::SimdCast { width, .. }
        | MirInstr::SimdBitcast { width, .. } => {
            if !valid_simd_width(*width) {
                errors.push(format!(
                    "{prefix}: SIMD width {width} is not a positive power of two"
                ));
            }
        }
        MirInstr::SimdShuffle {
            value, other, mask, ..
        } => {
            if !valid_simd_width(mask.len()) {
                errors.push(format!(
                    "{prefix}: SIMD shuffle mask length {} is not a positive power of two",
                    mask.len()
                ));
            }
            if let Some(source @ Ty::Simd { width, .. }) = cx.reg_ty(*value)
                && let Some(width) = width.known()
            {
                let lanes = if other.is_some() { width * 2 } else { width };
                if let Some(bad) = mask.iter().find(|lane| **lane as i64 >= lanes) {
                    errors.push(format!(
                        "{prefix}: SIMD shuffle lane {bad} is out of range for {lanes} source lanes"
                    ));
                }
                if let Some(joined) = other.as_ref().and_then(|register| cx.reg_ty(*register))
                    && joined != source
                {
                    errors.push(format!(
                        "{prefix}: SIMD join operand {joined} does not match {source}"
                    ));
                }
            }
        }
        _ => {}
    }
}
