//! Transfer effects: applying/baking/recording value-transfer and
//! call-through effects and replaying them at call sites.

use super::*;

impl Checker {
    /// Replay a callee's transfer effects against a call's actuals: enforce
    /// the store-outward rule across the boundary, merge the transferred
    /// origins into the destination binding's bookkeeping, derive the
    /// enclosing callable's transitive effect, and record the substituted
    /// transfer for MIR loan installation.
    pub(in crate::checker) fn apply_transfer_effects(
        &self,
        callee: &str,
        receiver: Option<&Expr>,
        args: &[Expr],
        span: &SourceSpan,
    ) -> Result<(), TypeError> {
        let effects = self.transfer_effects.borrow().get(callee).cloned();
        // First-seen observation (including "none"): the two-phase pass
        // reruns the check when this callee's final effects differ from what
        // the stalest query here saw.
        self.effect_observations
            .borrow_mut()
            .entry(callee.to_string())
            .or_insert_with(|| effects.clone().unwrap_or_default());
        let Some(effects) = effects else {
            return Ok(());
        };
        self.replay_transfer_effects(&effects, receiver, args, span)
    }

    /// A def name in value position carries the callable's committed
    /// transfer effects on the produced function type, so a later call
    /// through the VALUE replays them without name resolution. The
    /// first-seen observation keeps fixpoint staleness exact: when the
    /// name-keyed entry grows after this reference, the program re-checks
    /// and the rebake sees the full set. A shadowing local that happens to
    /// collide with an effectful declaration name over-approximates, the
    /// same union the name-keyed call path already applies.
    pub(in crate::checker) fn bake_value_transfer_effects(&self, name: &str, ty: Ty) -> Ty {
        let bakeable = match &ty {
            Ty::Func { .. } | Ty::GenericFunc { .. } => true,
            Ty::Overload(members) => members
                .iter()
                .any(|member| matches!(member, Ty::Func { .. } | Ty::GenericFunc { .. })),
            _ => false,
        };
        if !bakeable {
            return ty;
        }
        let effects = self.transfer_effects.borrow().get(name).cloned();
        self.effect_observations
            .borrow_mut()
            .entry(name.to_string())
            .or_insert_with(|| effects.clone().unwrap_or_default());
        let Some(effects) = effects else {
            return ty;
        };
        match ty {
            // Overloads share the bare-name effect entry; the union applies
            // to whichever member a later call selects.
            Ty::Overload(members) => Ty::Overload(
                members
                    .into_iter()
                    .map(|member| with_transfer_effects(member, &effects))
                    .collect(),
            ),
            other => with_transfer_effects(other, &effects),
        }
    }

    /// Record a higher-order residue when a body calls through its own
    /// callable parameter (runtime `def(...)` param or compile-time callable
    /// value param): the callable's effects are unknowable here, so store
    /// the signature abstraction of every actual and let each caller —
    /// which knows the concrete callable — resolve it.
    pub(in crate::checker) fn record_call_through(
        &self,
        callee_name: &str,
        callee_ty: &Ty,
        args: &[Expr],
    ) {
        use crate::checked::{CallThroughCallee, CallThroughEffect};
        if callable_contract_ty(callee_ty).is_none() {
            return;
        }
        let Some((callee, param_owners, self_owner)) = ({
            let frames = self.transfer_frames.borrow();
            frames.last().and_then(|frame| {
                let identity = if frame
                    .value_callables
                    .iter()
                    .any(|candidate| candidate == callee_name)
                {
                    Some(CallThroughCallee::ValueParam(callee_name.to_string()))
                } else {
                    self.lookup_owner(callee_name)
                        .and_then(|owner| {
                            frame
                                .param_owners
                                .iter()
                                .position(|candidate| *candidate == owner)
                        })
                        .map(CallThroughCallee::RuntimeParam)
                };
                identity.map(|callee| (callee, frame.param_owners.clone(), frame.self_owner))
            })
        }) else {
            return;
        };
        let args = args
            .iter()
            .map(|arg| self.call_through_arg(arg, &param_owners, self_owner))
            .collect();
        let effect = CallThroughEffect { callee, args };
        let mut frames = self.transfer_frames.borrow_mut();
        if let Some(frame) = frames.last_mut()
            && !frame.call_throughs.contains(&effect)
        {
            frame.call_throughs.push(effect);
        }
    }

