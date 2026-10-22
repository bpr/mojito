//! Standalone semantic verification for typed MIR.
//!
//! The verifier consumes a lowered [`MirProgram`] plus its checked declaration
//! metadata — never source AST — and reports every violation as a message in
//! the program's `invariant_errors` style. Production compilation and the VM
//! reject programs with any finding; use-after-move diagnostics remain owned
//! by `crate::analysis`, composed with this verifier by the pipeline.
//!
//! Findings locate themselves textually with the canonical prefixes
//! `MIR function '<name>'` and `MIR function '<name>' block <n>`.
//! `crate::mir::text::verify_artifact` maps those prefixes back to artifact
//! source locations — keep the spellings in sync with its candidate parser.
//!
//! Check classes:
//! - typed-place completeness and projection consistency;
//! - register bounds, and register- and used-slot type completeness;
//! - instruction and call type consistency (via the checker's coercion
//!   predicate; calls are compared against `MirFunctionDeclaration` facts when
//!   the callee is declared — builtin callees have no declaration and only
//!   participate in register checks);
//! - structured control flow: one block per region, the body included, as
//!   upstream's `hlcf` regions are `SizedRegion<1>`; each region terminator
//!   inside the region op it ends, a `break`/`continue` inside its loop, and
//!   `TryYield` only inside a `try` part;
//! - effects: a raising site (a `Raise`, or a call carrying a checked error
//!   type) inside a nonraising function must be protected by a handler;
//! - reference invariants: `StoreRef` initializes reference storage, and a
//!   declared write-back parameter receives a caller place.
//! - slot ends: a `drop.var` or `var.consume` of a slot every path reaching
//!   it already dropped, consumed, or moved.

use super::{
    CleanupDrop, FuncRef, MirBlock, MirDeclarations, MirFunction, MirFunctionDeclaration, MirInstr,
    MirIntrinsicSubscript, MirPlace, MirProgram, MirTerm, Proj, Reg, cleanup_slots,
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
    errors.extend(
        program
            .declarations
            .imported
            .iter()
            .flat_map(super::link::ImportedDeclarations::failures),
    );
    errors
}

/// [`verify`] for the one function `name` of a program whose declarations
/// are `declarations`.
pub fn verify_function_in(
    name: &str,
    function: &MirFunction,
    declarations: &MirDeclarations,
) -> Vec<String> {
    let mut errors = Vec::new();
    verify_function(name, function, declarations, &mut errors);
    errors
}

/// [`verify`] for elaborated MIR: the same structural rules, plus the
/// rejection of every symbolic type, compile-time parameter, and
/// erased-dispatch contract. Parametric MIR never passes this mode.
pub fn verify_concrete(program: &MirProgram) -> Vec<String> {
    let mut errors = verify(program);
    verify_concrete_program(program, &mut errors);
    errors
}

/// [`verify_concrete`] for the one function `name` of a concrete program
/// whose declarations are `declarations`: its structural and concreteness
/// findings, without the declarations' own.
pub fn verify_concrete_function_in(
    name: &str,
    function: &MirFunction,
    declarations: &MirDeclarations,
) -> Vec<String> {
    let mut errors = verify_function_in(name, function, declarations);
    errors.extend(concrete_function_findings(name, function));
    errors
}

/// Whether two register or place types agree as the verifier compares them
/// (`types_compatible`): the elaborator judges an instance's `rebind` by it.
pub fn types_agree(found: &Ty, expected: &Ty) -> bool {
    types_compatible(found, expected)
}

