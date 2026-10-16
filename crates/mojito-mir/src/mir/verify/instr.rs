//! The per-instruction verifier.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::TyKind;

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
        | MirInstr::ConstructType { .. }
        | MirInstr::SizeOf { .. }
        | MirInstr::ParamListAddress { .. }
        | MirInstr::StoreToMem { .. }
        | MirInstr::TypeName { .. }
        | MirInstr::CopyValue { .. }
        | MirInstr::Rebind { .. }
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
        // A slice bound's read names only declared constructors.
        MirInstr::GetField { constructors, .. } => {
            for constructor in constructors {
                if cx.declarations.function(constructor).is_none() {
                    errors.push(format!(
                        "{}: field read names the undeclared constructor `{constructor}`",
                        cx.prefix
                    ));
                }
            }
        }
        MirInstr::DropVar { var, destructor } => verify_lifecycle_symbol(
            &cx,
            "drop",
            function.var_tys.get(var),
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
        // A parameter value constant is a closed vector, a struct or tuple
        // of such values, scalars, and strings, a string read at the
        // nominal `String`, or a `UInt` read at `UInt`.
        MirInstr::Const {
            k: crate::mir::Const::Value(CtValue::UInt(_)),
            dest,
        } => {
            if let Some(ty) = cx.reg_ty(*dest)
                && !matches!(ty.kind(), mojito_types::types::TyKind::UInt)
            {
                errors.push(format!(
                    "{}: `UInt` value constant is read at `{ty}`",
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
    match instruction {
        MirInstr::Raise { .. } => {
            if !cx.function.raises && !cx.context.protected {
                errors.push(format!(
                    "{prefix}: unprotected raise in nonraising function"
                ));
            }
        }
        _ => verify_regions(cx, instruction, errors),
    }
    // A call carrying a checked error contract raises unless handled.
    if instruction.call_error().is_some() && !cx.function.raises && !cx.context.protected {
        errors.push(format!(
            "{prefix}: unprotected raising call in nonraising function"
        ));
    }
}

/// A drop names the destructor its value's struct records, and a copy or
/// move the constructor it runs, the template member in a parametric body
/// and its instance in concrete MIR, and only a declared function: a pass
/// that makes a drop, a copy, or a move names it, as upstream's destructor
/// and copy/move insertion name the member at each site. `member` is the
/// lifecycle member the site runs, `None` for a use that runs none.
fn verify_lifecycle_symbol(
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
                matches!(target.kind(), TyKind::Int | TyKind::UInt | TyKind::Float64)
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
                        matches!(target.kind(), TyKind::Float64)
                            || symbolic_lane
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
                && found != &TyKind::Int
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
                && found != &TyKind::StringLiteral
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
        // Two closed types a rebind names must already agree; the
        // elaborator judges one over a parameter per instance.
        MirInstr::Rebind { dest, value } => {
            if let (Some(found), Some(expected)) = (cx.reg_ty(*value), cx.reg_ty(*dest))
                && !types_compatible(found, expected)
            {
                errors.push(format!(
                    "{prefix}: rebinds {found} to {expected}, which never agree"
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
                if let Some(TyKind::Simd { width, .. }) = cx.reg_ty(*dest).map(Ty::kind)
                    && let Some(width) = width.known()
                    && width != mask.len() as i64
                {
                    errors.push(format!(
                        "{prefix}: SIMD shuffle result has {width} lanes, its mask {}",
                        mask.len()
                    ));
                }
                if let Some(TyKind::Simd { width, .. }) = cx.reg_ty(*value).map(Ty::kind)
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
                && joined != source
            {
                errors.push(format!(
                    "{prefix}: SIMD join operand {joined} does not match {source}"
                ));
            }
        }
        _ => {}
    }
}
