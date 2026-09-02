//! The per-instruction verifier.

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
    let reg_ty = |register: &Reg| function.reg_types.get(&register.0);
    let valid_simd_width = |width: usize| width >= 1 && (width & (width - 1)) == 0;
    match instruction {
        // Widths are validated during checked elaboration; this is the
        // phase-boundary backstop for assembled artifacts.
        MirInstr::MakeSimd { width, .. } | MirInstr::SimdCast { width, .. } => {
            if !valid_simd_width(*width) {
                errors.push(format!(
                    "{prefix}: SIMD width {width} is not a positive power of two"
                ));
            }
        }
        MirInstr::SimdShuffle { value, mask, .. } => {
            if !valid_simd_width(mask.len()) {
                errors.push(format!(
                    "{prefix}: SIMD shuffle mask length {} is not a positive power of two",
                    mask.len()
                ));
            }
            if let Some(Ty::Simd { width, .. }) = reg_ty(value)
                && let Some(bad) = mask.iter().find(|lane| **lane as i64 >= *width)
            {
                errors.push(format!(
                    "{prefix}: SIMD shuffle lane {bad} is out of range for width {width}"
                ));
            }
        }
        MirInstr::EstablishLoans {
            reference,
            loans,
            dest_interior,
            ..
        } => {
            if *reference as usize >= function.n_vars {
                errors.push(format!(
                    "{prefix}: loan generation has invalid reference slot {reference}"
                ));
            }
            if loans.is_empty() {
                errors.push(format!("{prefix}: loan generation has no owner loans"));
            }
            if let Some(domain) = dest_interior {
                if domain.root != *reference {
                    errors.push(format!(
                        "{prefix}: loan destination domain roots at slot {} instead of the \
                         generation's reference slot {reference}",
                        domain.root
                    ));
                }
                if domain.path.is_empty() {
                    errors.push(format!(
                        "{prefix}: loan destination domain has an empty interior path"
                    ));
                }
                // Transfer destinations name exact interior generations; the
                // conservative subtree form never designates a store target.
                if domain
                    .path
                    .iter()
                    .any(|segment| matches!(segment, crate::origin::OriginSeg::Subtree))
                {
                    errors.push(format!(
                        "{prefix}: loan destination domain contains a subtree segment"
                    ));
                }
            }
            for loan in loans {
                let through_capability = loan
                    .place
                    .through
                    .and_then(|through| function.var_tys.get(&through))
                    .and_then(reference_capability);
                let place_capability = make_ref_target(&loan.place);
                let permission = through_capability
                    .map(|capability| capability.permission)
                    .or_else(|| place_capability.and_then(|(_, permission)| permission));
                if loan.mutable
                    && permission.is_some_and(|permission| {
                        !permission.satisfies(ReferencePermission::Mutable)
                    })
                {
                    errors.push(format!(
                        "{prefix}: mutable loan recovers permission unavailable through its source capability"
                    ));
                }
                if let Some(capability) = through_capability
                    && let Some(root) = loan.place.root_ty.as_ref()
                    && let Some(target) = reference_capability(root)
                        .map(|root| root.target)
                        .or(Some(root))
                    && !types_compatible(capability.target, target)
                {
                    errors.push(format!(
                        "{prefix}: loan place root type {target} is incompatible with its through-reference capability target {}",
                        capability.target
                    ));
                }
                let Some(origin) = &loan.interior else {
                    continue;
                };
                if origin.root as usize >= function.n_vars {
                    errors.push(format!(
                        "{prefix}: interior loan has invalid root slot {}",
                        origin.root
                    ));
                }
                if origin.root != loan.place.root && loan.place.through.is_none() {
                    errors.push(format!(
                        "{prefix}: interior loan origin roots at slot {}, but its executable place roots at slot {}",
                        origin.root, loan.place.root
                    ));
                }
                // A domain loan names either a named interior generation or
                // the conservative subtree form; subtree is terminal.
                if let Some(position) = origin
                    .path
                    .iter()
                    .position(|segment| matches!(segment, crate::origin::OriginSeg::Subtree))
                    && position != origin.path.len() - 1
                {
                    errors.push(format!(
                        "{prefix}: interior loan origin rooted at slot {} has a non-terminal \
                         subtree segment",
                        origin.root
                    ));
                }
                if !origin.path.iter().any(|segment| {
                    matches!(
                        segment,
                        crate::origin::OriginSeg::Interior(_) | crate::origin::OriginSeg::Subtree
                    )
                }) {
                    errors.push(format!(
                        "{prefix}: interior loan origin rooted at slot {} has no interior segment",
                        origin.root
                    ));
                }
            }
        }
        MirInstr::InvalidateInteriors {
            base,
            except,
            include_base_generation,
            ..
        } => {
            if base.root as usize >= function.n_vars {
                errors.push(format!(
                    "{prefix}: interior invalidation has invalid root slot {}",
                    base.root
                ));
            }
            if let Some(reference) = except
                && *reference as usize >= function.n_vars
            {
                errors.push(format!(
                    "{prefix}: interior invalidation exception has invalid reference slot {reference}"
                ));
            }
            if *include_base_generation
                && !base
                    .path
                    .iter()
                    .any(|segment| matches!(segment, crate::origin::OriginSeg::Interior(_)))
            {
                errors.push(format!(
                    "{prefix}: inclusive interior invalidation has no named interior generation"
                ));
            }
        }
        MirInstr::MakeRef { dest, place } => {
            if let Some(destination) = reg_ty(dest) {
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
            if let Some(source) = reg_ty(reference) {
                let Some(capability) = reference_capability(source) else {
                    errors.push(format!(
                        "{prefix}: ReadRef source has non-reference-capability type {source}"
                    ));
                    return;
                };
                if let Some(destination) = reg_ty(dest)
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
            if let Some(source) = reg_ty(reference) {
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
                if let Some(value) = reg_ty(value)
                    && !types_compatible(value, capability.target)
                {
                    errors.push(format!(
                        "{prefix}: WriteRef value type {value} is incompatible with referent {}",
                        capability.target
                    ));
                }
            }
        }
        MirInstr::MaterializeLiteral { value, target, .. } => {
            let valid_target = matches!(target, Ty::Int | Ty::UInt | Ty::Float64)
                || matches!(target, Ty::Simd { width: 1, .. });
            if !valid_target {
                errors.push(format!(
                    "{prefix}: literal materialization has non-scalar target {target}"
                ));
            }
            if let Some(found) = reg_ty(value) {
                let valid_source = match found {
                    Ty::IntLiteral => matches!(
                        target,
                        Ty::Int | Ty::UInt | Ty::Float64 | Ty::Simd { width: 1, .. }
                    ),
                    Ty::FloatLiteral => {
                        matches!(target, Ty::Float64)
                            || matches!(target, Ty::Simd { dtype, width: 1 } if dtype.is_float())
                    }
                    _ => false,
                };
                if !valid_source {
                    errors.push(format!("{prefix}: cannot materialize {found} as {target}"));
                }
            }
        }
        MirInstr::ConstructTypeParam { dest, param } => {
            if let Some(found) = reg_ty(dest)
                && !matches!(found, Ty::Param { name, .. } if name == param)
                && !matches!(found, Ty::Struct(..))
            {
                errors.push(format!(
                    "{prefix}: type-parameter construction of '{param}' has result type {found}"
                ));
            }
        }
        MirInstr::SizeOf { dest, ty } => {
            if let Some(found) = reg_ty(dest)
                && found != &Ty::Int
            {
                errors.push(format!(
                    "{prefix}: size_of result register has type {found}, expected Int"
                ));
            }
            let target = crate::native::target::NativeTarget::new(
                crate::native::target::Triple::X86_64UnknownLinuxGnu,
            );
            let structs = crate::native::layout::StructFieldIndex::from_declarations(declarations);
            if let Err(error) = (crate::native::layout::LayoutCx {
                target: &target,
                structs: &structs,
            })
            .layout_of(ty)
            {
                errors.push(format!("{prefix}: size_of has no layout for {ty}: {error}"));
            }
        }
        MirInstr::CopyValue { dest, value } => {
            if let (Some(found), Some(expected)) = (reg_ty(value), reg_ty(dest))
                && found != expected
            {
                errors.push(format!(
                    "{prefix}: copied value has type {found}, destination has type {expected}"
                ));
            }
        }
        MirInstr::GetIter {
            source,
            dest,
            mode,
            prepare,
        } => {
            if *source as usize >= function.n_vars {
                errors.push(format!(
                    "{prefix}: GetIter uses invalid source slot {source}"
                ));
            }
            if *dest as usize >= function.n_vars {
                errors.push(format!(
                    "{prefix}: GetIter uses invalid destination slot {dest}"
                ));
            }
            let Some(first) = prepare.first() else {
                return;
            };
            let convention_matches = |convention| match mode {
                crate::checked::IterationMode::Borrowed => matches!(
                    convention,
                    None | Some(crate::ast::ArgConvention::Imm)
                        | Some(crate::ast::ArgConvention::Ref)
                ),
                crate::checked::IterationMode::Owned => {
                    convention == Some(crate::ast::ArgConvention::Var)
                }
            };
            match declared(declarations, first) {
                Some(declaration) => {
                    if !declaration.has_receiver || !declaration.param_types.is_empty() {
                        errors.push(format!(
                            "{prefix}: GetIter preparation method '{first}' is not a nullary receiver operation"
                        ));
                    }
                    if !convention_matches(declaration.receiver_convention) {
                        errors.push(format!(
                            "{prefix}: GetIter {mode:?} mode does not match preparation method '{first}' receiver convention {:?}",
                            declaration.receiver_convention
                        ));
                    }
                }
                None => {
                    let matches_dispatch = match mode {
                        crate::checked::IterationMode::Borrowed => {
                            first
                                == &crate::symbol::iterator_dispatch_symbol(
                                    crate::ast::ArgConvention::Imm,
                                )
                                || first
                                    == &crate::symbol::iterator_dispatch_symbol(
                                        crate::ast::ArgConvention::Ref,
                                    )
                        }
                        crate::checked::IterationMode::Owned => {
                            first
                                == &crate::symbol::iterator_dispatch_symbol(
                                    crate::ast::ArgConvention::Var,
                                )
                        }
                    };
                    if !matches_dispatch {
                        errors.push(format!(
                            "{prefix}: GetIter refers to undeclared preparation method '{first}'"
                        ));
                    }
                }
            }
        }
        MirInstr::Next { dest, iter, call } => {
            if *iter as usize >= function.n_vars {
                errors.push(format!("{prefix}: Next uses invalid iterator slot {iter}"));
            }
            let Some(call) = call else {
                return;
            };
            if reg_ty(dest) != Some(&call.result_ty) {
                errors.push(format!(
                    "{prefix}: Next result does not match its checked type {}",
                    call.result_ty
                ));
            }
            if call.raises.is_some() {
                errors.push(format!(
                    "{prefix}: bounded Next carries a raising iterator contract"
                ));
            }
            if verify_iterator_result_adapter(&prefix, call, errors) {
                return;
            }
            match declared(declarations, &call.target) {
                None => errors.push(format!(
                    "{prefix}: Next refers to undeclared iterator method '{}'",
                    call.target
                )),
                Some(declaration) => {
                    if !declaration.param_types.is_empty()
                        || declaration.receiver_convention != Some(crate::ast::ArgConvention::Mut)
                    {
                        errors.push(format!(
                            "{prefix}: Next method '{}' is not a nullary 'mut self' operation",
                            call.target
                        ));
                    }
                    if declaration.raises {
                        errors.push(format!(
                            "{prefix}: bounded Next method '{}' unexpectedly raises",
                            call.target
                        ));
                    }
                    if !iterator_result_matches_declaration(call, declaration) {
                        errors.push(format!(
                            "{prefix}: Next result contract does not match '{}'",
                            call.target
                        ));
                    }
                }
            }
        }
        MirInstr::TryNext {
            dest,
            yielded,
            iter,
            call,
            exhaustion,
        } => {
            if *iter as usize >= function.n_vars {
                errors.push(format!(
                    "{prefix}: TryNext uses invalid iterator slot {iter}"
                ));
            }
            if reg_ty(yielded).is_some_and(|ty| *ty != Ty::Bool) {
                errors.push(format!(
                    "{prefix}: TryNext yielded flag has non-Bool type {}",
                    reg_ty(yielded).expect("checked above")
                ));
            }
            let is_stop_iteration = matches!(
                exhaustion,
                Ty::Struct(name, arguments)
                    if arguments.is_empty()
                        && (name == "StopIteration" || name.ends_with("$StopIteration"))
            );
            if !is_stop_iteration {
                errors.push(format!(
                    "{prefix}: TryNext catches non-StopIteration type {exhaustion}"
                ));
            }
            if reg_ty(dest) != Some(&call.result_ty) {
                errors.push(format!(
                    "{prefix}: TryNext result does not match its checked type {}",
                    call.result_ty
                ));
            }
            if call.raises.as_ref() != Some(exhaustion) {
                errors.push(format!(
                    "{prefix}: TryNext exhaustion type {exhaustion} does not match its checked call effect"
                ));
            }
            if verify_iterator_result_adapter(&prefix, call, errors) {
                return;
            }
            match declared(declarations, &call.target) {
                None => errors.push(format!(
                    "{prefix}: TryNext refers to undeclared iterator method '{}'",
                    call.target
                )),
                Some(declaration) => {
                    if !declaration.param_types.is_empty()
                        || declaration.receiver_convention != Some(crate::ast::ArgConvention::Mut)
                    {
                        errors.push(format!(
                            "{prefix}: TryNext method '{}' is not a nullary 'mut self' operation",
                            call.target
                        ));
                    }
                    if !declaration.raises || declaration.error_ty.as_ref() != Some(exhaustion) {
                        errors.push(format!(
                            "{prefix}: TryNext exhaustion type {exhaustion} does not match '{}' raising contract",
                            call.target
                        ));
                    }
                    if !iterator_result_matches_declaration(call, declaration) {
                        errors.push(format!(
                            "{prefix}: TryNext result contract does not match '{}'",
                            call.target
                        ));
                    }
                }
            }
        }
        MirInstr::Store { place, src } => {
            if let (Some(expected), Some(found)) = (place.ty.as_ref(), reg_ty(src)) {
                let target = match expected {
                    Ty::Ref(reference) => reference.referent.as_ref(),
                    other => other,
                };
                if !types_compatible(found, target) {
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
                if let Some(source) = reg_ty(reference) {
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
        MirInstr::Index {
            dest,
            base,
            index,
            base_place,
            index_place,
            call,
            intrinsic,
            ..
        } => {
            verify_subscript_receiver_place(&prefix, reg_ty(base), base_place.as_ref(), errors);
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
                let positional_places = if call.arguments.is_empty() {
                    &[][..]
                } else {
                    std::slice::from_ref(index_place)
                };
                let positional_types = if call.arguments.is_empty() {
                    Vec::new()
                } else {
                    vec![reg_ty(index).cloned()]
                };
                verify_subscript_call(
                    &prefix,
                    function,
                    declarations,
                    call,
                    SubscriptSources {
                        receiver_ty: reg_ty(base),
                        method: "__getitem__",
                        receiver_place: base_place.as_ref(),
                        positional_places,
                        keyword_places: &[],
                        positional_types: &positional_types,
                        keyword_types: &[],
                        dest: Some(*dest),
                    },
                    errors,
                );
            } else if let Some(intrinsic) = intrinsic {
                verify_intrinsic_index(
                    &prefix,
                    *intrinsic,
                    reg_ty(base),
                    reg_ty(index),
                    reg_ty(dest),
                    errors,
                );
            }
        }
        MirInstr::Slice {
            dest,
            object,
            kind,
            object_place,
            arg_places,
            call,
            intrinsic,
            ..
        } => {
            verify_subscript_receiver_place(&prefix, reg_ty(object), object_place.as_ref(), errors);
            if arg_places.len() != 1 {
                errors.push(format!(
                    "{prefix}: slice place metadata is not aligned with its descriptor argument"
                ));
            }
            if call.is_some() == intrinsic.is_some() {
                errors.push(format!(
                    "{prefix}: slice operation must carry exactly one checked-call or intrinsic dispatch"
                ));
            }
            if let Some(call) = call {
                verify_subscript_call(
                    &prefix,
                    function,
                    declarations,
                    call,
                    SubscriptSources {
                        receiver_ty: reg_ty(object),
                        method: "__getitem__",
                        receiver_place: object_place.as_ref(),
                        positional_places: arg_places,
                        keyword_places: &[],
                        positional_types: &[Some(slice_descriptor_ty(*kind))],
                        keyword_types: &[],
                        dest: Some(*dest),
                    },
                    errors,
                );
            } else if let Some(intrinsic) = intrinsic {
                verify_intrinsic_slice(&prefix, *intrinsic, reg_ty(object), reg_ty(dest), errors);
            }
        }
        MirInstr::MultiIndex {
            dest,
            object,
            args,
            object_place,
            arg_places,
            kwargs,
            kwarg_places,
            call,
        } => {
            verify_subscript_receiver_place(&prefix, reg_ty(object), object_place.as_ref(), errors);
            if arg_places.len() != args.len() || kwarg_places.len() != kwargs.len() {
                errors.push(format!(
                    "{prefix}: multi-subscript place metadata is not aligned with its arguments"
                ));
            }
            let positional_types = args
                .iter()
                .map(|argument| subscript_argument_ty(function, argument))
                .collect::<Vec<_>>();
            let keyword_types = kwargs
                .iter()
                .map(|(_, argument)| subscript_argument_ty(function, argument))
                .collect::<Vec<_>>();
            if let Some(call) = call {
                verify_subscript_call(
                    &prefix,
                    function,
                    declarations,
                    call,
                    SubscriptSources {
                        receiver_ty: reg_ty(object),
                        method: "__getitem__",
                        receiver_place: object_place.as_ref(),
                        positional_places: arg_places,
                        keyword_places: kwarg_places,
                        positional_types: &positional_types,
                        keyword_types: &keyword_types,
                        dest: Some(*dest),
                    },
                    errors,
                );
            } else {
                errors.push(format!(
                    "{prefix}: multi-index operation lacks a checked call contract"
                ));
            }
        }
        MirInstr::MultiSet {
            receiver,
            receiver_place,
            args,
            arg_places,
            value,
            value_place,
            value_keyword,
            call,
            ..
        } => {
            verify_subscript_receiver_place(
                &prefix,
                reg_ty(receiver),
                receiver_place.as_ref(),
                errors,
            );
            if arg_places.len() != args.len() {
                errors.push(format!(
                    "{prefix}: subscript-set place metadata is not aligned with its index arguments"
                ));
            }
            let mut positional = arg_places.clone();
            let mut positional_types = args
                .iter()
                .map(|argument| subscript_argument_ty(function, argument))
                .collect::<Vec<_>>();
            let keyword = if *value_keyword {
                vec![value_place.clone()]
            } else {
                positional.push(value_place.clone());
                positional_types.push(reg_ty(value).cloned());
                Vec::new()
            };
            let keyword_types = if *value_keyword {
                vec![reg_ty(value).cloned()]
            } else {
                Vec::new()
            };
            verify_subscript_call(
                &prefix,
                function,
                declarations,
                call,
                SubscriptSources {
                    receiver_ty: reg_ty(receiver),
                    method: "__setitem__",
                    receiver_place: receiver_place.as_ref(),
                    positional_places: &positional,
                    keyword_places: &keyword,
                    positional_types: &positional_types,
                    keyword_types: &keyword_types,
                    dest: None,
                },
                errors,
            );
        }
        MirInstr::DefVar {
            src,
            binding_ty: Some(expected),
            ..
        } => {
            if let Some(found) = reg_ty(src)
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
            } else if let Some(found) = reg_ty(value)
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
            match declared(declarations, target) {
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
            if let Some(found) = reg_ty(dest)
                && !matches!(found, Ty::Func { .. } | Ty::GenericFunc { .. })
            {
                errors.push(format!(
                    "{prefix}: closure result has non-callable type {found}"
                ));
            }
        }
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
            match reg_ty(pointer) {
                Some(Ty::Pointer {
                    element: actual,
                    origin: crate::origin::PointerOrigin::Untracked { mutable: true },
                }) if types_compatible(actual, element) => {}
                Some(found) => errors.push(format!(
                    "{prefix}: compiler-private pointer storage operation expects Pointer[{element}, MutUntrackedOrigin], got {found}"
                )),
                None => {}
            }
            if let Some(found) = reg_ty(index)
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
            if let Some(found) = reg_ty(dest)
                && !types_compatible(found, expected)
            {
                errors.push(format!(
                    "{prefix}: compiler-private pointer storage result has type {found}, expected {expected}"
                ));
            }
        }
        MirInstr::UninitStorage { dest, init } => {
            if let Some(found) = reg_ty(dest)
                && crate::types::uninit_storage_element(found).is_none()
            {
                errors.push(format!(
                    "{prefix}: inline uninit storage construction has type {found}, expected {}",
                    crate::types::UNINIT_STORAGE_TYPE_NAME
                ));
            }
            if let Some(init) = init
                && let Some(element) = reg_ty(dest).and_then(crate::types::uninit_storage_element)
                && let Some(found) = reg_ty(init)
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
            if let Some(found) = reg_ty(storage)
                && !crate::types::uninit_storage_element(found)
                    .is_some_and(|actual| types_compatible(actual, element))
            {
                errors.push(format!(
                    "{prefix}: inline uninit storage operation expects {}[{element}], got {found}",
                    crate::types::UNINIT_STORAGE_TYPE_NAME
                ));
            }
            let expected = if matches!(instruction, MirInstr::UninitStorageTake { .. }) {
                element
            } else {
                &Ty::None
            };
            if let Some(found) = reg_ty(dest)
                && !types_compatible(found, expected)
            {
                errors.push(format!(
                    "{prefix}: inline uninit storage result has type {found}, expected {expected}"
                ));
            }
        }
        MirInstr::Call {
            func: FuncRef(callee),
            args,
            kwargs,
            arg_places,
            kwarg_places,
            capture_accesses,
            param_arg_regs,
            ..
        } => {
            verify_capture_accesses(&prefix, function, capture_accesses, errors);
            if let Some(declaration) = declared(declarations, callee) {
                verify_direct_call(
                    &prefix,
                    function,
                    declaration,
                    args,
                    kwargs,
                    arg_places,
                    errors,
                );
                verify_param_arguments(
                    &prefix,
                    function,
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
            ..
        } => {
            verify_capture_accesses(&prefix, function, capture_accesses, errors);
            let abstract_value_next = callee.starts_with("__trait_dispatch.")
                && method == "__next__"
                && reference_result.is_none();
            if let Some(reference) = reference_result {
                let expected = Ty::Ref(reference.clone());
                if function.reg_types.get(&dest.0) != Some(&expected) {
                    errors.push(format!(
                        "{prefix}: method-call reference ABI {expected} does not match its destination type"
                    ));
                }
            }
            match result_adapter {
                Some(crate::checked::CheckedResultAdapter::CopyIteratorReference) => {
                    if !abstract_value_next {
                        errors.push(format!(
                            "{prefix}: copy-reference result adapter is not attached to an abstract value-returning __next__ call"
                        ));
                    }
                }
                None if abstract_value_next => errors.push(format!(
                    "{prefix}: abstract value-returning __next__ call lacks its copy-reference adapter"
                )),
                None => {}
            }
            if let Some(declaration) = declared(declarations, callee) {
                verify_direct_call(
                    &prefix,
                    function,
                    declaration,
                    args,
                    kwargs,
                    arg_places,
                    errors,
                );
                let result_abi_matches = match reference_result {
                    Some(reference) => {
                        declaration.returns_reference
                            && types_compatible(&reference.referent, &declaration.ret_ty)
                    }
                    None => {
                        !declaration.returns_reference
                            && function
                                .reg_types
                                .get(&dest.0)
                                .is_some_and(|result| types_compatible(result, &declaration.ret_ty))
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
                    &prefix,
                    function,
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
        MirInstr::CallIndirect {
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
        } => {
            verify_capture_accesses(&prefix, function, capture_accesses, errors);
            if arg_places.len() != args.len() || kwarg_places.len() != kwargs.len() {
                errors.push(format!(
                    "{prefix}: indirect-call place metadata is not aligned with its arguments"
                ));
            }
            let stored_contract = reg_ty(callee).and_then(crate::checker::callable_contract_ty);
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
                    &prefix, function, contract, raises, *dest, args, kwargs, arg_places, errors,
                );
            }
            let checked_decls = reg_ty(callee).and_then(generic_callable_decls);
            if let Some(checked_decls) = checked_decls {
                if checked_decls != param_decls {
                    errors.push(format!(
                        "{prefix}: indirect-call compile-time parameter metadata does not match its callable contract"
                    ));
                }
                verify_param_arguments(&prefix, function, checked_decls, param_arg_regs, errors);
            } else if !param_decls.is_empty() {
                errors.push(format!(
                    "{prefix}: nongeneric indirect call carries compile-time parameter metadata"
                ));
            }
            let nominal_name = match reg_ty(callee) {
                Some(Ty::Struct(name, _)) => Some(name.as_str()),
                _ => None,
            };
            if let Some(target) = resolved {
                if nominal_name.is_none()
                    && let Some(expected) =
                        reg_ty(callee).and_then(crate::checker::callable_contract_target)
                    && target != &expected
                {
                    errors.push(format!(
                        "{prefix}: indirect-call target '{target}' does not match callable contract '{expected}'"
                    ));
                }
                let is_call_target = target.rsplit_once('.').is_some_and(|(_, method)| {
                    method == "__call__" || crate::symbol::is_overload_of(method, "__call__")
                });
                if !is_call_target {
                    errors.push(format!(
                        "{prefix}: indirect-call target '{target}' is not a __call__ method"
                    ));
                }
                let concrete = nominal_name
                    .and_then(|name| crate::symbol::retarget_method_symbol(target, name))
                    .unwrap_or_else(|| target.clone());
                if let Some(declaration) = declared(declarations, &concrete) {
                    verify_direct_call(
                        &prefix,
                        function,
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
        MirInstr::Raise { .. } => {
            if !function.raises && !context.protected {
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
                function_len: context.function_len,
                in_try_region: true,
                protected: handler.is_some() || context.protected,
            };
            verify_blocks(name, function, declarations, body, &body_context, errors);
            for region in handler
                .iter()
                .map(|(_, blocks)| blocks)
                .chain(orelse.iter())
                .chain(finalbody.iter())
            {
                let region_context = RegionContext {
                    region_len: region.len(),
                    function_len: context.function_len,
                    in_try_region: true,
                    protected: context.protected,
                };
                verify_blocks(
                    name,
                    function,
                    declarations,
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
        && !function.raises
        && !context.protected
    {
        errors.push(format!(
            "{prefix}: unprotected raising call in nonraising function"
        ));
    }
}
