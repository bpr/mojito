//! Execution-backend facade: the statically dispatched [`Backend`] enum and
//! VM live in `mojito-vm`; the feature-gated Pliron native backend remains
//! here until its own crate extraction.

pub use mojito_vm::backend::*;

#[cfg(feature = "backend-pliron")]
pub use mojito_pliron as pliron;
