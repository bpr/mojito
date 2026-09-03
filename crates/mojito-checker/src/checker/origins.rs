//! Origin and reference-handle machinery for the checker: origin-place
//! derivation, interior/aggregate-origin tracking and invalidation, reference
//! parameter handles, capture-origin collection, per-call origin solving, and
//! origin-signature lowering. Extracted from `checker.rs`; see `docs/symbol-map.md`.

use super::*;

pub(super) fn collect_origin_params(
    origin: &mojito_types::origin::Origin,
    out: &mut Vec<mojito_types::origin::OriginParamId>,
) {
    match origin {
        mojito_types::origin::Origin::Param(id) => {
            if !out.contains(id) {
                out.push(*id);
            }
        }
        mojito_types::origin::Origin::Union(origins) => {
            for origin in origins {
                collect_origin_params(origin, out);
            }
        }
        _ => {}
    }
}

type SolvedCallOrigins = (
    Vec<Option<ArgConvention>>,
    Option<mojito_types::origin::RefTy>,
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
