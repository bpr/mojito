//! Origin and reference-handle machinery for the checker: origin-place
//! derivation, interior/aggregate-origin tracking and invalidation, reference
//! parameter handles, capture-origin collection, per-call origin solving, and
//! origin-signature lowering. Extracted from `checker.rs`; see `docs/symbol-map.md`.

use super::*;

pub(super) fn collect_origin_params(
    origin: &crate::origin::Origin,
    out: &mut Vec<crate::origin::OriginParamId>,
) {
    match origin {
        crate::origin::Origin::Param(id) => {
            if !out.contains(id) {
                out.push(*id);
            }
        }
        crate::origin::Origin::Union(origins) => {
            for origin in origins {
                collect_origin_params(origin, out);
            }
        }
        _ => {}
    }
}

type SolvedCallOrigins = (
    Vec<Option<ArgConvention>>,
    Option<crate::origin::RefTy>,
    HashMap<usize, bool>,
);

mod actuals;
mod binders;
mod interior;
mod ref_params;
mod sig;
mod solve;
mod subst;
mod transfer;

pub(in crate::checker) use sig::*;
pub(in crate::checker) use subst::*;
