//! The authoritative whole-program compiler pipeline.

use crate::ast::ExprKind;
use crate::backend::BackendKind;
use crate::checked::CheckedProgram;
use crate::comptime::{
    ComptimeError, DefSpecializationRequest, Elaborated, MethodSpecializationRequest,
    StructInstanceRequest, TStringSpecializationRequest, TupleSpecializationRequest,
    TupleTransformRequest, bound_generic_template_names, elaborate_with_requests,
    tuple_materialized_callables, variadic_struct_template_names,
};
use crate::ct::CtValue;
use crate::error::{OwnershipError, ParseError, TypeError};
use crate::mir::MirProgram;
use crate::mir::text::{DisassembleError, disassemble};
use crate::module::{
    LinkOptions, ModuleError, inject_prelude, link_source_with_options, link_with_options,
};
use crate::runtime::RuntimeError;
use crate::runtime::Value;
use crate::timing;
use crate::{Stmt, ast::StmtKind, check_program, parse};
use crate::{Ty, TyArg};
use std::fmt;
use std::path::Path;
use std::sync::OnceLock;
/// A program that has passed linking, comptime elaboration, semantic checking,
/// and ownership analysis and is therefore ready for any backend.
#[derive(Debug, Clone)]
pub struct CompiledProgram {
    checked: CheckedProgram,
    mir: MirProgram,
    elaborated: OnceLock<MirProgram>,
}
impl CompiledProgram {
    /// The semantically checked program carried by this ownership-verified
    /// pipeline result.
    pub fn checked(&self) -> &CheckedProgram {
        &self.checked
    }

    /// The ownership-verified, pre-drop MIR produced by the authoritative
    /// compiler pipeline, from which the elaborated backend artifact derives.
    pub fn mir(&self) -> &MirProgram {
        &self.mir
    }

    /// The drop-elaborated, re-verified MIR — the exact artifact every
    /// backend consumes. Post-drop verification findings are folded into
    /// `invariant_errors`; consumers refuse a non-empty list.
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

