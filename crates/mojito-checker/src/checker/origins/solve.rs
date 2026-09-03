//! Per-call origin solving and aggregate-origin escape analysis.

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
            Origin::Param(_) | Origin::Static | Origin::Untracked { .. } => None,
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
            Origin::Param(_) | Origin::SelfParam | Origin::Static | Origin::Untracked { .. } => {
                false
            }
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
            if signature.as_ref().is_some_and(|signature| {
                matches!(signature.origin, mojito_types::origin::SigOrigin::Static)
            }) && origins
                .get(index)
                .and_then(Option::as_ref)
                .is_some_and(|origin| !matches!(origin, Origin::Static))
            {
                return Err(TypeError::Unsupported(
                    "a local place cannot satisfy ImmStaticOrigin".to_string(),
                ));
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
