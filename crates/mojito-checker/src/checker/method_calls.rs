//! Method-call type inference: `infer_method_call` dispatch, overload scoring,
//! call-boundary snapshots/adjustments, static- and pointer-method inference,
//! struct dunder resolution, and List/Tuple method inference. Extracted from
//! `checker.rs`; see `docs/symbol-map.md`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

mod builtin_types;
mod mc_infer;
mod selection;
mod statics;
