//! Reference actuals: origin places for argument expressions,
//! materialized borrows, and receiver origin-argument resolution.

use super::*;

impl Checker {
    /// Convert a source place into the stable, projection-sensitive identity
    /// used by checked origins. Index values are intentionally abstracted: the
    /// loan checker must conservatively treat arbitrary indices as overlapping.
    pub(in crate::checker) fn origin_place(
        &self,
        expr: &Expr,
    ) -> Result<mojito_types::origin::OriginPlace, TypeError> {
        use mojito_types::origin::{OriginPlace, OriginSeg};
        if let Some(interior) = self
            .interior_references
            .borrow()
            .get(&expr.source_span())
            .cloned()
        {
            return Ok(interior);
        }
        match &expr.kind {
            ExprKind::Identifier(name) => {
                if let Some(Ty::Ref(reference)) = self.lookup(name)
                    && let mojito_types::origin::Origin::Place(place) = &reference.origin
                {
                    return Ok(place.clone());
                }
                let root = self
                    .lookup_owner(name)
                    .ok_or_else(|| TypeError::UndefinedVariable(name.clone()))?;
                Ok(OriginPlace {
                    root,
                    path: Vec::new(),
                })
            }
            ExprKind::Member { object, field } => {
                if matches!(self.place_storage_ty(expr), Some(Ty::Ref(_))) {
                    fn collect_places(
                        origin: mojito_types::origin::Origin,
                        places: &mut Vec<OriginPlace>,
                    ) {
                        match origin {
                            mojito_types::origin::Origin::Place(place) => places.push(place),
                            mojito_types::origin::Origin::Union(members) => {
                                for member in members {
                                    collect_places(member, places);
                                }
                            }
                            mojito_types::origin::Origin::Param(_)
                            | mojito_types::origin::Origin::SelfParam
                            | mojito_types::origin::Origin::Static
                            | mojito_types::origin::Origin::Untracked { .. } => {}
                        }
                    }

                    let mut referents = Vec::new();
                    for origin in self.aggregate_origins(expr) {
                        collect_places(origin, &mut referents);
                    }
                    referents.sort();
                    referents.dedup();
                    if let [referent] = referents.as_slice() {
                        return Ok(referent.clone());
                    }
                }
                let mut place = self.origin_place(object)?;
                place.path.push(OriginSeg::Field(field.clone()));
                Ok(place)
            }
            ExprKind::Index { object, .. } => {
                let mut place = self.origin_place(object)?;
                place.path.push(OriginSeg::AnyIndex);
                Ok(place)
            }
            ExprKind::TypeApply { name, .. }
                if self
                    .operation_adjustments
                    .borrow()
                    .get(&expr.source_span())
                    .is_some_and(|operation| {
                        matches!(
                            operation,
                            mojito_checked::checked::SemanticAdjustment::VariantProject { .. }
                        )
                    }) =>
            {
                let root = self
                    .lookup_owner(name)
                    .ok_or_else(|| TypeError::UndefinedVariable(name.clone()))?;
                // `record_interior_reference` adds the payload's named
                // `Interior("value")` segment after this base is resolved.
                Ok(OriginPlace {
                    root,
                    path: Vec::new(),
                })
            }
            _ => Err(TypeError::Unsupported(
                "reference binding to a non-place expression".to_string(),
            )),
        }
    }

    /// Resolve the reference capability actually supplied by an expression.
    /// A reference-valued identifier/field can carry a union of concrete
    /// referents; collapsing it to the handle slot would lose both escape and
    /// interior-generation facts when the handle is forwarded through another
    /// call. Plain places synthesize the corresponding place reference.
    pub(in crate::checker) fn reference_actual(
        &self,
        expr: &Expr,
    ) -> Result<mojito_types::origin::RefTy, TypeError> {
        use mojito_types::origin::{Mutability, Origin, RefTy};

        if let Some(mut reference) = self.infer_reference_value(expr) {
            let retained = self.aggregate_origins(expr);
            if !retained.is_empty() {
                reference.origin = Origin::union(retained);
            }
            return Ok(reference);
        }

        let place = self.origin_place(expr)?;
        let mutability = if self.owner_is_mutable(place.root) {
            Mutability::Mutable
        } else {
            Mutability::Immutable
        };
        Ok(RefTy {
            referent: Box::new(self.infer(expr)?),
            origin: Origin::Place(place),
            mutability,
        })
    }

