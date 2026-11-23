//! Sig-origin lowering and substitution: projection, instantiation,
//! binding, and call-through translation.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::TyKind;

/// The mutability a parametric (`Origin[mut=mut]`) origin binder takes when
/// bound to an origin ([`OriginBindings::inferred`]); `None` leaves its
/// `mut` to the bound origin alone.
pub(in crate::checker) type BinderMutability<'a> =
    &'a dyn Fn(
        mojito_types::origin::OriginParamId,
        &mojito_types::origin::Origin,
    ) -> Option<mojito_types::origin::Mutability>;

/// What one origin substitution ([`substitute_struct_origins`]) binds:
/// each origin binder's origin, and the `mut` its implicit value binder
/// (upstream's `m` in `o: Origin[mut=m]`) takes with it — a `Bool`,
/// another binder's `mut`, or a symbolic cast's expression — as upstream
/// infers `m` with the origin and substitutes both at once.
#[derive(Clone, Debug, Default)]
pub(in crate::checker) struct OriginBindings {
    pub(in crate::checker) origins:
        HashMap<mojito_types::origin::OriginParamId, mojito_types::origin::Origin>,
    pub(in crate::checker) mutabilities: HashMap<mojito_types::origin::OriginParamId, CtValue>,
}

impl OriginBindings {
    /// `origins` with no binder's `mut` decided: an application whose
    /// `mut` reads a bound binder keeps that binder.
    pub(in crate::checker) fn of_origins(
        origins: HashMap<mojito_types::origin::OriginParamId, mojito_types::origin::Origin>,
    ) -> Self {
        Self {
            origins,
            mutabilities: HashMap::new(),
        }
    }

    /// `origins`, each binder's `mut` inferred from the origin it binds
    /// ([`inferred_mut_argument`]).
    pub(in crate::checker) fn inferred(
        origins: HashMap<mojito_types::origin::OriginParamId, mojito_types::origin::Origin>,
        binder_mutability: BinderMutability<'_>,
    ) -> Self {
        let mut bindings = Self::of_origins(origins);
        bindings.infer_mutabilities(binder_mutability);
        bindings
    }

    /// Decide the `mut` of every bound binder that has none yet
    /// ([`inferred_mut_argument`]).
    pub(in crate::checker) fn infer_mutabilities(
        &mut self,
        binder_mutability: BinderMutability<'_>,
    ) {
        for (id, origin) in &self.origins {
            if !self.mutabilities.contains_key(id)
                && let Some(value) = inferred_mut_argument(*id, origin, binder_mutability)
            {
                self.mutabilities.insert(*id, value);
            }
        }
    }

    /// Whether nothing is bound.
    pub(in crate::checker) fn is_empty(&self) -> bool {
        self.origins.is_empty()
    }

    /// The `mut` binder `id` is bound with, as a reference's mutability:
    /// `None` when undecided or a symbolic cast's expression.
    pub(in crate::checker) fn mutability(
        &self,
        id: mojito_types::origin::OriginParamId,
    ) -> Option<mojito_types::origin::Mutability> {
        use mojito_types::origin::Mutability;
        match self.mutabilities.get(&id)? {
            CtValue::Bool(mutable) => Some(Mutability::of(*mutable)),
            CtValue::Deferred(binder) => Mutability::of_mut_argument(binder),
            _ => None,
        }
    }

    /// `origin` with each bound binder but those `held` substituted.
    fn bind_origin(
        &self,
        origin: &mojito_types::origin::Origin,
        held: &[mojito_types::origin::OriginParamId],
    ) -> mojito_types::origin::Origin {
        substitute_origin_params(origin.clone(), &|id| {
            (!held.contains(&id))
                .then(|| self.origins.get(&id).cloned())
                .flatten()
        })
    }

    /// A `mut` argument reading a binder's `mut` this binds, substituted.
    fn bind_mut_argument(&self, value: &CtValue) -> CtValue {
        match value {
            CtValue::Deferred(binder)
                if let Some(mojito_types::origin::Mutability::Param(id)) =
                    mojito_types::origin::Mutability::of_mut_argument(binder) =>
            {
                self.mutabilities
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(|| value.clone())
            }
            other => other.clone(),
        }
    }

    /// The binders whose `mut` the arguments read and this leaves
    /// undecided: their origins stay unbound in that application, which
    /// would otherwise pair a bound origin with the callee's own `mut`.
    fn undecided_mut_binders(
        &self,
        arguments: &[TyArg],
    ) -> Vec<mojito_types::origin::OriginParamId> {
        arguments
            .iter()
            .filter_map(|argument| match argument {
                TyArg::Val(CtValue::Deferred(binder)) => {
                    match mojito_types::origin::Mutability::of_mut_argument(binder)? {
                        mojito_types::origin::Mutability::Param(id)
                            if !self.mutabilities.contains_key(&id) =>
                        {
                            Some(id)
                        }
                        _ => None,
                    }
                }
                _ => None,
            })
            .collect()
    }

