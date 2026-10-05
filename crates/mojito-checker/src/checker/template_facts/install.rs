//! Installation of a realized fact bundle into the checker's own tables, as
//! if the body had been inferred.

use super::{BodyParams, bind_struct_origins, call_result_immutable_binders, checked_origin};
use crate::checker::Checker;
use crate::checker::body_carry::ObservedEffects;
use mojito_ast::ast::Stmt;
use mojito_checked::templates::{
    CheckedBodyFacts, FactTable, OccurrenceId, TemplateCallContract, TemplateInvalidation,
    TemplateOrigin, TemplateOwner, TemplatePlace, TemplateReference, TemplateTransferDest,
    TypedOrigins, TypedTable,
};
use mojito_common::error::TypeError;
use mojito_common::token::SourceSpan;
use mojito_types::origin::OwnerId;
use mojito_types::types::Ty;
use std::collections::HashMap;

impl Checker {
    /// Remove one occurrence's entry from every occurrence-keyed fact table.
    /// `replace_body_facts` checks the list against [`FactTable::ALL`]; the
    /// augmented store drops what it recorded at a synthesized receiver.
    pub(in crate::checker) fn remove_occurrence_facts(&self, span: &SourceSpan) {
        self.overload_targets.borrow_mut().remove(span);
        self.contextual_bases.borrow_mut().remove(span);
        self.generic_instantiations.borrow_mut().remove(span);
        self.method_instantiations.borrow_mut().remove(span);
        self.call_transfers.borrow_mut().remove(span);
        self.implicit_conversions.borrow_mut().remove(span);
        self.implicit_conversion_types.borrow_mut().remove(span);
        self.implicit_conversion_raises.borrow_mut().remove(span);
        self.conversion_source_borrows.borrow_mut().remove(span);
        self.simd_constructions.borrow_mut().remove(span);
        self.parameterized_method_calls.borrow_mut().remove(span);
        self.operation_adjustments.borrow_mut().remove(span);
        self.construction_immutable_binders
            .borrow_mut()
            .remove(span);
        self.call_result_origins.borrow_mut().remove(span);
        self.tuple_unpack_plans.borrow_mut().remove(span);
        self.tuple_unpack_sources.borrow_mut().remove(span);
        self.interior_references.borrow_mut().remove(span);
        self.view_result_interiors.borrow_mut().remove(span);
        self.call_parameters.borrow_mut().remove(span);
        self.interior_invalidations.borrow_mut().remove(span);
        self.expression_types.borrow_mut().remove(span);
        self.expression_bindings.borrow_mut().remove(span);
        self.statement_bindings.borrow_mut().remove(span);
        self.with_desugars.borrow_mut().remove(span);
        self.declaration_captures.borrow_mut().remove(span);
        self.comprehension_bindings.borrow_mut().remove(span);
        self.comprehension_iterables.borrow_mut().remove(span);
        self.nested_def_params.borrow_mut().remove(span);
        self.expression_place_types.borrow_mut().remove(span);
        self.binding_types.borrow_mut().remove(span);
        self.expression_effects.borrow_mut().remove(span);
        self.selected_calls.borrow_mut().remove(span);
        self.subscript_descriptors.borrow_mut().remove(span);
        self.iteration_protocols.borrow_mut().remove(span);
        self.explicit_destroy_calls.borrow_mut().remove(span);
        self.reference_value_uses.borrow_mut().remove(span);
        self.copyable_reference_result_reads
            .borrow_mut()
            .remove(span);
        self.discarded_reference_results.borrow_mut().remove(span);
        self.borrowed_reference_receivers.borrow_mut().remove(span);
        self.copy_place_value_uses.borrow_mut().remove(span);
        self.call_place_uses.borrow_mut().remove(span);
        self.borrowed_read_call_places.borrow_mut().remove(span);
        self.read_temporary_arguments.borrow_mut().remove(span);
        self.unconsumed_temporaries.borrow_mut().remove(span);
        self.linear_temporaries.borrow_mut().remove(span);
        self.implicitly_copied_consuming_receivers
            .borrow_mut()
            .remove(span);
        self.truthiness_conditions.borrow_mut().remove(span);
        let mut deletability = self.explicit_destroy_deletability.borrow_mut();
        deletability.bindings.remove(span);
        deletability.linear_bindings.remove(span);
        self.rebind_assertions.borrow_mut().remove(span);
    }

