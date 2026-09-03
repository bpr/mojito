//! Execute verified textual MIR artifacts.
//!
//! This module composes the artifact loading gate (`mir::text::load_artifact`,
//! which parses and runs the canonical MIR semantic verifier) with the
//! backend's elaborated-MIR execution entry. It deliberately bypasses
//! [`crate::compiler::Compiler`]: artifacts carry no Mojo source, imports, or
//! checked AST — the serialized, drop-elaborated MIR is the complete program.

use crate::backend::BackendKind;
use crate::compiler::Execution;
use crate::mir::text::{ArtifactReport, load_artifact};
use crate::runtime::RuntimeError;
use std::fmt;

/// Load and execute one textual MIR artifact, capturing its output and final
/// top-level bindings. The loading gate (parse + canonical verify) is the
/// artifact's semantic gate; execution runs the program exactly as
/// serialized — no re-elaboration, re-verification, or ownership re-analysis.
pub fn run_artifact(
    input: &[u8],
    source_name: impl Into<String>,
    backend: BackendKind,
) -> Result<Execution, ArtifactRunError> {
    let parsed = load_artifact(input, source_name).map_err(ArtifactRunError::Load)?;
    let mut backend = backend.instantiate().map_err(ArtifactRunError::Backend)?;
    backend
        .run_elaborated(parsed.program)
        .map_err(ArtifactRunError::Runtime)?;
    Ok(Execution {
        output: backend.output(),
        bindings: backend.bindings(),
    })
}

/// Why an artifact failed to load or execute.
#[derive(Debug)]
pub enum ArtifactRunError {
    /// Artifact syntax, structure, or canonical-verifier findings, located at
    /// artifact source spans. The caller holds the input bytes, so rich span
    /// rendering is its job; `Display` shows the report summary.
    Load(ArtifactReport),
    /// The selected backend refused construction (only the VM executes
    /// artifacts today).
    Backend(String),
    /// The artifact loaded but execution failed.
    Runtime(RuntimeError),
}

impl fmt::Display for ArtifactRunError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ArtifactRunError::Load(report) => write!(formatter, "{report}"),
            ArtifactRunError::Backend(message) => write!(formatter, "{message}"),
            ArtifactRunError::Runtime(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for ArtifactRunError {}
