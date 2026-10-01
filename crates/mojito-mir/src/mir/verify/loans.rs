//! Loan-generation and interior-invalidation verification.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

pub(super) fn verify_loan_instruction(
    cx: &InstrCx<'_>,
    instruction: &MirInstr,
    errors: &mut Vec<String>,
) {
    let prefix = cx.prefix;
    match instruction {
        MirInstr::EstablishLoans {
            reference,
            loans,
            dest_interior,
            ..
        } => {
            if *reference as usize >= cx.function.n_vars {
                errors.push(format!(
                    "{prefix}: loan generation has invalid reference slot {reference}"
                ));
            }
            if loans.is_empty() {
                errors.push(format!("{prefix}: loan generation has no owner loans"));
            }
            if let Some(domain) = dest_interior {
                if domain.root != *reference {
                    errors.push(format!(
                        "{prefix}: loan destination domain roots at slot {} instead of the \
                         generation's reference slot {reference}",
                        domain.root
                    ));
                }
                if domain.path.is_empty() {
                    errors.push(format!(
                        "{prefix}: loan destination domain has an empty interior path"
                    ));
                }
                // Transfer destinations name exact interior generations; the
                // conservative subtree form never designates a store target.
                if domain
                    .path
                    .iter()
                    .any(|segment| matches!(segment, mojito_types::origin::OriginSeg::Subtree))
                {
                    errors.push(format!(
                        "{prefix}: loan destination domain contains a subtree segment"
                    ));
                }
            }
            for loan in loans {
                let through_capability = loan
                    .place
                    .through
                    .and_then(|through| cx.function.var_tys.get(&through))
                    .and_then(reference_capability);
                let place_capability = make_ref_target(&loan.place);
                let permission = through_capability
                    .map(|capability| capability.permission)
                    .or_else(|| place_capability.and_then(|(_, permission)| permission));
                if loan.mutable
                    && permission.is_some_and(|permission| {
                        !permission.satisfies(ReferencePermission::Mutable)
                    })
                {
                    errors.push(format!(
                        "{prefix}: mutable loan recovers permission unavailable through its source capability"
                    ));
                }
                // A through-reference handle designates a prefix of the loan
                // place — the root storage, or the result of any projection
                // the place applies on top of it — and the place's remaining
                // projections extend it. Which prefix is not recorded, so the
                // capability target has to match one of them.
                if let Some(capability) = through_capability
                    && let Some(designations) = through_designations(&loan.place)
                    && !designations
                        .iter()
                        .any(|target| types_compatible(capability.target, target))
                {
                    errors.push(format!(
                        "{prefix}: no prefix of the loan place ({}) has the through-reference capability target {}",
                        designations
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(", "),
                        capability.target
                    ));
                }
                let Some(origin) = &loan.interior else {
                    continue;
                };
                if origin.root as usize >= cx.function.n_vars {
                    errors.push(format!(
                        "{prefix}: interior loan has invalid root slot {}",
                        origin.root
                    ));
                }
                if origin.root != loan.place.root && loan.place.through.is_none() {
                    errors.push(format!(
                        "{prefix}: interior loan origin roots at slot {}, but its executable place roots at slot {}",
                        origin.root, loan.place.root
                    ));
                }
                // A domain loan names either a named interior generation or
                // the conservative subtree form; subtree is terminal.
                if let Some(position) = origin
                    .path
                    .iter()
                    .position(|segment| matches!(segment, mojito_types::origin::OriginSeg::Subtree))
                    && position != origin.path.len() - 1
                {
                    errors.push(format!(
                        "{prefix}: interior loan origin rooted at slot {} has a non-terminal \
                         subtree segment",
                        origin.root
                    ));
                }
                if !origin.path.iter().any(|segment| {
                    matches!(
                        segment,
                        mojito_types::origin::OriginSeg::Interior(_)
                            | mojito_types::origin::OriginSeg::Subtree
                    )
                }) {
                    errors.push(format!(
                        "{prefix}: interior loan origin rooted at slot {} has no interior segment",
                        origin.root
                    ));
                }
            }
        }
        MirInstr::InvalidateInteriors {
            base,
            except,
            include_base_generation,
            ..
        } => {
            if base.root as usize >= cx.function.n_vars {
                errors.push(format!(
                    "{prefix}: interior invalidation has invalid root slot {}",
                    base.root
                ));
            }
            if let Some(reference) = except
                && *reference as usize >= cx.function.n_vars
            {
                errors.push(format!(
                    "{prefix}: interior invalidation exception has invalid reference slot {reference}"
                ));
            }
            if *include_base_generation
                && !base
                    .path
                    .iter()
                    .any(|segment| matches!(segment, mojito_types::origin::OriginSeg::Interior(_)))
            {
                errors.push(format!(
                    "{prefix}: inclusive interior invalidation has no named interior generation"
                ));
            }
        }
        _ => {}
    }
}

/// The storage types a place's `through` handle may designate: the root
/// storage (the referent when the root slot is itself a capability) followed
/// by the result of each projection the place applies. The handle enters the
/// chain at one of these positions and the place's remaining projections run
/// from there; `through` does not record which position, so every one of them
/// is an admissible capability target.
fn through_designations(place: &MirPlace) -> Option<Vec<&Ty>> {
    let root = place.root_ty.as_ref()?;
    let mut designations =
        vec![reference_capability(root).map_or(root, |capability| capability.target)];
    designations.extend(place.projection_tys.iter());
    Some(designations)
}
