//! The semantic type vocabulary: the `Ty` lattice and its pure
//! coercion/contract predicates (`types`), origins and reference signatures
//! (`origin`), and compile-time values (`ct`). Sits above the AST and below
//! every checking/lowering phase.

pub mod ct;
pub mod origin;
pub mod types;
