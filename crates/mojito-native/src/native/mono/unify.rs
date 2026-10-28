//! Argument binding: ordering, defaults, and pattern/actual type
//! unification into `Bindings`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::TyKind;
use mojito_types::types::{CallableDefault, FuncTy};

/// A `Some[Trait]` sugar parameter is infer-only and absent from the
/// declaration's `param_decls`, yet each binding selects a different body
/// (`update(value: Some[Hashable])` hashes an `Int` and a `Pair` through
/// different leaves). Its binding joins the instance identity; the builtin
/// string binding — the `Some[Writer]` display accumulator and the
/// declaration-order default — keeps the unsuffixed spelling. An owner's
/// parameter (`Self.T` in a method signature) is likewise absent from the
/// method's `param_decls`, but when an instance-named owner
/// (`bindings.self_instance`) already carries the binding it must not join
/// again — only a `Some[..]` sugar binding does. A static path with no
/// owner instance keeps every resolved binding in the identity.
/// A pack's element type (`*values: Self.T`) selects a body the same way a
/// parameter's does, so it joins beside the parameter types.
pub(super) fn push_sugar_arguments(
    declaration: &MirFunctionDeclaration,
    bindings: &Bindings,
    arguments: &mut Vec<InstanceArg>,
) {
    for ty in declaration.param_types.iter().chain(&declaration.variadic) {
        if let TyKind::Param { binder, .. } = peel_refs(ty).kind()
            && (binder.name.starts_with("Some[") || bindings.self_instance.is_none())
            && !declaration
                .param_decls
                .iter()
                .any(|decl| *decl.id() == binder.id)
            && let Some(bound) = bindings.types.get(binder)
            && *bound != TyKind::StringLiteral
        {
            arguments.push(InstanceArg::Ty(bound.clone()));
        }
    }
}

pub(super) fn ordered_arguments(
    decls: &[ParamDecl],
    bindings: &Bindings,
    target: &str,
) -> Result<Vec<InstanceArg>, MonoError> {
    decls
        .iter()
        .map(|decl| bound_argument(decl, bindings, target))
        .collect()
}

/// The instance argument `bindings` give the parameter `decl` of `target`.
pub(super) fn bound_argument(
    decl: &ParamDecl,
    bindings: &Bindings,
    target: &str,
) -> Result<InstanceArg, MonoError> {
    match decl {
        ParamDecl::Type { .. } => bindings
            .types
            .get(&decl.binder())
            .cloned()
            .map(InstanceArg::Ty),
        ParamDecl::Value { .. } => bindings
            .values
            .get(&decl.binder())
            .cloned()
            .map(InstanceArg::Value),
    }
    .ok_or_else(|| MonoError {
        kind: MonoErrorKind::Unsupported,
        function: Some(target.to_string()),
        construct: format!(
            "monomorphization cannot resolve parameter `{}`",
            decl.name()
        ),
    })
}

/// The instance type of the struct `template` under `bindings`, named as
/// [`substitute_ty`] names an application of it: a bound pack spells its
/// elements one by one for a struct that binds its pack elementwise
/// (`Tuple`), and as one list for any other.
pub(super) fn owner_instance_ty(
    template: &str,
    decls: &[ParamDecl],
    bindings: &Bindings,
) -> Result<Ty, MonoError> {
    let elementwise = mojito_types::types::binds_pack_elementwise(template);
    let mut arguments = Vec::new();
    for (decl, argument) in decls
        .iter()
        .zip(ordered_arguments(decls, bindings, template)?)
    {
        match (decl, argument) {
            (ParamDecl::Type { variadic: true, .. }, InstanceArg::Ty(pack))
                if matches!(pack.kind(), TyKind::RuntimePack(_)) =>
            {
                let TyKind::RuntimePack(elements) = pack.into_kind() else {
                    unreachable!("guard established a runtime pack");
                };
                if elementwise {
                    arguments.extend(elements.into_iter().map(InstanceArg::Ty));
                } else {
                    arguments.push(InstanceArg::Value(CtValue::Tuple(
                        elements.into_iter().map(CtValue::Type).collect(),
                    )));
                }
            }
            (_, argument) => arguments.push(argument),
        }
    }
    let name = mojito_symbol::symbol::instance_symbol(template, &arguments);
    let ty_arguments: Vec<TyArg> = arguments
        .into_iter()
        .map(|argument| match argument {
            InstanceArg::Ty(ty) => TyArg::Ty(ty),
            InstanceArg::Value(value) => TyArg::Val(value),
        })
        .collect();
    Ok(TyKind::Struct(name, ty_arguments).into())
}

