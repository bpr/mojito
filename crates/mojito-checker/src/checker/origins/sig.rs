//! Origin-signature syntax: interior/subtree origin spellings,
//! origin-expression validation, and ref-signature lowering.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

/// The origin `X` a raw-origin argument `X._mlir_origin` reads: upstream's
/// `!lit.origin` attribute the `Origin` value `X` holds, spelled as a value
/// (`origin_of(x)._mlir_origin`) or a type projection (`o._mlir_origin`).
pub(in crate::checker) fn raw_origin_argument(
    argument: &mojito_ast::ast::ParamArg,
) -> Option<mojito_ast::ast::ParamArg> {
    use mojito_ast::ast::{ParamArg, SourceType};
    /// The raw attribute an origin value holds, upstream's
    /// `Origin._mlir_origin`.
    const MLIR_ORIGIN_FIELD: &str = "_mlir_origin";
    match argument {
        ParamArg::Value(Expr {
            kind: ExprKind::Member { object, field },
            ..
        }) if field == MLIR_ORIGIN_FIELD => Some(ParamArg::Value((**object).clone())),
        ParamArg::Type(SourceType::Assoc { base, name, args })
            if name == MLIR_ORIGIN_FIELD && args.is_empty() =>
        {
            Some(ParamArg::Type((**base).clone()))
        }
        _ => None,
    }
}

/// The declared origin values a signature lowering reads a named origin
/// through ([`Checker::origin_value_of`]): the origin a use names and
/// whether it is mutable, or `None` for a name that is no origin value.
pub(in crate::checker) type OriginValues<'a> =
    &'a dyn Fn(&Expr) -> Option<(mojito_types::origin::Origin, bool)>;

/// Whether `argument` is a raw `!lit.origin` rather than an `Origin`
/// value: an `X._mlir_origin` read ([`raw_origin_argument`]) or a
/// bracketed `__mlir_attr[…]` attribute.
pub(in crate::checker) fn reads_raw_origin(argument: &mojito_ast::ast::ParamArg) -> bool {
    raw_origin_argument(argument).is_some()
        || matches!(argument, mojito_ast::ast::ParamArg::Value(expr)
            if expr.bracket_application().is_some_and(|(name, _)| name == "__mlir_attr"))
}

/// The type of an origin value of mutability `fixed` (`None` when it is
/// symbolic): the `Origin` struct's (`MutOrigin`, `ImmOrigin`), or the raw
/// `!lit.origin`'s (`LITMutOrigin`, `LITImmOrigin`) when `lit`.
pub(in crate::checker) fn origin_value_type(fixed: Option<bool>, lit: bool) -> String {
    match (fixed, lit) {
        (Some(fixed), true) => mojito_types::types::LitOriginMut::Known(fixed).to_string(),
        (None, true) => "LITOrigin".to_string(),
        (Some(true), false) => "MutOrigin".to_string(),
        (Some(false), false) => "ImmOrigin".to_string(),
        (None, false) => "Origin".to_string(),
    }
}

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

