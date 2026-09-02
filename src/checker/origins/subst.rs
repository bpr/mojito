//! Sig-origin lowering and substitution: projection, instantiation,
//! binding, and call-through translation.

use super::*;

pub(in crate::checker) fn lower_sig_origin_expression(
    expression: &Expr,
    type_params: &[crate::ast::TypeParam],
    params: &[&FnParam],
    struct_params: usize,
) -> Result<crate::origin::SigOrigin, TypeError> {
    use crate::origin::SigOrigin;
    if subtree_origin_syntax(expression).is_some() {
        return Err(reject_subtree_origin_here("a reference origin clause"));
    }
    if let Some((base, name)) = interior_origin_syntax(expression) {
        return Ok(SigOrigin::Projected(
            Box::new(lower_sig_origin_expression(
                base,
                type_params,
                params,
                struct_params,
            )?),
            vec![crate::origin::OriginSeg::Interior(name.to_string())],
        ));
    }
    match &expression.kind {
        ExprKind::Identifier(name) if name == "self" => Ok(SigOrigin::Self_),
        ExprKind::Identifier(name) => {
            if let Some(index) = params.iter().position(|parameter| parameter.name == *name) {
                return Ok(SigOrigin::Param(index));
            }
            if let Some(origin) = sig_origin_for_binder(name, type_params, params) {
                if !name.starts_with("__")
                    && type_params[..struct_params.min(type_params.len())]
                        .iter()
                        .any(|parameter| parameter.name == *name)
                {
                    return Err(unqualified_struct_binder(name));
                }
                return Ok(origin);
            }
            Err(TypeError::UndefinedVariable(name.clone()))
        }
        // Upstream's qualified struct-binder spelling (`Self.o`), including
        // as the base of a projected clause such as
        // `Self.o._get_owned_interior["element"]`.
        ExprKind::Member { object, field }
            if matches!(&object.kind, ExprKind::Identifier(name) if name == "Self")
                && sig_origin_for_binder(field, type_params, params).is_some() =>
        {
            Ok(sig_origin_for_binder(field, type_params, params).expect("guarded above"))
        }
        ExprKind::Call {
            name,
            args,
            kwargs,
            param_args,
        } if name == "origin_of" && kwargs.is_empty() && param_args.is_empty() => {
            let members = args
                .iter()
                .map(|argument| {
                    let (root, path) = place_path(argument).ok_or_else(|| {
                        TypeError::Unsupported("origin_of requires parameter places".to_string())
                    })?;
                    let base = if root == "self" {
                        SigOrigin::Self_
                    } else {
                        let index = params
                            .iter()
                            .position(|parameter| parameter.name == root)
                            .ok_or_else(|| TypeError::UndefinedVariable(root.to_string()))?;
                        SigOrigin::Param(index)
                    };
                    Ok(project_sig_origin(base, &path))
                })
                .collect::<Result<Vec<_>, TypeError>>()?;
            Ok(match members.as_slice() {
                [single] => single.clone(),
                _ => SigOrigin::union(members),
            })
        }
        ExprKind::Member { .. } | ExprKind::Index { .. } => {
            let (root, path) = place_path(expression)
                .ok_or_else(|| TypeError::Unsupported("invalid origin place".to_string()))?;
            let base = if root == "self" {
                SigOrigin::Self_
            } else {
                let index = params
                    .iter()
                    .position(|parameter| parameter.name == root)
                    .ok_or_else(|| TypeError::UndefinedVariable(root.to_string()))?;
                SigOrigin::Param(index)
            };
            Ok(project_sig_origin(base, &path))
        }
        _ => Err(TypeError::Unsupported(
            "unsupported origin contract".to_string(),
        )),
    }
}

pub(in crate::checker) fn project_sig_origin(
    base: crate::origin::SigOrigin,
    path: &[PlaceSeg],
) -> crate::origin::SigOrigin {
    crate::origin::SigOrigin::Projected(
        Box::new(base),
        path.iter()
            .map(|segment| match segment {
                PlaceSeg::Field(name) => crate::origin::OriginSeg::Field(name.clone()),
                PlaceSeg::Index => crate::origin::OriginSeg::AnyIndex,
            })
            .collect(),
    )
}

pub(in crate::checker) fn project_origin(
    origin: crate::origin::Origin,
    path: &[crate::origin::OriginSeg],
) -> crate::origin::Origin {
    use crate::origin::Origin;
    match origin {
        Origin::Place(mut place) => {
            place.path.extend_from_slice(path);
            Origin::Place(place)
        }
        Origin::Union(members) => Origin::union(
            members
                .into_iter()
                .map(|member| project_origin(member, path)),
        ),
        other => other,
    }
}