    /// Emit this program as canonical, executable Mojito MIR assembly.
    pub fn emit_mir(&self) -> Result<String, DisassembleError> {
        disassemble(self.elaborated_mir())
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
    /// The iterated generic-instantiation discovery loop kept finding new
    /// closed instantiations after the round cap — e.g. inferred polymorphic
    /// recursion, where each clone requests one deeper instantiation.
    SpecializationDivergence {
        rounds: usize,
        callee: String,
    },
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
            Self::SpecializationDivergence { rounds, callee } => write!(
                f,
                "generic specialization did not converge after {rounds} discovery rounds; \
                 '{callee}' keeps requesting new instantiations (likely inferred polymorphic \
                 recursion) — supply explicit compile-time arguments or bound the recursion"
            ),
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
    pub fn new(link_options: LinkOptions, backend: BackendKind) -> Self {
        Self {
            link_options,
            backend,
            allow_executable_module_scope: false,
        }
    }
    /// Permit executable module-scope statements for isolated compiler tests.
    /// This accepts a non-Mojo snippet dialect and must not be used by the CLI or
    /// by conformance tests.
    pub fn with_snippet_module_scope(mut self) -> Self {
        self.allow_executable_module_scope = true;
        self
    }
    /// Link and compile a source entry path through ownership verification.
    pub fn compile_path(&self, entry: &Path) -> Result<CompiledProgram, CompilerError> {
        let linked =
            link_with_options(entry, self.link_options.clone()).map_err(CompilerError::Module)?;
        self.compile_linked(linked)
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
        self.compile_linked(linked)
    }
    /// Compile source without a module base, as used for standard input.
    pub fn compile_unlinked(&self, source: &str) -> Result<CompiledProgram, CompilerError> {
        let parsed = parse(source).map_err(CompilerError::Parse)?;
        let linked = inject_prelude(parsed).map_err(CompilerError::Module)?;
        self.compile_linked(linked)
    }
    /// Elaborate, check, verify, and ownership-verify an already linked
    /// statement set. Verification, ownership, artifact emission, and backend
    /// execution all consume the one cached `MirProgram` lowered here.
    pub fn compile_linked(&self, linked: Vec<Stmt>) -> Result<CompiledProgram, CompilerError> {
        // Public `Tuple[*Ts]` is a nominal variadic struct, but the element
        // types of a bare `Tuple(exprs...)` or tuple display are semantic
        // facts, and an inferred bound-generic call's instantiation is
        // likewise resolved only by the checker: pre-check elaboration cannot
        // infer arbitrary expression types. Iterate discovery to a fixpoint:
        // each check pass may record new closed instantiations (a requested
        // clone's body can itself contain inferred calls), so requests
        // accumulate monotonically and each round re-elaborates the original
        // linked program with the full set. Programs without generics converge
        // after the first pass with no re-elaboration, and tuple-only programs
        // keep their single re-elaboration; ownership and MIR verification run
        // exactly once, on the fixpoint program.
        const SPECIALIZATION_ROUNDS: usize = 5;
        let templates = bound_generic_template_names(&linked);
        let range_templates = scalar_range_template_names(&linked);
        let variadic_templates = variadic_struct_template_names(&linked);
        let mut tuple_requests: Vec<TupleSpecializationRequest> = Vec::new();
        let mut tstring_requests: Vec<TStringSpecializationRequest> = Vec::new();
        let mut def_requests: Vec<DefSpecializationRequest> = Vec::new();
        let mut method_requests: Vec<MethodSpecializationRequest> = Vec::new();
        let mut struct_requests: Vec<StructInstanceRequest> = Vec::new();
        // Occurrences whose recordings conflicted across rounds; determinism
        // should preclude this, but a poisoned key must stay abstract rather
        // than oscillate.
        let mut conflicted = std::collections::HashSet::new();
        let mut last_new_callee = String::from("Tuple");
        let _compile = timing::span("compile");
        let mut checked = {
            let Elaborated {
                program: discovery,
                instances: minted,
            } = {
                let _elaborate = timing::span("discovery.initial.elaborate");
                elaborate_with_requests(linked.clone(), &[], &[], &[], &[], &[])
                    .map_err(CompilerError::Comptime)?
            };
            struct_requests.extend(minted);
            if !self.allow_executable_module_scope {
                validate_module_scope(&discovery).map_err(CompilerError::Type)?;
            }
            let _check = timing::span("discovery.initial.check");
            check_program(&discovery).map_err(CompilerError::Type)?
        };
        let mut converged = false;
        for round in 0..=SPECIALIZATION_ROUNDS {
            let _round = timing::round("discovery.round", round);
            let requests = timing::span("requests");
            let mut grew = false;
            for request in tuple_specialization_requests(&checked) {
                if !tuple_requests.contains(&request) {
                    tuple_requests.push(request);
                    grew = true;
                }
            }
            for request in tstring_specialization_requests(&checked) {
                if !tstring_requests.contains(&request) {
                    last_new_callee = String::from("TString");
                    tstring_requests.push(request);
                    grew = true;
                }
            }
            for request in def_specialization_requests(&checked, &templates)
                .into_iter()
                .chain(scalar_range_requests(&checked, &range_templates))
            {
                if conflicted.contains(request.occurrence()) {
                    continue;
                }
                match def_requests
                    .iter()
                    .position(|existing| existing.occurrence() == request.occurrence())
                {
                    None => {
                        last_new_callee = request.callee().to_string();
                        def_requests.push(request);
                        grew = true;
                    }
                    Some(index) if def_requests[index] != request => {
                        conflicted.insert(def_requests.remove(index).occurrence().clone());
                    }
                    Some(_) => {}
                }
            }
            for request in method_specialization_requests(&checked, &variadic_templates) {
                if conflicted.contains(request.occurrence()) {
                    continue;
                }
                match method_requests
                    .iter()
                    .position(|existing| existing.occurrence() == request.occurrence())
                {
                    None => {
                        last_new_callee = format!("{}.{}", request.owner(), request.method());
                        method_requests.push(request);
                        grew = true;
                    }
                    Some(index) if method_requests[index] != request => {
                        conflicted.insert(method_requests.remove(index).occurrence().clone());
                    }
                    Some(_) => {}
                }
            }
            let mut instances_grew = false;
            for request in struct_instance_requests(&checked) {
                if !struct_requests.contains(&request) {
                    last_new_callee = request.template().to_string();
                    struct_requests.push(request);
                    instances_grew = true;
                }
            }
            drop(requests);
            timing::count("tuple_requests", tuple_requests.len() as u64);
            timing::count("tstring_requests", tstring_requests.len() as u64);
            timing::count("def_requests", def_requests.len() as u64);
            timing::count("method_requests", method_requests.len() as u64);
            timing::count("struct_requests", struct_requests.len() as u64);
            if !grew && !instances_grew {
                converged = true;
                break;
            }
            // Instance clones only upgrade calls from the erased template, so
            // an instance discovered at the round cap keeps that path rather
            // than reporting divergence.
            if !grew && round == SPECIALIZATION_ROUNDS {
                converged = true;
                break;
            }
            let Elaborated {
                program: elaborated,
                instances: minted,
            } = {
                let _elaborate = timing::span("elaborate");
                elaborate_with_requests(
                    linked.clone(),
                    &tuple_requests,
                    &tstring_requests,
                    &def_requests,
                    &method_requests,
                    &struct_requests,
                )
                .map_err(CompilerError::Comptime)?
            };
            // Instances the specializer minted on its own (closed applications
            // reached from user code and from other clones) are already
            // served; the checker's recordings of them are not new work.
            for instance in minted {
                if !struct_requests.contains(&instance) {
                    struct_requests.push(instance);
                }
            }
            if !self.allow_executable_module_scope {
                validate_module_scope(&elaborated).map_err(CompilerError::Type)?;
            }
            let _check = timing::span("check");
            checked = crate::checker::check_program_with_materialized_callables(
                &elaborated,
                tuple_materialized_callables(&tuple_requests),
            )
            .map_err(CompilerError::Type)?;
        }
        if !converged {
            return Err(CompilerError::SpecializationDivergence {
                rounds: SPECIALIZATION_ROUNDS,
                callee: last_new_callee,
            });
        }
        let mir = {
            let _lower = timing::span("mir.lower");
            crate::mir::lower_checked_program(&checked)
        };
        timing::count("mir_functions", mir.functions.len() as u64);
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
        })
    }
    /// Execute an ownership-verified program using the configured backend.
    pub fn execute(&self, program: &CompiledProgram) -> Result<Execution, CompilerError> {
        let mut backend = self.backend.instantiate().map_err(|unimplemented| {
            CompilerError::Runtime(RuntimeError::Unsupported(unimplemented))
        })?;
        let mir = {
            let _prepare = timing::span("prepare");
            let elaborated = program.elaborated_mir();
            let _clone = timing::span("mir_clone");
            elaborated.clone()
        };
        if !mir.invariant_errors.is_empty() {
            return Err(CompilerError::Verify(mir.invariant_errors));
        }
        let _run = timing::span("vm");
        backend
            .run_elaborated(mir)
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

fn tuple_specialization_requests(checked: &CheckedProgram) -> Vec<TupleSpecializationRequest> {
    let mut element_sets = Vec::<Vec<Ty>>::new();
    let mut calls = Vec::<(Vec<Ty>, crate::token::SourceSpan)>::new();
    let mut transforms = Vec::<(Vec<Ty>, TupleTransformRequest)>::new();

    for expression in checked.expressions() {
        if let Some(ty) = &expression.ty {
            collect_public_tuple_types(ty, &mut element_sets);
            if matches!(
                &expression.syntax.kind,
                ExprKind::Call {
                    name,
                    param_args,
                    ..
                } if name == "Tuple" && param_args.is_empty()
            ) && let Some(elements) = closed_public_tuple_elements(ty)
            {
                calls.push((elements, expression.syntax.source_span().without_syntax()));
            }
        }
        if let ExprKind::MethodCall {
            method,
            args,
            kwargs,
            ..
        } = &expression.syntax.kind
            && kwargs.is_empty()
            && let Some(receiver) = expression
                .children
                .first()
                .and_then(|id| checked.expression(*id))
                .and_then(|receiver| receiver.ty.as_ref())
                .and_then(closed_public_tuple_elements)
        {
            let transform = match method.as_str() {
                "reverse" if args.is_empty() => Some(TupleTransformRequest::Reverse),
                "concat" if args.len() == 1 => expression
                    .children
                    .get(1)
                    .and_then(|id| checked.expression(*id))
                    .and_then(|argument| argument.ty.as_ref())
                    .and_then(closed_public_tuple_elements)
                    .map(TupleTransformRequest::Concat),
                _ => None,
            };
            if let Some(transform) = transform {
                let request = (receiver, transform);
                if !transforms.contains(&request) {
                    transforms.push(request);
                }
            }
        }
        if let Some(ty) = &expression.place_ty {
            collect_public_tuple_types(ty, &mut element_sets);
        }
        if let Some(ty) = &expression.binding_ty {
            collect_public_tuple_types(ty, &mut element_sets);
        }
    }
    for declaration in checked.declarations() {
        if let Some(ty) = &declaration.ty {
            collect_public_tuple_types(ty, &mut element_sets);
        }
    }

    let mut requests = element_sets
        .into_iter()
        .map(TupleSpecializationRequest::declaration)
        .collect::<Vec<_>>();
    requests.extend(
        calls.into_iter().map(|(elements, occurrence)| {
            TupleSpecializationRequest::bare_call(elements, occurrence)
        }),
    );
    requests.extend(
        transforms.into_iter().map(|(elements, transform)| {
            TupleSpecializationRequest::transform(elements, transform)
        }),
    );
    requests
}

/// The checker-recorded inferred bound-generic instantiations that are closed
/// (fully concrete) and therefore replayable by elaboration. Conflicting
/// recordings for one source occurrence — `comptime for` unrolling duplicates
/// source spans across copies — drop the occurrence: those calls keep the
/// abstract erased path, which is always correct. The result is sorted so
/// request seeding, and therefore specialization order, is deterministic.
fn def_specialization_requests(
    checked: &CheckedProgram,
    templates: &std::collections::HashSet<String>,
) -> Vec<DefSpecializationRequest> {
    use std::collections::hash_map::Entry;
    let mut by_occurrence = std::collections::HashMap::new();
    let mut conflicted = std::collections::HashSet::new();
    for (span, instantiation) in checked.generic_instantiations() {
        if !templates.contains(&instantiation.callee)
            || !instantiation.arguments.iter().all(closed_generic_argument)
        {
            continue;
        }
        let request = DefSpecializationRequest::new(
            span.clone(),
            instantiation.callee.clone(),
            instantiation.arguments.clone(),
        );
        let key = request.occurrence().clone();
        if conflicted.contains(&key) {
            continue;
        }
        match by_occurrence.entry(key) {
            Entry::Occupied(existing) if *existing.get() != request => {
                let (key, _) = existing.remove_entry();
                conflicted.insert(key);
            }
            Entry::Occupied(_) => {}
            Entry::Vacant(slot) => {
                slot.insert(request);
            }
        }
    }
    let mut requests: Vec<DefSpecializationRequest> = by_occurrence.into_values().collect();
    requests.sort_by(|a, b| {
        let key = |request: &DefSpecializationRequest| {
            (
                request.occurrence().source.clone(),
                request.occurrence().span.0,
                request.occurrence().span.1,
                request.callee().to_string(),
            )
        };
        key(a).cmp(&key(b))
    });
    requests
}

/// The linked declaration names of the scalar range-family struct templates,
/// keyed by the plain family name the checker's scalar-`range` inference
/// records (the checker never sees the dropped comptime-class templates, so
/// it cannot record the module-mangled spelling itself).
/// The checker-recorded generic-method instantiations on specialized
/// variadic structs that are closed and therefore replayable as per-call
/// clones. Conflicting recordings for one occurrence drop it, as for defs.
fn method_specialization_requests(
    checked: &CheckedProgram,
    variadic_templates: &std::collections::HashSet<String>,
) -> Vec<MethodSpecializationRequest> {
    use std::collections::hash_map::Entry;
    let mut by_occurrence: std::collections::HashMap<
        mojito_common::token::SourceSpan,
        MethodSpecializationRequest,
    > = std::collections::HashMap::new();
    let mut conflicted = std::collections::HashSet::new();
    for (span, instantiation) in checked.method_instantiations() {
        let specialized_owner = variadic_templates
            .iter()
            .any(|template| instantiation.owner.starts_with(&format!("{template}$")));
        if !specialized_owner || !instantiation.arguments.iter().all(closed_generic_argument) {
            continue;
        }
        let request = MethodSpecializationRequest::new(
            span.clone(),
            instantiation.owner.clone(),
            instantiation.method.clone(),
            instantiation.parameter_names.clone(),
            instantiation.arguments.clone(),
        );
        let key = request.occurrence().clone();
        if conflicted.contains(&key) {
            continue;
        }
        match by_occurrence.entry(key) {
            Entry::Occupied(existing) if *existing.get() != request => {
                let (key, _) = existing.remove_entry();
                conflicted.insert(key);
            }
            Entry::Occupied(_) => {}
            Entry::Vacant(slot) => {
                slot.insert(request);
            }
        }
    }
    let mut requests: Vec<_> = by_occurrence.into_values().collect();
    requests.sort_by(|a, b| {
        let key = |request: &MethodSpecializationRequest| {
            (
                request.occurrence().source.clone(),
                request.occurrence().span.0,
                request.occurrence().span.1,
                request.owner().to_string(),
                request.method().to_string(),
            )
        };
        key(a).cmp(&key(b))
    });
    requests
}

fn scalar_range_template_names(linked: &[Stmt]) -> std::collections::HashMap<&'static str, String> {
    let mut names = std::collections::HashMap::new();
    for statement in linked {
        let StmtKind::Struct { name, .. } = &statement.kind else {
            continue;
        };
        if let Some(family) = crate::types::SCALAR_RANGE_FAMILY
            .iter()
            .find(|family| name == *family || name.ends_with(&format!("${family}")))
        {
            names.entry(*family).or_insert_with(|| name.clone());
        }
    }
    names
}