/// The owned-interior tags a struct-typed return annotation projects on its
/// receiver origin, innermost first:
/// `StringSpan[origin_of(self)._get_owned_interior["bytes"]]` gives
/// `["bytes"]`. A base under `self` (`origin_of(self.items)`) widens to the
/// receiver, as an abstract signature's `origin_of` does. Any other return,
/// or a projection over a non-receiver base, gives no tags.
pub(in crate::checker) fn view_return_interior_tags(ret: Option<&SourceType>) -> Vec<String> {
    let Some(SourceType::Named(_, arguments)) = ret else {
        return Vec::new();
    };
    arguments
        .iter()
        .find_map(|argument| {
            let mut expression = match argument {
                mojito_ast::ast::ParamArg::Value(expression) => expression,
                mojito_ast::ast::ParamArg::Named { value, .. } => match value.as_ref() {
                    mojito_ast::ast::ParamArg::Value(expression) => expression,
                    _ => return None,
                },
                mojito_ast::ast::ParamArg::Type(_) => return None,
            };
            let mut tags = Vec::new();
            while let Some((base, tag)) = interior_origin_syntax(expression) {
                tags.push(tag.to_string());
                expression = base;
            }
            if tags.is_empty() {
                return None;
            }
            let ExprKind::Call { name, args, .. } = &expression.kind else {
                return None;
            };
            let [place] = args.as_slice() else {
                return None;
            };
            (name == "origin_of"
                && matches!(crate::checker::places::place_path(place), Some(("self", _))))
            .then(|| {
                tags.reverse();
                tags
            })
        })
        .unwrap_or_default()
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

/// The immutable-origin capability cast: current Mojo's `ImmOrigin(o)` /
/// `Origin[mut=False](o)` (the `@implicit` `Origin` conversion). It pins a
/// capability to read-only while keeping `o`'s provenance. Returns the inner
/// origin expression; the upgrade direction (`MutOrigin(o)`, `mut=True`) is
/// rejected. The removed `Origin[mut=...].cast_from[o]` spelling is rejected
/// with a migration diagnostic, as `StaticConstantOrigin` is.
pub(in crate::checker) fn immutable_origin_cast(
    expression: &Expr,
) -> Option<Result<&Expr, TypeError>> {
    if let ExprKind::Call {
        name,
        args,
        kwargs,
        param_args,
    } = &expression.kind
    {
        let [inner] = args.as_slice() else {
            return None;
        };
        if !kwargs.is_empty() {
            return None;
        }
        let mutable = match (name.as_str(), param_args.as_slice()) {
            ("ImmOrigin", []) => false,
            ("MutOrigin", []) => true,
            (
                "Origin",
                [
                    mojito_ast::ast::ParamArg::Named {
                        name: keyword,
                        value,
                    },
                ],
            ) if keyword == "mut" => match value.as_ref() {
                mojito_ast::ast::ParamArg::Value(Expr {
                    kind: ExprKind::Bool(flag),
                    ..
                }) => *flag,
                _ => return None,
            },
            _ => return None,
        };
        return Some(if mutable {
            Err(TypeError::Unsupported(format!(
                "an origin cast cannot upgrade capability: '{name}(...)' with a mutable \
                 origin is rejected"
            )))
        } else {
            Ok(inner)
        });
    }
    let ExprKind::Index { object, .. } = &expression.kind else {
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
    let (name, _) = applied.bracket_application()?;
    if name != "Origin" {
        return None;
    }
    Some(Err(TypeError::Unsupported(
        "'Origin[mut=...].cast_from[o]' was removed; use 'ImmOrigin(o)'".to_string(),
    )))
}

pub(in crate::checker) fn validate_origin_expr(
    expr: &Expr,
    origin_params: &HashSet<&str>,
    value_params: &HashSet<&str>,
    values: OriginValues<'_>,
) -> Result<(), TypeError> {
    if let Some(inner) = immutable_origin_cast(expr) {
        return validate_origin_expr(inner?, origin_params, value_params, values);
    }
    if let Some((base, _)) = interior_origin_syntax(expr) {
        return validate_origin_expr(base, origin_params, value_params, values);
    }
    if subtree_origin_syntax(expr).is_some() {
        return Err(reject_subtree_origin_here("a reference origin clause"));
    }
    match &expr.kind {
        ExprKind::Identifier(name)
            if name == "_"
                || name == "self"
                || origin_params.contains(name.as_str())
                || value_params.contains(name.as_str()) =>
        {
            Ok(())
        }
        _ if values(expr).is_some() => Ok(()),
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
    decls: &[ParamDecl],
    params: &[&FnParam],
    struct_params: usize,
    values: OriginValues<'_>,
) -> Result<Vec<Option<mojito_types::origin::RefSig>>, TypeError> {
    params
        .iter()
        .map(|param| {
            if param.convention != Some(ArgConvention::Ref) {
                return Ok(None);
            }
            match &param.origin {
                Some(spec) => lower_ref_sig(spec, decls, params, struct_params, values).map(Some),
                None => Ok(Some(mojito_types::origin::RefSig {
                    origin: mojito_types::origin::SigOrigin::Infer,
                    mutability: mojito_types::origin::SigMutability::Infer,
                })),
            }
        })
        .collect()
}

/// The origin signature of a callable declaring `type_params`, classified
/// as `decls`: one [`CallableOriginParam`] per `Origin` binder of `decls`,
/// in declared order, with the value parameters whose clause names it.
pub(in crate::checker) fn callable_origin_signature(
    type_params: &[mojito_ast::ast::TypeParam],
    decls: &[ParamDecl],
    params: &[&FnParam],
    availability: Vec<GenericConstraint>,
) -> CallableOriginSignature {
    let origins = decls
        .iter()
        .filter(|decl| decl.is_origin() && !is_origin_set(decl))
        .map(|parameter| CallableOriginParam {
            name: parameter.name().to_string(),
            slots: params
                .iter()
                .enumerate()
                .filter_map(|(index, value_parameter)| {
                    value_parameter
                        .origin
                        .as_ref()
                        .is_some_and(|origin| {
                            origin.iter().any(|expression| {
                                origin_binder_name(expression) == Some(parameter.name())
                            })
                        })
                        .then_some(index)
                })
                .collect(),
        })
        .collect::<Vec<_>>();
    CallableOriginSignature {
        origins,
        binder_names: type_params
            .iter()
            .map(|parameter| parameter.name.clone())
            .collect(),
        availability,
        view_return: Vec::new(),
        unavailable: None,
    }
}

impl Checker {
    /// Reject a result that names the origin of a value with no memory to
    /// borrow, as upstream does: every `origin_of(p)` in the result
    /// annotation (`ret`, else the named `out` result's) judges its root `p`
    /// — a parameter of `params`, typed by `param_tys`, or the `receiver` —
    /// by convention and type. A `ref` place always has memory. Any other
    /// may not name a type that is, or might expand to, `RegisterPassable`,
    /// and a read trivially register-passable one has no memory origin at
    /// all. A root the
    /// signature cannot resolve is left to origin lowering.
    pub(in crate::checker) fn check_result_origins(
        &self,
        ret: Option<&SourceType>,
        params: &[FnParam],
        param_tys: &[Ty],
        receiver: Option<(Option<ArgConvention>, &Ty)>,
    ) -> Result<(), TypeError> {
        use super::traits::RegisterPassability;
        let Some(annotation) =
            ret.or_else(|| mojito_ast::ast::named_result(params).map(|result| &result.ty))
        else {
            return Ok(());
        };
        let mut roots = Vec::new();
        result_origin_roots(annotation, &mut roots);
        for root in roots {
            let (convention, ty) = if root == "self" {
                match receiver {
                    Some(receiver) => receiver,
                    None => continue,
                }
            } else {
                match params.iter().position(|param| param.name == root) {
                    Some(index) if index < param_tys.len() => {
                        (params[index].convention, &param_tys[index])
                    }
                    _ => continue,
                }
            };
            if matches!(convention, Some(ArgConvention::Ref | ArgConvention::Out)) {
                continue;
            }
            let read = matches!(convention, None | Some(ArgConvention::Imm));
            let place = root.to_string();
            match self.register_passability(ty) {
                RegisterPassability::Excluded => {}
                RegisterPassability::Possible => {
                    return Err(TypeError::ReturnedOriginRegisterPassable { place, ty: None });
                }
                RegisterPassability::Proven if read && self.is_trivial_register_passable(ty) => {
                    return Err(TypeError::NoMemoryOrigin {
                        ty: ty.to_string(),
                        context: "origin specifier".into(),
                    });
                }
                RegisterPassability::Proven => {
                    return Err(TypeError::ReturnedOriginRegisterPassable {
                        place,
                        ty: Some(ty.to_string()),
                    });
                }
            }
        }
        Ok(())
    }

    /// The origin slots a struct-typed return annotation binds to the
    /// callee's places (`-> P[origin_of(xs)]`, `-> Wrap[origin_of(self.xs)]`),
    /// one [`ViewReturnOrigin`] per resolvable slot. Each slot's argument
    /// lowers like a `ref [...]` clause; its mutability is the callee's own
    /// capability over the named place — a `mut` parameter or `mut self`
    /// writes, a read parameter or receiver does not — so a call site can
    /// judge a write through the result's pointer field as upstream does. A
    /// placeholder slot, or one naming what the signature cannot lower, is
    /// left symbolic.
    pub(in crate::checker) fn view_return_origins(
        &self,
        ret: Option<&SourceType>,
        decls: &[ParamDecl],
        params: &[&FnParam],
        struct_params: usize,
        self_convention: Option<ArgConvention>,
    ) -> Vec<ViewReturnOrigin> {
        use mojito_types::origin::{SigMutability, SigOrigin};
        let Some(SourceType::Named(name, arguments)) = ret else {
            return Vec::new();
        };
        let Some(info) = self.structs.get(name) else {
            return Vec::new();
        };
        let origin_id = |decl: &ParamDecl| match decl {
            ParamDecl::Origin { origin, .. } if !is_origin_set(decl) => Some(*origin),
            _ => None,
        };
        let explicit: Vec<&ParamDecl> = info.explicit_params().collect();
        // The capability the callee itself holds over the place a lowered
        // origin names.
        let place_mutability = |origin: &SigOrigin| -> Option<SigMutability> {
            let mut base = origin;
            while let SigOrigin::Projected(inner, _) = base {
                base = inner;
            }
            let convention = match base {
                SigOrigin::Self_ => self_convention,
                SigOrigin::Param(index) => params.get(*index)?.convention,
                _ => return None,
            };
            Some(match convention {
                Some(ArgConvention::Mut) => SigMutability::Mutable,
                Some(ArgConvention::Ref) => SigMutability::Infer,
                _ => SigMutability::Immutable,
            })
        };
        let mut slots = Vec::new();
        let mut positional = explicit.iter();
        for argument in arguments {
            let (slot, expression) = match argument {
                mojito_ast::ast::ParamArg::Named { name, value } => {
                    let Some(slot) = explicit
                        .iter()
                        .filter(|decl| decl.name() == name)
                        .find_map(|decl| origin_id(decl))
                    else {
                        continue;
                    };
                    match value.as_ref() {
                        mojito_ast::ast::ParamArg::Value(expression) => (slot, expression),
                        _ => continue,
                    }
                }
                mojito_ast::ast::ParamArg::Value(expression) => {
                    let Some(decl) = positional.next() else {
                        break;
                    };
                    let Some(slot) = origin_id(decl) else {
                        continue;
                    };
                    (slot, expression)
                }
                mojito_ast::ast::ParamArg::Type(_) => {
                    positional.next();
                    continue;
                }
            };
            // A struct's own origin binder (`Self.origin`) names no place of
            // the callee, so it is no view slot.
            if matches!(&expression.kind, ExprKind::Identifier(name) if name == "_" || name == "...")
                || expression.self_member().is_some()
            {
                continue;
            }
            let spec: mojito_ast::ast::OriginSpec = vec![expression.clone()];
            let Ok(mut sig) = lower_ref_sig(&spec, decls, params, struct_params, &|expr| {
                self.origin_value_of(expr)
            }) else {
                continue;
            };
            if sig.mutability == SigMutability::Infer
                && let Some(mutability) = place_mutability(&sig.origin)
            {
                sig.mutability = mutability;
            }
            slots.push(ViewReturnOrigin { slot, sig });
        }
        slots
    }
}

/// The name of a callable's `read` heterogeneous collector (`*args: *Ts`),
/// the one collector whose `VariadicPack` type carries the callable's
/// implicit pack origin.
pub(in crate::checker) fn pack_collector(params: &[FnParam]) -> Option<&str> {
    params
        .iter()
        .find(|param| {
            param.kind == mojito_ast::ast::ParamKind::Variadic
                && matches!(param.convention, None | Some(ArgConvention::Imm))
                && matches!(&param.ty, mojito_ast::ast::Type::Named(name, _) if name.starts_with('*'))
        })
        .map(|param| param.name.as_str())
}

/// Whether an origin-clause member names `collector`'s pack origin, as
/// upstream spells it: `args.origin` or `type_of(args).origin`.
pub(in crate::checker) fn names_pack_origin(member: &Expr, collector: &str) -> bool {
    let ExprKind::Member { object, field } = &member.kind else {
        return false;
    };
    let names_collector = |expression: &Expr| matches!(&expression.kind, ExprKind::Identifier(name) if name == collector);
    field == "origin"
        && match &object.kind {
            ExprKind::Identifier(_) => names_collector(object),
            ExprKind::Call {
                name,
                args,
                kwargs,
                param_args,
            } => {
                name == "type_of"
                    && kwargs.is_empty()
                    && param_args.is_empty()
                    && matches!(args.as_slice(), [argument] if names_collector(argument))
            }
            _ => false,
        }
}

/// The [`RefSig`](mojito_types::origin::RefSig) a `ref [spec]` clause
/// declares, its binders read from the classified list `decls` (the
/// enclosing struct's first `struct_params` origin ids, then the callable's).
pub(in crate::checker) fn lower_ref_sig(
    spec: &mojito_ast::ast::OriginSpec,
    decls: &[ParamDecl],
    params: &[&FnParam],
    struct_params: usize,
    values: OriginValues<'_>,
) -> Result<mojito_types::origin::RefSig, TypeError> {
    use mojito_types::origin::{Origin, RefSig, SigMutability, SigOrigin};
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
            let base = lower_sig_origin_expression(base_expression, decls, params, struct_params)?;
            // A projection off a named origin parameter carries that
            // parameter's declared mutability, exactly like the bare binder
            // (either spelling: `o` or `Self.o`).
            if let Some(base_name) = origin_binder_name(base_expression)
                && let Some((decl, _)) = origin_decl(decls, base_name)
            {
                mutability = origin_decl_mutability(decl, decls);
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
            _ if let Some((origin, mutable)) = values(expression).filter(|_| {
                origin_binder_name(expression).is_none_or(|name| origin_decl(decls, name).is_none())
            }) =>
            {
                members.push(match origin {
                    Origin::Static => SigOrigin::Static,
                    Origin::Untracked { mutable } => SigOrigin::Untracked { mutable },
                    Origin::UnsafeAny { mutable } => SigOrigin::UnsafeAny { mutable },
                    other => SigOrigin::Bound(other),
                });
                mutability = if mutable {
                    SigMutability::Mutable
                } else {
                    SigMutability::Immutable
                };
            }
            ExprKind::Identifier(name) => {
                if let Some(index) = params.iter().position(|param| param.name == *name) {
                    members.push(SigOrigin::Param(index));
                    continue;
                }
                // The pin requires the qualified spelling for struct origin
                // parameters referenced from member clauses.
                let (decl, origin_id) = origin_decl(decls, name)
                    .ok_or_else(|| TypeError::UndefinedVariable(name.clone()))?;
                if !name.starts_with("__") && (origin_id.0 as usize) < struct_params {
                    return Err(unqualified_struct_binder(name));
                }
                mutability = origin_decl_mutability(decl, decls);
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
                        origin_id,
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
                    && let Some((decl, origin_id)) = origin_decl(decls, field) =>
            {
                mutability = origin_decl_mutability(decl, decls);
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
                        origin_id,
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

/// The `Origin` binder `name` declares in the classified list `decls`
/// (never an `OriginSet` one), with the
/// [`OriginParamId`](mojito_types::origin::OriginParamId) it was classified
/// at.
pub(in crate::checker) fn origin_decl<'a>(
    decls: &'a [ParamDecl],
    name: &str,
) -> Option<(&'a ParamDecl, mojito_types::origin::OriginParamId)> {
    decls.iter().find_map(|decl| match decl {
        ParamDecl::Origin { origin, .. } if decl.name() == name && !is_origin_set(decl) => {
            Some((decl, *origin))
        }
        _ => None,
    })
}

/// The declared mutability of `decl`, an origin binder of `decls`, as a
/// signature reads it: `Fixed` as `Mutable`/`Immutable`, a sibling `Bool`
/// binder as `SigMutability::BoolParam` by its source index (the
/// `OriginParamId` domain, which no pack's implicit list binder takes a
/// slot in), and anything else as `Infer`.
pub(in crate::checker) fn origin_decl_mutability(
    decl: &ParamDecl,
    decls: &[ParamDecl],
) -> mojito_types::origin::SigMutability {
    use mojito_types::origin::SigMutability;
    use mojito_types::types::OriginMutability;
    let ParamDecl::Origin { mutability, .. } = decl else {
        return SigMutability::Infer;
    };
    match mutability {
        OriginMutability::Fixed(true) => SigMutability::Mutable,
        OriginMutability::Fixed(false) => SigMutability::Immutable,
        OriginMutability::Binder(binder) => decls
            .iter()
            .position(|sibling| *sibling.id() == binder.id)
            .map_or(SigMutability::Infer, |slot| {
                SigMutability::BoolParam(
                    decls[..slot]
                        .iter()
                        .filter(|earlier| !earlier.is_pack_list())
                        .count(),
                )
            }),
        OriginMutability::Set => SigMutability::Infer,
    }
}

/// The contract-level origin a named struct/callable origin binder denotes:
/// the value parameter(s) carrying it, else the checked semantic binder
/// itself (an enclosing struct origin bound only by reference-valued fields).
pub(in crate::checker) fn sig_origin_for_binder(
    name: &str,
    decls: &[ParamDecl],
    params: &[&FnParam],
) -> Option<mojito_types::origin::SigOrigin> {
    use mojito_types::origin::SigOrigin;
    let (_, origin_id) = origin_decl(decls, name)?;
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
        [] => SigOrigin::Bound(mojito_types::origin::Origin::Param(origin_id)),
        [single] => single.clone(),
        _ => SigOrigin::union(members),
    })
}

/// The root of every `origin_of(...)` place a result annotation names, in
/// source order: in a `ref [...]` clause, in a struct or associated-type
/// application's arguments, at any depth. A function type's own origins
/// are its contract's, not the result's.
fn result_origin_roots<'a>(annotation: &'a SourceType, roots: &mut Vec<&'a str>) {
    fn argument_roots<'a>(argument: &'a mojito_ast::ast::ParamArg, roots: &mut Vec<&'a str>) {
        match argument {
            mojito_ast::ast::ParamArg::Type(ty) => result_origin_roots(ty, roots),
            mojito_ast::ast::ParamArg::Value(expression) => origin_roots(expression, roots),
            mojito_ast::ast::ParamArg::Named { value, .. } => argument_roots(value, roots),
        }
    }
    match annotation {
        SourceType::Ref { referent, origin } => {
            for expression in origin.iter().flatten() {
                origin_roots(expression, roots);
            }
            result_origin_roots(referent, roots);
        }
        SourceType::Named(_, arguments) => {
            for argument in arguments {
                argument_roots(argument, roots);
            }
        }
        SourceType::Assoc {
            base,
            args: arguments,
            ..
        } => {
            result_origin_roots(base, roots);
            for argument in arguments {
                argument_roots(argument, roots);
            }
        }
        SourceType::IndexedProjection { base, index } => {
            result_origin_roots(base, roots);
            origin_roots(index, roots);
        }
        _ => {}
    }
}

/// The `origin_of(...)` place roots of one origin expression, under its
/// immutable cast and interior projections.
fn origin_roots<'a>(expression: &'a Expr, roots: &mut Vec<&'a str>) {
    let mut expression = expression;
    loop {
        if let Some(Ok(inner)) = immutable_origin_cast(expression) {
            expression = inner;
        } else if let Some((base, _)) = interior_origin_syntax(expression) {
            expression = base;
        } else if let Some(base) = subtree_origin_syntax(expression) {
            expression = base;
        } else {
            break;
        }
    }
    if let ExprKind::Call { name, args, .. } = &expression.kind
        && name == "origin_of"
    {
        roots.extend(
            args.iter()
                .filter_map(|argument| place_path(argument).map(|(root, _)| root)),
        );
    }
}
