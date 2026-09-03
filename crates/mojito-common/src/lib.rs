//! Cross-phase vocabulary shared by every compiler crate: source spans and
//! tokens, numeric literal semantics, and the frontend/checker error types.
//! This crate sits at the bottom of the workspace dependency DAG and must not
//! depend on any other mojito crate.

pub mod error;
pub mod literal;
pub mod token;
