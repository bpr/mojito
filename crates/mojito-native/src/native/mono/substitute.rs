//! Binding substitution over functions, declarations, instructions,
//! places, and types.

use super::*;

pub(super) fn substitute_function(
    function: &mut MirFunction,
    bindings: &Bindings,
) -> Result<(), MonoError> {
    substitute_value_parameter_reads(
        &mut function.blocks,
        &function.var_names,
        &function.var_tys,
        bindings,
    )?;
    for (var, name) in function.var_names.iter().enumerate() {
        if let Some(value) = bindings.values.get(name) {
            let ty = match value {
                CtValue::Int(_) => Ty::Int,
                CtValue::Bool(_) => Ty::Bool,
                _ => continue,
            };
            function.var_tys.insert(var as u32, ty);
        }
    }
    for ty in &mut function.param_types {
        *ty = substitute_ty(ty, bindings)?;
    }
    for ty in function.var_tys.values_mut() {
        *ty = substitute_ty(ty, bindings)?;
    }
    for ty in function.reg_types.values_mut() {
        *ty = substitute_ty(ty, bindings)?;
    }
    if let Some(ty) = &mut function.ret_ty {
        *ty = substitute_ty(ty, bindings)?;
    }
    if let Some(ty) = &mut function.error_ty {
        *ty = substitute_ty(ty, bindings)?;
    }
    substitute_blocks_metadata(&mut function.blocks, bindings)?;
    repair_storage_result_types(function);
    Ok(())
}

