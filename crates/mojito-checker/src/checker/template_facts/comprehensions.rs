//! Re-derivation of a comprehension's generator binders for an instance.
//!
//! A comprehension records one `CheckedComprehensionBinding` per generator
//! clause at the comprehension: the binder's owner, its type, the binding
//! plan of its clause's iterator protocol, and whether its storage is
//! droppable. All but the owner follow from the clause's protocol, which the
//! clause records at its iterable as a runtime `for` does and whose recipe is
//! a `TemplateIteration`. The recipe keeps the binder's identity by template
//! owner and the iterable whose protocol declares it. An instance selects
//! that protocol again (`realize_iterations`), and installation reads the
//! binder's plan from it and judges the binder's droppability at its type
//! ([`Checker::install_comprehension_bindings`]), exactly as the
//! comprehension's check does (`check_comprehension`).

use super::{Occurrence, without_struct_origins};
use crate::checker::Checker;
use mojito_checked::checked::CheckedComprehensionBinding;
use mojito_checked::templates::{
    CheckedBodyFacts, IncompleteReason, OccurrenceId, TemplateComprehensionBinding, TemplateOwner,
};
use mojito_common::error::TypeError;
use mojito_common::token::SourceSpan;
use mojito_types::origin::OwnerId;
use mojito_types::types::Ty;

impl Checker {
    /// The recipe of each comprehension at the body's occurrences.
    ///
    /// Capture proves each binder's recipe against what inference recorded:
    /// its plan must be the binding of the protocol its clause's iterable
    /// holds, its type that plan's binding type, and its droppability the
    /// judgment of that type. The iterable must be one of the body's
    /// occurrences.
    pub(super) fn captured_comprehension_bindings(
        &self,
        occurrences: &[Occurrence],
        local_owner: &dyn Fn(OwnerId) -> Result<TemplateOwner, IncompleteReason>,
    ) -> Result<Vec<(OccurrenceId, Vec<TemplateComprehensionBinding>)>, IncompleteReason> {
        let bindings = self.comprehension_bindings.borrow();
        let iterables = self.comprehension_iterables.borrow();
        occurrences
            .iter()
            .filter_map(|occurrence| {
                let recorded = bindings.get(&occurrence.span)?;
                let captured = iterables
                    .get(&occurrence.span)
                    .filter(|iterables| iterables.len() == recorded.len())
                    .ok_or(IncompleteReason::ComprehensionRecipe)
                    .and_then(|iterables| {
                        recorded
                            .iter()
                            .zip(iterables)
                            .map(|(binding, iterable)| {
                                self.captured_comprehension_binding(
                                    binding,
                                    iterable,
                                    occurrences,
                                    local_owner,
                                )
                            })
                            .collect()
                    });
                Some(captured.map(|binders| (occurrence.id, binders)))
            })
            .collect()
    }

    /// Substitute each binder's type, which only the method grammar reads:
    /// the binder's plan is selected again with its clause's protocol.
    pub(super) fn realize_comprehension_bindings(
        facts: &mut CheckedBodyFacts,
        substitute: &dyn Fn(&Ty) -> Ty,
    ) {
        for binder in facts
            .comprehension_bindings
            .iter_mut()
            .flat_map(|(_, binders)| binders)
        {
            binder.ty = substitute(&binder.ty);
        }
    }

    /// Write each comprehension's binders, each declared from the protocol
    /// installed at its clause's iterable and owned by the instance's own
    /// binding of it. Installation follows `install_iterations`.
    pub(super) fn install_comprehension_bindings(
        &self,
        facts: &CheckedBodyFacts,
        span: &dyn Fn(&OccurrenceId) -> Result<SourceSpan, TypeError>,
        owner: &dyn Fn(&TemplateOwner) -> Result<OwnerId, TypeError>,
    ) -> Result<(), TypeError> {
        for (id, binders) in &facts.comprehension_bindings {
            let mut bindings = Vec::with_capacity(binders.len());
            let mut iterables = Vec::with_capacity(binders.len());
            for binder in binders {
                let iterable = span(&binder.iterable)?;
                let plan = self
                    .iteration_protocols
                    .borrow()
                    .get(&iterable)
                    .and_then(|protocol| protocol.binding.as_deref().cloned())
                    .ok_or_else(|| {
                        TypeError::InvariantViolation(
                            "template derivation lost a comprehension clause's protocol".into(),
                        )
                    })?;
                bindings.push(CheckedComprehensionBinding {
                    name: binder.name.clone(),
                    owner: owner(&binder.owner)?,
                    ty: plan.binding_ty.clone(),
                    deinitable: self.is_deinitable(&plan.binding_ty),
                    plan,
                });
                iterables.push(iterable);
            }
            let site = span(id)?;
            self.comprehension_iterables
                .borrow_mut()
                .insert(site.clone(), iterables);
            self.comprehension_bindings
                .borrow_mut()
                .insert(site, bindings);
        }
        Ok(())
    }

    fn captured_comprehension_binding(
        &self,
        recorded: &CheckedComprehensionBinding,
        iterable: &SourceSpan,
        occurrences: &[Occurrence],
        local_owner: &dyn Fn(OwnerId) -> Result<TemplateOwner, IncompleteReason>,
    ) -> Result<TemplateComprehensionBinding, IncompleteReason> {
        let declared = self
            .iteration_protocols
            .borrow()
            .get(iterable)
            .and_then(|protocol| protocol.binding.as_deref())
            .is_some_and(|plan| *plan == recorded.plan);
        if !declared
            || recorded.ty != recorded.plan.binding_ty
            || self.is_deinitable(&recorded.ty) != recorded.deinitable
        {
            return Err(IncompleteReason::ComprehensionRecipe);
        }
        let iterable = occurrences
            .iter()
            .find(|occurrence| occurrence.span == *iterable)
            .map(|occurrence| occurrence.id)
            .ok_or(IncompleteReason::ComprehensionRecipe)?;
        let (reference, ty) = match &recorded.ty {
            Ty::Ref(reference) => (true, reference.referent.as_ref()),
            ty => (false, ty),
        };
        Ok(TemplateComprehensionBinding {
            name: recorded.name.clone(),
            owner: local_owner(recorded.owner)?,
            iterable,
            reference,
            ty: without_struct_origins(ty),
        })
    }
}
