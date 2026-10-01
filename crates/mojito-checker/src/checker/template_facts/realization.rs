//! Instance derivation: the substitution a clone's arguments induce and the
//! template's facts realized under it, obligation by obligation.

use super::realization_folds::{
    closed_pack_values, construct_folded_vectors, constructed_element_indices, fold_binder_views,
    fold_vector_values, folded_literals, loop_element_indices, merge_element_constructions,
    realize_lane_comparisons, realize_lane_float_methods, realize_simd_intrinsics,
    realize_value_shaped_constructions, relocate_packs, simd_binder_values, spread_packs,
    transferred_element_indices,
};
use super::{
    BodyDeclaration, BodyParams, BodyRole, BodySite, DerivedBody, ElementIndices,
    InstanceSubstitution, Occurrence, VectorFold, bound_binder, callable_binder,
    canonical_hash_leaves, clone_origin_binder, element_construction_part, fact_at,
    nested_def_calls, note_realized_callee, origin_binder, push_unique, sig_origin_members,
    sorted_applications, without_struct_origins,
};
use crate::checker::Checker;
use mojito_ast::ast::{CaptureKind, Stmt, StmtKind};
use mojito_checked::templates::{
    CheckedBodyFacts, CheckedTemplate, InstanceTrace, MethodFeatures, OccurrenceId,
    PackElementNode, TemplateAugmentedSubscript, TemplateCallContract, TemplateCallTransfer,
    TemplateClass, TemplateCoverage, TemplateEffectSource, TemplateOrigin, TemplateOwner,
    TemplateReference, TemplateTransferEffect, TemplateTransferSource,
};
use mojito_common::error::TypeError;
use mojito_common::timing;
use mojito_common::token::SyntaxId;
use mojito_types::origin::OwnerId;
use mojito_types::types::{ParamDecl, Ty, TySubst};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