/// Checker-recorded scalar-range instantiations, rewritten from the plain
/// family name to the linked struct-template name and sorted like
/// [`def_specialization_requests`]. Occurrence conflicts share the caller's
/// def-request conflict handling.
fn scalar_range_requests(
    checked: &CheckedProgram,
    templates: &std::collections::HashMap<&'static str, String>,
) -> Vec<DefSpecializationRequest> {
    let mut requests: Vec<DefSpecializationRequest> = checked
        .generic_instantiations()
        .iter()
        .filter_map(|(span, instantiation)| {
            let linked = templates.get(instantiation.callee.as_str())?;
            if !instantiation.arguments.iter().all(closed_generic_argument) {
                return None;
            }
            Some(DefSpecializationRequest::new(
                span.clone(),
                linked.clone(),
                instantiation.arguments.clone(),
            ))
        })
        .collect();
    requests.sort_by(|a, b| {
        let key = |request: &DefSpecializationRequest| {
            (
                request.occurrence().source.clone(),
                request.occurrence().span.0,
                request.occurrence().span.1,
                request.callee().to_string(),
            )
        };
        key(a).cmp(&key(b))
    });
    requests
}

/// Whether a recorded instantiation argument is concrete enough to replay.
/// The checker-recorded generic-struct applications that are closed and
/// therefore replayable as per-instantiation method clones. A symbolic value
/// argument (`Array[Int, n]` inside a generic body) is not an instance.
fn struct_instance_requests(checked: &CheckedProgram) -> Vec<StructInstanceRequest> {
    checked
        .struct_instantiations()
        .iter()
        .filter(|instantiation| {
            instantiation.arguments.iter().all(|argument| {
                closed_generic_argument(argument)
                    && !matches!(argument, TyArg::Val(CtValue::Param(_)))
            })
        })
        .map(|instantiation| {
            StructInstanceRequest::new(
                instantiation.template.clone(),
                instantiation.arguments.clone(),
            )
        })
        .collect()
}

