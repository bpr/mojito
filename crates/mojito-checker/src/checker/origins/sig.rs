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

/// The `std.origin` declarations a signature lowering, which holds no
/// checker, reads origins through; the checker implements it.
pub(in crate::checker) trait OriginDecls {
    /// The origin a use of a declared origin value names, and whether it is
    /// mutable ([`Checker::named_origin_value`]); `None` for an expression
    /// naming no origin value.
    fn value(&self, expr: &Expr) -> Option<(mojito_types::origin::Origin, bool)>;

    /// The operand of an `Origin` conversion (`ImmOrigin(o)`), read
    /// through `Origin`'s declared `@implicit` initializer
    /// ([`Checker::origin_conversion`]), and whether its result is
    /// mutable; `None` for an expression that is no such conversion.
    fn conversion<'e>(&self, expr: &'e Expr) -> Option<Result<(&'e Expr, bool), TypeError>>;

    /// The address space a `ref[...]` clause operand
    /// [`is_address_space_operand`] names ([`Checker::address_space_arg`]).
    fn address_space(
        &self,
        expr: &Expr,
    ) -> Result<mojito_types::types::AddressSpaceSlot, TypeError>;
}

/// Whether the `ref[...]` clause operand `expr` is the reference's address
/// space rather than one of its origins. Upstream classifies a
/// specifier's operands by type: an `AddressSpace` value — spelled through
/// the type (`AddressSpace.GENERIC`, `AddressSpace(1)`, `.GENERIC`), or a
/// binder `decls` declares of type `AddressSpace` (`a`, `Self.a`) — is the
/// address space, in any position; every other operand is an origin.
pub(in crate::checker) fn is_address_space_operand(expr: &Expr, decls: &[ParamDecl]) -> bool {
    let names_type = |expression: &Expr| {
        matches!(&expression.kind, ExprKind::Identifier(name)
            if name == mojito_types::types::ADDRESS_SPACE_TYPE_NAME)
    };
    match &expr.kind {
        ExprKind::Call { name, .. } => name == mojito_types::types::ADDRESS_SPACE_TYPE_NAME,
        ExprKind::Member { object, .. } if names_type(object) => true,
        _ if super::super::inference::contextual_root_span(expr).is_some() => true,
        _ => origin_binder_name(expr).is_some_and(|name| {
            decls.iter().any(|decl| {
                matches!(decl, ParamDecl::Value { name: declared, ty, .. }
                    if declared == name && mojito_types::types::is_address_space(ty))
            })
        }),
    }
}

/// The origin operands of the `ref[...]` clause `spec`, its address-space
/// operand ([`is_address_space_operand`]) dropped: `None` for a clause
/// naming no origin, whose origin infers as a bare `ref`'s does
/// (`ref[AddressSpace.GENERIC] x`).
pub(in crate::checker) fn clause_origins<'s>(
    spec: Option<&'s [Expr]>,
    decls: &[ParamDecl],
) -> Option<std::borrow::Cow<'s, [Expr]>> {
    let spec = spec?;
    let origins = if spec
        .iter()
        .any(|operand| is_address_space_operand(operand, decls))
    {
        std::borrow::Cow::Owned(
            spec.iter()
                .filter(|operand| !is_address_space_operand(operand, decls))
                .cloned()
                .collect(),
        )
    } else {
        std::borrow::Cow::Borrowed(spec)
    };
    (!origins.is_empty()).then_some(origins)
}

/// The `ref[...]` clause `spec` split by operand type: its origin operands
/// and the address space it names, `AddressSpace.GENERIC` when it names
/// none. A second address-space operand is the pin's error.
pub(in crate::checker) fn split_ref_clause<'s>(
    spec: &'s [Expr],
    decls: &[ParamDecl],
    values: &dyn OriginDecls,
) -> Result<
    (
        std::borrow::Cow<'s, [Expr]>,
        mojito_types::types::AddressSpaceSlot,
    ),
    TypeError,
> {
    let mut spaces = spec
        .iter()
        .filter(|operand| is_address_space_operand(operand, decls));
    let address_space = match (spaces.next(), spaces.next()) {
        (None, _) => mojito_types::types::AddressSpaceSlot::GENERIC,
        (Some(operand), None) => values.address_space(operand)?,
        (Some(_), Some(_)) => {
            return Err(TypeError::Unsupported(
                "address space must be specified once; remove duplicate address space \
                 specifications"
                    .to_string(),
            ));
        }
    };
    let origins = clause_origins(Some(spec), decls).unwrap_or_default();
    Ok((origins, address_space))
}

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

