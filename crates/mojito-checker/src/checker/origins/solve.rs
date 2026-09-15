//! Per-call origin solving and aggregate-origin escape analysis.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

impl Checker {
    /// Whether an owner is bound BELOW the current function's scope base —
    /// enclosing-frame storage a nested def reaches through captures
    /// (`self`, parameters, or enclosing locals), as opposed to storage the
    /// current frame itself introduced.
    pub(in crate::checker) fn owner_in_enclosing_scope(
        &self,
        owner: mojito_types::origin::OwnerId,
    ) -> bool {
        let Some(base) = self.function_bases.last().copied() else {
            return false;
        };
        self.owner_scopes
            .iter()
            .take(base)
            .any(|scope| scope.values().any(|candidate| *candidate == owner))
    }

    /// Judge the struct origin tails of a returned value against the
    /// enclosing body's return annotation resolved over the body's own
    /// places, where `origin_of(self.items)` names the receiver's field and
    /// not, as in the signature, the whole receiver. A tail that fits takes
    /// the declared signature origin, so the ordinary return coercion sees
    /// the signature's spelling; one that does not is rejected with
    /// upstream's text. Every non-tail part of the type stays as found.
    pub(in crate::checker) fn reconcile_return_origin_tails(
        &self,
        found: &Ty,
        declared: &Ty,
    ) -> Result<Ty, TypeError> {
        use mojito_types::origin::{Origin, OriginSeg};
        let Some(Some((annotation, generated))) = self.return_annotations.last() else {
            return Ok(found.clone());
        };
        let resolved = if *generated {
            self.resolve_generated_return_annotation(annotation)
        } else {
            self.resolve_return_annotation(annotation)
        };
        let Ok(bound) = resolved else {
            return Ok(found.clone());
        };
        let frames = self.transfer_frames.borrow();
        let frame = frames.last();
        // A found tail fits the bound one when it coerces, or when both name
        // the same receiver or parameter storage itself — one symbolically,
        // the other as its place, or the bound through the storage's owned
        // interior regions. A projection onto a field never fits a wider
        // bound.
        let self_owner = frame.and_then(|frame| frame.self_owner);
        let is_param_owner = |root| frame.is_some_and(|frame| frame.param_owners.contains(&root));
        let interior_only = |place: &mojito_types::origin::OriginPlace| {
            place
                .path
                .iter()
                .all(|segment| matches!(segment, OriginSeg::Interior(_) | OriginSeg::Subtree))
        };
        let fits = |actual: &Origin, bound: &Origin| {
            actual.coerces_to(bound)
                || match (actual, bound) {
                    (Origin::Place(place), Origin::SelfParam) => {
                        interior_only(place) && self_owner == Some(place.root)
                    }
                    (Origin::Place(place), Origin::Param(_)) => {
                        interior_only(place) && is_param_owner(place.root)
                    }
                    (Origin::Place(place), Origin::Place(bound)) => {
                        interior_only(place) && bound.root == place.root && interior_only(bound)
                    }
                    (Origin::SelfParam, Origin::Place(bound)) => {
                        interior_only(bound) && self_owner == Some(bound.root)
                    }
                    (Origin::Param(_), Origin::Place(bound)) => {
                        interior_only(bound) && is_param_owner(bound.root)
                    }
                    _ => false,
                }
        };
        let mut mismatch = false;
        let reconciled = reconcile_origin_tails(found, declared, &bound, &fits, &mut mismatch);
        if mismatch {
            return Err(TypeError::OriginIdentityMismatch {
                found: self.display_ty_with_origin_names(found),
                expected: self.display_ty_with_origin_names(&bound),
            });
        }
        Ok(reconciled)
    }

