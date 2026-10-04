//! Binding substitution over functions, declarations, instructions,
//! places, and types.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

/// Rewrite `T()` over a binder these bindings solve to a SIMD type into the
/// zero vector of that type: a splat of one zero lane, held in a fresh
/// register. `Defaultable` admits every SIMD value, and a single
/// instruction cannot spell the splat.
pub(super) fn default_construct_simd_parameters(function: &mut MirFunction, bindings: &Bindings) {
    use mojito_ast::ast::Dtype;
    for block in &mut function.blocks {
        let mut index = 0;
        while index < block.instrs.len() {
            let MirInstr::ConstructTypeParam { dest, param } = &block.instrs[index] else {
                index += 1;
                continue;
            };
            let Some(Ty::Simd { dtype, width }) = bindings.types.get(param) else {
                index += 1;
                continue;
            };
            let (Some(dtype), Some(width)) = (
                dtype.known(),
                width.known().and_then(|width| usize::try_from(width).ok()),
            ) else {
                index += 1;
                continue;
            };
            let (zero, lane_ty) = match dtype {
                Dtype::Bool => (Const::Bool(false), Ty::Bool),
                Dtype::Float16 | Dtype::Float32 | Dtype::Float64 => {
                    (Const::Float(0.0), Ty::Float64)
                }
                _ => (Const::Int(0), Ty::Int),
            };
            let dest = *dest;
            let lane = Reg(function.n_regs);
            function.n_regs += 1;
            function.reg_types.insert(lane.0, lane_ty);
            block.instrs[index] = MirInstr::MakeSimd {
                dest,
                dtype: SimdDtype::Known(dtype),
                width: SimdWidth::Known(width as i64),
                elems: vec![lane],
            };
            block.instrs.insert(
                index,
                MirInstr::Const {
                    dest: lane,
                    k: zero,
                },
            );
            index += 2;
        }
    }
}

