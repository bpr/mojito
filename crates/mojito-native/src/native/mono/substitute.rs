//! Binding substitution over functions, declarations, instructions,
//! places, and types.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_checked::checked::ComptimeSequence;
use mojito_checked::declaration::AssociatedLookupError;
use mojito_hir::hir::VarId;
use mojito_mir::mir::variadics::FrameTables;
use mojito_mir::mir::visit::{
    MutTypeVisitor, ParamArgSite, ParamDeclSite, TySite, walk_blocks_mut, walk_function_mut,
    walk_instruction_mut,
};
use mojito_mir::mir::{MirParamArg, Proj};
use mojito_types::param_expr::ParamRef;
use mojito_types::types::LaneMask;
use mojito_types::types::TyKind;
use mojito_types::types::{ArraySize, FuncTy, VariantIndex};

/// The declarations a reflected field-name list's construction names: the
/// list-literal initializer ([`array_display_construction`]).
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

/// The register and slot tables of the function an instance's
/// construction of a parameter value extends.
pub(super) struct FrameSlots<'a> {
    pub(super) n_regs: &'a mut u32,
    pub(super) reg_types: &'a mut HashMap<u32, Ty>,
    pub(super) n_vars: &'a mut usize,
    pub(super) var_names: &'a mut Vec<String>,
    pub(super) var_tys: &'a mut HashMap<VarId, Ty>,
}

impl FrameTables for FrameSlots<'_> {
    fn fresh_register(&mut self, ty: Ty) -> Reg {
        fresh_register(ty, self.n_regs, self.reg_types)
    }

    fn fresh_slot(&mut self, name: String, ty: Ty) -> VarId {
        let slot = VarId::try_from(*self.n_vars).expect("variable count fits a VarId");
        *self.n_vars += 1;
        self.var_names.push(name);
        self.var_tys.insert(slot, ty);
        slot
    }
}

/// Rewrite the type constructions these bindings decide into the bound
/// type's own ([`mojito_mir::mir::construct_type_parameters`]). A
/// construction they leave open — an element whose `comptime for` index is
/// not yet unrolled — stays for the copy that binds it.
pub(super) fn default_construct_parameters(
    blocks: &mut [MirBlock],
    bindings: &Bindings,
    declarations: &MirDeclarations,
) -> Result<(), MonoError> {
    mojito_mir::mir::construct_type_parameters(blocks, bindings, declarations)
}

/// Substitute `bindings` through `function`. Each read of a retained
/// callable runtime parameter is the function its argument names, and each
/// `conforms_to` query in a call's bracket argument is the `Bool`
/// `conforms` answers for it, as upstream's elaborator folds
/// `#kgen.type_conforms_to_trait` over a concrete type.
pub(super) fn substitute_function(
    function: &mut MirFunction,
    bindings: &Bindings,
    conforms: &ConformsAnswer<'_>,
) -> Result<(), MonoError> {
    substitute_callable_parameter_reads(
        &mut function.blocks,
        &function.var_names,
        &bindings.callables,
    );
    walk_function_mut(
        &mut Substitution {
            bindings,
            conforms: Some(conforms),
        },
        function,
    )?;
    copy_builtin_receivers(&mut function.blocks, &function.reg_types);
    Ok(())
}

/// What answers a `conforms_to` query at the instance a substitution
/// builds: whether the subject conforms to the named trait, or `None` for a
/// subject the instance leaves open.
pub(super) type ConformsAnswer<'a> = dyn Fn(&ParamExpr, &str) -> Option<bool> + 'a;

/// Rewrite each dispatched `copy()` whose receiver this instance makes a
/// built-in with a value-read copy into that value copy. `Copyable.copy` is
/// `Self(copy=self)`, so at such a `Self` it is the [`MirInstr::CopyValue`]
/// [`mojito_mir::mir::construct_type_parameters`] writes for `T(copy=x)`.
/// The copy is no call, so a binding of its result is no longer in place.
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
                        constructor: None,
                    };
                }
                MirInstr::DefVar { src, in_place, .. } if copied.contains(&src.0) => {
                    *in_place = false;
                }
                instruction => {
                    for region in mojito_mir::mir::instruction_regions_mut(instruction) {
                        copy_builtin_receivers(region, reg_types);
                    }
                }
            }
        }
    }
}

