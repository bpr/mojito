//! The authoritative whole-program compiler pipeline.

use crate::backend::BackendKind;
use crate::checked::CheckedProgram;
use crate::checker::CheckContext;
use crate::comptime::{ComptimeError, elaborate_prepared, prepare};
use crate::error::{OwnershipError, ParseError, TypeError};
use crate::mir::MirProgram;
use crate::mir::text::{DisassembleError, disassemble};
use crate::module::{
    LinkOptions, ModuleError, inject_prelude, link_source_with_options, link_with_options,
};
use crate::native::mono::{MonoError, SpecializedProgram};
use crate::runtime::RuntimeError;
use crate::runtime::Value;
use crate::timing;
use crate::{Stmt, ast::StmtKind, parse};
use std::fmt;

use std::path::Path;
use std::sync::OnceLock;
/// A program that has passed linking, comptime elaboration, semantic checking,
/// and ownership analysis and is therefore ready for any backend.
///
/// It holds the three MIR phases apart: parametric MIR ([`Self::mir`]),
/// drop-elaborated MIR ([`Self::drop_elaborated_mir`]), and concrete MIR
/// ([`Self::concrete_mir`]). The last two are computed once, on first use.
#[derive(Debug, Clone)]
pub struct CompiledProgram {
    checked: CheckedProgram,
    mir: MirProgram,
    elaborated: OnceLock<MirProgram>,
    concrete: OnceLock<Result<SpecializedProgram, MonoError>>,
    /// The native target the elaborator answers layout queries for.
    target: Option<crate::native::target::NativeTarget>,
}
impl CompiledProgram {
    /// The semantically checked program carried by this ownership-verified
    /// pipeline result.
    pub const fn checked(&self) -> &CheckedProgram {
        &self.checked
    }

    /// Parametric MIR: the ownership-verified, pre-drop MIR produced by the
    /// authoritative compiler pipeline, whose generic bodies still name their
    /// parameters. The later phases derive from it.
    pub const fn mir(&self) -> &MirProgram {
        &self.mir
    }

    /// Drop-elaborated MIR: parametric MIR with drops inserted, re-verified.
    /// It may still be generic. It is the serialized artifact and the
    /// elaborator's input. Post-drop verification findings are folded into
    /// `invariant_errors`; consumers refuse a non-empty list.
    pub fn drop_elaborated_mir(&self) -> &MirProgram {
        self.elaborated.get_or_init(|| {
            let mut mir = {
                let _drops = timing::span("drops.elaborate");
                crate::analysis::elaborate_drops_program(self.mir.clone())
            };
            let _verify = timing::span("mir.verify.post_drop");
            let findings = crate::mir::verify::verify(&mir);
            mir.invariant_errors.extend(findings);
            mir
        })
    }

    /// Concrete MIR: the drop-elaborated program elaborated by
    /// `native::mono` from its entry roots and verified concrete. Every
    /// backend consumes this one graph; no backend elaborates again.
    pub fn concrete_mir(&self) -> Result<&SpecializedProgram, CompilerError> {
        self.concrete
            .get_or_init(|| {
                let mir = self.drop_elaborated_mir();
                let _elaborate = timing::span("elaborate");
                let concrete = crate::native::mono::specialize(
                    mir,
                    &crate::native::mono::entry_roots(mir),
                    self.target.as_ref(),
                );
                if let Ok(concrete) = &concrete {
                    timing::count(
                        "concrete_functions",
                        concrete.program.functions.len() as u64,
                    );
                }
                concrete
            })
            .as_ref()
            .map_err(|error| CompilerError::Elaborate(error.clone()))
    }