    /// [`Self::reference_actual`], extended with upstream's temporary-lifetime
    /// rule for borrow positions that accept an rvalue (`ref` constructor and
    /// origin-annotated parameters, `ref` bindings): a non-place expression
    /// materializes as an anonymous mutable owned binding — lowering stores it
    /// in a hidden frame slot registered under the minted identity, so the
    /// borrow roots at a real frame-local place and the temporary lives as
    /// long as its borrower.
    pub(in crate::checker) fn materialized_reference_actual(
        &self,
        expr: &Expr,
    ) -> Result<mojito_types::origin::RefTy, TypeError> {
        use mojito_types::origin::{Mutability, Origin, OriginPlace, RefTy};
        match self.reference_actual(expr) {
            Err(TypeError::Unsupported(message))
                if message.contains("reference binding to a non-place expression") =>
            {
                let referent = self.infer(expr)?;
                let mut adjustments = self.operation_adjustments.borrow_mut();
                let owner = match adjustments.get(&expr.source_span()) {
                    Some(
                        mojito_checked::checked::SemanticAdjustment::MaterializeBorrowSource {
                            owner,
                        },
                    ) => *owner,
                    // Another adjustment already owns this span's lowering
                    // contract; keep the plain rejection rather than fight it.
                    Some(_) => {
                        return Err(TypeError::Unsupported(
                            "reference binding to a non-place expression".to_string(),
                        ));
                    }
                    None => {
                        let owner = self.fresh_owner()?;
                        adjustments.insert(
                            expr.source_span(),
                            mojito_checked::checked::SemanticAdjustment::MaterializeBorrowSource {
                                owner,
                            },
                        );
                        owner
                    }
                };
                Ok(RefTy {
                    referent: Box::new(referent),
                    origin: Origin::Place(OriginPlace {
                        root: owner,
                        path: Vec::new(),
                    }),
                    mutability: Mutability::Mutable,
                })
            }
            other => other,
        }
    }