/// A top-level bare `CtValue::Param` is admitted: it is the checker's
/// callable-value placeholder, which the elaborator's alignment walk consumes
/// and drops (or rejects) — rejecting it here would wrongly exclude every
/// call to a generic with a callable-value parameter. Origins erase from the
/// runtime ABI and never gate replay.
fn closed_generic_argument(argument: &TyArg) -> bool {
    static EMPTY: std::sync::OnceLock<std::collections::HashSet<String>> =
        std::sync::OnceLock::new();
    let empty = EMPTY.get_or_init(std::collections::HashSet::new);
    match argument {
        TyArg::Ty(ty) => tuple_specialization_type_is_closed(ty),
        TyArg::Val(CtValue::Param(_)) => true,
        TyArg::Val(value) => tuple_specialization_value_is_closed_in(value, empty, empty),
        TyArg::Origin(_) => true,
    }
}

fn public_tuple_elements(ty: &Ty) -> Option<Vec<Ty>> {
    let Ty::Struct(name, arguments) = ty else {
        return None;
    };
    if name != crate::types::TUPLE_TYPE_NAME
        && !name.ends_with(&format!("${}", crate::types::TUPLE_TYPE_NAME))
        && !name.starts_with(&format!("{}$", crate::types::TUPLE_TYPE_NAME))
        && !name.contains(&format!("${}$", crate::types::TUPLE_TYPE_NAME))
    {
        return None;
    }
    arguments
        .iter()
        .map(|argument| match argument {
            // A bare literal has a flexible checker type, but public Tuple
            // storage is runtime storage and therefore uses the literal's
            // default materialization.  Canonicalize recursively here so the
            // constructor occurrence, its inferred binding, and later method
            // receiver all request one specialization identity.  Without this,
            // `Tuple(1, 2)` generated `[IntLiteral, IntLiteral]` while
            // `pair.reverse()` requested `[Int, Int]`, leaving the concrete
            // declaration without its discovered transform methods.
            TyArg::Ty(ty) => Some(runtime_tuple_element_type(ty)),
            TyArg::Val(_) | TyArg::Origin(_) => None,
        })
        .collect()
}

