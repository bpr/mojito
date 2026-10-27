//! Expression lowering: short-circuit/ternary/compare chains, collection and
//! comprehension construction, nested-closure emission, and the `expr_unconverted`
//! expression dispatcher.
//! Extracted from `mir.rs`; see `docs/symbol-map.md`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

mod calls;
mod ctrl;
mod entry;
mod expr;
mod expr_access;
mod expr_call;
mod expr_method;
mod mlir_op;

use ctrl::{CollectionPlan, array_literal_plan, collection_plan, dict_literal_plan};

/// The checked inline uninit-storage method being lowered.
enum UninitStorageOp {
    Write,
    Take,
    Destroy,
}