    /// Abstract one inner-call actual to the enclosing signature: its own
    /// place, the origins it carries, and whether anything roots at
    /// frame-local storage.
    pub(super) fn call_through_arg(
        &self,
        arg: &Expr,
        param_owners: &[crate::origin::OwnerId],
        self_owner: Option<crate::origin::OwnerId>,
    ) -> crate::checked::CallThroughArg {
        use crate::origin::Origin;
        let mut out = crate::checked::CallThroughArg::default();
        if let Ok(place) = self.origin_place(arg) {
            let origin = Origin::Place(place);
            match self.abstract_body_origin(&origin, param_owners, self_owner) {
                Some(sig) => out.place = Some(sig),
                None => out.local = true,
            }
        }
        let mut carried = self.aggregate_origins(arg);
        if let Some(reference) = self.infer_reference_value(arg)
            && !carried.contains(&reference.origin)
        {
            carried.push(reference.origin);
        }
        for origin in carried {
            if matches!(origin, Origin::Static | Origin::Untracked { .. }) {
                continue;
            }
            match self.abstract_body_origin(&origin, param_owners, self_owner) {
                Some(sig) => {
                    if !out.carried.contains(&sig) {
                        out.carried.push(sig);
                    }
                }
                None => out.local = true,
            }
        }
        out
    }

    /// Resolve a callee's recorded call-through residues against THIS call's
    /// concrete callable actuals: translate the concrete callable's effects
    /// into effects of the callee and replay them, or — when the callable is
    /// itself a callable parameter of the current frame — derive a composed
    /// residue on the current frame.
    pub(in crate::checker) fn apply_call_through_effects(
        &self,
        callee: &str,
        callee_decls: &[crate::types::ParamDecl],
        receiver: Option<&Expr>,
        param_args: &[crate::ast::ParamArg],
        args: &[Expr],
        span: &SourceSpan,
    ) -> Result<(), TypeError> {
        use crate::checked::CallThroughCallee;
        let throughs = self.call_through_effects.borrow().get(callee).cloned();
        self.call_through_observations
            .borrow_mut()
            .entry(callee.to_string())
            .or_insert_with(|| throughs.clone().unwrap_or_default());
        let Some(throughs) = throughs else {
            return Ok(());
        };
        for through in &throughs {
            let concrete = match &through.callee {
                CallThroughCallee::RuntimeParam(index) => {
                    args.get(*index).and_then(|arg| match &arg.kind {
                        ExprKind::Identifier(name) => Some(name.clone()),
                        _ => None,
                    })
                }
                CallThroughCallee::ValueParam(decl_name) => {
                    callable_value_argument(callee_decls, decl_name, param_args)
                }
            };
            let Some(concrete) = concrete else {
                // An unnamed or defaulted callable actual stays permissive;
                // the observation above keeps convergence exact.
                continue;
            };
            // The supplied callable is itself a callable parameter of the
            // CURRENT frame: derive a composed residue instead of replaying.
            if let Some(identity) = self.frame_callable_identity(&concrete) {
                let composed = crate::checked::CallThroughEffect {
                    callee: identity,
                    args: through
                        .args
                        .iter()
                        .map(|arg| self.compose_call_through_arg(arg, receiver, args))
                        .collect(),
                };
                let mut frames = self.transfer_frames.borrow_mut();
                if let Some(frame) = frames.last_mut()
                    && !frame.call_throughs.contains(&composed)
                {
                    frame.call_throughs.push(composed);
                }
                continue;
            }
            let effects = self.named_callable_effects(&concrete);
            if effects.is_empty() {
                continue;
            }
            let translated = translate_call_through(&effects, through)?;
            if !translated.is_empty() {
                self.replay_transfer_effects(&translated, receiver, args, span)?;
            }
        }
        Ok(())
    }

    /// The current frame's callable-parameter identity of a name, when the
    /// name denotes one (the composition trigger).
    pub(super) fn frame_callable_identity(
        &self,
        name: &str,
    ) -> Option<crate::checked::CallThroughCallee> {
        use crate::checked::CallThroughCallee;
        let frames = self.transfer_frames.borrow();
        let frame = frames.last()?;
        if frame
            .value_callables
            .iter()
            .any(|candidate| candidate == name)
        {
            return Some(CallThroughCallee::ValueParam(name.to_string()));
        }
        self.lookup_owner(name)
            .and_then(|owner| {
                frame
                    .param_owners
                    .iter()
                    .position(|candidate| *candidate == owner)
            })
            .map(CallThroughCallee::RuntimeParam)
    }

    /// Committed transfer effects of a named concrete callable: the
    /// name-keyed entry for a `def`, or the `Struct.__call__` entry for a
    /// callable-struct binding (whose `Self_` terms the translation maps to
    /// the callable slot itself). Records the fixpoint observation.
    pub(super) fn named_callable_effects(&self, name: &str) -> Vec<crate::checked::TransferEffect> {
        let key = match self.lookup(name) {
            Some(Ty::Struct(struct_name, _)) => format!("{struct_name}.__call__"),
            _ => name.to_string(),
        };
        let effects = self.transfer_effects.borrow().get(&key).cloned();
        self.effect_observations
            .borrow_mut()
            .entry(key)
            .or_insert_with(|| effects.clone().unwrap_or_default());
        effects.unwrap_or_default()
    }