/// Rewrite each read of a retained callable runtime parameter's slot in
/// `blocks`, its regions included, into the function its argument names. A
/// read is a `UseVar` of the slot or a whole-slot `LoadPlace` (a `read`
/// parameter passed on to another call); a projected or `through` load is
/// not a read of the parameter's value. A callable binder's read is its
/// parameter constant, which the instance closes like any other.
pub(super) fn substitute_callable_parameter_reads(
    blocks: &mut [MirBlock],
    var_names: &[String],
    callables: &HashMap<String, String>,
) {
    for block in blocks {
        for instruction in &mut block.instrs {
            let read = match instruction {
                MirInstr::UseVar { dest, var, .. } => Some((*dest, *var)),
                MirInstr::LoadPlace { dest, place }
                    if place.through.is_none() && place.proj.is_empty() =>
                {
                    Some((*dest, place.root))
                }
                _ => None,
            };
            if let Some((dest, var)) = read
                && let Some(callable) = var_names
                    .get(var as usize)
                    .and_then(|name| callables.get(name))
            {
                *instruction = MirInstr::Const {
                    dest,
                    k: Const::Function(callable.clone()),
                };
            } else {
                for region in mojito_mir::mir::instruction_regions_mut(instruction) {
                    substitute_callable_parameter_reads(region, var_names, callables);
                }
            }
        }
    }
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
    }
    if let Some(ty) = &mut decl.kw_variadic {
        *ty = substitute_ty(ty, bindings)?;
    }
    decl.ret_ty = substitute_ty(&decl.ret_ty, bindings)?;
    if let Some(ty) = &mut decl.error_ty {
        *ty = substitute_ty(ty, bindings)?;
    }
    for default in decl.defaults.iter_mut().flatten() {
        if let CheckedConst::Construct {
            parameter: Some(ty),
            ..
        } = default
        {
            *ty = substitute_ty(ty, bindings)?;
        }
    }
    Ok(())
}

pub(super) fn substitute_blocks_metadata(
    blocks: &mut [MirBlock],
    bindings: &Bindings,
) -> Result<(), MonoError> {
    walk_blocks_mut(
        &mut Substitution {
            bindings,
            conforms: None,
        },
        blocks,
    )
}

