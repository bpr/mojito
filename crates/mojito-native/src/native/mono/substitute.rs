//! Binding substitution over functions, declarations, instructions,
//! places, and types.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_checked::declaration::AssociatedParameter;
use mojito_mir::mir::Proj;
use mojito_types::types::TyKind;
use mojito_types::types::{FuncTy, VariantIndex};

/// The declarations an `Array` display's construction names: the
/// list-literal initializer.
#[derive(Clone, Copy)]
pub(super) struct ConstructionDecls<'d, 'a> {
    pub(super) functions: &'d Catalog<'a, MirFunctionDeclaration>,
}

impl<'a> ConstructionDecls<'_, 'a> {
    /// The list-literal initializer of the struct `template`, as the
    /// checker selects it for a collection display.
    fn list_literal_constructor(self, template: &str) -> Option<&'a str> {
        let constructors = format!("{template}.__init__");
        self.functions.prefixed(&constructors).find(|symbol| {
            mojito_symbol::symbol::lifecycle_constructor(symbol) == Some((template, "__init__"))
                && self.functions.get(symbol).is_some_and(|declaration| {
                    mojito_symbol::symbol::is_list_literal_constructor(
                        &declaration.param_names,
                        declaration.variadic.is_some(),
                    )
                })
        })
    }
}

/// Rewrite each nullary `T()`, `Ts[i]()`, or construction of a type
/// expression (`types[i]()` over a reflected field-type list) whose
/// constructed type these bindings decide into that type's default
/// construction ([`default_construction`]). A construction they leave open
/// — an element whose `comptime for` index is not yet unrolled — stays for
/// the copy that binds it.
pub(super) fn default_construct_parameters(
    blocks: &mut [MirBlock],
    n_regs: &mut u32,
    reg_types: &mut HashMap<u32, Ty>,
    bindings: &Bindings,
) -> Result<(), MonoError> {
    for block in blocks {
        let mut index = 0;
        while index < block.instrs.len() {
            let constructed = match &mut block.instrs[index] {
                MirInstr::ConstructTypeParam {
                    dest,
                    param,
                    kwargs,
                    element,
                    ..
                } if kwargs.is_empty() => {
                    constructed_type(param, element.as_ref(), bindings)?.map(|ty| (*dest, ty))
                }
                MirInstr::ConstructType { dest, ty } => substitute_ty(ty, bindings)
                    .ok()
                    .filter(|ty| !mojito_types::types::is_symbolic(ty))
                    .map(|ty| (*dest, ty)),
                MirInstr::Try {
                    body,
                    handler,
                    orelse,
                    finalbody,
                    ..
                } => {
                    for region in std::iter::once(body)
                        .chain(handler.iter_mut().map(|(_, blocks)| blocks))
                        .chain(orelse.iter_mut())
                        .chain(finalbody.iter_mut())
                    {
                        default_construct_parameters(region, n_regs, reg_types, bindings)?;
                    }
                    None
                }
                _ => None,
            };
            let Some((dest, ty)) = constructed else {
                index += 1;
                continue;
            };
            let built = default_construction(dest, &ty, n_regs, reg_types)?;
            let count = built.len();
            block.instrs.splice(index..=index, built);
            index += count;
        }
    }
    Ok(())
}