/// The checker-typed `t"…"` occurrences whose interleaved element lists are
/// fully concrete and therefore materializable as `TString` specializations.
/// Open occurrences (a t-string inside a still-abstract generic template body)
/// produce no request this round; when the enclosing template is specialized,
/// the cloned body's t-string checks concretely and the fixpoint collects it.
fn tstring_specialization_requests(checked: &CheckedProgram) -> Vec<TStringSpecializationRequest> {
    let mut requests = Vec::new();
    for expression in checked.expressions() {
        if matches!(&expression.syntax.kind, ExprKind::TString { .. })
            && let Some(ty) = &expression.ty
            && let Some(elements) = closed_tstring_elements(ty)
        {
            let request = TStringSpecializationRequest::new(
                elements,
                expression.syntax.source_span().without_syntax(),
            );
            if !requests.contains(&request) {
                requests.push(request);
            }
        }
    }
    requests
}

fn closed_tstring_elements(ty: &Ty) -> Option<Vec<Ty>> {
    let elements = crate::types::tstring_elements(ty)?
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    elements
        .iter()
        .all(tuple_specialization_type_is_closed)
        .then_some(elements)
}

/// A public Tuple implementation is a concrete nominal declaration, so a
/// checker fact from a generic signature cannot request one until every free
/// type/value component has been substituted at an executable use. Origin
/// parameters are deliberately not considered free here: Tuple specialization
/// retains those as explicit inferred parameters for reference-valued elements.
fn closed_public_tuple_elements(ty: &Ty) -> Option<Vec<Ty>> {
    let elements = public_tuple_elements(ty)?;
    elements
        .iter()
        .all(tuple_specialization_type_is_closed)
        .then_some(elements)
}

fn tuple_specialization_type_is_closed(ty: &Ty) -> bool {
    tuple_specialization_type_is_closed_in(ty, &Default::default(), &Default::default())
}