/// A closed struct type's source declaration, with its own parameters bound
/// at the type's arguments.
pub(super) struct StructInstance<'b> {
    /// The source declaration's name, which a generic instance's symbol
    /// extends.
    pub(super) template: &'b str,
    pub(super) declaration: &'b MirStructDeclaration,
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
    let Some((struct_name, struct_args)) =
        mojito_types::types::nominal_parts(ty).filter(|_| !ty.is_simd())
    else {
        return Ok(None);
    };
    let Some((template, declaration)) = [struct_name, nominal_template(struct_name)]
        .into_iter()
        .find_map(|name| Some((name, bindings.structs.declaration(name)?)))
    else {
        return Ok(None);
    };
    let mut instance = Bindings {
        structs: Rc::clone(&bindings.structs),
        self_instance: Some((template.to_string(), ty.clone())),
        applications: bindings.applications.clone(),
        ..Bindings::default()
    };
    bind_ty_args(&declaration.param_decls, &struct_args, &mut instance).map_err(|construct| {
        MonoError {
            kind: MonoErrorKind::Unsupported,
            function: None,
            construct,
        }
    })?;
    apply_defaults(&declaration.param_decls, &[], &mut instance)?;
    Ok(Some(StructInstance {
        template,
        declaration,
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
        TyKind::Param { binder, .. } => match bindings.types.get(binder) {
            Some(ty) => ty.clone(),
            None if bindings.removed.contains(binder) => {
                return Err(MonoError {
                    kind: MonoErrorKind::Instantiation,
                    function: None,
                    construct: removed_parameter_read(binder),
                });
            }
            None => {
                return Err(unsupported(format!(
                    "unresolved type parameter `{}`",
                    binder.name
                )));
            }
        },
        struct_ty if let Some((name, original)) = struct_ty.as_non_simd_struct() => {
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
            // An argument that names a parameter the instance removed is
            // erased whole, as upstream's generator spells `_REMOVED_ARG`;
            // a struct type argument erases its own arguments instead.
            let erased: Vec<TyArg>;
            let original = if original.iter().any(|arg| erases_whole(arg, bindings)) {
                erased = original
                    .iter()
                    .map(|arg| {
                        if erases_whole(arg, bindings) {
                            TyArg::Val(CtValue::RemovedArg)
                        } else {
                            arg.clone()
                        }
                    })
                    .collect();
                &erased
            } else {
                original
            };
            // A spread of a bound pack is a whole argument list: element by
            // element for `Tuple` and `TString`, one bound list for any other
            // struct, as `expand_pack_spread` spells it. A spread of a list
            // computed from packs is that list's elements under the
            // instance's bindings. The list sits beside its visible binder's
            // zero-sized `TypeList`, which an elementwise spelling drops.
            let args = substituted_struct_arguments(name, original, bindings)?;
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
                || name == mojito_types::types::SIMD_TYPE_NAME
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
        // A symbolic size the bindings close becomes the known slot; one
        // they do not close stays as spelled, for the concreteness check.
        TyKind::PopArray { size, element } => TyKind::PopArray {
            size: match size {
                ArraySize::Expr(expr) => match eval_ct(expr, bindings) {
                    Ok(CtValue::Index(known)) if known >= 0 => ArraySize::Known(known as u64),
                    _ => size.clone(),
                },
                ArraySize::Known(_) => size.clone(),
            },
            element: substitute_ty(element, bindings)?,
        }
        .into(),
        TyKind::RuntimePack(v) => TyKind::RuntimePack(sub_spread_types(v, bindings)?).into(),
        TyKind::Variant(v) => TyKind::Variant(sub_spread_types(v, bindings)?).into(),
        TyKind::Overload(v) => TyKind::Overload(sub_types(v, bindings)?).into(),
        TyKind::ComptimeList(v) => TyKind::ComptimeList(substitute_ty(v, bindings)?).into(),
        TyKind::ParamList(v) => TyKind::ParamList(substitute_ty(v, bindings)?).into(),
        // A collector over a bound type pack is the tuple of its elements.
        TyKind::VariadicPack(v)
            if let Some(TyKind::Param { binder, .. }) =
                (mojito_types::types::pack_spread(std::slice::from_ref(v))).map(Ty::kind)
                && let Some(TyKind::RuntimePack(elements)) =
                    bindings.types.get(binder).map(Ty::kind) =>
        {
            TyKind::Tuple(sub_types(elements, bindings)?).into()
        }
        TyKind::VariadicPack(v) => TyKind::VariadicPack(substitute_ty(v, bindings)?).into(),
        TyKind::Pointer { element, origin } => TyKind::Pointer {
            element: substitute_ty(element, bindings)?,
            origin: origin.clone(),
        }
        .into(),
        TyKind::KgenPointer(element) => {
            TyKind::KgenPointer(substitute_ty(element, bindings)?).into()
        }
        TyKind::LitRef { element, origin } => TyKind::LitRef {
            element: substitute_ty(element, bindings)?,
            origin: origin.clone(),
        }
        .into(),
        TyKind::LitRefPack { elements, origin } => TyKind::LitRefPack {
            elements: sub_spread_types(elements, bindings)?,
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
                CtValue::Int(v) | CtValue::Index(v) => usize::try_from(v).ok(),
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
        kind if let Some((dtype, width)) = kind.as_simd() => Ty::simd(
            match &dtype {
                SimdDtype::Expr(expr) => match eval_ct(expr, bindings) {
                    Ok(value) if let Some(known) = mojito_types::types::dtype_of_value(&value) => {
                        SimdDtype::Known(known)
                    }
                    _ => dtype,
                },
                SimdDtype::Known(_) => dtype,
            },
            match width {
                SimdWidth::Expr(_) => match width
                    .lanes(&ParamContext::detached())
                    .map(|lanes| eval_ct(&lanes, bindings))
                {
                    Ok(Ok(CtValue::Int(known))) => SimdWidth::Known(known),
                    _ => width,
                },
                SimdWidth::Known(_) => width,
            },
        ),
        // A literal's symbolic value closes as the literal it names; one
        // the bindings do not close stays as spelled, for the concreteness
        // check to name.
        kind if let Some(value) = kind
            .string_literal_value()
            .or_else(|| kind.numeric_literal_slot()) =>
        {
            closed_literal(ty, value, bindings)
        }
        // A vector's storage closes as the vector it stores.
        TyKind::KgenSimd { dtype, width } => {
            let vector = substitute_ty(&Ty::simd(dtype.clone(), width.clone()), bindings)?;

            mojito_types::types::simd_storage(&vector).unwrap_or(vector)
        }
        _ => ty.clone(),
    })
}

/// [`sub_types`] over an element list that may be one spread of a bound
/// pack (`__RuntimeTuple[*Self.Ts]`): the spread is the pack's elements, as
/// `expand_pack_spread` spells it.
/// The literal type `ty` (`StringLiteral[v]`, `IntLiteral[v]`,
/// `FloatLiteral[v]`) at its `value` closed under `bindings`, through the
/// [`Ty::new`] bridge; `ty` itself while the value stays symbolic.
fn closed_literal(ty: &Ty, value: &CtValue, bindings: &Bindings) -> Ty {
    let CtValue::Expr(expr) = value else {
        return ty.clone();
    };
    eval_ct(expr, bindings)
        .ok()
        .filter(|closed| !matches!(closed, CtValue::Expr(_)))
        .and_then(|closed| {
            let (name, _) = mojito_types::types::nominal_parts(ty)?;
            let closed = Ty::new(TyKind::Struct(name.to_string(), vec![TyArg::Val(closed)]));
            (!closed.kind().is_non_simd_struct()).then_some(closed)
        })
        .unwrap_or_else(|| ty.clone())
}

/// The arguments `original` of the struct `name` under `bindings`.
fn substituted_struct_arguments(
    name: &str,
    original: &[TyArg],
    bindings: &Bindings,
) -> Result<Vec<TyArg>, MonoError> {
    let unsupported = |what: String| MonoError {
        kind: MonoErrorKind::Unsupported,
        function: None,
        construct: what,
    };
    let elementwise = mojito_types::types::binds_pack_elementwise(name);
    let spread_arguments = |argument: &TyArg| -> Result<Option<Vec<TyArg>>, MonoError> {
        let TyArg::Ty(ty) = argument else {
            return Ok(None);
        };
        if let Some(TyKind::Param { binder, .. }) =
            mojito_types::types::pack_spread(std::slice::from_ref(ty)).map(Ty::kind)
        {
            return Ok(match bindings.types.get(binder).map(Ty::kind) {
                Some(TyKind::RuntimePack(elements)) => {
                    let elements = sub_types(elements, bindings)?;
                    Some(if elementwise {
                        elements.into_iter().map(TyArg::Ty).collect()
                    } else {
                        vec![TyArg::Val(CtValue::Tuple(
                            elements.into_iter().map(CtValue::Type).collect(),
                        ))]
                    })
                }
                _ => None,
            });
        }
        let Some(list) = mojito_types::types::list_spread(ty) else {
            return Ok(None);
        };
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
            .collect::<Result<Vec<_>, _>>()
            .map(Some)
    };
    let mut args = Vec::with_capacity(original.len());
    for argument in original {
        match spread_arguments(argument)? {
            Some(spread) => args.extend(spread),
            None if elementwise
                && matches!(argument, TyArg::Val(value)
                    if ParamDecl::is_pack_value(value)) => {}
            None => args.push(substitute_arg(argument, bindings)?),
        }
    }
    Ok(args)
}

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
/// A struct field's type under `bindings`. A pointer field whose pointee
/// names a parameter the instance removed, or erases an argument
/// (`Optional[Entry[_REMOVED_ARG]]`), points to `None`, as `LowerLIT`
/// lowers a pointer field's pointee: the instance's layout holds its
/// address alone, and no layout of the pointee is needed.
pub(super) fn substitute_field(ty: &Ty, bindings: &Bindings) -> Result<Ty, MonoError> {
    let TyKind::Pointer { element, origin } = ty.kind() else {
        return substitute_ty(ty, bindings);
    };
    let erased = || TyKind::Pointer {
        element: Ty::none(),
        origin: origin.clone(),
    };
    if bindings
        .removed
        .iter()
        .any(|binder| mojito_types::types::names_binder(element, binder))
    {
        return substitute_ty(&erased().into(), bindings);
    }
    let substituted = substitute_ty(ty, bindings)?;
    match substituted.kind() {
        TyKind::Pointer { element, origin } if mojito_types::types::erases_an_argument(element) => {
            Ok(TyKind::Pointer {
                element: Ty::none(),
                origin: origin.clone(),
            }
            .into())
        }
        _ => Ok(substituted),
    }
}

pub(super) fn sub_types(types: &[Ty], bindings: &Bindings) -> Result<Vec<Ty>, MonoError> {
    types.iter().map(|ty| substitute_ty(ty, bindings)).collect()
}

/// The explicit failure of a read of a parameter an erased struct instance
/// does not bind: its layout or a member's body needs an argument the
/// instance's generator removed.
pub(super) fn removed_parameter_read(binder: &ParamRef) -> String {
    format!(
        "parameter `{}` is read, but an instance that removed it erased its \
         struct argument (`_REMOVED_ARG`)",
        binder.name
    )
}

/// Whether the struct argument `arg` names a parameter the instance
/// removed ([`Bindings::removed`]) other than inside a struct type it
/// holds, whose own arguments erase where they name one.
fn erases_whole(arg: &TyArg, bindings: &Bindings) -> bool {
    fn names_outside_structs(ty: &Ty, removed: &[ParamRef]) -> bool {
        match ty.kind() {
            struct_ty if let Some(_) = struct_ty.as_non_simd_struct() => false,
            TyKind::Pointer { element, .. }
            | TyKind::KgenPointer(element)
            | TyKind::LitRef { element, .. }
            | TyKind::PopArray { element, .. } => names_outside_structs(element, removed),
            TyKind::Ref(reference) => names_outside_structs(&reference.referent, removed),
            TyKind::Tuple(elements)
            | TyKind::RuntimePack(elements)
            | TyKind::Variant(elements)
            | TyKind::LitRefPack { elements, .. } => elements
                .iter()
                .any(|element| names_outside_structs(element, removed)),
            _ => removed
                .iter()
                .any(|binder| mojito_types::types::names_binder(ty, binder)),
        }
    }
    if bindings.removed.is_empty() {
        return false;
    }
    match arg {
        TyArg::Ty(ty) => names_outside_structs(ty, &bindings.removed),
        // A pack bound as one list of types (`VariadicPack`'s) is never
        // erased whole: each type substitutes as a tuple's element does.
        TyArg::Val(CtValue::Tuple(values))
            if !values.is_empty()
                && values.iter().all(|value| matches!(value, CtValue::Type(_))) =>
        {
            false
        }
        TyArg::Val(_) => {
            let holder = TyKind::Struct(String::new(), vec![arg.clone()]).into();
            bindings
                .removed
                .iter()
                .any(|binder| mojito_types::types::names_binder(&holder, binder))
        }
        TyArg::Origin(_) => false,
    }
}

/// The reflected field-name list a parameter constant `k` read at `ty` is
/// constructed from rather than read as a constant: a tuple of names read
/// at an `Array` ([`array_display_construction`]), as
/// `reflect[T].field_names()` answers. A display constant is the frozen
/// `Array` its lifted application built, read as any other constant.
pub(super) fn constructed_parameter_constant(k: &Const, ty: &Ty) -> Option<CtValue> {
    match k {
        Const::Value(value @ CtValue::Tuple(names))
            if array_instance_parts(ty).is_some()
                && names.iter().all(|name| matches!(name, CtValue::Str(_))) =>
        {
            Some(value.clone())
        }
        _ => None,
    }
}

/// The run-time construction of the parameter value `value`, of the concrete
/// type `ty`, into `dest`: a reflected field-name list at an `Array` is the
/// list-literal initializer over its names (the call a display `[a, b]`
/// lowers to), and any other value its typed constant
/// ([`Const::of_parameter_value`]).
pub(super) fn array_display_construction(
    dest: Reg,
    value: &CtValue,
    ty: &Ty,
    frame: &mut FrameSlots<'_>,
    decls: ConstructionDecls,
) -> Result<Vec<MirInstr>, MonoError> {
    let unsupported = || MonoError {
        kind: MonoErrorKind::Unsupported,
        function: None,
        construct: format!("constructing the parameter value `{value}` at `{ty}`"),
    };
    match value {
        CtValue::Tuple(elements) if let Some((element, length)) = array_instance_parts(ty) => {
            if usize::try_from(length).ok() != Some(elements.len()) {
                return Err(unsupported());
            }
            let constructor = decls
                .list_literal_constructor(mojito_types::types::ARRAY_TYPE_NAME)
                .ok_or_else(unsupported)?;
            let owned = decls.functions.get(constructor).is_some_and(|declaration| {
                declaration.variadic_convention == Some(mojito_ast::ast::ArgConvention::Var)
            });
            let parts = elements.iter().map(|value| (value, element.clone()));
            let (mut instrs, args) = constructed_parts(parts, frame, decls)?;
            instrs.extend(array_literal_construction(
                dest,
                (constructor, owned),
                element,
                args,
                length,
                frame,
            ));
            Ok(instrs)
        }
        _ => {
            let k = Const::of_parameter_value(value, Some(ty)).ok_or_else(unsupported)?;
            Ok(vec![MirInstr::Const { dest, k }])
        }
    }
}

/// `bindings` substituted through every type and parameter site of a body,
/// as upstream's elaborator substitutes through MLIR's `AttrTypeReplacer`.
struct Substitution<'b> {
    bindings: &'b Bindings,
    conforms: Option<&'b ConformsAnswer<'b>>,
}

impl MutTypeVisitor for Substitution<'_> {
    type Error = MonoError;

    fn visit_ty(&mut self, ty: &mut Ty, site: TySite) -> Result<(), MonoError> {
        if let Some(conforms) = self.conforms
            && !ty.is_closed()
        {
            *ty = ParamContext::detached()
                .answer_type_conformances(ty, &mut |subject, trait_name| {
                    conforms(subject, trait_name)
                })
                .map_err(|error| unsupported_param(&error))?;
        }
        match (substitute_ty(ty, self.bindings), site) {
            (Ok(closed), _) => *ty = closed,
            // A construction left sits in a compile-time branch not yet
            // selected, or its type stays symbolic for the verifier's
            // concrete mode to name; a subscript parameter type naming the
            // callee's own binder stays as the contract wrote it.
            (Err(_), TySite::ConstructedType | TySite::SubscriptParameter) => {}
            (Err(error), _) => return Err(error),
        }
        Ok(())
    }

    fn visit_ty_arg(&mut self, arg: &mut TyArg) -> Result<(), MonoError> {
        if let (TyArg::Val(CtValue::Expr(expr)), Some(conforms)) = (&*arg, self.conforms) {
            let answered = ParamContext::detached()
                .answer_conformances(expr, &mut |subject, trait_name| {
                    conforms(subject, trait_name)
                })
                .map_err(|error| unsupported_param(&error))?;
            *arg = TyArg::Val(CtValue::Expr(answered));
        }
        *arg = substitute_arg(arg, self.bindings)?;
        Ok(())
    }

    fn visit_variant_alternatives(&mut self, alternatives: &mut Vec<Ty>) -> Result<(), MonoError> {
        *alternatives = sub_spread_types(alternatives, self.bindings)?;
        Ok(())
    }

    fn visit_variant_index(&mut self, index: &mut VariantIndex) -> Result<(), MonoError> {
        close_variant_index(index, self.bindings)
    }

    fn visit_field_step(&mut self, step: &mut Proj, base: Option<&Ty>) -> Result<(), MonoError> {
        close_field_step(step, base, self.bindings)
    }

    /// A template's lane-mask attribute closes to its constant; the
    /// instance checks the closed mask against its operands
    /// (`close_lane_masks`).
    fn visit_lane_mask(&mut self, mask: &mut LaneMask) -> Result<(), MonoError> {
        if let LaneMask::Attribute(value) = mask
            && let Some(constant) = eval_ct(value, self.bindings)
                .ok()
                .and_then(|constant| ParamContext::detached().constant(constant).ok())
        {
            *value = constant;
        }
        Ok(())
    }

    /// A subscript's compile-time index over binders the bindings close (a
    /// `comptime for` index in its iteration's copy) is its value from here.
    /// A call's are answered by `answer_param_constants`.
    fn visit_param_arg(
        &mut self,
        arg: &mut MirParamArg,
        site: ParamArgSite,
    ) -> Result<(), MonoError> {
        if site == ParamArgSite::Subscript
            && let Some(value) = arg
                .expr
                .as_ref()
                .and_then(|expr| eval_ct(expr, self.bindings).ok())
                .and_then(|value| ParamContext::detached().constant(value).ok())
        {
            arg.expr = Some(value);
        }
        Ok(())
    }

    /// A contract's own binders are bound by its callee, not substituted.
    fn visit_param_decls(
        &mut self,
        _: &mut [ParamDecl],
        _: ParamDeclSite,
    ) -> Result<(), MonoError> {
        Ok(())
    }

    /// Parameter constants are answered by `answer_param_constants`.
    fn visit_constant(&mut self, _: &mut mojito_mir::mir::Const) -> Result<(), MonoError> {
        Ok(())
    }

    /// A constructed type parameter is rewritten by [`Self::visit_instruction`]
    /// first; a `comptime for` binder is bound by the unroller.
    fn visit_param_ref(&mut self, _: &mut ParamRef) -> Result<(), MonoError> {
        Ok(())
    }

    /// A `comptime for` sequence is stepped by the unroller.
    fn visit_sequence(&mut self, _: &mut ComptimeSequence) -> Result<(), MonoError> {
        Ok(())
    }

    fn visit_instruction(&mut self, instruction: &mut MirInstr) -> Result<(), MonoError> {
        reject_unconstructed(instruction)?;
        walk_instruction_mut(self, instruction)
    }
}