    /// Install a realized bundle as if the body had been inferred: each fact
    /// at the occurrence that kept the template occurrence's identity, each
    /// binding at the body's own parameter, a fresh local, or the named
    /// module-scope declaration, and each callee summary the body depends on
    /// observed as empty so the transfer fixpoint re-runs if it grows.
    pub(super) fn install_body_facts(
        &self,
        facts: &CheckedBodyFacts,
        spans: &HashMap<OccurrenceId, SourceSpan>,
        param_owners: &BodyParams,
    ) -> Result<(), TypeError> {
        let corrupt =
            |what: &str| TypeError::InvariantViolation(format!("template derivation lost {what}"));
        let local_start = self
            .reserve_owners(facts.locals)
            .map_err(|_| corrupt("its binding identity range"))?;
        let owner = |owner: &TemplateOwner| match owner {
            TemplateOwner::Param(index) => param_owners
                .runtime
                .get(*index)
                .copied()
                .flatten()
                .ok_or_else(|| corrupt("a parameter binding")),
            TemplateOwner::Receiver => param_owners
                .receiver
                .ok_or_else(|| corrupt("the receiver binding")),
            TemplateOwner::CompileTimeParam(name) => param_owners
                .compile_time
                .iter()
                .find(|(kept, _)| kept == name)
                .map(|(_, owner)| *owner)
                .ok_or_else(|| corrupt("the fold of a compile-time parameter")),
            TemplateOwner::Local(index) => Ok(OwnerId(local_start + index)),
            TemplateOwner::Global(name) => self
                .owner_scopes
                .first()
                .and_then(|globals| globals.get(name))
                .copied()
                .ok_or_else(|| corrupt("a module-scope binding")),
        };
        let span = |id: &OccurrenceId| {
            spans
                .get(id)
                .cloned()
                .ok_or_else(|| corrupt("a body occurrence"))
        };
        let rooted = |place: &TemplatePlace| {
            Ok::<_, TypeError>(mojito_types::origin::OriginPlace {
                root: owner(&place.root)?,
                path: place.path.clone(),
            })
        };
        // A struct type kept with its origin slots unbound gets the
        // instance's own bindings back in them.
        let typed = |table: TypedTable, id: &OccurrenceId, ty: &Ty| {
            facts
                .typed_origins
                .iter()
                .find(|typed| typed.table == table && typed.occurrence == *id)
                .map_or_else(
                    || Ok(ty.clone()),
                    |typed| bind_typed_origins(ty, typed, &rooted),
                )
        };
        for (id, ty) in &facts.expression_types {
            self.expression_types
                .borrow_mut()
                .insert(span(id)?, typed(TypedTable::Expression, id, ty)?);
        }
        for (id, ty) in &facts.expression_place_types {
            self.expression_place_types
                .borrow_mut()
                .insert(span(id)?, typed(TypedTable::Place, id, ty)?);
        }
        for (id, ty) in &facts.binding_types {
            self.binding_types
                .borrow_mut()
                .insert(span(id)?, typed(TypedTable::Binding, id, ty)?);
        }
        for (id, binding) in &facts.expression_bindings {
            self.expression_bindings
                .borrow_mut()
                .insert(span(id)?, owner(binding)?);
        }
        for (id, binding) in &facts.statement_bindings {
            self.statement_bindings
                .borrow_mut()
                .insert(span(id)?, owner(binding)?);
        }
        for (id, effects) in &facts.expression_effects {
            self.expression_effects
                .borrow_mut()
                .insert(span(id)?, effects.clone());
        }
        for (id, adjustment) in &facts.operation_adjustments {
            self.operation_adjustments
                .borrow_mut()
                .insert(span(id)?, adjustment.clone());
        }
        let referenced = |reference: &TemplateReference| {
            Ok::<_, TypeError>(mojito_types::origin::RefTy {
                referent: Box::new(reference.referent.clone()),
                origin: checked_origin(&reference.origin, &rooted)?,
                mutability: reference.mutability,
            })
        };
        for (id, reference) in &facts.reference_results {
            self.operation_adjustments.borrow_mut().insert(
                span(id)?,
                mojito_checked::checked::SemanticAdjustment::ReferenceResult {
                    reference: referenced(reference)?,
                },
            );
        }
        for (id, place) in &facts.interior_references {
            self.interior_references
                .borrow_mut()
                .insert(span(id)?, rooted(place)?);
        }
        self.install_call_results(facts, &span, &rooted)?;
        self.install_transfers(facts, &span, &rooted, &owner)?;
        for (id, reference) in &facts.reference_binding_types {
            self.binding_types
                .borrow_mut()
                .insert(span(id)?, Ty::Ref(referenced(reference)?));
        }
        for (id, reference) in &facts.reference_place_types {
            self.expression_place_types
                .borrow_mut()
                .insert(span(id)?, Ty::Ref(referenced(reference)?));
        }
        for id in &facts.copyable_reference_result_reads {
            self.copyable_reference_result_reads
                .borrow_mut()
                .insert(span(id)?);
        }
        for (id, instantiation) in &facts.generic_instantiations {
            self.generic_instantiations
                .borrow_mut()
                .insert(span(id)?, instantiation.clone());
        }
        for (id, target) in &facts.overload_targets {
            self.overload_targets
                .borrow_mut()
                .insert(span(id)?, target.clone());
        }
        self.install_conversions(
            &facts
                .conversions
                .iter()
                .map(|(id, conversion)| Ok((span(id)?, conversion)))
                .collect::<Result<Vec<_>, TypeError>>()?,
        );
        for (id, parameters) in &facts.call_parameters {
            self.call_parameters.borrow_mut().insert(
                span(id)?,
                parameters
                    .iter()
                    .map(|parameter| crate::checker::CallParameter {
                        name: parameter.name.clone(),
                        convention: parameter.convention,
                        ty: parameter.ty.clone(),
                    })
                    .collect(),
            );
        }
        for id in &facts.borrowed_read_call_places {
            self.borrowed_read_call_places
                .borrow_mut()
                .insert(span(id)?);
        }
        for id in &facts.borrowed_reference_receivers {
            self.borrowed_reference_receivers
                .borrow_mut()
                .insert(span(id)?);
        }
        for (id, descriptors) in &facts.subscript_descriptors {
            self.subscript_descriptors
                .borrow_mut()
                .insert(span(id)?, descriptors.clone());
        }
        for (id, dimensions) in &facts.simd_constructions {
            self.simd_constructions
                .borrow_mut()
                .insert(span(id)?, dimensions.clone());
        }
        for (id, base) in &facts.contextual_bases {
            self.contextual_bases
                .borrow_mut()
                .insert(span(id)?, base.clone());
        }
        for (id, decls) in &facts.parameterized_method_calls {
            self.parameterized_method_calls
                .borrow_mut()
                .insert(span(id)?, decls.clone());
        }
        self.install_iterations(facts, &span, &rooted)?;
        self.install_comprehension_bindings(facts, &span, &owner)?;
        self.install_nested_defs(facts, &span, &owner, &rooted)?;
        self.install_tuple_unpacks(facts, &span, &referenced)?;
        for id in &facts.call_place_uses {
            self.call_place_uses.borrow_mut().insert(span(id)?);
        }
        for id in &facts.read_temporary_arguments {
            self.read_temporary_arguments.borrow_mut().insert(span(id)?);
        }
        for (id, assertion) in &facts.rebind_assertions {
            self.rebind_assertions
                .borrow_mut()
                .insert(span(id)?, assertion.clone());
        }
        for id in &facts.copy_place_value_uses {
            self.copy_place_value_uses.borrow_mut().insert(span(id)?);
        }
        let placed = |invalidations: &[TemplateInvalidation]| {
            invalidations
                .iter()
                .map(|invalidation| {
                    Ok(mojito_checked::checked::InteriorInvalidation {
                        base: mojito_types::origin::OriginPlace {
                            root: owner(&invalidation.root)?,
                            path: invalidation.path.clone(),
                        },
                        except: invalidation.except.as_ref().map(&owner).transpose()?,
                        include_base_generation: invalidation.include_base_generation,
                    })
                })
                .collect::<Result<Vec<_>, TypeError>>()
        };
        for (id, invalidations) in &facts.interior_invalidations {
            self.interior_invalidations
                .borrow_mut()
                .insert(span(id)?, placed(invalidations)?);
        }
        for (id, instantiation) in &facts.method_instantiations {
            self.method_instantiations
                .borrow_mut()
                .insert(span(id)?, instantiation.clone());
        }
        // A construction's immutable-binder record: the kept one where it
        // binds a slot immutably, else the empty one `infer_construction`
        // writes at every construction.
        for id in &facts.constructions {
            self.construction_immutable_binders
                .borrow_mut()
                .insert(span(id)?, Vec::new());
        }
        for (id, binders) in &facts.construction_immutable_binders {
            self.construction_immutable_binders
                .borrow_mut()
                .insert(span(id)?, binders.clone());
        }
        for (id, writable) in &facts.reference_value_uses {
            self.reference_value_uses
                .borrow_mut()
                .insert(span(id)?, *writable);
        }
        self.install_occurrence_marks(facts, &span)?;
        let checked_contract = |call: &TemplateCallContract| {
            checked_contract(call, &span, &placed, &rooted, &referenced)
        };
        for (id, call) in &facts.selected_calls {
            self.selected_calls
                .borrow_mut()
                .insert(span(id)?, checked_contract(call)?);
        }
        self.install_element_stores(facts, &span, &checked_contract)?;
        self.install_inplace_updates(facts, &span, &checked_contract)?;
        // The body's own source decides, exactly as it does for an inferred
        // body, whether an application it reaches is user-reachable.
        let source = spans.values().next().and_then(|span| span.source.clone());
        for (template, arguments) in &facts.struct_applications {
            self.record_struct_instantiation(template, arguments, source.as_deref());
        }
        for callee in &facts.effect_free_callees {
            self.note_body_effect_read(callee, ObservedEffects::Transfers(Vec::new()));
            self.note_body_effect_read(callee, ObservedEffects::CallThroughs(Vec::new()));
            self.effect_observations
                .borrow_mut()
                .entry(callee.clone())
                .or_default();
            self.call_through_observations
                .borrow_mut()
                .entry(callee.clone())
                .or_default();
        }
        for (callee, residue) in &facts.call_through_reads {
            self.note_body_effect_read(callee, ObservedEffects::CallThroughs(residue.clone()));
            self.call_through_observations
                .borrow_mut()
                .entry(callee.clone())
                .or_insert_with(|| residue.clone());
        }
        for (callee, read) in &facts.transfer_reads {
            self.note_body_effect_read(callee, ObservedEffects::Transfers(read.clone()));
            self.note_body_effect_read(callee, ObservedEffects::CallThroughs(Vec::new()));
            self.effect_observations
                .borrow_mut()
                .entry(callee.clone())
                .or_insert_with(|| read.clone());
            self.call_through_observations
                .borrow_mut()
                .entry(callee.clone())
                .or_default();
        }
        // The residue goes on the body's own frame, which publishes it under
        // the body's key when it is popped, as an inferred body's would be.
        if let Some(frame) = self.transfer_frames.borrow_mut().last_mut() {
            for residue in &facts.call_throughs {
                if !frame.call_throughs.contains(residue) {
                    frame.call_throughs.push(residue.clone());
                }
            }
        }
        Ok(())
    }

