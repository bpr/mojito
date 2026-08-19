//! Experimental Pliron native backend (roadmap section 4, Stage 1).
//!
//! Compiles the scalar subset of verified, drop-elaborated MIR — Int/Bool
//! constants, arithmetic, comparisons, blocks, branches, direct calls,
//! recursion, and return — to pliron's LLVM dialect and on to LLVM IR,
//! bitcode, objects, and host executables. Compilation-only: execution stays
//! with the register VM (`run --backend pliron` is deliberately not offered),
//! and unsupported constructs fail with contextual diagnostics rather than
//! falling back. The backend consumes `MirProgram` facts exclusively; it
//! imports no AST, HIR, or checker representation. Pins, divergence policies,
//! and design notes: `docs/notes/pliron-stage1.md`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;

use pliron::builtin::ops::ModuleOp;
use pliron::context::Context;
use pliron::op::Op;
use pliron::pass::{AnalysisManager, NestedOpsPass, OpPass, Pass, Passes};
use pliron::printable::Printable;

use std::path::Path;

use crate::mir::{MirFunction, MirInstr, MirProgram};
use crate::token::SourceSpan;

mod emit;
mod jit;
mod lower;
mod mangle;

/// Compile the call-graph closure of `options.entries` to an LLVM-dialect
/// module: verify the constructed IR, run the mem2reg/DCE cleanup pipeline,
/// verify again, and cache the canonical Pliron text. The supported-subset
/// contract applies to the reachable set; the first construct outside it
/// fails compilation with a contextual diagnostic.
pub fn compile(
    program: &MirProgram,
    options: &CompileOptions,
) -> Result<NativeModule, PlironError> {
    check_invariants(program)?;
    let reachable = reachable_set(program, &options.entries)?;

    let mut context = Context::new();
    let locator = lower::Locator::new(&mut context, &options.sources);
    let module = ModuleOp::new(&mut context, "mojito".try_into().expect("valid identifier"));

    // Declare every reachable function first (program order, so output is
    // deterministic and calls may reference any of them), then lower bodies.
    let mut signatures = HashMap::new();
    let mut functions = HashMap::new();
    let mut declared = Vec::new();
    for (name, function) in &program.functions {
        if !reachable.contains(name.as_str()) {
            continue;
        }
        let (func_op, signature) = lower::declare_function(&mut context, module, name, function)?;
        functions.insert(
            name.clone(),
            FnMeta {
                mangled: signature.mangled.clone(),
                returns_value: signature.returns_value,
                n_params: function.param_types.len(),
            },
        );
        signatures.insert(name.clone(), signature);
        declared.push((name.as_str(), function, func_op));
    }
    for (name, function, func_op) in declared {
        lower::lower_body(&mut context, name, function, func_op, &signatures, &locator)?;
    }

    verify_module(&context, module)?;
    run_cleanup_passes(&mut context, module)?;
    verify_module(&context, module)?;

    pliron::debug_info::erase_given_names(&mut context, module.get_operation());
    let canonical_text = module.get_operation().disp(&context).to_string();

    Ok(NativeModule {
        context,
        module,
        canonical_text,
        functions,
        exe_wrapper_added: false,
    })
}

/// A compiled LLVM-dialect module plus its cached canonical text and the
/// MIR-name to native-symbol map.
pub struct NativeModule {
    context: Context,
    module: ModuleOp,
    canonical_text: String,
    functions: HashMap<String, FnMeta>,
    exe_wrapper_added: bool,
}

impl NativeModule {
    /// Canonical Pliron textual IR (byte-stable across compilations; cached
    /// before any executable wrapper is synthesized).
    pub fn plir_text(&self) -> &str {
        &self.canonical_text
    }

    /// Textual LLVM IR of the converted module.
    pub fn llvm_ir(&self) -> Result<String, PlironError> {
        emit::llvm_ir(&self.context, self.module)
    }

    /// Write LLVM bitcode to `path`.
    pub fn write_bitcode(&self, path: &Path) -> Result<(), PlironError> {
        emit::write_bitcode(&self.context, self.module, path)
    }

