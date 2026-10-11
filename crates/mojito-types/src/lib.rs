//! The semantic type vocabulary: the `Ty` lattice and its pure
//! coercion/contract predicates (`types`), origins and reference signatures
//! (`origin`), shape-decided trait conformance (`conformance`), compile-time
//! values (`ct`), the typed canonical parameter-expression attributes
//! (`param_expr`), and the package section that holds each type once
//! (`type_section`). Sits above the AST and below every checking/lowering
//! phase.

pub mod conformance;
pub mod ct;
pub mod ffi;
pub mod origin;
pub mod param_expr;
pub mod type_section;
pub mod types;