/// The elaborator's bindings decide a construction as its instance's
/// substitution does: a pack element's index evaluates under them, and a
/// selected element closes through them.
impl mojito_mir::mir::ConstructionTypes for Bindings {
    type Error = MonoError;

    fn bound(&self, param: &ParamRef) -> Option<&Ty> {
        self.types.get(param)
    }

    fn element_index(&self, element: &MirParamArg) -> Option<usize> {
        element
            .expr
            .as_ref()
            .and_then(|expr| eval_ct(expr, self).ok())
            .as_ref()
            .and_then(mojito_types::param_expr::fold::integer_value)
            .and_then(|value| value.to_i64())
            .and_then(|value| usize::try_from(value).ok())
    }

    fn close(&self, ty: &Ty) -> Result<Ty, MonoError> {
        substitute_ty(ty, self)
    }
}

/// Reject a construction of a type parameter the instance left: every
/// construction its bindings decide was written as the bound type's own
/// before substitution ([`default_construct_parameters`]).
fn reject_unconstructed(instruction: &MirInstr) -> Result<(), MonoError> {
    let MirInstr::ConstructTypeParam {
        param,
        kwargs,
        element,
        ..
    } = instruction
    else {
        return Ok(());
    };
    let construct = match (kwargs.as_slice(), element) {
        ([], Some(_)) => format!(
            "constructing an element of pack `{}` at an index the instance does not decide",
            param.name
        ),
        ([], None) => format!(
            "constructing type parameter `{}` without a concrete binding",
            param.name
        ),
        ([(keyword, _)], _) if keyword == "copy" => format!(
            "copying through type parameter `{}` without a concrete binding",
            param.name
        ),
        _ => format!(
            "constructing type parameter `{}` through an initializer other than `copy=`",
            param.name
        ),
    };
    Err(MonoError {
        kind: MonoErrorKind::Unsupported,
        function: None,
        construct,
    })
}