/// Instantiate a callee's declared signature origin against a receiver's
/// concrete struct type arguments (`TyArg::Origin` entries are retained in
/// struct args): `Self_` maps to the receiver itself, `Bound(Param(i))` to
/// the receiver's i-th origin argument (with a single-origin fallback), and
/// projections/unions recurse. Shared by the iteration protocol and the
/// delegated-call expression-origin resolution.
pub(in crate::checker) fn instantiate_sig_origin(
    signature: &crate::origin::SigOrigin,
    arguments: &[TyArg],
) -> crate::origin::Origin {
    use crate::origin::{Origin, OriginParamId, SigOrigin};

    match signature {
        SigOrigin::Self_ | SigOrigin::Infer => Origin::SelfParam,
        SigOrigin::Param(index) => Origin::Param(OriginParamId(*index as u32)),
        SigOrigin::Bound(origin) => instantiate_bound_origin(origin, arguments),
        SigOrigin::Static => Origin::Static,
        SigOrigin::Untracked { mutable } => Origin::Untracked { mutable: *mutable },
        SigOrigin::Projected(base, path) => {
            project_origin(instantiate_sig_origin(base, arguments), path)
        }
        SigOrigin::Union(members) => Origin::union(
            members
                .iter()
                .map(|member| instantiate_sig_origin(member, arguments)),
        ),
    }
}

/// See [`instantiate_sig_origin`]: resolve a bound origin's parameters against
/// the receiver's origin arguments, leaving unresolvable parameters symbolic.
pub(in crate::checker) fn instantiate_bound_origin(
    origin: &crate::origin::Origin,
    arguments: &[TyArg],
) -> crate::origin::Origin {
    use crate::origin::Origin;

    match origin {
        Origin::Param(parameter) => struct_origin_argument(arguments, parameter.0 as usize)
            .unwrap_or(Origin::Param(*parameter)),
        Origin::Union(members) => Origin::union(
            members
                .iter()
                .map(|member| instantiate_bound_origin(member, arguments)),
        ),
        _ => origin.clone(),
    }
}

/// The origin argument at `index` in a struct's type arguments, falling back
/// to the unique origin argument when the index does not line up (origin
/// params are erased from the explicit decl list, so indices can shift).
pub(in crate::checker) fn struct_origin_argument(
    arguments: &[TyArg],
    index: usize,
) -> Option<crate::origin::Origin> {
    if let Some(TyArg::Origin(origin)) = arguments.get(index) {
        return Some(origin.clone());
    }
    let mut origins = arguments.iter().filter_map(|argument| match argument {
        TyArg::Origin(origin) => Some(origin),
        TyArg::Ty(_) | TyArg::Val(_) => None,
    });
    let only = origins.next()?.clone();
    origins.next().is_none().then_some(only)
}