    /// Discard what inferring `body` recorded and install `facts` in its
    /// place.
    pub(super) fn replace_body_facts(
        &self,
        body: &[Stmt],
        facts: &CheckedBodyFacts,
        param_owners: &BodyParams,
    ) -> Result<(), TypeError> {
        let occurrences = self.body_occurrences(body);
        // A `with` desugar is the body's syntax, which the derived facts
        // describe; it outlives the clear.
        let desugars: Vec<_> = {
            let recorded = self.with_desugars.borrow();
            occurrences
                .iter()
                .filter_map(|occurrence| {
                    let desugar = recorded.get(&occurrence.span)?;
                    Some((occurrence.span.clone(), desugar.clone()))
                })
                .collect()
        };
        for occurrence in &occurrences {
            self.remove_occurrence_facts(&occurrence.span);
        }
        if let Some(table) = FactTable::ALL.into_iter().find(|table| {
            let entries = self.span_table(*table);
            occurrences
                .iter()
                .any(|occurrence| entries.has(&occurrence.span))
        }) {
            return Err(TypeError::InvariantViolation(format!(
                "template derivation: {table:?} is not cleared by remove_occurrence_facts"
            )));
        }
        self.install_body_facts(
            facts,
            &occurrences
                .into_iter()
                .map(|occurrence| (occurrence.id, occurrence.span))
                .collect(),
            param_owners,
        )?;
        self.with_desugars.borrow_mut().extend(desugars);
        Ok(())
    }

