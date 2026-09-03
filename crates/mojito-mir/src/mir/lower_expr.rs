//! Expression lowering: short-circuit/ternary/compare chains, collection and
//! comprehension construction, nested-closure emission, and the `expr_unconverted`
//! expression dispatcher.
//! Extracted from `mir.rs`; see `docs/symbol-map.md`.

use super::*;

mod calls;
mod ctrl;
mod entry;
mod expr;

/// The checked inline uninit-storage method being lowered.
enum UninitStorageOp {
    Write,
    Take,
    Destroy,
}
