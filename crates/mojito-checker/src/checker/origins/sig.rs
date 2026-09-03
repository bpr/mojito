//! Origin-signature syntax: interior/subtree origin spellings,
//! origin-expression validation, and ref-signature lowering.

use super::*;

/// Recognize the current-nightly origin-attribute spelling
/// `base._get_owned_interior["tag"]`. It is accepted only in origin clauses;
/// ordinary expression typing still has no runtime member by this name.
pub(in crate::checker) fn interior_origin_syntax(expr: &Expr) -> Option<(&Expr, &str)> {
    let ExprKind::Index { object, index } = &expr.kind else {
        return None;
    };
    let ExprKind::Member {
        object: base,
        field,
    } = &object.kind
    else {
        return None;
    };
    let ExprKind::Str(name) = &index.kind else {
        return None;
    };
    (field == "_get_owned_interior").then_some((base, name.as_str()))
}

/// Recognize the experimental conservative-origin spelling `base._subtree`
/// (current Mojo's `Origin._subtree` member). Like `_get_owned_interior`, it is
/// accepted only in origin positions — this pass, Pointer origin arguments and
/// `unsafe_origin_cast` targets; `ref [...]` clauses reject it explicitly.
pub(in crate::checker) fn subtree_origin_syntax(expr: &Expr) -> Option<&Expr> {
    let ExprKind::Member { object, field } = &expr.kind else {
        return None;
    };
    (field == "_subtree").then_some(object)
}

/// The uniform rejection for `._subtree` in origin positions outside the
/// accepted first-pass surface.
pub(in crate::checker) fn reject_subtree_origin_here(context: &str) -> TypeError {
    TypeError::Unsupported(format!(
        "'_subtree' origins are supported only as Pointer origin arguments and \
         unsafe_origin_cast targets, not in {context}"
    ))
}

/// The declaration-level immutable-origin cast `Origin[mut=False].cast_from[o]`
/// — current Mojo's spelling for pinning a reference result's capability to
/// read-only independent of the origin parameter's own `mut=`. Returns the
/// inner origin expression; the upgrade direction (`mut=True`) is rejected.
pub(in crate::checker) fn immutable_origin_cast(
    expression: &Expr,
) -> Option<Result<&Expr, TypeError>> {
    let ExprKind::Index { object, index } = &expression.kind else {
        return None;
    };
    let ExprKind::Member {
        object: applied,
        field,
    } = &object.kind
    else {
        return None;
    };
    if field != "cast_from" {
        return None;
    }
    let ExprKind::TypeApply { name, args } = &applied.kind else {
        return None;
    };
    if name != "Origin" {
        return None;
    }
    let [
        mojito_ast::ast::ParamArg::Named {
            name: keyword,
            value,
        },
    ] = args.as_slice()
    else {
        return None;
    };
    if keyword != "mut" {
        return None;
    }
    let mojito_ast::ast::ParamArg::Value(flag) = value.as_ref() else {
        return None;
    };
    match &flag.kind {
        ExprKind::Bool(false) => Some(Ok(index)),
        ExprKind::Bool(true) => Some(Err(TypeError::Unsupported(
            "an origin cast cannot upgrade capability: 'Origin[mut=True].cast_from' is rejected"
                .to_string(),
        ))),
        _ => None,
    }
}

pub(in crate::checker) fn validate_origin_expr(
    expr: &Expr,
    origin_params: &HashSet<&str>,
    value_params: &HashSet<&str>,
) -> Result<(), TypeError> {
    if let Some(inner) = immutable_origin_cast(expr) {
        return validate_origin_expr(inner?, origin_params, value_params);
    }
    if let Some((base, _)) = interior_origin_syntax(expr) {
        return validate_origin_expr(base, origin_params, value_params);
    }
    if subtree_origin_syntax(expr).is_some() {
        return Err(reject_subtree_origin_here("a reference origin clause"));
    }
    match &expr.kind {
        ExprKind::Identifier(name)
            if name == "_"
                || name == "self"
                || name == "ImmStaticOrigin"
                || name == "ImmUntrackedOrigin"
                || name == "MutUnsafeAnyOrigin"
                || origin_params.contains(name.as_str())
                || value_params.contains(name.as_str()) =>
        {
            Ok(())
        }
        ExprKind::Call {
            name,
            args,
            kwargs,
            param_args,
        } if name == "origin_of" && kwargs.is_empty() && param_args.is_empty() => {
            if args.is_empty() {
                return Err(TypeError::Unsupported(
                    "origin_of requires at least one parameter place".to_string(),
                ));
            }
            for argument in args {
                let Some((root, _)) = place_path(argument) else {
                    return Err(TypeError::Unsupported(
                        "origin_of requires parameter places".to_string(),
                    ));
                };
                if root != "self" && !value_params.contains(root) {
                    return Err(TypeError::UndefinedVariable(root.to_string()));
                }
            }
            Ok(())
        }
        // Upstream's qualified struct-binder spelling: `ref [Self.o]` names
        // the enclosing origin parameter exactly like the bare binder.
        ExprKind::Member { object, field }
            if matches!(&object.kind, ExprKind::Identifier(name) if name == "Self")
                && origin_params.contains(field.as_str()) =>
        {
            Ok(())
        }
        ExprKind::Member { .. } | ExprKind::Index { .. } => {
            let Some((root, _)) = place_path(expr) else {
                return Err(TypeError::Unsupported("invalid origin place".to_string()));
            };
            if root == "self" || value_params.contains(root) {
                Ok(())
            } else {
                Err(TypeError::UndefinedVariable(root.to_string()))
            }
        }
        ExprKind::Identifier(name) => Err(TypeError::UndefinedVariable(name.clone())),
        _ => Err(TypeError::Unsupported(
            "origin clauses must name origins or parameter places".to_string(),
        )),
    }
}

