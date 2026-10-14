//! The authoritative whole-program compiler pipeline.

use crate::backend::BackendKind;
use crate::checked::CheckedProgram;
use crate::checker::{CheckContext, InstalledInterfaces};
use crate::comptime::{
    ComptimeError, ImportedScope, elaborate_exporting, elaborate_importing, prepare_importing,
};
use crate::error::{OwnershipError, ParseError, TypeError};
use crate::mir::MirProgram;
use crate::mir::text::{DisassembleError, disassemble};
use crate::module::{
    LinkOptions, LinkedProgram, ModuleError, inject_prelude, inject_prelude_with_package,
    link_source_with_options, link_source_with_package, link_with_options, link_with_package,
};
use crate::native::mono::{MonoError, SpecializedProgram};
use crate::runtime::RuntimeError;
use crate::runtime::Value;
use crate::timing;
use crate::{Stmt, ast::StmtKind, parse};
use std::fmt;

use crate::package::ImportedPackage;
use std::path::Path;
use std::sync::{Arc, OnceLock};
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
    /// With an imported package, the elaborated program with every package
    /// body joined to it.
    complete: OnceLock<MirProgram>,
    concrete: OnceLock<Result<SpecializedProgram, MonoError>>,
    /// The native target the elaborator answers layout queries for.
    target: Option<crate::native::target::NativeTarget>,
    /// The compiled package the program imports, whose bodies were checked
    /// and lowered when it was built.
    package: Option<Arc<ImportedPackage>>,
}
impl CompiledProgram {
    /// The semantically checked program carried by this ownership-verified
    /// pipeline result. An imported package's bodies are not in it.
    pub const fn checked(&self) -> &CheckedProgram {
        &self.checked
    }

    /// Parametric MIR: the ownership-verified, pre-drop MIR produced by the
    /// authoritative compiler pipeline, whose generic bodies still name their
    /// parameters. The later phases derive from it. With an imported
    /// package it holds the program's own functions and declarations, with
    /// the package's declarations attached, each resolved the first time a
    /// lookup names it; the package's bodies stay in the package.
    pub const fn mir(&self) -> &MirProgram {
        &self.mir
    }

    /// The compiled package the program imports, if any.
    pub fn imported_package(&self) -> Option<&ImportedPackage> {
        self.package.as_deref()
    }