    /// The abstract origin a returned reference stays within when it is carried by,
    /// or projected out of, a reference *value* — a `ref[origin] T` field or
    /// binding whose declared origin is a struct/callable origin parameter rather
    /// than a concrete place. Such a handle already names its borrowed region, so
    /// returning the handle itself, or a field/element reached through it, stays
    /// within that region's parameter; `reference_actual` would otherwise
    /// re-synthesize the storage as a concrete place rooted at the receiver, losing
    /// the parameter binding a `ref[origin]` return contract is checked against.
    ///
    /// Recognized: a directly-read reference value (`self.value`), a field/element
    /// projected through one (`self.src[i]`, `self.pair.first`), and dereferencing
    /// an origin-bearing *pointer* field whose origin is a parameter (`self.p[0]`,
    /// with `p: UnsafePointer[T, o]`). In each case the VM re-roots the runtime
    /// handle at the borrowed storage; for the pointer deref it forwards the
    /// offset-0 index to the single pointee.
    pub(in crate::checker) fn returned_reference_parameter_origin(
        &self,
        expr: &Expr,
    ) -> Option<mojito_types::origin::Origin> {
        use mojito_types::origin::Origin;
        fn is_abstract(origin: &Origin) -> bool {
            match origin {
                Origin::Place(_) => false,
                Origin::Union(members) => !members.is_empty() && members.iter().all(is_abstract),
                Origin::Param(_)
                | Origin::SelfParam
                | Origin::Static
                | Origin::Untracked { .. } => true,
            }
        }
        match &expr.kind {
            ExprKind::Transfer(inner) | ExprKind::Named { value: inner, .. } => {
                self.returned_reference_parameter_origin(inner)
            }
            // A reference-valued place carries its declared origin directly; a
            // field/element projected out of one stays within that origin's region.
            ExprKind::Member { object, .. } => self
                .infer_reference_value(expr)
                .map(|reference| reference.origin)
                .filter(is_abstract)
                .or_else(|| self.returned_reference_parameter_origin(object)),
            // Dereferencing an origin-bearing pointer field (`self.p[0]`) reaches
            // storage inside the pointer's origin parameter, just like indexing a
            // `ref[o]` aggregate. The pointer is not a reference *value*, so recover
            // the parameter directly from its declared origin; the VM re-roots the
            // returned handle at the pointee and forwards the offset-0 index.
            ExprKind::Index { object, .. } => {
                if let Ok(Ty::Pointer {
                    origin: mojito_types::origin::PointerOrigin::Param { id, .. },
                    ..
                }) = self.infer(object)
                {
                    return Some(Origin::Param(id));
                }
                self.returned_reference_parameter_origin(object)
            }
            ExprKind::Identifier(_) => self
                .infer_reference_value(expr)
                .map(|reference| reference.origin)
                .filter(is_abstract),
            // A delegated ref-returning call: the returned handle stays within
            // the callee's declared return region. The call-site record may
            // already carry the abstract origin; otherwise re-instantiate the
            // callee's contract against the receiver's checked struct type
            // arguments (the loan-oriented record concretizes to the receiver
            // place, which is the wrong identity for the return contract).
            ExprKind::MethodCall { object, method, .. } => {
                let recorded = self
                    .operation_adjustments
                    .borrow()
                    .get(&expr.source_span())
                    .and_then(|adjustment| match adjustment {
                        mojito_checked::checked::SemanticAdjustment::ReferenceResult {
                            reference,
                        } => Some(reference.origin.clone()),
                        _ => None,
                    });
                if let Some(origin) = recorded.filter(is_abstract) {
                    return Some(origin);
                }
                let Ok(Ty::Struct(name, arguments)) = self.infer(object) else {
                    return None;
                };
                let info = self.structs.get(&name)?;
                let signature = info
                    .methods
                    .get(method)?
                    .iter()
                    .find(|signature| signature.ref_return.is_some())?;
                let declared = signature.ref_return.as_ref()?;
                let mut origin = instantiate_sig_origin(&declared.origin, &arguments);
                // Origin arguments erase from struct type identity, so the
                // callee's origin binder usually stays symbolic in its own
                // namespace; remap it through the field application's
                // recorded binder correspondences, then fall back to the
                // unambiguous single-binder case the signature resolver uses
                // for any binder the record leaves unmapped (the same
                // map-then-fallback chain as `map_delegated_sig_origin`).
                let map = self
                    .delegated_receiver_binder_map(object)
                    .unwrap_or_default();
                let unmapped = std::cell::Cell::new(false);
                origin = super::origins::substitute_origin_params(origin, &|id| {
                    let mapped = map
                        .iter()
                        .find(|(callee_index, _)| *callee_index == id.0)
                        .map(|(_, enclosing)| {
                            Origin::Param(mojito_types::origin::OriginParamId(*enclosing))
                        });
                    if mapped.is_none() {
                        unmapped.set(true);
                    }
                    mapped
                });
                if unmapped.get() {
                    let callee_binders = info
                        .source_params
                        .iter()
                        .filter(|param| param.bounds.as_slice() == ["Origin"])
                        .count();
                    let mut enclosing = self
                        .enclosing_type_params
                        .iter()
                        .enumerate()
                        .filter(|(_, param)| param.bounds.as_slice() == ["Origin"]);
                    if let (Some((index, _)), None, 1) =
                        (enclosing.next(), enclosing.next(), callee_binders)
                    {
                        let binder =
                            Origin::Param(mojito_types::origin::OriginParamId(index as u32));
                        origin = super::origins::substitute_origin_params(origin, &|_| {
                            Some(binder.clone())
                        });
                    } else if !receiver_rooted_at_self(object) {
                        // A parameter-rooted delegation nothing can name
                        // resolves to the carrier itself (the signature
                        // resolver's `receiver_fallback`): the concrete
                        // returned origin is the carrier's place.
                        return self.origin_place(object).ok().map(Origin::Place);
                    }
                }
                Some(origin).filter(is_abstract)
            }
            _ => None,
        }
    }