/// Close a template's symbolic field step once the instance binds its index
/// and the struct it selects into: the field at that position of the
/// struct's declaration, as upstream's `lit.ref.struct.ger` folds. A step
/// the bindings leave open stays for a later instance.
fn close_field_step(
    step: &mut Proj,
    base: Option<&Ty>,
    bindings: &Bindings,
) -> Result<(), MonoError> {
    let Proj::FieldAt(index) = &*step else {
        return Ok(());
    };
    let Some(position) = eval_ct(index, bindings)
        .ok()
        .and_then(|value| mojito_types::param_expr::fold::integer_value(&value))
        .and_then(|value| value.to_u64())
    else {
        return Ok(());
    };
    let Some(base) = base.filter(|base| !is_symbolic(base)) else {
        return Ok(());
    };
    let Some(instance) = struct_instance(base, bindings)? else {
        return Err(MonoError {
            kind: MonoErrorKind::Instantiation,
            function: None,
            construct: format!("a field reference into '{base}', which is not a struct"),
        });
    };
    let name = usize::try_from(position)
        .ok()
        .and_then(|position| instance.declaration.fields.get(position))
        .map(|(name, _)| name.clone())
        .ok_or_else(|| MonoError {
            kind: MonoErrorKind::Instantiation,
            function: None,
            construct: format!(
                "field index {position} is out of range for '{base}' with {} fields",
                instance.declaration.fields.len()
            ),
        })?;
    *step = Proj::Field(name);
    Ok(())
}