    /// Write each realized conversion back into the four conversion tables,
    /// as `record_selected_conversion` writes one: the converted-to type, the
    /// error type, and the source borrow only where the selection has them.
    fn install_conversions(
        &self,
        conversions: &[(SourceSpan, &mojito_checked::templates::TemplateConversion)],
    ) {
        for (site, conversion) in conversions {
            self.implicit_conversions
                .borrow_mut()
                .insert(site.clone(), conversion.target.clone());
            if let Some(result) = &conversion.result {
                self.implicit_conversion_types
                    .borrow_mut()
                    .insert(site.clone(), result.clone());
            }
            if let Some(raises) = &conversion.raises {
                self.implicit_conversion_raises
                    .borrow_mut()
                    .insert(site.clone(), raises.clone());
            }
            if let Some(mutable) = conversion.source_borrow {
                self.conversion_source_borrows
                    .borrow_mut()
                    .insert(site.clone(), mutable);
            }
        }
    }

    /// Install what a view-returning call records about its result: the
    /// origins its contract binds, the immutable binders those imply, and
    /// the owned-interior tags its callee's return origin projects.
    fn install_call_results(
        &self,
        facts: &CheckedBodyFacts,
        span: &dyn Fn(&OccurrenceId) -> Result<SourceSpan, TypeError>,
        rooted: &dyn Fn(&TemplatePlace) -> Result<mojito_types::origin::OriginPlace, TypeError>,
    ) -> Result<(), TypeError> {
        for (id, slots) in &facts.call_result_origins {
            let resolved = slots
                .iter()
                .map(|resolved| {
                    Ok((
                        resolved.slot,
                        checked_origin(&resolved.origin, rooted)?,
                        resolved.mutability,
                    ))
                })
                .collect::<Result<Vec<_>, TypeError>>()?;
            let binders = call_result_immutable_binders(&resolved);
            if !binders.is_empty() {
                self.construction_immutable_binders
                    .borrow_mut()
                    .insert(span(id)?, binders);
            }
            self.call_result_origins
                .borrow_mut()
                .insert(span(id)?, resolved);
        }
        for (id, tags) in &facts.view_result_interiors {
            self.view_result_interiors
                .borrow_mut()
                .insert(span(id)?, tags.clone());
        }
        Ok(())
    }