/// The removed `Origin[mut=...].cast_from[o]` spelling's migration
/// diagnostic, as `StaticConstantOrigin`'s is; `None` for any other
/// expression.
pub(in crate::checker) fn removed_origin_cast_from(expression: &Expr) -> Option<TypeError> {
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
    let (name, _) = applied.bracket_application()?;
    (field == "cast_from" && name == "Origin").then(|| {
        TypeError::Unsupported(
            "'Origin[mut=...].cast_from[o]' was removed; use 'ImmOrigin(o)'".to_string(),
        )
    })
}

/// The operand of a call with one positional argument (`ImmOrigin(o)`,
/// `MyImm(o)`), which may convert an origin; `None` for any other
/// expression, an `origin_of` observation included.
pub(in crate::checker) fn conversion_operand(expression: &Expr) -> Option<&Expr> {
    match &expression.kind {
        ExprKind::Call {
            name, args, kwargs, ..
        } if kwargs.is_empty() && name != "origin_of" => match args.as_slice() {
            [operand] => Some(operand),
            _ => None,
        },
        _ => None,
    }
}

/// Upstream's `o.unsafe_mut_cast[m]()` (`Origin.unsafe_mut_cast`) in an
/// origin clause: the receiver, and the destination mutability argument.
/// The signature lowerings hold no checker, so they recognize the cast by
/// its spelling; the checker's own readers read it through the declaration
/// ([`Checker::origin_method_cast`]).
pub(in crate::checker) fn origin_mut_cast_syntax(
    expression: &Expr,
) -> Option<(&Expr, &mojito_ast::ast::ParamArg)> {
    let ExprKind::Invoke {
        callee,
        param_args,
        args,
        kwargs,
    } = &expression.kind
    else {
        return None;
    };
    let ExprKind::Member { object, field } = &callee.kind else {
        return None;
    };
    let [destination] = param_args.as_slice() else {
        return None;
    };
    let destination = match destination {
        mojito_ast::ast::ParamArg::Named { name, value } if name == "dest_mut" => value.as_ref(),
        mojito_ast::ast::ParamArg::Named { .. } => return None,
        positional => positional,
    };
    (field == "unsafe_mut_cast" && args.is_empty() && kwargs.is_empty())
        .then_some((object.as_ref(), destination))
}