    /// The return annotation a body's `return` re-resolves — a value return,
    /// not a reference return — flagged when it is the rebound annotation of
    /// a compiler-generated (`$`-mangled) specialization.
    pub(in crate::checker) fn body_return_annotation(
        annotation: Option<&mojito_ast::ast::SourceType>,
        name: &str,
    ) -> Option<(mojito_ast::ast::SourceType, bool)> {
        annotation
            .filter(|annotation| !matches!(annotation, mojito_ast::ast::SourceType::Ref { .. }))
            .map(|annotation| (annotation.clone(), name.contains('$')))
    }

    /// Abstract a body-level origin to a signature-relative origin for a
    /// transfer effect. `None` means no caller-side loan is needed
    /// (static/untracked storage) or the origin is not signature-expressible.
    /// Interior paths abstract to their root — a coarser, sound
    /// over-approximation of the transferred loan. A capture-reachable
    /// enclosing owner abstracts to a concrete `Bound` origin (owner ids are
    /// checker-global), grounded where the storage lives.
    pub(in crate::checker) fn abstract_body_origin(
        &self,
        origin: &mojito_types::origin::Origin,
        param_owners: &[mojito_types::origin::OwnerId],
        self_owner: Option<mojito_types::origin::OwnerId>,
    ) -> Option<mojito_types::origin::SigOrigin> {
        use mojito_types::origin::{Origin, SigOrigin};
        match origin {
            Origin::Place(place) => {
                if Some(place.root) == self_owner {
                    return Some(SigOrigin::Self_);
                }
                if let Some(index) = param_owners.iter().position(|owner| *owner == place.root) {
                    return Some(SigOrigin::Param(index));
                }
                self.owner_in_enclosing_scope(place.root).then(|| {
                    SigOrigin::Bound(Origin::Place(mojito_types::origin::OriginPlace {
                        root: place.root,
                        path: Vec::new(),
                    }))
                })
            }
            Origin::SelfParam => Some(SigOrigin::Self_),
            Origin::Union(origins) => {
                let members: Vec<_> = origins
                    .iter()
                    .filter_map(|origin| {
                        self.abstract_body_origin(origin, param_owners, self_owner)
                    })
                    .collect();
                match members.len() {
                    0 => None,
                    1 => members.into_iter().next(),
                    _ => Some(SigOrigin::union(members)),
                }
            }
            Origin::Param(_) | Origin::Static | Origin::Untracked { .. } | Origin::Unbound => None,
        }
    }

    pub(in crate::checker) fn aggregate_origin_escapes(
        &self,
        origin: &mojito_types::origin::Origin,
    ) -> bool {
        use mojito_types::origin::Origin;
        let Some((base, allowed)) = self.aggregate_escape_contexts.last() else {
            return false;
        };
        match origin {
            Origin::Place(place) => {
                let scope = self
                    .owner_scopes
                    .iter()
                    .position(|owners| owners.values().any(|candidate| *candidate == place.root));
                match scope {
                    Some(scope) => scope >= *base && !allowed.contains(&place.root),
                    // An owner registered in no named scope is a materialized
                    // borrow-source temporary: frame-local by construction,
                    // so it escapes unless explicitly allowed.
                    None => !allowed.contains(&place.root),
                }
            }
            Origin::Union(origins) => origins
                .iter()
                .any(|origin| self.aggregate_origin_escapes(origin)),
            Origin::Param(_)
            | Origin::SelfParam
            | Origin::Static
            | Origin::Untracked { .. }
            | Origin::Unbound => false,
        }
    }