impl Checker {
    /// The type of the binding an owner identifies, from the scope that
    /// registered it.
    pub(super) fn owner_binding_type(&self, owner: OwnerId) -> Option<Ty> {
        self.owner_scopes
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, scope)| {
                scope
                    .iter()
                    .find(|(_, candidate)| **candidate == owner)
                    .and_then(|(name, _)| self.scopes.get(index)?.get(name).cloned())
            })
    }

    /// The facts a body may take instead of being inferred, realized for
    /// it, with the body's occurrence spans by the identity each kept from
    /// the template: a clone the elaborator traced to a certified template,
    /// or (`template`) a certified template's own body in a later pass,
    /// under the identity substitution. `None` infers the body; for a traced
    /// clone the reason is counted.
    pub(super) fn derivable_facts(
        &self,
        site: &BodySite<'_>,
        param_owners: &BodyParams,
    ) -> Option<DerivedBody> {
        let name = &site.display;
        let template = site.role == BodyRole::Template;
        let trace = if template {
            InstanceTrace {
                template: site.template_id.clone(),
                type_bindings: Vec::new(),
                value_bindings: Vec::new(),
                pack_bindings: Vec::new(),
                residual: Vec::new(),
                first_copy_template: false,
            }
        } else {
            self.instance_trace(site)?
        };
        let refusals = RefCell::new(Vec::new());
        let refuse = |reason: &'static str| {
            if template {
                timing::count("template_bodies.reuse_ineligible", 1);
                timing::note("template_bodies.reuse_ineligible", || {
                    format!("{name}: {reason}")
                });
            } else {
                timing::count("template_derivations.ineligible", 1);
                timing::note("template_derivations.ineligible", || {
                    format!("{name}: {reason}")
                });
                refusals
                    .borrow_mut()
                    .push((name.clone(), reason.to_string()));
            }
            None
        };
        let elements = if template {
            Ok(None)
        } else {
            self.element_construction_facts(site, &trace)
        };
        let derived = match elements {
            Ok(elements) => self.derive(
                site,
                param_owners,
                &trace,
                template,
                elements.as_ref(),
                &refuse,
            ),
            Err(reason) => refuse(reason),
        };
        self.template_catalog
            .borrow_mut()
            .stats_mut()
            .refused
            .extend(refusals.into_inner());
        derived
    }

    /// A retained type under an instance's arguments, naming the generated
    /// Tuple the clone check selects for a closed public one.
    pub(super) fn instance_ty(&self, ty: &Ty, substitution: &TySubst) -> Ty {
        self.canonicalize_public_tuple_types(mojito_types::types::substitute(ty, substitution))
    }

    /// [`TemplateObligation::ReplayedTransfers`]: replay the template's
    /// transfers for the instance.
    ///
    /// A source stands where its binding's substituted type may still carry
    /// a loan, and vanishes where it is plain data, which is what the
    /// instance's own check records for such a binding. A call whose realized
    /// callee publishes an empty summary records nothing, so every source
    /// there must vanish; one whose summary is the one the template read
    /// replays the kept sources; any other summary refuses. A read whose
    /// realized summary is empty is observed empty, as a plain-data clone's
    /// check observes it; any other read must find the summary it recorded.
    fn realize_transfers(
        &self,
        facts: &mut CheckedBodyFacts,
        template: &CheckedBodyFacts,
        substitute: &dyn Fn(&Ty) -> Ty,
    ) -> Result<(), &'static str> {
        let kept = |source: &TemplateTransferSource| {
            source
                .root_ty
                .as_ref()
                .is_none_or(|ty| !self.loan_free(&substitute(ty)))
        };
        let realized_source = |source: &TemplateTransferSource| TemplateTransferSource {
            origin: source.origin.clone(),
            root_ty: source.root_ty.as_ref().map(substitute),
        };
        let summaries = self.transfer_effects.borrow();
        let mut call_transfers = Vec::new();
        for (id, transfers) in &template.call_transfers {
            let callee = facts
                .selected_calls
                .iter()
                .find(|(call, _)| call == id)
                .map(|(_, call)| call.contract.target.as_str())
                .or_else(|| {
                    facts
                        .overload_targets
                        .iter()
                        .find(|(call, _)| call == id)
                        .map(|(_, target)| target.as_str())
                })
                .ok_or("a replayed transfer's call names no realized callee")?;
            let read = facts
                .transfer_reads
                .iter()
                .find(|(read, _)| read == callee)
                .map(|(_, effects)| effects)
                .ok_or("a replayed transfer's callee summary was not read")?;
            let realized: Vec<TemplateCallTransfer> = transfers
                .iter()
                .filter_map(|transfer| {
                    let sources: Vec<_> = transfer
                        .sources
                        .iter()
                        .filter(|source| kept(source))
                        .map(realized_source)
                        .collect();
                    (!sources.is_empty()).then(|| TemplateCallTransfer {
                        sources,
                        ..transfer.clone()
                    })
                })
                .collect();
            let summary = summaries.get(callee).map(Vec::as_slice).unwrap_or_default();
            if summary.is_empty() {
                if !realized.is_empty() {
                    return Err("a transfer survives a callee whose summary is empty");
                }
            } else if summary != read.as_slice() {
                return Err("a callee's transfer summary is no longer the one the template read");
            }
            if !realized.is_empty() {
                call_transfers.push((*id, realized));
            }
        }
        facts.call_transfers = call_transfers;
        facts.transferred_origins = template
            .transferred_origins
            .iter()
            .filter_map(|(dest, sources)| {
                let sources: Vec<_> = sources
                    .iter()
                    .filter(|source| kept(source))
                    .map(realized_source)
                    .collect();
                (!sources.is_empty()).then(|| (dest.clone(), sources))
            })
            .collect();
        // A source member whose binding is plain data vanishes, as the
        // instance's own check abstracts only the origins a loan-carrying
        // value holds: an effect left with no member vanishes, and one left
        // with a single member takes that member's own place flag, or
        // vanishes where it is the destination. A latent effect is published
        // where its stored type now carries a loan, stays latent while that
        // type is still symbolic, and vanishes otherwise, as the instance's
        // own check of the store decides.
        facts.transfer_effects = template
            .transfer_effects
            .iter()
            .filter_map(|effect| {
                let (members, sources): (Vec<_>, Vec<_>) = sig_origin_members(&effect.effect.src)
                    .into_iter()
                    .zip(effect.sources.iter().map(|source| TemplateEffectSource {
                        ty: substitute(&source.ty),
                        is_place: source.is_place,
                    }))
                    .filter(|(_, source)| !self.loan_free(&source.ty))
                    .unzip();
                let src_is_place = match sources.as_slice() {
                    [only] => only.is_place,
                    _ => false,
                };
                let src = mojito_types::origin::SigOrigin::union(members);
                (!sources.is_empty() && src != effect.effect.dest).then(|| TemplateTransferEffect {
                    effect: mojito_types::types::TransferEffect {
                        src,
                        src_is_place,
                        ..effect.effect.clone()
                    },
                    sources,
                    latent: effect.latent.clone(),
                })
            })
            .filter_map(|effect| {
                let latent = match effect.latent.as_ref().map(substitute) {
                    None => None,
                    Some(stored) if self.type_carries_loans(&stored) => None,
                    Some(stored)
                        if mojito_types::types::is_symbolic(&stored)
                            && self.type_may_carry_loans(&stored) =>
                    {
                        Some(stored)
                    }
                    Some(_) => return None,
                };
                Some(TemplateTransferEffect { latent, ..effect })
            })
            .collect();
        let (empty, replayed): (Vec<_>, Vec<_>) = std::mem::take(&mut facts.transfer_reads)
            .into_iter()
            .partition(|(callee, _)| summaries.get(callee).is_none_or(Vec::is_empty));
        if replayed
            .iter()
            .any(|(callee, read)| summaries.get(callee) != Some(read))
        {
            return Err("a callee's transfer summary is no longer the one the template read");
        }
        facts.transfer_reads = replayed;
        for (callee, _) in empty {
            if !facts.effect_free_callees.contains(&callee) {
                facts.effect_free_callees.push(callee);
            }
        }
        Ok(())
    }

    /// [`Self::derivable_facts`] once the body's trace is known.
    fn derive(
        &self,
        site: &BodySite<'_>,
        param_owners: &BodyParams,
        trace: &InstanceTrace,
        template: bool,
        elements: Option<&CheckedBodyFacts>,
        refuse: &dyn Fn(&'static str) -> Option<DerivedBody>,
    ) -> Option<DerivedBody> {
        let name = &site.display;
        let body = site.body;
        let catalog = self.template_catalog.borrow();
        let Some(checked) = catalog.template(&trace.template) else {
            return refuse("its template has no retained facts");
        };
        let TemplateCoverage::Certified(class) = &checked.coverage else {
            return refuse("its template is not certified");
        };
        // Both enabled classes bake every parameter: a clone that keeps a
        // binder, or folds a value, is outside them.
        // An origin binder and a trait-bounded type binder are the
        // exceptions: a clone keeps each, bound symbolically as the
        // template's is, and no fact substitutes it. So is a method's own
        // scalar value binder (`VALUE_BINDERS`), which the parser spells as
        // a bound (`n: Int`): a per-instantiation clone keeps it, and an
        // occurrence reading it is bound to the clone's own parameter. A
        // compile-time callable binder (`CALLABLE_BINDERS`) is kept by every
        // clone the same way.
        let kept_binders = matches!(class, TemplateClass::MethodBody(features)
                if features.contains(MethodFeatures::ORIGIN_PARAMETERS)
                    || features.contains(MethodFeatures::BOUND_BINDERS)
                    || features.contains(MethodFeatures::VALUE_BINDERS)
                    || features.contains(MethodFeatures::CALLABLE_BINDERS))
            && matches!(site.declaration, BodyDeclaration::Method(method)
            if method.type_params.iter().all(|binder| {
                origin_binder(binder)
                    || bound_binder(binder)
                    || (callable_binder(binder)
                        && matches!(class, TemplateClass::MethodBody(features)
                            if features.contains(MethodFeatures::CALLABLE_BINDERS)))
            })
                && trace.residual.iter().all(|name| {
                    method.type_params.iter().any(|binder| binder.name == *name)
                }));
        // A body the elaborator shaped itself names no binder its clone
        // keeps: a SIMD-keyed method's stub keeps the wildcard vector binder.
        let baked = trace.first_copy_template
            || ((trace.residual.is_empty() && !site.residual_binders) || kept_binders)
                && match class {
                    // A value-keyed struct specialized whole folds its own
                    // values, and a per-call clone its method's own, which
                    // `instance_substitution` binds and the occurrences read as
                    // the literals they folded to (`folded_literals`).
                    TemplateClass::MethodBody(features)
                        if features.contains(MethodFeatures::VALUE_BINDERS) =>
                    {
                        true
                    }
                    TemplateClass::MethodScalarBody | TemplateClass::MethodBody(_) => {
                        trace.value_bindings.is_empty()
                            || (matches!(&site.receiver_arguments, Some(arguments) if arguments.is_empty())
                                && matches!(site.declaration, BodyDeclaration::Method(method)
                                if trace.value_bindings.iter().all(|(name, _)| {
                                    method.type_params.iter().all(|binder| binder.name != *name)
                                })))
                    }
                    // The folded values selected the arms, or are read as the
                    // literals each occurrence folded to (`folded_literals`). A
                    // folded loop index is read back from the copy it fixed.
                    TemplateClass::ClosedScalarBody
                    | TemplateClass::FixedCalls
                    | TemplateClass::BoundedOperations
                    | TemplateClass::ScalarBranches
                    | TemplateClass::PackElements
                    | TemplateClass::FunctionBody(_) => true,
                };
        if !template && !baked {
            return refuse("the clone keeps or folds a compile-time parameter");
        }
        if param_owners.runtime.iter().any(Option::is_none) {
            return refuse("a parameter has no binding identity");
        }
        let Ok(substitution) = self.instance_substitution(site, checked, trace) else {
            return refuse("an instance argument does not resolve");
        };
        // A clone whose only binders are the elaborator's origin binders
        // (`Span[Int, __clone_origin0]`) carries exactly those binders' loans
        // in its arguments, and an argument whose origins the elaborator
        // erased (`Tuple[StringSpan[_], StringSpan[_]]`) carries loans to no
        // place in either check; the transfer recipe judges each replayed
        // source by its binding's substituted type.
        let binder_clone = site
            .declaration
            .type_params()
            .is_some_and(|binders| !binders.is_empty() && binders.iter().all(clone_origin_binder));
        if matches!(
            class,
            TemplateClass::MethodBody(_) | TemplateClass::FunctionBody(_)
        ) && !substitution
            .types
            .values()
            .chain(substitution.packs.values().flatten())
            .all(|ty| {
                self.plain_data(ty)
                    || ((erased_tail_loans(ty) || (binder_clone && binder_tail_loans(ty)))
                        && !self.type_contains_reference(ty)
                        && !mentions_callable(ty))
            })
        {
            return refuse("an instance argument carries a loan, a reference, or a callable");
        }
        let Some(desugars) = self.instance_with_desugars(body, &checked.facts.with_forms) else {
            return refuse("a `with` statement has no desugar form in its template");
        };
        let mut occurrences =
            self.occurrences_over(body, &desugars, Some(&checked.facts.occurrences));
        // An element's concrete construction keeps the identity of the
        // `Ts[i]()` it elaborates, and its other nodes derive from it.
        let element_part = |occurrence: &Occurrence| {
            element_construction_part(&checked.facts.element_constructions, occurrence)
        };
        fold_vector_values(
            &mut occurrences,
            &checked.facts.occurrences,
            &checked.facts.element_constructions,
        );
        // Every occurrence of the body is one the template checked. A class
        // without compile-time control flow keeps them all, once each; a
        // keyed one keeps the arms the elaborator selected, once per loop
        // iteration it unrolled, and drops the rest, facts and all.
        let keyed = class.keyed();
        // A relocated pack's spread and transfer have no instance occurrence,
        // nor has a spread pack's collector, which each element names anew.
        let relocated: Vec<SyntaxId> = checked
            .facts
            .pack_relocations
            .iter()
            .flat_map(|relocation| [relocation.spread.syntax, relocation.transfer.syntax])
            .chain(checked.facts.pack_spreads.iter().flat_map(|spread| {
                [
                    spread.spread.syntax,
                    spread.transfer.syntax,
                    spread.pack.syntax,
                ]
            }))
            .collect();
        let spread_element = |occurrence: &Occurrence| {
            PackElementNode::of(occurrence.id.syntax).is_some_and(|(spread, _, _)| {
                checked
                    .facts
                    .pack_spreads
                    .iter()
                    .any(|pack| pack.spread.syntax == spread)
            })
        };
        // A folded struct value (`Self.key`, `Self.n`) drops the `Self` its
        // name was read on, the template occurrence right after the name's
        // in pre-order, which the instance no longer holds; a vector adds
        // its lanes, which no template occurrence has. A type name's lone
        // binder argument (`_unqualified_type_name[T]()` in a `def`) is a
        // spelling the template records nothing at, which the elaborator
        // writes as the substituted type.
        let instance_syntax: HashSet<SyntaxId> = occurrences
            .iter()
            .map(|occurrence| occurrence.id.syntax)
            .collect();
        let folded_selves: Vec<SyntaxId> = occurrences
            .iter()
            .filter_map(|occurrence| {
                let order = &checked.facts.occurrences;
                let at = order
                    .iter()
                    .position(|id| id.syntax == occurrence.id.syntax)?;
                let next = order.get(at + 1)?.syntax;
                match occurrence.vector_fold {
                    Some(VectorFold::Construction) => Some(next),
                    None if (occurrence.literal.is_some()
                        || occurrence.callee.as_deref() == Some("_unqualified_type_name"))
                        && !instance_syntax.contains(&next) =>
                    {
                        Some(next)
                    }
                    _ => None,
                }
            })
            .collect();
        let template_occurrences = checked
            .facts
            .occurrences
            .iter()
            .filter(|id| !relocated.contains(&id.syntax) && !folded_selves.contains(&id.syntax))
            .count();
        let elaborated = |occurrence: &Occurrence| {
            matches!(
                occurrence.vector_fold,
                Some(VectorFold::Lane(_) | VectorFold::Dimension)
            ) || spread_element(occurrence)
                || element_part(occurrence)
        };
        let lanes = occurrences
            .iter()
            .filter(|occurrence| elaborated(occurrence))
            .count();
        let traced = occurrences.iter().all(|occurrence| {
            elaborated(occurrence)
                || ((keyed || occurrence.id.copy == 0)
                    && !folded_selves.contains(&occurrence.id.syntax)
                    && checked
                        .facts
                        .occurrences
                        .iter()
                        .any(|id| id.syntax == occurrence.id.syntax))
        }) && (keyed || occurrences.len() - lanes == template_occurrences);
        if !traced {
            timing::note("template_derivations.untraced", || {
                let strangers: Vec<String> = occurrences
                    .iter()
                    .filter(|occurrence| {
                        !checked
                            .facts
                            .occurrences
                            .iter()
                            .any(|id| id.syntax == occurrence.id.syntax)
                    })
                    .map(|occurrence| format!("{:?}", occurrence.span.span))
                    .collect();
                format!("{name}: {}", strangers.join(" "))
            });
            return refuse("the clone's occurrences are not the template's");
        }
        occurrences
            .retain(|occurrence| !matches!(occurrence.vector_fold, Some(VectorFold::Constant)));
        let ids: Vec<OccurrenceId> = occurrences.iter().map(|occurrence| occurrence.id).collect();
        let Some(folded) = folded_literals(&checked.facts, &occurrences) else {
            return refuse("a folded compile-time value does not fit its template type");
        };
        let indices = occurrences
            .iter()
            .filter_map(|occurrence| {
                let (_, index) = occurrence.folded_index?;
                let (_, binder) = checked
                    .facts
                    .expression_types
                    .iter()
                    .map(|(id, ty)| (id, ty))
                    .chain(
                        checked
                            .facts
                            .rebind_assertions
                            .iter()
                            .map(|(id, assertion)| (id, &assertion.operand)),
                    )
                    .filter(|(id, _)| id.syntax == occurrence.id.syntax)
                    .find_map(|(_, ty)| match ty {
                        Ty::Dependent(dependent) => dependent.pack_element(),
                        _ => None,
                    })?;
                match binder.kind() {
                    mojito_types::param_expr::ParamKind::DeclRef(reference) => Some((
                        occurrence.id,
                        (reference.id.clone(), mojito_types::ct::CtValue::Int(index)),
                    )),
                    _ => None,
                }
            })
            .collect();
        let indices = loop_element_indices(indices, &checked.facts, &occurrences);
        let Some(indices) = constructed_element_indices(
            indices,
            &checked.facts,
            elements,
            &substitution,
            &occurrences,
        ) else {
            return refuse("an element's default construction does not name its loop index");
        };
        let indices = transferred_element_indices(indices, &occurrences);
        let mut selected = checked.facts.selected(&ids, &folded);
        if !relocate_packs(&mut selected, &ids) {
            return refuse("a relocated pack is not the instance's transfer");
        }
        if !construct_folded_vectors(&mut selected, &occurrences) {
            return refuse("a folded vector value is not the template's closed vector");
        }
        // A reference accessor's value twin (`__getitem_param_value__$k`)
        // returns by value what its template handed out as a reference.
        let returns_reference = self
            .return_ref_contracts
            .last()
            .is_some_and(Option::is_some);
        if matches!(class, TemplateClass::MethodBody(features)
            if features.contains(MethodFeatures::REFERENCE_RESULT))
            && !returns_reference
        {
            let returned: Vec<SyntaxId> = returned_values(body)
                .into_iter()
                .map(|syntax| self.syntax_origins.origin(syntax))
                .collect();
            read_returns_by_value(&mut selected, &returned);
        }
        let realized = self
            .realize_instance_facts(&selected, &substitution, &indices, &occurrences)
            .and_then(|mut facts| {
                let constructed = std::mem::take(&mut facts.element_constructions);
                match elements {
                    Some(elements) => {
                        merge_element_constructions(&mut facts, elements, &constructed, &ids)?;
                    }
                    None if constructed.is_empty() => {}
                    None => return Err("an element construction was not checked"),
                }
                Ok(facts)
            });
        match realized {
            Ok(facts) => Some(DerivedBody {
                facts,
                spans: occurrences
                    .into_iter()
                    .map(|occurrence| (occurrence.id, occurrence.span))
                    .collect(),
                desugars,
            }),
            Err(reason) => refuse(reason),
        }
    }

    /// [`TemplateObligation::PlainDataArguments`] for one instance argument.
    fn plain_data(&self, ty: &Ty) -> bool {
        !self.type_may_carry_loans(ty)
            && !self.type_contains_reference(ty)
            && !mentions_callable(ty)
    }

    /// [`TemplateObligation::ReplayedTransfers`] for one source's binding type: a
    /// closed type whose storage, with its fields at their own arguments,
    /// holds no loan, no reference, and no callable. `plain_data` judges an
    /// instance argument, which may be symbolic.
    fn loan_free(&self, ty: &Ty) -> bool {
        let callable_field = matches!(ty, Ty::Struct(name, _)
        if self.structs.get(name).is_some_and(|info| {
            info.fields.iter().any(|(_, field)| mentions_callable(field))
        }));
        !mojito_types::types::is_symbolic(ty)
            && !self.type_carries_loans(ty)
            && !self.type_contains_reference(ty)
            && !mentions_callable(ty)
            && !callable_field
    }

    /// [`TemplateObligation::CallThroughResidue`] for one substituted type:
    /// closed, and holding no loan and no reference in its storage, its
    /// fields at their own arguments, or holding only loans whose origins
    /// the elaborator erased (`StringSpan[_]`), which name no place a
    /// residue argument could carry. A callable is not refused as
    /// `loan_free` refuses one: what a callable's storage carries is its
    /// environment's, which no substitution changes, so a `thin` one carries
    /// nothing in either check and a `capturing` one the same open set.
    fn residue_plain(&self, ty: &Ty) -> bool {
        !mojito_types::types::is_symbolic(ty)
            && (!self.type_carries_loans(ty) || erased_tail_loans(ty))
            && !self.type_contains_reference(ty)
    }

    /// The checked type each baked type parameter stands for in a clone.
    ///
    /// A `def` clone's are the source types the elaborator wrote, resolved as
    /// the clone's own annotations are. A method clone's are the arguments of
    /// the receiver type the checker already resolved for it, in the struct's
    /// binder order: the raw request passes through origin erasure and
    /// literal defaulting before the clone is minted, so only the resolved
    /// receiver says what `Self.T` is inside the clone. A per-call clone's
    /// own binders are the source types and packs its trace names, resolved
    /// as a `def` clone's are.
    fn instance_substitution(
        &self,
        site: &BodySite<'_>,
        template: &CheckedTemplate,
        trace: &InstanceTrace,
    ) -> Result<InstanceSubstitution, TypeError> {
        // A binding names the template's own binder; the declaration
        // carries its identity.
        let resolve = |source: &mojito_ast::ast::Type| self.trace_source_ty(source);
        let Some(arguments) = &site.receiver_arguments else {
            let decl_named = |name: &str| {
                template
                    .param_decls
                    .iter()
                    .find(|decl| decl.name().trim_start_matches('*') == name)
            };
            let mut values = Vec::new();
            let mut views = Vec::new();
            let types = trace
                .type_bindings
                .iter()
                .filter_map(|(name, source)| {
                    decl_named(name).map(|decl| {
                        let ty = resolve(source)?;
                        values.extend(simd_binder_values(decl, &ty, &mut views)?);
                        Ok((decl.id().clone(), ty))
                    })
                })
                .collect::<Result<_, _>>()?;
            let packs = trace
                .pack_bindings
                .iter()
                .filter_map(|(name, sources)| {
                    let elements = sources.iter().map(resolve);
                    decl_named(name)
                        .map(|decl| Ok((decl.id().clone(), elements.collect::<Result<_, _>>()?)))
                })
                .collect::<Result<_, _>>()?;
            values.extend(trace.value_bindings.iter().filter_map(|(name, value)| {
                decl_named(name).map(|decl| (decl.id().clone(), value.clone()))
            }));
            return Ok(InstanceSubstitution {
                types,
                packs,
                views,
                values,
                kept_values: Vec::new(),
                named_self: None,
            });
        };
        // A body the elaborator shaped itself names nothing its receiver
        // binds.
        if site.role == BodyRole::Template || trace.first_copy_template {
            return Ok(InstanceSubstitution {
                types: HashMap::new(),
                packs: HashMap::new(),
                views: Vec::new(),
                values: Vec::new(),
                kept_values: Vec::new(),
                named_self: None,
            });
        }
        let unresolved = || {
            TypeError::InvariantViolation(
                "a method clone's receiver does not bind its struct's parameters".to_string(),
            )
        };
        // The declarations are the struct's binders followed by the method's
        // own. A per-instantiation clone keeps its own symbolic
        // (`BOUND_BINDERS`); a per-call clone bakes them, and its trace
        // names the source type, element list, or value written for each. A member
        // of a variadic struct specialized whole (`Tuple$t2[String, Int]`)
        // has the pack's elements as its receiver's arguments, as many as its
        // trace names; a specialization whose receiver carries none (a user
        // struct's `Pair$t2[…]`, `TString$…`) binds the element sources its
        // trace names, resolved as a `def` clone's are.
        let struct_owner = template
            .id
            .owner
            .as_deref()
            .map(crate::checker::annotations::binder_owner);
        let struct_count = template
            .param_decls
            .iter()
            .take_while(|decl| struct_owner.as_deref() == Some(&*decl.id().owner))
            .count();
        let (struct_decls, own) = template.param_decls.split_at(struct_count);
        let resolved_arguments = || {
            arguments
                .iter()
                .map(|argument| match argument {
                    mojito_types::types::TyArg::Ty(ty) => Ok(ty.clone()),
                    _ => Err(unresolved()),
                })
                .collect::<Result<Vec<_>, _>>()
        };
        let mut types = TySubst::new();
        let mut packs = HashMap::new();
        let mut views = Vec::new();
        let mut values = Vec::new();
        let mut kept_values = Vec::new();
        let mut named_self = None;
        match struct_decls {
            [
                ParamDecl::Type {
                    id,
                    name,
                    variadic: true,
                    ..
                },
            ] => {
                let traced = trace
                    .pack_bindings
                    .iter()
                    .find(|(bound, _)| bound == name.trim_start_matches('*'))
                    .map(|(_, elements)| elements)
                    .ok_or_else(unresolved)?;
                let elements = if arguments.is_empty() {
                    let elements = traced.iter().map(resolve).collect::<Result<Vec<_>, _>>()?;
                    // The template's `Self` names this instance only when the
                    // resolved pack mangles back to it.
                    let own = template.id.owner.as_deref().map(|owner| {
                        self.specialized_value_structs(
                            &self.canonicalize_public_tuple_types(Ty::Struct(
                                owner.to_string(),
                                elements
                                    .iter()
                                    .cloned()
                                    .map(mojito_types::types::TyArg::Ty)
                                    .collect(),
                            )),
                        )
                    });
                    match own {
                        Some(Ty::Struct(name, arguments))
                            if arguments.is_empty()
                                && site.instance.owner.as_deref() == Some(&*name) => {}
                        Some(own) => {
                            named_self = Some((
                                own,
                                self.tstring_specialization(site, &elements)
                                    .ok_or_else(unresolved)?,
                            ));
                        }
                        None => return Err(unresolved()),
                    }
                    elements
                } else if traced.len() == arguments.len() {
                    resolved_arguments()?
                } else {
                    return Err(unresolved());
                };
                if elements.is_empty() && named_self.is_none() {
                    return Err(unresolved());
                }
                packs.insert(id.clone(), elements);
            }
            _ if struct_decls.len() == arguments.len() => {
                for (decl, ty) in struct_decls.iter().zip(resolved_arguments()?) {
                    match decl {
                        ParamDecl::Type {
                            id,
                            variadic: false,
                            ..
                        } => {
                            types.insert(id.clone(), ty);
                        }
                        _ => return Err(unresolved()),
                    }
                }
            }
            // A value-keyed struct specialized whole folds its values, which
            // its members' traces name.
            _ if arguments.is_empty() => {
                for decl in struct_decls {
                    let value = match decl {
                        ParamDecl::Value {
                            variadic: false, ..
                        } => trace
                            .value_bindings
                            .iter()
                            .find(|(name, _)| name == decl.name())
                            .map(|(_, value)| value.clone()),
                        _ => None,
                    };
                    values.push((decl.id().clone(), value.ok_or_else(unresolved)?));
                }
            }
            _ => return Err(unresolved()),
        }
        for decl in own {
            let name = decl.name().trim_start_matches('*');
            if let Some((_, source)) = trace.type_bindings.iter().find(|(bound, _)| bound == name) {
                let ty = resolve(source)?;
                values.extend(simd_binder_values(decl, &ty, &mut views)?);
                types.insert(decl.id().clone(), ty);
            } else if let Some((_, sources)) =
                trace.pack_bindings.iter().find(|(bound, _)| bound == name)
            {
                let elements = sources.iter().map(resolve).collect::<Result<_, _>>()?;
                packs.insert(decl.id().clone(), elements);
            } else if let Some((_, value)) = trace
                .value_bindings
                .iter()
                .find(|(bound, _)| bound == name)
                .filter(|_| matches!(decl, ParamDecl::Value { .. }))
            {
                values.push((decl.id().clone(), value.clone()));
            } else if matches!(decl, ParamDecl::Value { .. })
                && site
                    .declaration
                    .type_params()
                    .is_some_and(|binders| binders.iter().any(|binder| binder.name == name))
            {
                kept_values.push(name.to_string());
            } else if !matches!(decl, ParamDecl::Type { bounds, .. } if !bounds.is_empty()) {
                return Err(unresolved());
            }
        }
        Ok(InstanceSubstitution {
            types,
            packs,
            views,
            values,
            kept_values,
            named_self,
        })
    }

    /// A trace's source type, resolved as the clone's own signature
    /// resolved it: a generated declaration's spelling of an already-checked
    /// type (`StringLiteral`) is admitted where a user-spelled one is not. A
    /// t-string element that is itself a t-string (`TString[StringLiteral,
    /// Int]`) names the specialization discovery minted for its elements by
    /// its symbol alone, as the clone check does; no annotation may spell it.
    fn trace_source_ty(&self, source: &mojito_ast::ast::Type) -> Result<Ty, TypeError> {
        if let mojito_ast::ast::Type::Named(name, arguments) = source
            && name == mojito_types::types::TSTRING_TYPE_NAME
        {
            let elements = arguments
                .iter()
                .map(|argument| match argument {
                    mojito_ast::ast::ParamArg::Type(element) => self.trace_source_ty(element),
                    _ => Err(TypeError::UnknownType(name.clone())),
                })
                .collect::<Result<Vec<_>, _>>()?;
            let symbol = mojito_symbol::symbol::tstring_specialization_symbol(&elements);
            return if self.structs.contains_key(&symbol) {
                Ok(Ty::Struct(symbol, Vec::new()))
            } else {
                Err(TypeError::UnknownType(symbol))
            };
        }
        let generated = self.generated_declaration.replace(true);
        self.bare_string_literal_parameter.set(
            crate::checker::declarations::is_string_literal_annotation(source),
        );
        let ty = self.ty_from_anno(source);
        self.bare_string_literal_parameter.set(false);
        self.generated_declaration.set(generated);
        ty
    }

    /// A substituted type as the clone check names it: a closed public
    /// `Tuple` by the specialization selected for it
    /// (`canonicalize_public_tuple_types`), a value-keyed or variadic struct
    /// at closed arguments likewise (`specialized_value_structs`), and a
    /// `TString`'s `Self` by its own symbol (`named_self`).
    fn instance_names(&self, ty: Ty, named_self: &[(Ty, Ty)]) -> Ty {
        fold_binder_views(
            &self.specialized_value_structs(&self.canonicalize_public_tuple_types(ty)),
            named_self,
        )
    }

    /// The `TString` specialization a member instance belongs to, when its
    /// storage is the public tuple of `elements`: the type the clone check
    /// names its `Self` by. Its symbol names the public segments, which the
    /// storage pack no longer tells apart (`tstring_storage_elements`).
    fn tstring_specialization(&self, site: &BodySite<'_>, elements: &[Ty]) -> Option<Ty> {
        let owner = site.instance.owner.as_deref()?;
        let storage = self.public_tuple_type(elements.to_vec());
        let stored = self
            .structs
            .get(owner)?
            .fields
            .iter()
            .all(|(_, ty)| *ty == storage);
        (stored
            && mojito_symbol::symbol::specialization_template(owner)
                == Some(mojito_types::types::TSTRING_TYPE_NAME))
        .then(|| Ty::Struct(owner.to_string(), Vec::new()))
    }

    /// A template's facts for one instance: every retained type substituted,
    /// and every direct call realized against the instance's own syntax and
    /// the declarations in scope now.
    ///
    /// The selected callee is the template's choice and is never re-ranked.
    /// What is per-instance is only what the clone check also decides after
    /// selection: whether the closed application already has a clone
    /// (`existing_def_clone`), and — when the elaborator already retargeted
    /// the call to that clone — the clone's own declared parameters. The
    /// callee's effect summaries must still be empty, as the template saw
    /// them. An `Err` is a refusal, never a verdict on the program.
    ///
    /// `indices` fixes, per occurrence, the loop index a pack element was
    /// copied for: a type keyed by that occurrence substitutes under it, so
    /// the dependent `Ts[i]` folds to the copy's own element.
    fn realize_instance_facts(
        &self,
        template: &CheckedBodyFacts,
        instance: &InstanceSubstitution,
        indices: &ElementIndices,
        occurrences: &[Occurrence],
    ) -> Result<CheckedBodyFacts, &'static str> {
        let InstanceSubstitution {
            types: substitution,
            packs,
            views,
            values,
            kept_values,
            named_self,
        } = instance;
        let canonical = |ty: Ty| self.instance_names(ty, named_self.as_slice());
        let substitute = |ty: &Ty| {
            canonical(mojito_types::types::substitute_packs(
                &fold_binder_views(ty, views),
                substitution,
                packs,
                values,
            ))
        };
        let demands = self.hash_leaf_demands.borrow().len();
        let mut facts = substituted_facts(template, instance, indices, &canonical)?;
        spread_packs(&mut facts, occurrences)?;
        realize_value_shaped_constructions(template, &mut facts, occurrences)?;
        realize_simd_intrinsics(template, &mut facts, occurrences)?;
        realize_lane_float_methods(template, &mut facts, occurrences)?;
        realize_lane_comparisons(template, &mut facts, occurrences)?;
        self.realize_lane_literals(template, &mut facts, occurrences)?;
        let pack_accessors = self.realize_pack_accessors(&mut facts, occurrences)?;
        // A per-call request the template recorded names the caller's own
        // binders; an instance that closed it would retarget the call in the
        // clone check, which no recipe repeats.
        for (_, instantiation) in &mut facts.method_instantiations {
            let arguments = mojito_types::types::map_tyargs(&instantiation.arguments, &substitute);
            if arguments != instantiation.arguments {
                return Err("an instance closes a per-call clone request");
            }
            instantiation.owner_arguments =
                mojito_types::types::map_tyargs(&instantiation.owner_arguments, &substitute);
        }
        // Inference marks a reference result a copyable read where its
        // referent is implicitly copyable, whatever reads it. A mark the
        // template made is one a by-value read may rest on. A copying
        // pointer write marks its value whatever the value's type, with no
        // reference result there (`BodyShape::copied_writes`).
        facts.copyable_reference_result_reads = facts
            .reference_results
            .iter()
            .filter(|(_, reference)| self.is_implicitly_copyable(&reference.referent))
            .map(|(id, _)| *id)
            .chain(
                template
                    .copyable_reference_result_reads
                    .iter()
                    .filter(|id| fact_at(&template.reference_results, **id).is_none())
                    .copied(),
            )
            .collect();
        if !template
            .copyable_reference_result_reads
            .iter()
            .all(|read| facts.copyable_reference_result_reads.contains(read))
        {
            return Err("a reference result is not implicitly copyable for the instance");
        }
        // The equality source validation took on faith. A false one is the
        // clone check's to report, in its own words.
        if facts
            .rebind_assertions
            .iter()
            .any(|(_, assertion)| assertion.operand != assertion.dest)
        {
            return Err("a rebind assertion does not hold for the instance");
        }
        if !self.copied_places_hold(&facts, occurrences) {
            return Err("a copied place is not implicitly copyable for the instance");
        }
        if !self.copied_receivers_hold(&facts, occurrences) {
            return Err("a copied consuming receiver is not implicitly copyable for the instance");
        }
        // A call-through residue is republished verbatim on the same ground:
        // an argument carries an origin only while its binding's type carries
        // a loan. A type the substitution left alone carries in the instance
        // what it carried in the template, so only the substituted types are
        // judged, and a callable among them is judged by its environment
        // alone, which never substitutes (`residue_plain`).
        let residue = !template.call_throughs.is_empty() || !template.call_through_reads.is_empty();
        let substituted_plain = facts
            .expression_types
            .iter()
            .zip(&template.expression_types)
            .chain(facts.binding_types.iter().zip(&template.binding_types))
            .filter(|((_, ty), (_, declared))| {
                mojito_types::types::is_symbolic(declared) && ty != declared
            })
            .all(|((_, ty), _)| self.residue_plain(ty));
        if residue && !substituted_plain {
            return Err("a substituted value may carry a loan where the body keeps a residue");
        }
        // A residue naming a compile-time callable names it by the binder's
        // name, which only an instance keeping that binder still declares.
        let kept_callables = template.call_throughs.iter().all(|residue| {
            !matches!(&residue.callee,
                mojito_checked::checked::CallThroughCallee::ValueParam(name)
                    if !instance.kept_values.contains(name))
        });
        if !kept_callables {
            return Err("a residue names a compile-time callable the instance does not keep");
        }
        // `Movable`, which a symbolic parameter always is.
        let movable = template.transfers.iter().all(|transfer| {
            fact_at(&template.expression_types, *transfer)
                .filter(|ty| mojito_types::types::is_symbolic(ty))
                .is_none_or(|ty| self.is_movable(&substitute(ty)))
        });
        if !movable {
            return Err("a transferred value is not movable for the instance");
        }
        // `check_capture_capability`'s demand on a nested `def`'s owned
        // capture, at the instance's type.
        let capturable = template
            .nested_defs
            .iter()
            .flat_map(|(_, recipe)| &recipe.captures)
            .filter(|capture| mojito_types::types::is_symbolic(&capture.ty))
            .all(|capture| match capture.kind {
                CaptureKind::Copy => self.is_implicitly_copyable(&substitute(&capture.ty)),
                CaptureKind::Move => self.is_movable(&substitute(&capture.ty)),
                CaptureKind::Imm | CaptureKind::Mut | CaptureKind::Ref => true,
            });
        if !capturable {
            return Err("a nested def's owned capture is not capturable for the instance");
        }
        // The declaration's own judgment of each binding whose type mentions
        // a parameter, at the instance's type: deletable where the type is
        // `Deinitable`, linear where it is still a bare parameter. A type
        // built over a parameter answers from its own conformance under the
        // instance's arguments. A tuple unpacking's target is judged nowhere.
        let unpacked: Vec<OccurrenceId> = template
            .tuple_unpacks
            .iter()
            .flat_map(|(_, unpack)| unpack.targets.iter().copied())
            .collect();
        for (id, declared) in &template.binding_types {
            if !mojito_types::types::is_symbolic(declared) || unpacked.contains(id) {
                continue;
            }
            let realized = substitute(declared);
            facts.deletable_bindings.retain(|binding| binding != id);
            facts.linear_bindings.retain(|binding| binding != id);
            if self.is_deinitable(&realized) {
                facts.deletable_bindings.push(*id);
            } else if matches!(realized, Ty::Param { .. }) {
                facts.linear_bindings.push(*id);
            }
        }
        let order = |id: &OccurrenceId| occurrences.iter().position(|found| found.id == *id);
        facts.deletable_bindings.sort_by_key(order);
        facts.linear_bindings.sort_by_key(order);
        facts.linear_temporaries.retain(|temporary| {
            fact_at(&facts.expression_types, *temporary)
                .is_some_and(|ty| matches!(ty, Ty::Param { .. }))
        });
        // `expect_bool`'s judgment of each condition the template tested
        // through `Bool(x)`, at the instance's type: a `Bool` is read as it
        // stands. A condition the template read as a `Bool` stays one.
        let mut truthiness = Vec::new();
        for condition in &facts.truthiness_conditions {
            let converts = fact_at(&facts.expression_types, *condition)
                .ok_or("a truthiness condition has no recorded type")
                .and_then(|ty| {
                    self.condition_truthiness(ty, "condition")
                        .map_err(|_| "a condition is not boolable for the instance")
                })?;
            if converts {
                truthiness.push(*condition);
            }
        }
        facts.truthiness_conditions = truthiness;
        renumber_locals(&mut facts)?;
        let folded = |owner: &TemplateOwner| matches!(owner, TemplateOwner::CompileTimeParam(name) if !kept_values.contains(name));
        if facts
            .expression_bindings
            .iter()
            .chain(&facts.statement_bindings)
            .any(|(_, owner)| folded(owner))
        {
            return Err("an occurrence still names a folded compile-time parameter");
        }
        // A nested `def`'s snapshot of a parameter the instance folds is no
        // capture there: the body reads the folded value.
        for (_, recipe) in &mut facts.nested_defs {
            recipe.captures.retain(|capture| !folded(&capture.owner));
        }
        // A nested `def`'s call selects the declaration the body itself
        // introduces, whose signature no instance changes, and reads its
        // summaries under the same name.
        let nested_calls = nested_def_calls(template);
        for (_, callee) in &nested_calls {
            note_realized_callee(&mut facts, callee, callee);
        }
        for (id, _) in &template.call_parameters {
            // A method call records its (empty) parameters here too. Its
            // callee is realized from its contract below, and a call through
            // a callable parameter from the instance's own binding of it.
            if template.selected_calls.iter().any(|(call, _)| call == id)
                || template
                    .inplace_updates
                    .iter()
                    .any(|(update, _)| update == id)
                || template.callable_calls.contains(id)
                || nested_calls.iter().any(|(call, _)| call == id)
            {
                continue;
            }
            self.realize_direct_call(&mut facts, template, *id, occurrences)?;
        }
        for call in &template.callable_calls {
            self.realize_callable_call(&mut facts, *call, occurrences)?;
        }
        facts.callable_calls.clear();
        self.realize_bound_builtins(template, &mut facts, occurrences)?;
        self.realize_iterations(&mut facts, &substitute)?;
        Self::realize_comprehension_bindings(&mut facts, &substitute);
        Self::realize_nested_defs(&mut facts, &substitute);
        self.realize_tuple_unpacks(&mut facts, &substitute)?;
        facts.struct_applications =
            self.instance_struct_applications(template, instance, &substitute);
        realize_pack_accessor_applications(&mut facts, occurrences, &pack_accessors);
        // A call through a bound is realized first: a built-in instance
        // drops it from the calls, and a struct instance gives it a nominal
        // target the closed-call recipe then leaves as it stands.
        let bound_dispatches: Vec<OccurrenceId> = facts
            .selected_calls
            .iter()
            .filter(|(_, call)| {
                mojito_symbol::symbol::is_trait_dispatch_symbol(&call.contract.target)
            })
            .map(|(id, _)| *id)
            .collect();
        for call in &bound_dispatches {
            self.realize_bound_dispatch(&mut facts, *call, occurrences)?;
        }
        // An element store's value getter is realized on the instance's
        // subscripted value, and its in-place dunder on the instance's
        // element where the template dispatched it through the element's
        // bound; a closed one stands as the template selected it
        // (`substituted_element_stores`).
        self.realize_element_getters(&mut facts, occurrences, substitution, &substitute)?;
        self.realize_element_dunders(&mut facts)?;
        self.realize_inplace_updates(&mut facts, substitution, &substitute)?;
        let inverted_writes = self.realize_inverted_writes(&mut facts, occurrences)?;
        for index in 0..facts.selected_calls.len() {
            let id = facts.selected_calls[index].0;
            if bound_dispatches.contains(&id) || inverted_writes.contains(&id) {
                continue;
            }
            self.realize_method_call(&mut facts, index, occurrences, substitution, &substitute)?;
        }
        self.realize_tuple_elements(&mut facts, occurrences, &pack_accessors)?;
        self.realize_static_overloads(&mut facts, occurrences, substitution)?;
        // A construction selected its constructor from the arguments' types
        // and named the instance's clone of it, where one exists.
        for construction in &template.constructions {
            self.realize_construction(&mut facts, *construction, occurrences, substitution)?;
        }
        for operator in &template.operators {
            self.realize_operator(&mut facts, *operator, occurrences)?;
        }
        facts.operators.clear();
        for (call, builtin) in &template.bound_builtins {
            self.realize_bound_builtin(&mut facts, *call, *builtin, occurrences)?;
        }
        facts.bound_builtins.clear();
        for call in &template.repr_calls {
            self.realize_repr_call(&facts, *call, occurrences)?;
        }
        facts.repr_calls.clear();
        for call in &template.print_calls {
            self.realize_print_call(&mut facts, *call, occurrences)?;
        }
        facts.print_calls.clear();
        self.realize_stringify_calls(&mut facts, occurrences)?;
        // A conversion is selected last: its source type is one the call and
        // construction recipes may have realized.
        for index in 0..facts.conversions.len() {
            self.realize_conversion(&mut facts, index, substitution)?;
        }
        realize_boundary_conversions(&mut facts)?;
        self.realize_transfers(&mut facts, template, &substitute)?;
        // Kept with struct origin slots unbound, as capture keeps them: a
        // loan-carrying argument substitutes a clone's origin binder there.
        facts.struct_applications = sorted_applications(
            std::mem::take(&mut facts.struct_applications)
                .into_iter()
                .map(|(name, arguments)| {
                    match without_struct_origins(&Ty::Struct(name.clone(), arguments.clone())) {
                        Ty::Struct(name, arguments) => (name, arguments),
                        _ => (name, arguments),
                    }
                })
                .collect(),
        );
        facts.effect_free_callees.sort();
        let summaries_empty = facts.effect_free_callees.iter().all(|callee| {
            self.transfer_effects
                .borrow()
                .get(callee)
                .is_none_or(Vec::is_empty)
                && self
                    .call_through_effects
                    .borrow()
                    .get(callee)
                    .is_none_or(Vec::is_empty)
        });
        if !summaries_empty {
            return Err("a callee's effect summary is no longer empty");
        }
        // Every residue read was rekeyed to a realized callee, which must
        // publish the residue the template read.
        let realized_targets: Vec<&str> = facts
            .selected_calls
            .iter()
            .map(|(_, call)| call.contract.target.as_str())
            .chain(
                facts
                    .overload_targets
                    .iter()
                    .map(|(_, target)| target.as_str()),
            )
            .collect();
        for (callee, residue) in &facts.call_through_reads {
            if !realized_targets.contains(&callee.as_str()) {
                return Err("a call-through residue was read from a callee no admitted call names");
            }
            let published = self.call_through_effects.borrow();
            if published.get(callee).map(Vec::as_slice) != Some(residue.as_slice()) {
                return Err("a callee's call-through residue is not the template's");
            }
        }
        let position = |id: &OccurrenceId| {
            occurrences
                .iter()
                .position(|occurrence| occurrence.id == *id)
        };
        facts.overload_targets.sort_by_key(|(id, _)| position(id));
        // An operator's copy, conversion, and adjustment are appended where
        // its recipe ran; the inferred bundle holds each in occurrence order.
        facts.copy_place_value_uses.sort_by_key(position);
        facts.conversions.sort_by_key(|(id, _)| position(id));
        facts
            .operation_adjustments
            .sort_by_key(|(id, _)| position(id));
        // The template's leaves are closed; a bound builtin or dispatch
        // realized above recorded the instance's own.
        let mut leaves = std::mem::take(&mut facts.hash_leaves);
        leaves.extend(self.hash_leaves_since(demands));
        facts.hash_leaves = canonical_hash_leaves(leaves);
        Ok(facts)
    }

    /// The generic-struct applications a template reached, substituted for
    /// an instance. A value-keyed struct's application at the instance's
    /// values names the specialization the elaborator minted, which the
    /// clone check records no application of.
    fn instance_struct_applications(
        &self,
        template: &CheckedBodyFacts,
        InstanceSubstitution {
            types,
            packs,
            values,
            ..
        }: &InstanceSubstitution,
        substitute: &dyn Fn(&Ty) -> Ty,
    ) -> Vec<(String, Vec<mojito_types::types::TyArg>)> {
        template
            .struct_applications
            .iter()
            .filter(|(name, arguments)| {
                let application = mojito_types::types::substitute_packs(
                    &Ty::Struct(name.clone(), arguments.clone()),
                    types,
                    packs,
                    values,
                );
                !matches!(self.specialized_value_structs(&application),
                    Ty::Struct(specialization, _) if specialization != *name)
            })
            .map(|(name, arguments)| {
                (
                    name.clone(),
                    mojito_types::types::map_tyargs(arguments, substitute),
                )
            })
            .collect()
    }

    /// `infer_method_call`'s demand on a place a consuming call copies, at
    /// the instance's type.
    fn copied_receivers_hold(&self, facts: &CheckedBodyFacts, occurrences: &[Occurrence]) -> bool {
        facts
            .implicitly_copied_consuming_receivers
            .iter()
            .all(|call| {
                occurrences
                    .iter()
                    .find(|occurrence| occurrence.id == *call)
                    .and_then(|occurrence| occurrence.method_call.as_ref())
                    .and_then(|(receiver, _)| {
                        let receiver = OccurrenceId {
                            syntax: *receiver,
                            copy: call.copy,
                        };
                        fact_at(&facts.expression_types, receiver)
                    })
                    .is_some_and(|ty| self.is_implicitly_copyable(ty))
            })
    }

    /// `check_consuming`'s demand on each copied place, at the instance's
    /// type. The synthesized `copy`'s receiver is copied explicitly, which
    /// demands only `Copyable`.
    fn copied_places_hold(&self, facts: &CheckedBodyFacts, occurrences: &[Occurrence]) -> bool {
        let explicit_copy = |place: &OccurrenceId| {
            occurrences.iter().any(|occurrence| {
                occurrence.callee.as_deref() == Some("__mojito_fieldwise_copy")
                    && occurrence.id.copy == place.copy
                    && occurrence.arguments == [place.syntax]
            })
        };
        facts.copy_place_value_uses.iter().all(|place| {
            facts
                .expression_types
                .iter()
                .find(|(id, _)| id == place)
                .is_some_and(|(_, ty)| {
                    self.is_copyable(ty)
                        && (explicit_copy(place) || self.is_implicitly_copyable(ty))
                })
        })
    }

    /// `ty` with every application of a value-keyed struct at closed values
    /// (`_StridedRange[DType.int]`) named as the specialization the
    /// elaborator minted for it (`_StridedRange$dint;`), which is the type
    /// the clone check reads, and so is a user variadic struct at a closed
    /// pack (`Bag[Int, String]` as `Bag$t2[…]`). An application with no
    /// minted specialization is left as it is.
    fn specialized_value_structs(&self, ty: &Ty) -> Ty {
        struct Specialized<'a>(&'a Checker);

        impl mojito_types::types::TyRewrite for Specialized<'_> {
            fn whole(&mut self, ty: &Ty) -> Option<Ty> {
                let Ty::Struct(name, arguments) = ty else {
                    return None;
                };
                let values = arguments
                    .iter()
                    .map(|argument| match argument {
                        mojito_types::types::TyArg::Val(value) => Some(value.clone()),
                        _ => None,
                    })
                    .collect::<Option<Vec<_>>>()
                    .filter(|values| !values.is_empty())
                    .or_else(|| closed_pack_values(arguments))?;
                let mangled = mojito_symbol::symbol::mangle(name, &values).ok()?;
                self.0
                    .structs
                    .get(&mangled)
                    .is_some_and(|info| info.fixed_arguments.is_none())
                    .then(|| Ty::Struct(mangled, Vec::new()))
            }

            fn expr(
                &mut self,
                expr: &mojito_types::param_expr::ParamExpr,
            ) -> Result<mojito_types::param_expr::ParamExpr, mojito_types::param_expr::ParamError>
            {
                Ok(expr.clone())
            }
        }

        mojito_types::types::rewrite_ty(ty, &mut Specialized(self)).unwrap_or_else(|_| ty.clone())
    }
}