    /// Map a recorded call-through argument (in the CALLEE's signature
    /// terms) through this call's actuals into the CURRENT frame's signature
    /// terms, for a composed residue. Union-conservative: a mapped actual's
    /// own place joins the carried set.
    pub(super) fn compose_call_through_arg(
        &self,
        arg: &crate::checked::CallThroughArg,
        receiver: Option<&Expr>,
        args: &[Expr],
    ) -> crate::checked::CallThroughArg {
        use crate::origin::SigOrigin;
        let (param_owners, self_owner) = {
            let frames = self.transfer_frames.borrow();
            match frames.last() {
                Some(frame) => (frame.param_owners.clone(), frame.self_owner),
                None => (Vec::new(), None),
            }
        };
        let actual_for = |sig: &SigOrigin| -> Option<&Expr> {
            match sig {
                SigOrigin::Self_ => receiver,
                SigOrigin::Param(index) => args.get(*index),
                _ => None,
            }
        };
        let mut out = crate::checked::CallThroughArg {
            place: None,
            carried: Vec::new(),
            local: arg.local,
        };
        if let Some(sig) = &arg.place {
            match actual_for(sig) {
                Some(actual) => {
                    let mapped = self.call_through_arg(actual, &param_owners, self_owner);
                    out.place = mapped.place;
                    out.local |= mapped.local;
                    for sig in mapped.carried {
                        if !out.carried.contains(&sig) {
                            out.carried.push(sig);
                        }
                    }
                }
                None => out.local = true,
            }
        }
        for sig in &arg.carried {
            match actual_for(sig) {
                Some(actual) => {
                    let mapped = self.call_through_arg(actual, &param_owners, self_owner);
                    out.local |= mapped.local;
                    for sig in mapped.place.into_iter().chain(mapped.carried) {
                        if !out.carried.contains(&sig) {
                            out.carried.push(sig);
                        }
                    }
                }
                None => out.local = true,
            }
        }
        out
    }