    /// Write a relocatable object file to `path`.
    pub fn write_object(&self, path: &Path) -> Result<(), PlironError> {
        emit::write_object(&self.context, self.module, path)
    }

    /// Link a host executable at `path`. Requires a compiled zero-argument
    /// non-returning `main`; the synthesized C `main` wrapper calls
    /// `__toplevel__` (when compiled), then `main`, then returns 0.
    pub fn write_executable(&mut self, path: &Path) -> Result<(), PlironError> {
        self.ensure_exe_wrapper()?;
        emit::write_executable(&self.context, self.module, path)
    }

    /// JIT-execute a compiled zero-argument `Int`-returning MIR function and
    /// return its value. The differential harness's native side.
    pub fn jit_i64(&self, entry: &str) -> Result<i64, PlironError> {
        let Some(meta) = self.functions.get(entry) else {
            return Err(PlironError {
                function: None,
                kind: PlironErrorKind::Emit(format!("function `{entry}` was not compiled")),
                location: None,
            });
        };
        jit::run_i64(&self.context, self.module, &meta.mangled)
    }

    /// The native symbol a MIR function was mangled to, when compiled.
    pub fn mangled_name(&self, mir_name: &str) -> Option<&str> {
        self.functions
            .get(mir_name)
            .map(|meta| meta.mangled.as_str())
    }

    fn ensure_exe_wrapper(&mut self) -> Result<(), PlironError> {
        if self.exe_wrapper_added {
            return Ok(());
        }
        let mut callees = Vec::new();
        if let Some(toplevel) = self.functions.get("__toplevel__") {
            callees.push(toplevel.mangled.as_str());
        }
        let Some(main) = self.functions.get("main") else {
            return Err(PlironError {
                function: None,
                kind: PlironErrorKind::Emit(
                    "executable emission requires compiling entry `main`".to_string(),
                ),
                location: None,
            });
        };
        if main.n_params != 0 || main.returns_value {
            return Err(PlironError {
                function: Some("main".to_string()),
                kind: PlironErrorKind::Emit(
                    "executable emission requires a zero-argument, non-returning `main`"
                        .to_string(),
                ),
                location: None,
            });
        }
        callees.push(main.mangled.as_str());
        let callees: Vec<String> = callees.iter().map(|s| s.to_string()).collect();
        lower::synthesize_exe_wrapper(&mut self.context, self.module, &callees)?;
        verify_module(&self.context, self.module)?;
        self.exe_wrapper_added = true;
        Ok(())
    }
}

/// Per-compiled-function facts retained for emission and the JIT harness.
struct FnMeta {
    mangled: String,
    returns_value: bool,
    n_params: usize,
}

/// Options for a native compilation.
pub struct CompileOptions {
    /// MIR symbol names to compile from; the backend compiles the transitive
    /// call-graph closure of these entries and applies its supported-subset
    /// contract to that reachable set only. The CLI passes `main` (plus
    /// `__toplevel__` when present); the differential harness passes the
    /// pure scalar entry under test.
    pub entries: Vec<String>,
    /// `(source name, source text)` pairs used to convert MIR span byte
    /// offsets into line/column locations for diagnostics and IR locations.
    pub sources: Vec<(String, String)>,
}

/// What `compile --emit` should produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmitKind {
    /// Canonical Pliron textual IR (stdout-friendly).
    Plir,
    /// Textual LLVM IR (stdout-friendly).
    LlvmIr,
    /// LLVM bitcode (requires an output path).
    Bitcode,
    /// A relocatable object file (requires an output path).
    Object,
    /// A linked host executable (requires an output path).
    Exe,
}

impl EmitKind {
    pub fn parse(s: &str) -> Result<EmitKind, String> {
        match s {
            "plir" => Ok(EmitKind::Plir),
            "ll" => Ok(EmitKind::LlvmIr),
            "bc" => Ok(EmitKind::Bitcode),
            "obj" => Ok(EmitKind::Object),
            "exe" => Ok(EmitKind::Exe),
            other => Err(format!(
                "unknown emit kind '{other}' (expected: plir, ll, bc, obj, exe)"
            )),
        }
    }