/// Number the locals an instance keeps as its own check would mint them.
///
/// A template numbers every local its body declares, in checking order. An
/// instance declares only those in the arms the elaborator selected, once per
/// unrolled copy of their declaration, and never a `comptime for` variable.
/// A fact naming a local names the declaration in scope where the fact sits:
/// the latest copy at or before its occurrence in pre-order, since each
/// unrolled copy is a scope of its own. A fact that sits at no occurrence may
/// name only a local declared once. The declarations are renumbered densely
/// in declaration order. A local no retained declaration introduces is left
/// alone; installing it then fails as a lost binding.
fn renumber_locals(facts: &mut CheckedBodyFacts) -> Result<(), &'static str> {
    let occurrences = facts.occurrences.clone();
    let position = |id: OccurrenceId| occurrences.iter().position(|found| *found == id);
    // A tuple unpacking declares each of its targets, whose binding sits at
    // the target.
    let unpacked: Vec<OccurrenceId> = facts
        .tuple_unpacks
        .iter()
        .filter(|(_, unpack)| unpack.declares)
        .flat_map(|(_, unpack)| unpack.targets.iter().copied())
        .collect();
    let mut declared: Vec<(usize, u32)> = facts
        .statement_bindings
        .iter()
        .chain(
            facts
                .expression_bindings
                .iter()
                .filter(|(id, _)| unpacked.contains(id)),
        )
        .map(|(id, owner)| (id, owner))
        .chain(
            facts
                .comprehension_bindings
                .iter()
                .flat_map(|(id, binders)| binders.iter().map(move |binder| (id, &binder.owner))),
        )
        // A nested `def` declares its parameters at its statement, after
        // the name the statement binds.
        .chain(
            facts
                .nested_defs
                .iter()
                .flat_map(|(id, recipe)| recipe.params.iter().map(move |param| (id, param))),
        )
        .filter_map(|(id, owner)| match owner {
            TemplateOwner::Local(index) => Some((position(*id)?, *index)),
            _ => None,
        })
        .collect();
    // A comprehension's binders are declared while the statement holding it
    // is checked, before the statement's own binding, so pre-order is not
    // the checking order there. Only a body with no unrolled copies holds
    // one, and it keeps every declaration, in the template's own order.
    if facts.comprehension_bindings.is_empty() {
        declared.sort_by_key(|(at, _)| *at);
    } else if occurrences.iter().all(|id| id.copy == 0) {
        declared.sort_by_key(|(_, index)| *index);
    } else {
        return Err("a comprehension sits in an unrolled copy");
    }
    let mut ambiguous = false;
    for_each_owner(facts, &mut |at, owner| {
        let TemplateOwner::Local(index) = owner else {
            return;
        };
        let mut copies = declared
            .iter()
            .enumerate()
            .filter(|(_, (_, local))| local == index);
        let declaration = if let Some(at) = at {
            let at = position(at);
            copies.rfind(|(_, (declared_at, _))| at.is_some_and(|at| *declared_at <= at))
        } else {
            let only = copies.next();
            ambiguous |= copies.next().is_some();
            only
        };
        if let Some((dense, _)) = declaration {
            *index = u32::try_from(dense).unwrap_or(u32::MAX);
        }
    });
    if ambiguous {
        return Err("a local declared in several unrolled copies is named outside any occurrence");
    }
    facts.locals = u32::try_from(declared.len()).unwrap_or(u32::MAX);
    Ok(())
}