pub(super) fn repair_storage_result_types(function: &mut MirFunction) {
    pub(super) fn collect_retyped_iterator_slots(blocks: &[MirBlock], slots: &mut HashSet<u32>) {
        for block in blocks {
            for instruction in &block.instrs {
                match instruction {
                    MirInstr::GetIter { source, dest, .. } if source == dest => {
                        slots.insert(*dest);
                    }
                    MirInstr::Try {
                        body,
                        handler,
                        orelse,
                        finalbody,
                        ..
                    } => {
                        collect_retyped_iterator_slots(body, slots);
                        if let Some((_, blocks)) = handler {
                            collect_retyped_iterator_slots(blocks, slots);
                        }
                        if let Some(blocks) = orelse {
                            collect_retyped_iterator_slots(blocks, slots);
                        }
                        if let Some(blocks) = finalbody {
                            collect_retyped_iterator_slots(blocks, slots);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    pub(super) fn visit(
        blocks: &[MirBlock],
        var_tys: &HashMap<u32, Ty>,
        reg_tys: &HashMap<u32, Ty>,
        retyped_iterator_slots: &HashSet<u32>,
        reg_repairs: &mut Vec<(u32, Ty)>,
        var_repairs: &mut Vec<(u32, Ty)>,
    ) {
        for block in blocks {
            for instruction in &block.instrs {
                match instruction {
                    MirInstr::UseVar { dest, var, .. } if !retyped_iterator_slots.contains(var) => {
                        if let Some(ty) = var_tys.get(var) {
                            reg_repairs.push((dest.0, ty.clone()));
                        }
                    }
                    MirInstr::LoadPlace { dest, place }
                        if place.proj.is_empty()
                            && !retyped_iterator_slots.contains(&place.root) =>
                    {
                        if let Some(ty) = var_tys.get(&place.root) {
                            // A load through a reference-holding root
                            // (`through`) reads the referent, not the handle.
                            let ty = match ty {
                                Ty::Ref(reference) if place.through.is_some() => {
                                    (*reference.referent).clone()
                                }
                                other => other.clone(),
                            };
                            reg_repairs.push((dest.0, ty));
                        }
                    }
                    MirInstr::DefVar { var, src, .. } if !retyped_iterator_slots.contains(var) => {
                        if let Some(ty) = reg_tys.get(&src.0) {
                            var_repairs.push((*var, ty.clone()));
                        }
                    }
                    MirInstr::Try {
                        body,
                        handler,
                        orelse,
                        finalbody,
                        ..
                    } => {
                        visit(
                            body,
                            var_tys,
                            reg_tys,
                            retyped_iterator_slots,
                            reg_repairs,
                            var_repairs,
                        );
                        if let Some((_, blocks)) = handler {
                            visit(
                                blocks,
                                var_tys,
                                reg_tys,
                                retyped_iterator_slots,
                                reg_repairs,
                                var_repairs,
                            );
                        }
                        if let Some(blocks) = orelse {
                            visit(
                                blocks,
                                var_tys,
                                reg_tys,
                                retyped_iterator_slots,
                                reg_repairs,
                                var_repairs,
                            );
                        }
                        if let Some(blocks) = finalbody {
                            visit(
                                blocks,
                                var_tys,
                                reg_tys,
                                retyped_iterator_slots,
                                reg_repairs,
                                var_repairs,
                            );
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    let mut retyped_iterator_slots = HashSet::new();
    collect_retyped_iterator_slots(&function.blocks, &mut retyped_iterator_slots);
    for _ in 0..3 {
        let mut reg_repairs = Vec::new();
        let mut var_repairs = Vec::new();
        visit(
            &function.blocks,
            &function.var_tys,
            &function.reg_types,
            &retyped_iterator_slots,
            &mut reg_repairs,
            &mut var_repairs,
        );
        function.reg_types.extend(reg_repairs);
        function.var_tys.extend(var_repairs);
    }
}

pub(super) fn substitute_value_parameter_reads(
    blocks: &mut [MirBlock],
    var_names: &[String],
    var_tys: &HashMap<u32, Ty>,
    bindings: &Bindings,
) -> Result<(), MonoError> {
    for block in blocks {
        for instruction in &mut block.instrs {
            if let MirInstr::UseVar { dest, var, .. } = instruction
                && let Some(name) = var_names.get(*var as usize)
                && let Some(value) = bindings.values.get(name)
            {
                let constant = if let Some(callable) = bindings.callables.get(name) {
                    Const::Function(callable.clone())
                } else {
                    match value {
                        CtValue::Int(value) => Const::Int(*value),
                        CtValue::Bool(value) => Const::Bool(*value),
                        CtValue::Str(value)
                            if matches!(
                                var_tys.get(var),
                                Some(Ty::Func { .. } | Ty::GenericFunc { .. })
                            ) =>
                        {
                            Const::Function(value.clone())
                        }
                        _ => {
                            return Err(MonoError {
                                function: None,
                                construct: format!("unsupported runtime value parameter `{value}`"),
                            });
                        }
                    }
                };
                *instruction = MirInstr::Const {
                    dest: *dest,
                    k: constant,
                };
            } else if let MirInstr::Try {
                body,
                handler,
                orelse,
                finalbody,
                ..
            } = instruction
            {
                substitute_value_parameter_reads(body, var_names, var_tys, bindings)?;
                if let Some((_, blocks)) = handler {
                    substitute_value_parameter_reads(blocks, var_names, var_tys, bindings)?;
                }
                if let Some(blocks) = orelse {
                    substitute_value_parameter_reads(blocks, var_names, var_tys, bindings)?;
                }
                if let Some(blocks) = finalbody {
                    substitute_value_parameter_reads(blocks, var_names, var_tys, bindings)?;
                }
            }
        }
    }
    Ok(())
}

pub(super) fn substitute_declaration(
    decl: &mut MirFunctionDeclaration,
    bindings: &Bindings,
) -> Result<(), MonoError> {
    for ty in &mut decl.param_types {
        *ty = substitute_ty(ty, bindings)?;
    }
    if let Some(ty) = &mut decl.variadic {
        *ty = substitute_ty(ty, bindings)?;
        // An arity-specialized instance's pack reifies as the concrete
        // tuple shape the call site collected.
        if let Some(arity) = bindings.variadic_arity
            && !matches!(ty, Ty::RuntimePack(_) | Ty::Tuple(_))
        {
            *ty = Ty::RuntimePack(vec![ty.clone(); arity]);
        }
    }
    if let Some(ty) = &mut decl.kw_variadic {
        *ty = substitute_ty(ty, bindings)?;
    }
    decl.ret_ty = substitute_ty(&decl.ret_ty, bindings)?;
    if let Some(ty) = &mut decl.error_ty {
        *ty = substitute_ty(ty, bindings)?;
    }
    Ok(())
}

pub(super) fn substitute_blocks_metadata(
    blocks: &mut [MirBlock],
    bindings: &Bindings,
) -> Result<(), MonoError> {
    for block in blocks {
        for instruction in &mut block.instrs {
            substitute_instruction(instruction, bindings)?;
        }
    }
    Ok(())
}

pub(super) fn substitute_instruction(
    instruction: &mut MirInstr,
    bindings: &Bindings,
) -> Result<(), MonoError> {
    use MirInstr::*;
    match instruction {
        EstablishLoans { loans, .. } => {
            for loan in loans {
                substitute_place(&mut loan.place, bindings)?;
            }
        }
        MakeRef { place, .. }
        | MovePlace { place, .. }
        | Store { place, .. }
        | StoreRef { place, .. }
        | LoadPlace { place, .. }
        | ConsumePlace { place, .. } => substitute_place(place, bindings)?,
        MakeClosure { captures, .. } => {
            for capture in captures {
                substitute_place(&mut capture.place, bindings)?;
            }
        }
        MaterializeLiteral { target, .. }
        | SizeOf { ty: target, .. }
        | PointerStorageTake {
            element: target, ..
        }
        | PointerStorageDestroy {
            element: target, ..
        }
        | UninitStorageTake {
            element: target, ..
        }
        | UninitStorageDestroy {
            element: target, ..
        } => *target = substitute_ty(target, bindings)?,
        Next {
            call: Some(call), ..
        } => substitute_iterator_call(call, bindings)?,
        TryNext {
            call, exhaustion, ..
        } => {
            substitute_iterator_call(call, bindings)?;
            *exhaustion = substitute_ty(exhaustion, bindings)?;
        }
        DefVar {
            binding_ty: Some(ty),
            ..
        } => *ty = substitute_ty(ty, bindings)?,
        Call {
            raises,
            arg_places,
            kwarg_places,
            ..
        } => {
            sub_opt_ty(raises, bindings)?;
            sub_places(arg_places, bindings)?;
            sub_places(kwarg_places, bindings)?;
        }
        // `H()` on a type parameter constructs the bound struct: once the
        // binding is concrete this is an ordinary nullary constructor call,
        // which the call rewriting below then instantiates.
        ConstructTypeParam { dest, param } => {
            let Some(Ty::Struct(struct_name, _)) = bindings.types.get(param.as_str()) else {
                return Err(MonoError {
                    function: None,
                    construct: format!(
                        "constructing type parameter `{param}` without a concrete struct binding"
                    ),
                });
            };
            *instruction = Call {
                dest: *dest,
                func: mojito_mir::mir::FuncRef::named(struct_name),
                raises: None,
                args: Vec::new(),
                kwargs: Vec::new(),
                arg_places: Vec::new(),
                kwarg_places: Vec::new(),
                capture_accesses: Vec::new(),
                param_arg_regs: Vec::new(),
            };
        }
        CallIndirect {
            raises,
            callee_place,
            arg_places,
            kwarg_places,
            instantiated_contract,
            instantiated_args,
            ..
        } => {
            sub_opt_ty(raises, bindings)?;
            sub_place_opt(callee_place, bindings)?;
            sub_places(arg_places, bindings)?;
            sub_places(kwarg_places, bindings)?;
            sub_opt_ty(instantiated_contract, bindings)?;
            for arg in instantiated_args {
                *arg = substitute_arg(arg, bindings)?;
            }
        }
        MethodCall {
            raises,
            reference_result,
            recv_place,
            arg_places,
            kwarg_places,
            ..
        } => {
            sub_opt_ty(raises, bindings)?;
            sub_ref_opt(reference_result, bindings)?;
            sub_place_opt(recv_place, bindings)?;
            sub_places(arg_places, bindings)?;
            sub_places(kwarg_places, bindings)?;
        }
        Index {
            base_place,
            index_place,
            call,
            ..
        } => {
            sub_place_opt(base_place, bindings)?;
            sub_place_opt(index_place, bindings)?;
            if let Some(call) = call {
                substitute_subscript_call(call, bindings)?;
            }
        }
        Slice {
            object_place,
            arg_places,
            call,
            ..
        }
        | MultiIndex {
            object_place,
            arg_places,
            call,
            ..
        } => {
            sub_place_opt(object_place, bindings)?;
            sub_places(arg_places, bindings)?;
            if let Some(call) = call {
                substitute_subscript_call(call, bindings)?;
            }
        }
        MultiSet {
            receiver_place,
            arg_places,
            value_place,
            call,
            ..
        } => {
            sub_place_opt(receiver_place, bindings)?;
            sub_places(arg_places, bindings)?;
            sub_place_opt(value_place, bindings)?;
            substitute_subscript_call(call, bindings)?;
        }
        MakeTuple {
            element_types: Some(types),
            ..
        }
        | MakeVariant {
            alternatives: types,
            ..
        } => {
            for ty in types {
                *ty = substitute_ty(ty, bindings)?;
            }
        }
        VariantSet { place, .. }
        | VariantSetInitWith { place, .. }
        | VariantReplace { place, .. } => substitute_place(place, bindings)?,
        Try {
            body,
            handler,
            orelse,
            finalbody,
            ..
        } => {
            substitute_blocks_metadata(body, bindings)?;
            if let Some((_, b)) = handler {
                substitute_blocks_metadata(b, bindings)?;
            }
            if let Some(b) = orelse {
                substitute_blocks_metadata(b, bindings)?;
            }
            if let Some(b) = finalbody {
                substitute_blocks_metadata(b, bindings)?;
            }
        }
        _ => {}
    }
    Ok(())
}

pub(super) fn substitute_subscript_call(
    call: &mut mojito_mir::mir::MirSubscriptCall,
    bindings: &Bindings,
) -> Result<(), MonoError> {
    sub_opt_ty(&mut call.raises, bindings)?;
    call.result_ty = substitute_ty(&call.result_ty, bindings)?;
    sub_ref_opt(&mut call.reference_result, bindings)
}

pub(super) fn substitute_place(place: &mut MirPlace, bindings: &Bindings) -> Result<(), MonoError> {
    sub_opt_ty(&mut place.root_ty, bindings)?;
    for ty in &mut place.projection_tys {
        *ty = substitute_ty(ty, bindings)?;
    }
    sub_opt_ty(&mut place.ty, bindings)
}
pub(super) fn sub_places(
    places: &mut [Option<MirPlace>],
    bindings: &Bindings,
) -> Result<(), MonoError> {
    for place in places {
        sub_place_opt(place, bindings)?;
    }
    Ok(())
}
pub(super) fn sub_place_opt(
    place: &mut Option<MirPlace>,
    bindings: &Bindings,
) -> Result<(), MonoError> {
    if let Some(place) = place {
        substitute_place(place, bindings)?;
    }
    Ok(())
}
pub(super) fn sub_opt_ty(ty: &mut Option<Ty>, bindings: &Bindings) -> Result<(), MonoError> {
    if let Some(ty) = ty {
        *ty = substitute_ty(ty, bindings)?;
    }
    Ok(())
}
pub(super) fn sub_ref_opt(
    ty: &mut Option<mojito_types::origin::RefTy>,
    bindings: &Bindings,
) -> Result<(), MonoError> {
    if let Some(ty) = ty {
        *ty.referent = substitute_ty(&ty.referent, bindings)?;
    }
    Ok(())
}

pub(super) fn substitute_iterator_call(
    call: &mut mojito_checked::checked::CheckedIteratorCall,
    bindings: &Bindings,
) -> Result<(), MonoError> {
    call.result_ty = substitute_ty(&call.result_ty, bindings)?;
    sub_opt_ty(&mut call.raises, bindings)?;
    sub_ref_opt(&mut call.reference_result, bindings)?;
    Ok(())
}

pub(super) fn substitute_ty(ty: &Ty, bindings: &Bindings) -> Result<Ty, MonoError> {
    let unsupported = |what: String| MonoError {
        function: None,
        construct: what,
    };
    Ok(match ty {
        Ty::Param { name, .. } => bindings
            .types
            .get(name)
            .cloned()
            .ok_or_else(|| unsupported(format!("unresolved type parameter `{name}`")))?,
        Ty::Struct(name, args) => {
            if args.is_empty() {
                // The bare in-body `self` spelling of a generic owner resolves
                // to the concrete instance being materialized; other bare
                // names are non-generic (or unresolvable, failing later).
                if let Some((template, concrete)) = &bindings.self_instance
                    && template == name
                {
                    return Ok(concrete.clone());
                }
                return Ok(Ty::Struct(name.clone(), Vec::new()));
            }
            let args = args
                .iter()
                .map(|arg| substitute_arg(arg, bindings))
                .collect::<Result<Vec<_>, _>>()?;
            // Every concrete application of a generic template takes its
            // instance symbol, so distinct instantiations get distinct output
            // declarations. Checker-specialized structs (empty `param_decls`)
            // and already-renamed instances keep their names; symbolic
            // applications stay for a later substitution or a contextual
            // rejection.
            let concrete_name = if args.iter().any(arg_has_symbolic)
                || nominal_template(name) != name
                || !bindings.generic_templates.contains(name.as_str())
            {
                name.clone()
            } else {
                mojito_symbol::symbol::instance_symbol(
                    name,
                    &args
                        .iter()
                        .filter_map(|arg| match arg {
                            TyArg::Ty(ty) => Some(InstanceArg::Ty(ty.clone())),
                            TyArg::Val(value) => Some(InstanceArg::Value(value.clone())),
                            TyArg::Origin(_) => None,
                        })
                        .collect::<Vec<_>>(),
                )
            };
            Ty::Struct(concrete_name, args)
        }
        Ty::Tuple(v) => Ty::Tuple(sub_types(v, bindings)?),
        Ty::RuntimePack(v) => Ty::RuntimePack(sub_types(v, bindings)?),
        Ty::Variant(v) => Ty::Variant(sub_types(v, bindings)?),
        Ty::Overload(v) => Ty::Overload(sub_types(v, bindings)?),
        Ty::ComptimeList(v) => Ty::ComptimeList(Box::new(substitute_ty(v, bindings)?)),
        Ty::VariadicPack(v) => {
            let element = substitute_ty(v, bindings)?;
            match bindings.variadic_arity {
                // An unspecialized variadic callee instantiates at its
                // call-site arity: the pack becomes a concrete tuple shape.
                Some(arity) => Ty::RuntimePack(vec![element; arity]),
                None => Ty::VariadicPack(Box::new(element)),
            }
        }
        Ty::Pointer { element, origin } => Ty::Pointer {
            element: Box::new(substitute_ty(element, bindings)?),
            origin: origin.clone(),
        },
        Ty::Ref(value) => {
            let mut value = value.clone();
            value.referent = Box::new(substitute_ty(&value.referent, bindings)?);
            Ty::Ref(value)
        }
        Ty::Dependent(DependentType::Indexed { elements, index }) => {
            let value = eval_ct(index, bindings)?;
            let index = match value {
                CtValue::Int(v) => usize::try_from(v).ok(),
                CtValue::UInt(v) => usize::try_from(v).ok(),
                _ => None,
            }
            .ok_or_else(|| {
                unsupported("dependent type index is not a non-negative integer".to_string())
            })?;
            substitute_ty(
                elements.get(index).ok_or_else(|| {
                    unsupported(format!("dependent type index {index} is out of range"))
                })?,
                bindings,
            )?
        }
        Ty::Assoc { .. } => bindings
            .associated
            .get(&ty.to_string())
            .cloned()
            .ok_or_else(|| {
                unsupported(format!(
                    "associated type `{ty}` has no concrete MIR declaration fact"
                ))
            })?,
        // A generic callable remains as a transient storage type until its
        // statically named producer and dependent call sites are rewritten.
        // `ensure_concrete_function` rejects it if any executable use survives.
        Ty::GenericFunc { .. } => ty.clone(),
        Ty::SelfType | Ty::Infer => {
            return Err(unsupported(format!("unresolved type `{ty}`")));
        }
        Ty::Func {
            environment,
            params,
            names,
            ret,
            required,
            variadic,
            kw_variadic,
            positional_only,
            keyword_only,
            raises,
            error,
            conventions,
            ref_params,
            ref_return,
            transfers,
        } => Ty::Func {
            environment: environment.clone(),
            params: sub_types(params, bindings)?,
            names: names.clone(),
            ret: Box::new(substitute_ty(ret, bindings)?),
            required: required.clone(),
            variadic: variadic
                .as_ref()
                .map(|t| substitute_ty(t, bindings).map(Box::new))
                .transpose()?,
            kw_variadic: kw_variadic
                .as_ref()
                .map(|t| substitute_ty(t, bindings).map(Box::new))
                .transpose()?,
            positional_only: *positional_only,
            keyword_only: *keyword_only,
            raises: *raises,
            error: error
                .as_ref()
                .map(|t| substitute_ty(t, bindings).map(Box::new))
                .transpose()?,
            conventions: conventions.clone(),
            ref_params: ref_params.clone(),
            ref_return: ref_return.clone(),
            transfers: transfers.clone(),
        },
        other => other.clone(),
    })
}

pub(super) fn substitute_arg(arg: &TyArg, bindings: &Bindings) -> Result<TyArg, MonoError> {
    Ok(match arg {
        TyArg::Ty(ty) => TyArg::Ty(substitute_ty(ty, bindings)?),
        TyArg::Val(CtValue::Param(name)) => TyArg::Val(
            bindings
                .values
                .get(name)
                .cloned()
                .ok_or_else(|| MonoError {
                    function: None,
                    construct: format!("unresolved value parameter `{name}`"),
                })?,
        ),
        TyArg::Val(value) => TyArg::Val(value.clone()),
        TyArg::Origin(origin) => TyArg::Origin(origin.clone()),
    })
}
pub(super) fn sub_types(types: &[Ty], bindings: &Bindings) -> Result<Vec<Ty>, MonoError> {
    types.iter().map(|ty| substitute_ty(ty, bindings)).collect()
}