/// Map a delegated callee's declared ref-return origin into the delegating
/// caller's signature terms: the callee's `Self` becomes the caller's
/// receiver path, and each callee struct origin parameter must resolve to the
/// receiver type's corresponding origin argument (already spelled in caller
/// terms). Unresolvable forms are contextual errors, not silent fallbacks —
/// a wrong origin identity would let a returned reference escape checking.
/// `receiver_fallback`: for a parameter-rooted receiver, the receiver origin a
/// callee struct binder resolves to when nothing names it (see
/// `delegated_call_ref_sig`).
pub(in crate::checker) fn map_delegated_sig_origin(
    callee: &crate::origin::SigOrigin,
    receiver_args: &[TyArg],
    receiver: &crate::origin::SigOrigin,
    correspondences: &[(u32, u32)],
    caller_binder: Option<&crate::origin::Origin>,
    receiver_fallback: Option<&crate::origin::SigOrigin>,
) -> Result<crate::origin::SigOrigin, TypeError> {
    use crate::origin::{Origin, SigOrigin};

    pub(super) fn map_bound(
        origin: &Origin,
        receiver_args: &[TyArg],
        correspondences: &[(u32, u32)],
        caller_binder: Option<&Origin>,
    ) -> Result<Origin, TypeError> {
        match origin {
            Origin::Param(parameter) => struct_origin_argument(receiver_args, parameter.0 as usize)
                // The field application's recorded binder correspondence
                // (`var iter: EntryIter[Self.o2]` maps EntryIter's binder to
                // `o2`) — origin arguments erase from checked identity, so
                // this record resolves what the type arguments cannot.
                .or_else(|| {
                    correspondences
                        .iter()
                        .find(|(callee_index, _)| *callee_index == parameter.0)
                        .map(|(_, enclosing)| {
                            Origin::Param(crate::origin::OriginParamId(*enclosing))
                        })
                })
                .or_else(|| caller_binder.cloned())
                .ok_or_else(|| {
                    TypeError::Unsupported(
                        "the delegated callee's origin parameter is not bound by the \
                             receiver's type arguments"
                            .to_string(),
                    )
                }),
            Origin::Union(members) => Ok(Origin::union(
                members
                    .iter()
                    .map(|member| map_bound(member, receiver_args, correspondences, caller_binder))
                    .collect::<Result<Vec<_>, _>>()?,
            )),
            other => Ok(other.clone()),
        }
    }

    match callee {
        SigOrigin::Self_ => Ok(receiver.clone()),
        SigOrigin::Bound(origin) => {
            match map_bound(origin, receiver_args, correspondences, caller_binder) {
                Ok(bound) => Ok(SigOrigin::Bound(bound)),
                Err(error) => receiver_fallback.cloned().ok_or(error),
            }
        }
        SigOrigin::Static => Ok(SigOrigin::Static),
        SigOrigin::Untracked { mutable } => Ok(SigOrigin::Untracked { mutable: *mutable }),
        SigOrigin::Projected(base, path) => Ok(SigOrigin::Projected(
            Box::new(map_delegated_sig_origin(
                base,
                receiver_args,
                receiver,
                correspondences,
                caller_binder,
                receiver_fallback,
            )?),
            path.clone(),
        )),
        SigOrigin::Union(members) => Ok(SigOrigin::Union(
            members
                .iter()
                .map(|member| {
                    map_delegated_sig_origin(
                        member,
                        receiver_args,
                        receiver,
                        correspondences,
                        caller_binder,
                        receiver_fallback,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?,
        )),
        SigOrigin::Param(_) | SigOrigin::Infer => Err(TypeError::Unsupported(
            "the delegated callee's ref-return origin must be declared in terms of its \
             receiver or struct origin parameters"
                .to_string(),
        )),
    }
}

/// Replace the slot-relative parts belonging to source `Origin` parameters
/// with the concrete caller origins captured by a specialized function value.
pub(in crate::checker) fn bind_sig_origin(
    signature: &crate::origin::SigOrigin,
    bindings: &[(Vec<usize>, crate::origin::Origin)],
) -> crate::origin::SigOrigin {
    use crate::origin::SigOrigin;
    match signature {
        SigOrigin::Param(index) => bindings
            .iter()
            .find(|(slots, _)| slots.contains(index))
            .map(|(_, origin)| SigOrigin::Bound(origin.clone()))
            .unwrap_or_else(|| signature.clone()),
        SigOrigin::Projected(base, path) => {
            SigOrigin::Projected(Box::new(bind_sig_origin(base, bindings)), path.clone())
        }
        SigOrigin::Union(members) => SigOrigin::union(
            members
                .iter()
                .map(|member| bind_sig_origin(member, bindings)),
        ),
        _ => signature.clone(),
    }
}

pub(in crate::checker) fn sig_origin_has_bound(signature: &crate::origin::SigOrigin) -> bool {
    use crate::origin::SigOrigin;
    match signature {
        SigOrigin::Bound(_) => true,
        SigOrigin::Projected(base, _) => sig_origin_has_bound(base),
        SigOrigin::Union(members) => members.iter().any(sig_origin_has_bound),
        _ => false,
    }
}

/// Whether a struct field's declared type borrows origin parameter `id` — a
/// `ref[o]` field or an origin-bearing `UnsafePointer[..., o]` field.
/// Whether any place constituent of `origin` roots at `owner`. Used to detect
/// an origin-parameter resolution that fell back to the receiver's own storage
/// (no construction-time binding was recorded) rather than naming the borrowed
/// source — such a resolution is still symbolic for write-legality judgments.
pub(in crate::checker) fn origin_rooted_at(
    origin: &crate::origin::Origin,
    owner: crate::origin::OwnerId,
) -> bool {
    use crate::origin::Origin;
    match origin {
        Origin::Place(place) => place.root == owner,
        Origin::Union(members) => members.iter().any(|member| origin_rooted_at(member, owner)),
        _ => false,
    }
}

pub(in crate::checker) fn field_carries_origin_param(
    ty: &Ty,
    id: crate::origin::OriginParamId,
) -> bool {
    use crate::origin::{Origin, PointerOrigin};
    match ty {
        Ty::Ref(reference) => matches!(reference.origin, Origin::Param(k) if k == id),
        Ty::Pointer { origin, .. } => {
            matches!(origin, PointerOrigin::Param { id: k, .. } if *k == id)
        }
        _ => false,
    }
}

/// Replace each `Origin::Param(id)` for which `concrete` yields a binding with that
/// concrete origin, recursing through unions and leaving every other origin as-is.
pub(in crate::checker) fn substitute_origin_params(
    origin: crate::origin::Origin,
    concrete: &impl Fn(crate::origin::OriginParamId) -> Option<crate::origin::Origin>,
) -> crate::origin::Origin {
    use crate::origin::Origin;
    match origin {
        Origin::Param(id) => concrete(id).unwrap_or(Origin::Param(id)),
        Origin::Union(members) => Origin::union(
            members
                .into_iter()
                .map(|member| substitute_origin_params(member, concrete)),
        ),
        other => other,
    }
}

pub(in crate::checker) fn substitute_sig_origin(
    signature: &crate::origin::SigOrigin,
    actual: &[Option<crate::origin::Origin>],
) -> crate::origin::Origin {
    use crate::origin::{Origin, SigOrigin};
    match signature {
        SigOrigin::Self_ => Origin::Union(vec![]),
        SigOrigin::Bound(origin) => origin.clone(),
        SigOrigin::Param(index) => actual
            .get(*index)
            .and_then(Clone::clone)
            .unwrap_or(Origin::Union(vec![])),
        SigOrigin::Static => Origin::Static,
        SigOrigin::Untracked { mutable } => Origin::Untracked { mutable: *mutable },
        SigOrigin::Projected(base, path) => {
            project_origin(substitute_sig_origin(base, actual), path)
        }
        SigOrigin::Union(members) => Origin::union(
            members
                .iter()
                .map(|member| substitute_sig_origin(member, actual)),
        ),
        SigOrigin::Infer => Origin::union(actual.iter().filter_map(Clone::clone)),
    }
}

pub(in crate::checker) fn substitute_sig_origin_with_self(
    signature: &crate::origin::SigOrigin,
    actual: &[Option<crate::origin::Origin>],
    self_origin: Option<crate::origin::Origin>,
) -> crate::origin::Origin {
    use crate::origin::{Origin, SigOrigin};
    match signature {
        SigOrigin::Self_ => self_origin.clone().unwrap_or_else(|| Origin::Union(vec![])),
        SigOrigin::Union(members) => Origin::union(
            members
                .iter()
                .map(|member| substitute_sig_origin_with_self(member, actual, self_origin.clone())),
        ),
        SigOrigin::Projected(base, path) => project_origin(
            substitute_sig_origin_with_self(base, actual, self_origin),
            path,
        ),
        _ => substitute_sig_origin(signature, actual),
    }
}

pub(in crate::checker) fn origin_is_within(
    actual: &crate::origin::Origin,
    allowed: &crate::origin::Origin,
) -> bool {
    use crate::origin::Origin;
    match actual {
        Origin::Union(members) => members
            .iter()
            .all(|member| origin_is_within(member, allowed)),
        _ => match allowed {
            Origin::Union(members) => members
                .iter()
                .any(|member| origin_is_within(actual, member)),
            _ => actual.overlaps(allowed),
        },
    }
}

pub(in crate::checker) fn ref_parameter_is_writable(
    parameter: &FnParam,
    type_params: &[crate::ast::TypeParam],
) -> bool {
    ref_binding_is_writable(
        parameter.convention,
        parameter.origin.as_deref(),
        type_params,
    )
}

/// Whether a parameter/receiver may be mutated while its generic body is
/// checked. A bare `ref` has parametric mutability: it propagates the caller's
/// capability to returned references, but its body cannot assume that the
/// caller supplied mutable storage. Only an explicitly mutable origin grants
/// unconditional write access.
pub(in crate::checker) fn ref_binding_is_writable(
    convention: Option<ArgConvention>,
    origin: Option<&[Expr]>,
    type_params: &[crate::ast::TypeParam],
) -> bool {
    if convention != Some(ArgConvention::Ref) {
        return parameter_is_writable(convention);
    }
    let Some(origin_name) = origin.and_then(|expressions| match expressions {
        [expression] => origin_binder_name(expression),
        _ => None,
    }) else {
        return false;
    };
    if origin_name == "MutUnsafeAnyOrigin" {
        return true;
    }
    let Some(origin) = type_params.iter().find(|candidate| {
        candidate.name == *origin_name && candidate.bounds.as_slice() == ["Origin"]
    }) else {
        return false;
    };
    matches!(
        origin.origin_mutability.as_ref().map(|expr| &expr.kind),
        Some(ExprKind::Bool(true))
    )
}

/// Translate a concrete callable's effects through a recorded call-through
/// mapping into effects of the OUTER callee. A frame-local source flowing
/// into a signature destination rejects: the body-local loan would dangle
/// once the higher-order body returns. `Self_` terms of a callable-struct's
/// `__call__` map to the callable slot itself (its runtime-parameter
/// storage); for a compile-time callable value there is no such storage, so
/// those terms stay a documented permissive residue.
pub(in crate::checker) fn translate_call_through(
    effects: &[crate::checked::TransferEffect],
    through: &crate::checked::CallThroughEffect,
) -> Result<Vec<crate::checked::TransferEffect>, TypeError> {
    use crate::checked::{CallThroughCallee, TransferEffect};
    use crate::origin::SigOrigin;
    let callee_slot = match &through.callee {
        CallThroughCallee::RuntimeParam(index) => Some(*index),
        CallThroughCallee::ValueParam(_) => None,
    };
    let mut translated: Vec<TransferEffect> = Vec::new();
    let mut push = |effect: TransferEffect| {
        if effect.dest != effect.src && !translated.contains(&effect) {
            translated.push(effect);
        }
    };
    for effect in effects {
        let translate_base = |base: &SigOrigin| match base {
            SigOrigin::Self_ => callee_slot.map(SigOrigin::Param),
            SigOrigin::Param(index) => through
                .args
                .get(*index)
                .and_then(|argument| argument.place.clone()),
            // A concrete captured owner survives frame boundaries verbatim.
            bound @ SigOrigin::Bound(_) => Some(bound.clone()),
            _ => None,
        };
        let dest = match &effect.dest {
            SigOrigin::Projected(base, path) => translate_base(base)
                .map(|translated| SigOrigin::Projected(Box::new(translated), path.clone())),
            dest => translate_base(dest),
        };
        // A frame-local destination dies with the higher-order body; there
        // is nothing for the caller to install (documented residue).
        let Some(dest) = dest else {
            continue;
        };
        match &effect.src {
            SigOrigin::Param(index) => {
                let Some(argument) = through.args.get(*index) else {
                    continue;
                };
                if argument.local {
                    return Err(TypeError::StoredReferenceEscapesOrigin);
                }
                if effect.src_is_place
                    && let Some(place) = &argument.place
                {
                    push(TransferEffect {
                        dest: dest.clone(),
                        src: place.clone(),
                        src_is_place: true,
                        mutable: effect.mutable,
                    });
                }
                for sig in &argument.carried {
                    push(TransferEffect {
                        dest: dest.clone(),
                        src: sig.clone(),
                        src_is_place: false,
                        mutable: effect.mutable,
                    });
                }
            }
            SigOrigin::Self_ => {
                if let Some(slot) = callee_slot {
                    push(TransferEffect {
                        dest,
                        src: SigOrigin::Param(slot),
                        src_is_place: effect.src_is_place,
                        mutable: effect.mutable,
                    });
                }
            }
            // A concrete captured owner survives frame boundaries verbatim.
            bound @ SigOrigin::Bound(_) => {
                push(TransferEffect {
                    dest,
                    src: bound.clone(),
                    src_is_place: effect.src_is_place,
                    mutable: effect.mutable,
                });
            }
            _ => {}
        }
    }
    Ok(translated)
}

/// The name supplied for a callable value parameter at a call site: matched
/// positionally over the callee's declarations, or by keyword. `None` for a
/// defaulted, unnamed, or unmatched argument (permissive).
pub(in crate::checker) fn callable_value_argument(
    decls: &[crate::types::ParamDecl],
    decl_name: &str,
    param_args: &[crate::ast::ParamArg],
) -> Option<String> {
    use crate::ast::ParamArg;
    pub(super) fn argument_name(argument: &ParamArg) -> Option<String> {
        match argument {
            ParamArg::Type(crate::ast::Type::Named(name, arguments)) if arguments.is_empty() => {
                Some(name.clone())
            }
            ParamArg::Value(Expr {
                kind: ExprKind::Identifier(name),
                ..
            }) => Some(name.clone()),
            _ => None,
        }
    }
    let mut positional = param_args
        .iter()
        .filter(|argument| !matches!(argument, ParamArg::Named { .. }));
    for decl in decls.iter() {
        let supplied = param_args.iter().find_map(|argument| match argument {
            ParamArg::Named { name, value } if name == decl.name() => Some(value.as_ref()),
            _ => None,
        });
        let argument = match supplied {
            Some(argument) => Some(argument),
            None => positional.next(),
        };
        if decl.name() == decl_name {
            return argument.and_then(argument_name);
        }
    }
    None
}