    pub(in crate::checker) fn solve_call_origins(
        &self,
        slots: &[ArgSlot],
        conventions: &[Option<ArgConvention>],
        signatures: &[Option<mojito_types::origin::RefSig>],
        return_signature: Option<&mojito_types::origin::RefSig>,
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Result<
        (
            Vec<Option<ArgConvention>>,
            Option<mojito_types::origin::RefTy>,
        ),
        TypeError,
    > {
        let (conventions, returned, _) = self.solve_call_origins_with_bool_bindings(
            slots,
            conventions,
            signatures,
            return_signature,
            args,
            kwargs,
            false,
        )?;
        Ok((conventions, returned))
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::checker) fn solve_call_origins_with_bool_bindings(
        &self,
        slots: &[ArgSlot],
        conventions: &[Option<ArgConvention>],
        signatures: &[Option<mojito_types::origin::RefSig>],
        return_signature: Option<&mojito_types::origin::RefSig>,
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
        carrier_slots: bool,
    ) -> Result<SolvedCallOrigins, TypeError> {
        use mojito_types::origin::{Mutability, Origin, RefTy, SigMutability};
        let mut effective = conventions.to_vec();
        let mut origins = vec![None; slots.len()];
        let mut mutable = vec![false; slots.len()];
        // The declaration convention, not the effective alias-checking
        // convention below, determines whether execution needs the caller's
        // place. An immutable `ref` becomes a shared read for conflict
        // checking, but the VM still needs its handle through the call.
        for (index, convention) in conventions.iter().enumerate() {
            if !matches!(convention, Some(ArgConvention::Mut | ArgConvention::Ref)) {
                continue;
            }
            let Some(slot) = slots.get(index) else {
                continue;
            };
            let expression = match slot {
                ArgSlot::Positional(position) => &args[*position],
                ArgSlot::Keyword(position) => &kwargs[*position].value,
                ArgSlot::Default => continue,
            };
            self.call_place_uses
                .borrow_mut()
                .insert(expression.source_span());
        }
        for (index, signature) in signatures.iter().enumerate() {
            let Some(signature) = signature else { continue };
            let Some(slot) = slots.get(index) else {
                continue;
            };
            let expression = match slot {
                ArgSlot::Positional(position) => &args[*position],
                ArgSlot::Keyword(position) => &kwargs[*position].value,
                ArgSlot::Default => continue,
            };
            let actual = self.materialized_reference_actual(expression)?;
            let is_mutable = actual.mutability == Mutability::Mutable;
            let requires_mutable = matches!(signature.mutability, SigMutability::Mutable);
            if requires_mutable && !is_mutable {
                return Err(TypeError::ImmutableBinding(
                    "reference argument".to_string(),
                ));
            }
            origins[index] = Some(actual.origin);
            mutable[index] = match signature.mutability {
                SigMutability::Immutable => false,
                SigMutability::Mutable => true,
                SigMutability::BoolParam(_) | SigMutability::Infer => is_mutable,
            };
            if !mutable[index] {
                effective[index] = Some(ArgConvention::Imm);
            }
        }
        // A by-value carrier parameter the returned reference borrows
        // (`def first_key(c: EntryCursor) -> ref[c.current().key] Int` — a
        // parameter-rooted delegated clause resolved to the carrier itself):
        // the returned reference designates storage the carrier borrows, so
        // it loans the sources the carrier holds (its construction-time
        // origins — upstream's exact origin; the executable handle re-roots
        // at that storage, not at the carrier). A carrier holding nothing
        // falls back to the caller's place itself. Free-function calls only
        // (`carrier_slots`): in a method signature `SigOrigin::Param` indexes
        // the struct's origin binders, not argument slots.
        if let (true, Some(return_signature)) = (carrier_slots, return_signature) {
            for (index, slot) in slots.iter().enumerate() {
                if origins[index].is_some()
                    || !sig_origin_mentions_param(&return_signature.origin, index)
                {
                    continue;
                }
                let expression = match slot {
                    ArgSlot::Positional(position) => &args[*position],
                    ArgSlot::Keyword(position) => &kwargs[*position].value,
                    ArgSlot::Default => continue,
                };
                if let Ok(place) = self.origin_place(expression) {
                    let carried = self.aggregate_origins(expression);
                    origins[index] = Some(if carried.is_empty() {
                        Origin::Place(place)
                    } else {
                        Origin::union(carried)
                    });
                }
            }
        }
        // A mutable or parametrically-mutable argument may redefine every
        // interior origin below the passed place. This is an explicit checked
        // call effect; lowering must not infer it from a generic `Call` place.
        for (index, convention) in effective.iter().enumerate() {
            if !matches!(convention, Some(ArgConvention::Mut | ArgConvention::Ref)) {
                continue;
            }
            let Some(slot) = slots.get(index) else {
                continue;
            };
            let expression = match slot {
                ArgSlot::Positional(position) => &args[*position],
                ArgSlot::Keyword(position) => &kwargs[*position].value,
                ArgSlot::Default => continue,
            };
            if let Some(origin) = origins.get(index).and_then(Clone::clone) {
                let except = match &expression.kind {
                    ExprKind::Identifier(name) if matches!(self.lookup(name), Some(Ty::Ref(_))) => {
                        self.lookup_owner(name)
                    }
                    _ => None,
                };
                self.record_aggregate_origin_invalidation_except(
                    expression.source_span(),
                    origin,
                    except,
                );
            } else {
                self.record_interior_invalidation(expression.source_span(), expression);
            }
        }
        for (index, signature) in signatures.iter().enumerate() {
            // An exact-origin clause (`ImmStaticOrigin`, `ImmUntrackedOrigin`,
            // `MutUntrackedOrigin`) binds only an actual carrying that origin:
            // a tracked place does not convert, as upstream. (`*UnsafeAnyOrigin`
            // binds anything.)
            if let Some(signature) = signature
                && let Some(actual) = origins.get(index).and_then(Option::as_ref)
                && let Some(expected) = exact_origin_mismatch(&signature.origin, actual)
            {
                let expression = match slots.get(index) {
                    Some(ArgSlot::Positional(position)) => Some(&args[*position]),
                    Some(ArgSlot::Keyword(position)) => Some(&kwargs[*position].value),
                    _ => None,
                };
                let ty = expression
                    .map(|expression| self.infer(expression))
                    .transpose()?
                    .map(|ty| ty.to_string())
                    .unwrap_or_default();
                return Err(TypeError::RefOriginMismatch {
                    slot: index,
                    ty,
                    expected: expected.to_string(),
                    actual: expression
                        .and_then(spelled_origin)
                        .unwrap_or_else(|| actual.to_string()),
                });
            }
            if let Some(signature) = signature
                && sig_origin_has_bound(&signature.origin)
                && let Some(actual) = origins.get(index).and_then(Option::as_ref)
            {
                let allowed = substitute_sig_origin(&signature.origin, &origins);
                if !origin_is_within(actual, &allowed) {
                    return Err(TypeError::TypeMismatch {
                        expected: format!("the specialized origin {allowed:?}"),
                        found: format!("the argument origin {actual:?}"),
                        context: "call through an origin-specialized function value".to_string(),
                    });
                }
            }
        }
        let returned = return_signature.map(|signature| {
            let origin = substitute_sig_origin(&signature.origin, &origins);
            let is_mutable = match &signature.mutability {
                SigMutability::Immutable => false,
                SigMutability::Mutable => true,
                SigMutability::BoolParam(parameter) => {
                    signatures.iter().enumerate().any(|(i, sig)| {
                    sig.as_ref().is_some_and(|sig| {
                            matches!(sig.mutability, SigMutability::BoolParam(other) if other == *parameter)
                            && mutable[i]
                    })
                    })
                }
                SigMutability::Infer => origins
                    .iter()
                    .enumerate()
                    .any(|(i, o)| o.is_some() && mutable[i]),
            };
            RefTy {
                referent: Box::new(Ty::None), // replaced by the caller's declared return type
                origin,
                mutability: if is_mutable {
                    Mutability::Mutable
                } else {
                    Mutability::Immutable
                },
            }
        });
        let mut bool_bindings = HashMap::new();
        for (index, signature) in signatures.iter().enumerate() {
            let Some(mojito_types::origin::RefSig {
                mutability: SigMutability::BoolParam(parameter),
                ..
            }) = signature
            else {
                continue;
            };
            if origins.get(index).and_then(Option::as_ref).is_none() {
                continue;
            }
            if let Some(previous) = bool_bindings.insert(*parameter, mutable[index])
                && previous != mutable[index]
            {
                return Err(TypeError::BadCall {
                    func: "reference argument".to_string(),
                    reason:
                        "arguments infer conflicting values for one origin mutability parameter"
                            .to_string(),
                });
            }
        }
        Ok((effective, returned, bool_bindings))
    }
}

impl Checker {
    /// Judge a `var` initializer against the explicit origin arguments its
    /// annotation supplied (erased from the resolved type): an
    /// `ImmStaticOrigin` slot rejects any borrowed local place (the `ref`
    /// demand's verdict, `solve_call_origins_with_bool_bindings`), and an
    /// `origin_of(place)` slot rejects a value borrowing only other roots.
    /// A value borrowing nothing (a literal view) satisfies any demand.
    #[allow(
        clippy::unused_self,
        reason = "TODO: make an associated function or use the receiver"
    )]
    pub(in crate::checker) fn check_storage_origin_demands(
        &self,
        context: &str,
        demands: &[(String, mojito_types::origin::Origin)],
        actuals: &[mojito_types::origin::Origin],
    ) -> Result<(), TypeError> {
        use mojito_types::origin::Origin;
        for (parameter, demand) in demands {
            match demand {
                Origin::Static => {
                    if actuals
                        .iter()
                        .any(|actual| !matches!(actual, Origin::Static))
                    {
                        return Err(TypeError::Unsupported(
                            "a local place cannot satisfy ImmStaticOrigin".to_string(),
                        ));
                    }
                }
                Origin::Place(place)
                    if !actuals.is_empty()
                        && !actuals
                            .iter()
                            .any(|actual| origin_rooted_at(actual, place.root)) =>
                {
                    return Err(TypeError::TypeMismatch {
                        expected: format!(
                            "a value borrowing the place named by the '{parameter}' origin \
                             argument"
                        ),
                        found: "a value borrowing a different place".to_string(),
                        context: context.to_string(),
                    });
                }
                _ => {}
            }
        }
        Ok(())
    }
}