/// Visit every binding a bundle names by template owner, with the occurrence
/// the fact naming it sits at: the binding tables, each invalidation's root
/// and exception, and the root of every place a reference's origin holds. A
/// transferred origin is the body's, and sits at none.
fn for_each_owner(
    facts: &mut CheckedBodyFacts,
    visit: &mut dyn FnMut(Option<OccurrenceId>, &mut TemplateOwner),
) {
    fn origin_owners(
        origin: &mut TemplateOrigin,
        at: Option<OccurrenceId>,
        visit: &mut dyn FnMut(Option<OccurrenceId>, &mut TemplateOwner),
    ) {
        match origin {
            TemplateOrigin::Place(place) => visit(at, &mut place.root),
            TemplateOrigin::Union(members) => members
                .iter_mut()
                .for_each(|member| origin_owners(member, at, visit)),
            TemplateOrigin::Unrooted(_) => {}
        }
    }
    /// Every call contract a bundle keeps, at its call: the selected calls,
    /// those an element store embeds, and each place's in-place update.
    fn calls<'f>(
        selected: &'f mut [(OccurrenceId, TemplateCallContract)],
        stores: &'f mut [(OccurrenceId, TemplateAugmentedSubscript)],
        updates: &'f mut [(OccurrenceId, TemplateCallContract)],
    ) -> impl Iterator<Item = (OccurrenceId, &'f mut TemplateCallContract)> {
        selected
            .iter_mut()
            .chain(updates)
            .map(|(id, call)| (*id, call))
            .chain(stores.iter_mut().flat_map(|(id, store)| {
                let id = *id;
                store.contracts_mut().map(move |call| (id, call))
            }))
    }
    for (id, owner) in facts
        .statement_bindings
        .iter_mut()
        .chain(&mut facts.expression_bindings)
    {
        visit(Some(*id), owner);
    }
    for (id, place) in &mut facts.interior_references {
        visit(Some(*id), &mut place.root);
    }
    for (id, iteration) in &mut facts.iterations {
        if let Some(source) = &mut iteration.source {
            visit(Some(*id), &mut source.root);
        }
    }
    for (id, unpack) in &mut facts.tuple_unpacks {
        if let Some(source) = &mut unpack.source {
            origin_owners(&mut source.origin, Some(*id), visit);
        }
    }
    for (id, binders) in &mut facts.comprehension_bindings {
        for binder in binders {
            visit(Some(*id), &mut binder.owner);
        }
    }
    for (id, recipe) in &mut facts.nested_defs {
        for param in &mut recipe.params {
            visit(Some(*id), param);
        }
        for origin in &mut recipe.function_origins {
            origin_owners(origin, Some(*id), visit);
        }
        for capture in &mut recipe.captures {
            visit(Some(*id), &mut capture.owner);
            for captured in &mut capture.origins {
                origin_owners(&mut captured.origin, Some(*id), visit);
            }
        }
    }
    for (id, accesses) in &mut facts.capture_accesses {
        for access in accesses {
            origin_owners(&mut access.origin, Some(*id), visit);
        }
    }
    for (id, reference) in facts
        .reference_results
        .iter_mut()
        .chain(&mut facts.reference_binding_types)
        .chain(&mut facts.reference_place_types)
    {
        origin_owners(&mut reference.origin, Some(*id), visit);
    }
    for (id, call) in calls(
        &mut facts.selected_calls,
        &mut facts.augmented_subscripts,
        &mut facts.inplace_updates,
    ) {
        if let Some(reference) = &mut call.reference_result {
            origin_owners(&mut reference.origin, Some(id), visit);
        }
        for origin in &mut call.result_origins {
            origin_owners(origin, Some(id), visit);
        }
    }
    for typed in &mut facts.typed_origins {
        for origin in &mut typed.origins {
            origin_owners(origin, Some(typed.occurrence), visit);
        }
    }
    for (id, slots) in &mut facts.call_result_origins {
        for resolved in slots {
            origin_owners(&mut resolved.origin, Some(*id), visit);
        }
    }
    for (id, transfers) in &mut facts.call_transfers {
        for source in transfers
            .iter_mut()
            .flat_map(|transfer| &mut transfer.sources)
        {
            origin_owners(&mut source.origin, Some(*id), visit);
        }
    }
    for (dest, sources) in &mut facts.transferred_origins {
        visit(None, dest);
        for source in sources {
            origin_owners(&mut source.origin, None, visit);
        }
    }
    let call_invalidations = calls(
        &mut facts.selected_calls,
        &mut facts.augmented_subscripts,
        &mut facts.inplace_updates,
    )
    .flat_map(|(id, call)| {
        call.arguments
            .iter_mut()
            .flat_map(|argument| &mut argument.invalidations)
            .chain(&mut call.invalidations)
            .map(move |invalidation| (id, invalidation))
    });
    let invalidations = facts
        .interior_invalidations
        .iter_mut()
        .flat_map(|(id, invalidations)| {
            let id = *id;
            invalidations
                .iter_mut()
                .map(move |invalidation| (id, invalidation))
        })
        .chain(call_invalidations);
    for (id, invalidation) in invalidations {
        visit(Some(id), &mut invalidation.root);
        if let Some(except) = &mut invalidation.except {
            visit(Some(id), except);
        }
    }
}