    /// Resolve any abstract struct origin parameters in a method-returned
    /// reference's origin to the receiver's concrete stored origin arguments. A
    /// method returning `ref[o] T` for a struct origin parameter `o` yields the
    /// abstract `Origin::Param(o)`; the loan machinery only tracks concrete places,
    /// so this maps `o` back to the origin the receiver's `ref[o]` (or origin-bearing
    /// `UnsafePointer[..., o]`) field borrows — recorded when the aggregate was
    /// constructed — mirroring what `reference_actual` does for a directly-read
    /// reference field. Without it a returned reference records no loan on its
    /// ultimate source, so the source is dropped while the reference is still live.
    pub(in crate::checker) fn resolve_receiver_origin_arguments(
        &self,
        origin: mojito_types::origin::Origin,
        object: &Expr,
    ) -> mojito_types::origin::Origin {
        let Ok(Ty::Struct(name, _)) = self.infer(object) else {
            return origin;
        };
        let Some(info) = self.structs.get(&name) else {
            return origin;
        };
        let field_origins = self.aggregate_field_origins(object);
        let flat = self.aggregate_origins(object);
        // The concrete origin(s) the receiver's field carrying origin parameter
        // `id` borrows, from the aggregate's construction-time bindings (falling
        // back to the flat receiver origins for a single-origin-param struct).
        let concrete =
            |id: mojito_types::origin::OriginParamId| -> Option<mojito_types::origin::Origin> {
                let mut retained: Vec<mojito_types::origin::Origin> = Vec::new();
                for (field, ty) in &info.fields {
                    // A direct ref/pointer field names its binder in its checked
                    // type; a struct-typed field's linkage survives only in the
                    // recorded origin application (`var second: EntryCursor[Self.o2]`).
                    let carries = field_carries_origin_param(ty, id)
                        || info.field_origin_arguments.get(field).is_some_and(|pairs| {
                            pairs.iter().any(|(_, enclosing)| *enclosing == id.0)
                        });
                    if !carries {
                        continue;
                    }
                    let origins = field_origins
                        .get(field)
                        .filter(|origins| !origins.is_empty())
                        .cloned()
                        .unwrap_or_else(|| flat.clone());
                    for origin in origins {
                        if !retained.contains(&origin) {
                            retained.push(origin);
                        }
                    }
                }
                if retained.is_empty() {
                    let origin_params = info
                        .source_params
                        .iter()
                        .enumerate()
                        .filter(|(_, parameter)| parameter.bounds.as_slice() == ["Origin"])
                        .map(|(index, _)| mojito_types::origin::OriginParamId(index as u32))
                        .collect::<Vec<_>>();
                    if origin_params.as_slice() == [id] {
                        retained.extend(flat.clone());
                    }
                }
                (!retained.is_empty()).then(|| mojito_types::origin::Origin::union(retained))
            };
        substitute_origin_params(origin, &concrete)
    }

    /// Lower a reference-signature origin clause, first resolving upstream's
    /// expression-origin spelling `ref [recv.method().field...]` — a delegated
    /// ref-returning call projection — into the existing `SigOrigin` forms.
    /// Non-delegated clauses fall through to the pure `lower_ref_sig`.
    ///
    /// Supported delegated subset (each violation is a contextual error): a
    /// single origin member; a zero-argument callee method; a receiver path
    /// rooted at `self` or a parameter through struct fields; a callee whose
    /// explicit (non-`Infer`) ref-return contract is already registered.
    /// Trailing field projections (`.key`) stay within the delegated call's
    /// returned region and are dropped from the origin identity, exactly as
    /// `returned_reference_parameter_origin` documents for projections
    /// through a reference value.
    pub(in crate::checker) fn lower_ref_sig_resolved(
        &self,
        spec: &mojito_ast::ast::OriginSpec,
        type_params: &[mojito_ast::ast::TypeParam],
        params: &[&FnParam],
        struct_params: usize,
    ) -> Result<mojito_types::origin::RefSig, TypeError> {
        if let [member] = spec.as_slice()
            && let Some(resolved) = self.delegated_call_ref_sig(member, type_params, params)?
        {
            return Ok(resolved);
        }
        lower_ref_sig(spec, type_params, params, struct_params)
    }
}