/// The pin's rejection for a bare struct-parameter reference in a member
/// origin clause.
pub(in crate::checker) fn unqualified_struct_binder(name: &str) -> TypeError {
    TypeError::Unsupported(format!(
        "unqualified access to struct parameter '{name}'; use 'Self.{name}' instead"
    ))
}

/// Whether a signature origin names parameter slot `index` (directly, or
/// under a projection or union).
pub(in crate::checker) fn sig_origin_mentions_param(
    origin: &mojito_types::origin::SigOrigin,
    index: usize,
) -> bool {
    use mojito_types::origin::SigOrigin;
    match origin {
        SigOrigin::Param(slot) => *slot == index,
        SigOrigin::Projected(base, _) => sig_origin_mentions_param(base, index),
        SigOrigin::Union(members) => members
            .iter()
            .any(|member| sig_origin_mentions_param(member, index)),
        _ => false,
    }
}

/// Whether a delegated-call receiver path (`self.a.b`) roots at `self`.
pub(in crate::checker) fn receiver_rooted_at_self(object: &Expr) -> bool {
    let mut root = object;
    while let ExprKind::Member { object, .. } = &root.kind {
        root = object;
    }
    matches!(&root.kind, ExprKind::Identifier(name) if name == "self")
}

/// The origin-parameter binder an origin-clause expression names: the bare
/// binder (`o`) or upstream's qualified struct-binder spelling (`Self.o`).
pub(in crate::checker) fn origin_binder_name(expression: &Expr) -> Option<&str> {
    match &expression.kind {
        ExprKind::Identifier(name) => Some(name),
        ExprKind::Member { object, field } if matches!(&object.kind, ExprKind::Identifier(name) if name == "Self") => {
            Some(field)
        }
        _ => None,
    }
}

pub(in crate::checker) fn lower_ref_param_sigs(
    type_params: &[mojito_ast::ast::TypeParam],
    params: &[&FnParam],
    struct_params: usize,
) -> Result<Vec<Option<mojito_types::origin::RefSig>>, TypeError> {
    params
        .iter()
        .map(|param| {
            if param.convention != Some(ArgConvention::Ref) {
                return Ok(None);
            }
            match &param.origin {
                Some(spec) => lower_ref_sig(spec, type_params, params, struct_params).map(Some),
                None => Ok(Some(mojito_types::origin::RefSig {
                    origin: mojito_types::origin::SigOrigin::Infer,
                    mutability: mojito_types::origin::SigMutability::Infer,
                })),
            }
        })
        .collect()
}

pub(in crate::checker) fn callable_origin_signature(
    type_params: &[mojito_ast::ast::TypeParam],
    params: &[&FnParam],
    availability: Vec<GenericConstraint>,
) -> CallableOriginSignature {
    let origins = type_params
        .iter()
        .filter(|parameter| parameter.bounds.as_slice() == ["Origin"])
        .map(|parameter| CallableOriginParam {
            name: parameter.name.clone(),
            slots: params
                .iter()
                .enumerate()
                .filter_map(|(index, value_parameter)| {
                    value_parameter
                        .origin
                        .as_ref()
                        .is_some_and(|origin| {
                            origin.iter().any(|expression| {
                                origin_binder_name(expression) == Some(parameter.name.as_str())
                            })
                        })
                        .then_some(index)
                })
                .collect(),
        })
        .collect::<Vec<_>>();
    let source = type_params
        .iter()
        .map(|parameter| CallableSourceParam {
            name: parameter.name.clone(),
            infer_only: parameter.infer_only,
            origin: origins
                .iter()
                .position(|origin| origin.name == parameter.name),
            ordinary: !matches!(
                parameter.bounds.as_slice(),
                [only] if only == "Origin" || only == "OriginSet"
            ),
        })
        .collect();
    CallableOriginSignature {
        origins,
        source,
        availability,
    }
}