/// Whether a type is, holds, or is applied to a callable.
fn mentions_callable(ty: &Ty) -> bool {
    match ty {
        Ty::Func { .. } | Ty::GenericFunc { .. } | Ty::Overload(_) => true,
        Ty::Struct(_, arguments) => arguments.iter().any(|argument| {
            matches!(argument, mojito_types::types::TyArg::Ty(element) if mentions_callable(element))
        }),
        Ty::Tuple(elements) | Ty::RuntimePack(elements) | Ty::Variant(elements) => {
            elements.iter().any(mentions_callable)
        }
        _ => false,
    }
}

/// Whether every origin a type argument names sits in a struct origin tail
/// bound to a binder (`Span[Int, __clone_origin0]`), with no pointer or
/// reference beside it: the loans such a value carries are exactly those
/// binders'. [`TemplateObligation::PlainDataArguments`] admits it for a
/// clone whose only binders are the elaborator's origin binders.
fn binder_tail_loans(ty: &Ty) -> bool {
    use mojito_types::origin::Origin;
    use mojito_types::types::TyArg;
    let tail_binder = mojito_types::types::mentions(ty, &|candidate| {
        matches!(candidate, Ty::Struct(_, arguments)
            if arguments.iter().any(|argument| matches!(argument, TyArg::Origin(Origin::Param(_)))))
    });
    let other_origin = mojito_types::types::mentions(ty, &|candidate| {
        match candidate {
        Ty::Pointer { .. } | Ty::Ref(_) => true,
        Ty::Struct(_, arguments) => arguments
            .iter()
            .any(|argument| matches!(argument, TyArg::Origin(origin) if !matches!(origin, Origin::Param(_)))),
        _ => false,
    }
    });
    tail_binder && !other_origin
}