fn tuple_specialization_type_is_closed_in(
    ty: &Ty,
    type_binders: &std::collections::HashSet<String>,
    value_binders: &std::collections::HashSet<String>,
) -> bool {
    match ty {
        Ty::Infer | Ty::SelfType => false,
        Ty::Param { name, .. } => type_binders.contains(name.trim_start_matches('*')),
        Ty::Assoc { base, .. } => {
            tuple_specialization_type_is_closed_in(base, type_binders, value_binders)
        }
        Ty::Dependent(crate::types::DependentType::Indexed { elements, index }) => {
            elements.iter().all(|element| {
                tuple_specialization_type_is_closed_in(element, type_binders, value_binders)
            }) && tuple_specialization_ct_expr_is_closed(index, type_binders, value_binders)
        }
        Ty::Struct(_, arguments) => arguments.iter().all(|argument| match argument {
            TyArg::Ty(ty) => {
                tuple_specialization_type_is_closed_in(ty, type_binders, value_binders)
            }
            // Origins erase from the runtime ABI and carry no type/value binder.
            TyArg::Origin(_) => true,
            TyArg::Val(value) => {
                tuple_specialization_value_is_closed_in(value, type_binders, value_binders)
            }
        }),
        Ty::Func {
            params,
            ret,
            variadic,
            kw_variadic,
            error,
            ..
        } => {
            params.iter().all(|parameter| {
                tuple_specialization_type_is_closed_in(parameter, type_binders, value_binders)
            }) && tuple_specialization_type_is_closed_in(ret, type_binders, value_binders)
                && variadic.as_deref().is_none_or(|ty| {
                    tuple_specialization_type_is_closed_in(ty, type_binders, value_binders)
                })
                && kw_variadic.as_deref().is_none_or(|ty| {
                    tuple_specialization_type_is_closed_in(ty, type_binders, value_binders)
                })
                && error.as_deref().is_none_or(|ty| {
                    tuple_specialization_type_is_closed_in(ty, type_binders, value_binders)
                })
        }
        Ty::GenericFunc {
            decls,
            params,
            ret,
            variadic,
            kw_variadic,
            error,
            ..
        } => {
            let mut nested_types = type_binders.clone();
            let mut nested_values = value_binders.clone();
            for declaration in decls {
                match declaration {
                    crate::types::ParamDecl::Type { name, .. } => {
                        nested_types.insert(name.trim_start_matches('*').to_string());
                    }
                    crate::types::ParamDecl::Value { name, .. } => {
                        nested_values.insert(name.trim_start_matches('*').to_string());
                    }
                }
            }
            tuple_specialization_decls_are_closed(decls, &nested_types, &nested_values)
                && params.iter().all(|parameter| {
                    tuple_specialization_type_is_closed_in(parameter, &nested_types, &nested_values)
                })
                && tuple_specialization_type_is_closed_in(ret, &nested_types, &nested_values)
                && variadic.as_deref().is_none_or(|ty| {
                    tuple_specialization_type_is_closed_in(ty, &nested_types, &nested_values)
                })
                && kw_variadic.as_deref().is_none_or(|ty| {
                    tuple_specialization_type_is_closed_in(ty, &nested_types, &nested_values)
                })
                && error.as_deref().is_none_or(|ty| {
                    tuple_specialization_type_is_closed_in(ty, &nested_types, &nested_values)
                })
        }
        Ty::Overload(types) | Ty::Tuple(types) | Ty::RuntimePack(types) | Ty::Variant(types) => {
            types
                .iter()
                .all(|ty| tuple_specialization_type_is_closed_in(ty, type_binders, value_binders))
        }
        Ty::ComptimeList(element) | Ty::VariadicPack(element) | Ty::Pointer { element, .. } => {
            tuple_specialization_type_is_closed_in(element, type_binders, value_binders)
        }
        Ty::Ref(reference) => {
            tuple_specialization_type_is_closed_in(&reference.referent, type_binders, value_binders)
        }
        Ty::Int
        | Ty::UInt
        | Ty::Bool
        | Ty::StringLiteral
        | Ty::Float64
        | Ty::Dtype
        | Ty::None
        | Ty::Never
        | Ty::IntLiteral
        | Ty::FloatLiteral
        | Ty::Simd { .. }
        | Ty::Error => true,
    }
}

fn tuple_specialization_value_is_closed_in(
    value: &crate::ct::CtValue,
    type_binders: &std::collections::HashSet<String>,
    value_binders: &std::collections::HashSet<String>,
) -> bool {
    use crate::ct::CtValue;
    match value {
        CtValue::Param(name) => value_binders.contains(name.trim_start_matches('*')),
        CtValue::Tuple(values) | CtValue::List(values) => values.iter().all(|value| {
            tuple_specialization_value_is_closed_in(value, type_binders, value_binders)
        }),
        CtValue::Struct { fields, .. } => fields.iter().all(|(_, value)| {
            tuple_specialization_value_is_closed_in(value, type_binders, value_binders)
        }),
        CtValue::Type(ty) | CtValue::Reflected(ty) => {
            tuple_specialization_type_is_closed_in(ty, type_binders, value_binders)
        }
        CtValue::Int(_)
        | CtValue::UInt(_)
        | CtValue::Float(_)
        | CtValue::IntLiteral(_)
        | CtValue::FloatLiteral(_)
        | CtValue::Bool(_)
        | CtValue::Dtype(_)
        | CtValue::Str(_) => true,
    }
}

fn tuple_specialization_decls_are_closed(
    declarations: &[crate::types::ParamDecl],
    type_binders: &std::collections::HashSet<String>,
    value_binders: &std::collections::HashSet<String>,
) -> bool {
    declarations.iter().all(|declaration| match declaration {
        crate::types::ParamDecl::Type {
            callable_bound,
            default,
            constraints,
            ..
        } => {
            callable_bound.as_deref().is_none_or(|ty| {
                tuple_specialization_type_is_closed_in(ty, type_binders, value_binders)
            }) && default.as_deref().is_none_or(|ty| {
                tuple_specialization_type_is_closed_in(ty, type_binders, value_binders)
            }) && constraints.iter().all(|constraint| {
                tuple_specialization_constraint_is_closed(constraint, type_binders, value_binders)
            })
        }
        crate::types::ParamDecl::Value {
            ty,
            default,
            callable_default,
            constraints,
            ..
        } => {
            tuple_specialization_type_is_closed_in(ty, type_binders, value_binders)
                && default.as_ref().is_none_or(|expression| {
                    tuple_specialization_ct_expr_is_closed(expression, type_binders, value_binders)
                })
                && callable_default.as_ref().is_none_or(|default| {
                    tuple_specialization_callable_default_is_closed(
                        default,
                        type_binders,
                        value_binders,
                    )
                })
                && constraints.iter().all(|constraint| {
                    tuple_specialization_constraint_is_closed(
                        constraint,
                        type_binders,
                        value_binders,
                    )
                })
        }
    })
}