pub(super) fn bind_explicit_value_arguments(
    decls: &[ParamDecl],
    removed: &[mojito_types::param_expr::ParamId],
    arguments: &[mojito_mir::mir::MirParamArg],
    constant_values: &HashMap<u32, CtValue>,
    bindings: &mut Bindings,
    target: &str,
    enclosing: &Bindings,
) -> Result<(), MonoError> {
    let mut packs: Vec<(ParamRef, Vec<CtValue>)> = Vec::new();
    for (declaration, value_reg, argument) in
        mojito_mir::mir::matched_parameter_arguments(decls, arguments)
    {
        match declaration {
            // A constant register is the value; an argument built from the
            // caller's value binders is its recorded expression under the
            // caller instance's bindings.
            // A binder typed by a type binder (`v: T`) keeps the checker's
            // solution, materialized at `T`'s: the register holds the
            // literal as spelled (`2` where `T` is `Float64`).
            ParamDecl::Value { .. }
                if declaration
                    .value_element_ty()
                    .is_some_and(|ty| matches!(ty.kind(), TyKind::Param { .. }))
                    && bindings
                        .values
                        .get(&declaration.binder())
                        .is_some_and(|bound| !mojito_types::types::ct_value_is_symbolic(bound)) => {
            }
            ParamDecl::Value { name, .. } => {
                let value = match (constant_values.get(&value_reg.0), &argument.expr) {
                    (Some(value), _) => value.clone(),
                    (None, Some(expr)) => eval_ct(expr, enclosing).map_err(|mut error| {
                        error.function.get_or_insert_with(|| target.to_string());
                        error
                    })?,
                    // The checker's recorded solution for the call already
                    // bound the binder (a local `comptime` `DType` spelled in
                    // the brackets); the register is its runtime shadow.
                    (None, None) if bindings.values.contains_key(&declaration.binder()) => {
                        continue;
                    }
                    // No instance of the callee reads a parameter it
                    // removed, so a runtime argument for it (a capturing
                    // callable the caller took as a runtime parameter)
                    // binds nothing.
                    (None, None) if removed.contains(declaration.id()) => continue,
                    (None, None) => {
                        return Err(MonoError {
                            kind: MonoErrorKind::Unsupported,
                            function: Some(target.to_string()),
                            construct: format!(
                                "value parameter `{name}` is not compile-time constant"
                            ),
                        });
                    }
                };
                let binder = declaration.binder();
                if !matches!(declaration, ParamDecl::Value { variadic: true, .. }) {
                    bindings.values.insert(binder, value);
                } else if let Some((_, elements)) =
                    packs.iter_mut().find(|(pack, _)| *pack == binder)
                {
                    elements.push(value);
                } else {
                    packs.push((binder, vec![value]));
                }
            }
            // A type argument is compile-time data on the call, bound from
            // its instantiated arguments.
            ParamDecl::Type { .. } => {}
        }
    }
    // A value pack binds the list of its positional arguments.
    for (binder, elements) in packs {
        bindings.values.insert(binder, CtValue::Tuple(elements));
    }
    Ok(())
}

pub(super) fn apply_defaults(
    decls: &[ParamDecl],
    bindings: &mut Bindings,
) -> Result<(), MonoError> {
    for decl in decls {
        // An erased parameter takes no default: it has no value at all.
        if bindings.removed.contains(&decl.binder()) {
            continue;
        }
        match decl {
            ParamDecl::Type {
                default: Some(default),
                ..
            } if !bindings.types.contains_key(&decl.binder()) => {
                bindings
                    .types
                    .insert(decl.binder(), substitute_ty(default, bindings)?);
            }
            ParamDecl::Value {
                default: Some(default),
                ty,
                ..
            } if !bindings.values.contains_key(&decl.binder()) => {
                // A literal default binds at its declared type, as a supplied
                // argument's constant register reads.
                let value = match eval_ct(default, bindings)? {
                    CtValue::IntLiteral(literal) if **ty != TyKind::IntLiteral => literal
                        .to_i64()
                        .map_or(CtValue::IntLiteral(literal), CtValue::Int),
                    value => value,
                };
                bindings.values.insert(decl.binder(), value);
            }
            ParamDecl::Value {
                callable_default: Some(default),
                ..
            } if !bindings.values.contains_key(&decl.binder()) => {
                if let Some(callable) = resolve_callable_default(default, bindings)? {
                    bindings.values.insert(decl.binder(), callable);
                }
            }
            _ => {}
        }
    }
    Ok(())
}

