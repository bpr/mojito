//! Origin and reference-handle machinery for the checker: origin-place
//! derivation, interior/aggregate-origin tracking and invalidation, reference
//! parameter handles, capture-origin collection, per-call origin solving, and
//! origin-signature lowering. Extracted from `checker.rs`; see `docs/symbol-map.md`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

/// One call's `ref` contracts: per argument slot, the result's, and which
/// of the callee's origin binders the call binds from its arguments.
pub(in crate::checker) type CallRefContracts<'a> = (
    &'a [Option<mojito_types::origin::RefSig>],
    Option<&'a mojito_types::origin::RefSig>,
    &'a dyn Fn(mojito_types::origin::OriginParamId) -> bool,
);

type SolvedCallOrigins = (
    Vec<Option<ArgConvention>>,
    Option<mojito_types::origin::RefTy>,
    HashMap<usize, bool>,
);

mod actuals;
mod binders;
mod construct;
mod exclusivity;
mod interior;
mod ref_params;
mod result_alias;
mod sig;
mod solve;
mod subst;
mod transfer;

pub(in crate::checker) use binders::origin_construction;
pub(in crate::checker) use construct::{
    CallValues, CalleeBinders, ConstructorOriginBindings, StructApplication,
    bracket_mutable_binders, declared_origin_binders, names_origin_binder, names_origin_binder_in,
};
pub(in crate::checker) use exclusivity::{
    CollectedArguments, ExclusivityCallee, ExclusivityReceiver, NestedOrigins,
};
pub(in crate::checker) use interior::validate_origin_signature;
pub(in crate::checker) use sig::*;
pub(in crate::checker) use subst::*;
