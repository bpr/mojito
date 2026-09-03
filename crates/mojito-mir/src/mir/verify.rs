//! Standalone semantic verification for typed MIR.
//!
//! The verifier consumes a lowered [`MirProgram`] plus its checked declaration
//! metadata — never source AST — and reports every violation as a message in
//! the program's `invariant_errors` style. Production compilation and the VM
//! reject programs with any finding; ownership dataflow remains owned by
//! `crate::analysis` and is composed with this verifier by the pipeline.
//!
//! Findings locate themselves textually with the canonical prefixes
//! `MIR function '<name>'` and `MIR function '<name>' block <n>`.
//! `crate::mir::text::verify_artifact` maps those prefixes back to artifact
//! source locations — keep the spellings in sync with its candidate parser.
//!
//! Check classes:
//! - typed-place completeness and projection consistency;
//! - register bounds and register-type completeness;
//! - instruction and call type consistency (via the checker's coercion
//!   predicate; calls are compared against `MirFunctionDeclaration` facts when
//!   the callee is declared — builtin callees have no declaration and only
//!   participate in register checks);
//! - CFG edges: jump-target bounds per region, `FallOff`/`EscapeJump` only
//!   inside `try` sub-regions;
//! - effects: a raising site (a `Raise`, or a call carrying a checked error
//!   type) inside a nonraising function must be protected by a handler;
//! - reference invariants: `StoreRef` initializes reference storage, and a
//!   declared write-back parameter receives a caller place.

use super::{
    FuncRef, MirBlock, MirDeclarations, MirFunction, MirFunctionDeclaration, MirInstr,
    MirIntrinsicSubscript, MirPlace, MirProgram, MirTerm, Proj, Reg,
};
use mojito_types::ct::CtValue;
use mojito_types::origin::{Mutability, PointerOrigin};
use mojito_types::types::{DependentType, ParamDecl, Ty, TyArg, tuple_elements};
use std::collections::{HashMap, HashSet};

pub fn verify(program: &MirProgram) -> Vec<String> {
    let mut errors = Vec::new();
    verify_runtime_pack_abi(&program.declarations, &mut errors);
    for (name, function) in &program.functions {
        verify_function(name, function, &program.declarations, &mut errors);
    }
    errors
}

mod calls;
mod instr;
mod intrinsics;
mod places;
mod regs;
mod subscripts;
mod types;

use calls::*;
use instr::*;
use intrinsics::*;
use places::*;
pub use regs::*;
use subscripts::*;
use types::*;

#[derive(Clone, Copy)]
enum ReferencePermission {
    Immutable,
    Mutable,
    Param(mojito_types::origin::OriginParamId),
}

impl ReferencePermission {
    fn from_mutability(mutability: Mutability) -> Self {
        match mutability {
            Mutability::Immutable => Self::Immutable,
            Mutability::Mutable => Self::Mutable,
            Mutability::Param(parameter) => Self::Param(parameter),
        }
    }

    fn allows_write(self) -> bool {
        !matches!(self, Self::Immutable)
    }

    /// Whether a capability with this permission can initialize or be viewed
    /// as one requiring `target`. A symbolic permission has already been
    /// constrained by the checker; retaining it here avoids guessing that an
    /// executable generic body is either mutable or immutable.
    fn satisfies(self, target: Self) -> bool {
        match target {
            Self::Immutable => true,
            Self::Mutable => self.allows_write(),
            Self::Param(target) => match self {
                Self::Mutable => true,
                Self::Param(found) => found == target,
                Self::Immutable => false,
            },
        }
    }
}

/// Where a run of blocks sits: the function's top level, or one `try`
/// sub-region (with its handler-protection status).
struct RegionContext {
    /// Number of blocks in this region — the bound for region-local jumps.
    region_len: usize,
    /// Number of blocks in the enclosing function — the bound for
    /// `EscapeJump` targets.
    function_len: usize,
    /// Whether this run of blocks is a `try` sub-region (where `FallOff` and
    /// `EscapeJump` are legal terminators).
    in_try_region: bool,
    /// Whether a raise from this position reaches an `except` handler before
    /// leaving the function.
    protected: bool,
}

struct SubscriptSources<'a> {
    receiver_ty: Option<&'a Ty>,
    method: &'static str,
    receiver_place: Option<&'a MirPlace>,
    positional_places: &'a [Option<MirPlace>],
    keyword_places: &'a [Option<MirPlace>],
    positional_types: &'a [Option<Ty>],
    keyword_types: &'a [Option<Ty>],
    dest: Option<Reg>,
}

#[derive(Clone, Copy)]
struct ReferenceCapability<'a> {
    target: &'a Ty,
    permission: ReferencePermission,
}

struct GenericArgumentMaps {
    types: HashMap<String, Ty>,
    values: HashMap<String, CtValue>,
}