pub(super) fn bind_ty_args(
    decls: &[ParamDecl],
    args: &[TyArg],
    bindings: &mut Bindings,
) -> Result<(), String> {
    // A struct keyed on its lone pack applied element by element
    // (`Row[Int, Bool]`) binds the pack to every element.
    if let [decl @ ParamDecl::Type { variadic: true, .. }] = decls
        && !matches!(args, [TyArg::Val(_)])
        && !matches!(args, [TyArg::Ty(pack)] if matches!(pack.kind(), TyKind::RuntimePack(_)))
    {
        let elements = args
            .iter()
            .filter_map(|argument| match argument {
                TyArg::Ty(ty) => Some(Ok(ty.clone())),
                TyArg::Origin(_) => None,
                TyArg::Val(_) => Some(Err(format!(
                    "argument for `{}` has the wrong parameter kind",
                    decl.name()
                ))),
            })
            .collect::<Result<Vec<_>, _>>()?;
        bind_pack(&decl.binder(), elements, bindings);
        return Ok(());
    }
    for (decl, arg) in decls.iter().zip(args) {
        match (decl, arg) {
            // An erased argument binds nothing: its parameter is one the
            // instance's identity does not hold.
            (_, TyArg::Val(CtValue::RemovedArg)) => bindings.removed.push(decl.binder()),
            // A pack bound whole: its element tuple, or the runtime pack a
            // forwarding application spells.
            (ParamDecl::Type { variadic: true, .. }, TyArg::Val(CtValue::Tuple(elements))) => {
                let elements = elements
                    .iter()
                    .map(|element| match element {
                        CtValue::Type(ty) => Ok(ty.clone()),
                        other => Err(format!(
                            "element `{other}` of the pack `{}` is not a type",
                            decl.name()
                        )),
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                bind_pack(&decl.binder(), elements, bindings);
            }
            (ParamDecl::Type { variadic: true, .. }, TyArg::Ty(pack))
                if let TyKind::RuntimePack(elements) = pack.kind() =>
            {
                bind_pack(&decl.binder(), elements.clone(), bindings);
            }
            (ParamDecl::Type { .. }, TyArg::Ty(ty)) => bind_type(&decl.binder(), ty, bindings)?,
            (ParamDecl::Value { .. }, TyArg::Val(value)) => {
                bind_value(&decl.binder(), value, bindings)?;
            }
            (_, TyArg::Origin(_)) => {}
            _ => {
                return Err(format!(
                    "argument for `{}` has the wrong parameter kind",
                    decl.name()
                ));
            }
        }
    }
    Ok(())
}

/// Whether a spread over the struct `pattern` names the same declaration
/// as `actual`, named `instance`: one template, or the public `Tuple`
/// against a specialization of it.
fn spreads_into(pattern: &str, instance: &str, actual: &Ty) -> bool {
    nominal_template(pattern) == nominal_template(instance)
        || (pattern == mojito_types::types::TUPLE_TYPE_NAME
            && mojito_types::types::tuple_elements(actual).is_some())
}

pub(super) fn unify(pattern: &Ty, actual: &Ty, bindings: &mut Bindings) -> Result<(), String> {
    match pattern.kind() {
        TyKind::Param { binder, .. } => bind_type(binder, actual, bindings),
        TyKind::Assoc { .. } => {
            let key = pattern.to_string();
            match bindings.associated.get(&key) {
                // Origins erase from the runtime ABI, as for a type binder.
                Some(known) if known != actual && !ty_equal_modulo_origins(known, actual) => {
                    Err(format!(
                        "conflicting solutions for associated type `{key}`: `{known}` and `{actual}`"
                    ))
                }
                Some(_) => Ok(()),
                None => {
                    bindings.associated.insert(key, actual.clone());
                    Ok(())
                }
            }
        }
        // A literal-typed register materializes into whatever concrete
        // storage the checker admitted (`MaterializeLiteral` converts the
        // value at the boundary); the pattern constrains nothing here.
        _ if matches!(
            actual.kind(),
            TyKind::IntLiteral | TyKind::FloatLiteral | TyKind::StringLiteral
        ) && pattern != actual =>
        {
            Ok(())
        }
        TyKind::Struct(pn, pa) => match actual.kind() {
            TyKind::Struct(an, _)
                if nominal_template(pn) == nominal_template(an) && pa.is_empty() =>
            {
                Ok(())
            }
            // A spread of a list computed from packs solves nothing: it
            // closes once the packs it reads are bound.
            TyKind::Struct(an, _)
                if spreads_into(pn, an, actual)
                    && mojito_types::types::list_spread_argument(pa).is_some() =>
            {
                Ok(())
            }
            // A spread of a pack binds it to the actual's whole element
            // list, unless the call's recorded arguments already did.
            TyKind::Struct(an, aa)
                if spreads_into(pn, an, actual)
                    && let Some(TyKind::Param { binder, .. }) =
                        (mojito_types::types::pack_spread_argument(pa)).map(Ty::kind) =>
            {
                let elements = match aa.as_slice() {
                    [TyArg::Val(CtValue::Tuple(values))] => values
                        .iter()
                        .map(|value| match value {
                            CtValue::Type(ty) => Some(ty.clone()),
                            _ => None,
                        })
                        .collect::<Option<Vec<_>>>(),
                    arguments => arguments
                        .iter()
                        .map(|argument| match argument {
                            TyArg::Ty(ty) => Some(ty.clone()),
                            TyArg::Val(_) | TyArg::Origin(_) => None,
                        })
                        .collect(),
                };
                match elements {
                    Some(elements) if !bindings.types.contains_key(binder) => {
                        bind_pack(binder, elements, bindings);
                        Ok(())
                    }
                    Some(_) => Ok(()),
                    None => Err(format!("expected `{pattern}`, found `{actual}`")),
                }
            }
            TyKind::Struct(an, aa)
                if nominal_template(pn) == nominal_template(an) && pa.len() == aa.len() =>
            {
                pa.iter()
                    .zip(aa)
                    .try_for_each(|(p, a)| unify_arg(p, a, bindings))
            }
            _ => Err(format!("expected `{pattern}`, found `{actual}`")),
        },
        TyKind::Tuple(p) | TyKind::RuntimePack(p) | TyKind::Variant(p) => match actual.kind() {
            TyKind::Tuple(a) | TyKind::RuntimePack(a) | TyKind::Variant(a)
                if p.len() == a.len() =>
            {
                p.iter().zip(a).try_for_each(|(p, a)| unify(p, a, bindings))
            }
            _ => Err(format!("expected `{pattern}`, found `{actual}`")),
        },
        TyKind::ComptimeList(p) | TyKind::VariadicPack(p) => match actual.kind() {
            TyKind::ComptimeList(a) | TyKind::VariadicPack(a) => unify(p, a, bindings),
            _ => Err(format!("expected `{pattern}`, found `{actual}`")),
        },
        TyKind::ParamList(p) => match actual.kind() {
            TyKind::ParamList(a) => unify(p, a, bindings),
            _ => Err(format!("expected `{pattern}`, found `{actual}`")),
        },
        TyKind::Pointer { element: p, .. } => match actual.kind() {
            TyKind::Pointer { element: a, .. } => unify(p, a, bindings),
            _ => Err(format!("expected `{pattern}`, found `{actual}`")),
        },
        TyKind::Ref(p) => match actual.kind() {
            TyKind::Ref(a) if p.mutability == a.mutability => {
                unify(&p.referent, &a.referent, bindings)
            }
            _ => Err(format!("expected `{pattern}`, found `{actual}`")),
        },
        // A vector's lane dtype or width that is a bare binder binds from the
        // argument's slot (`Scalar[dt]` against a `Float32`, `SIMD[dt, n]`
        // against a four-lane vector); a compound expression (`2 * n`) is
        // bound from the call's recorded arguments, as the pin binds it from
        // the brackets, and only its evaluation is compared.
        TyKind::Simd { dtype, width } => {
            let Some((actual_dtype, actual_width)) = mojito_types::types::simd_slots(actual) else {
                return Err(format!("expected `{pattern}`, found `{actual}`"));
            };
            unify_simd_slot(
                dtype.is_expr().then(|| slot_expr(dtype)).flatten(),
                dtype.known().map(CtValue::Dtype),
                actual_dtype.known().map(CtValue::Dtype),
                pattern,
                actual,
                bindings,
            )?;
            // A `SIMDLength` binder's lane count binds the binder to the
            // length of the argument's lanes.
            if let Some(length) = width_expr(width)
                .and_then(mojito_types::types::simd_length_of_image)
                .and_then(ParamExpr::as_decl_ref)
                && let Some(lanes) = actual_width.known()
            {
                return bind_value(
                    length,
                    &mojito_types::types::simd_length_value(lanes),
                    bindings,
                );
            }
            unify_simd_slot(
                width.is_expr().then(|| width_expr(width)).flatten(),
                width.known().map(CtValue::Int),
                actual_width.known().map(CtValue::Int),
                pattern,
                actual,
                bindings,
            )
        }
        // Callable contracts unify on their runtime structure — parameters,
        // return, raising — never on the environment (`thin` vs
        // `capturing[...]`) or origin spellings, which erase from the ABI.
        TyKind::Func(func) => {
            let FuncTy {
                params: p_params,
                ret: p_ret,
                required: p_required,
                variadic: p_variadic,
                kw_variadic: p_kw_variadic,
                positional_only: p_positional_only,
                keyword_only: p_keyword_only,
                raises: p_raises,
                error: p_error,
                ..
            } = &**func;
            match actual.kind() {
                TyKind::Func(func)
                    if let FuncTy {
                        params: a_params,
                        ret: a_ret,
                        required: a_required,
                        variadic: a_variadic,
                        kw_variadic: a_kw_variadic,
                        positional_only: a_positional_only,
                        keyword_only: a_keyword_only,
                        raises: a_raises,
                        error: a_error,
                        ..
                    } = &**func
                        && p_params.len() == a_params.len()
                        && p_required == a_required
                        && p_positional_only == a_positional_only
                        && p_keyword_only == a_keyword_only
                        && p_raises == a_raises =>
                {
                    let unify_option = |p: &Option<Ty>,
                                        a: &Option<Ty>,
                                        bindings: &mut Bindings|
                     -> Result<(), String> {
                        match (p, a) {
                            (Some(p), Some(a)) => unify(p, a, bindings),
                            (None, None) => Ok(()),
                            _ => Err(format!("expected `{pattern}`, found `{actual}`")),
                        }
                    };
                    p_params
                        .iter()
                        .zip(a_params)
                        .try_for_each(|(p, a)| unify(p, a, bindings))?;
                    unify(p_ret, a_ret, bindings)?;
                    unify_option(p_variadic, a_variadic, bindings)?;
                    unify_option(p_kw_variadic, a_kw_variadic, bindings)?;
                    unify_option(p_error, a_error, bindings)
                }
                _ => Err(format!("expected `{pattern}`, found `{actual}`")),
            }
        }
        _ if pattern == actual => Ok(()),
        // A type computed from parameters (`Ts[index]`) solves nothing: it
        // closes once the parameters it reads are bound.
        TyKind::Dependent(_) => Ok(()),
        _ => Err(format!("expected `{pattern}`, found `{actual}`")),
    }
}

/// Unify a callee's declared result against the caller's checked result type,
/// stripping `ref` layers on both sides first: a reference-returning call
/// spells its declared referent and the checked handle with differing layers.
pub(super) fn unify_result(
    pattern: &Ty,
    actual: &Ty,
    bindings: &mut Bindings,
) -> Result<(), String> {
    let mut pattern = pattern;
    while let TyKind::Ref(reference) = pattern.kind() {
        pattern = &reference.referent;
    }
    let mut actual = actual;
    while let TyKind::Ref(reference) = actual.kind() {
        actual = &reference.referent;
    }
    // A container element may itself be a reference. Receiver inference has
    // then already bound `T = ref U`, while the checker-flattened reference
    // result is spelled `ref U`; stripping its handle above leaves `U`.
    // Preserve the established element solution instead of mistaking the
    // flattened handle for a conflicting `T = U` solution.
    if let TyKind::Param { binder, .. } = pattern.kind()
        && let Some(TyKind::Ref(reference)) = bindings.types.get(binder).map(Ty::kind)
        && ty_equal_modulo_origins(&reference.referent, actual)
    {
        return Ok(());
    }
    unify(pattern, actual, bindings)
}

pub(super) fn unify_arg(
    pattern: &TyArg,
    actual: &TyArg,
    bindings: &mut Bindings,
) -> Result<(), String> {
    match (pattern, actual) {
        (TyArg::Ty(p), TyArg::Ty(a)) => unify(p, a, bindings),
        // An erased argument stands in for a type binder the instance does
        // not hold, which it binds to nothing, as a receiver's does.
        (TyArg::Ty(p), TyArg::Val(CtValue::RemovedArg))
            if let TyKind::Param { binder, .. } = p.kind()
                && !bindings.types.contains_key(binder) =>
        {
            if !bindings.removed.contains(binder) {
                bindings.removed.push(binder.clone());
            }
            Ok(())
        }
        // A direct reference binds; any other residual must already agree
        // with the actual once the environment closes it.
        (TyArg::Val(CtValue::Expr(expr)), TyArg::Val(value)) => match expr.as_decl_ref() {
            Some(reference) => bind_value(reference, value, bindings),
            None => match eval_ct(expr, bindings) {
                Ok(closed) if mojito_types::param_expr::identity_eq(&closed, value) => Ok(()),
                Ok(_) => Err("generic application arguments disagree".to_string()),
                Err(_) if struct_value_agrees(expr, value, bindings) => Ok(()),
                Err(error) => Err(error.construct),
            },
        },
        (TyArg::Val(p), TyArg::Val(a)) if p == a => Ok(()),
        // A pack bound as one list of types (`Pair[T, Int]`) unifies type by
        // type.
        (TyArg::Val(CtValue::Tuple(p)), TyArg::Val(CtValue::Tuple(a))) if p.len() == a.len() => {
            p.iter().zip(a).try_for_each(|pair| match pair {
                (CtValue::Type(p), CtValue::Type(a)) => unify(p, a, bindings),
                (p, a) if p == a => Ok(()),
                _ => Err("generic application arguments disagree".to_string()),
            })
        }
        (TyArg::Origin(_), TyArg::Origin(_)) => Ok(()),
        _ => Err("generic application arguments disagree".to_string()),
    }
}

/// Whether `expr`, a struct value a builtin `__init__` folds to whose
/// construction no substitution has run yet, has the fields of the frozen
/// `value`: a struct value's identity is its fields.
fn struct_value_agrees(expr: &ParamExpr, value: &CtValue, bindings: &Bindings) -> bool {
    let (ParamKind::StructValue { fields, .. }, CtValue::Struct { fields: frozen, .. }) =
        (expr.kind(), value)
    else {
        return false;
    };
    fields.len() == frozen.len()
        && fields
            .iter()
            .zip(frozen)
            .all(|((name, field), (frozen_name, frozen_value))| {
                name == frozen_name
                    && eval_ct(field, bindings).is_ok_and(|field| {
                        mojito_types::param_expr::identity_eq(&field, frozen_value)
                    })
            })
}

/// Bind the type pack `binder` to `elements`: as the runtime pack every
/// spelling of the pack substitutes to, and as the element tuple the
/// parameter expressions over it (`Ts[i]`, the pack's length) fold under.
pub(super) fn bind_pack(binder: &ParamRef, elements: Vec<Ty>, bindings: &mut Bindings) {
    // An element spelled as a literal type (`(1, "a")` is typed over
    // `IntLiteral`) is stored materialized, and names the instance so.
    let elements: Vec<Ty> = elements
        .into_iter()
        .map(|element| mojito_types::types::default_literal(&element))
        .collect();
    bindings.values.insert(
        binder.clone(),
        CtValue::Tuple(
            elements
                .iter()
                .map(|element| CtValue::Type(element.clone()))
                .collect(),
        ),
    );
    bindings
        .types
        .insert(binder.clone(), TyKind::RuntimePack(elements).into());
}

pub(super) fn bind_type(binder: &ParamRef, ty: &Ty, bindings: &mut Bindings) -> Result<(), String> {
    let name = &binder.name;
    if is_symbolic(ty) {
        return Err(format!("solution for `{name}` is not concrete: `{ty}`"));
    }
    // Solutions join instance identity: erase callable-environment spellings
    // so `capturing[origin@N]` and `thin` variants of one contract are one
    // instance.
    let ty = &materialize_nested_literals(&canonicalize_callable(ty));
    let literal = |ty: &Ty| {
        matches!(
            ty.kind(),
            TyKind::IntLiteral | TyKind::FloatLiteral | TyKind::StringLiteral
        )
    };
    match bindings.types.get(binder) {
        // A literal-typed actual materializes into whatever concrete storage
        // is already bound, and a concrete solution upgrades an earlier
        // literal-only binding — mirroring `unify`'s literal escape. Binding
        // order varies by call shape (receiver-first vs result-last), so the
        // merge must be order-independent.
        Some(old) if literal(ty) && !literal(old) => Ok(()),
        Some(old) if literal(old) && !literal(ty) => {
            bindings.types.insert(binder.clone(), ty.clone());
            Ok(())
        }
        // Origins erase from the runtime ABI, so solutions differing only in
        // `ref`/pointer origins are one instance — the first spelling wins.
        // `Ty`'s `Display` collapses distinct types (`IntLiteral` renders as
        // `Int`), so the conflict text carries the structural form too.
        Some(old) if old != ty && !ty_equal_modulo_origins(old, ty) => Err(format!(
            "conflicting solutions for `{name}`: `{old}` ({old:?}) and `{ty}` ({ty:?})"
        )),
        Some(_) => Ok(()),
        None => {
            bindings.types.insert(binder.clone(), ty.clone());
            Ok(())
        }
    }
}

pub(super) fn bind_value(
    binder: &ParamRef,
    value: &CtValue,
    bindings: &mut Bindings,
) -> Result<(), String> {
    let name = &binder.name;
    if matches!(
        value,
        CtValue::Expr(_) | CtValue::Deferred(_) | CtValue::Marker(_)
    ) {
        return Err(format!("solution for `{name}` is not constant"));
    }
    match bindings.values.get(binder) {
        // As in `bind_type`, `Display` can collapse distinct values (an Int
        // and a UInt render alike), so the conflict text carries the
        // structural forms.
        Some(old) if old != value => Err(format!(
            "conflicting solutions for `{name}`: `{old}` ({old:?}) and `{value}` ({value:?})"
        )),
        Some(_) => Ok(()),
        None => {
            bindings.values.insert(binder.clone(), value.clone());
            Ok(())
        }
    }
}

/// One slot of a vector pattern against the argument's: a bare binder
/// binds the known value, a known slot must match, and any other expression
/// is left to the recorded arguments.
fn unify_simd_slot(
    expr: Option<&ParamExpr>,
    known: Option<CtValue>,
    actual: Option<CtValue>,
    pattern: &Ty,
    actual_ty: &Ty,
    bindings: &mut Bindings,
) -> Result<(), String> {
    match (expr.map(ParamExpr::kind), known, actual) {
        (Some(ParamKind::DeclRef(reference)), _, Some(value)) => {
            bind_value(reference, &value, bindings)
        }
        (None, Some(known), Some(value)) if known != value => {
            Err(format!("expected `{pattern}`, found `{actual_ty}`"))
        }
        _ => Ok(()),
    }
}

const fn slot_expr(dtype: &SimdDtype) -> Option<&ParamExpr> {
    match dtype {
        SimdDtype::Expr(expr) => Some(expr),
        SimdDtype::Known(_) => None,
    }
}

const fn width_expr(width: &SimdWidth) -> Option<&ParamExpr> {
    match width {
        SimdWidth::Expr(expr) => Some(expr),
        SimdWidth::Known(_) => None,
    }
}

/// The callable a callable parameter's default names under `bindings`: a
/// function symbol, an earlier callable parameter's binding, or the arm a
/// compile-time condition selects. `None` leaves the slot unresolved.
fn resolve_callable_default(
    default: &CallableDefault,
    bindings: &Bindings,
) -> Result<Option<CtValue>, MonoError> {
    match default {
        CallableDefault::Symbol(symbol) => Ok(Some(CtValue::Str(symbol.clone()))),
        CallableDefault::Parameter(parameter) => Ok(bindings.values.get(parameter).cloned()),
        CallableDefault::If {
            condition,
            then_value,
            else_value,
        } => match eval_ct(condition, bindings)? {
            CtValue::Bool(true) => resolve_callable_default(then_value, bindings),
            CtValue::Bool(false) => resolve_callable_default(else_value, bindings),
            _ => Ok(None),
        },
    }
}
