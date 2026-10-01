//! The A1 shadow core: verified, drop-elaborated MIR re-expressed as the
//! `mojito` Pliron dialect, beside the MIR the backends consume.
//!
//! Optional and removable (`a1-core`). The contract, verdict conditions, and
//! measurements are in `docs/notes/pliron-a1.md`.

use std::fmt;

pub mod attrs;
pub mod execute;
pub mod export;
pub mod import;
pub mod inventory;
pub mod ir_framework;
pub mod lifecycle;
pub mod measure;
pub mod ops;
pub mod opt;
pub mod outcomes;
pub mod params;
pub mod provenance;
pub mod text;
pub mod types;
pub mod verify;

/// A diagnostic from the shadow core: a rejected input, a malformed module,
/// or a conversion that cannot complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct A1Error {
    pub kind: A1ErrorKind,
    /// The function the diagnostic belongs to, when it has one.
    pub function: Option<String>,
    pub message: String,
}

impl A1Error {
    pub fn new(kind: A1ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            function: None,
            message: message.into(),
        }
    }

    pub fn unsupported_type(what: impl fmt::Display) -> Self {
        Self::new(
            A1ErrorKind::UnsupportedType,
            format!("{what} is outside the core type vocabulary"),
        )
    }

    #[must_use]
    pub fn in_function(mut self, function: &str) -> Self {
        self.function.get_or_insert_with(|| function.to_string());
        self
    }

    /// The same diagnostic under the census class it counts in.
    #[must_use]
    pub fn classified(mut self, class: &str) -> Self {
        self.message = format!("{class}: {}", self.message);
        self
    }
}

impl fmt::Display for A1Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.function {
            Some(function) => write!(f, "a1 {:?} in `{function}`: {}", self.kind, self.message),
            None => write!(f, "a1 {:?}: {}", self.kind, self.message),
        }
    }
}

impl std::error::Error for A1Error {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum A1ErrorKind {
    /// A checked type the core vocabulary does not contain.
    UnsupportedType,
    /// A MIR form the importer has no rule for.
    UnsupportedForm,
    /// The module failed Pliron or core verification.
    Verification,
    /// Core text failed to parse.
    Parse,
    /// An operation, type, or symbol illegal at a conversion boundary.
    Legality,
    /// The exported MIR failed `mir::verify`.
    Export,
}
