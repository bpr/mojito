//! Cross-phase vocabulary shared by every compiler crate: source spans and
//! tokens, numeric literal semantics, the frontend/checker error types, and
//! the opt-in phase-timing collector.
//! This crate sits at the bottom of the workspace dependency DAG and must not
//! depend on any other mojito crate.

pub mod error;
pub mod literal;
pub mod timing;
pub mod token;