fn tuple_specialization_ct_expr_is_closed(
    expression: &crate::ct::CtExpr,
    type_binders: &std::collections::HashSet<String>,
    value_binders: &std::collections::HashSet<String>,
) -> bool {
    use crate::ct::CtExpr;
    match expression {
        CtExpr::Value(value) => {
            tuple_specialization_value_is_closed_in(value, type_binders, value_binders)
        }
        CtExpr::Param(name) => value_binders.contains(name.trim_start_matches('*')),
        CtExpr::Neg(value) => {
            tuple_specialization_ct_expr_is_closed(value, type_binders, value_binders)
        }
        CtExpr::Add(left, right)
        | CtExpr::Sub(left, right)
        | CtExpr::Mul(left, right)
        | CtExpr::FloorDiv(left, right)
        | CtExpr::Mod(left, right)
        | CtExpr::Pow(left, right) => {
            tuple_specialization_ct_expr_is_closed(left, type_binders, value_binders)
                && tuple_specialization_ct_expr_is_closed(right, type_binders, value_binders)
        }
    }
}

fn tuple_specialization_callable_default_is_closed(
    default: &crate::types::CallableDefault,
    type_binders: &std::collections::HashSet<String>,
    value_binders: &std::collections::HashSet<String>,
) -> bool {
    use crate::types::CallableDefault;
    match default {
        CallableDefault::Symbol(_) => true,
        CallableDefault::Parameter(name) => value_binders.contains(name.trim_start_matches('*')),
        CallableDefault::If {
            condition,
            then_value,
            else_value,
        } => {
            tuple_specialization_ct_expr_is_closed(condition, type_binders, value_binders)
                && tuple_specialization_callable_default_is_closed(
                    then_value,
                    type_binders,
                    value_binders,
                )
                && tuple_specialization_callable_default_is_closed(
                    else_value,
                    type_binders,
                    value_binders,
                )
        }
    }
}

fn tuple_specialization_constraint_is_closed(
    constraint: &crate::types::GenericConstraint,
    type_binders: &std::collections::HashSet<String>,
    value_binders: &std::collections::HashSet<String>,
) -> bool {
    use crate::types::GenericConstraint;
    match constraint {
        GenericConstraint::WithMessage(condition, _) => {
            tuple_specialization_constraint_is_closed(condition, type_binders, value_binders)
        }
        GenericConstraint::Conforms { param, .. }
        | GenericConstraint::ConformsPack { param, .. }
        | GenericConstraint::PackPredicate { param, .. } => {
            type_binders.contains(param.trim_start_matches('*'))
        }
        GenericConstraint::PackContains { param, element } => {
            type_binders.contains(param.trim_start_matches('*'))
                && tuple_specialization_constraint_operand_is_closed(
                    element,
                    type_binders,
                    value_binders,
                )
        }
        GenericConstraint::Trivial(_, operand) => {
            tuple_specialization_constraint_operand_is_closed(operand, type_binders, value_binders)
        }
        GenericConstraint::Eq(left, right)
        | GenericConstraint::Ne(left, right)
        | GenericConstraint::Lt(left, right)
        | GenericConstraint::Le(left, right)
        | GenericConstraint::Gt(left, right)
        | GenericConstraint::Ge(left, right) => {
            tuple_specialization_constraint_operand_is_closed(left, type_binders, value_binders)
                && tuple_specialization_constraint_operand_is_closed(
                    right,
                    type_binders,
                    value_binders,
                )
        }
        GenericConstraint::And(left, right) | GenericConstraint::Or(left, right) => {
            tuple_specialization_constraint_is_closed(left, type_binders, value_binders)
                && tuple_specialization_constraint_is_closed(right, type_binders, value_binders)
        }
        GenericConstraint::Not(value) => {
            tuple_specialization_constraint_is_closed(value, type_binders, value_binders)
        }
        GenericConstraint::Bool(_) => true,
    }
}

fn tuple_specialization_constraint_operand_is_closed(
    operand: &crate::types::ConstraintOperand,
    type_binders: &std::collections::HashSet<String>,
    value_binders: &std::collections::HashSet<String>,
) -> bool {
    match operand {
        crate::types::ConstraintOperand::Param(name) => {
            type_binders.contains(name.trim_start_matches('*'))
                || value_binders.contains(name.trim_start_matches('*'))
        }
        crate::types::ConstraintOperand::Value(value) => {
            tuple_specialization_value_is_closed_in(value, type_binders, value_binders)
        }
        crate::types::ConstraintOperand::Type(ty) => {
            tuple_specialization_type_is_closed_in(ty, type_binders, value_binders)
        }
        crate::types::ConstraintOperand::PackLength(name) => {
            type_binders.contains(name.trim_start_matches('*'))
        }
    }
}