/// Whether every origin a type argument names is an erased struct origin
/// tail (`StringSpan[_]`), with no pointer or reference beside it: the
/// elaborator erased the loan's origin before minting the clone, so the
/// clone's own check tracks the loan to no place either.
fn erased_tail_loans(ty: &Ty) -> bool {
    use mojito_types::origin::Origin;
    use mojito_types::types::TyArg;
    let erased_tail = mojito_types::types::mentions(ty, &|candidate| {
        matches!(candidate, Ty::Struct(_, arguments)
            if arguments.iter().any(|argument| matches!(argument, TyArg::Origin(Origin::Unbound))))
    });
    let other_origin = mojito_types::types::mentions(ty, &|candidate| {
        match candidate {
        Ty::Pointer { .. } | Ty::Ref(_) => true,
        Ty::Struct(_, arguments) => arguments.iter().any(
            |argument| matches!(argument, TyArg::Origin(origin) if !matches!(origin, Origin::Unbound)),
        ),
        _ => false,
    }
    });
    erased_tail && !other_origin
}

/// The applications a pack accessor's call reaches in the instance: the
/// template's `Tuple[*Ts]` receiver is the generated Tuple the instance's
/// field holds, recorded only from a source that records applications at
/// all, as a tuple element's is (`realize_tuple_elements`).
fn realize_pack_accessor_applications(
    facts: &mut CheckedBodyFacts,
    occurrences: &[Occurrence],
    pack_accessors: &[OccurrenceId],
) {
    if pack_accessors.is_empty() {
        return;
    }
    facts.struct_applications.retain(|(name, arguments)| {
        name != mojito_types::types::TUPLE_TYPE_NAME
            || !arguments.iter().all(|argument| {
                matches!(argument, mojito_types::types::TyArg::Ty(Ty::RuntimePack(_)))
            })
    });
    for occurrence in occurrences
        .iter()
        .filter(|occurrence| pack_accessors.contains(&occurrence.id))
    {
        let source = occurrence.span.source.as_deref();
        let receiver = occurrence
            .method_call
            .as_ref()
            .map(|(receiver, _)| OccurrenceId {
                syntax: *receiver,
                copy: occurrence.id.copy,
            });
        if let Some(Ty::Struct(owner, arguments)) =
            receiver.and_then(|receiver| fact_at(&facts.expression_types, receiver))
            && source.is_some()
            && !crate::checker::overload_support::is_bundled_module_source(source)
        {
            let application = (owner.clone(), arguments.clone());
            facts.struct_applications.push(application);
        }
    }
}