/// The declared exact origin `signature` demands when `actual` does not
/// carry it: `Static` needs a static actual; `Untracked { mutable }` needs an
/// untracked actual whose permission covers the demand.
fn exact_origin_mismatch(
    signature: &mojito_types::origin::SigOrigin,
    actual: &mojito_types::origin::Origin,
) -> Option<&'static str> {
    use mojito_types::origin::{Origin, SigOrigin};
    match signature {
        SigOrigin::Static => (!matches!(actual, Origin::Static)).then_some("ImmStaticOrigin"),
        SigOrigin::Untracked { mutable } => {
            let satisfied =
                matches!(actual, Origin::Untracked { mutable: have } if !*mutable || *have);
            (!satisfied).then_some(if *mutable {
                "MutUntrackedOrigin"
            } else {
                "ImmUntrackedOrigin"
            })
        }
        _ => None,
    }
}

/// Upstream's `origin_of(<place>)` spelling of an argument's origin, for a
/// place spelled as an identifier or a member chain.
fn spelled_origin(expression: &Expr) -> Option<String> {
    fn place_text(expression: &Expr) -> Option<String> {
        match &expression.kind {
            ExprKind::Identifier(name) => Some(name.clone()),
            ExprKind::Member { object, field } => Some(format!("{}.{field}", place_text(object)?)),
            _ => None,
        }
    }
    place_text(expression).map(|text| format!("origin_of({text})"))
}