    /// Install a derivation's replayed transfers: the call transfers at
    /// their spans, the merged origins at the instance's own bindings, and
    /// the effects on the body's frame, which publishes them under the body's
    /// key when it is popped, as an inferred body's would be.
    fn install_transfers(
        &self,
        facts: &CheckedBodyFacts,
        span: &dyn Fn(&OccurrenceId) -> Result<SourceSpan, TypeError>,
        rooted: &dyn Fn(&TemplatePlace) -> Result<mojito_types::origin::OriginPlace, TypeError>,
        owner: &dyn Fn(&TemplateOwner) -> Result<OwnerId, TypeError>,
    ) -> Result<(), TypeError> {
        use mojito_checked::checked::{CheckedCallTransfer, CheckedTransferDest};
        for (id, transfers) in &facts.call_transfers {
            let resolved = transfers
                .iter()
                .map(|transfer| {
                    Ok(CheckedCallTransfer {
                        dest: match transfer.dest {
                            TemplateTransferDest::Receiver => CheckedTransferDest::Receiver,
                            TemplateTransferDest::Argument(index) => {
                                CheckedTransferDest::Argument(index)
                            }
                        },
                        dest_path: transfer.dest_path.clone(),
                        sources: transfer
                            .sources
                            .iter()
                            .map(|source| checked_origin(&source.origin, rooted))
                            .collect::<Result<_, _>>()?,
                        mutable: transfer.mutable,
                    })
                })
                .collect::<Result<Vec<_>, TypeError>>()?;
            self.call_transfers.borrow_mut().insert(span(id)?, resolved);
        }
        for (dest, sources) in &facts.transferred_origins {
            let dest = owner(dest)?;
            let origins = sources
                .iter()
                .map(|source| checked_origin(&source.origin, rooted))
                .collect::<Result<Vec<_>, TypeError>>()?;
            let mut overlay = self.transferred_origins.borrow_mut();
            let merged = overlay.entry(dest).or_default();
            for origin in origins {
                if !merged.contains(&origin) {
                    merged.push(origin);
                }
            }
        }
        if let Some(frame) = self.transfer_frames.borrow_mut().last_mut() {
            for effect in &facts.transfer_effects {
                frame.record(effect.effect.clone(), effect.latent.clone());
            }
        }
        Ok(())
    }