fn runtime_tuple_element_type(ty: &Ty) -> Ty {
    match ty {
        Ty::IntLiteral => Ty::Int,
        Ty::FloatLiteral => Ty::Float64,
        Ty::Struct(name, arguments) => Ty::Struct(
            name.clone(),
            arguments
                .iter()
                .map(|argument| match argument {
                    TyArg::Ty(ty) => TyArg::Ty(runtime_tuple_element_type(ty)),
                    TyArg::Val(value) => TyArg::Val(value.clone()),
                    TyArg::Origin(origin) => TyArg::Origin(origin.clone()),
                })
                .collect(),
        ),
        Ty::ComptimeList(element) => {
            Ty::ComptimeList(Box::new(runtime_tuple_element_type(element)))
        }
        Ty::Tuple(elements) => Ty::Tuple(elements.iter().map(runtime_tuple_element_type).collect()),
        Ty::RuntimePack(elements) => {
            Ty::RuntimePack(elements.iter().map(runtime_tuple_element_type).collect())
        }
        Ty::VariadicPack(element) => {
            Ty::VariadicPack(Box::new(runtime_tuple_element_type(element)))
        }
        Ty::Variant(alternatives) => Ty::Variant(
            alternatives
                .iter()
                .map(runtime_tuple_element_type)
                .collect(),
        ),
        Ty::Pointer { element, origin } => Ty::Pointer {
            element: Box::new(runtime_tuple_element_type(element)),
            origin: origin.clone(),
        },
        Ty::Ref(reference) => {
            let mut reference = reference.clone();
            reference.referent = Box::new(runtime_tuple_element_type(&reference.referent));
            Ty::Ref(reference)
        }
        other => other.clone(),
    }
}

fn collect_public_tuple_types(ty: &Ty, output: &mut Vec<Vec<Ty>>) {
    if let Some(elements) = public_tuple_elements(ty) {
        if elements.iter().all(tuple_specialization_type_is_closed) && !output.contains(&elements) {
            output.push(elements.clone());
        }
        for element in &elements {
            collect_public_tuple_types(element, output);
        }
    }
    match ty {
        Ty::Func {
            params,
            ret,
            variadic,
            kw_variadic,
            error,
            ..
        }
        | Ty::GenericFunc {
            params,
            ret,
            variadic,
            kw_variadic,
            error,
            ..
        } => {
            for ty in params {
                collect_public_tuple_types(ty, output);
            }
            collect_public_tuple_types(ret, output);
            if let Some(ty) = variadic {
                collect_public_tuple_types(ty, output);
            }
            if let Some(ty) = kw_variadic {
                collect_public_tuple_types(ty, output);
            }
            if let Some(ty) = error {
                collect_public_tuple_types(ty, output);
            }
        }
        Ty::Overload(types) | Ty::Tuple(types) | Ty::RuntimePack(types) | Ty::Variant(types) => {
            for ty in types {
                collect_public_tuple_types(ty, output);
            }
        }
        Ty::Struct(_, arguments) => {
            for argument in arguments {
                if let TyArg::Ty(ty) = argument {
                    collect_public_tuple_types(ty, output);
                }
            }
        }
        Ty::Param {
            callable_bound: Some(bound),
            ..
        }
        | Ty::Assoc { base: bound, .. }
        | Ty::ComptimeList(bound)
        | Ty::VariadicPack(bound)
        | Ty::Pointer { element: bound, .. } => collect_public_tuple_types(bound, output),
        Ty::Ref(reference) => collect_public_tuple_types(&reference.referent, output),
        _ => {}
    }
}

impl Default for Compiler {
    fn default() -> Self {
        Self::new(LinkOptions::default(), BackendKind::Vm)
    }
}

#[cfg(test)]
mod tuple_callable_closedness_tests {
    use super::*;

    fn type_parameter(name: &str) -> Ty {
        Ty::Param {
            name: name.to_string(),
            bounds: vec!["Movable".to_string()],
            callable_bound: None,
        }
    }

    fn generic_callable(declared: &str, parameter: Ty) -> Ty {
        Ty::GenericFunc {
            environment: crate::origin::CallableEnvironment::Thin,
            decls: vec![crate::types::ParamDecl::Type {
                name: declared.to_string(),
                bounds: vec!["Movable".to_string()],
                callable_bound: None,
                default: None,
                infer_only: false,
                variadic: false,
                constraints: Vec::new(),
            }],
            params: vec![parameter.clone()],
            names: vec!["value".to_string()],
            ret: Box::new(parameter),
            required: vec![true],
            variadic: None,
            kw_variadic: None,
            positional_only: None,
            keyword_only: None,
            raises: false,
            error: None,
            conventions: vec![None],
            ref_params: Box::new(vec![None]),
            ref_return: None,
            transfers: Default::default(),
        }
    }

    #[test]
    fn a_generic_callable_closes_only_its_own_type_parameters() {
        assert!(tuple_specialization_type_is_closed(&generic_callable(
            "T",
            type_parameter("T"),
        )));
        assert!(!tuple_specialization_type_is_closed(&generic_callable(
            "U",
            type_parameter("T"),
        )));
    }
}