    /// Emit this program as canonical, executable Mojito MIR assembly.
    pub fn emit_mir(&self) -> Result<String, DisassembleError> {
        disassemble(self.drop_elaborated_mir())
    }
}
#[derive(Debug, Clone)]
/// Observable result of executing a compiled program.
pub struct Execution {
    /// Captured standard output.
    pub output: String,
    /// Final named module-scope values exposed by the backend for inspection.
    pub bindings: Vec<(String, Value)>,
}
/// The stage at which the authoritative pipeline stopped.
#[derive(Debug)]
pub enum CompilerError {
    Module(ModuleError),
    Parse(ParseError),
    Comptime(ComptimeError),
    Type(TypeError),
    Ownership(OwnershipError),
    /// Typed-MIR semantic verification findings — compiler invariant
    /// violations, never user errors: the checker accepted the program, so an
    /// entry here means lowering produced metadata the backend must refuse.
    Verify(Vec<String>),
    /// The elaborator refused to instantiate a body the entry roots reach.
    Elaborate(MonoError),
    Runtime(RuntimeError),
}
impl fmt::Display for CompilerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Module(error) => error.fmt(f),
            Self::Parse(error) => error.fmt(f),
            Self::Comptime(error) => error.fmt(f),
            Self::Type(error) => error.fmt(f),
            Self::Ownership(error) => error.fmt(f),
            Self::Verify(findings) => {
                write!(f, "invalid checked program: {}", findings.join("; "))
            }
            Self::Elaborate(error) => write!(f, "Elaboration error: {error}"),
            Self::Runtime(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for CompilerError {}
/// Owns stage ordering and backend selection for normal whole-program use.
#[derive(Debug, Clone)]
pub struct Compiler {
    link_options: LinkOptions,
    backend: BackendKind,
    allow_executable_module_scope: bool,
    /// Carry an unchanged body's facts from one checker pass to the next
    /// instead of inferring it again. `None` defers to the
    /// `MOJITO_BODY_FACT_REUSE` environment variable (`0` disables).
    body_fact_reuse: Option<bool>,
    /// The native target the elaborator answers layout queries for: the
    /// `--target` of a native compile, otherwise the host. `None` on a host
    /// with no native target, where a program that asks a layout fails.
    target: Option<crate::native::target::NativeTarget>,
}
/// Reject runtime statements at module scope, matching Mojo's source rules.
/// Declarations, imports, compile-time constants, and `pass` are permitted.
pub fn validate_module_scope(stmts: &[Stmt]) -> Result<(), TypeError> {
    for stmt in stmts {
        let statement = match &stmt.kind {
            StmtKind::Def { .. }
            | StmtKind::Struct { .. }
            | StmtKind::Trait { .. }
            | StmtKind::Comptime { .. }
            | StmtKind::Import { .. }
            | StmtKind::FromImport { .. }
            | StmtKind::Pass => continue,
            StmtKind::Scope(body) => {
                validate_module_scope(body)?;
                continue;
            }
            StmtKind::VarDecl { .. } => "variable declaration",
            StmtKind::RefDecl { .. } => "reference declaration",
            StmtKind::Assign { .. } | StmtKind::SetPlace { .. } => "assignment",
            StmtKind::AugAssign { .. } => "augmented assignment",
            StmtKind::Unpack { .. } => "unpacking assignment",
            StmtKind::ComptimeIf { .. } | StmtKind::ComptimeFor { .. } => {
                "unelaborated compile-time statement"
            }
            StmtKind::If { .. } => "if statement",
            StmtKind::While { .. } => "while statement",
            StmtKind::For { .. } => "for statement",
            StmtKind::Return(_) => "return statement",
            StmtKind::Raise(_) => "raise statement",
            StmtKind::With { .. } => "with statement",
            StmtKind::Try { .. } => "try statement",
            StmtKind::Break => "break statement",
            StmtKind::Continue => "continue statement",
            StmtKind::Expr(_) => "expression statement",
        };
        return Err(TypeError::InvalidModuleScope(statement.to_string()));
    }
    Ok(())
}
impl Compiler {
    /// Construct a compiler with explicit module-link and backend policy.
    #[must_use]
    pub const fn new(link_options: LinkOptions, backend: BackendKind) -> Self {
        Self {
            link_options,
            backend,
            allow_executable_module_scope: false,
            body_fact_reuse: None,
            target: match crate::native::target::Triple::host() {
                Some(triple) => Some(crate::native::target::NativeTarget::new(triple)),
                None => None,
            },
        }
    }
    /// Elaborate for `target` instead of the host.
    #[must_use]
    pub const fn with_target(mut self, target: crate::native::target::NativeTarget) -> Self {
        self.target = Some(target);
        self
    }
    /// Turn body-fact carry-over on or off, whatever the environment says:
    /// off, every checker pass infers every body.
    #[must_use]
    pub const fn with_body_fact_reuse(mut self, reuse: bool) -> Self {
        self.body_fact_reuse = Some(reuse);
        self
    }
    /// Permit executable module-scope statements for isolated compiler tests.
    /// This accepts a non-Mojo snippet dialect and must not be used by the CLI or
    /// by conformance tests.
    #[must_use]
    pub const fn with_snippet_module_scope(mut self) -> Self {
        self.allow_executable_module_scope = true;
        self
    }
    /// Link and compile a source entry path through ownership verification.
    pub fn compile_path(&self, entry: &Path) -> Result<CompiledProgram, CompilerError> {
        let linked =
            link_with_options(entry, self.link_options.clone()).map_err(CompilerError::Module)?;
        self.compile_linked(&linked)
    }
    /// Link in-memory source as `entry` and compile it through ownership
    /// verification.
    pub fn compile_source(
        &self,
        source: &str,
        entry: &Path,
    ) -> Result<CompiledProgram, CompilerError> {
        let linked = link_source_with_options(source, entry, self.link_options.clone())
            .map_err(CompilerError::Module)?;
        self.compile_linked(&linked)
    }
    /// Compile source without a module base, as used for standard input.
    pub fn compile_unlinked(&self, source: &str) -> Result<CompiledProgram, CompilerError> {
        let parsed = parse(source).map_err(CompilerError::Parse)?;
        let linked = inject_prelude(parsed).map_err(CompilerError::Module)?;
        self.compile_linked(&linked)
    }
    /// Elaborate, check, verify, and ownership-verify an already linked
    /// statement set. Verification, ownership, artifact emission, and backend
    /// execution all consume the one cached `MirProgram` lowered here.
    pub fn compile_linked(&self, linked: &[Stmt]) -> Result<CompiledProgram, CompilerError> {
        let _compile = timing::span("compile");
        let prepared = {
            let _prepare = timing::span("prepare");
            prepare(linked.to_vec()).map_err(CompilerError::Comptime)?
        };
        let mut context = CheckContext::new();
        context.set_body_fact_reuse(self.body_fact_reuse.unwrap_or_else(|| {
            std::env::var_os("MOJITO_BODY_FACT_REUSE").is_none_or(|value| value != "0")
        }));
        // One elaboration and one check: every body is checked once with
        // every `comptime if` arm and `comptime for` body open and its
        // binders symbolic, and the elaborator below MIR decides, unrolls,
        // and finds the instances the entries reach.
        let elaborated = {
            let _elaborate = timing::span("elaborate");
            elaborate_prepared(&prepared).map_err(CompilerError::Comptime)?
        };
        if !self.allow_executable_module_scope {
            validate_module_scope(&elaborated).map_err(CompilerError::Type)?;
        }
        let checked = {
            let _check = timing::span("check");
            crate::checker::check_program_in(&elaborated, &mut context)
                .map_err(CompilerError::Type)?
        };
        let mir = {
            let _lower = timing::span("mir.lower");
            crate::mir::lower_checked_program(&checked)
        };
        timing::count("mir_functions", mir.functions.len() as u64);
        let param_stats = context.param_context().stats();
        timing::count("param_expr.interned", param_stats.interned);
        timing::count("param_expr.intern_hits", param_stats.hits);
        timing::count("param_expr.constant_folds", param_stats.constant_folds);
        timing::count("param_expr.replacements", param_stats.replacements);
        timing::count("param_expr.contexts", param_stats.contexts);
        if !mir.invariant_errors.is_empty() {
            return Err(CompilerError::Verify(mir.invariant_errors));
        }
        {
            let _ownership = timing::span("ownership");
            crate::analysis::check_ownership_program(&mir).map_err(CompilerError::Ownership)?;
        }
        Ok(CompiledProgram {
            checked,
            mir,
            elaborated: OnceLock::new(),
            concrete: OnceLock::new(),
            target: self.target,
        })
    }
    /// Execute an ownership-verified program using the configured backend.
    pub fn execute(&self, program: &CompiledProgram) -> Result<Execution, CompilerError> {
        let mut backend = self.backend.instantiate().map_err(|unimplemented| {
            CompilerError::Runtime(RuntimeError::Unsupported(unimplemented))
        })?;
        let elaborated = {
            let _prepare = timing::span("prepare");
            program.drop_elaborated_mir()
        };
        if !elaborated.invariant_errors.is_empty() {
            return Err(CompilerError::Verify(elaborated.invariant_errors.clone()));
        }
        let concrete = {
            let _prepare = timing::span("prepare");
            let concrete = program.concrete_mir()?;
            let _clone = timing::span("mir_clone");
            concrete.program.clone()
        };
        {
            let _run = timing::span("vm");
            backend.run_concrete(concrete)
        }
        .map_err(CompilerError::Runtime)?;
        Ok(Execution {
            output: backend.output(),
            bindings: backend.bindings(),
        })
    }
    /// Compile and execute an entry path.
    pub fn run_path(&self, entry: &Path) -> Result<Execution, CompilerError> {
        let program = self.compile_path(entry)?;
        self.execute(&program)
    }
}

impl Default for Compiler {
    fn default() -> Self {
        Self::new(LinkOptions::default(), BackendKind::Vm)
    }
}