pub(in crate::checker) fn lower_ref_sig(
    spec: &mojito_ast::ast::OriginSpec,
    type_params: &[mojito_ast::ast::TypeParam],
    params: &[&FnParam],
    struct_params: usize,
) -> Result<mojito_types::origin::RefSig, TypeError> {
    use mojito_types::origin::{RefSig, SigMutability, SigOrigin};
    let mut members = Vec::new();
    let mut mutability = SigMutability::Infer;
    let mut cast_immutable = false;
    for expression in spec {
        // The immutable-origin cast wraps one member; unwrap it and pin the
        // whole signature's capability after the member lowers normally.
        let expression = match immutable_origin_cast(expression) {
            Some(inner) => {
                cast_immutable = true;
                inner?
            }
            None => expression,
        };
        if subtree_origin_syntax(expression).is_some() {
            return Err(reject_subtree_origin_here("a reference origin clause"));
        }
        if let Some((base_expression, name)) = interior_origin_syntax(expression) {
            let base =
                lower_sig_origin_expression(base_expression, type_params, params, struct_params)?;
            // A projection off a named origin parameter carries that
            // parameter's declared mutability, exactly like the bare binder
            // (either spelling: `o` or `Self.o`).
            if let Some(base_name) = origin_binder_name(base_expression)
                && let Some(origin_param) = type_params.iter().find(|parameter| {
                    parameter.name == base_name && parameter.bounds.as_slice() == ["Origin"]
                })
            {
                mutability = origin_param_mutability(origin_param, type_params);
            }
            members.push(SigOrigin::Projected(
                Box::new(base),
                vec![mojito_types::origin::OriginSeg::Interior(name.to_string())],
            ));
            continue;
        }
        match &expression.kind {
            ExprKind::Identifier(name) if name == "_" => members.push(SigOrigin::Infer),
            ExprKind::Identifier(name) if name == "self" => members.push(SigOrigin::Self_),
            ExprKind::Identifier(name) if name == "ImmStaticOrigin" => {
                members.push(SigOrigin::Static);
                mutability = SigMutability::Immutable;
            }
            ExprKind::Identifier(name) if name == "ImmUntrackedOrigin" => {
                members.push(SigOrigin::Untracked { mutable: false });
                mutability = SigMutability::Immutable;
            }
            ExprKind::Identifier(name) if name == "MutUnsafeAnyOrigin" => {
                members.push(SigOrigin::Untracked { mutable: true });
                mutability = SigMutability::Mutable;
            }
            ExprKind::Identifier(name) => {
                if let Some(index) = params.iter().position(|param| param.name == *name) {
                    members.push(SigOrigin::Param(index));
                    continue;
                }
                // The pin requires the qualified spelling for struct origin
                // parameters referenced from member clauses.
                if !name.starts_with("__")
                    && type_params[..struct_params.min(type_params.len())]
                        .iter()
                        .any(|parameter| {
                            parameter.name == *name && parameter.bounds.as_slice() == ["Origin"]
                        })
                {
                    return Err(unqualified_struct_binder(name));
                }
                let (origin_param_index, origin_param) = type_params
                    .iter()
                    .enumerate()
                    .find(|(_, param)| param.name == *name && param.bounds.as_slice() == ["Origin"])
                    .ok_or_else(|| TypeError::UndefinedVariable(name.clone()))?;
                mutability = origin_param_mutability(origin_param, type_params);
                let first_member = members.len();
                for (index, param) in params.iter().enumerate() {
                    if param.origin.as_ref().is_some_and(|origin| {
                        matches!(origin.as_slice(), [expression] if origin_binder_name(expression) == Some(name.as_str()))
                    }) {
                        members.push(SigOrigin::Param(index));
                    }
                }
                // An enclosing struct Origin can be carried by reference-valued
                // fields even when no ordinary method parameter binds it. Keep
                // that checked semantic binder directly in the method contract
                // instead of collapsing it to an empty inferred union.
                if members.len() == first_member {
                    members.push(SigOrigin::Bound(mojito_types::origin::Origin::Param(
                        mojito_types::origin::OriginParamId(origin_param_index as u32),
                    )));
                }
            }
            ExprKind::Call { name, args, .. } if name == "origin_of" => {
                for argument in args {
                    let (root, path) = place_path(argument).ok_or_else(|| {
                        TypeError::Unsupported("origin_of requires parameter places".to_string())
                    })?;
                    let base = if root == "self" {
                        SigOrigin::Self_
                    } else {
                        let index = params
                            .iter()
                            .position(|param| param.name == root)
                            .ok_or_else(|| TypeError::UndefinedVariable(root.to_string()))?;
                        SigOrigin::Param(index)
                    };
                    members.push(project_sig_origin(base, &path));
                }
            }
            // Upstream's qualified struct-binder spelling: `ref [Self.o]` names
            // the enclosing origin parameter exactly like the bare binder.
            ExprKind::Member { object, field }
                if matches!(&object.kind, ExprKind::Identifier(name) if name == "Self")
                    && type_params.iter().any(|param| {
                        param.name == *field && param.bounds.as_slice() == ["Origin"]
                    }) =>
            {
                let (origin_param_index, origin_param) = type_params
                    .iter()
                    .enumerate()
                    .find(|(_, param)| {
                        param.name == *field && param.bounds.as_slice() == ["Origin"]
                    })
                    .expect("guarded above");
                mutability = origin_param_mutability(origin_param, type_params);
                let first_member = members.len();
                for (index, param) in params.iter().enumerate() {
                    if param.origin.as_ref().is_some_and(|origin| {
                        matches!(origin.as_slice(), [expression] if origin_binder_name(expression) == Some(field.as_str()))
                    }) {
                        members.push(SigOrigin::Param(index));
                    }
                }
                if members.len() == first_member {
                    members.push(SigOrigin::Bound(mojito_types::origin::Origin::Param(
                        mojito_types::origin::OriginParamId(origin_param_index as u32),
                    )));
                }
            }
            ExprKind::Member { .. } | ExprKind::Index { .. } => {
                let (root, path) = place_path(expression)
                    .ok_or_else(|| TypeError::Unsupported("invalid origin place".to_string()))?;
                let base = if root == "self" {
                    SigOrigin::Self_
                } else {
                    let index = params
                        .iter()
                        .position(|param| param.name == root)
                        .ok_or_else(|| TypeError::UndefinedVariable(root.to_string()))?;
                    SigOrigin::Param(index)
                };
                members.push(project_sig_origin(base, &path));
            }
            _ => {
                return Err(TypeError::Unsupported(
                    "unsupported origin contract".to_string(),
                ));
            }
        }
    }
    members.sort_by_key(|member| match member {
        SigOrigin::Self_ => 0,
        SigOrigin::Param(i) => i + 1,
        _ => usize::MAX,
    });
    members.dedup();
    let origin = match members.as_slice() {
        [] => SigOrigin::Infer,
        [single] => single.clone(),
        _ => SigOrigin::union(members),
    };
    if cast_immutable {
        mutability = SigMutability::Immutable;
    }
    Ok(RefSig { origin, mutability })
}