pub(in crate::checker) fn validate_origin_expr(
    expr: &Expr,
    origin_params: &HashSet<&str>,
    value_params: &HashSet<&str>,
    values: &dyn OriginDecls,
) -> Result<(), TypeError> {
    if let Some(error) = removed_origin_cast_from(expr) {
        return Err(error);
    }
    if let Some(conversion) = values.conversion(expr) {
        return validate_origin_expr(conversion?.0, origin_params, value_params, values);
    }
    if let Some((receiver, _)) = origin_mut_cast_syntax(expr) {
        return validate_origin_expr(receiver, origin_params, value_params, values);
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
        _ if values.value(expr).is_some() => Ok(()),
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
    values: &dyn OriginDecls,
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
                    address_space: mojito_types::types::AddressSpaceSlot::GENERIC,
                })),
            }
        })
        .collect()
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
        // A binder the result names stands for the `ref` parameters bound
        // through it, as a call binds it.
        let ref_params =
            lower_ref_param_sigs(decls, params, struct_params, self).unwrap_or_default();
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
            let Ok(sig) = lower_ref_sig(&spec, decls, params, struct_params, self) else {
                continue;
            };
            let Some(mut sig) = bind_ref_binders(&ref_params, Some(&sig), &|_| true).1 else {
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
    values: &dyn OriginDecls,
) -> Result<mojito_types::origin::RefSig, TypeError> {
    use mojito_types::origin::{Origin, RefSig, SigMutability, SigOrigin};
    let (spec, address_space) = split_ref_clause(spec, decls, values)?;
    let mut members = Vec::new();
    let mut mutability = SigMutability::Infer;
    let mut cast = None;
    for expression in spec.iter() {
        // A mutability cast (`ImmOrigin(o)`, `o.unsafe_mut_cast[m]()`)
        // wraps one member; unwrap it and pin the whole signature's
        // capability, the outermost cast's, after the member lowers
        // normally.
        let mut expression = expression;
        let mut outermost = None;
        loop {
            if let Some(error) = removed_origin_cast_from(expression) {
                return Err(error);
            }
            if let Some(conversion) = values.conversion(expression) {
                let (operand, mutable) = conversion?;
                outermost.get_or_insert(if mutable {
                    SigMutability::Mutable
                } else {
                    SigMutability::Immutable
                });
                expression = operand;
            } else if let Some((receiver, destination)) = origin_mut_cast_syntax(expression) {
                if outermost.is_none() {
                    outermost = Some(cast_sig_mutability(destination, decls)?);
                }
                expression = receiver;
            } else {
                break;
            }
        }
        if outermost.is_some() {
            cast = outermost;
        }
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
            _ if let Some((origin, mutable)) = values.value(expression).filter(|_| {
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
                // The callable's own binder names itself, as upstream's
                // contract does; a call binds it from the `ref` arguments
                // whose clauses name it ([`bind_ref_binders`]).
                if (origin_id.0 as usize) >= struct_params {
                    members.push(SigOrigin::Bound(Origin::Param(origin_id)));
                    continue;
                }
                let first_member = members.len();
                for (index, param) in params.iter().enumerate() {
                    if clause_origins(param.origin.as_deref(), decls).is_some_and(|origin| {
                        matches!(&*origin, [expression] if origin_binder_name(expression) == Some(name.as_str()))
                    }) {
                        members.push(SigOrigin::Param(index));
                    }
                }
                // An enclosing struct Origin can be carried by reference-valued
                // fields even when no ordinary method parameter binds it. Keep
                // that checked semantic binder directly in the method contract
                // instead of collapsing it to an empty inferred union.
                if members.len() == first_member {
                    members.push(SigOrigin::Bound(Origin::Param(origin_id)));
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
                    if clause_origins(param.origin.as_deref(), decls).is_some_and(|origin| {
                        matches!(&*origin, [expression] if origin_binder_name(expression) == Some(field.as_str()))
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
    if let Some(cast) = cast {
        mutability = cast;
    }
    Ok(RefSig {
        origin,
        mutability,
        address_space,
    })
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

/// The contract-level origin a named origin binder denotes: a callable's
/// own binder (declared at or past `struct_params`) itself; an enclosing
/// struct's, the value parameter(s) carrying it, else the checked semantic
/// binder itself (one bound only by reference-valued fields).
pub(in crate::checker) fn sig_origin_for_binder(
    name: &str,
    decls: &[ParamDecl],
    params: &[&FnParam],
    struct_params: usize,
) -> Option<mojito_types::origin::SigOrigin> {
    use mojito_types::origin::SigOrigin;
    let (_, origin_id) = origin_decl(decls, name)?;
    if (origin_id.0 as usize) >= struct_params {
        return Some(SigOrigin::Bound(mojito_types::origin::Origin::Param(
            origin_id,
        )));
    }
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
/// mutability casts and interior projections.
fn origin_roots<'a>(expression: &'a Expr, roots: &mut Vec<&'a str>) {
    let mut expression = expression;
    loop {
        if let Some(operand) = conversion_operand(expression) {
            expression = operand;
        } else if let Some((receiver, _)) = origin_mut_cast_syntax(expression) {
            expression = receiver;
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

/// The capability `o.unsafe_mut_cast[destination]()` pins a `ref` clause
/// to: a `Bool` literal's, or the `Bool` binder of `decls` the destination
/// names (`m`, `Self.mut`) as [`origin_decl_mutability`] numbers it.
fn cast_sig_mutability(
    destination: &mojito_ast::ast::ParamArg,
    decls: &[ParamDecl],
) -> Result<mojito_types::origin::SigMutability, TypeError> {
    use mojito_ast::ast::{ParamArg, Type};
    use mojito_types::origin::SigMutability;
    let flag = |mutable: bool| {
        if mutable {
            SigMutability::Mutable
        } else {
            SigMutability::Immutable
        }
    };
    let name = match destination {
        ParamArg::Value(Expr {
            kind: ExprKind::Bool(mutable),
            ..
        }) => return Ok(flag(*mutable)),
        ParamArg::Type(Type::Named(name, args)) if args.is_empty() => match name.as_str() {
            "True" => return Ok(flag(true)),
            "False" => return Ok(flag(false)),
            name => Some(name),
        },
        ParamArg::Value(Expr {
            kind: ExprKind::Identifier(name),
            ..
        }) => Some(name.as_str()),
        ParamArg::Value(Expr {
            kind: ExprKind::Member { object, field },
            ..
        }) if matches!(&object.kind, ExprKind::Identifier(name) if name == "Self") => {
            Some(field.as_str())
        }
        _ => None,
    };
    name.and_then(|name| {
        decls
            .iter()
            .position(|decl| matches!(decl, ParamDecl::Value { .. }) && decl.name() == name)
    })
    .map(|slot| {
        SigMutability::BoolParam(
            decls[..slot]
                .iter()
                .filter(|earlier| !earlier.is_pack_list())
                .count(),
        )
    })
    .ok_or_else(|| {
        TypeError::Unsupported(
            "an 'unsafe_mut_cast' in a reference origin clause whose mutability is neither \
             a Bool literal nor a Bool binder in scope"
                .to_string(),
        )
    })
}
