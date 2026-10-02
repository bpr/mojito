//! The semantic type vocabulary: the `Ty` lattice and its pure
//! coercion/contract predicates (`types`), origins and reference signatures
//! (`origin`), shape-decided trait conformance (`conformance`), compile-time
//! values (`ct`), and the typed canonical parameter-expression attributes
//! (`param_expr`). Sits above the AST and below every checking/lowering
//! phase.

pub mod conformance;
pub mod ct;
pub mod ffi;
pub mod origin;
pub mod param_expr;
pub mod types;
