//! Argument binding: ordering, defaults, and pattern/actual type
//! unification into `Bindings`.

use super::*;

/// A `Some[Trait]` sugar parameter is infer-only and absent from the
/// declaration's `param_decls`, yet each binding selects a different body
/// (`update(value: Some[Hashable])` hashes an `Int` and a `Pair` through
/// different leaves). Its binding joins the instance identity; the builtin
/// string binding — the `Some[Writer]` display accumulator and the
/// declaration-order default — keeps the unsuffixed spelling.
pub(super) fn push_sugar_arguments(
    declaration: &MirFunctionDeclaration,
    bindings: &Bindings,
    arguments: &mut Vec<InstanceArg>,
) {
    for ty in &declaration.param_types {
        if let Ty::Param { name, .. } = peel_refs(ty)
            && !declaration
                .param_decls
                .iter()
                .any(|decl| decl.name().trim_start_matches('*') == name)
            && let Some(bound) = bindings.types.get(name.as_str())
            && *bound != Ty::StringLiteral
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
        .map(|decl| {
            match decl {
                ParamDecl::Type { name, .. } => {
                    bindings.types.get(name).cloned().map(InstanceArg::Ty)
                }
                ParamDecl::Value { name, .. } => {
                    bindings.values.get(name).cloned().map(InstanceArg::Value)
                }
            }
            .ok_or_else(|| MonoError {
                function: Some(target.to_string()),
                construct: format!(
                    "monomorphization cannot resolve parameter `{}`",
                    decl.name()
                ),
            })
        })
        .collect()
}

pub(super) fn bind_explicit_value_arguments(
    decls: &[ParamDecl],
    arguments: &[crate::mir::MirParamArg],
    constant_values: &HashMap<u32, CtValue>,
    bindings: &mut Bindings,
    target: &str,
) -> Result<(), MonoError> {
    let mut positional = 0;
    for argument in arguments {
        let Some(value_reg) = argument.value else {
            if argument.name.is_none() {
                positional += 1;
            }
            continue;
        };
        let declaration = if let Some(name) = &argument.name {
            decls.iter().find(|declaration| declaration.name() == name)
        } else {
            let declaration = decls.get(positional);
            positional += 1;
            declaration
        };
        let Some(ParamDecl::Value { name, .. }) = declaration else {
            continue;
        };
        let value = constant_values
            .get(&value_reg.0)
            .cloned()
            .ok_or_else(|| MonoError {
                function: Some(target.to_string()),
                construct: format!("value parameter `{name}` is not compile-time constant"),
            })?;
        bindings.values.insert(name.clone(), value);
    }
    Ok(())
}

pub(super) fn apply_defaults(
    decls: &[ParamDecl],
    bindings: &mut Bindings,
) -> Result<(), MonoError> {
    for decl in decls {
        match decl {
            ParamDecl::Type {
                name,
                default: Some(default),
                ..
            } if !bindings.types.contains_key(name) => {
                bindings
                    .types
                    .insert(name.clone(), substitute_ty(default, bindings)?);
            }
            ParamDecl::Value {
                name,
                default: Some(default),
                ..
            } if !bindings.values.contains_key(name) => {
                bindings
                    .values
                    .insert(name.clone(), eval_ct(default, bindings)?);
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
    for (decl, arg) in decls.iter().zip(args) {
        match (decl, arg) {
            (ParamDecl::Type { name, .. }, TyArg::Ty(ty)) => bind_type(name, ty, bindings)?,
            (ParamDecl::Value { name, .. }, TyArg::Val(value)) => {
                bind_value(name, value, bindings)?
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

pub(super) fn unify(pattern: &Ty, actual: &Ty, bindings: &mut Bindings) -> Result<(), String> {
    match pattern {
        Ty::Param { name, .. } => bind_type(name, actual, bindings),
        Ty::Assoc { .. } => {
            let key = pattern.to_string();
            match bindings.associated.get(&key) {
                Some(known) if known != actual => Err(format!(
                    "conflicting solutions for associated type `{key}`: `{known}` and `{actual}`"
                )),
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
            actual,
            Ty::IntLiteral | Ty::FloatLiteral | Ty::StringLiteral
        ) && pattern != actual =>
        {
            Ok(())
        }
        Ty::Struct(pn, pa) => match actual {
            Ty::Struct(an, _) if nominal_template(pn) == nominal_template(an) && pa.is_empty() => {
                Ok(())
            }
            Ty::Struct(an, aa)
                if nominal_template(pn) == nominal_template(an) && pa.len() == aa.len() =>
            {
                pa.iter()
                    .zip(aa)
                    .try_for_each(|(p, a)| unify_arg(p, a, bindings))
            }
            _ => Err(format!("expected `{pattern}`, found `{actual}`")),
        },
        Ty::Tuple(p) | Ty::RuntimePack(p) | Ty::Variant(p) => match actual {
            Ty::Tuple(a) | Ty::RuntimePack(a) | Ty::Variant(a) if p.len() == a.len() => {
                p.iter().zip(a).try_for_each(|(p, a)| unify(p, a, bindings))
            }
            _ => Err(format!("expected `{pattern}`, found `{actual}`")),
        },
        Ty::ComptimeList(p) | Ty::VariadicPack(p) => match actual {
            Ty::ComptimeList(a) | Ty::VariadicPack(a) => unify(p, a, bindings),
            _ => Err(format!("expected `{pattern}`, found `{actual}`")),
        },
        Ty::Pointer { element: p, .. } => match actual {
            Ty::Pointer { element: a, .. } => unify(p, a, bindings),
            _ => Err(format!("expected `{pattern}`, found `{actual}`")),
        },
        Ty::Ref(p) => match actual {
            Ty::Ref(a) if p.mutability == a.mutability => unify(&p.referent, &a.referent, bindings),
            _ => Err(format!("expected `{pattern}`, found `{actual}`")),
        },
        // Callable contracts unify on their runtime structure — parameters,
        // return, raising — never on the environment (`thin` vs
        // `capturing[...]`) or origin spellings, which erase from the ABI.
        Ty::Func {
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
        } => match actual {
            Ty::Func {
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
            } if p_params.len() == a_params.len()
                && p_required == a_required
                && p_positional_only == a_positional_only
                && p_keyword_only == a_keyword_only
                && p_raises == a_raises =>
            {
                let unify_option = |p: &Option<Box<Ty>>,
                                    a: &Option<Box<Ty>>,
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
        },
        _ if pattern == actual => Ok(()),
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
    while let Ty::Ref(reference) = pattern {
        pattern = &reference.referent;
    }
    let mut actual = actual;
    while let Ty::Ref(reference) = actual {
        actual = &reference.referent;
    }
    // A container element may itself be a reference. Receiver inference has
    // then already bound `T = ref U`, while the checker-flattened reference
    // result is spelled `ref U`; stripping its handle above leaves `U`.
    // Preserve the established element solution instead of mistaking the
    // flattened handle for a conflicting `T = U` solution.
    if let Ty::Param { name, .. } = pattern
        && let Some(Ty::Ref(reference)) = bindings.types.get(name)
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
        (TyArg::Val(CtValue::Param(name)), TyArg::Val(value)) => bind_value(name, value, bindings),
        (TyArg::Val(p), TyArg::Val(a)) if p == a => Ok(()),
        (TyArg::Origin(_), TyArg::Origin(_)) => Ok(()),
        _ => Err("generic application arguments disagree".to_string()),
    }
}

pub(super) fn bind_type(name: &str, ty: &Ty, bindings: &mut Bindings) -> Result<(), String> {
    if is_symbolic(ty) {
        return Err(format!("solution for `{name}` is not concrete: `{ty}`"));
    }
    // Solutions join instance identity: erase callable-environment spellings
    // so `capturing[origin@N]` and `thin` variants of one contract are one
    // instance.
    let ty = &canonicalize_callable(ty);
    let literal = |ty: &Ty| matches!(ty, Ty::IntLiteral | Ty::FloatLiteral | Ty::StringLiteral);
    match bindings.types.get(name) {
        // A literal-typed actual materializes into whatever concrete storage
        // is already bound, and a concrete solution upgrades an earlier
        // literal-only binding — mirroring `unify`'s literal escape. Binding
        // order varies by call shape (receiver-first vs result-last), so the
        // merge must be order-independent.
        Some(old) if literal(ty) && !literal(old) => Ok(()),
        Some(old) if literal(old) && !literal(ty) => {
            bindings.types.insert(name.to_string(), ty.clone());
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
            bindings.types.insert(name.to_string(), ty.clone());
            Ok(())
        }
    }
}

pub(super) fn bind_value(
    name: &str,
    value: &CtValue,
    bindings: &mut Bindings,
) -> Result<(), String> {
    if matches!(value, CtValue::Param(_)) {
        return Err(format!("solution for `{name}` is not constant"));
    }
    match bindings.values.get(name) {
        // As in `bind_type`, `Display` can collapse distinct values (an Int
        // and a UInt render alike), so the conflict text carries the
        // structural forms.
        Some(old) if old != value => Err(format!(
            "conflicting solutions for `{name}`: `{old}` ({old:?}) and `{value}` ({value:?})"
        )),
        Some(_) => Ok(()),
        None => {
            bindings.values.insert(name.to_string(), value.clone());
            Ok(())
        }
    }
}