    /// Binary kinds must go to a file; text kinds may print to stdout.
    pub fn is_binary(self) -> bool {
        matches!(self, EmitKind::Bitcode | EmitKind::Object | EmitKind::Exe)
    }
}

/// A native-compilation failure with enough context to act on: the MIR
/// function it arose in (unmangled), the failure kind, and the source span
/// when the instruction carried one.
#[derive(Debug)]
pub struct PlironError {
    pub function: Option<String>,
    pub kind: PlironErrorKind,
    pub location: Option<SourceSpan>,
}

impl PlironError {
    fn render_location(&self, sources: &[(String, String)]) -> Option<String> {
        let span = self.location.as_ref()?;
        let name = span.source.as_deref()?;
        let mut rendered = format!("{name}:{}..{}", span.span.0, span.span.1);
        if let Some((_, text)) = sources.iter().find(|(n, _)| n == name)
            && let Some((line, column)) = line_column(text, span.span.0)
        {
            rendered = format!("{name}:{line}:{column}");
        }
        Some(rendered)
    }

    /// Render with line/column resolution against the compilation's sources.
    pub fn display_with_sources(&self, sources: &[(String, String)]) -> String {
        let mut out = String::from("pliron backend: ");
        if let Some(function) = &self.function {
            out.push_str(&format!("in `{function}`: "));
        }
        out.push_str(&self.kind.to_string());
        if let Some(loc) = self.render_location(sources) {
            out.push_str(&format!(" at {loc}"));
        }
        out
    }
}

impl fmt::Display for PlironError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.display_with_sources(&[]))
    }
}

impl std::error::Error for PlironError {}

/// The failure categories of the native backend.
#[derive(Debug)]
pub enum PlironErrorKind {
    /// A construct outside the advertised scalar subset.
    Unsupported { construct: String },
    /// An integer literal that does not fit its materialization target.
    LiteralOutOfRange {
        literal: String,
        target: &'static str,
    },
    /// The producer handed over a program with MIR invariant errors.
    InvariantViolations(Vec<String>),
    /// The pliron verifier rejected constructed or converted IR.
    Verify(String),
    /// LLVM conversion, bitcode/object/executable emission, or JIT failure.
    Emit(String),
}

impl fmt::Display for PlironErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlironErrorKind::Unsupported { construct } => {
                write!(f, "unsupported {construct}")
            }
            PlironErrorKind::LiteralOutOfRange { literal, target } => {
                write!(f, "integer literal {literal} does not fit {target}")
            }
            PlironErrorKind::InvariantViolations(errors) => {
                write!(f, "MIR invariant violations: {}", errors.join("; "))
            }
            PlironErrorKind::Verify(message) => write!(f, "IR verification failed: {message}"),
            PlironErrorKind::Emit(message) => write!(f, "emission failed: {message}"),
        }
    }
}

/// The transitive call-graph closure of `entries` over `MirInstr::Call`
/// edges. Edges to names with no MIR function (builtins, unknowns) are left
/// for body lowering to reject with per-call context.
fn reachable_set<'p>(
    program: &'p MirProgram,
    entries: &[String],
) -> Result<HashSet<&'p str>, PlironError> {
    let functions: HashMap<&str, &MirFunction> = program
        .functions
        .iter()
        .map(|(name, function)| (name.as_str(), function))
        .collect();
    let mut reachable = HashSet::new();
    let mut queue = VecDeque::new();
    for entry in entries {
        let Some((name, _)) = functions.get_key_value(entry.as_str()) else {
            return Err(PlironError {
                function: None,
                kind: PlironErrorKind::Unsupported {
                    construct: format!("entry function `{entry}` (not found in the MIR program)"),
                },
                location: None,
            });
        };
        if reachable.insert(*name) {
            queue.push_back(*name);
        }
    }
    while let Some(name) = queue.pop_front() {
        for block in &functions[name].blocks {
            for instr in &block.instrs {
                if let MirInstr::Call { func, .. } = instr
                    && let Some((callee, _)) = functions.get_key_value(func.0.as_str())
                    && reachable.insert(*callee)
                {
                    queue.push_back(*callee);
                }
            }
        }
    }
    Ok(reachable)
}

