//! Terminator and place verification.

use super::*;

pub(super) fn verify_terminator(
    name: &str,
    function: &MirFunction,
    block_index: usize,
    terminator: &MirTerm,
    context: &RegionContext,
    errors: &mut Vec<String>,
) {
    let prefix = format!("MIR function '{name}' block {block_index}");
    match terminator {
        MirTerm::Jump(target) => {
            if *target >= context.region_len {
                errors.push(format!("{prefix}: jump to invalid block {target}"));
            }
        }
        MirTerm::Branch {
            cond,
            then_b,
            else_b,
        } => {
            for target in [then_b, else_b] {
                if *target >= context.region_len {
                    errors.push(format!("{prefix}: branch to invalid block {target}"));
                }
            }
            if let Some(found) = function.reg_types.get(&cond.0)
                && *found != Ty::Bool
            {
                errors.push(format!("{prefix}: branch condition has type {found}"));
            }
        }
        MirTerm::Return(value) | MirTerm::ReturnWithCleanup { value, .. } => {
            // `Return(None)` doubles as the lowering placeholder terminator, so
            // only value-carrying returns are checked.
            if let Some(register) = value
                && !function.returns_reference
                && let (Some(found), Some(expected)) = (
                    function.reg_types.get(&register.0),
                    function.ret_ty.as_ref(),
                )
                && !types_compatible(found, expected)
            {
                errors.push(format!(
                    "{prefix}: return of {found} from a function returning {expected}"
                ));
            }
            if let MirTerm::ReturnWithCleanup { cleanup, .. } = terminator {
                for variable in cleanup {
                    if *variable as usize >= function.n_vars {
                        errors.push(format!(
                            "{prefix}: return cleanup uses invalid variable {variable}"
                        ));
                    }
                }
            }
        }
        MirTerm::FallOff => {
            if !context.in_try_region {
                errors.push(format!("{prefix}: FallOff terminator outside a try region"));
            }
        }
        MirTerm::EscapeJump { target, .. } => {
            if !context.in_try_region {
                errors.push(format!(
                    "{prefix}: EscapeJump terminator outside a try region"
                ));
            }
            if *target >= context.function_len {
                errors.push(format!("{prefix}: escape to invalid block {target}"));
            }
        }
    }
}

pub(super) fn instruction_places(instruction: &MirInstr) -> Vec<&MirPlace> {
    match instruction {
        MirInstr::EstablishLoans { loans, .. } => loans.iter().map(|loan| &loan.place).collect(),
        MirInstr::MakeRef { place, .. }
        | MirInstr::MovePlace { place, .. }
        | MirInstr::Store { place, .. }
        | MirInstr::StoreRef { place, .. }
        | MirInstr::LoadPlace { place, .. }
        | MirInstr::VariantSet { place, .. }
        | MirInstr::VariantSetInitWith { place, .. }
        | MirInstr::VariantReplace { place, .. }
        | MirInstr::ConsumePlace { place, .. } => vec![place],
        MirInstr::MakeClosure { captures, .. } => {
            captures.iter().map(|capture| &capture.place).collect()
        }
        MirInstr::Call {
            arg_places,
            kwarg_places,
            ..
        } => arg_places
            .iter()
            .flatten()
            .chain(kwarg_places.iter().flatten())
            .collect(),
        MirInstr::CallIndirect {
            callee_place,
            arg_places,
            kwarg_places,
            ..
        } => callee_place
            .iter()
            .chain(arg_places.iter().flatten())
            .chain(kwarg_places.iter().flatten())
            .collect(),
        MirInstr::MethodCall {
            recv_place,
            arg_places,
            kwarg_places,
            ..
        } => recv_place
            .iter()
            .chain(arg_places.iter().flatten())
            .chain(kwarg_places.iter().flatten())
            .collect(),
        MirInstr::Index {
            base_place,
            index_place,
            ..
        } => base_place.iter().chain(index_place.iter()).collect(),
        MirInstr::Slice {
            object_place,
            arg_places,
            ..
        }
        | MirInstr::MultiIndex {
            object_place,
            arg_places,
            ..
        } => object_place
            .iter()
            .chain(arg_places.iter().flatten())
            .collect(),
        MirInstr::MultiSet {
            receiver_place,
            arg_places,
            value_place,
            ..
        } => receiver_place
            .iter()
            .chain(arg_places.iter().flatten())
            .chain(value_place.iter())
            .collect(),
        _ => Vec::new(),
    }
}