/// Substitute `bindings` through `function`, whose declaration declares the
/// compile-time parameters in `scope`.
pub(super) fn substitute_function(
    function: &mut MirFunction,
    bindings: &Bindings,
    scope: &[ParamDecl],
) -> Result<(), MonoError> {
    let locals = bound_parameter_locals(scope, bindings);
    substitute_value_parameter_reads(
        &mut function.blocks,
        &function.var_names,
        &function.var_tys,
        &locals,
        &bindings.callables,
    )?;
    for (var, name) in function.var_names.iter().enumerate() {
        if let Some(value) = locals.get(name.as_str()) {
            let ty = match value {
                CtValue::Int(_) => Ty::Int,
                CtValue::Bool(_) => Ty::Bool,
                _ => continue,
            };
            function.var_tys.insert(var as u32, ty);
        }
    }
    initialize_captured_value_parameters(function, &locals, &bindings.callables);
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
    locals: &HashMap<String, &CtValue>,
    callables: &HashMap<String, String>,
) -> Result<(), MonoError> {
    for block in blocks {
        for instruction in &mut block.instrs {
            if let MirInstr::UseVar { dest, var, .. } = instruction
                && let Some(name) = var_names.get(*var as usize)
                && (callables.contains_key(name) || locals.contains_key(name))
            {
                let constant = if let Some(callable) = callables.get(name) {
                    Const::Function(callable.clone())
                } else {
                    let value = locals[name];
                    value_parameter_constant(value, var_tys.get(var)).ok_or_else(|| MonoError {
                        function: None,
                        construct: format!("unsupported runtime value parameter `{value}`"),
                    })?
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
                substitute_value_parameter_reads(body, var_names, var_tys, locals, callables)?;
                if let Some((_, blocks)) = handler {
                    substitute_value_parameter_reads(
                        blocks, var_names, var_tys, locals, callables,
                    )?;
                }
                if let Some(blocks) = orelse {
                    substitute_value_parameter_reads(
                        blocks, var_names, var_tys, locals, callables,
                    )?;
                }
                if let Some(blocks) = finalbody {
                    substitute_value_parameter_reads(
                        blocks, var_names, var_tys, locals, callables,
                    )?;
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
    use MirInstr::{
        Call, CallIndirect, Const, ConstructTypeParam, ConsumePlace, DefVar, DropPlace,
        EstablishLoans, Index, LoadPlace, MakeClosure, MakeRef, MakeSimd, MakeTuple, MakeVariant,
        MaterializeLiteral, MethodCall, MovePlace, MultiIndex, MultiSet, PointerStorageDestroy,
        PointerStorageTake, SimdBitcast, SimdCast, SizeOf, Slice, Store, StoreRef, Try, TryNext,
        TypeName, UninitStorageDestroy, UninitStorageTake, VariantReplace, VariantSet,
        VariantSetInitWith,
    };
    match instruction {
        // A SIMD instruction's slots close as the vector type they build
        // does; one the bindings leave symbolic stays for the concreteness
        // check to name.
        MakeSimd { dtype, width, .. }
        | SimdCast { dtype, width, .. }
        | SimdBitcast { dtype, width, .. } => {
            let built = substitute_ty(
                &Ty::Simd {
                    dtype: dtype.clone(),
                    width: width.clone(),
                },
                bindings,
            )?;
            if let Some((closed_dtype, closed_width)) = mojito_types::types::simd_slots(&built) {
                *dtype = closed_dtype;
                *width = closed_width;
            }
        }
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
        | ConsumePlace { place, .. }
        | DropPlace { place } => substitute_place(place, bindings)?,
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
            receiver,
            instantiated_args,
            ..
        } => {
            sub_opt_ty(raises, bindings)?;
            sub_places(arg_places, bindings)?;
            sub_places(kwarg_places, bindings)?;
            sub_opt_ty(receiver, bindings)?;
            for arg in instantiated_args {
                *arg = substitute_arg(arg, bindings)?;
            }
        }
        // A type name is spelled from the instance's type; a type the
        // bindings leave symbolic stays for the verifier's concrete mode.
        TypeName { dest, ty } => {
            *ty = substitute_ty(ty, bindings)?;
            if !mojito_types::types::is_symbolic(ty) {
                *instruction = Const {
                    dest: *dest,
                    k: mojito_mir::mir::Const::Str(
                        mojito_symbol::symbol::unqualified_instance_name(ty),
                    ),
                };
            }
        }
        // `H()` on a type parameter constructs the bound struct: once the
        // binding is concrete this is an ordinary nullary constructor call,
        // which the call rewriting below then instantiates.
        ConstructTypeParam { dest, param } => {
            // A scalar binding default-constructs as its zero value (the
            // VM's `ConstructTypeParam` answer for the built-in
            // `Defaultable` types).
            let scalar_default = match bindings.types.get(&*param) {
                Some(Ty::Int | Ty::IntLiteral) => Some(mojito_mir::mir::Const::Int(0)),
                Some(Ty::UInt) => Some(mojito_mir::mir::Const::Int(0)),
                Some(Ty::Bool) => Some(mojito_mir::mir::Const::Bool(false)),
                Some(Ty::Float64 | Ty::FloatLiteral) => Some(mojito_mir::mir::Const::Float(0.0)),
                Some(Ty::StringLiteral) => Some(mojito_mir::mir::Const::Str(String::new())),
                Some(Ty::None) => Some(mojito_mir::mir::Const::None),
                _ => None,
            };
            if let Some(k) = scalar_default {
                *instruction = Const { dest: *dest, k };
                return Ok(());
            }
            let Some(Ty::Struct(struct_name, _)) = bindings.types.get(&*param) else {
                return Err(MonoError {
                    function: None,
                    construct: format!(
                        "constructing type parameter `{}` without a concrete struct binding",
                        param.name
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
                receiver: None,
                instantiated_args: Vec::new(),
                spread: None,
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
    // A parameter type closes under the caller's bindings when the checker
    // instantiated it at the call; one naming the callee's own binder stays
    // as the contract wrote it.
    for argument in &mut call.arguments {
        if let Ok(ty) = substitute_ty(&argument.parameter_ty, bindings) {
            argument.parameter_ty = ty;
        }
    }
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
        Ty::Param { binder, .. } => {
            bindings.types.get(binder).cloned().ok_or_else(|| {
                unsupported(format!("unresolved type parameter `{}`", binder.name))
            })?
        }
        Ty::Struct(name, original) => {
            if original.is_empty() {
                // The bare in-body `self` spelling of a generic owner resolves
                // to the concrete instance being materialized; other bare
                // names are non-generic (or unresolvable, failing later).
                if let Some((template, concrete)) = &bindings.self_instance
                    && template == name
                {
                    return Ok(concrete.clone());
                }
                return Ok(Ty::Struct(name.clone(), Vec::new().into()));
            }
            let args = original
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
            Ty::Struct(concrete_name, original.reusing(args))
        }
        Ty::Tuple(v) => Ty::Tuple(sub_types(v, bindings)?),
        Ty::RuntimePack(v) => Ty::RuntimePack(sub_types(v, bindings)?),
        Ty::Variant(v) => Ty::Variant(sub_types(v, bindings)?),
        Ty::Overload(v) => Ty::Overload(sub_types(v, bindings)?),
        Ty::ComptimeList(v) => Ty::ComptimeList(Box::new(substitute_ty(v, bindings)?)),
        // A collector over a bound type pack is the tuple of its elements.
        Ty::VariadicPack(v)
            if let Some(Ty::Param { binder, .. }) =
                mojito_types::types::pack_spread(std::slice::from_ref(&**v))
                && let Some(Ty::RuntimePack(elements)) = bindings.types.get(binder) =>
        {
            Ty::Tuple(sub_types(elements, bindings)?)
        }
        Ty::VariadicPack(v) => {
            let element = substitute_ty(v, bindings)?;
            match bindings.variadic_arity {
                // An unspecialized variadic callee instantiates at its
                // call-site arity: the pack becomes the concrete tuple shape
                // the body sees once the arguments are matched. The runtime
                // pack stays the declaration's variadic ABI root alone.
                Some(arity) => Ty::Tuple(vec![element; arity]),
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
        Ty::Dependent(dependent) => {
            let Some((elements, index)) = dependent.selection() else {
                // An element of a pack that is still a parameter (`Ts[i]`)
                // closes once the pack and the index are bound.
                let closed = mojito_types::types::replace_parameters(
                    &ParamContext::detached(),
                    ty,
                    &ct_bindings(bindings),
                    0,
                )
                .map_err(|error| unsupported(error.to_string()))?;
                if matches!(closed, Ty::Dependent(_)) {
                    return Err(unsupported(format!(
                        "dependent type `{ty}` has no concrete MIR declaration fact"
                    )));
                }
                return substitute_ty(&closed, bindings);
            };
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
        Ty::Assoc { base, name, args } => match bindings.associated.get(&ty.to_string()) {
            Some(solved) => solved.clone(),
            None => declared_associated_type(base, name, args, bindings)?.ok_or_else(|| {
                unsupported(format!(
                    "associated type `{ty}` has no concrete MIR declaration fact"
                ))
            })?,
        },
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
        // A symbolic lane or width the bindings close — a layout application
        // the instance's oracle answers — becomes the known slot; one they do
        // not close stays as spelled, for the concreteness check to name.
        Ty::Simd { dtype, width } => Ty::Simd {
            dtype: match dtype {
                SimdDtype::Expr(expr) => match eval_ct(expr, bindings) {
                    Ok(CtValue::Dtype(known)) => SimdDtype::Known(known),
                    _ => dtype.clone(),
                },
                SimdDtype::Known(known) => SimdDtype::Known(*known),
            },
            width: match width {
                SimdWidth::Expr(expr) => match eval_ct(expr, bindings) {
                    Ok(CtValue::Int(known)) => SimdWidth::Known(known),
                    _ => width.clone(),
                },
                SimdWidth::Known(known) => SimdWidth::Known(*known),
            },
        },
        other => other.clone(),
    })
}

pub(super) fn substitute_arg(arg: &TyArg, bindings: &Bindings) -> Result<TyArg, MonoError> {
    Ok(match arg {
        TyArg::Ty(ty) => TyArg::Ty(substitute_ty(ty, bindings)?),
        // A residual closes under the mono environment or it is the
        // contextual unsupported boundary; nothing symbolic reaches lowering.
        TyArg::Val(CtValue::Expr(expr)) => TyArg::Val(eval_ct(expr, bindings)?),
        TyArg::Val(value) => TyArg::Val(value.clone()),
        TyArg::Origin(origin) => TyArg::Origin(origin.clone()),
    })
}
pub(super) fn sub_types(types: &[Ty], bindings: &Bindings) -> Result<Vec<Ty>, MonoError> {
    types.iter().map(|ty| substitute_ty(ty, bindings)).collect()
}

/// The constant a value parameter's slot holds: a scalar, or a function
/// named by a callable-typed slot.
pub(super) fn value_parameter_constant(value: &CtValue, slot_ty: Option<&Ty>) -> Option<Const> {
    match value {
        CtValue::Int(value) => Some(Const::Int(*value)),
        CtValue::Bool(value) => Some(Const::Bool(*value)),
        CtValue::Dtype(value) => Some(Const::Dtype(*value)),
        CtValue::Str(value)
            if matches!(slot_ty, Some(Ty::Func { .. } | Ty::GenericFunc { .. })) =>
        {
            Some(Const::Function(value.clone()))
        }
        _ => None,
    }
}

/// Every read of a value parameter's slot folds to its bound constant, so
/// the slot itself is never stored; a closure capturing it would borrow
/// uninitialized storage. The entry block stores the constant into each
/// captured slot first.
fn initialize_captured_value_parameters(
    function: &mut MirFunction,
    locals: &HashMap<String, &CtValue>,
    callables: &HashMap<String, String>,
) {
    let mut captured = HashSet::new();
    collect_captured_vars(&function.blocks, &mut captured);
    let mut initializers = Vec::new();
    for (var, name) in function.var_names.iter().enumerate() {
        let var = var as u32;
        if !captured.contains(&var) {
            continue;
        }
        let Some(ty) = function.var_tys.get(&var).cloned() else {
            continue;
        };
        let constant = callables
            .get(name)
            .map(|callable| Const::Function(callable.clone()))
            .or_else(|| {
                locals
                    .get(name.as_str())
                    .and_then(|value| value_parameter_constant(value, Some(&ty)))
            });
        let Some(k) = constant else {
            continue;
        };
        let dest = Reg(function.n_regs);
        function.n_regs += 1;
        function.reg_types.insert(dest.0, ty.clone());
        initializers.push(MirInstr::Const { dest, k });
        initializers.push(MirInstr::DefVar {
            var,
            src: dest,
            binding_ty: Some(ty),
        });
    }
    if let Some(entry) = function.blocks.first_mut() {
        entry.instrs.splice(0..0, initializers);
    }
}

fn collect_captured_vars(blocks: &[MirBlock], captured: &mut HashSet<u32>) {
    for instruction in blocks.iter().flat_map(|block| &block.instrs) {
        match instruction {
            MirInstr::MakeClosure { captures, .. } => {
                captured.extend(captures.iter().map(|capture| capture.place.root));
            }
            MirInstr::Try {
                body,
                handler,
                orelse,
                finalbody,
                ..
            } => {
                collect_captured_vars(body, captured);
                for blocks in handler
                    .iter()
                    .map(|(_, blocks)| blocks)
                    .chain(orelse)
                    .chain(finalbody)
                {
                    collect_captured_vars(blocks, captured);
                }
            }
            _ => {}
        }
    }
}

/// The constant each compile-time parameter's local reads. MIR names a
/// value binder's local by its spelling, which the declaration's `scope`
/// resolves to the binder: a signature declares a spelling once. A callable
/// promoted to a runtime parameter keeps its reads.
pub(super) fn bound_parameter_locals<'a>(
    scope: &[ParamDecl],
    bindings: &'a Bindings,
) -> HashMap<String, &'a CtValue> {
    scope
        .iter()
        .filter(|decl| matches!(decl, ParamDecl::Value { .. }))
        .map(ParamDecl::binder)
        .filter(|binder| !bindings.runtime_callables.contains(binder))
        .filter_map(|binder| {
            let value = bindings.values.get(&binder)?;
            Some((binder.name.trim_start_matches('*').to_string(), value))
        })
        .collect()
}

/// The associated type `name` of the instance `base` resolves to, read off
/// the member its struct declares: the member's type under the instance's
/// own arguments. `None` where `base` is no struct that declares the member,
/// or the member takes arguments of its own.
fn declared_associated_type(
    base: &Ty,
    name: &str,
    args: &[TyArg],
    bindings: &Bindings,
) -> Result<Option<Ty>, MonoError> {
    if !args.is_empty() {
        return Ok(None);
    }
    let base = substitute_ty(base, bindings)?;
    let Ty::Struct(struct_name, struct_args) = peel_refs(&base) else {
        return Ok(None);
    };
    let Some((template, declared)) = bindings
        .associated_types
        .get_key_value(struct_name.as_str())
        .or_else(|| {
            bindings
                .associated_types
                .get_key_value(nominal_template(struct_name))
        })
    else {
        return Ok(None);
    };
    let Some((_, member)) = declared.members.iter().find(|(member, _)| member == name) else {
        return Ok(None);
    };
    let mut instance = Bindings {
        generic_templates: Rc::clone(&bindings.generic_templates),
        associated_types: Rc::clone(&bindings.associated_types),
        self_instance: Some((template.clone(), peel_refs(&base).clone())),
        ..Bindings::default()
    };
    bind_ty_args(&declared.param_decls, struct_args, &mut instance).map_err(|construct| {
        MonoError {
            function: None,
            construct,
        }
    })?;
    apply_defaults(&declared.param_decls, &mut instance)?;
    substitute_ty(member, &instance).map(Some)
}
