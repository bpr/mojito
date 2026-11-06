//! The per-instruction verifier.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_ast::ast::ArgConvention;
use mojito_types::types::TyKind;
use mojito_types::types::storage_ty;

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
        MirInstr::MakeRefPack { dest, elems } => verify_ref_pack_create(&cx, *dest, elems, errors),
        MirInstr::LoadIndirect { dest, pack } => verify_load_indirect(&cx, *dest, *pack, errors),
        MirInstr::ArrayRepeat { .. }
        | MirInstr::ArrayGet { .. }
        | MirInstr::ArrayReplace { .. } => {
            verify_array_op(&cx, instruction, errors);
        }
        MirInstr::ArrayGep { dest, array, index } => {
            verify_array_gep(&cx, *dest, *array, *index, errors);
        }
        MirInstr::CallLlvmIntrinsic { args, spread, .. } => {
            verify_llvm_intrinsic(&cx, args, *spread, errors);
        }
        MirInstr::MaterializeLiteral { .. }
        | MirInstr::ConstructTypeParam { .. }
        | MirInstr::ConstructType { .. }
        | MirInstr::ParamListAddress { .. }
        | MirInstr::StoreToMem { .. }
        | MirInstr::CopyValue { .. }
        | MirInstr::Rebind { .. }
        | MirInstr::DefVar { .. }
        | MirInstr::MakeStruct { .. }
        | MirInstr::MakeVariant { .. }
        | MirInstr::MakeClosure { .. }
        | MirInstr::BinOp { .. } => verify_value_instruction(&cx, instruction, errors),
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
        MirInstr::MakeTuple {
            elems,
            element_types: Some(element_types),
            ..
        } => verify_tuple_elements(&cx, elems, element_types, errors),
        // A parameter value constant is a closed vector, a struct or tuple
        // of such values, scalars, and strings, a string read at the
        // nominal `String`, a `UInt` read at `UInt` or its storage, or an
        // `index` read at `index`.
        MirInstr::Const {
            k: crate::mir::Const::Value(CtValue::UInt(_)),
            dest,
        } => {
            if let Some(ty) = cx.reg_ty(*dest)
                && !mojito_types::types::storage_value_ty(ty).is_uint()
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
            k: crate::mir::Const::Value(value @ CtValue::Generator(_)),
            dest,
        } => {
            if let Some(ty) = cx.reg_ty(*dest)
                && !matches!(ty.kind(), TyKind::Generator(_))
            {
                errors.push(format!(
                    "{}: generator constant `{value}` is read at `{ty}`, not a generator type",
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

/// The slots of a builtin vector instruction's `register`, which upstream's
/// `pop` vector ops verify as a `!pop.simd` value: a `SIMD` struct or any
/// other type is a finding.
fn storage_slots(
    cx: &InstrCx<'_>,
    register: Reg,
    mnemonic: &str,
    role: &str,
    errors: &mut Vec<String>,
) -> Option<(
    mojito_types::types::SimdDtype,
    mojito_types::types::SimdWidth,
)> {
    let ty = cx.reg_ty(register)?;
    let slots = mojito_types::types::kgen_simd_slots(ty.kind());
    if slots.is_none() {
        errors.push(non_storage_finding(cx.prefix, mnemonic, role, ty));
    }
    slots
}

/// A builtin vector instruction's `role` register of the type `ty`, which
/// is not `!kgen.simd` storage.
fn non_storage_finding(prefix: &str, mnemonic: &str, role: &str, ty: &Ty) -> String {
    if ty.kind().as_simd().is_some() {
        format!("{prefix}: {mnemonic} {role} is the struct {ty}, not its !kgen.simd storage")
    } else {
        format!("{prefix}: {mnemonic} {role} has the non-SIMD type {ty}")
    }
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
            // Upstream's `#pop.int_literal_convert` and
            // `float_literal_convert` are typed at the `!pop.simd` storage;
            // a `SIMD` value is a `struct.create` over the conversion.
            let Some(target) = &mojito_types::types::storage_simd(target) else {
                errors.push(format!(
                    "{prefix}: literal materialization targets {target}, not a `!kgen.simd` storage"
                ));
                return;
            };
            let scalar_target = target.is_int()
                || target.is_uint()
                || target.is_float64()
                || mojito_types::types::is_scalar_simd(target);
            // A vector of any other width, known or symbolic, splats the
            // converted lane across the storage.
            let vector_lane = target
                .as_simd()
                .filter(|(_, width)| width.known() != Some(1))
                .map(|(dtype, _)| dtype);
            if !scalar_target && vector_lane.is_none() {
                errors.push(format!(
                    "{prefix}: literal materialization has non-SIMD target {target}"
                ));
            }
            // A target whose lane dtype is still a parameter expression takes
            // either literal; the instance's dtype decides the value.
            let symbolic_lane = target
                .as_simd()
                .is_some_and(|(dtype, _)| dtype.is_symbolic());
            if let Some(found) = cx.reg_ty(*value) {
                let valid_source = match found.kind() {
                    kind if kind.is_int_literal() && scalar_target => true,
                    kind if kind.is_float_literal() && scalar_target => {
                        target.is_float64()
                            || symbolic_lane
                            || mojito_types::types::scalar_simd_dtype(target)
                                .is_some_and(mojito_ast::ast::Dtype::is_float)
                    }
                    kind if kind.is_numeric_literal() => vector_lane
                        .as_ref()
                        .is_some_and(|lane| mojito_types::types::splats_to(kind, lane)),
                    _ => false,
                };
                // The pin's literal conversion is a constrained `__init__`
                // call, judged only in the instance that reaches it.
                if !valid_source && !cx.context.judged_per_instance() {
                    errors.push(format!("{prefix}: cannot materialize {found} as {target}"));
                }
            }
        }
        // Upstream's literal operators are `IntLiteral`/`FloatLiteral`
        // dunders over two literals; a literal beside any other operand
        // converts first (`literal.materialize`).
        MirInstr::BinOp { op, a, b, .. } => {
            if let (Some(left), Some(right)) = (cx.reg_ty(*a), cx.reg_ty(*b))
                && left.is_numeric_literal() != right.is_numeric_literal()
                && !contains_type_param(left)
                && !contains_type_param(right)
            {
                errors.push(format!(
                    "{prefix}: operator {op:?} takes a numeric literal beside {}; the literal \
                     converts first",
                    if left.is_numeric_literal() {
                        right
                    } else {
                        left
                    }
                ));
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
                && !found.kind().is_non_simd_struct()
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
                    if !(0..captures.len()).all(|index| capture_binds(declaration, index)) {
                        errors.push(format!(
                            "{prefix}: closure environment for '{target}' binds neither a leading reference parameter nor a leading `read` one"
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
                && (found.is_int_literal() || !types_compatible(found, &Ty::int()))
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
        MirInstr::MakeSimd {
            dest, dtype, width, ..
        }
        | MirInstr::SimdCast {
            dest, dtype, width, ..
        }
        | MirInstr::SimdBitcast {
            dest, dtype, width, ..
        } => {
            if let Some(width) = width.known()
                && !usize::try_from(width).is_ok_and(valid_simd_width)
            {
                errors.push(format!(
                    "{prefix}: SIMD width {width} is not a positive power of two"
                ));
            }
            let mnemonic = crate::mir::text::instruction_mnemonic(instruction);
            if let Some((found_dtype, found_width)) =
                storage_slots(cx, *dest, mnemonic, "result", errors)
                && let (Some(found_dtype), Some(expected_dtype)) =
                    (found_dtype.known(), dtype.known())
                && let (Some(found_width), Some(expected_width)) =
                    (found_width.known(), width.known())
                && (found_dtype, found_width) != (expected_dtype, expected_width)
            {
                errors.push(format!(
                    "{prefix}: {mnemonic} defines {}, not {}",
                    cx.reg_ty(*dest)
                        .map_or_else(String::new, ToString::to_string),
                    mojito_types::types::kgen_simd_ty(expected_dtype, expected_width)
                ));
            }
            match instruction {
                MirInstr::SimdCast { value, .. } | MirInstr::SimdBitcast { value, .. } => {
                    storage_slots(cx, *value, mnemonic, "operand", errors);
                }
                // An element is a lane scalar, as upstream's `pop.simd.splat`
                // takes one: a literal reaches it through `literal.materialize`.
                MirInstr::MakeSimd { elems, .. } => {
                    for element in elems {
                        match cx.reg_ty(*element) {
                            Some(ty) if ty.kind().as_simd().is_some() => {
                                errors.push(non_storage_finding(prefix, mnemonic, "element", ty));
                            }
                            Some(ty) if ty.is_numeric_literal() => errors.push(format!(
                                "{prefix}: {mnemonic} element r{} is the literal {ty}, \
                                 which no conversion materialized",
                                element.0
                            )),
                            _ => {}
                        }
                    }
                }
                _ => {}
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
                if let Some((_, width)) = storage_slots(cx, *dest, "simd.shuffle", "result", errors)
                    && let Some(width) = width.known()
                    && width != mask.len() as i64
                {
                    errors.push(format!(
                        "{prefix}: SIMD shuffle result has {width} lanes, its mask {}",
                        mask.len()
                    ));
                }
                if let Some((_, width)) =
                    storage_slots(cx, *value, "simd.shuffle", "vector", errors)
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

/// The lane instructions, where their registers are typed: a `simd.select`
/// takes a `bool` mask and cases of its result type at the mask's width; a
/// `simd.extract` and a `simd.reduce` give one lane of the vector's dtype,
/// a `simd.insert` takes one (a `Bool` for `bool` lanes) and gives the
/// vector's type; and a lane index
/// is an `Int`, a `UInt`, or an `index`, a literal one converted first.
fn verify_simd_lane_instruction(
    cx: &InstrCx<'_>,
    instruction: &MirInstr,
    errors: &mut Vec<String>,
) {
    let prefix = cx.prefix;
    let mnemonic = crate::mir::text::instruction_mnemonic(instruction);
    let mut expect_vector =
        |register: Reg, role: &str| storage_slots(cx, register, mnemonic, role, errors);
    let lane_of = |dtype: &mojito_types::types::SimdDtype| {
        Some(Ty::from(TyKind::KgenSimd {
            dtype: dtype.clone(),
            width: mojito_types::types::SimdWidth::Known(1),
        }))
    };
    let mut mismatches = Vec::new();
    let mut expect_type = |register: Reg, expected: Option<Ty>, role: &str| {
        if let (Some(found), Some(expected)) = (cx.reg_ty(register), expected)
            && *found != expected
        {
            mismatches.push(format!(
                "{prefix}: {mnemonic} {role} has type {found}, expected {expected}"
            ));
        }
    };
    let index_ok = |register: Reg| {
        cx.reg_ty(register).is_none_or(|ty| {
            types_compatible(ty, &Ty::int()) || ty.is_uint() || matches!(ty.kind(), TyKind::Index)
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
                    .map(|ty| mojito_types::types::kgen_simd_slots(ty.kind())),
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
            expect_type(element, lane, "element");
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
/// projection's do. A closed `SIMD` is its declared instance once the
/// elaborator has run ([`MirDeclarations::struct_instance`]).
fn verify_struct_creation(cx: &InstrCx<'_>, dest: Reg, fields: &[Reg], errors: &mut Vec<String>) {
    let prefix = cx.prefix;
    let declared = cx
        .reg_ty(dest)
        .and_then(|ty| cx.declarations.struct_instance(ty))
        .map(|(declaration, arguments)| (declaration, arguments.is_empty()));
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

/// Whether a closure's capture binds the leading parameter `index` of its
/// lifted body `declaration`: a reference parameter, or a `read` one. A
/// method's receiver binds no capture: a closure's storage reaches its
/// method as the receiver of a direct call, never as an environment.
fn capture_binds(declaration: &MirFunctionDeclaration, index: usize) -> bool {
    !declaration.has_receiver
        && (declaration.ref_params.get(index).copied().unwrap_or(false)
            || matches!(
                declaration.param_conventions.get(index),
                Some(None | Some(ArgConvention::Imm))
            ))
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
    if op.is_dtype_code() {
        let (operand, expected) = if op == mojito_types::mlir::PopOp::DtypeToUi8 {
            (Ty::kgen_dtype(), Ty::ui8())
        } else {
            (Ty::ui8(), Ty::kgen_dtype())
        };
        if *result != expected {
            errors.push(format!(
                "{prefix}: pop.{} defines a register of type {result}, not {expected}",
                op.mnemonic()
            ));
        }
        if let Some(found) = operands.first().and_then(|reg| cx.reg_ty(*reg))
            && *found != operand
        {
            errors.push(format!(
                "{prefix}: pop.{} reads an operand of type {found}, not {operand}",
                op.mnemonic()
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
    if mojito_types::types::kgen_simd_slots(result.kind()).is_none() && *result != Ty::index() {
        let mnemonic = format!("pop.{}", op.mnemonic());
        errors.push(non_storage_finding(prefix, &mnemonic, "result", result));
    }
    for operand in operands {
        if let Some(found) = cx.reg_ty(*operand)
            && found != result
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
        && !target.casts_from(found)
        && !matches!(
            mojito_types::types::kgen_simd_slots(found.kind()),
            Some((_, mojito_types::types::SimdWidth::Known(1)))
        )
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
        && storage_ty(found) != mojito_types::types::kgen_simd_ty(mojito_ast::ast::Dtype::Bool, 1)
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
            && storage_ty(found) != storage_ty(result)
        {
            errors.push(format!(
                "{prefix}: select {role} case has type {found}, expected {result}"
            ));
        }
    }
}

/// Upstream's `lit.ref.pack.create` verifier: the result is a reference
/// pack with one operand per element, each a reference to that element.
fn verify_ref_pack_create(cx: &InstrCx<'_>, dest: Reg, elems: &[Reg], errors: &mut Vec<String>) {
    let prefix = cx.prefix;
    let Some(TyKind::LitRefPack { elements, .. }) = cx.reg_ty(dest).map(Ty::kind) else {
        errors.push(format!(
            "{prefix}: lit.ref.pack.create result is not a reference pack"
        ));
        return;
    };
    if mojito_types::types::pack_spread(elements).is_some() {
        return;
    }
    if elements.len() != elems.len() {
        errors.push(format!(
            "{prefix}: lit.ref.pack.create passes {} operands for {} elements",
            elems.len(),
            elements.len()
        ));
        return;
    }
    for (index, (operand, element)) in elems.iter().zip(elements).enumerate() {
        match cx.reg_ty(*operand).map(Ty::kind) {
            Some(TyKind::Pointer { element: found, .. } | TyKind::LitRef { element: found, .. })
                if types_compatible(found, element) => {}
            Some(TyKind::Ref(reference)) if types_compatible(&reference.referent, element) => {}
            Some(found) => errors.push(format!(
                "{prefix}: lit.ref.pack.create operand {index} has type {found}, not a reference to {element}"
            )),
            None => errors.push(format!(
                "{prefix}: lit.ref.pack.create operand {index} has no type"
            )),
        }
    }
}

/// Upstream's `kgen.struct.load_indirect` verifier: the operand is a
/// reference pack and the result the literal struct of its elements.
fn verify_load_indirect(cx: &InstrCx<'_>, dest: Reg, pack: Reg, errors: &mut Vec<String>) {
    let prefix = cx.prefix;
    let Some(TyKind::LitRefPack { elements, .. }) = cx.reg_ty(pack).map(Ty::kind) else {
        errors.push(format!(
            "{prefix}: kgen.struct.load_indirect operand is not a reference pack"
        ));
        return;
    };
    let loaded: Ty = TyKind::Tuple(elements.clone()).into();
    if let Some(found) = cx.reg_ty(dest)
        && !types_compatible(found, &loaded)
    {
        errors.push(format!(
            "{prefix}: kgen.struct.load_indirect result has type {found}, not {loaded}"
        ));
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

/// Upstream's `pop.array.gep` verifier: the operand is a `!kgen.pointer`
/// to a `!pop.array`, the index an `index`, and the result a
/// `!kgen.pointer` to the array's element type.
fn verify_array_gep(cx: &InstrCx<'_>, dest: Reg, array: Reg, index: Reg, errors: &mut Vec<String>) {
    let prefix = cx.prefix;
    let element = cx.reg_ty(array).and_then(|pointer| match pointer.kind() {
        TyKind::KgenPointer(array) => match array.kind() {
            TyKind::PopArray { element, .. } => Some(element.clone()),
            _ => None,
        },
        _ => None,
    });
    let Some(element) = element else {
        errors.push(format!(
            "{prefix}: array.gep over a register that is no `!kgen.pointer` to a `!pop.array`"
        ));
        return;
    };
    if let Some(found) = cx.reg_ty(index)
        && !matches!(found.kind(), TyKind::Index)
    {
        errors.push(format!(
            "{prefix}: array.gep index has type {found}, not index"
        ));
    }
    if let Some(result) = cx.reg_ty(dest)
        && !matches!(result.kind(), TyKind::KgenPointer(found) if types_compatible(found, &element))
    {
        errors.push(format!(
            "{prefix}: array.gep result {result} is not a `!kgen.pointer` to its element type \
             {element}"
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
        // A `ref` value is the `!lit.ref` upstream holds it as.
        TyKind::Ref(reference) => Some(reference.referent.clone()),
        _ => None,
    };
    let (Some(to_element), Some(from_element)) = (element(to), element(from)) else {
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
    let (to, from) = (&storage_ty(to), &storage_ty(from));
    let pairs = [
        (Ty::index(), storage_ty(&Ty::int())),
        (
            Ty::i1(),
            mojito_types::types::kgen_simd_ty(mojito_ast::ast::Dtype::Bool, 1),
        ),
        (
            Ty::ui8(),
            mojito_types::types::kgen_simd_ty(mojito_ast::ast::Dtype::UInt8, 1),
        ),
    ];
    if !pairs.iter().any(|(builtin, storage)| {
        (from == builtin && to == storage) || (from == storage && to == builtin)
    }) {
        errors.push(format!(
            "{}: builtin cast from {from} to {to} is not between `index` and `Int`, \
             `i1` and `Scalar[DType.bool]`, or `ui8` and `Scalar[DType.uint8]`",
            cx.prefix
        ));
    }
}

/// Each element of a `tuple.make` reaches its element type converted, as
/// upstream's `Tuple.__init__(var *args: *Ts)` binds each at its `Ts`.
fn verify_tuple_elements(
    cx: &InstrCx<'_>,
    elems: &[Reg],
    element_types: &[Ty],
    errors: &mut Vec<String>,
) {
    for (element, expected) in elems.iter().zip(element_types) {
        if let Some(found) = cx.reg_ty(*element)
            && !types_compatible(found, expected)
        {
            errors.push(format!(
                "{}: tuple element %r{} has type {found}, declared {expected}",
                cx.prefix, element.0
            ));
        }
    }
}
