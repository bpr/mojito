//! Execute verified textual MIR artifacts.
//!
//! This module composes the artifact loading gate (`mir::text::load_artifact`,
//! which parses and runs the canonical MIR semantic verifier) with the
//! elaborator and the backend's concrete-MIR execution entry. It deliberately
//! bypasses [`crate::compiler::Compiler`]: artifacts carry no Mojo source,
//! imports, or checked AST — the serialized, drop-elaborated MIR is the
//! complete program, and it is elaborated after it is loaded.

use crate::backend::BackendKind;
use crate::compiler::{Execution, VmInstantiation};
use crate::mir::text::{ArtifactReport, load_artifact};
use crate::native::mono::{MonoError, entry_roots, specialize};
use crate::runtime::RuntimeError;
use std::fmt;

/// Load and execute one textual MIR artifact, capturing its output and final
/// top-level bindings.
///
/// The loading gate (parse + canonical verify) is the artifact's semantic
/// gate: the loaded program is not drop-elaborated or ownership-analyzed
/// again. It is elaborated to concrete MIR from its entry roots, as a
/// compiled source program is, unless `MOJITO_VM_ERASED` selects the erased
/// oracle.
pub fn run_artifact(
    input: &[u8],
    source_name: impl Into<String>,
    backend: BackendKind,
) -> Result<Execution, ArtifactRunError> {
    run_artifact_as(input, source_name, backend, VmInstantiation::from_env())
}

/// [`run_artifact`] with the VM's instantiation chosen by the caller.
pub fn run_artifact_as(
    input: &[u8],
    source_name: impl Into<String>,
    backend: BackendKind,
    instantiation: VmInstantiation,
) -> Result<Execution, ArtifactRunError> {
    let parsed = load_artifact(input, source_name).map_err(ArtifactRunError::Load)?;
    let mut backend = backend.instantiate().map_err(ArtifactRunError::Backend)?;
    match instantiation {
        VmInstantiation::Concrete => {
            let concrete = specialize(
                &parsed.program,
                &entry_roots(&parsed.program),
                crate::native::target::NativeTarget::host().as_ref(),
            )
            .map_err(ArtifactRunError::Elaborate)?;
            backend.run_concrete(concrete.program)
        }
        VmInstantiation::Erased => backend.run_elaborated(parsed.program),
    }
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
    /// The elaborator refused to instantiate a body the artifact's entry
    /// roots reach.
    Elaborate(MonoError),
    /// The artifact loaded but execution failed.
    Runtime(RuntimeError),
}

impl fmt::Display for ArtifactRunError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Load(report) => write!(formatter, "{report}"),
            Self::Backend(message) => write!(formatter, "{message}"),
            Self::Elaborate(error) => write!(formatter, "Elaboration error: {error}"),
            Self::Runtime(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for ArtifactRunError {}
