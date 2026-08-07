//! Origin and reference-handle machinery for the checker: origin-place
//! derivation, interior/aggregate-origin tracking and invalidation, reference
//! parameter handles, capture-origin collection, per-call origin solving, and
//! origin-signature lowering. Extracted from `checker.rs`; see `docs/symbol-map.md`.

use super::*;

impl Checker {
    /// Convert a source place into the stable, projection-sensitive identity
    /// used by checked origins. Index values are intentionally abstracted: the
    /// loan checker must conservatively treat arbitrary indices as overlapping.
    pub(super) fn origin_place(
        &self,
        expr: &Expr,
    ) -> Result<crate::origin::OriginPlace, TypeError> {
        use crate::origin::{OriginPlace, OriginSeg};
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
                    && let crate::origin::Origin::Place(place) = &reference.origin
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
                        origin: crate::origin::Origin,
                        places: &mut Vec<OriginPlace>,
                    ) {
                        match origin {
                            crate::origin::Origin::Place(place) => places.push(place),
                            crate::origin::Origin::Union(members) => {
                                for member in members {
                                    collect_places(member, places);
                                }
                            }
                            crate::origin::Origin::Param(_)
                            | crate::origin::Origin::SelfParam
                            | crate::origin::Origin::Static
                            | crate::origin::Origin::Untracked { .. } => {}
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
                            crate::checked::SemanticAdjustment::VariantProject { .. }
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
    pub(super) fn reference_actual(&self, expr: &Expr) -> Result<crate::origin::RefTy, TypeError> {
        use crate::origin::{Mutability, Origin, RefTy};

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
    pub(super) fn returned_reference_parameter_origin(
        &self,
        expr: &Expr,
    ) -> Option<crate::origin::Origin> {
        use crate::origin::Origin;
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
                    origin: crate::origin::PointerOrigin::Param { id, .. },
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
    pub(super) fn resolve_receiver_origin_arguments(
        &self,
        origin: crate::origin::Origin,
        object: &Expr,
    ) -> crate::origin::Origin {
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
        let concrete = |id: crate::origin::OriginParamId| -> Option<crate::origin::Origin> {
            let mut retained: Vec<crate::origin::Origin> = Vec::new();
            for (field, ty) in &info.fields {
                if !field_carries_origin_param(ty, id) {
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
            (!retained.is_empty()).then(|| crate::origin::Origin::union(retained))
        };
        substitute_origin_params(origin, &concrete)
    }

    /// Resolve the compile-time value accepted by an `Origin` parameter at a
    /// function-value specialization site. `origin_of` observes checked places
    /// (including reference-valued places) and never evaluates at runtime.
    pub(super) fn explicit_origin_argument(
        &self,
        argument: &crate::ast::ParamArg,
    ) -> Result<crate::origin::Origin, TypeError> {
        use crate::ast::ParamArg;
        use crate::origin::Origin;

        let expression = match argument {
            ParamArg::Value(expression) => expression,
            ParamArg::Named { value, .. } => return self.explicit_origin_argument(value),
            ParamArg::Type(_) => {
                return Err(TypeError::TypeMismatch {
                    expected: "an Origin value".to_string(),
                    found: "a type".to_string(),
                    context: "explicit callable origin specialization".to_string(),
                });
            }
        };
        match &expression.kind {
            ExprKind::Call {
                name,
                args,
                kwargs,
                param_args,
            } if name == "origin_of" && kwargs.is_empty() && param_args.is_empty() => {
                if args.is_empty() {
                    return Err(TypeError::Unsupported(
                        "origin_of requires at least one place".to_string(),
                    ));
                }
                args.iter()
                    .map(|place| {
                        // In an abstract signature — a trait method's return type
                        // such as `Self.IteratorType[origin_of(self)]` — there is
                        // no bound `self` place. Represent the receiver origin
                        // symbolically so the application still carries an
                        // argument; it is resolved to the concrete receiver origin
                        // at the conformance/call site and erases at runtime.
                        if let ExprKind::Identifier(name) = &place.kind
                            && name == "self"
                            && self.lookup_owner("self").is_none()
                        {
                            return Ok(Origin::SelfParam);
                        }
                        self.reference_actual(place)
                            .map(|reference| reference.origin)
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map(Origin::union)
            }
            ExprKind::Identifier(name) if name == "StaticOrigin" => Ok(Origin::Static),
            ExprKind::Identifier(name) if name == "UntrackedOrigin" => {
                Ok(Origin::Untracked { mutable: false })
            }
            ExprKind::Identifier(name) if name == "UnsafeAnyOrigin" => {
                Ok(Origin::Untracked { mutable: true })
            }
            _ => Err(TypeError::TypeMismatch {
                expected: "origin_of(place) or a builtin Origin value".to_string(),
                found: "a runtime value".to_string(),
                context: "explicit callable origin specialization".to_string(),
            }),
        }
    }

    /// Replay a callee's transfer effects against a call's actuals: enforce
    /// the store-outward rule across the boundary, merge the transferred
    /// origins into the destination binding's bookkeeping, derive the
    /// enclosing callable's transitive effect, and record the substituted
    /// transfer for MIR loan installation.
    pub(super) fn apply_transfer_effects(
        &self,
        callee: &str,
        receiver: Option<&Expr>,
        args: &[Expr],
        span: &SourceSpan,
    ) -> Result<(), TypeError> {
        use crate::checked::{CheckedCallTransfer, CheckedTransferDest};
        use crate::origin::SigOrigin;
        let effects = match self.transfer_effects.borrow().get(callee) {
            Some(effects) => effects.clone(),
            None => return Ok(()),
        };
        let mut call_transfers = Vec::new();
        for effect in &effects {
            let actual = |origin: &SigOrigin| match origin {
                SigOrigin::Self_ => receiver,
                SigOrigin::Param(index) => args.get(*index),
                _ => None,
            };
            let (Some(dest_expr), Some(src_expr)) = (actual(&effect.dest), actual(&effect.src))
            else {
                continue;
            };
            // The caller-side origins of the source ACTUAL EXPRESSION — this
            // covers moved temporaries (`RefBox(alias)`), whose loans root at
            // the places their construction borrowed.
            let mut sources = self.aggregate_origins(src_expr);
            // A reference-valued source contributes its referent's origin; a
            // plain moved value transfers ownership and adds no loan of its
            // own storage.
            if let Some(reference) = self.infer_reference_value(src_expr)
                && !sources.contains(&reference.origin)
            {
                sources.push(reference.origin);
            }
            // A borrowed-parameter source loans the actual's own storage.
            if effect.src_is_place
                && let Ok(place) = self.origin_place(src_expr)
            {
                let origin = crate::origin::Origin::Place(place);
                if !sources.contains(&origin) {
                    sources.push(origin);
                }
            }
            sources.retain(|origin| {
                !matches!(
                    origin,
                    crate::origin::Origin::Static | crate::origin::Origin::Untracked { .. }
                )
            });
            if sources.is_empty() {
                continue;
            }
            let dest_root_owner =
                place_root_name(dest_expr).and_then(|root| self.lookup_owner(root));
            // The store-outward rule, across the call: a destination rooted
            // at an outliving owner must not receive an escaping loan.
            if let (Some((_, allowed)), Some(owner)) =
                (self.aggregate_escape_contexts.last(), dest_root_owner)
                && allowed.contains(&owner)
                && sources
                    .iter()
                    .any(|origin| self.aggregate_origin_escapes(origin))
            {
                return Err(TypeError::StoredReferenceEscapesOrigin);
            }
            // Merge into the destination binding's origin bookkeeping so the
            // checker's own return-escape rule sees callee-installed loans.
            if let Some(owner) = dest_root_owner {
                let mut overlay = self.transferred_origins.borrow_mut();
                let merged = overlay.entry(owner).or_default();
                for origin in &sources {
                    if !merged.contains(origin) {
                        merged.push(origin.clone());
                    }
                }
            }
            // Transitive derivation: a destination rooted at the enclosing
            // callable's parameter or `self` re-abstracts, so wrappers carry
            // their callees' effects outward.
            if let Some(owner) = dest_root_owner {
                let (dest_sig, param_owners, self_owner) = {
                    let frames = self.transfer_frames.borrow();
                    match frames.last() {
                        Some(frame) => {
                            let dest_sig = if Some(owner) == frame.self_owner {
                                Some(SigOrigin::Self_)
                            } else {
                                frame
                                    .param_owners
                                    .iter()
                                    .position(|candidate| *candidate == owner)
                                    .map(SigOrigin::Param)
                            };
                            (dest_sig, frame.param_owners.clone(), frame.self_owner)
                        }
                        None => (None, Vec::new(), None),
                    }
                };
                if let Some(dest_sig) = dest_sig {
                    for origin in &sources {
                        if let Some(src_sig) =
                            self.abstract_body_origin(origin, &param_owners, self_owner)
                            && src_sig != dest_sig
                        {
                            let src_is_place = {
                                let frames = self.transfer_frames.borrow();
                                match (&src_sig, frames.last()) {
                                    (SigOrigin::Param(index), Some(frame)) => {
                                        frame.param_borrowed.get(*index).copied().unwrap_or(false)
                                    }
                                    (SigOrigin::Self_, _) => true,
                                    _ => false,
                                }
                            };
                            let derived = crate::checked::TransferEffect {
                                dest: dest_sig.clone(),
                                src: src_sig,
                                src_is_place,
                                mutable: effect.mutable,
                            };
                            let mut frames = self.transfer_frames.borrow_mut();
                            if let Some(frame) = frames.last_mut()
                                && !frame.effects.contains(&derived)
                            {
                                frame.effects.push(derived);
                            }
                        }
                    }
                }
            }
            let dest = match &effect.dest {
                SigOrigin::Self_ => CheckedTransferDest::Receiver,
                SigOrigin::Param(index) => CheckedTransferDest::Argument(*index),
                _ => continue,
            };
            call_transfers.push(CheckedCallTransfer {
                dest,
                sources,
                mutable: effect.mutable,
            });
        }
        if !call_transfers.is_empty() {
            self.call_transfers
                .borrow_mut()
                .insert(span.clone(), call_transfers);
        }
        Ok(())
    }

    /// Record an accepted outliving store as a transfer effect on the
    /// enclosing callable's accumulation frame. Constructor bodies record
    /// nothing (their arguments are caller-visible; the local aggregate path
    /// installs those loans), and self-to-self transfers are skipped so
    /// internal reshuffles do not self-loan every call.
    pub(super) fn record_transfer_effect(
        &self,
        place: &Expr,
        origins: &[crate::origin::Origin],
        storage: &Option<Ty>,
    ) {
        use crate::origin::SigOrigin;
        if self.self_initializing {
            return;
        }
        let mut frames = self.transfer_frames.borrow_mut();
        let Some(frame) = frames.last_mut() else {
            return;
        };
        let Some(root) = place_root_name(place) else {
            return;
        };
        let Some(owner) = self.lookup_owner(root) else {
            return;
        };
        let dest = if Some(owner) == frame.self_owner {
            SigOrigin::Self_
        } else if let Some(index) = frame
            .param_owners
            .iter()
            .position(|candidate| *candidate == owner)
        {
            SigOrigin::Param(index)
        } else {
            return;
        };
        let mutable = match storage {
            Some(Ty::Ref(reference)) => reference.mutability == crate::origin::Mutability::Mutable,
            _ => true,
        };
        let (param_owners, param_borrowed, self_owner) = (
            frame.param_owners.clone(),
            frame.param_borrowed.clone(),
            frame.self_owner,
        );
        for origin in origins {
            let Some(src) = self.abstract_body_origin(origin, &param_owners, self_owner) else {
                continue;
            };
            if src == dest {
                continue;
            }
            // A loan rooted at a borrowed (`mut`/`ref`) parameter's own
            // place is a loan on the CALLER's storage bound to that slot; an
            // owned parameter only forwards loans its moved value carries.
            let src_is_place = match &src {
                SigOrigin::Param(index) => param_borrowed.get(*index).copied().unwrap_or(false),
                SigOrigin::Self_ => true,
                _ => false,
            };
            let effect = crate::checked::TransferEffect {
                dest: dest.clone(),
                src,
                src_is_place,
                mutable,
            };
            if !frame.effects.contains(&effect) {
                frame.effects.push(effect);
            }
        }
    }

    /// Abstract a body-level origin to a signature-relative origin for a
    /// transfer effect. `None` means no caller-side loan is needed
    /// (static/untracked storage) or the origin is not signature-expressible.
    /// Interior paths abstract to their root — a coarser, sound
    /// over-approximation of the transferred loan.
    pub(super) fn abstract_body_origin(
        &self,
        origin: &crate::origin::Origin,
        param_owners: &[crate::origin::OwnerId],
        self_owner: Option<crate::origin::OwnerId>,
    ) -> Option<crate::origin::SigOrigin> {
        use crate::origin::{Origin, SigOrigin};
        match origin {
            Origin::Place(place) => {
                if Some(place.root) == self_owner {
                    return Some(SigOrigin::Self_);
                }
                param_owners
                    .iter()
                    .position(|owner| *owner == place.root)
                    .map(SigOrigin::Param)
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

    pub(super) fn aggregate_origin_escapes(&self, origin: &crate::origin::Origin) -> bool {
        use crate::origin::Origin;
        let Some((base, allowed)) = self.aggregate_escape_contexts.last() else {
            return false;
        };
        match origin {
            Origin::Place(place) => {
                let scope = self
                    .owner_scopes
                    .iter()
                    .position(|owners| owners.values().any(|candidate| *candidate == place.root));
                scope.is_some_and(|scope| scope >= *base && !allowed.contains(&place.root))
            }
            Origin::Union(origins) => origins
                .iter()
                .any(|origin| self.aggregate_origin_escapes(origin)),
            Origin::Param(_) | Origin::SelfParam | Origin::Static | Origin::Untracked { .. } => {
                false
            }
        }
    }

    pub(super) fn solve_call_origins(
        &self,
        slots: &[ArgSlot],
        conventions: &[Option<ArgConvention>],
        signatures: &[Option<crate::origin::RefSig>],
        return_signature: Option<&crate::origin::RefSig>,
        args: &[Expr],
        kwargs: &[crate::ast::KwArg],
    ) -> Result<(Vec<Option<ArgConvention>>, Option<crate::origin::RefTy>), TypeError> {
        use crate::origin::{Mutability, Origin, RefTy, SigMutability};
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
            let actual = self.reference_actual(expression)?;
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
                effective[index] = Some(ArgConvention::Read);
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
                matches!(signature.origin, crate::origin::SigOrigin::Static)
            }) && origins
                .get(index)
                .and_then(Option::as_ref)
                .is_some_and(|origin| !matches!(origin, Origin::Static))
            {
                return Err(TypeError::Unsupported(
                    "a local place cannot satisfy StaticOrigin".to_string(),
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
        Ok((effective, returned))
    }
}

/// Recognize the current-nightly origin-attribute spelling
/// `base._get_owned_interior["tag"]`. It is accepted only in origin clauses;
/// ordinary expression typing still has no runtime member by this name.
pub(super) fn interior_origin_syntax(expr: &Expr) -> Option<(&Expr, &str)> {
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

/// The declaration-level immutable-origin cast `Origin[mut=False].cast_from[o]`
/// — current Mojo's spelling for pinning a reference result's capability to
/// read-only independent of the origin parameter's own `mut=`. Returns the
/// inner origin expression; the upgrade direction (`mut=True`) is rejected.
pub(super) fn immutable_origin_cast(expression: &Expr) -> Option<Result<&Expr, TypeError>> {
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
        crate::ast::ParamArg::Named {
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
    let crate::ast::ParamArg::Value(flag) = value.as_ref() else {
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

pub(super) fn validate_origin_expr(
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
    match &expr.kind {
        ExprKind::Identifier(name)
            if name == "_"
                || name == "self"
                || name == "StaticOrigin"
                || name == "UntrackedOrigin"
                || name == "UnsafeAnyOrigin"
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

pub(super) fn lower_ref_param_sigs(
    type_params: &[crate::ast::TypeParam],
    params: &[&FnParam],
) -> Result<Vec<Option<crate::origin::RefSig>>, TypeError> {
    params
        .iter()
        .map(|param| {
            if param.convention != Some(ArgConvention::Ref) {
                return Ok(None);
            }
            match &param.origin {
                Some(spec) => lower_ref_sig(spec, type_params, params).map(Some),
                None => Ok(Some(crate::origin::RefSig {
                    origin: crate::origin::SigOrigin::Infer,
                    mutability: crate::origin::SigMutability::Infer,
                })),
            }
        })
        .collect()
}

pub(super) fn callable_origin_signature(
    type_params: &[crate::ast::TypeParam],
    params: &[&FnParam],
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
                                matches!(
                                    &expression.kind,
                                    ExprKind::Identifier(name) if name == &parameter.name
                                )
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
    CallableOriginSignature { origins, source }
}

pub(super) fn lower_ref_sig(
    spec: &crate::ast::OriginSpec,
    type_params: &[crate::ast::TypeParam],
    params: &[&FnParam],
) -> Result<crate::origin::RefSig, TypeError> {
    use crate::origin::{RefSig, SigMutability, SigOrigin};
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
        if let Some((base_expression, name)) = interior_origin_syntax(expression) {
            let base = lower_sig_origin_expression(base_expression, type_params, params)?;
            // A projection off a named origin parameter carries that
            // parameter's declared mutability, exactly like the bare binder.
            if let ExprKind::Identifier(base_name) = &base_expression.kind
                && let Some(origin_param) = type_params.iter().find(|parameter| {
                    parameter.name == *base_name && parameter.bounds.as_slice() == ["Origin"]
                })
            {
                mutability = origin_param_mutability(origin_param, type_params);
            }
            members.push(SigOrigin::Projected(
                Box::new(base),
                vec![crate::origin::OriginSeg::Interior(name.to_string())],
            ));
            continue;
        }
        match &expression.kind {
            ExprKind::Identifier(name) if name == "_" => members.push(SigOrigin::Infer),
            ExprKind::Identifier(name) if name == "self" => members.push(SigOrigin::Self_),
            ExprKind::Identifier(name) if name == "StaticOrigin" => {
                members.push(SigOrigin::Static);
                mutability = SigMutability::Immutable;
            }
            ExprKind::Identifier(name) if name == "UntrackedOrigin" => {
                members.push(SigOrigin::Untracked { mutable: false });
                mutability = SigMutability::Immutable;
            }
            ExprKind::Identifier(name) if name == "UnsafeAnyOrigin" => {
                members.push(SigOrigin::Untracked { mutable: true });
                mutability = SigMutability::Mutable;
            }
            ExprKind::Identifier(name) => {
                if let Some(index) = params.iter().position(|param| param.name == *name) {
                    members.push(SigOrigin::Param(index));
                    continue;
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
                        matches!(origin.as_slice(), [Expr { kind: ExprKind::Identifier(bound), .. }] if bound == name)
                    }) {
                        members.push(SigOrigin::Param(index));
                    }
                }
                // An enclosing struct Origin can be carried by reference-valued
                // fields even when no ordinary method parameter binds it. Keep
                // that checked semantic binder directly in the method contract
                // instead of collapsing it to an empty inferred union.
                if members.len() == first_member {
                    members.push(SigOrigin::Bound(crate::origin::Origin::Param(
                        crate::origin::OriginParamId(origin_param_index as u32),
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
fn origin_param_mutability(
    origin_param: &crate::ast::TypeParam,
    type_params: &[crate::ast::TypeParam],
) -> crate::origin::SigMutability {
    use crate::origin::SigMutability;
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

pub(super) fn lower_sig_origin_expression(
    expression: &Expr,
    type_params: &[crate::ast::TypeParam],
    params: &[&FnParam],
) -> Result<crate::origin::SigOrigin, TypeError> {
    use crate::origin::SigOrigin;
    if let Some((base, name)) = interior_origin_syntax(expression) {
        return Ok(SigOrigin::Projected(
            Box::new(lower_sig_origin_expression(base, type_params, params)?),
            vec![crate::origin::OriginSeg::Interior(name.to_string())],
        ));
    }
    match &expression.kind {
        ExprKind::Identifier(name) if name == "self" => Ok(SigOrigin::Self_),
        ExprKind::Identifier(name) => {
            if let Some(index) = params.iter().position(|parameter| parameter.name == *name) {
                return Ok(SigOrigin::Param(index));
            }
            if let Some((origin_param_index, _)) =
                type_params.iter().enumerate().find(|(_, parameter)| {
                    parameter.name == *name && parameter.bounds.as_slice() == ["Origin"]
                })
            {
                // A named origin parameter is represented by the value
                // parameter(s) carrying it in this callable contract. When no
                // parameter carries it — an enclosing struct origin bound only
                // by reference-valued fields — keep the checked semantic
                // binder itself rather than collapsing to an inferred union.
                let members = params
                    .iter()
                    .enumerate()
                    .filter_map(|(index, parameter)| {
                        parameter.origin.as_ref().is_some_and(|origin| {
                            matches!(origin.as_slice(), [Expr { kind: ExprKind::Identifier(bound), .. }] if bound == name)
                        }).then_some(SigOrigin::Param(index))
                    })
                    .collect::<Vec<_>>();
                return Ok(match members.as_slice() {
                    [] => SigOrigin::Bound(crate::origin::Origin::Param(
                        crate::origin::OriginParamId(origin_param_index as u32),
                    )),
                    [single] => single.clone(),
                    _ => SigOrigin::union(members),
                });
            }
            Err(TypeError::UndefinedVariable(name.clone()))
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

pub(super) fn project_sig_origin(
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

pub(super) fn project_origin(
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

/// Replace the slot-relative parts belonging to source `Origin` parameters
/// with the concrete caller origins captured by a specialized function value.
pub(super) fn bind_sig_origin(
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

pub(super) fn sig_origin_has_bound(signature: &crate::origin::SigOrigin) -> bool {
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
fn field_carries_origin_param(ty: &Ty, id: crate::origin::OriginParamId) -> bool {
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
fn substitute_origin_params(
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

pub(super) fn substitute_sig_origin(
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

pub(super) fn substitute_sig_origin_with_self(
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

pub(super) fn origin_is_within(
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

pub(super) fn ref_parameter_is_writable(
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
pub(super) fn ref_binding_is_writable(
    convention: Option<ArgConvention>,
    origin: Option<&[Expr]>,
    type_params: &[crate::ast::TypeParam],
) -> bool {
    if convention != Some(ArgConvention::Ref) {
        return parameter_is_writable(convention);
    }
    let Some(
        [
            Expr {
                kind: ExprKind::Identifier(origin_name),
                ..
            },
        ],
    ) = origin
    else {
        return false;
    };
    if origin_name == "UnsafeAnyOrigin" {
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

/// Origin-signature validation and interior/aggregate-origin tracking moved from `checker.rs`.
impl Checker {
    pub(super) fn validate_origin_signature(
        &self,
        type_params: &[crate::ast::TypeParam],
        params: &[crate::ast::FnParam],
        self_origin: Option<&crate::ast::OriginSpec>,
    ) -> Result<(), TypeError> {
        let origin_params: HashSet<&str> = type_params
            .iter()
            .filter(|param| param.bounds.as_slice() == ["Origin"])
            .map(|param| param.name.as_str())
            .collect();
        let value_params: HashSet<&str> = params.iter().map(|param| param.name.as_str()).collect();
        let bool_params: HashSet<&str> = type_params
            .iter()
            .filter(|param| param.bounds.as_slice() == ["Bool"])
            .map(|param| param.name.as_str())
            .collect();

        for origin in type_params
            .iter()
            .filter(|param| param.bounds.as_slice() == ["Origin"])
        {
            if let Some(expr) = &origin.origin_mutability
                && !matches!(expr.kind, ExprKind::Bool(_))
                && !matches!(&expr.kind, ExprKind::Identifier(name) if bool_params.contains(name.as_str()))
            {
                return Err(TypeError::Unsupported(format!(
                    "origin mutability for '{}' must be Bool or a Bool parameter",
                    origin.name
                )));
            }
        }

        let validate = |spec: &crate::ast::OriginSpec| {
            for expr in spec {
                validate_origin_expr(expr, &origin_params, &value_params)?;
            }
            Ok::<(), TypeError>(())
        };
        if let Some(spec) = self_origin {
            validate(spec)?;
        }
        for param in params {
            if param.convention != Some(ArgConvention::Ref) && param.origin.is_some() {
                return Err(TypeError::Unsupported(format!(
                    "origin clause on non-ref parameter '{}'",
                    param.name
                )));
            }
            if let Some(spec) = &param.origin {
                validate(spec)?;
            }
        }
        Ok(())
    }

    /// Record that evaluating `expression` as a reference produces a fresh
    /// generation in the named interior region owned by `base`.
    pub(super) fn record_interior_reference(&self, site: SourceSpan, base: &Expr, name: &str) {
        if self
            .interior_references
            .borrow()
            .get(&site)
            .is_some_and(|origin| {
                matches!(origin.path.last(), Some(crate::origin::OriginSeg::Interior(tag)) if tag == name)
            })
        {
            // Inference is intentionally repeatable. Do not project the fact
            // through itself when the same checked expression is revisited.
            return;
        }
        if let Ok(mut origin) = self.origin_place(base) {
            origin
                .path
                .push(crate::origin::OriginSeg::Interior(name.to_string()));
            self.interior_references.borrow_mut().insert(site, origin);
        }
    }

    /// Record Mojo's owned-interior generation refresh for a named region.
    /// Defining a new `base._get_owned_interior["name"]` origin invalidates an
    /// older generation of that same region, but not sibling regions below the
    /// owner. Dict lookup uses this for `"value"`, so a new lookup stales an
    /// earlier value reference without invalidating the `"element"` generation
    /// retained by key iteration.
    pub(super) fn record_replacing_interior_reference(
        &self,
        site: SourceSpan,
        base: &Expr,
        name: &str,
    ) {
        if let Ok(mut origin) = self.origin_place(base) {
            origin
                .path
                .push(crate::origin::OriginSeg::Interior(name.to_string()));
            self.record_origin_invalidation_kind(site.clone(), origin, None, true);
        }
        self.record_interior_reference(site, base, name);
    }

    /// Record a mutation of `base`. Existing generations rooted below this
    /// path become stale. If `base` is itself a local reference, mutations
    /// through that handle preserve its own generation while still invalidating
    /// interiors nested underneath it.
    pub(super) fn record_interior_invalidation(&self, site: SourceSpan, base: &Expr) {
        let Ok(origin) = self.origin_place(base) else {
            return;
        };
        let except = match &base.kind {
            ExprKind::Identifier(name) if matches!(self.lookup(name), Some(Ty::Ref(_))) => {
                self.lookup_owner(name)
            }
            _ => None,
        };
        self.record_origin_invalidation(site, origin, except);
    }

    /// Record the storage generation replaced by a checked place write. Index
    /// and Variant-payload targets are places too: replacing one preserves a
    /// reference to that exact generation, but invalidates references into
    /// interiors nested below it. Two handle-bearing place forms need their
    /// semantic referent rather than their syntactic storage path:
    ///
    /// * `pointer[0]` replaces the origin-bearing pointer's proven source place;
    /// * assigning through a reference-valued aggregate field replaces the
    ///   place(s) whose handles the aggregate retains.
    pub(super) fn record_place_write_invalidation(&self, site: SourceSpan, place: &Expr) {
        if let ExprKind::Index { object, .. } = &place.kind
            && let Ok(Ty::Pointer {
                origin: crate::origin::PointerOrigin::Place { place: origin, .. },
                ..
            }) = self.infer(object)
        {
            self.record_origin_invalidation(site, origin, None);
            return;
        }

        if matches!(self.place_storage_ty(place), Some(Ty::Ref(_))) {
            // An `out self` initializer stores the incoming reference handle;
            // subsequent assignments write through that established handle.
            if self.self_initializing && place_root_name(place) == Some("self") {
                return;
            }

            let mut origins = self.aggregate_origins(place).into_iter().peekable();
            if origins.peek().is_some() {
                for origin in origins {
                    self.record_aggregate_origin_invalidation(site.clone(), origin);
                }
                return;
            }
        }

        self.record_interior_invalidation(site, place);
    }

    pub(super) fn record_aggregate_origin_invalidation(
        &self,
        site: SourceSpan,
        origin: crate::origin::Origin,
    ) {
        self.record_aggregate_origin_invalidation_except(site, origin, None);
    }

    pub(super) fn record_aggregate_origin_invalidation_except(
        &self,
        site: SourceSpan,
        origin: crate::origin::Origin,
        except: Option<crate::origin::OwnerId>,
    ) {
        match origin {
            crate::origin::Origin::Place(place) => {
                self.record_origin_invalidation(site, place, except);
            }
            crate::origin::Origin::Union(members) => {
                for member in members {
                    self.record_aggregate_origin_invalidation_except(site.clone(), member, except);
                }
            }
            crate::origin::Origin::Param(_)
            | crate::origin::Origin::SelfParam
            | crate::origin::Origin::Static
            | crate::origin::Origin::Untracked { .. } => {}
        }
    }

    pub(super) fn record_origin_invalidation(
        &self,
        site: SourceSpan,
        base: crate::origin::OriginPlace,
        except: Option<crate::origin::OwnerId>,
    ) {
        self.record_origin_invalidation_kind(site, base, except, false);
    }

    pub(super) fn record_origin_invalidation_kind(
        &self,
        site: SourceSpan,
        base: crate::origin::OriginPlace,
        except: Option<crate::origin::OwnerId>,
        include_base_generation: bool,
    ) {
        let fact = crate::checked::InteriorInvalidation {
            base,
            except,
            include_base_generation,
        };
        let mut invalidations = self.interior_invalidations.borrow_mut();
        let values = invalidations.entry(site).or_default();
        if !values.contains(&fact) {
            values.push(fact);
        }
    }

    pub(super) fn record_owner_invalidation(
        &self,
        site: SourceSpan,
        owner: crate::origin::OwnerId,
        path: Vec<crate::origin::OriginSeg>,
    ) {
        let fact = crate::checked::InteriorInvalidation {
            base: crate::origin::OriginPlace { root: owner, path },
            except: None,
            include_base_generation: false,
        };
        let mut invalidations = self.interior_invalidations.borrow_mut();
        let values = invalidations.entry(site).or_default();
        if !values.contains(&fact) {
            values.push(fact);
        }
    }

    pub(super) fn lookup_aggregate_origins(&self, name: &str) -> Vec<crate::origin::Origin> {
        let mut origins = self.lookup_scoped_aggregate_origins(name);
        // Merge origins a callee's store transferred into this binding.
        if let Some(owner) = self.lookup_owner(name)
            && let Some(transferred) = self.transferred_origins.borrow().get(&owner)
        {
            for origin in transferred {
                if !origins.contains(origin) {
                    origins.push(origin.clone());
                }
            }
        }
        origins
    }

    fn lookup_scoped_aggregate_origins(&self, name: &str) -> Vec<crate::origin::Origin> {
        self.aggregate_origin_scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
            .unwrap_or_default()
    }

    pub(super) fn set_aggregate_origins(
        &mut self,
        name: &str,
        origins: Vec<crate::origin::Origin>,
    ) {
        let Some(scope) = self.binding_scope(name) else {
            return;
        };
        if origins.is_empty() {
            self.aggregate_origin_scopes[scope].remove(name);
        } else {
            self.aggregate_origin_scopes[scope].insert(name.to_string(), origins);
        }
    }

    pub(super) fn lookup_aggregate_field_origins(
        &self,
        name: &str,
    ) -> HashMap<String, Vec<crate::origin::Origin>> {
        self.aggregate_field_origin_scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
            .unwrap_or_default()
    }

    pub(super) fn set_aggregate_field_origins(
        &mut self,
        name: &str,
        fields: HashMap<String, Vec<crate::origin::Origin>>,
    ) {
        let Some(scope) = self.binding_scope(name) else {
            return;
        };
        if fields.is_empty() {
            self.aggregate_field_origin_scopes[scope].remove(name);
        } else {
            self.aggregate_field_origin_scopes[scope].insert(name.to_string(), fields);
        }
    }

    /// Origins retained by each direct field of an aggregate value. The flat
    /// aggregate origin set remains useful for lifetime extension, but it
    /// cannot identify the referent of `pair.right` when `pair` also retains a
    /// distinct `left` origin.
    pub(super) fn aggregate_field_origins(
        &self,
        expression: &Expr,
    ) -> HashMap<String, Vec<crate::origin::Origin>> {
        fn append_unique(
            into: &mut Vec<crate::origin::Origin>,
            values: impl IntoIterator<Item = crate::origin::Origin>,
        ) {
            for value in values {
                if !into.contains(&value) {
                    into.push(value);
                }
            }
        }

        match &expression.kind {
            ExprKind::Identifier(name) => self.lookup_aggregate_field_origins(name),
            ExprKind::Transfer(inner) | ExprKind::Named { value: inner, .. } => {
                self.aggregate_field_origins(inner)
            }
            ExprKind::IfExpr {
                then_branch,
                else_branch,
                ..
            } => {
                let mut result = self.aggregate_field_origins(then_branch);
                for (field, origins) in self.aggregate_field_origins(else_branch) {
                    append_unique(result.entry(field).or_default(), origins);
                }
                result
            }
            ExprKind::Call {
                name, args, kwargs, ..
            } => {
                let Some(info) = self.structs.get(name) else {
                    return HashMap::new();
                };
                let fields = info.fields.clone();
                let mut result = HashMap::new();
                if info.fieldwise_init {
                    for ((field_name, field_ty), argument) in fields.into_iter().zip(args) {
                        let origins = if matches!(field_ty, Ty::Ref(_)) {
                            self.infer_reference_value(argument)
                                .map(|reference| vec![reference.origin])
                                .unwrap_or_default()
                        } else if self.type_may_carry_loans(&field_ty) {
                            self.aggregate_origins(argument)
                        } else {
                            Vec::new()
                        };
                        if !origins.is_empty() {
                            result.insert(field_name, origins);
                        }
                    }
                    return result;
                }

                // A conventional handwritten initializer commonly forwards a
                // same-named ref parameter into each reference field. Preserve
                // that field identity at the call site too; arbitrary computed
                // initializer data flow remains represented by the flat,
                // conservative aggregate origin set.
                let Some(signature) = info.methods.get("__init__").and_then(|signatures| {
                    signatures
                        .iter()
                        .find(|signature| signature.params.len() >= args.len())
                }) else {
                    return result;
                };
                for (field_name, field_ty) in fields {
                    if !matches!(field_ty, Ty::Ref(_)) {
                        continue;
                    }
                    let Some(index) = signature
                        .names
                        .iter()
                        .position(|parameter| parameter == &field_name)
                    else {
                        continue;
                    };
                    let argument = args.get(index).or_else(|| {
                        kwargs
                            .iter()
                            .find(|argument| argument.name == field_name)
                            .map(|argument| &argument.value)
                    });
                    if let Some(argument) = argument {
                        let origins = self
                            .reference_actual(argument)
                            .ok()
                            .map(|reference| vec![reference.origin])
                            .or_else(|| {
                                signature
                                    .ref_params
                                    .get(index)
                                    .is_some_and(Option::is_some)
                                    .then(|| {
                                        self.origin_place(argument)
                                            .ok()
                                            .map(crate::origin::Origin::Place)
                                            .into_iter()
                                            .collect::<Vec<_>>()
                                    })
                            })
                            .unwrap_or_default();
                        if !origins.is_empty() {
                            result.insert(field_name, origins);
                        }
                    }
                }
                result
            }
            _ => HashMap::new(),
        }
    }

    pub(super) fn register_reference_parameter(&mut self, name: &str, referent: Ty, mutable: bool) {
        let Some(scope) = self.binding_scope(name) else {
            return;
        };
        let Some(owner) = self.lookup_owner(name) else {
            return;
        };
        self.reference_parameter_scopes[scope].insert(
            name.to_string(),
            crate::origin::RefTy {
                referent: Box::new(referent),
                origin: crate::origin::Origin::Place(crate::origin::OriginPlace {
                    root: owner,
                    path: Vec::new(),
                }),
                mutability: if mutable {
                    crate::origin::Mutability::Mutable
                } else {
                    crate::origin::Mutability::Immutable
                },
            },
        );
    }

    pub(super) fn lookup_reference_parameter(&self, name: &str) -> Option<crate::origin::RefTy> {
        self.reference_parameter_scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
    }

    pub(super) fn type_contains_reference(&self, ty: &Ty) -> bool {
        self.type_storage_contains(ty, false)
    }

    /// Whether storage of this type carries owner loans: reference-valued
    /// leaves, or pointers whose provenance designates checked storage.
    pub(super) fn type_carries_loans(&self, ty: &Ty) -> bool {
        self.type_storage_contains(ty, true)
    }

    /// Whether a type MAY carry loans once populated: it carries loans
    /// already, or it contains a capturing callable whose declared
    /// environment has not yet been narrowed to a concrete capture set
    /// (`capturing[_]` / `capturing[origins]` storage). Origin bookkeeping
    /// for such storage must record the populating value's concrete capture
    /// origins even though the declared type alone is loan-blind.
    pub(super) fn type_may_carry_loans(&self, ty: &Ty) -> bool {
        fn contains_open_captures(ty: &Ty) -> bool {
            use crate::origin::{CallableEnvironment, CaptureOriginSet};
            match ty {
                // An abstract type parameter may be instantiated with a
                // loan-carrying type: origin bookkeeping stays conservative
                // so generic bodies record transfer effects.
                Ty::Param { .. } => true,
                Ty::Func { environment, .. } => matches!(
                    environment,
                    CallableEnvironment::Capturing(
                        CaptureOriginSet::Infer | CaptureOriginSet::Param(_)
                    )
                ),
                Ty::Struct(_, arguments) => arguments.iter().any(|argument| match argument {
                    crate::types::TyArg::Ty(ty) => contains_open_captures(ty),
                    _ => false,
                }),
                Ty::Ref(reference) => contains_open_captures(&reference.referent),
                Ty::Tuple(elements) => elements.iter().any(contains_open_captures),
                _ => false,
            }
        }
        self.type_carries_loans(ty) || contains_open_captures(ty) || {
            // A struct's declared fields may hold open-capture callables.
            match ty {
                Ty::Struct(name, _) => self.structs.get(name).is_some_and(|info| {
                    info.fields
                        .iter()
                        .any(|(_, field)| contains_open_captures(field))
                }),
                _ => false,
            }
        }
    }

    pub(super) fn capture_origins_in_type(&self, ty: &Ty) -> Vec<crate::origin::CaptureOrigin> {
        use crate::origin::{CallableEnvironment, CaptureOrigin, CaptureOriginSet};

        fn collect(checker: &Checker, ty: &Ty, out: &mut Vec<CaptureOrigin>) {
            if let Some(element) = list_element(ty).or_else(|| set_element(ty)) {
                collect(checker, element, out);
                return;
            }
            if let Some((key, value)) = dict_elements(ty) {
                collect(checker, key, out);
                collect(checker, value, out);
                return;
            }
            if let Some(elements) = tuple_elements(ty) {
                for element in elements {
                    collect(checker, element, out);
                }
                return;
            }
            match ty {
                Ty::Ref(reference) => out.push(CaptureOrigin::read(reference.origin.clone())),
                Ty::Pointer { element, origin } => {
                    if let Some(origin) = origin.as_origin() {
                        out.push(CaptureOrigin::read(origin));
                    }
                    collect(checker, element, out);
                }
                Ty::ComptimeList(element) => collect(checker, element, out),
                Ty::Tuple(elements) | Ty::RuntimePack(elements) | Ty::Variant(elements) => {
                    for element in elements {
                        collect(checker, element, out);
                    }
                }
                Ty::Struct(name, arguments) => {
                    let Some(info) = checker.structs.get(name) else {
                        return;
                    };
                    let subst = struct_subst(&info.decls, arguments);
                    for (_, field) in &info.fields {
                        collect(checker, &substitute(field, &subst), out);
                    }
                }
                Ty::Func {
                    environment:
                        CallableEnvironment::Capturing(CaptureOriginSet::Concrete(captures)),
                    ..
                }
                | Ty::GenericFunc {
                    environment:
                        CallableEnvironment::Capturing(CaptureOriginSet::Concrete(captures)),
                    ..
                } => out.extend(captures.iter().cloned()),
                _ => {}
            }
        }

        let mut origins = Vec::new();
        collect(self, ty, &mut origins);
        let CaptureOriginSet::Concrete(origins) = CaptureOriginSet::concrete(origins) else {
            unreachable!("concrete capture canonicalization stays concrete")
        };
        origins
    }

    pub(super) fn checked_capture(
        &self,
        name: &str,
        binding: crate::origin::OwnerId,
        ty: Ty,
        kind: crate::ast::CaptureKind,
    ) -> crate::checked::CheckedCapture {
        use crate::origin::{CaptureAccess, CaptureOrigin, Origin, OriginPlace};
        let mut origins = self.capture_origins_in_type(&ty);
        match kind {
            crate::ast::CaptureKind::Read => origins.push(CaptureOrigin {
                origin: Origin::Place(OriginPlace {
                    root: binding,
                    path: Vec::new(),
                }),
                access: CaptureAccess::Read,
            }),
            crate::ast::CaptureKind::Mut | crate::ast::CaptureKind::Ref => {
                origins.push(CaptureOrigin {
                    origin: Origin::Place(OriginPlace {
                        root: binding,
                        path: Vec::new(),
                    }),
                    access: CaptureAccess::Write,
                })
            }
            crate::ast::CaptureKind::Copy | crate::ast::CaptureKind::Move => {}
        }
        let crate::origin::CaptureOriginSet::Concrete(origins) =
            crate::origin::CaptureOriginSet::concrete(origins)
        else {
            unreachable!("concrete capture canonicalization stays concrete")
        };
        crate::checked::CheckedCapture {
            name: name.to_string(),
            binding,
            ty,
            kind,
            origins,
        }
    }

    pub(super) fn type_storage_contains(&self, ty: &Ty, pointer_loans: bool) -> bool {
        fn contains(
            checker: &Checker,
            ty: &Ty,
            pointer_loans: bool,
            seen: &mut HashSet<String>,
        ) -> bool {
            if let Some(element) = list_element(ty).or_else(|| set_element(ty)) {
                return contains(checker, element, pointer_loans, seen);
            }
            if let Some((key, value)) = dict_elements(ty) {
                return contains(checker, key, pointer_loans, seen)
                    || contains(checker, value, pointer_loans, seen);
            }
            if let Some(elements) = tuple_elements(ty) {
                return elements
                    .into_iter()
                    .any(|element| contains(checker, element, pointer_loans, seen));
            }
            match ty {
                Ty::Ref(_) => true,
                Ty::Pointer { element, origin } => {
                    (pointer_loans && origin.as_origin().is_some())
                        || contains(checker, element, pointer_loans, seen)
                }
                Ty::ComptimeList(element) => contains(checker, element, pointer_loans, seen),
                Ty::Tuple(elements) | Ty::RuntimePack(elements) => elements
                    .iter()
                    .any(|element| contains(checker, element, pointer_loans, seen)),
                Ty::Variant(alternatives) => alternatives
                    .iter()
                    .any(|alternative| contains(checker, alternative, pointer_loans, seen)),
                Ty::Struct(name, args) => {
                    let key = ty.to_string();
                    if !seen.insert(key.clone()) {
                        return false;
                    }
                    let result = checker.structs.get(name).is_some_and(|info| {
                        let subst = struct_subst(&info.decls, args);
                        info.fields
                            .iter()
                            .map(|(_, field)| substitute(field, &subst))
                            .any(|field| contains(checker, &field, pointer_loans, seen))
                    });
                    seen.remove(&key);
                    result
                }
                Ty::Func {
                    environment:
                        crate::origin::CallableEnvironment::Capturing(
                            crate::origin::CaptureOriginSet::Concrete(captures),
                        ),
                    ..
                }
                | Ty::GenericFunc {
                    environment:
                        crate::origin::CallableEnvironment::Capturing(
                            crate::origin::CaptureOriginSet::Concrete(captures),
                        ),
                    ..
                } => captures.iter().any(|capture| {
                    matches!(
                        capture.origin,
                        crate::origin::Origin::Place(_) | crate::origin::Origin::Param(_)
                    )
                }),
                _ => false,
            }
        }
        contains(self, ty, pointer_loans, &mut HashSet::new())
    }

    pub(super) fn type_contains_unsafe_any_pointer(ty: &Ty) -> bool {
        if let Some(element) = list_element(ty).or_else(|| set_element(ty)) {
            return Self::type_contains_unsafe_any_pointer(element);
        }
        if let Some((key, value)) = dict_elements(ty) {
            return Self::type_contains_unsafe_any_pointer(key)
                || Self::type_contains_unsafe_any_pointer(value);
        }
        if let Some(elements) = tuple_elements(ty) {
            return elements
                .into_iter()
                .any(Self::type_contains_unsafe_any_pointer);
        }
        match ty {
            Ty::Pointer {
                origin: crate::origin::PointerOrigin::UnsafeAny { .. },
                ..
            } => true,
            Ty::Pointer { element, .. } | Ty::ComptimeList(element) => {
                Self::type_contains_unsafe_any_pointer(element)
            }
            Ty::Tuple(elements) | Ty::RuntimePack(elements) | Ty::Variant(elements) => {
                elements.iter().any(Self::type_contains_unsafe_any_pointer)
            }
            _ => false,
        }
    }

    /// Origins retained by a value expression. This follows only value flow;
    /// ordinary arithmetic and reads cannot invent a stored reference handle.
    pub(super) fn aggregate_origins(&self, expression: &Expr) -> Vec<crate::origin::Origin> {
        use crate::origin::Origin;

        fn append_unique(into: &mut Vec<Origin>, values: impl IntoIterator<Item = Origin>) {
            for value in values {
                if !into.contains(&value) {
                    into.push(value);
                }
            }
        }

        match &expression.kind {
            ExprKind::Identifier(name) => {
                let aggregate = self.lookup_aggregate_origins(name);
                if !aggregate.is_empty() {
                    return aggregate;
                }
                match self.lookup(name) {
                    Some(Ty::Ref(reference)) => vec![reference.origin.clone()],
                    Some(Ty::Pointer { origin, .. }) => origin
                        .as_origin()
                        .map(|origin| vec![origin])
                        .unwrap_or_default(),
                    Some(ty @ (Ty::Func { .. } | Ty::GenericFunc { .. })) => self
                        .capture_origins_in_type(ty)
                        .into_iter()
                        .map(|capture| capture.origin)
                        .collect(),
                    _ => self
                        .lookup_reference_parameter(name)
                        .map(|reference| vec![reference.origin])
                        .unwrap_or_default(),
                }
            }
            ExprKind::Member { object, field } => {
                if let Some(origins) = self.aggregate_field_origins(object).get(field) {
                    return origins.clone();
                }
                let aggregate = self.aggregate_origins(object);
                if !aggregate.is_empty() {
                    aggregate
                } else {
                    self.infer_reference_value(expression)
                        .map(|reference| vec![reference.origin])
                        .unwrap_or_default()
                }
            }
            ExprKind::Transfer(inner) | ExprKind::Named { value: inner, .. } => {
                self.aggregate_origins(inner)
            }
            ExprKind::ListLit(values) | ExprKind::TupleLit(values) => {
                let mut result = Vec::new();
                for value in values {
                    append_unique(&mut result, self.aggregate_origins(value));
                }
                result
            }
            ExprKind::IfExpr {
                then_branch,
                else_branch,
                ..
            } => {
                let mut result = self.aggregate_origins(then_branch);
                append_unique(&mut result, self.aggregate_origins(else_branch));
                result
            }
            ExprKind::Call {
                name, args, kwargs, ..
            } => {
                // A checked pointer construction retains exactly its source
                // place; the checker recorded the decision when it typed the
                // call, so shadowed `UnsafePointer` names cannot match here.
                if self
                    .operation_adjustments
                    .borrow()
                    .get(&expression.source_span())
                    .is_some_and(|operation| {
                        matches!(
                            operation,
                            crate::checked::SemanticAdjustment::PointerToPlace { .. }
                        )
                    })
                    && let Some(argument) = kwargs.first()
                    && let Ok(place) = self.origin_place(&argument.value)
                {
                    return vec![Origin::Place(place)];
                }
                let mut result = Vec::new();
                if let Some(info) = self.structs.get(name) {
                    if info.fieldwise_init {
                        let fields: Vec<Ty> =
                            info.fields.iter().map(|(_, ty)| ty.clone()).collect();
                        for (field, argument) in fields.iter().zip(args) {
                            if matches!(field, Ty::Ref(_)) {
                                if let Ok(reference) = self.reference_actual(argument) {
                                    append_unique(&mut result, [reference.origin]);
                                }
                            } else {
                                append_unique(&mut result, self.aggregate_origins(argument));
                            }
                        }
                    } else if let Some(signature) =
                        info.methods.get("__init__").and_then(|signatures| {
                            signatures.iter().find(|sig| sig.params.len() == args.len())
                        })
                    {
                        let refs = signature.ref_params.clone();
                        for (index, argument) in args.iter().enumerate() {
                            if refs.get(index).is_some_and(Option::is_some) {
                                if let Ok(reference) = self.reference_actual(argument) {
                                    append_unique(&mut result, [reference.origin]);
                                }
                            } else {
                                append_unique(&mut result, self.aggregate_origins(argument));
                            }
                        }
                    }
                }
                if result.is_empty() {
                    for argument in args {
                        append_unique(&mut result, self.aggregate_origins(argument));
                    }
                    for argument in kwargs {
                        append_unique(&mut result, self.aggregate_origins(&argument.value));
                    }
                }
                result
            }
            ExprKind::Invoke { args, kwargs, .. } | ExprKind::MethodCall { args, kwargs, .. } => {
                let mut result = Vec::new();
                for argument in args {
                    append_unique(&mut result, self.aggregate_origins(argument));
                }
                for argument in kwargs {
                    append_unique(&mut result, self.aggregate_origins(&argument.value));
                }
                result
            }
            _ => Vec::new(),
        }
    }
}