/// Substitute `bindings` through `function`, whose declaration declares the
/// compile-time parameters in `scope`. Each read of a value parameter is its
/// typed constant, an `Array` display's its construction
/// ([`construct_parameter_reads`]) with the initializer `decls` declares. A
/// slot of a value owning storage holds nothing, so no place of it may
/// remain.
pub(super) fn substitute_function(
    function: &mut MirFunction,
    bindings: &Bindings,
    scope: &[ParamDecl],
    decls: ConstructionDecls,
) -> Result<(), MonoError> {
    let (constructed, locals): (HashMap<_, _>, HashMap<_, _>) =
        bound_parameter_locals(scope, bindings)
            .into_iter()
            .partition(|(name, value)| is_array_display(value, parameter_slot_ty(function, name)));
    substitute_value_parameter_reads(
        &mut function.blocks,
        &function.var_names,
        &function.var_tys,
        &locals,
        &bindings.callables,
    )?;
    seed_captured_parameter_slots(function, &locals, &bindings.callables);
    for (var, name) in function.var_names.iter().enumerate() {
        if let Some(value) = locals.get(name.as_str())
            && value.is_constructed_parameter_value()
        {
            reject_parameter_storage_reads(&function.blocks, var as u32, value)?;
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
    for (var, name) in function.var_names.iter().enumerate() {
        let Some(value) = constructed.get(name.as_str()) else {
            continue;
        };
        let var = var as u32;
        let ty = function
            .var_tys
            .get(&var)
            .cloned()
            .ok_or_else(|| MonoError {
                kind: MonoErrorKind::Unsupported,
                function: None,
                construct: format!("value parameter `{value}` has no declared type"),
            })?;
        construct_parameter_reads(
            &mut function.blocks,
            var,
            value,
            &ty,
            &mut function.n_regs,
            &mut function.reg_types,
            decls,
        )?;
        reject_parameter_storage_reads(&function.blocks, var, value)?;
    }
    substitute_blocks_metadata(&mut function.blocks, bindings)?;
    check_reinterpretation_widths(&function.blocks, &function.reg_types)?;
    copy_builtin_receivers(&mut function.blocks, &function.reg_types);
    Ok(())
}

/// Rewrite each dispatched `copy()` whose receiver this instance makes a
/// built-in with a value-read copy into that value copy. `Copyable.copy` is
/// `Self(copy=self)`, so at such a `Self` it is the [`MirInstr::CopyValue`]
/// the `T(copy=x)` arm of [`substitute_instruction`] writes. The copy is no
/// call, so a binding of its result is no longer in place.
pub(super) fn copy_builtin_receivers(blocks: &mut [MirBlock], reg_types: &HashMap<u32, Ty>) {
    for block in blocks {
        let mut copied = HashSet::new();
        for instruction in &mut block.instrs {
            match instruction {
                MirInstr::MethodCall {
                    dest,
                    recv,
                    method,
                    resolved,
                    raises: None,
                    args,
                    kwargs,
                    ..
                } if method == "copy"
                    && args.is_empty()
                    && kwargs.is_empty()
                    && resolved
                        .as_deref()
                        .is_none_or(|target| target.starts_with("__trait_dispatch."))
                    && reg_types.get(&recv.0).is_some_and(|ty| {
                        mojito_types::types::builtin_copy_is_value_read(ty.kind())
                    }) =>
                {
                    copied.insert(dest.0);
                    *instruction = MirInstr::CopyValue {
                        dest: *dest,
                        value: *recv,
                    };
                }
                MirInstr::DefVar { src, in_place, .. } if copied.contains(&src.0) => {
                    *in_place = false;
                }
                MirInstr::Try {
                    body,
                    handler,
                    orelse,
                    finalbody,
                    ..
                } => {
                    for region in std::iter::once(body)
                        .chain(handler.iter_mut().map(|(_, blocks)| blocks))
                        .chain(orelse.iter_mut())
                        .chain(finalbody.iter_mut())
                    {
                        copy_builtin_receivers(region, reg_types);
                    }
                }
                _ => {}
            }
        }
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
                        kind: MonoErrorKind::Unsupported,
                        function: None,
                        construct: format!("unsupported runtime value parameter `{value}`"),
                    })?
                };
                *instruction = MirInstr::Const {
                    dest: *dest,
                    k: constant,
                };
            } else if let MirInstr::LoadPlace { dest, place } = instruction
                && let Some(value) = var_names
                    .get(place.root as usize)
                    .and_then(|name| locals.get(name))
                && let Some(constant) = if place.is_whole_root() {
                    value_parameter_constant(value, var_tys.get(&place.root))
                } else {
                    projected_parameter_constant(value, &place.storage().proj, place.ty.as_ref())
                }
            {
                // A whole aggregate parameter read as a place (`h(p)`), or a
                // field of a struct-typed one (`e.rows`).
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

/// Construct the `Array` display `value` wherever `blocks` read its slot
/// `slot` — a `comptime for` binder's or a value parameter's — at the
/// read's type (`ty`, the slot's, where the read has none)
/// ([`array_display_construction`]), whose result the read's consumer — a
/// materialized temporary or a copy — owns.
pub(super) fn construct_parameter_reads(
    blocks: &mut [MirBlock],
    slot: u32,
    value: &CtValue,
    ty: &Ty,
    n_regs: &mut u32,
    reg_types: &mut HashMap<u32, Ty>,
    decls: ConstructionDecls,
) -> Result<(), MonoError> {
    for block in blocks {
        let mut instrs = Vec::with_capacity(block.instrs.len());
        for mut instruction in std::mem::take(&mut block.instrs) {
            if let MirInstr::UseVar { dest, var, .. } = instruction
                && var == slot
            {
                let read = reg_types
                    .get(&dest.0)
                    .cloned()
                    .unwrap_or_else(|| ty.clone());
                instrs.extend(array_display_construction(
                    dest, value, &read, n_regs, reg_types, decls,
                )?);
                continue;
            }
            if let MirInstr::Try {
                body,
                handler,
                orelse,
                finalbody,
                ..
            } = &mut instruction
            {
                for region in std::iter::once(body)
                    .chain(handler.iter_mut().map(|(_, blocks)| blocks))
                    .chain(orelse.iter_mut())
                    .chain(finalbody.iter_mut())
                {
                    construct_parameter_reads(region, slot, value, ty, n_regs, reg_types, decls)?;
                }
            }
            instrs.push(instruction);
        }
        block.instrs = instrs;
    }
    Ok(())
}

/// Reject what is left of the slot `slot` of the compile-time parameter
/// `value` once its reads are materialized: a place rooted at the slot, or
/// a drop of it, remains only where the check or drop elaboration treated
/// the parameter as storage, and is an error when the value owns storage
/// its slot does not hold.
pub(super) fn reject_parameter_storage_reads(
    blocks: &[MirBlock],
    slot: u32,
    value: &CtValue,
) -> Result<(), MonoError> {
    for instruction in blocks.iter().flat_map(|block| &block.instrs) {
        if let MirInstr::Try {
            body,
            handler,
            orelse,
            finalbody,
            ..
        } = instruction
        {
            for region in std::iter::once(body)
                .chain(handler.iter().map(|(_, blocks)| blocks))
                .chain(orelse)
                .chain(finalbody)
            {
                reject_parameter_storage_reads(region, slot, value)?;
            }
        } else if matches!(
            instruction,
            MirInstr::DropVar { var } | MirInstr::ConsumeVar { var } if *var == slot
        ) || mojito_mir::mir::verify::instruction_places(instruction)
            .iter()
            .any(|place| place.root == slot || place.through == Some(slot))
        {
            return Err(MonoError {
                kind: MonoErrorKind::Unsupported,
                function: None,
                construct: format!(
                    "a place read of the compile-time parameter `{value}`, which holds no storage"
                ),
            });
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
            && !matches!(ty.kind(), TyKind::RuntimePack(_) | TyKind::Tuple(_))
        {
            *ty = TyKind::RuntimePack(vec![ty.clone(); arity]).into();
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
        BinOp, Call, CallIndirect, Const, ConstructTypeParam, ConsumePlace, DefVar, DropPlace,
        EstablishLoans, Index, LoadPlace, MakeClosure, MakeRef, MakeSimd, MakeTuple, MakeVariant,
        MarkDestroyed, MarkInitialized, MaterializeLiteral, MethodCall, MovePlace, MultiIndex,
        MultiSet, PointerStorageDestroy, PointerStorageTake, SimdBitcast, SimdCast, SimdShuffle,
        SizeOf, Slice, Store, StoreRef, Try, TryNext, TypeName, UnOp, UninitStorageDestroy,
        UninitStorageTake, VariantReplace, VariantSet, VariantSetInitWith,
    };
    close_instruction_variant_indices(instruction, bindings)?;
    match instruction {
        // A SIMD instruction's slots close as the vector type they build
        // does; one the bindings leave symbolic stays for the concreteness
        // check to name.
        MakeSimd { dtype, width, .. }
        | SimdCast { dtype, width, .. }
        | SimdBitcast { dtype, width, .. } => {
            let built = substitute_ty(
                &Ty::new(TyKind::Simd {
                    dtype: dtype.clone(),
                    width: width.clone(),
                }),
                bindings,
            )?;
            if let Some((closed_dtype, closed_width)) = mojito_types::types::simd_slots(&built) {
                *dtype = closed_dtype;
                *width = closed_width;
            }
        }
        // A template's lane mask closes part by part; the instance checks
        // the closed mask against its receiver (`close_lane_masks`).
        SimdShuffle { mask, .. } => {
            *mask = mask.close_with(&|expr| match eval_ct(expr, bindings) {
                Ok(CtValue::Int(value)) => Some(value),
                _ => None,
            });
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
        | DropPlace { place }
        | MarkInitialized { place }
        | MarkDestroyed { place } => substitute_place(place, bindings)?,
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
            sub_opt_ty(raises.as_mut(), bindings)?;
            sub_places(arg_places, bindings)?;
            sub_places(kwarg_places, bindings)?;
            sub_opt_ty(receiver.as_mut(), bindings)?;
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
        // `T(copy=x)` copies its source through the bound type's copy
        // initializer: a built-in value copies by the value read, a struct
        // by the ordinary `Name(copy=x)` construction.
        ConstructTypeParam {
            dest,
            param,
            kwargs,
            kwarg_places,
            ..
        } if !kwargs.is_empty() => {
            let binding = bindings.types.get(&*param);
            let source = match kwargs.as_slice() {
                [(keyword, source)] if keyword == "copy" => *source,
                _ => {
                    return Err(MonoError {
                        kind: MonoErrorKind::Unsupported,
                        function: None,
                        construct: format!(
                            "constructing type parameter `{}` through an initializer other than `copy=`",
                            param.name
                        ),
                    });
                }
            };
            match binding.map(Ty::kind) {
                Some(ty) if mojito_types::types::builtin_copy_is_value_read(ty) => {
                    *instruction = MirInstr::CopyValue {
                        dest: *dest,
                        value: source,
                    };
                }
                Some(TyKind::Struct(struct_name, _)) => {
                    sub_places(kwarg_places, bindings)?;
                    *instruction = Call {
                        dest: *dest,
                        func: mojito_mir::mir::FuncRef::named(struct_name),
                        raises: None,
                        args: Vec::new(),
                        kwargs: std::mem::take(kwargs),
                        arg_places: Vec::new(),
                        kwarg_places: std::mem::take(kwarg_places),
                        capture_accesses: Vec::new(),
                        param_arg_regs: Vec::new(),
                        receiver: None,
                        instantiated_args: Vec::new(),
                        spread: None,
                    };
                }
                _ => {
                    return Err(MonoError {
                        kind: MonoErrorKind::Unsupported,
                        function: None,
                        construct: format!(
                            "copying through type parameter `{}` without a concrete binding",
                            param.name
                        ),
                    });
                }
            }
        }
        // Every construction the bindings decide was written as its type's
        // default construction (`default_construct_parameters`). One left
        // sits in a compile-time branch not yet selected, or its type stays
        // symbolic for the verifier's concrete mode to name.
        MirInstr::ConstructType { ty, .. } => {
            if let Ok(closed) = substitute_ty(ty, bindings) {
                *ty = closed;
            }
        }
        // Every nullary construction the bindings decide was written as its
        // type's default construction (`default_construct_parameters`).
        ConstructTypeParam { param, element, .. } => {
            return Err(MonoError {
                kind: MonoErrorKind::Unsupported,
                function: None,
                construct: match element {
                    Some(_) => format!(
                        "constructing an element of pack `{}` at an index the instance does not decide",
                        param.name
                    ),
                    None => format!(
                        "constructing type parameter `{}` without a concrete binding",
                        param.name
                    ),
                },
            });
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
            sub_opt_ty(raises.as_mut(), bindings)?;
            sub_place_opt(callee_place.as_deref_mut(), bindings)?;
            sub_places(arg_places, bindings)?;
            sub_places(kwarg_places, bindings)?;
            sub_opt_ty(instantiated_contract.as_mut(), bindings)?;
            for arg in instantiated_args {
                *arg = substitute_arg(arg, bindings)?;
            }
        }
        UnOp { raises, .. } | BinOp { raises, .. } => sub_opt_ty(raises.as_mut(), bindings)?,
        MethodCall {
            raises,
            reference_result,
            recv_place,
            arg_places,
            kwarg_places,
            instantiated_args,
            ..
        } => {
            sub_opt_ty(raises.as_mut(), bindings)?;
            sub_ref_opt(reference_result.as_deref_mut(), bindings)?;
            sub_place_opt(recv_place.as_deref_mut(), bindings)?;
            sub_places(arg_places, bindings)?;
            sub_places(kwarg_places, bindings)?;
            for arg in instantiated_args {
                *arg = substitute_arg(arg, bindings)?;
            }
        }
        Index {
            base_place,
            index_place,
            call,
            ..
        } => {
            sub_place_opt(base_place.as_deref_mut(), bindings)?;
            sub_place_opt(index_place.as_deref_mut(), bindings)?;
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
            sub_place_opt(object_place.as_deref_mut(), bindings)?;
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
            sub_place_opt(receiver_place.as_deref_mut(), bindings)?;
            sub_places(arg_places, bindings)?;
            sub_place_opt(value_place.as_deref_mut(), bindings)?;
            substitute_subscript_call(call, bindings)?;
        }
        MakeTuple {
            element_types: Some(types),
            ..
        } => {
            for ty in types {
                *ty = substitute_ty(ty, bindings)?;
            }
        }
        MakeVariant { alternatives, .. } => {
            *alternatives = sub_spread_types(alternatives, bindings)?;
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
    sub_opt_ty(call.raises.as_mut(), bindings)?;
    call.result_ty = substitute_ty(&call.result_ty, bindings)?;
    // A parameter type closes under the caller's bindings when the checker
    // instantiated it at the call; one naming the callee's own binder stays
    // as the contract wrote it.
    for argument in &mut call.arguments {
        if let Ok(ty) = substitute_ty(&argument.parameter_ty, bindings) {
            argument.parameter_ty = ty;
        }
    }
    // A compile-time index over binders the bindings close (a `comptime
    // for` index in its iteration's copy) is its value from here.
    for argument in &mut call.param_arg_regs {
        if let Some(value) = argument
            .expr
            .as_ref()
            .and_then(|expr| eval_ct(expr, bindings).ok())
            .and_then(|value| ParamContext::detached().constant(value).ok())
        {
            argument.expr = Some(value);
        }
    }
    sub_ref_opt(call.reference_result.as_mut(), bindings)
}

pub(super) fn substitute_place(place: &mut MirPlace, bindings: &Bindings) -> Result<(), MonoError> {
    sub_opt_ty(place.root_ty.as_mut(), bindings)?;
    for ty in &mut place.projection_tys {
        *ty = substitute_ty(ty, bindings)?;
    }
    for projection in &mut place.proj {
        if let Proj::Variant(index) = projection {
            close_variant_index(index, bindings)?;
        }
    }
    sub_opt_ty(place.ty.as_mut(), bindings)
}

/// Close the alternatives a `Variant` operation selects.
fn close_instruction_variant_indices(
    instruction: &mut MirInstr,
    bindings: &Bindings,
) -> Result<(), MonoError> {
    match instruction {
        MirInstr::MakeVariant { index, .. }
        | MirInstr::VariantIs { index, .. }
        | MirInstr::VariantGet { index, .. }
        | MirInstr::VariantSet { index, .. }
        | MirInstr::VariantTake { index, .. }
        | MirInstr::VariantSetInitWith { index, .. }
        | MirInstr::VariantDeinitWith { index, .. } => close_variant_index(index, bindings),
        MirInstr::VariantReplace {
            input_index,
            output_index,
            ..
        } => {
            close_variant_index(input_index, bindings)?;
            close_variant_index(output_index, bindings)
        }
        _ => Ok(()),
    }
}

/// Close a template's `_get_type_index[T, *Ts]()` once the instance binds
/// the pack and `T`. A `T` the pack lacks fails the instance, as upstream's
/// `Variant._check[T]()` does; an index the bindings leave open stays for
/// the concreteness check to name.
fn close_variant_index(index: &mut VariantIndex, bindings: &Bindings) -> Result<(), MonoError> {
    let VariantIndex::Expr(expr) = &*index else {
        return Ok(());
    };
    let ParamKind::PackQuery {
        pack,
        query: PackQuery::IndexOf(element),
    } = expr.kind()
    else {
        return Ok(());
    };
    let Some(TyKind::RuntimePack(alternatives)) = bindings.types.get(pack).map(Ty::kind) else {
        return Ok(());
    };
    let alternatives = sub_types(alternatives, bindings)?;
    let selected = match element.kind() {
        ParamKind::TypeShape(ty) => substitute_ty(ty, bindings)?,
        ParamKind::Constant(CtValue::Type(ty)) => ty.clone(),
        _ => return Ok(()),
    };
    if is_symbolic(&selected) || alternatives.iter().any(is_symbolic) {
        return Ok(());
    }
    *index = VariantIndex::Known(
        alternatives
            .iter()
            .position(|alternative| super::equiv::ty_equal_modulo_origins(alternative, &selected))
            .ok_or_else(|| MonoError {
                kind: MonoErrorKind::Instantiation,
                function: None,
                construct: format!(
                    "constraint failed: Type does not exist in Variant. ('{selected}' is not \
                     one of {})",
                    TyKind::Variant(alternatives.clone())
                ),
            })?,
    );
    Ok(())
}
pub(super) fn sub_places(
    places: &mut [Option<MirPlace>],
    bindings: &Bindings,
) -> Result<(), MonoError> {
    for place in places {
        sub_place_opt(place.as_mut(), bindings)?;
    }
    Ok(())
}
pub(super) fn sub_place_opt(
    place: Option<&mut MirPlace>,
    bindings: &Bindings,
) -> Result<(), MonoError> {
    if let Some(place) = place {
        substitute_place(place, bindings)?;
    }
    Ok(())
}
pub(super) fn sub_opt_ty(ty: Option<&mut Ty>, bindings: &Bindings) -> Result<(), MonoError> {
    if let Some(ty) = ty {
        *ty = substitute_ty(ty, bindings)?;
    }
    Ok(())
}
pub(super) fn sub_ref_opt(
    ty: Option<&mut mojito_types::origin::RefTy>,
    bindings: &Bindings,
) -> Result<(), MonoError> {
    if let Some(ty) = ty {
        ty.referent = substitute_ty(&ty.referent, bindings)?;
    }
    Ok(())
}

pub(super) fn substitute_iterator_call(
    call: &mut mojito_checked::checked::CheckedIteratorCall,
    bindings: &Bindings,
) -> Result<(), MonoError> {
    call.result_ty = substitute_ty(&call.result_ty, bindings)?;
    sub_opt_ty(call.raises.as_mut(), bindings)?;
    sub_ref_opt(call.reference_result.as_mut(), bindings)?;
    Ok(())
}

/// A closed struct type's source declaration, with its own parameters bound
/// at the type's arguments.
pub(super) struct StructInstance<'b> {
    /// The source declaration's name, which a generic instance's symbol
    /// extends.
    pub(super) template: &'b str,
    pub(super) shape: &'b StructShape,
    /// The bindings a member type over the struct's own parameters
    /// substitutes under.
    pub(super) bindings: Bindings,
}

/// The source struct `ty` instantiates, read off the shapes `bindings`
/// carries. `None` where `ty` is no struct the source declares.
pub(super) fn struct_instance<'b>(
    ty: &'b Ty,
    bindings: &'b Bindings,
) -> Result<Option<StructInstance<'b>>, MonoError> {
    let TyKind::Struct(struct_name, struct_args) = ty.kind() else {
        return Ok(None);
    };
    let Some((template, shape)) = [struct_name.as_str(), nominal_template(struct_name)]
        .into_iter()
        .find_map(|name| Some((name, bindings.structs.shape(name)?)))
    else {
        return Ok(None);
    };
    let mut instance = Bindings {
        structs: Rc::clone(&bindings.structs),
        self_instance: Some((template.to_string(), ty.clone())),
        applications: bindings.applications.clone(),
        ..Bindings::default()
    };
    bind_ty_args(&shape.param_decls, struct_args, &mut instance).map_err(|construct| {
        MonoError {
            kind: MonoErrorKind::Unsupported,
            function: None,
            construct,
        }
    })?;
    apply_defaults(&shape.param_decls, &mut instance)?;
    Ok(Some(StructInstance {
        template,
        shape,
        bindings: instance,
    }))
}

pub(super) fn substitute_ty(ty: &Ty, bindings: &Bindings) -> Result<Ty, MonoError> {
    let unsupported = |what: String| MonoError {
        kind: MonoErrorKind::Unsupported,
        function: None,
        construct: what,
    };
    Ok(match ty.kind() {
        TyKind::Param { binder, .. } => {
            bindings.types.get(binder).cloned().ok_or_else(|| {
                unsupported(format!("unresolved type parameter `{}`", binder.name))
            })?
        }
        TyKind::Struct(name, original) => {
            if original.is_empty() {
                // The bare in-body `self` spelling of a generic owner resolves
                // to the concrete instance being materialized; other bare
                // names are non-generic (or unresolvable, failing later).
                if let Some((template, concrete)) = &bindings.self_instance
                    && template == name
                {
                    return Ok(concrete.clone());
                }
                // `Tuple` applied to no element is the empty tuple's
                // instance, not the template.
                if mojito_types::types::binds_pack_elementwise(name)
                    && bindings.structs.is_generic(name)
                {
                    return Ok(TyKind::Struct(
                        mojito_symbol::symbol::instance_symbol(name, &[]),
                        Vec::new(),
                    )
                    .into());
                }
                return Ok(TyKind::Struct(name.clone(), Vec::new()).into());
            }
            // A spread of a bound pack is a whole argument list: element by
            // element for `Tuple` and `TString`, one bound list for any other
            // struct, as `expand_pack_spread` spells it.
            let spread = match (mojito_types::types::pack_spread_argument(original)).map(Ty::kind) {
                Some(TyKind::Param { binder, .. }) => {
                    match bindings.types.get(binder).map(Ty::kind) {
                        Some(TyKind::RuntimePack(elements)) => Some(sub_types(elements, bindings)?),
                        _ => None,
                    }
                }
                _ => None,
            };
            let args = match spread {
                Some(elements) if mojito_types::types::binds_pack_elementwise(name) => {
                    elements.into_iter().map(TyArg::Ty).collect()
                }
                Some(elements) => vec![TyArg::Val(CtValue::Tuple(
                    elements.into_iter().map(CtValue::Type).collect(),
                ))],
                // A spread of a list computed from packs is that list's
                // elements under the instance's bindings.
                None if let Some(list) = mojito_types::types::list_spread_argument(original) => {
                    let closed = ParamContext::detached()
                        .replace(list, &ct_bindings(bindings))
                        .and_then(|list| mojito_types::types::spread_arguments(name, &list))
                        .map_err(|error| unsupported(error.to_string()))?;
                    if mojito_types::types::list_spread_argument(&closed).is_some() {
                        return Err(unsupported(format!(
                            "spread list `{list}` has no concrete elements"
                        )));
                    }
                    closed
                        .iter()
                        .map(|arg| substitute_arg(arg, bindings))
                        .collect::<Result<Vec<_>, _>>()?
                }
                None => original
                    .iter()
                    .map(|arg| substitute_arg(arg, bindings))
                    .collect::<Result<Vec<_>, _>>()?,
            };
            // A literal type argument (`Tuple[IntLiteral, String]`, the type
            // of `(1, "a")`) names the instance its materialized form names.
            let args: Vec<TyArg> = args
                .into_iter()
                .map(|argument| match argument {
                    TyArg::Ty(ty) => TyArg::Ty(mojito_types::types::default_literal(&ty)),
                    other => other,
                })
                .collect();
            // Every concrete application of a generic template takes its
            // instance symbol, so distinct instantiations get distinct output
            // declarations. Checker-specialized structs (empty `param_decls`)
            // and already-renamed instances keep their names; symbolic
            // applications stay for a later substitution or a contextual
            // rejection.
            let concrete_name = if args.iter().any(arg_has_symbolic)
                || nominal_template(name) != name
                || !bindings.structs.is_generic(name)
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
            ty.reusing(TyKind::Struct(concrete_name, args))
        }
        TyKind::Tuple(v) => TyKind::Tuple(sub_spread_types(v, bindings)?).into(),
        TyKind::RuntimePack(v) => TyKind::RuntimePack(sub_spread_types(v, bindings)?).into(),
        TyKind::Variant(v) => TyKind::Variant(sub_spread_types(v, bindings)?).into(),
        TyKind::Overload(v) => TyKind::Overload(sub_types(v, bindings)?).into(),
        TyKind::ComptimeList(v) => TyKind::ComptimeList(substitute_ty(v, bindings)?).into(),
        // A collector over a bound type pack is the tuple of its elements.
        TyKind::VariadicPack(v)
            if let Some(TyKind::Param { binder, .. }) =
                (mojito_types::types::pack_spread(std::slice::from_ref(v))).map(Ty::kind)
                && let Some(TyKind::RuntimePack(elements)) =
                    bindings.types.get(binder).map(Ty::kind) =>
        {
            TyKind::Tuple(sub_types(elements, bindings)?).into()
        }
        TyKind::VariadicPack(v) => {
            let element = substitute_ty(v, bindings)?;
            match bindings.variadic_arity {
                // An unspecialized variadic callee instantiates at its
                // call-site arity: the pack becomes the concrete tuple shape
                // the body sees once the arguments are matched. The runtime
                // pack stays the declaration's variadic ABI root alone.
                Some(arity) => TyKind::Tuple(vec![element; arity]).into(),
                None => TyKind::VariadicPack(element).into(),
            }
        }
        TyKind::Pointer { element, origin } => TyKind::Pointer {
            element: substitute_ty(element, bindings)?,
            origin: origin.clone(),
        }
        .into(),
        TyKind::Ref(value) => {
            let mut value = value.clone();
            value.referent = substitute_ty(&value.referent, bindings)?;
            TyKind::Ref(value).into()
        }
        TyKind::Dependent(dependent) => {
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
                if !matches!(closed.kind(), TyKind::Dependent(_)) {
                    return substitute_ty(&closed, bindings);
                }
                // An element of a reflected field-type list
                // (`reflect[T].field_types()[i]`) closes once the instance
                // answers the query.
                return match eval_ct(dependent.expr(), bindings) {
                    Ok(CtValue::Type(element)) => substitute_ty(&element, bindings),
                    _ => Err(unsupported(format!(
                        "dependent type `{ty}` has no concrete MIR declaration fact"
                    ))),
                };
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
        TyKind::Assoc { base, name, args } => match bindings.associated.get(&ty.to_string()) {
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
        TyKind::GenericFunc(_) => ty.clone(),
        TyKind::SelfType | TyKind::Infer => {
            return Err(unsupported(format!("unresolved type `{ty}`")));
        }
        TyKind::Func(func) => {
            let FuncTy {
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
            } = &**func;
            TyKind::Func(Box::new(FuncTy {
                environment: environment.clone(),
                params: sub_types(params, bindings)?,
                names: names.clone(),
                ret: substitute_ty(ret, bindings)?,
                required: required.clone(),
                variadic: variadic
                    .as_ref()
                    .map(|t| substitute_ty(t, bindings))
                    .transpose()?,
                kw_variadic: kw_variadic
                    .as_ref()
                    .map(|t| substitute_ty(t, bindings))
                    .transpose()?,
                positional_only: *positional_only,
                keyword_only: *keyword_only,
                raises: *raises,
                error: error
                    .as_ref()
                    .map(|t| substitute_ty(t, bindings))
                    .transpose()?,
                conventions: conventions.clone(),
                ref_params: ref_params.clone(),
                ref_return: ref_return.clone(),
                transfers: transfers.clone(),
            }))
            .into()
        }
        // A symbolic lane or width the bindings close — a layout application
        // the instance's oracle answers — becomes the known slot; one they do
        // not close stays as spelled, for the concreteness check to name.
        TyKind::Simd { dtype, width } => TyKind::Simd {
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
        }
        .into(),
        _ => ty.clone(),
    })
}

/// [`sub_types`] over an element list that may be one spread of a bound
/// pack (`__RuntimeTuple[*Self.Ts]`): the spread is the pack's elements, as
/// `expand_pack_spread` spells it.
fn sub_spread_types(types: &[Ty], bindings: &Bindings) -> Result<Vec<Ty>, MonoError> {
    match (mojito_types::types::pack_spread(types)).map(Ty::kind) {
        Some(TyKind::Param { binder, .. })
            if let Some(TyKind::RuntimePack(elements)) =
                bindings.types.get(binder).map(Ty::kind) =>
        {
            sub_types(elements, bindings)
        }
        _ => sub_types(types, bindings),
    }
}

pub(super) fn substitute_arg(arg: &TyArg, bindings: &Bindings) -> Result<TyArg, MonoError> {
    Ok(match arg {
        TyArg::Ty(ty) => TyArg::Ty(substitute_ty(ty, bindings)?),
        // A residual closes under the mono environment or it is the
        // contextual unsupported boundary; nothing symbolic reaches lowering.
        TyArg::Val(CtValue::Expr(expr)) => TyArg::Val(eval_ct(expr, bindings)?),
        // A pack bound as one list of types closes type by type, each
        // materialized as an elementwise argument is.
        TyArg::Val(CtValue::Tuple(values))
            if values.iter().any(|value| matches!(value, CtValue::Type(_))) =>
        {
            TyArg::Val(CtValue::Tuple(
                values
                    .iter()
                    .map(|value| match value {
                        CtValue::Type(ty) => substitute_ty(ty, bindings)
                            .map(|ty| CtValue::Type(mojito_types::types::default_literal(&ty))),
                        other => Ok(other.clone()),
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            ))
        }
        TyArg::Val(value) => TyArg::Val(value.clone()),
        // A template's origin is its own binder or a place its body owns:
        // upstream keeps an instance's origin parameters unspecialized, and
        // the bindings carry none ([`Bindings::erase_origins`]).
        TyArg::Origin(origin) => TyArg::Origin(origin.clone()),
    })
}
pub(super) fn sub_types(types: &[Ty], bindings: &Bindings) -> Result<Vec<Ty>, MonoError> {
    types.iter().map(|ty| substitute_ty(ty, bindings)).collect()
}

/// The constant a read of the parameter value `value` at `slot_ty` is: a
/// scalar, a function named by a callable-typed slot, a string read at the
/// nominal `String` or a vector, struct, or tuple as the typed constant a
/// backend materializes at that type (upstream's `kgen.param.constant`),
/// and any other string as its literal.
pub(super) fn value_parameter_constant(value: &CtValue, slot_ty: Option<&Ty>) -> Option<Const> {
    match value {
        CtValue::Int(value) => Some(Const::Int(*value)),
        CtValue::UInt(value) => Some(Const::Value(CtValue::UInt(*value))),
        CtValue::Float(bits) => Some(Const::Float(f64::from_bits(*bits))),
        CtValue::Bool(value) => Some(Const::Bool(*value)),
        CtValue::Dtype(value) => Some(Const::Dtype(*value)),
        CtValue::Str(value)
            if matches!(
                slot_ty.map(Ty::kind),
                Some(TyKind::Func(_) | TyKind::GenericFunc(_))
            ) =>
        {
            Some(Const::Function(value.clone()))
        }
        CtValue::Str(_) if slot_ty.is_some_and(mojito_types::types::is_nominal_string) => {
            Some(Const::Value(value.clone()))
        }
        CtValue::Str(value) => Some(Const::Str(value.clone())),
        value @ (CtValue::Simd { .. } | CtValue::Struct { .. } | CtValue::Tuple(_)) => {
            Some(Const::Value(value.clone()))
        }
        _ => None,
    }
}

/// The display a parameter constant `k` read at `ty` is constructed from
/// rather than read as a constant: a list or tuple read at an `Array`
/// ([`array_display_construction`]).
pub(super) fn constructed_parameter_constant(k: &Const, ty: &Ty) -> Option<CtValue> {
    match k {
        Const::Value(value @ (CtValue::Tuple(_) | CtValue::List(_)))
            if array_instance_parts(ty).is_some() =>
        {
            Some(value.clone())
        }
        _ => None,
    }
}

/// Whether a parameter value bound to a slot of `ty` is an `Array` display,
/// constructed at each read ([`construct_parameter_reads`]).
pub(super) fn is_array_display(value: &CtValue, ty: Option<&Ty>) -> bool {
    matches!(value, CtValue::Tuple(_) | CtValue::List(_))
        && ty.is_some_and(|ty| array_instance_parts(ty).is_some())
}

/// The constant a chain of field projections selects out of a struct-typed
/// parameter value, read at `ty`, `None` unless every step names a field
/// of a closed struct value.
fn projected_parameter_constant(
    value: &CtValue,
    projections: &[mojito_mir::mir::Proj],
    ty: Option<&Ty>,
) -> Option<Const> {
    let value =
        projections
            .iter()
            .try_fold(value, |value, projection| match (projection, value) {
                (mojito_mir::mir::Proj::Field(field), CtValue::Struct { fields, .. }) => fields
                    .iter()
                    .find(|(name, _)| name == field)
                    .map(|(_, value)| value),
                _ => None,
            })?;
    (!projections.is_empty())
        .then(|| value_parameter_constant(value, ty))
        .flatten()
}

/// Seed, at the entry block, every folded value parameter slot a closure
/// captures. A folded slot is never stored, so the capture would borrow
/// uninitialized storage: it is stored its constant first. A slot is no
/// drop root, so a value owning a buffer is not seeded, and a string is
/// its literal.
fn seed_captured_parameter_slots(
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
                    .filter(|value| !value.is_constructed_parameter_value())
                    .and_then(|value| match value_parameter_constant(value, Some(&ty))? {
                        Const::Value(CtValue::Str(text)) => Some(Const::Str(text)),
                        constant => Some(constant),
                    })
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
            in_place: false,
        });
    }
    if let Some(entry) = function.blocks.first_mut() {
        entry.instrs.splice(0..0, initializers);
    }
}

/// The run-time construction of the parameter value `value`, of the concrete
/// type `ty`, into `dest`: a list at an `Array` is the list-literal
/// initializer over its elements (the call a display `[a, b]` lowers to),
/// and any other value its typed constant ([`value_parameter_constant`]).
pub(super) fn array_display_construction(
    dest: Reg,
    value: &CtValue,
    ty: &Ty,
    n_regs: &mut u32,
    reg_types: &mut HashMap<u32, Ty>,
    decls: ConstructionDecls,
) -> Result<Vec<MirInstr>, MonoError> {
    let unsupported = || MonoError {
        kind: MonoErrorKind::Unsupported,
        function: None,
        construct: format!("constructing the parameter value `{value}` at `{ty}`"),
    };
    match value {
        CtValue::Tuple(elements) | CtValue::List(elements)
            if let Some((element, length)) = array_instance_parts(ty) =>
        {
            if usize::try_from(length).ok() != Some(elements.len()) {
                return Err(unsupported());
            }
            let constructor = decls
                .list_literal_constructor(mojito_types::types::ARRAY_TYPE_NAME)
                .ok_or_else(unsupported)?;
            let parts = elements.iter().map(|value| (value, element.clone()));
            let (mut instrs, args) = constructed_parts(parts, n_regs, reg_types, decls)?;
            instrs.extend(array_literal_construction(
                dest,
                constructor,
                args,
                length,
                n_regs,
                reg_types,
            ));
            Ok(instrs)
        }
        _ => {
            let k = value_parameter_constant(value, Some(ty)).ok_or_else(unsupported)?;
            Ok(vec![MirInstr::Const { dest, k }])
        }
    }
}

/// The declared type of the slot `function` names `name`.
fn parameter_slot_ty<'f>(function: &'f MirFunction, name: &str) -> Option<&'f Ty> {
    let var = function.var_names.iter().position(|slot| slot == name)?;
    function.var_tys.get(&u32::try_from(var).ok()?)
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
/// promoted to a runtime parameter keeps its reads, and a `comptime for`
/// index, which a thunk reads as a parameter reference, has no local.
pub(super) fn bound_parameter_locals<'a>(
    scope: &[ParamDecl],
    bindings: &'a Bindings,
) -> HashMap<String, &'a CtValue> {
    scope
        .iter()
        .filter(|decl| matches!(decl, ParamDecl::Value { .. }))
        .map(ParamDecl::binder)
        .filter(|binder| !binder.id.is_comptime_for_binder())
        .filter(|binder| !bindings.runtime_callables.contains(binder))
        .filter_map(|binder| {
            let value = bindings.values.get(&binder)?;
            Some((binder.name.trim_start_matches('*').to_string(), value))
        })
        .collect()
}

/// The associated type `name` of the instance `base` resolves to, read off
/// the member its struct declares: the member's type under the instance's
/// own arguments and, for a parameterized member (`C.IteratorType[o]`), the
/// application's. `None` where `base` is no struct that declares the member.
fn declared_associated_type(
    base: &Ty,
    name: &str,
    args: &[TyArg],
    bindings: &Bindings,
) -> Result<Option<Ty>, MonoError> {
    let base = substitute_ty(base, bindings)?;
    let Some(instance) = struct_instance(peel_refs(&base), bindings)? else {
        return Ok(None);
    };
    if args.is_empty()
        && let Some((_, member)) = instance
            .shape
            .members
            .iter()
            .find(|(member, _)| member == name)
    {
        return substitute_ty(member, &instance.bindings).map(Some);
    }
    let Some(member) = instance
        .shape
        .parameterized
        .iter()
        .find(|member| member.name == name)
    else {
        return Ok(None);
    };
    if member.params.len() != args.len() {
        return Ok(None);
    }
    let mut member_bindings = instance.bindings.clone();
    let mut origins = HashMap::new();
    for (param, arg) in member.params.iter().zip(args) {
        match (param, arg) {
            (AssociatedParameter::Type(binder), TyArg::Ty(ty)) => {
                member_bindings
                    .types
                    .insert(binder.clone(), substitute_ty(ty, bindings)?);
            }
            (AssociatedParameter::Origin(id), TyArg::Origin(origin)) => {
                origins.insert(id.0, origin.clone());
            }
            _ => {
                return Err(MonoError {
                    kind: MonoErrorKind::Unsupported,
                    function: None,
                    construct: format!(
                        "associated type `{base}.{name}`: a value parameter of an associated \
                         type is not elaborated"
                    ),
                });
            }
        }
    }
    let template = mojito_types::types::substitute_origin_params(&member.template, &origins);
    substitute_ty(&template, &member_bindings).map(Some)
}

/// The type a nullary construction builds under `bindings`: the binder's
/// own binding, or for `Ts[i]()` the element its evaluated index selects
/// from the pack's. `None` while either is still symbolic.
fn constructed_type(
    param: &ParamRef,
    element: Option<&mojito_mir::mir::MirParamArg>,
    bindings: &Bindings,
) -> Result<Option<Ty>, MonoError> {
    let Some(bound) = bindings.types.get(param) else {
        return Ok(None);
    };
    let Some(element) = element else {
        return Ok((!mojito_types::types::is_symbolic(bound)).then(|| bound.clone()));
    };
    let TyKind::RuntimePack(elements) = bound.kind() else {
        return Err(MonoError {
            kind: MonoErrorKind::Unsupported,
            function: None,
            construct: format!(
                "pack `{}` is bound to the non-pack type `{bound}`",
                param.name
            ),
        });
    };
    let Some(index) = element
        .expr
        .as_ref()
        .and_then(|expr| eval_ct(expr, bindings).ok())
        .as_ref()
        .and_then(mojito_types::param_expr::fold::integer_value)
        .and_then(|value| value.to_i64())
        .and_then(|value| usize::try_from(value).ok())
    else {
        return Ok(None);
    };
    let chosen = elements.get(index).ok_or_else(|| MonoError {
        kind: MonoErrorKind::Unsupported,
        function: None,
        construct: format!(
            "element {index} of pack `{}` is out of range for its {} elements",
            param.name,
            elements.len()
        ),
    })?;
    substitute_ty(chosen, bindings).map(Some)
}

/// The default construction of the concrete `ty` into `dest`: a scalar's
/// zero value (the VM's `ConstructTypeParam` answer for the built-in
/// `Defaultable` types); a SIMD type's zero vector, a splat of one zero lane
/// held in a fresh register, as `Defaultable` admits every SIMD value and a
/// single instruction cannot spell the splat; or a struct's nullary
/// constructor call, which the call rewriting then instantiates.
fn default_construction(
    dest: Reg,
    ty: &Ty,
    n_regs: &mut u32,
    reg_types: &mut HashMap<u32, Ty>,
) -> Result<Vec<MirInstr>, MonoError> {
    use mojito_ast::ast::Dtype;
    use mojito_mir::mir::Const;
    let zero = match ty.kind() {
        TyKind::Int | TyKind::IntLiteral | TyKind::UInt => Some(Const::Int(0)),
        TyKind::Bool => Some(Const::Bool(false)),
        TyKind::Float64 | TyKind::FloatLiteral => Some(Const::Float(0.0)),
        TyKind::StringLiteral => Some(Const::Str(String::new())),
        TyKind::None => Some(Const::None),
        _ => None,
    };
    if let Some(k) = zero {
        return Ok(vec![MirInstr::Const { dest, k }]);
    }
    match ty.kind() {
        TyKind::Simd { dtype, width } => {
            let (Some(dtype), Some(width)) = (dtype.known(), width.known()) else {
                return Err(MonoError {
                    kind: MonoErrorKind::Unsupported,
                    function: None,
                    construct: format!("default-constructing the open vector type `{ty}`"),
                });
            };
            let (zero, lane_ty) = match dtype {
                Dtype::Bool => (Const::Bool(false), Ty::bool()),
                Dtype::Float16 | Dtype::Float32 | Dtype::Float64 => {
                    (Const::Float(0.0), Ty::float64())
                }
                _ => (Const::Int(0), Ty::int()),
            };
            let lane = Reg(*n_regs);
            *n_regs += 1;
            reg_types.insert(lane.0, lane_ty);
            Ok(vec![
                MirInstr::Const {
                    dest: lane,
                    k: zero,
                },
                MirInstr::MakeSimd {
                    dest,
                    dtype: SimdDtype::Known(dtype),
                    width: SimdWidth::Known(width),
                    elems: vec![lane],
                },
            ])
        }
        TyKind::Struct(struct_name, _) => Ok(vec![plain_call(dest, struct_name, Vec::new())]),
        _ => Err(MonoError {
            kind: MonoErrorKind::Unsupported,
            function: None,
            construct: format!("default-constructing `{ty}`, which is no struct"),
        }),
    }
}

/// A call of `func` over the positional `args`, with nothing else bound,
/// which the call rewriting then resolves and instantiates.
fn plain_call(dest: Reg, func: &str, args: Vec<Reg>) -> MirInstr {
    MirInstr::Call {
        dest,
        func: mojito_mir::mir::FuncRef::named(func),
        raises: None,
        arg_places: vec![None; args.len()],
        args,
        kwargs: Vec::new(),
        kwarg_places: Vec::new(),
        capture_accesses: Vec::new(),
        param_arg_regs: Vec::new(),
        receiver: None,
        instantiated_args: Vec::new(),
        spread: None,
    }
}

/// Construct each part of a parameter value at its type into a fresh
/// register: the instructions, and the registers in order.
fn constructed_parts<'v>(
    parts: impl IntoIterator<Item = (&'v CtValue, Ty)>,
    n_regs: &mut u32,
    reg_types: &mut HashMap<u32, Ty>,
    decls: ConstructionDecls,
) -> Result<(Vec<MirInstr>, Vec<Reg>), MonoError> {
    let mut instrs = Vec::new();
    let mut args = Vec::new();
    for (part, part_ty) in parts {
        let arg = fresh_register(part_ty.clone(), n_regs, reg_types);
        instrs.extend(array_display_construction(
            arg, part, &part_ty, n_regs, reg_types, decls,
        )?);
        args.push(arg);
    }
    Ok((instrs, args))
}

/// The element type and length of an `Array` instance, a mono-named one
/// included.
fn array_instance_parts(ty: &Ty) -> Option<(&Ty, i64)> {
    let TyKind::Struct(name, arguments) = ty.kind() else {
        return None;
    };
    if nominal_template(name) != mojito_types::types::ARRAY_TYPE_NAME {
        return None;
    }
    match arguments.as_slice() {
        [TyArg::Ty(element), TyArg::Val(CtValue::Int(length))] => Some((element, *length)),
        _ => None,
    }
}

/// The list-literal initializer call over `elements`, as MIR lowers a
/// display: `__list_literal__` selects the overload, and the `length`
/// parameter argument reifies the constructed value's length.
fn array_literal_construction(
    dest: Reg,
    constructor: &str,
    elements: Vec<Reg>,
    length: i64,
    n_regs: &mut u32,
    reg_types: &mut HashMap<u32, Ty>,
) -> Vec<MirInstr> {
    let none = fresh_register(Ty::none(), n_regs, reg_types);
    let length_reg = fresh_register(Ty::int(), n_regs, reg_types);
    let unbound = || mojito_mir::mir::MirParamArg {
        name: None,
        value: None,
        expr: None,
    };
    vec![
        MirInstr::Const {
            dest: none,
            k: Const::None,
        },
        MirInstr::Const {
            dest: length_reg,
            k: Const::Int(length),
        },
        MirInstr::Call {
            dest,
            func: mojito_mir::mir::FuncRef::named(constructor),
            raises: None,
            arg_places: vec![None; elements.len()],
            args: elements,
            kwargs: vec![("__list_literal__".to_string(), none)],
            kwarg_places: vec![None],
            capture_accesses: Vec::new(),
            param_arg_regs: vec![
                unbound(),
                mojito_mir::mir::MirParamArg {
                    value: Some(length_reg),
                    ..unbound()
                },
            ],
            receiver: None,
            instantiated_args: Vec::new(),
            spread: None,
        },
    ]
}

fn fresh_register(ty: Ty, n_regs: &mut u32, reg_types: &mut HashMap<u32, Ty>) -> Reg {
    let reg = Reg(*n_regs);
    *n_regs += 1;
    reg_types.insert(reg.0, ty);
    reg
}

/// `SIMD.to_bits`' constraint, decided once an instance closes its source
/// lane: the target must be at least as wide, as the pin's instantiation
/// requires.
fn check_reinterpretation_widths(
    blocks: &[MirBlock],
    reg_types: &HashMap<u32, Ty>,
) -> Result<(), MonoError> {
    for block in blocks {
        for instruction in &block.instrs {
            match instruction {
                MirInstr::SimdBitcast {
                    value,
                    dtype: SimdDtype::Known(target),
                    ..
                } => {
                    if let Some(TyKind::Simd {
                        dtype: SimdDtype::Known(source),
                        ..
                    }) = reg_types.get(&value.0).map(Ty::kind)
                        && target.bit_width() < source.bit_width()
                    {
                        return Err(MonoError {
                            kind: MonoErrorKind::Unsupported,
                            function: None,
                            construct: format!(
                                "constraint failed: the target type `{}` of `to_bits` must be at least as wide as the source lane `{}`",
                                target.name(),
                                source.name()
                            ),
                        });
                    }
                }
                MirInstr::Try {
                    body,
                    handler,
                    orelse,
                    finalbody,
                    ..
                } => {
                    let regions = std::iter::once(body)
                        .chain(handler.iter().map(|(_, blocks)| blocks))
                        .chain(orelse.iter())
                        .chain(finalbody.iter());
                    for region in regions {
                        check_reinterpretation_widths(region, reg_types)?;
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}
