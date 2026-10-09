//! The register VM: the sole runtime and executable semantic oracle, plus the
//! statically dispatched `Backend` enum. Its one executable input is concrete
//! MIR, the elaborator's verified output.

pub mod backend;
pub mod builtins;
pub mod crossing;
pub mod runtime;
