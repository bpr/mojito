//! Method-call type inference: `infer_method_call` dispatch, overload scoring,
//! call-boundary snapshots/adjustments, static- and pointer-method inference,
//! struct dunder resolution, and List/Tuple method inference. Extracted from
//! `checker.rs`; see `docs/symbol-map.md`.

use super::*;

mod builtin_types;
mod mc_infer;
mod selection;
mod statics;
