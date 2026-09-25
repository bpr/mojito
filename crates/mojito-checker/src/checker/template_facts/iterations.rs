//! Re-selection of a runtime `for`'s iterator protocol for an instance.
//!
//! A loop records one `IterationProtocol` at its iterable: the `__iter__`
//! chain and `__next__` selected from the iterable's type (named for the
//! instance's clone family where one exists), the iterator's declared
//! projection, and — for a borrowed place — origins rooted at the source
//! binding. The recipe keeps what the selection is made from, not what it
//! found: the iterable's type, the loop's modes, the source place by
//! template owner, and that binding's mutability. An instance selects the
//! protocol again from its substituted iterable type
//! ([`Checker::realize_iterations`], obligation 21 of
//! `realize_instance_facts`) and resolves it against its own binding of the
//! source, exactly as the loop statement does (`loop_site_protocol`).

use super::Occurrence;
use crate::checker::Checker;
use mojito_checked::checked::IterationProtocol;
use mojito_checked::templates::{
    CheckedBodyFacts, IncompleteReason, OccurrenceId, TemplateIteration, TemplatePlace,
};
use mojito_common::error::TypeError;
use mojito_common::token::SourceSpan;
use mojito_types::origin::OriginPlace;
use mojito_types::types::Ty;

impl Checker {
    /// The recipe of each loop at the body's occurrences.
    ///
    /// Capture proves each recipe by rebuilding the template's own protocol
    /// from it and comparing with what inference recorded, so the recipe
    /// cannot drift from the loop statement's check. The source's mutability
    /// is read back the same way: the first value that reproduces the
    /// protocol, `true` where resolution never consulted it.
    pub(super) fn captured_iterations(
        &self,
        occurrences: &[Occurrence],
        local_place: &dyn Fn(&OriginPlace) -> Result<TemplatePlace, IncompleteReason>,
    ) -> Result<Vec<(OccurrenceId, TemplateIteration)>, IncompleteReason> {
        let protocols = self.iteration_protocols.borrow();
        let types = self.expression_types.borrow();
        occurrences
            .iter()
            .filter_map(|occurrence| {
                let recorded = protocols.get(&occurrence.span)?;
                Some(
                    self.captured_iteration(recorded, types.get(&occurrence.span), local_place)
                        .map(|iteration| (occurrence.id, iteration)),
                )
            })
            .collect()
    }

    /// Select each loop's protocol again from the instance's iterable type.
    ///
    /// The iterable must stay the same nominal struct, whose `__iter__` and
    /// `__next__` declarations are then the template's, or a variadic pack.
    /// Only the selection can fail for an instance: its clone family, its
    /// `where` clauses, and owned iteration's element bound. Resolution at the
    /// source rebinds origins and mutability, which installation does on the
    /// instance's own bindings. A loop over a type the instance keeps at
    /// another kind, or whose `__iter__` raises, refuses.
    pub(super) fn realize_iterations(
        &self,
        facts: &mut CheckedBodyFacts,
        substitute: &dyn Fn(&Ty) -> Ty,
    ) -> Result<(), &'static str> {
        for (_, iteration) in &mut facts.iterations {
            let iterable = substitute(&iteration.iterable);
            let same_kind = match (&iteration.iterable, &iterable) {
                (Ty::Struct(template, _), Ty::Struct(instance, _)) => template == instance,
                (Ty::VariadicPack(_), Ty::VariadicPack(_)) => true,
                _ => false,
            };
            if !same_kind {
                return Err("a loop's iterable changes kind for the instance");
            }
            let (_, raises) = self
                .loop_site_protocol(
                    &iterable,
                    iteration.mode,
                    iteration.binding,
                    None,
                    iteration.source_mutable,
                )
                .map_err(|_| "a loop's iterable has no iterator protocol for the instance")?;
            if !raises.is_empty() {
                return Err("a loop's iterator raises for the instance");
            }
            iteration.iterable = iterable;
        }
        Ok(())
    }

    /// Write each loop's protocol, selected from the instance's iterable and
    /// resolved against the instance's own binding of its source.
    pub(super) fn install_iterations(
        &self,
        facts: &CheckedBodyFacts,
        span: &dyn Fn(&OccurrenceId) -> Result<SourceSpan, TypeError>,
        rooted: &dyn Fn(&TemplatePlace) -> Result<OriginPlace, TypeError>,
    ) -> Result<(), TypeError> {
        for (id, iteration) in &facts.iterations {
            let source = iteration.source.as_ref().map(rooted).transpose()?;
            let (protocol, _) = self.loop_site_protocol(
                &iteration.iterable,
                iteration.mode,
                iteration.binding,
                source.as_ref(),
                iteration.source_mutable,
            )?;
            self.iteration_protocols
                .borrow_mut()
                .insert(span(id)?, protocol);
        }
        Ok(())
    }

    fn captured_iteration(
        &self,
        recorded: &IterationProtocol,
        iterable: Option<&Ty>,
        local_place: &dyn Fn(&OriginPlace) -> Result<TemplatePlace, IncompleteReason>,
    ) -> Result<TemplateIteration, IncompleteReason> {
        let iterable = iterable
            .filter(|ty| matches!(ty, Ty::Struct(..) | Ty::VariadicPack(_)))
            .ok_or(IncompleteReason::IterationRecipe)?;
        let binding = recorded
            .binding
            .as_ref()
            .ok_or(IncompleteReason::IterationRecipe)?
            .mode;
        // The attached origin is the source place extended by the iterator's
        // projection.
        let source = recorded
            .borrowed_origin
            .as_ref()
            .map(|origin| {
                origin
                    .path
                    .strip_suffix(recorded.yield_interior.as_slice())
                    .map(|path| OriginPlace {
                        root: origin.root,
                        path: path.to_vec(),
                    })
                    .ok_or(IncompleteReason::IterationRecipe)
            })
            .transpose()?;
        let source_mutable = [true, false]
            .into_iter()
            .find(|mutable| {
                self.loop_site_protocol(iterable, recorded.mode, binding, source.as_ref(), *mutable)
                    .is_ok_and(|(protocol, raises)| raises.is_empty() && protocol == *recorded)
            })
            .ok_or(IncompleteReason::IterationRecipe)?;
        Ok(TemplateIteration {
            mode: recorded.mode,
            binding,
            iterable: iterable.clone(),
            source: source.as_ref().map(local_place).transpose()?,
            source_mutable,
        })
    }
}