    /// Install the bare per-occurrence marks: temporaries no one consumes,
    /// discarded results, explicit-destroy calls, consuming calls on a copied
    /// receiver, truthiness conditions, and each binding's deletability.
    fn install_occurrence_marks(
        &self,
        facts: &CheckedBodyFacts,
        span: &dyn Fn(&OccurrenceId) -> Result<SourceSpan, TypeError>,
    ) -> Result<(), TypeError> {
        for id in &facts.unconsumed_temporaries {
            self.unconsumed_temporaries.borrow_mut().insert(span(id)?);
        }
        for id in &facts.discarded_reference_results {
            self.discarded_reference_results
                .borrow_mut()
                .insert(span(id)?);
        }
        for id in &facts.explicit_destroy_calls {
            self.explicit_destroy_calls.borrow_mut().insert(span(id)?);
        }
        for id in &facts.implicitly_copied_consuming_receivers {
            self.implicitly_copied_consuming_receivers
                .borrow_mut()
                .insert(span(id)?);
        }
        for id in &facts.linear_temporaries {
            self.linear_temporaries.borrow_mut().insert(span(id)?);
        }
        for id in &facts.truthiness_conditions {
            self.truthiness_conditions.borrow_mut().insert(span(id)?);
        }
        let mut deletability = self.explicit_destroy_deletability.borrow_mut();
        for id in &facts.deletable_bindings {
            deletability.bindings.insert(span(id)?);
        }
        for id in &facts.linear_bindings {
            deletability.linear_bindings.insert(span(id)?);
        }
        Ok(())
    }

    /// Install each element store: the call installed at its site is its
    /// getter for a store through a reference and its setter otherwise, and
    /// the adjustment overwrites that call's reference as the checker's own
    /// insert does. A store through a setter binds its computed value at
    /// the site.
    fn install_element_stores(
        &self,
        facts: &CheckedBodyFacts,
        span: &dyn Fn(&OccurrenceId) -> Result<SourceSpan, TypeError>,
        checked_contract: &dyn Fn(
            &TemplateCallContract,
        ) -> Result<
            mojito_checked::checked::CheckedCallContract,
            TypeError,
        >,
    ) -> Result<(), TypeError> {
        for (id, store) in &facts.augmented_subscripts {
            let site = span(id)?;
            let installed = self
                .selected_calls
                .borrow()
                .get(&site)
                .cloned()
                .ok_or_else(|| {
                    TypeError::InvariantViolation(
                        "a derived element store has no call at its site".to_string(),
                    )
                })?;
            let (getter, setter) = match &store.getter {
                Some(getter) => (checked_contract(getter)?, Some(installed)),
                None => (installed, None),
            };
            let value_source = setter.is_some().then(|| site.clone());
            self.operation_adjustments.borrow_mut().insert(
                site,
                mojito_checked::checked::SemanticAdjustment::AugmentedSubscript(Box::new(
                    mojito_checked::checked::CheckedAugmentedSubscript {
                        getter,
                        setter,
                        inplace: store.inplace.as_ref().map(checked_contract).transpose()?,
                        operand_ty: store.operand_ty.clone(),
                        result_ty: store.result_ty.clone(),
                        value_source,
                    },
                )),
            );
        }
        Ok(())
    }