/// Write each realized conversion back into the call boundary that carries
/// it a second time.
///
/// A converting argument records its conversion twice: in the four
/// conversion tables, which [`Checker::realize_conversion`] has just
/// re-selected at the instance's types, and in the selected call's own
/// boundary, which lowering reads to build the argument's register. The
/// boundary names the constructor the template found, so an instance takes
/// the target back from the conversion at the same occurrence; a boundary
/// conversion the tables no longer hold is one the recipes did not reach.
fn realize_boundary_conversions(facts: &mut CheckedBodyFacts) -> Result<(), &'static str> {
    let realized: Vec<(OccurrenceId, String)> = facts
        .conversions
        .iter()
        .map(|(id, conversion)| (*id, conversion.target.clone()))
        .collect();
    for (_, call) in facts
        .selected_calls
        .iter_mut()
        .chain(&mut facts.inplace_updates)
    {
        for argument in &mut call.arguments {
            for adjustment in &mut argument.adjustments {
                let mojito_checked::checked::CheckedCallValueAdjustment::ImplicitConversion {
                    target,
                } = adjustment
                else {
                    continue;
                };
                let selected = realized
                    .iter()
                    .find(|(id, _)| *id == argument.value)
                    .map(|(_, target)| target)
                    .ok_or("a converted argument kept no conversion of its own")?;
                selected.clone_into(target);
            }
        }
    }
    Ok(())
}