mod calls;
mod concrete;
mod drops;
mod instr;
mod intrinsics;
pub(in crate::mir) use intrinsics::instantiate_generic_callable_contract;
mod iteration;
mod loans;
mod places;
mod regs;
mod scope;
mod structure;
mod subscripts;
mod types;

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use calls::*;
pub use concrete::concrete_function_findings;
use concrete::verify_concrete_program;
pub use drops::cut_try_cleanups;
use drops::verify_slot_ends;
#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use instr::*;
#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use intrinsics::*;
use iteration::verify_iteration_instruction;
use loans::verify_loan_instruction;
#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use places::*;
pub use places::{instruction_places, instruction_places_mut, place_base_ty};
pub use regs::*;
use scope::verify_scope;
pub use scope::{instruction_named_types, instruction_variant_indices};
use structure::{verify_region_terminator, verify_regions};
#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use subscripts::*;
#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use types::*;

#[derive(Clone, Copy)]
enum ReferencePermission {
    Immutable,
    Mutable,
    Param(mojito_types::origin::OriginParamId),
}

impl ReferencePermission {
    const fn from_mutability(mutability: Mutability) -> Self {
        match mutability {
            Mutability::Immutable => Self::Immutable,
            Mutability::Mutable => Self::Mutable,
            Mutability::Param(parameter) => Self::Param(parameter),
        }
    }

    const fn allows_write(self) -> bool {
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

/// Where a block sits: the function's top level, or one region of a region
/// op (with its handler-protection status).
struct RegionContext {
    /// The kind of each region from the function's top level in, this
    /// region's last: a region terminator is checked against the regions
    /// enclosing it.
    regions: Vec<RegionKind>,
    /// Whether a raise from this position reaches an `except` handler before
    /// leaving the function.
    protected: bool,
}

impl RegionContext {
    fn kind(&self) -> &RegionKind {
        self.regions.last().unwrap_or(&RegionKind::Function)
    }

    /// Whether this block is a `try` part's, where `TryYield` is the legal
    /// way to complete.
    fn is_try_part(&self) -> bool {
        matches!(self.kind(), RegionKind::TryPart)
    }
}

/// What a region is to the op that holds it.
#[derive(Clone)]
enum RegionKind {
    Function,
    TryPart,
    /// An `if`'s then, elif, or else arm, ended by `Yield` with its
    /// op's results.
    IfArm(Vec<Reg>),
    /// An elif's condition region, ended by `ElifCond`, whose values enter
    /// its arm's `then_args` and the next region's `next_args`.
    ElifCond {
        then_args: Vec<Reg>,
        next_args: Vec<Reg>,
    },
    /// A loop's body, entered with its op's operands as its `args` and
    /// resumed by a `Continue` carrying them, left by a `Break` carrying its
    /// op's results.
    LoopBody {
        label: Option<super::LoopLabel>,
        args: Vec<Reg>,
        results: Vec<Reg>,
    },
    /// A `comptime if` arm, ended by `ComptimeYield` with its op's results.
    ComptimeIfArm(Vec<Reg>),
    /// A `comptime for`'s body, entered with its op's operands as its
    /// `args` and resumed by a `ComptimeContinue` carrying them, left by a
    /// `ComptimeBreak` carrying its op's results.
    ComptimeForBody {
        args: Vec<Reg>,
        results: Vec<Reg>,
    },
    ComptimeForElse,
    /// A `match` case, left by `MatchComplete` carrying its op's
    /// `results`, or by `MatchNext` carrying the next case's or the else
    /// region's arguments, `next_args`.
    MatchCase {
        next_args: Vec<Reg>,
        results: Vec<Reg>,
    },
}

/// One instruction's verification scope: the function it sits in, the
/// declarations its calls resolve against, its region, and the finding prefix
/// naming its block.
struct InstrCx<'a> {
    name: &'a str,
    prefix: &'a str,
    function: &'a MirFunction,
    declarations: &'a MirDeclarations,
    context: &'a RegionContext,
}

impl<'a> InstrCx<'a> {
    fn reg_ty(&self, register: Reg) -> Option<&'a Ty> {
        self.function.reg_types.get(&register.0)
    }
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
    types: mojito_types::types::TySubst,
    values: HashMap<String, CtValue>,
}