    /// A reference's origin and mutability with what this binds
    /// substituted, unchanged when no pointer holds the result.
    fn bind_reference(
        &self,
        origin: &mojito_types::origin::Origin,
        mutability: mojito_types::origin::Mutability,
    ) -> (
        mojito_types::origin::Origin,
        mojito_types::origin::Mutability,
    ) {
        use mojito_types::origin::{Mutability, PointerOrigin};
        let (held, bound_mutability) = match mutability {
            Mutability::Param(id) => match self.mutability(id) {
                Some(bound) => (Vec::new(), bound),
                None => (vec![id], mutability),
            },
            fixed => (Vec::new(), fixed),
        };
        let bound = self.bind_origin(origin, &held);
        if PointerOrigin::from_parts(bound.clone(), bound_mutability).is_some() {
            (bound, bound_mutability)
        } else {
            (origin.clone(), mutability)
        }
    }
}

pub(in crate::checker) fn lower_sig_origin_expression(
    expression: &Expr,
    decls: &[ParamDecl],
    params: &[&FnParam],
    struct_params: usize,
) -> Result<mojito_types::origin::SigOrigin, TypeError> {
    use mojito_types::origin::SigOrigin;
    if subtree_origin_syntax(expression).is_some() {
        return Err(reject_subtree_origin_here("a reference origin clause"));
    }
    // A mutability cast names its receiver's storage; the capability it
    // pins is the reference contract's ([`lower_ref_sig`]).
    if let Some((receiver, _)) = origin_mut_cast_syntax(expression) {
        return lower_sig_origin_expression(receiver, decls, params, struct_params);
    }
    if let Some((base, name)) = interior_origin_syntax(expression) {
        return Ok(SigOrigin::Projected(
            Box::new(lower_sig_origin_expression(
                base,
                decls,
                params,
                struct_params,
            )?),
            vec![mojito_types::origin::OriginSeg::Interior(name.to_string())],
        ));
    }
    match &expression.kind {
        ExprKind::Identifier(name) if name == "self" => Ok(SigOrigin::Self_),
        ExprKind::Identifier(name) => {
            if let Some(index) = params.iter().position(|parameter| parameter.name == *name) {
                return Ok(SigOrigin::Param(index));
            }
            if let Some(origin) = sig_origin_for_binder(name, decls, params, struct_params) {
                if !name.starts_with("__")
                    && origin_decl(decls, name)
                        .is_some_and(|(_, id)| (id.0 as usize) < struct_params)
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
                && let Some(origin) =
                    sig_origin_for_binder(field, decls, params, struct_params) =>
        {
            Ok(origin)
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
    base: mojito_types::origin::SigOrigin,
    path: &[PlaceSeg],
) -> mojito_types::origin::SigOrigin {
    mojito_types::origin::SigOrigin::Projected(
        Box::new(base),
        path.iter()
            .map(|segment| match segment {
                PlaceSeg::Field(name) => mojito_types::origin::OriginSeg::Field(name.clone()),
                PlaceSeg::Index => mojito_types::origin::OriginSeg::AnyIndex,
            })
            .collect(),
    )
}

pub(in crate::checker) fn project_origin(
    origin: mojito_types::origin::Origin,
    path: &[mojito_types::origin::OriginSeg],
) -> mojito_types::origin::Origin {
    use mojito_types::origin::Origin;
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
/// origin tail, keyed by binder): `Self_` maps to the receiver itself,
/// `Bound(Param(i))` to the origin the receiver's tail binds for binder `i`,
/// and projections/unions recurse. Shared by the iteration protocol and the
/// delegated-call expression-origin resolution.
pub(in crate::checker) fn instantiate_sig_origin(
    signature: &mojito_types::origin::SigOrigin,
    tail: &HashMap<mojito_types::origin::OriginParamId, mojito_types::origin::Origin>,
) -> mojito_types::origin::Origin {
    use mojito_types::origin::{Origin, OriginParamId, SigOrigin};

    match signature {
        SigOrigin::Self_ | SigOrigin::Infer => Origin::SelfParam,
        SigOrigin::Carried(_) => Origin::Union(vec![]),
        SigOrigin::Param(index) => Origin::Param(OriginParamId(*index as u32)),
        SigOrigin::Bound(origin) => instantiate_bound_origin(origin, tail),
        SigOrigin::Static => Origin::Static,
        SigOrigin::Untracked { mutable } | SigOrigin::UnsafeAny { mutable } => {
            Origin::Untracked { mutable: *mutable }
        }
        SigOrigin::Projected(base, path) => {
            project_origin(instantiate_sig_origin(base, tail), path)
        }
        SigOrigin::Union(members) => Origin::union(
            members
                .iter()
                .map(|member| instantiate_sig_origin(member, tail)),
        ),
    }
}

/// See [`instantiate_sig_origin`]: resolve a bound origin's parameters against
/// the receiver's origin tail, leaving unresolvable parameters symbolic.
pub(in crate::checker) fn instantiate_bound_origin(
    origin: &mojito_types::origin::Origin,
    tail: &HashMap<mojito_types::origin::OriginParamId, mojito_types::origin::Origin>,
) -> mojito_types::origin::Origin {
    use mojito_types::origin::Origin;

    match origin {
        Origin::Param(parameter) => match tail.get(parameter) {
            Some(bound) if !matches!(bound, Origin::Unbound) => bound.clone(),
            _ => Origin::Param(*parameter),
        },
        Origin::Union(members) => Origin::union(
            members
                .iter()
                .map(|member| instantiate_bound_origin(member, tail)),
        ),
        Origin::Projected(base, path) => instantiate_bound_origin(base, tail).reprojected(path),
        _ => origin.clone(),
    }
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
    callee: &mojito_types::origin::SigOrigin,
    receiver_tail: &HashMap<mojito_types::origin::OriginParamId, mojito_types::origin::Origin>,
    receiver: &mojito_types::origin::SigOrigin,
    correspondences: &[(u32, u32)],
    caller_binder: Option<&mojito_types::origin::Origin>,
    receiver_fallback: Option<&mojito_types::origin::SigOrigin>,
) -> Result<mojito_types::origin::SigOrigin, TypeError> {
    use mojito_types::origin::{Origin, SigOrigin};

    pub(super) fn map_bound(
        origin: &Origin,
        receiver_tail: &HashMap<mojito_types::origin::OriginParamId, Origin>,
        correspondences: &[(u32, u32)],
        caller_binder: Option<&Origin>,
    ) -> Result<Origin, TypeError> {
        match origin {
            Origin::Param(parameter) => receiver_tail
                .get(parameter)
                .filter(|bound| !matches!(bound, Origin::Unbound))
                .cloned()
                // The field application's recorded binder correspondence
                // (`var iter: EntryIter[Self.o2]` maps EntryIter's binder to
                // `o2`) resolves a tail the receiver type left symbolic.
                .or_else(|| {
                    correspondences
                        .iter()
                        .find(|(callee_index, _)| *callee_index == parameter.0)
                        .map(|(_, enclosing)| {
                            Origin::Param(mojito_types::origin::OriginParamId(*enclosing))
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
                    .map(|member| map_bound(member, receiver_tail, correspondences, caller_binder))
                    .collect::<Result<Vec<_>, _>>()?,
            )),
            Origin::Projected(base, path) => {
                map_bound(base, receiver_tail, correspondences, caller_binder)
                    .map(|base| base.reprojected(path))
            }
            other => Ok(other.clone()),
        }
    }

    match callee {
        SigOrigin::Self_ => Ok(receiver.clone()),
        SigOrigin::Bound(origin) => {
            match map_bound(origin, receiver_tail, correspondences, caller_binder) {
                Ok(bound) => Ok(SigOrigin::Bound(bound)),
                Err(error) => receiver_fallback.cloned().ok_or(error),
            }
        }
        SigOrigin::Static => Ok(SigOrigin::Static),
        SigOrigin::Untracked { mutable } => Ok(SigOrigin::Untracked { mutable: *mutable }),
        SigOrigin::UnsafeAny { mutable } => Ok(SigOrigin::UnsafeAny { mutable: *mutable }),
        SigOrigin::Projected(base, path) => Ok(SigOrigin::Projected(
            Box::new(map_delegated_sig_origin(
                base,
                receiver_tail,
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
                        receiver_tail,
                        receiver,
                        correspondences,
                        caller_binder,
                        receiver_fallback,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?,
        )),
        SigOrigin::Param(_) | SigOrigin::Infer | SigOrigin::Carried(_) => {
            Err(TypeError::Unsupported(
                "the delegated callee's ref-return origin must be declared in terms of its \
             receiver or struct origin parameters"
                    .to_string(),
            ))
        }
    }
}

/// A call's `ref` contracts, `signatures` per argument slot and `result`,
/// with each origin binder `open` admits bound from the `ref` arguments
/// whose own clause names it, as upstream's call infers a binder from the
/// arguments passed to it: the contract over the binder names those slots'
/// union (`SigOrigin::Param`), which the call's solve resolves to their
/// origins. A binder no parameter's clause names keeps its own name, for
/// pointer and struct-tail solving to bind.
pub(in crate::checker) fn bind_ref_binders(
    signatures: &[Option<mojito_types::origin::RefSig>],
    result: Option<&mojito_types::origin::RefSig>,
    open: &dyn Fn(mojito_types::origin::OriginParamId) -> bool,
) -> (
    Vec<Option<mojito_types::origin::RefSig>>,
    Option<mojito_types::origin::RefSig>,
) {
    bind_ref_binders_with_receiver(None, signatures, result, open)
}

/// [`bind_ref_binders`] for a method, whose receiver is argument 0: a
/// binder the receiver's clause `receiver` names (`ref[o] self`) also
/// stands for the receiver (`SigOrigin::Self_`), joined with the slots
/// naming it.
pub(in crate::checker) fn bind_ref_binders_with_receiver(
    receiver: Option<&mojito_types::origin::RefSig>,
    signatures: &[Option<mojito_types::origin::RefSig>],
    result: Option<&mojito_types::origin::RefSig>,
    open: &dyn Fn(mojito_types::origin::OriginParamId) -> bool,
) -> (
    Vec<Option<mojito_types::origin::RefSig>>,
    Option<mojito_types::origin::RefSig>,
) {
    use mojito_types::origin::{Origin, OriginParamId, RefSig, SigOrigin};
    fn bind(
        origin: &SigOrigin,
        slots: &HashMap<OriginParamId, Vec<usize>>,
        receiver: Option<OriginParamId>,
    ) -> SigOrigin {
        match origin {
            SigOrigin::Bound(Origin::Param(id)) => {
                let named: Vec<SigOrigin> = (receiver == Some(*id))
                    .then_some(SigOrigin::Self_)
                    .into_iter()
                    .chain(
                        slots
                            .get(id)
                            .into_iter()
                            .flatten()
                            .copied()
                            .map(SigOrigin::Param),
                    )
                    .collect();
                match named.len() {
                    0 => origin.clone(),
                    1 => named.into_iter().next().expect("one binding"),
                    _ => SigOrigin::union(named),
                }
            }
            SigOrigin::Projected(base, path) => {
                SigOrigin::Projected(Box::new(bind(base, slots, receiver)), path.clone())
            }
            SigOrigin::Union(members) => {
                SigOrigin::union(members.iter().map(|member| bind(member, slots, receiver)))
            }
            other => other.clone(),
        }
    }
    let receiver = match receiver {
        Some(RefSig {
            origin: SigOrigin::Bound(Origin::Param(id)),
            ..
        }) if open(*id) => Some(*id),
        _ => None,
    };
    let mut slots = HashMap::<OriginParamId, Vec<usize>>::new();
    for (index, signature) in signatures.iter().enumerate() {
        if let Some(RefSig {
            origin: SigOrigin::Bound(Origin::Param(id)),
            ..
        }) = signature
            && open(*id)
        {
            slots.entry(*id).or_default().push(index);
        }
    }
    let rebind = |signature: &RefSig| RefSig {
        origin: bind(&signature.origin, &slots, receiver),
        mutability: signature.mutability.clone(),
        address_space: signature.address_space.clone(),
    };
    (
        signatures
            .iter()
            .map(|signature| signature.as_ref().map(rebind))
            .collect(),
        result.map(rebind),
    )
}

/// `signature` with each contract over an origin binder `tails` binds
/// naming the bound origin instead (`g[origin_of(xs)]` over
/// `-> ref [o] Int`).
pub(in crate::checker) fn bind_sig_origin(
    signature: &mojito_types::origin::SigOrigin,
    tails: &HashMap<mojito_types::origin::OriginParamId, mojito_types::origin::Origin>,
) -> mojito_types::origin::SigOrigin {
    use mojito_types::origin::{Origin, SigOrigin};
    match signature {
        SigOrigin::Bound(Origin::Param(id)) => tails.get(id).map_or_else(
            || signature.clone(),
            |origin| SigOrigin::Bound(origin.clone()),
        ),
        SigOrigin::Projected(base, path) => {
            SigOrigin::Projected(Box::new(bind_sig_origin(base, tails)), path.clone())
        }
        SigOrigin::Union(members) => {
            SigOrigin::union(members.iter().map(|member| bind_sig_origin(member, tails)))
        }
        _ => signature.clone(),
    }
}

pub(in crate::checker) fn sig_origin_has_bound(
    signature: &mojito_types::origin::SigOrigin,
) -> bool {
    use mojito_types::origin::SigOrigin;
    match signature {
        SigOrigin::Bound(_) => true,
        SigOrigin::Projected(base, _) => sig_origin_has_bound(base),
        SigOrigin::Union(members) => members.iter().any(sig_origin_has_bound),
        _ => false,
    }
}

/// Whether a struct field's declared type borrows origin parameter `id` — a
/// `ref[o]` field or an origin-bearing `Pointer[..., o]` field.
/// Whether any place constituent of `origin` roots at `owner`. Used to detect
/// an origin-parameter resolution that fell back to the receiver's own storage
/// (no construction-time binding was recorded) rather than naming the borrowed
/// source — such a resolution is still symbolic for write-legality judgments.
pub(in crate::checker) fn origin_rooted_at(
    origin: &mojito_types::origin::Origin,
    owner: mojito_types::origin::OwnerId,
) -> bool {
    use mojito_types::origin::Origin;
    match origin {
        Origin::Place(place) => place.root == owner,
        Origin::Union(members) => members.iter().any(|member| origin_rooted_at(member, owner)),
        Origin::MutCast(inner, _) => origin_rooted_at(inner, owner),
        _ => false,
    }
}

pub(in crate::checker) fn field_carries_origin_param(
    ty: &Ty,
    id: mojito_types::origin::OriginParamId,
) -> bool {
    use mojito_types::origin::{Origin, PointerOrigin};
    match ty.kind() {
        TyKind::Ref(reference) => {
            matches!(reference.origin, Origin::Param(k) if k == id)
                || matches!(reference.pointer_origin(), Some(PointerOrigin::Param { id: k, .. }) if k == id)
        }
        TyKind::LitRefPack {
            origin, mutability, ..
        } => matches!(
            PointerOrigin::from_parts(origin.clone(), *mutability),
            Some(PointerOrigin::Param { id: k, .. }) if k == id
        ),
        pointer => pointer
            .pointer_origin()
            .is_some_and(|origin| matches!(origin, PointerOrigin::Param { id: k, .. } if k == id)),
    }
}

/// Substitute what `bindings` binds into `ty`, simultaneously, recursing
/// through struct arguments, tuples, and reference referents: every struct
/// origin argument naming a bound binder, a projection re-applied below
/// the bound origin ([`Origin::reprojected`]), and every `mut` argument
/// reading a bound binder's `mut`. A `Pointer` is one more struct
/// application, refolded by [`Ty::new`], so a pointer over a binder bound
/// to a symbolic cast becomes the nominal symbolic-cast pointer. An
/// application that held a pointer keeps its origin and `mut` arguments
/// when the bound ones are no pointer's (an unbound slot, a `mut` its
/// origin contradicts), and a reference or reference pack its own,
/// alike.
///
/// [`Origin::reprojected`]: mojito_types::origin::Origin::reprojected
pub(in crate::checker) fn substitute_struct_origins(ty: &Ty, bindings: &OriginBindings) -> Ty {
    if bindings.is_empty() {
        return ty.clone();
    }
    let recur = |ty: &Ty| substitute_struct_origins(ty, bindings);
    match ty.kind() {
        struct_ty if let Some((name, arguments)) = struct_ty.as_non_simd_struct() => {
            let held = bindings.undecided_mut_binders(arguments);
            let rebuild = |bind: bool| {
                ty.reusing(TyKind::Struct(
                    name.clone(),
                    arguments
                        .iter()
                        .map(|argument| match argument {
                            TyArg::Ty(ty) => TyArg::Ty(recur(ty)),
                            TyArg::Origin(origin) if bind => {
                                TyArg::Origin(bindings.bind_origin(origin, &held))
                            }
                            TyArg::Val(value) if bind => {
                                TyArg::Val(bindings.bind_mut_argument(value))
                            }
                            other => other.clone(),
                        })
                        .collect(),
                ))
            };
            let bound = rebuild(true);
            if struct_ty.is_pointer() && !bound.kind().is_pointer() && !holds_symbolic_cast(&bound)
            {
                rebuild(false)
            } else {
                bound
            }
        }
        TyKind::Ref(reference) => {
            let (origin, mutability) =
                bindings.bind_reference(&reference.origin, reference.mutability);
            TyKind::Ref(mojito_types::origin::RefTy {
                referent: recur(&reference.referent),
                origin,
                mutability,
                address_space: reference.address_space.clone(),
            })
            .into()
        }
        TyKind::LitRefPack {
            elements,
            origin,
            mutability,
        } => {
            let (origin, mutability) = bindings.bind_reference(origin, *mutability);
            TyKind::LitRefPack {
                elements: elements.iter().map(recur).collect(),
                origin,
                mutability,
            }
            .into()
        }
        TyKind::Tuple(elements) => TyKind::Tuple(elements.iter().map(recur).collect()).into(),
        _ => ty.clone(),
    }
}

/// Bind a trait method's `origin_of(self)` arguments to the receiver's
/// `origin`: `C.IteratorType[origin_of(self)]` from `items.__iter__()` names
/// `C.IteratorType[origin_of(items)]`, recursing through an associated
/// projection's base (`C.IteratorType[o].Element`) and struct arguments.
pub(in crate::checker) fn bind_self_origin_arguments(
    ty: &Ty,
    origin: &mojito_types::origin::Origin,
) -> Ty {
    use mojito_types::origin::Origin;
    // The receiver's origin in place of `origin_of(self)`, below an
    // `ImmOrigin(...)` cast and with an interior or subtree projection
    // re-applied (`origin_of(self)._get_owned_interior["bytes"]`); a
    // receiver origin that cannot carry the projection binds whole.
    fn rebase(
        argument: &mojito_types::origin::Origin,
        receiver: &mojito_types::origin::Origin,
    ) -> Option<mojito_types::origin::Origin> {
        match argument {
            Origin::SelfParam => Some(receiver.clone()),
            Origin::Projected(base, path) if **base == Origin::SelfParam => Some(
                receiver
                    .project_path(path)
                    .unwrap_or_else(|| receiver.clone()),
            ),
            Origin::MutCast(inner, mutability) => {
                rebase(inner, receiver).map(|origin| Origin::mut_cast(origin, mutability.clone()))
            }
            _ => None,
        }
    }
    let bind = |argument: &TyArg| match argument {
        TyArg::Origin(argument) => {
            TyArg::Origin(rebase(argument, origin).unwrap_or_else(|| argument.clone()))
        }
        TyArg::Ty(ty) => TyArg::Ty(bind_self_origin_arguments(ty, origin)),
        TyArg::Val(_) => argument.clone(),
    };
    match ty.kind() {
        struct_ty if let Some((name, arguments)) = struct_ty.as_non_pointer_struct() => ty.reusing(
            TyKind::Struct(name.clone(), arguments.iter().map(bind).collect()),
        ),
        TyKind::Assoc { base, name, args } => ty.reusing(TyKind::Assoc {
            base: bind_self_origin_arguments(base, origin),
            name: name.clone(),
            args: args.iter().map(bind).collect(),
        }),
        _ => ty.clone(),
    }
}

/// Replace each `Origin::Param(id)` for which `concrete` yields a binding with that
/// concrete origin, recursing through unions and leaving every other origin as-is.
pub(in crate::checker) fn substitute_origin_params(
    origin: mojito_types::origin::Origin,
    concrete: &impl Fn(mojito_types::origin::OriginParamId) -> Option<mojito_types::origin::Origin>,
) -> mojito_types::origin::Origin {
    use mojito_types::origin::Origin;
    match origin {
        Origin::Param(id) => concrete(id).unwrap_or(Origin::Param(id)),
        Origin::Union(members) => Origin::union(
            members
                .into_iter()
                .map(|member| substitute_origin_params(member, concrete)),
        ),
        Origin::MutCast(inner, mutability) => {
            Origin::mut_cast(substitute_origin_params(*inner, concrete), mutability)
        }
        Origin::Projected(base, path) => {
            substitute_origin_params(*base, concrete).reprojected(&path)
        }
        other => other,
    }
}

pub(in crate::checker) fn substitute_sig_origin(
    signature: &mojito_types::origin::SigOrigin,
    actual: &[Option<mojito_types::origin::Origin>],
) -> mojito_types::origin::Origin {
    use mojito_types::origin::{Origin, SigOrigin};
    match signature {
        SigOrigin::Self_ | SigOrigin::Carried(_) => Origin::Union(vec![]),
        SigOrigin::Bound(origin) => origin.clone(),
        SigOrigin::Param(index) => actual
            .get(*index)
            .and_then(Clone::clone)
            .unwrap_or(Origin::Union(vec![])),
        SigOrigin::Static => Origin::Static,
        SigOrigin::Untracked { mutable } | SigOrigin::UnsafeAny { mutable } => {
            Origin::Untracked { mutable: *mutable }
        }
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
    signature: &mojito_types::origin::SigOrigin,
    actual: &[Option<mojito_types::origin::Origin>],
    self_origin: Option<mojito_types::origin::Origin>,
) -> mojito_types::origin::Origin {
    use mojito_types::origin::{Origin, SigOrigin};
    match signature {
        SigOrigin::Self_ => self_origin.unwrap_or_else(|| Origin::Union(vec![])),
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

/// Whether `origin` ends in a `._subtree` projection, below which nothing
/// projects.
pub(in crate::checker) fn ends_in_subtree(origin: &mojito_types::origin::Origin) -> bool {
    use mojito_types::origin::{Origin, OriginSeg};
    match origin {
        Origin::Place(place) => matches!(place.path.last(), Some(OriginSeg::Subtree)),
        Origin::Projected(_, path) => matches!(path.last(), Some(OriginSeg::Subtree)),
        Origin::MutCast(inner, _) => ends_in_subtree(inner),
        _ => false,
    }
}

/// `origin` with the receiver's symbolic origin (`origin_of(self)`) read as
/// `receiver`, an interior or subtree projection re-applied below it.
pub(in crate::checker) fn rebase_self_origin(
    origin: &mojito_types::origin::Origin,
    receiver: &mojito_types::origin::Origin,
) -> mojito_types::origin::Origin {
    use mojito_types::origin::Origin;
    match origin {
        Origin::SelfParam => receiver.clone(),
        Origin::Projected(base, path) if **base == Origin::SelfParam => receiver
            .project_path(path)
            .unwrap_or_else(|| receiver.clone()),
        Origin::MutCast(inner, mutability) => {
            Origin::mut_cast(rebase_self_origin(inner, receiver), mutability.clone())
        }
        Origin::Union(members) => Origin::union(
            members
                .iter()
                .map(|member| rebase_self_origin(member, receiver)),
        ),
        other => other.clone(),
    }
}

pub(in crate::checker) fn origin_is_within(
    actual: &mojito_types::origin::Origin,
    allowed: &mojito_types::origin::Origin,
) -> bool {
    use mojito_types::origin::Origin;
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

/// Whether an argument's `actual` origin converts to the `allowed` origin an
/// explicit origin argument fixes ([`origin_is_declared`]). Two staged
/// projections read as the origin upstream gives them: an owner's interior
/// generation is its owner's origin, as upstream names a container's owned
/// storage by the container's origin, and the conservative subtree a pointer
/// minted from a `ref` binding takes is the binding's own origin, as
/// upstream's `Pointer(to=r)` is `origin_of(r)`.
pub(in crate::checker) fn origin_converts_to(
    actual: &mojito_types::origin::Origin,
    allowed: &mojito_types::origin::Origin,
) -> bool {
    origin_is_declared(actual, allowed)
        || origin_is_declared(&actual.without_subtree().interior_owner(), allowed)
}

/// Whether a returned reference's origin is one its declaration names: every
/// member of `actual` must be a member of `declared` itself, not storage
/// within one. A field is a different origin from its owner, and an element
/// interior from its container, as upstream's return check compares them.
/// An index whose value is not part of the origin identity stands for the one
/// element segment at its position, a declared `_subtree` covers the places
/// beneath its base, an untracked declared origin accepts any reference, and
/// a mutability cast (`ImmOrigin(o)`) names the storage of the origin it casts.
pub(in crate::checker) fn origin_is_declared(
    actual: &mojito_types::origin::Origin,
    declared: &mojito_types::origin::Origin,
) -> bool {
    use mojito_types::origin::{Origin, OriginSeg};
    let same_segment = |actual: &OriginSeg, declared: &OriginSeg| {
        let element =
            |segment: &OriginSeg| matches!(segment, OriginSeg::AnyIndex | OriginSeg::Interior(_));
        actual == declared
            || (matches!(actual, OriginSeg::AnyIndex) && element(declared))
            || (matches!(declared, OriginSeg::AnyIndex) && element(actual))
    };
    let same_path = |actual: &[OriginSeg], declared: &[OriginSeg]| {
        actual.len() == declared.len()
            && actual
                .iter()
                .zip(declared)
                .all(|(actual, declared)| same_segment(actual, declared))
    };
    match (actual, declared) {
        (Origin::MutCast(actual, _), _) => origin_is_declared(actual, declared),
        (_, Origin::MutCast(declared, _)) => origin_is_declared(actual, declared),
        (Origin::Union(members), _) => members
            .iter()
            .all(|member| origin_is_declared(member, declared)),
        (_, Origin::Union(members)) => members
            .iter()
            .any(|member| origin_is_declared(actual, member)),
        (_, declared) if declared.is_untracked() => true,
        (Origin::Place(actual), Origin::Place(declared)) => {
            actual.root == declared.root
                && match declared.path.split_last() {
                    Some((OriginSeg::Subtree, base)) => actual
                        .path
                        .get(..base.len())
                        .is_some_and(|prefix| same_path(prefix, base)),
                    _ => same_path(&actual.path, &declared.path),
                }
        }
        _ => actual == declared,
    }
}

pub(in crate::checker) fn ref_parameter_is_writable(
    parameter: &FnParam,
    decls: &[ParamDecl],
    values: &dyn OriginDecls,
) -> bool {
    ref_binding_is_writable(
        parameter.convention,
        parameter.origin.as_deref(),
        decls,
        values,
    )
}

/// Whether a parameter/receiver may be mutated while its generic body is
/// checked. A bare `ref` has parametric mutability: it propagates the caller's
/// capability to returned references, but its body cannot assume that the
/// caller supplied mutable storage. Only an explicitly mutable origin grants
/// unconditional write access: one through a builtin mutable origin, or
/// through an origin binder of the classified list `decls` declared
/// `mut=True`.
pub(in crate::checker) fn ref_binding_is_writable(
    convention: Option<ArgConvention>,
    origin: Option<&[Expr]>,
    decls: &[ParamDecl],
    values: &dyn OriginDecls,
) -> bool {
    if convention != Some(ArgConvention::Ref) {
        return parameter_is_writable(&convention);
    }
    let origins = clause_origins(origin, decls);
    let Some([expression]) = origins.as_deref() else {
        return false;
    };
    let Some(origin_name) = origin_binder_name(expression) else {
        return values.value(expression).is_some_and(|(_, mutable)| mutable);
    };
    match origin_decl(decls, origin_name) {
        Some((decl, _)) => {
            origin_decl_mutability(decl, decls) == mojito_types::origin::SigMutability::Mutable
        }
        None => values.value(expression).is_some_and(|(_, mutable)| mutable),
    }
}

/// Translate a concrete callable's effects through a recorded call-through
/// mapping into effects of the OUTER callee. A frame-local source flowing
/// into a signature destination rejects: the body-local loan would dangle
/// once the higher-order body returns. `Self_` terms of a callable-struct's
/// `__call__` map to the callable slot itself (its runtime-parameter
/// storage); for a compile-time callable value there is no such storage, so
/// those terms stay a documented permissive residue.
pub(in crate::checker) fn translate_call_through(
    effects: &[mojito_checked::checked::TransferEffect],
    through: &mojito_checked::checked::CallThroughEffect,
) -> Result<Vec<mojito_checked::checked::TransferEffect>, TypeError> {
    use mojito_checked::checked::{CallThroughCallee, TransferEffect};
    use mojito_types::origin::SigOrigin;
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

/// The name supplied for a callable value parameter at a call site, the
/// bracket list bound to the callee's declarations as the check binds it
/// ([`mojito_ast::call::bind_parameter_slots`]). `None` for a defaulted,
/// unnamed, unbound (`_`, `...`), or unmatched argument (permissive).
pub(in crate::checker) fn callable_value_argument(
    decls: &[mojito_types::types::ParamDecl],
    decl_name: &str,
    param_args: &[mojito_ast::ast::ParamArg],
) -> Option<String> {
    use mojito_ast::ast::ParamArg;
    let index = decls.iter().position(|decl| decl.name() == decl_name)?;
    let keywords = mojito_ast::call::bracket_keywords(param_args);
    let argument = mojito_ast::call::bind_parameter_slots(decls, &keywords)
        .into_iter()
        .zip(param_args)
        .find_map(|(slot, argument)| (slot == Ok(index)).then_some(argument))?;
    let argument = match argument {
        ParamArg::Named { value, .. } => value.as_ref(),
        positional => positional,
    };
    if mojito_ast::call::unbound_marker(argument).is_some() {
        return None;
    }
    match argument {
        ParamArg::Type(mojito_ast::ast::Type::Named(name, arguments)) if arguments.is_empty() => {
            Some(name.clone())
        }
        ParamArg::Value(Expr {
            kind: ExprKind::Identifier(name),
            ..
        }) => Some(name.clone()),
        _ => None,
    }
}

/// The `mut` an `Origin[mut=m]` binder `id` takes bound to `origin`, as
/// upstream infers `m` from the bound origin's type: a symbolic cast's
/// own expression, else `binder_mutability`'s answer (a place's storage
/// never takes a binder's), else the binder or receiver the origin is
/// rooted in, else the mutability the origin alone decides. An unbound
/// slot decides none.
fn inferred_mut_argument(
    id: mojito_types::origin::OriginParamId,
    origin: &mojito_types::origin::Origin,
    binder_mutability: BinderMutability<'_>,
) -> Option<CtValue> {
    use mojito_types::origin::{Mutability, Origin};
    if matches!(origin, Origin::Unbound) {
        return None;
    }
    if let Some(expression) = origin.symbolic_cast_mutability() {
        return Some(CtValue::Expr(expression.clone()));
    }
    let symbolic = origin.uncast().binder_mutability().or_else(|| {
        matches!(origin.uncast().projection_base(), Origin::SelfParam)
            .then_some(Mutability::RECEIVER)
    });
    let mutability = match binder_mutability(id, origin) {
        Some(Mutability::Param(_)) if symbolic.is_none() => Mutability::Immutable,
        Some(mutability) => mutability,
        None => symbolic.or_else(|| origin.static_mutability().map(Mutability::of))?,
    };
    Some(mutability.mut_argument())
}

/// Whether `ty` is an application with a symbolic-cast origin argument
/// (`o.unsafe_mut_cast[m]()`), which stays nominal until `m` folds.
fn holds_symbolic_cast(ty: &Ty) -> bool {
    ty.kind()
        .as_non_simd_struct()
        .is_some_and(|(_, arguments)| {
            arguments.iter().any(|argument| {
            matches!(argument, TyArg::Origin(origin) if origin.symbolic_cast_mutability().is_some())
        })
        })
}