/// See [`Checker::reconcile_return_origin_tails`]: walk `found`, `declared`,
/// and the body-resolved `bound` in parallel. A found tail origin that
/// `fits` the bound one takes the declared origin, a tail still left to
/// inference stays as found, and any other tail sets `mismatch`.
fn reconcile_origin_tails(
    found: &Ty,
    declared: &Ty,
    bound: &Ty,
    fits: &dyn Fn(&mojito_types::origin::Origin, &mojito_types::origin::Origin) -> bool,
    mismatch: &mut bool,
) -> Ty {
    use mojito_types::origin::Origin;
    match (found, declared, bound) {
        (
            Ty::Struct(found_name, found_args),
            Ty::Struct(declared_name, declared_args),
            Ty::Struct(bound_name, bound_args),
        ) if found_name == declared_name
            && declared_name == bound_name
            && found_args.len() == declared_args.len()
            && declared_args.len() == bound_args.len() =>
        {
            Ty::Struct(
                found_name.clone(),
                found_args
                    .iter()
                    .zip(declared_args)
                    .zip(bound_args)
                    .map(
                        |((found, declared), bound)| match (found, declared, bound) {
                            (TyArg::Ty(found), TyArg::Ty(declared), TyArg::Ty(bound)) => TyArg::Ty(
                                reconcile_origin_tails(found, declared, bound, fits, mismatch),
                            ),
                            (
                                TyArg::Origin(Origin::Unbound),
                                TyArg::Origin(_),
                                TyArg::Origin(_),
                            ) => found.clone(),
                            (
                                TyArg::Origin(actual),
                                TyArg::Origin(expected),
                                TyArg::Origin(bound),
                            ) => {
                                if fits(actual, bound) {
                                    TyArg::Origin(expected.clone())
                                } else {
                                    *mismatch |= body_place_origin(actual);
                                    found.clone()
                                }
                            }
                            _ => found.clone(),
                        },
                    )
                    .collect(),
            )
        }
        (Ty::Tuple(found_elements), Ty::Tuple(declared_elements), Ty::Tuple(bound_elements))
            if found_elements.len() == declared_elements.len()
                && declared_elements.len() == bound_elements.len() =>
        {
            Ty::Tuple(
                found_elements
                    .iter()
                    .zip(declared_elements)
                    .zip(bound_elements)
                    .map(|((found, declared), bound)| {
                        reconcile_origin_tails(found, declared, bound, fits, mismatch)
                    })
                    .collect(),
            )
        }
        // The public `Tuple` and its generated specialization symbol compare
        // by element (see `coerces`); reconcile the elements in place.
        (Ty::Struct(found_name, found_args), _, _)
            if let (Some(found_elements), Some(declared_elements), Some(bound_elements)) = (
                mojito_types::types::tuple_elements(found),
                mojito_types::types::tuple_elements(declared),
                mojito_types::types::tuple_elements(bound),
            ) && found_elements.len() == declared_elements.len()
                && declared_elements.len() == bound_elements.len() =>
        {
            let reconciled: Vec<Ty> = found_elements
                .iter()
                .zip(declared_elements)
                .zip(bound_elements)
                .map(|((found, declared), bound)| {
                    reconcile_origin_tails(found, declared, bound, fits, mismatch)
                })
                .collect();
            let mut elements = reconciled.into_iter();
            Ty::Struct(
                found_name.clone(),
                found_args
                    .iter()
                    .map(|argument| match argument {
                        TyArg::Ty(_) => elements.next().map_or_else(|| argument.clone(), TyArg::Ty),
                        TyArg::Val(_) | TyArg::Origin(_) => argument.clone(),
                    })
                    .collect(),
            )
        }
        _ => found.clone(),
    }
}

/// Whether a found tail names the body's own storage — a place, or a union
/// of places — rather than a symbolic origin left from a callee's signature,
/// which the return annotation cannot judge.
fn body_place_origin(origin: &mojito_types::origin::Origin) -> bool {
    use mojito_types::origin::Origin;
    match origin {
        Origin::Place(_) => true,
        Origin::Union(members) => members.iter().all(body_place_origin),
        _ => false,
    }
}