/// Rebuild SSA out of the variable-slot allocas and drop the dead scaffolding.
fn run_cleanup_passes(context: &mut Context, module: ModuleOp) -> Result<(), PlironError> {
    let mut module_passes = OpPass::<ModuleOp, Passes>::default();
    let mut per_func = Passes::default();
    per_func.add_pass(OpPass::<
        pliron_llvm::ops::FuncOp,
        pliron::opts::mem2reg::Mem2RegPass,
    >::default());
    per_func.add_pass(OpPass::<pliron_llvm::ops::FuncOp, pliron::opts::dce::DCEPass>::default());
    module_passes.add_pass(NestedOpsPass::new(per_func));
    module_passes
        .run(
            module.get_operation(),
            context,
            &mut AnalysisManager::default(),
        )
        .map_err(|error| PlironError {
            function: None,
            kind: PlironErrorKind::Emit(format!(
                "cleanup pass pipeline failed: {}",
                error.disp(context)
            )),
            location: None,
        })?;
    Ok(())
}

/// Run the pliron verifier over the whole module.
fn verify_module(context: &Context, module: ModuleOp) -> Result<(), PlironError> {
    pliron::op::verify_op(&module, context).map_err(|error| PlironError {
        function: None,
        kind: PlironErrorKind::Verify(error.disp(context).to_string()),
        location: None,
    })
}

/// Refuse a program whose producer recorded invariant errors.
fn check_invariants(program: &MirProgram) -> Result<(), PlironError> {
    if program.invariant_errors.is_empty() {
        return Ok(());
    }
    Err(PlironError {
        function: None,
        kind: PlironErrorKind::InvariantViolations(program.invariant_errors.clone()),
        location: None,
    })
}

/// Byte offset -> 1-based (line, column) in `text`.
fn line_column(text: &str, byte: usize) -> Option<(usize, usize)> {
    if byte > text.len() {
        return None;
    }
    let prefix = &text[..byte];
    let line = prefix.bytes().filter(|b| *b == b'\n').count() + 1;
    let column = prefix
        .rfind('\n')
        .map_or(byte + 1, |newline| byte - newline);
    Some((line, column))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "Invalid IR fails verification": a value-returning function whose body
    /// returns nothing is rejected by the pliron verifier as a
    /// [`PlironErrorKind::Verify`] diagnostic, never a panic.
    #[test]
    fn pliron_verify_rejects_invalid_ir() {
        use pliron::builtin::op_interfaces::SingleBlockRegionInterface;
        use pliron::builtin::types::{IntegerType, Signedness};
        use pliron::op::Op;
        use pliron_llvm::ops::{FuncOp, ReturnOp};
        use pliron_llvm::types::FuncType;

        let mut context = Context::new();
        let module = ModuleOp::new(&mut context, "bad".try_into().expect("identifier"));
        let i64_ty = IntegerType::get(&context, 64, Signedness::Signless);
        let func_ty = FuncType::get(&context, i64_ty.into(), vec![], false);
        let func = FuncOp::new(&mut context, "f".try_into().expect("identifier"), func_ty);
        module.append_operation(&mut context, func.get_operation(), 0);
        let entry = func.get_or_create_entry_block(&mut context);
        let ret = ReturnOp::new(&mut context, None);
        ret.get_operation().insert_at_back(entry, &context);

        let error = verify_module(&context, module)
            .expect_err("a value-less return in an i64 function must fail verification");
        assert!(matches!(error.kind, PlironErrorKind::Verify(_)), "{error}");
        assert!(error.to_string().contains("verification failed"), "{error}");
    }

    /// A program carrying producer invariant errors is refused up front.
    #[test]
    fn pliron_compile_refuses_invariant_errors() {
        let program = MirProgram {
            functions: Vec::new(),
            declarations: Default::default(),
            invariant_errors: vec!["broken".to_string()],
        };
        let options = CompileOptions {
            entries: Vec::new(),
            sources: Vec::new(),
        };
        let Err(error) = compile(&program, &options) else {
            panic!("a program with invariant errors must be refused");
        };
        assert!(matches!(
            error.kind,
            PlironErrorKind::InvariantViolations(_)
        ));
    }
}