    /// The program's own drop-elaborated MIR, re-verified: [`Self::mir`]
    /// with drops inserted. It is the elaborator's input, beside an
    /// imported package's bodies, whose drops are inserted as each is
    /// materialized.
    /// Post-drop verification findings are folded into `invariant_errors`;
    /// consumers refuse a non-empty list.
    pub fn elaborated_mir(&self) -> &MirProgram {
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

    /// Drop-elaborated MIR: parametric MIR with drops inserted, re-verified.
    /// It may still be generic. It is the whole program: an imported
    /// package's bodies, every one lowered, join [`Self::elaborated_mir`],
    /// so it serializes as one artifact.
    pub fn drop_elaborated_mir(&self) -> &MirProgram {
        let elaborated = self.elaborated_mir();
        let Some(package) = &self.package else {
            return elaborated;
        };
        self.complete.get_or_init(|| {
            let mut complete = elaborated.clone();
            complete.functions.extend(package.bodies.all());
            complete.invariant_errors.extend(package.bodies.failures());
            complete
                .invariant_errors
                .extend(package.declarations.failures());
            complete
        })
    }

    /// Concrete MIR: the drop-elaborated program elaborated by
    /// `native::mono` from its entry roots and verified concrete. Every
    /// backend consumes this one graph; no backend elaborates again. An
    /// imported package's body is lowered from its unit when the elaboration first
    /// reaches it.
    pub fn concrete_mir(&self) -> Result<&SpecializedProgram, CompilerError> {
        self.concrete
            .get_or_init(|| {
                let mir = self.elaborated_mir();
                let _elaborate = timing::span("elaborate");
                let concrete = crate::native::mono::specialize_importing(
                    mir,
                    self.package.as_deref().map(|package| &package.bodies),
                    &crate::native::mono::entry_roots(mir),
                    self.target.as_ref(),
                )
                .and_then(|concrete| {
                    let failures = self
                        .package
                        .as_ref()
                        .map(|package| package.declarations.failures())
                        .unwrap_or_default();
                    if failures.is_empty() {
                        Ok(concrete)
                    } else {
                        Err(crate::native::mono::MonoError {
                            kind: crate::native::mono::MonoErrorKind::Unsupported,
                            function: None,
                            construct: failures.join("; "),
                        })
                    }
                });
                if let Ok(concrete) = &concrete {
                    timing::count(
                        "concrete_functions",
                        concrete.program.functions.len() as u64,
                    );
                }
                if let Some(package) = &self.package {
                    timing::count(
                        "package_bodies_decoded",
                        package.bodies.materialized() as u64,
                    );
                    timing::count(
                        "package_units_lowered",
                        package.bodies.units_lowered() as u64,
                    );
                    timing::count(
                        "package_declarations_decoded",
                        package.declarations.decoded() as u64,
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
    /// Import the bundled library's compiled package instead of checking
    /// its bodies again. `None` defers to the `MOJITO_STD_PACKAGE`
    /// environment variable (`0` compiles the library from source, a
    /// development build's seam an installed compiler refuses).
    std_package: Option<bool>,
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
            std_package: None,
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
    /// Import the bundled library's compiled package, or compile the
    /// library from source, whatever the environment says; an installed
    /// compiler, which has no library source, refuses the second.
    #[must_use]
    pub const fn with_std_package(mut self, import: bool) -> Self {
        self.std_package = Some(import);
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
        self.compile_linking(|package| match package {
            Some(package) => link_with_package(entry, self.link_options.clone(), &package.modules),
            None => link_with_options(entry, self.link_options.clone()).map(from_source),
        })
    }
    /// Link in-memory source as `entry` and compile it through ownership
    /// verification.
    pub fn compile_source(
        &self,
        source: &str,
        entry: &Path,
    ) -> Result<CompiledProgram, CompilerError> {
        self.compile_linking(|package| match package {
            Some(package) => {
                link_source_with_package(source, entry, self.link_options.clone(), &package.modules)
            }
            None => {
                link_source_with_options(source, entry, self.link_options.clone()).map(from_source)
            }
        })
    }
    /// Compile source without a module base, as used for standard input.
    pub fn compile_unlinked(&self, source: &str) -> Result<CompiledProgram, CompilerError> {
        let parsed = parse(source).map_err(CompilerError::Parse)?;
        self.compile_linking(|package| match package {
            Some(package) => inject_prelude_with_package(
                parsed.clone(),
                LinkOptions::default(),
                &package.modules,
            ),
            None => inject_prelude(parsed.clone()).map(from_source),
        })
    }
    /// Elaborate, check, verify, and ownership-verify an already linked
    /// statement set. Verification, ownership, artifact emission, and backend
    /// execution all consume the one cached `MirProgram` lowered here.
    ///
    /// `linked` is a source link, whose library declarations are as their
    /// source spells them, so the program compiles with the library from
    /// source: only a link that read the package's module records
    /// ([`Self::compile_path`] and its siblings) imports it.
    pub fn compile_linked(&self, linked: &[Stmt]) -> Result<CompiledProgram, CompilerError> {
        self.compile_importing(linked, None, &mut CheckContext::new(), false)
    }

    /// Compile `linked` from source, with what a package built from it
    /// exports.
    pub(crate) fn compile_for_package(
        &self,
        linked: &[Stmt],
    ) -> Result<(CompiledProgram, crate::checker::InterfaceExport), CompilerError> {
        let mut context = CheckContext::new();
        context.record_interfaces();
        let program = self.compile_importing(linked, None, &mut context, true)?;
        let interfaces = context
            .take_interfaces()
            .map_err(|error| CompilerError::Verify(vec![error]))?;
        Ok((program, interfaces))
    }

    /// The bundled library's compiled package, when this compiler imports
    /// it; an installed compiler's missing or unreadable package is an
    /// error, as is an installed compiler told not to import it.
    fn std_package(&self) -> Result<Option<Arc<ImportedPackage>>, CompilerError> {
        let imports = self.std_package.unwrap_or_else(|| {
            std::env::var_os("MOJITO_STD_PACKAGE").is_none_or(|value| value != "0")
        });
        if !imports {
            return match crate::module::installed_prefix() {
                Some(prefix) => Err(CompilerError::Module(ModuleError::InstalledPackage {
                    path: crate::package::installed_package(&prefix),
                    reason: crate::package::NO_LIBRARY_SOURCE.to_string(),
                })),
                None => Ok(None),
            };
        }
        crate::package::bundled_std_package().map_err(CompilerError::Module)
    }

    /// Link with `link` and compile the result, importing the bundled
    /// library's package when there is one, so the link reads each library
    /// module from its record, as upstream's importer reads `std.mojoc`: a
    /// program that cannot link against the package is an error, never a
    /// compile of the library's source.
    fn compile_linking(
        &self,
        link: impl Fn(Option<&ImportedPackage>) -> Result<LinkedProgram, ModuleError>,
    ) -> Result<CompiledProgram, CompilerError> {
        let package = self.std_package()?;
        let linked = link(package.as_deref()).map_err(CompilerError::Module)?;
        self.compile_importing(
            &linked.statements,
            package.map(|package| importing(package, &linked.packaged)),
            &mut CheckContext::new(),
            false,
        )
    }

    /// Compile `linked`, importing `package` when there is one: the
    /// package's installed declaration interfaces are read beside the
    /// program's own statements, which are all `linked` holds, and only the
    /// program's own are prepared, elaborated, and checked. A package build's
    /// (`exporting`) check exports its module constants as its elaboration
    /// resolved them.
    fn compile_importing(
        &self,
        linked: &[Stmt],
        package: Option<(Arc<ImportedPackage>, InstalledInterfaces)>,
        context: &mut CheckContext,
        exporting: bool,
    ) -> Result<CompiledProgram, CompilerError> {
        let _compile = timing::span("compile");
        if let Some((_, installed)) = &package {
            context.install(installed.clone());
        }
        let prepared = {
            let _prepare = timing::span("prepare");
            prepare_importing(
                linked.to_vec(),
                package.as_ref().map(|(_, installed)| installed),
            )
            .map_err(CompilerError::Comptime)?
        };
        context.set_body_fact_reuse(self.body_fact_reuse.unwrap_or_else(|| {
            std::env::var_os("MOJITO_BODY_FACT_REUSE").is_none_or(|value| value != "0")
        }));
        // One elaboration and one check: every body is checked once with
        // every `comptime if` arm and `comptime for` body open and its
        // binders symbolic, and the elaborator below MIR decides, unrolls,
        // and finds the instances the entries reach.
        let own = {
            let _elaborate = timing::span("elaborate");
            if exporting {
                let (elaborated, constants) =
                    elaborate_exporting(&prepared).map_err(CompilerError::Comptime)?;
                context.record_elaborated(constants);
                elaborated
            } else {
                elaborate_importing(
                    &prepared,
                    ImportedScope {
                        installed: package.as_ref().map(|(_, installed)| installed),
                    },
                )
                .map_err(CompilerError::Comptime)?
            }
        };
        if !self.allow_executable_module_scope {
            validate_module_scope(&own).map_err(CompilerError::Type)?;
        }
        let (package, installed): (Option<_>, Option<_>) = package.unzip();
        let checked = {
            let _check = timing::span("check");
            crate::checker::check_program_in(&own, context)
        };
        // A package declaration whose record does not decode is missing from
        // every lookup, so its failure outranks the check's verdict.
        if let Some(installed) = &installed {
            timing::count("package.interfaces_decoded", installed.decoded() as u64);
            let failures = installed.failures();
            if !failures.is_empty() {
                return Err(CompilerError::Verify(failures));
            }
        }
        let checked = checked.map_err(CompilerError::Type)?;
        let mir = {
            let _lower = timing::span("mir.lower");
            crate::mir::lower_checked_program_importing(
                &checked,
                package.as_deref().map(|package| &package.declarations),
            )
        };
        timing::count("mir_functions", mir.functions.len() as u64);
        let param_stats = context.param_context().stats();
        timing::count("param_expr.interned", param_stats.interned);
        timing::count("param_expr.intern_hits", param_stats.hits);
        timing::count("types.interned", param_stats.types_interned);
        timing::count("types.intern_hits", param_stats.type_hits);
        timing::count("param_expr.constant_folds", param_stats.constant_folds);
        timing::count("param_expr.replacements", param_stats.replacements);
        timing::count("param_expr.contexts", param_stats.contexts);
        if !mir.invariant_errors.is_empty() {
            return Err(CompilerError::Verify(mir.invariant_errors));
        }
        if let Some(package) = &package {
            let shared: Vec<String> = mir
                .functions
                .iter()
                .filter(|(name, _)| package.bodies.contains(name))
                .map(|(name, _)| format!("`{name}` is defined by the program and by its package"))
                .collect();
            if !shared.is_empty() {
                return Err(CompilerError::Verify(shared));
            }
        }
        {
            let _ownership = timing::span("ownership");
            crate::analysis::check_ownership_program(&mir).map_err(CompilerError::Ownership)?;
        }
        Ok(CompiledProgram {
            checked,
            mir,
            elaborated: OnceLock::new(),
            complete: OnceLock::new(),
            concrete: OnceLock::new(),
            target: self.target,
            package,
        })
    }
    /// Execute an ownership-verified program using the configured backend.
    pub fn execute(&self, program: &CompiledProgram) -> Result<Execution, CompilerError> {
        let mut backend = self.backend.instantiate().map_err(|unimplemented| {
            CompilerError::Runtime(RuntimeError::Unsupported(unimplemented))
        })?;
        let elaborated = {
            let _prepare = timing::span("prepare");
            program.elaborated_mir()
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

/// A source link, which read no package record.
const fn from_source(statements: Vec<Stmt>) -> LinkedProgram {
    LinkedProgram {
        statements,
        packaged: Vec::new(),
    }
}

/// `package` with the declaration interfaces of the modules `packaged`, the
/// link read from its records, each declaration's to decode on first
/// lookup.
fn importing(
    package: Arc<ImportedPackage>,
    packaged: &[u32],
) -> (Arc<ImportedPackage>, InstalledInterfaces) {
    let _decode = timing::span("package.interfaces");
    let installed = InstalledInterfaces::reaching(&package.checked, packaged.iter().copied());
    (package, installed)
}

impl Default for Compiler {
    fn default() -> Self {
        Self::new(LinkOptions::default(), BackendKind::Vm)
    }
}