pub(super) fn verify_place(
    function_name: &str,
    block: usize,
    function: &MirFunction,
    declarations: &MirDeclarations,
    place: &MirPlace,
    executable: bool,
    errors: &mut Vec<String>,
) {
    let prefix = format!("MIR function '{function_name}' block {block}");
    if place.root as usize >= function.n_vars {
        errors.push(format!(
            "{prefix} place has invalid root slot {}",
            place.root
        ));
    } else if let Some(declared) = function.var_tys.get(&place.root)
        && let Some(recorded) = &place.root_ty
        && !types_compatible(recorded, declared)
    {
        errors.push(format!(
            "{prefix} place root slot {} is typed {recorded}, declared {declared}",
            place.root
        ));
    }
    if let Some(through) = place.through {
        if through as usize >= function.n_vars {
            errors.push(format!(
                "{prefix} place has invalid through-reference slot {through}"
            ));
        } else {
            let parameter_handle = (through as usize) < function.n_params
                && function
                    .ref_params
                    .get(through as usize)
                    .copied()
                    .unwrap_or(false);
            let receiver_handle = through == 0
                && declarations.functions.iter().any(|declaration| {
                    declaration.lowered_name == function_name
                        && declaration.has_receiver
                        && matches!(
                            declaration.receiver_convention,
                            Some(crate::ast::ArgConvention::Mut | crate::ast::ArgConvention::Ref)
                        )
                });
            let local_handle = match function.var_tys.get(&through) {
                Some(Ty::Ref(_)) => true,
                Some(Ty::Pointer { origin, .. }) => origin.as_origin().is_some(),
                _ => false,
            };
            if !parameter_handle && !receiver_handle && !local_handle {
                let declared = function
                    .var_tys
                    .get(&through)
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "<untyped>".to_string());
                errors.push(format!(
                    "{prefix} place through slot {through} is not a checked reference-capability binding (declared {declared})"
                ));
            }
        }
    }
    if !place.is_typed() {
        errors.push(format!(
            "{prefix} place rooted at slot {} lacks complete checked type metadata",
            place.root
        ));
        return;
    }
    if place.root_ty.as_ref().is_some_and(contains_runtime_pack)
        || place.projection_tys.iter().any(contains_runtime_pack)
        || place.ty.as_ref().is_some_and(contains_runtime_pack)
    {
        errors.push(format!(
            "{prefix} place rooted at slot {} retains ABI-only RuntimePack type metadata",
            place.root
        ));
    }
    // A materialized reference-result place is physically rooted in a hidden
    // `ref T` slot, while its projections address the referent. Ordinary
    // substituted local aliases may instead keep an owned root plus analytical
    // `through` metadata, so only an actually reference-typed root is unwrapped.
    let mut current = match &place.root_ty {
        Some(Ty::Ref(reference))
            if place.through.is_some()
                && (!place.proj.is_empty() || place.ty.as_ref() != place.root_ty.as_ref()) =>
        {
            Some((*reference.referent).clone())
        }
        other => other.clone(),
    };
    for (projection, projected) in place.proj.iter().zip(&place.projection_tys) {
        match projection {
            Proj::Index(register) => {
                if register.0 >= function.n_regs {
                    errors.push(format!(
                        "{prefix} place index uses invalid register r{}",
                        register.0
                    ));
                } else if let Some(index_ty) = function.reg_types.get(&register.0)
                    && !types_compatible(index_ty, &Ty::Int)
                    && !matches!(index_ty, Ty::UInt)
                {
                    errors.push(format!(
                        "{prefix} place index register r{} has non-Indexer type {index_ty}",
                        register.0
                    ));
                }
                if let Some(base) = &current {
                    match indexed_place_element_types(base) {
                        Some(candidates)
                            if !candidates.iter().any(|candidate| {
                                types_compatible(projected, candidate)
                                    || matches!(candidate, Ty::Ref(reference) if types_compatible(projected, &reference.referent))
                            }) =>
                        {
                            errors.push(format!(
                                "{prefix} place dynamic element typed {projected}, incompatible with checked base type {base}"
                            ));
                        }
                        None if executable => errors.push(format!(
                            "{prefix} dynamic element projection requires checked indexed storage, got {base}"
                        )),
                        None => {}
                        Some(_) => {}
                    }
                }
            }
            Proj::ConstIndex(index) => match &current {
                Some(Ty::Tuple(elements)) => match elements.get(*index) {
                    None => errors.push(format!(
                        "{prefix} place projects Tuple element {index} out of {}",
                        elements.len()
                    )),
                    Some(element) if !types_compatible(projected, element) => errors.push(
                        format!(
                            "{prefix} place Tuple element {index} typed {projected}, declared {element}"
                        ),
                    ),
                    Some(_) => {}
                },
                Some(other) => errors.push(format!(
                    "{prefix} constant element projection requires compiler-private Tuple storage, got {other}"
                )),
                None => {}
            },
            Proj::Field(field) => {
                // A concrete non-generic struct's field projection must agree
                // with its declared layout; generic layouts would need
                // substitution the verifier deliberately does not re-derive.
                if let Some(Ty::Struct(struct_name, arguments)) = &current
                    && arguments.is_empty()
                    && let Some(declaration) = declarations
                        .structs
                        .iter()
                        .find(|declaration| &declaration.name == struct_name)
                {
                    match declaration
                        .fields
                        .iter()
                        .find(|(candidate, _)| candidate == field)
                    {
                        // A value parameter reads through field syntax; its
                        // declaration lives in `param_decls`, not the layout.
                        None if declaration
                            .param_decls
                            .iter()
                            .any(|decl| {
                                matches!(decl, crate::types::ParamDecl::Value { name, .. } if name == field)
                            }) => {}
                        None => errors.push(format!(
                            "{prefix} place projects unknown field '{field}' of '{struct_name}'"
                        )),
                        Some((_, declared)) if !types_compatible(projected, declared) => errors
                            .push(format!(
                                "{prefix} place field '{field}' of '{struct_name}' typed \
                                 {projected}, declared {declared}"
                            )),
                        Some(_) => {}
                    }
                }
            }
            Proj::Variant(index) => {
                if let Some(Ty::Variant(alternatives)) = &current
                    && *index >= alternatives.len()
                {
                    errors.push(format!(
                        "{prefix} place projects variant alternative {index} out of {}",
                        alternatives.len()
                    ));
                }
            }
            Proj::UninitPayload => {
                if let Some(base) = &current {
                    match crate::types::uninit_storage_element(base) {
                        Some(element) if !types_compatible(projected, element) => {
                            errors.push(format!(
                                "{prefix} place uninit payload typed {projected}, declared {element}"
                            ));
                        }
                        Some(_) => {}
                        None => errors.push(format!(
                            "{prefix} payload projection requires compiler-private inline uninit storage, got {base}"
                        )),
                    }
                }
            }
        }
        current = Some(projected.clone());
    }
    if let (Some(current), Some(terminal)) = (&current, &place.ty)
        && !types_compatible(terminal, current)
    {
        errors.push(format!(
            "{prefix} place terminal type {terminal} does not match projected type {current}"
        ));
    }
}