/// Close a template's `_get_type_index[T, *Ts]()` once the instance binds
/// the pack and `T`, or an `enum` storage's case index (`id` in
/// `_unsafe_get_enum_payload[id]`) once it binds the index. A `T` the pack
/// lacks fails the instance, as upstream's `Variant._check[T]()` does; an
/// index the bindings leave open stays for the concreteness check to name.
fn close_variant_index(index: &mut VariantIndex, bindings: &Bindings) -> Result<(), MonoError> {
    let VariantIndex::Expr(expr) = &*index else {
        return Ok(());
    };
    let ParamKind::PackQuery {
        pack,
        query: PackQuery::IndexOf(element),
    } = expr.kind()
    else {
        if let Ok(value) = eval_ct(expr, bindings)
            && let Some(case) = mojito_types::param_expr::fold::integer_value(&value)
                .and_then(|case| case.to_u64())
                .and_then(|case| usize::try_from(case).ok())
        {
            *index = VariantIndex::Known(case);
        }
        return Ok(());
    };
    // The pack is its binder, bound to the instance's runtime pack, or the
    // list of types a substitution above bound it to.
    let alternatives = match pack.kind() {
        ParamKind::DeclRef(binder) => match bindings.types.get(binder).map(Ty::kind) {
            Some(TyKind::RuntimePack(alternatives)) => alternatives.clone(),
            _ => return Ok(()),
        },
        ParamKind::Constant(CtValue::Tuple(elements)) => {
            let Some(types) = elements
                .iter()
                .map(|element| match element {
                    CtValue::Type(ty) => Some(ty.clone()),
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()
            else {
                return Ok(());
            };
            types
        }
        _ => return Ok(()),
    };
    let alternatives = sub_types(&alternatives, bindings)?;
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

/// The associated type `name` of the instance `base` resolves to: the
/// member its struct's declaration finds
/// ([`StructDeclaration::associated_member`]), the struct's own origins
/// and the member's bound there, under the instance's bindings and, for a
/// parameterized member (`C.IteratorType[o]`), the application's type
/// arguments. `None` where `base` is no struct that declares the member.
///
/// [`StructDeclaration::associated_member`]: mojito_checked::declaration::StructDeclaration::associated_member
fn declared_associated_type(
    base: &Ty,
    name: &str,
    args: &[TyArg],
    bindings: &Bindings,
) -> Result<Option<Ty>, MonoError> {
    let base = substitute_ty(base, bindings)?;
    let instance_ty = peel_refs(&base);
    let Some(instance) = struct_instance(instance_ty, bindings)? else {
        return Ok(None);
    };
    let arguments = mojito_types::types::nominal_parts(instance_ty)
        .map(|(_, arguments)| arguments)
        .unwrap_or_default();
    let member = match instance
        .declaration
        .associated_member(&arguments, name, args)
    {
        None => return Ok(None),
        Some(Ok(member)) => member,
        Some(Err(error)) => {
            let why = match error {
                AssociatedLookupError::ValueParameter => {
                    "a value parameter of an associated type is not elaborated"
                }
                AssociatedLookupError::Mismatch => "an argument's kind is not its parameter's",
            };
            return Err(MonoError {
                kind: MonoErrorKind::Unsupported,
                function: None,
                construct: format!("associated type `{base}.{name}`: {why}"),
            });
        }
    };
    let mut member_bindings = instance.bindings;
    for (binder, ty) in member.types {
        member_bindings
            .types
            .insert(binder, substitute_ty(&ty, bindings)?);
    }
    substitute_ty(&member.template, &member_bindings).map(Some)
}

/// Construct each part of a parameter value at its type into a fresh
/// register: the instructions, and the registers in order.
fn constructed_parts<'v>(
    parts: impl IntoIterator<Item = (&'v CtValue, Ty)>,
    frame: &mut FrameSlots<'_>,
    decls: ConstructionDecls,
) -> Result<(Vec<MirInstr>, Vec<Reg>), MonoError> {
    let mut instrs = Vec::new();
    let mut args = Vec::new();
    for (part, part_ty) in parts {
        let arg = frame.fresh_register(part_ty.clone());
        instrs.extend(array_display_construction(
            arg, part, &part_ty, frame, decls,
        )?);
        args.push(arg);
    }
    Ok((instrs, args))
}

/// The element type and length of an `Array` instance, a mono-named one
/// included.
fn array_instance_parts(ty: &Ty) -> Option<(&Ty, i64)> {
    let (name, arguments) = ty.kind().as_non_simd_struct()?;
    if nominal_template(name) != mojito_types::types::ARRAY_TYPE_NAME {
        return None;
    }
    match arguments.as_slice() {
        [TyArg::Ty(element), TyArg::Val(CtValue::Int(length))] => Some((element, *length)),
        _ => None,
    }
}

/// The list-literal initializer call over `elements` of type `element`, as
/// MIR lowers a display: the call builds the `VariadicList` over the
/// elements (`owned` for the initializer's `var` collector, each element
/// then the list's) and passes it, `__list_literal__` selects the overload,
/// and the `length` parameter argument reifies the constructed value's
/// length. Concrete MIR holds no lifetime marker, so the slots an owned
/// list's construction ends and what it keeps live past the call go
/// unsaid.
fn array_literal_construction(
    dest: Reg,
    (constructor, owned): (&str, bool),
    element: &Ty,
    elements: Vec<Reg>,
    length: i64,
    frame: &mut FrameSlots<'_>,
) -> Vec<MirInstr> {
    let built = mojito_mir::mir::variadics::build_variadic_list(
        frame,
        element,
        owned,
        elements
            .into_iter()
            .map(mojito_mir::mir::variadics::ListElement::Value)
            .collect(),
    );
    let none = frame.fresh_register(Ty::none());
    let length_reg = frame.fresh_register(Ty::int());
    let unbound = || mojito_mir::mir::MirParamArg {
        name: None,
        value: None,
        expr: None,
    };
    let mut instrs = built.before;
    instrs.extend([
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
            arg_places: vec![None],
            args: vec![built.list],
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
            spread: Some(0),
        },
    ]);
    instrs
}

fn fresh_register(ty: Ty, n_regs: &mut u32, reg_types: &mut HashMap<u32, Ty>) -> Reg {
    let reg = Reg(*n_regs);
    *n_regs += 1;
    reg_types.insert(reg.0, ty);
    reg
}

/// A parameter expression error a substitution meets as the contextual
/// unsupported boundary.
fn unsupported_param(error: &mojito_types::param_expr::ParamError) -> MonoError {
    MonoError {
        kind: MonoErrorKind::Unsupported,
        function: None,
        construct: error.to_string(),
    }
}
