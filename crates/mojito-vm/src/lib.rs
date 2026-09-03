//! The register VM: the sole runtime and executable semantic oracle, plus the
//! statically dispatched `Backend` enum. `VmBackend::run` re-runs checking and
//! ownership analysis on its input (the stage-composed seam contract).

pub mod backend;
pub mod runtime;
