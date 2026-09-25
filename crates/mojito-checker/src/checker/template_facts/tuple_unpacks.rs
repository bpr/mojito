//! Re-derivation of a tuple unpacking's element reads for an instance.
//!
//! An unpacking records one plan at its value: each element's checked type
//! and, for a generated Tuple, the accessor that reads it and the reference
//! a place accessor yields. The recipe keeps what the plan is built from, not
//! what it holds: the value's type, the reference the unpacked place yields
//! (`None` for a temporary), and the statement's named targets. An
//! instance substitutes the value's type, which names the generated Tuple
//! the clone check selects (`canonicalize_public_tuple_types`), and builds
//! the plan again from it and its own binding of the place
//! ([`Checker::realize_tuple_unpacks`], [`Checker::install_tuple_unpacks`]),
//! exactly as the statement does (`tuple_unpack_plan`).

use super::Occurrence;
use crate::checker::Checker;
use mojito_checked::checked::CheckedTupleUnpackElement;
use mojito_checked::templates::{
    CheckedBodyFacts, IncompleteReason, OccurrenceId, TemplateReference, TemplateTupleUnpack,
};
use mojito_common::error::TypeError;
use mojito_common::token::SourceSpan;
use mojito_types::origin::RefTy;
use mojito_types::types::Ty;

impl Checker {
    /// The recipe of each unpacking at the body's occurrences.
    ///
    /// Capture proves each recipe by building the template's own plan from
    /// it and comparing with what inference recorded, so the recipe cannot
    /// drift from the statement's check. A named target must be one of the
    /// body's occurrences.
    pub(super) fn captured_tuple_unpacks(
        &self,
        occurrences: &[Occurrence],
        local_reference: &dyn Fn(&RefTy) -> Result<TemplateReference, IncompleteReason>,
    ) -> Result<Vec<(OccurrenceId, TemplateTupleUnpack)>, IncompleteReason> {
        let plans = self.tuple_unpack_plans.borrow();
        occurrences
            .iter()
            .filter_map(|occurrence| {
                let recorded = plans.get(&occurrence.span)?;
                Some(
                    self.captured_tuple_unpack(
                        &occurrence.span,
                        recorded,
                        occurrences,
                        local_reference,
                    )
                    .map(|unpack| (occurrence.id, unpack)),
                )
            })
            .collect()
    }

    /// Derive each unpacking's value type for the instance, and prove its
    /// plan builds there.
    ///
    /// The value must stay a tuple. Only the accessor lookup can fail for an
    /// instance: a temporary of a generated Tuple is read through value
    /// accessors, which exist only for implicitly copyable elements. The
    /// place's origin does not decide whether the plan builds, so the proof
    /// roots it nowhere; installation roots it at the instance's binding.
    pub(super) fn realize_tuple_unpacks(
        &self,
        facts: &mut CheckedBodyFacts,
        substitute: &dyn Fn(&Ty) -> Ty,
    ) -> Result<(), &'static str> {
        for (_, unpack) in &mut facts.tuple_unpacks {
            unpack.value = substitute(&unpack.value);
            if let Some(source) = &mut unpack.source {
                source.referent = substitute(&source.referent);
            }
            let proof = unpack.source.as_ref().map(|source| RefTy {
                referent: Box::new(source.referent.clone()),
                origin: mojito_types::origin::Origin::Static,
                mutability: source.mutability,
            });
            self.tuple_unpack_plan(&unpack.value, proof.is_some(), proof.as_ref())
                .map_err(|_| "a tuple unpacking has no element reads for the instance")?;
        }
        Ok(())
    }

    /// Write each unpacking's plan, built from the instance's value type and
    /// its own binding of the unpacked place.
    pub(super) fn install_tuple_unpacks(
        &self,
        facts: &CheckedBodyFacts,
        span: &dyn Fn(&OccurrenceId) -> Result<SourceSpan, TypeError>,
        referenced: &dyn Fn(&TemplateReference) -> Result<RefTy, TypeError>,
    ) -> Result<(), TypeError> {
        for (id, unpack) in &facts.tuple_unpacks {
            let source = unpack.source.as_ref().map(referenced).transpose()?;
            let plan = self.tuple_unpack_plan(&unpack.value, source.is_some(), source.as_ref())?;
            self.tuple_unpack_plans.borrow_mut().insert(span(id)?, plan);
        }
        Ok(())
    }

    fn captured_tuple_unpack(
        &self,
        value: &SourceSpan,
        recorded: &[CheckedTupleUnpackElement],
        occurrences: &[Occurrence],
        local_reference: &dyn Fn(&RefTy) -> Result<TemplateReference, IncompleteReason>,
    ) -> Result<TemplateTupleUnpack, IncompleteReason> {
        let sources = self.tuple_unpack_sources.borrow();
        let source = sources
            .get(value)
            .ok_or(IncompleteReason::TupleUnpackRecipe)?;
        let value_ty = self
            .expression_types
            .borrow()
            .get(value)
            .cloned()
            .ok_or(IncompleteReason::TupleUnpackRecipe)?;
        let rebuilt = self
            .tuple_unpack_plan(
                &value_ty,
                source.reference.is_some(),
                source.reference.as_ref(),
            )
            .map_err(|_| IncompleteReason::TupleUnpackRecipe)?;
        if rebuilt != recorded {
            return Err(IncompleteReason::TupleUnpackRecipe);
        }
        let targets = source
            .targets
            .iter()
            .map(|target| {
                occurrences
                    .iter()
                    .find(|occurrence| occurrence.span == *target)
                    .map(|occurrence| occurrence.id)
                    .ok_or(IncompleteReason::TupleUnpackRecipe)
            })
            .collect::<Result<_, _>>()?;
        Ok(TemplateTupleUnpack {
            value: value_ty,
            source: source.reference.as_ref().map(local_reference).transpose()?,
            targets,
            declares: source.declares,
        })
    }
}