/// A construction of a binder the instance keeps symbolic (`H()` in a
/// method's `[H: Hasher]`), which the instance records as the template did.
///
/// The adjustment names the binder only by spelling; the recorded type names
/// its declaration, which the substitution leaves alone exactly when the
/// binder is the method's own rather than the struct's.
fn kept_binder_construction(
    template: &CheckedBodyFacts,
    id: OccurrenceId,
    adjustment: &mojito_checked::checked::SemanticAdjustment,
    substitute: &dyn Fn(&Ty) -> Ty,
) -> Option<mojito_checked::checked::SemanticAdjustment> {
    let mojito_checked::checked::SemanticAdjustment::ConstructTypeParam { param } = adjustment
    else {
        return None;
    };
    fact_at(&template.expression_types, id)
        .filter(|ty| matches!(ty, Ty::Param { binder, .. } if binder == param))
        .filter(|ty| substitute(ty) == **ty)
        .map(|_| adjustment.clone())
}

/// The template's facts with every retained type substituted for an
/// instance, before any call is realized: an adjustment through its recipe,
/// a type keyed by a pack-element occurrence under that copy's loop index,
/// every other type under the instance's arguments alone.
fn substituted_facts(
    template: &CheckedBodyFacts,
    InstanceSubstitution {
        types: substitution,
        packs,
        views,
        values,
        ..
    }: &InstanceSubstitution,
    indices: &ElementIndices,
    canonical: &dyn Fn(Ty) -> Ty,
) -> Result<CheckedBodyFacts, &'static str> {
    let substitute = |ty: &Ty| {
        canonical(mojito_types::types::substitute_packs(
            &fold_binder_views(ty, views),
            substitution,
            packs,
            values,
        ))
    };
    // A pack element's index binder is the copy's own at its occurrence.
    let substitute_at = |id: &OccurrenceId, ty: &Ty| {
        let values: Vec<_> = indices
            .get(id)
            .cloned()
            .into_iter()
            .chain(values.iter().cloned())
            .collect();
        canonical(mojito_types::types::substitute_packs(
            &fold_binder_views(ty, views),
            substitution,
            packs,
            &values,
        ))
    };
    let typed = |entries: &[(OccurrenceId, Ty)]| -> Vec<(OccurrenceId, Ty)> {
        entries
            .iter()
            .map(|(id, ty)| (*id, substitute_at(id, ty)))
            .collect()
    };
    let substituted_reference = |(id, reference): &(OccurrenceId, TemplateReference)| {
        (
            *id,
            TemplateReference {
                referent: substitute_at(id, &reference.referent),
                ..reference.clone()
            },
        )
    };
    Ok(CheckedBodyFacts {
        operation_adjustments: template
            .operation_adjustments
            .iter()
            .map(|(id, adjustment)| {
                kept_binder_construction(template, *id, adjustment, &substitute)
                    .or_else(|| {
                        mojito_checked::templates::derive_adjustment(adjustment, &substitute)
                    })
                    // An erased type argument names no type: a certificate
                    // admits one only where the syntax spells a closed type
                    // (`BodyShape::closed_type_argument`).
                    .or_else(|| {
                        matches!(
                            adjustment,
                            mojito_checked::checked::SemanticAdjustment::EraseCompileTimeArgument
                        )
                        .then(|| adjustment.clone())
                    })
                    .map(|derived| (*id, derived))
            })
            .collect::<Option<_>>()
            .ok_or("an operation adjustment has no derivation recipe")?,
        expression_types: typed(&template.expression_types),
        expression_place_types: typed(&template.expression_place_types),
        binding_types: typed(&template.binding_types),
        expression_effects: template
            .expression_effects
            .iter()
            .map(|(id, effects)| {
                (
                    *id,
                    mojito_checked::checked::EffectFacts {
                        raises: effects.raises.as_ref().map(&substitute),
                        ..effects.clone()
                    },
                )
            })
            .collect(),
        generic_instantiations: template
            .generic_instantiations
            .iter()
            .map(|(id, instantiation)| {
                (
                    *id,
                    mojito_checked::checked::GenericInstantiation {
                        arguments: mojito_types::types::map_tyargs(
                            &instantiation.arguments,
                            &substitute,
                        ),
                        ..instantiation.clone()
                    },
                )
            })
            .collect(),
        rebind_assertions: template
            .rebind_assertions
            .iter()
            .map(|(id, assertion)| {
                (
                    *id,
                    mojito_checked::templates::RebindAssertion {
                        operand: substitute_at(id, &assertion.operand),
                        dest: substitute_at(id, &assertion.dest),
                        by_value: assertion.by_value,
                    },
                )
            })
            .collect(),
        reference_results: template
            .reference_results
            .iter()
            .map(substituted_reference)
            .collect(),
        augmented_subscripts: substituted_element_stores(
            &template.augmented_subscripts,
            &substitute,
        )?,
        reference_binding_types: template
            .reference_binding_types
            .iter()
            .map(substituted_reference)
            .collect(),
        reference_place_types: template
            .reference_place_types
            .iter()
            .map(substituted_reference)
            .collect(),
        // A value-position read is under the same name in the instance;
        // every call's read is under the callee realized for it.
        effect_free_callees: template.value_callees.clone(),
        ..template.clone()
    })
}

/// The element stores of an instance: the template's, their types
/// substituted. The value getter a store embeds is realized with the call
/// at the site (`realize_element_getters`), and an in-place dunder
/// dispatched through a bound on the instance's element
/// (`realize_element_dunders`); any other dunder is kept as it stands, so
/// it must name only closed types.
fn substituted_element_stores(
    stores: &[(OccurrenceId, TemplateAugmentedSubscript)],
    substitute: &dyn Fn(&Ty) -> Ty,
) -> Result<Vec<(OccurrenceId, TemplateAugmentedSubscript)>, &'static str> {
    stores
        .iter()
        .map(|(id, store)| {
            let mut store = store.clone();
            let closed = store.inplace.as_ref().is_none_or(|call| {
                mojito_symbol::symbol::is_trait_dispatch_symbol(&call.contract.target)
                    || !mojito_types::types::is_symbolic(&call.contract.result_ty)
                        && call.contract.arguments.iter().all(|argument| {
                            !mojito_types::types::is_symbolic(&argument.parameter_ty)
                        })
            });
            if !closed {
                return Err("an element store embeds a call of a parameter type");
            }
            store.operand_ty = substitute(&store.operand_ty);
            store.result_ty = substitute(&store.result_ty);
            Ok((*id, store))
        })
        .collect()
}

/// The occurrence identities of the values a body's `return`s hand out.
fn returned_values(body: &[Stmt]) -> Vec<SyntaxId> {
    struct Returns(Vec<SyntaxId>);

    impl mojito_ast::visit::Visitor for Returns {
        fn visit_stmt(&mut self, statement: &Stmt) {
            if let StmtKind::Return(Some(value)) = &statement.kind {
                self.0.push(value.syntax_id);
            }
        }
    }
    let mut returns = Returns(Vec::new());
    mojito_ast::visit::walk_block(&mut returns, body);
    returns.0
}

/// Read each returned place the template handed out as a reference by
/// value instead: the place is copied where it lies, which the instance
/// owes at its own type (`realize_instance_facts`' copy check).
fn read_returns_by_value(facts: &mut CheckedBodyFacts, returned: &[SyntaxId]) {
    let (copied, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut facts.reference_value_uses)
        .into_iter()
        .partition(|(id, through)| !through && returned.contains(&id.syntax));
    facts.reference_value_uses = kept;
    for (id, _) in copied {
        push_unique(&mut facts.copy_place_value_uses, id);
    }
}