/// The declared mutability of a named `Origin[mut=...]` type parameter:
/// literal `True`/`False`, a sibling `Bool` parameter (`SigMutability::
/// BoolParam` by its index among the raw type parameters), or `Infer`.
pub(in crate::checker) fn origin_param_mutability(
    origin_param: &mojito_ast::ast::TypeParam,
    type_params: &[mojito_ast::ast::TypeParam],
) -> mojito_types::origin::SigMutability {
    use mojito_types::origin::SigMutability;
    match origin_param.origin_mutability.as_ref().map(|e| &e.kind) {
        Some(ExprKind::Bool(true)) => SigMutability::Mutable,
        Some(ExprKind::Bool(false)) => SigMutability::Immutable,
        Some(ExprKind::Identifier(value)) => SigMutability::BoolParam(
            type_params
                .iter()
                .position(|parameter| {
                    parameter.name == *value && parameter.bounds.as_slice() == ["Bool"]
                })
                .expect("validated Origin mutability names a Bool parameter"),
        ),
        _ => SigMutability::Infer,
    }
}

/// The contract-level origin a named struct/callable origin binder denotes:
/// the value parameter(s) carrying it, else the checked semantic binder
/// itself (an enclosing struct origin bound only by reference-valued fields).
pub(in crate::checker) fn sig_origin_for_binder(
    name: &str,
    type_params: &[mojito_ast::ast::TypeParam],
    params: &[&FnParam],
) -> Option<mojito_types::origin::SigOrigin> {
    use mojito_types::origin::SigOrigin;
    let (origin_param_index, _) = type_params.iter().enumerate().find(|(_, parameter)| {
        parameter.name == *name && parameter.bounds.as_slice() == ["Origin"]
    })?;
    let members = params
        .iter()
        .enumerate()
        .filter_map(|(index, parameter)| {
            parameter
                .origin
                .as_ref()
                .is_some_and(|origin| {
                    matches!(origin.as_slice(), [expression] if origin_binder_name(expression) == Some(name))
                })
                .then_some(SigOrigin::Param(index))
        })
        .collect::<Vec<_>>();
    Some(match members.as_slice() {
        [] => SigOrigin::Bound(mojito_types::origin::Origin::Param(
            mojito_types::origin::OriginParamId(origin_param_index as u32),
        )),
        [single] => single.clone(),
        _ => SigOrigin::union(members),
    })
}