    /// Install each in-place update as the adjustment at its place, as the
    /// checker's own selection records it.
    fn install_inplace_updates(
        &self,
        facts: &CheckedBodyFacts,
        span: &dyn Fn(&OccurrenceId) -> Result<SourceSpan, TypeError>,
        checked_contract: &dyn Fn(
            &TemplateCallContract,
        ) -> Result<
            mojito_checked::checked::CheckedCallContract,
            TypeError,
        >,
    ) -> Result<(), TypeError> {
        for (id, call) in &facts.inplace_updates {
            self.record_augmented_in_place(span(id)?, checked_contract(call)?);
        }
        Ok(())
    }
}

/// The inverse of [`typed_origins`]: a kept pointer's provenance rooted at
/// the instance's own place, then its struct origins written back.
fn bind_typed_origins(
    ty: &Ty,
    typed: &TypedOrigins,
    rooted: &dyn Fn(&TemplatePlace) -> Result<mojito_types::origin::OriginPlace, TypeError>,
) -> Result<Ty, TypeError> {
    use mojito_types::origin::PointerOrigin;
    let (Some(mutable), Ty::Pointer { element, .. }, [TemplateOrigin::Place(pointee), rest @ ..]) =
        (typed.pointer, ty, typed.origins.as_slice())
    else {
        return bind_struct_origins(ty, &typed.origins, rooted);
    };
    Ok(Ty::Pointer {
        element: Box::new(bind_struct_origins(element, rest, rooted)?),
        origin: PointerOrigin::Place {
            place: rooted(pointee)?,
            mutable,
        },
    })
}

/// One kept call's contract under an instance's own spans and bindings: the
/// inverse of [`local_contract`].
fn checked_contract(
    call: &TemplateCallContract,
    span: &dyn Fn(&OccurrenceId) -> Result<SourceSpan, TypeError>,
    placed: &PlacedInvalidations<'_>,
    rooted: &dyn Fn(&TemplatePlace) -> Result<mojito_types::origin::OriginPlace, TypeError>,
    referenced: &dyn Fn(&TemplateReference) -> Result<mojito_types::origin::RefTy, TypeError>,
) -> Result<mojito_checked::checked::CheckedCallContract, TypeError> {
    let arguments = call
        .arguments
        .iter()
        .map(|argument| {
            Ok(mojito_checked::checked::CheckedCallArgumentBoundary {
                source: argument.source,
                value_source: span(&argument.value)?,
                adjustments: argument.adjustments.clone(),
                invalidations: placed(&argument.invalidations)?,
            })
        })
        .collect::<Result<Vec<_>, TypeError>>()?;
    let reference_result = call.reference_result.as_ref().map(referenced).transpose()?;
    Ok(mojito_checked::checked::CheckedCallContract {
        result_ty: match reference_result.clone() {
            Some(reference) => Ty::Ref(reference),
            None if call.result_origins.is_empty() => call.contract.result_ty.clone(),
            None => bind_struct_origins(&call.contract.result_ty, &call.result_origins, rooted)?,
        },
        reference_result,
        boundary: mojito_checked::checked::CheckedCallBoundary {
            arguments,
            invalidations: placed(&call.invalidations)?,
        },
        ..call.contract.clone()
    })
}

/// Kept invalidations under an instance's own bindings.
type PlacedInvalidations<'a> = dyn Fn(
        &[TemplateInvalidation],
    ) -> Result<Vec<mojito_checked::checked::InteriorInvalidation>, TypeError>
    + 'a;