    /// Replay already-resolved callee effects against the call's actuals.
    /// Effects arriving here came from the name-keyed map (with its
    /// observation recorded by the caller) or from a function-typed value
    /// (observed when the def name was baked into the type).
    pub(in crate::checker) fn replay_transfer_effects(
        &self,
        effects: &[crate::checked::TransferEffect],
        receiver: Option<&Expr>,
        args: &[Expr],
        span: &SourceSpan,
    ) -> Result<(), TypeError> {
        use crate::checked::{CheckedCallTransfer, CheckedTransferDest};
        use crate::origin::SigOrigin;
        let mut call_transfers = Vec::new();
        for effect in effects {
            let actual = |origin: &SigOrigin| match origin {
                SigOrigin::Self_ => receiver,
                SigOrigin::Param(index) => args.get(*index),
                _ => None,
            };
            // A `Bound` destination is a concrete captured owner: it needs
            // no actual expression, and it always outlives the callee frame
            // (it lives in an ancestor of the closure's declaration).
            let (dest_root_owner, dest_path) = match &effect.dest {
                SigOrigin::Bound(crate::origin::Origin::Place(place)) => {
                    (Some(place.root), place.path.clone())
                }
                dest => {
                    let (base, effect_path) = match dest {
                        SigOrigin::Projected(base, path) => (base.as_ref(), path.as_slice()),
                        other => (other, [].as_slice()),
                    };
                    let Some(expr) = actual(base) else {
                        continue;
                    };
                    // Compose the actual's own interior path with the
                    // effect's: `feed(t.a, ...)` storing into its param's
                    // `.items` lands at `t.a.items`.
                    let mut path = self
                        .origin_place(expr)
                        .map(|place| place.path)
                        .unwrap_or_default();
                    path.extend(effect_path.iter().cloned());
                    (
                        place_root_name(expr).and_then(|root| self.lookup_owner(root)),
                        path,
                    )
                }
            };
            // A `Bound` source is already a concrete caller-side origin.
            let mut sources = match &effect.src {
                SigOrigin::Bound(origin) => vec![origin.clone()],
                src => {
                    let Some(src_expr) = actual(src) else {
                        continue;
                    };
                    // The caller-side origins of the source ACTUAL EXPRESSION
                    // — this covers moved temporaries (`RefBox(alias)`),
                    // whose loans root at the places their construction
                    // borrowed.
                    let mut sources = self.aggregate_origins(src_expr);
                    // A reference-valued source contributes its referent's
                    // origin; a plain moved value transfers ownership and
                    // adds no loan of its own storage.
                    if let Some(reference) = self.infer_reference_value(src_expr)
                        && !sources.contains(&reference.origin)
                    {
                        sources.push(reference.origin);
                    }
                    // A borrowed-parameter source loans the actual's own
                    // storage.
                    if effect.src_is_place
                        && let Ok(place) = self.origin_place(src_expr)
                    {
                        let origin = crate::origin::Origin::Place(place);
                        if !sources.contains(&origin) {
                            sources.push(origin);
                        }
                    }
                    sources
                }
            };
            sources.retain(|origin| {
                !matches!(
                    origin,
                    crate::origin::Origin::Static | crate::origin::Origin::Untracked { .. }
                )
            });
            if sources.is_empty() {
                continue;
            }
            // The store-outward rule, across the call: a destination rooted
            // at an outliving owner must not receive an escaping loan. A
            // `Bound` destination participates through its concrete owner —
            // outliving when the invocation frame's context says so, an
            // ordinary local store when invoked in the owning frame itself.
            let dest_outlives = matches!(
                (self.aggregate_escape_contexts.last(), dest_root_owner),
                (Some((_, allowed)), Some(owner)) if allowed.contains(&owner)
            );
            if dest_outlives
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
                // A destination owned by an ANCESTOR frame — captured
                // enclosing storage — derives a concrete `Bound` effect
                // (verbatim for an already-`Bound` dest); the frame that
                // owns the storage re-abstracts it above.
                let derived_dest = dest_sig
                    .map(|base| {
                        if dest_path.is_empty() {
                            base
                        } else {
                            SigOrigin::Projected(Box::new(base), dest_path.clone())
                        }
                    })
                    .or_else(|| {
                        self.owner_in_enclosing_scope(owner).then(|| {
                            SigOrigin::Bound(crate::origin::Origin::Place(
                                crate::origin::OriginPlace {
                                    root: owner,
                                    path: dest_path.clone(),
                                },
                            ))
                        })
                    });
                if let Some(dest_sig) = derived_dest {
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
            let dest_base = match &effect.dest {
                SigOrigin::Projected(base, _) => base.as_ref(),
                dest => dest,
            };
            let dest = match dest_base {
                SigOrigin::Self_ => CheckedTransferDest::Receiver,
                SigOrigin::Param(index) => CheckedTransferDest::Argument(*index),
                SigOrigin::Bound(crate::origin::Origin::Place(place)) => {
                    CheckedTransferDest::Owner(place.root)
                }
                _ => continue,
            };
            call_transfers.push(CheckedCallTransfer {
                dest,
                dest_path,
                sources,
                mutable: effect.mutable,
            });
        }
        if !call_transfers.is_empty() {
            // Extend rather than insert: a call site may replay both the
            // name-keyed entry and the value-carried set; recorded transfers
            // union across the channels (deduplicated).
            let mut recorded = self.call_transfers.borrow_mut();
            let entry = recorded.entry(span.clone()).or_default();
            for transfer in call_transfers {
                if !entry.contains(&transfer) {
                    entry.push(transfer);
                }
            }
        }
        Ok(())
    }

    /// Record an accepted outliving store as a transfer effect on the
    /// enclosing callable's accumulation frame. Constructor bodies record
    /// nothing (their arguments are caller-visible; the local aggregate path
    /// installs those loans), and self-to-self transfers are skipped so
    /// internal reshuffles do not self-loan every call.
    pub(in crate::checker) fn record_transfer_effect(
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
        // The interior path below the destination root survives on the
        // effect, so call sites can install domain-precise loans.
        let dest_path = self
            .origin_place(place)
            .map(|destination| destination.path)
            .unwrap_or_default();
        let dest = if Some(owner) == frame.self_owner {
            SigOrigin::Self_
        } else if let Some(index) = frame
            .param_owners
            .iter()
            .position(|candidate| *candidate == owner)
        {
            SigOrigin::Param(index)
        } else if self.owner_in_enclosing_scope(owner) {
            // A store through a CAPTURED enclosing owner (`self`/parameters
            // reached from a nested def) is not expressible relative to this
            // frame's signature. Owner ids are checker-global, so record the
            // concrete owner; invocation sites ground it directly and
            // enclosing frames propagate it verbatim until the frame that
            // owns the storage re-abstracts it.
            SigOrigin::Bound(crate::origin::Origin::Place(crate::origin::OriginPlace {
                root: owner,
                path: dest_path.clone(),
            }))
        } else {
            // The remaining accepted-outward owners (e.g. the callee's own
            // variadic collector) are callee-owned storage: no caller-side
            // loan is implied.
            return;
        };
        let dest = match dest {
            bound @ SigOrigin::Bound(_) => bound,
            base if dest_path.is_empty() => base,
            base => SigOrigin::Projected(Box::new(base), dest_path),
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
}
