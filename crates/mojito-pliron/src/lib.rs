//! Experimental Pliron native backend (roadmap: native backend, Stages 1-4).
//!
//! Compiles the checked scalar subset of verified, drop-elaborated MIR —
//! Int/UInt/Float64/Bool constants, every scalar operator and the builtin
//! conversions, keyword/default call binding, checked runtime traps through
//! `mjrt_trap`, blocks, branches, direct calls, recursion, and return — plus
//! strings, aggregates, and allocation (Stage 3) and Stage 4's exceptional
//! control flow and references: raising functions with tagged `{tag, ok,
//! err}` outcomes and explicit CFG edges, structural `try`/`except`/`else`/
//! `finally`, flag-guarded destruction consuming drop-elaborated MIR exactly
//! as emitted, references as verified place addresses, and test-lane
//! lifecycle-event tracing — to pliron's LLVM dialect and on to LLVM IR,
//! bitcode, objects, and host executables at `O0` or `release`. The CLI's
//! `run --backend pliron` executes the advertised subset through a temporary
//! executable; every unsupported construct fails with a contextual diagnostic
//! rather than falling back to the VM. The backend consumes `MirProgram`
//! facts exclusively; it imports no AST, HIR, or checker representation.
//! Pins, divergence policies, and design notes: `docs/notes/pliron-stage4.md`
//! (earlier stages: `pliron-stage1.md` through `pliron-stage3.md`).

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;

use pliron::builtin::ops::ModuleOp;
use pliron::context::Context;
use pliron::op::Op;
use pliron::printable::Printable;

use std::path::Path;

use mojito_common::token::SourceSpan;
use mojito_mir::mir::{MirFunction, MirInstr, MirProgram, MirStructDeclaration, Reg};
use mojito_types::types::{Ty, TyArg};

pub use emit::link_object;
pub use mojito_native::native::target::{DebugInfo, EmitKind, NativeTarget, OptLevel};
pub use toolchain::{check_toolchain, set_runtime_override, toolchain_report};

pub mod capability;
pub mod inspect;

mod artifact;
mod debug;
mod emit;
mod jit;
mod lower;
mod pipeline;
mod toolchain;

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
    let specialized =
        mojito_native::native::mono::specialize(program, &options.entries).map_err(|error| {
            PlironError {
                function: error.function,
                kind: PlironErrorKind::Unsupported {
                    construct: error.construct,
                },
                location: None,
            }
        })?;
    let program = &specialized.program;
    let concrete_entries = options
        .entries
        .iter()
        .map(|entry| specialized.entries[entry].clone())
        .collect::<Vec<_>>();
    let reachable = reachable_set(program, &concrete_entries)?;

    let mut context = Context::new();
    let locator = lower::Locator::new(&mut context, &options.sources);
    let module = ModuleOp::new(&mut context, "mojito".try_into().expect("valid identifier"));
    let mut shared = lower::ModuleShared::new(module);
    let declarations: HashMap<String, mojito_mir::mir::MirFunctionDeclaration> = program
        .declarations
        .functions
        .iter()
        .map(|decl| (decl.lowered_name.clone(), decl.clone()))
        .collect();
    let struct_decls: HashMap<&str, &MirStructDeclaration> = program
        .declarations
        .structs
        .iter()
        .map(|decl| (decl.name.as_str(), decl))
        .collect();
    let struct_index = mojito_mir::mir::struct_field_index(&program.declarations);
    let layout = mojito_native::native::layout::LayoutCx {
        target: &options.target,
        structs: &struct_index,
    };

    // Declare every reachable function first (program order, so output is
    // deterministic and calls may reference any of them), then lower bodies.
    let mut signatures = HashMap::new();
    let mut functions = HashMap::new();
    let mut declared = Vec::new();
    for (name, function) in &program.functions {
        if !reachable.contains(name.as_str()) {
            continue;
        }
        let (func_op, signature) =
            lower::declare_function(&mut context, module, name, function, &layout)?;
        functions.insert(
            name.clone(),
            FnMeta {
                mangled: signature.mangled.clone(),
                returns_value: signature.returns_value,
                n_params: function.param_types.len(),
                ret: signature.ret,
                outcome: signature
                    .outcome
                    .as_ref()
                    .map(|outcome| (outcome.layout, outcome.err_offset)),
            },
        );
        signatures.insert(name.clone(), signature);
        declared.push((name.as_str(), function, func_op));
    }
    let env = lower::LowerEnv {
        signatures: &signatures,
        declarations: &declarations,
        struct_decls: &struct_decls,
        layout,
        locator: &locator,
        trace_lifecycle: options.trace_lifecycle,
    };
    for (name, function, func_op) in declared {
        lower::lower_body(&mut context, name, function, func_op, &env, &mut shared)?;
    }

    // Raise lowering leaves unreachable blocks holding the dead remainders
    // of raising MIR blocks; pliron's dominance verifier does not tolerate
    // value uses inside unreachable blocks, so prune them before verifying.
    let mut rewriter =
        pliron::irbuild::rewriter::IRRewriter::<pliron::irbuild::listener::Recorder>::default();
    pliron::opts::simplify_cfg::remove_blocks_inside_op(
        module.get_operation(),
        &mut context,
        &mut rewriter,
    );

    verify_module(&context, module)?;
    pipeline::timing("pliron-passes", || {
        pipeline::Pipeline::run_pliron_passes(&mut context, module)
    })?;
    verify_module(&context, module)?;

    pliron::debug_info::erase_given_names(&mut context, module.get_operation());
    let canonical_text = module.get_operation().disp(&context).to_string();
    // Collected after the cleanup pipeline: passes may delete calls and
    // unreachable blocks, and correlation must see the final IR.
    let debug_table = debug::DebugTable::collect(&context, module, &locator);

    Ok(NativeModule {
        context,
        module,
        canonical_text,
        functions,
        entries: specialized.entries,
        target: options.target,
        exe_wrapper_added: false,
        unhandled_error_declared: shared.declared_rt("mjrt_unhandled_error"),
        debug_table,
    })
}

/// Render the textual LLVM declarations of the runtime ABI contract table
/// (`mojito_native::native::rt_abi`): every exported data symbol and function, in
/// table order. This is the declaration set generated code links against once
/// it starts calling the runtime (Stage 3); until then the backend tests pin
/// it as the mechanical LLVM-side rendering of the contract.
pub fn runtime_declarations() -> String {
    use mojito_native::native::rt_abi::{CAbiTy, RT_DATA_SYMBOLS, RT_SYMBOLS};

    fn llvm_ty(ty: CAbiTy) -> &'static str {
        match ty {
            CAbiTy::U32 => "i32",
            // LLVM integers are signless; U64 and I64 share i64.
            CAbiTy::U64 | CAbiTy::I64 => "i64",
            CAbiTy::F64 => "double",
            CAbiTy::PtrConstU8 | CAbiTy::PtrMutU8 => "ptr",
        }
    }

    let mut out = String::new();
    for (symbol, ty) in RT_DATA_SYMBOLS {
        out.push_str(&format!("@{symbol} = external global {}\n", llvm_ty(*ty)));
    }
    for sig in RT_SYMBOLS {
        let ret = sig.ret.map(llvm_ty).unwrap_or("void");
        let params: Vec<&str> = sig.params.iter().map(|(_, ty)| llvm_ty(*ty)).collect();
        let attrs = if sig.noreturn { " noreturn" } else { "" };
        out.push_str(&format!(
            "declare {ret} @{}({}){attrs}\n",
            sig.symbol,
            params.join(", ")
        ));
    }
    out
}

/// A compiled LLVM-dialect module plus its cached canonical text and the
/// MIR-name to native-symbol map.
pub struct NativeModule {
    context: Context,
    module: ModuleOp,
    canonical_text: String,
    functions: HashMap<String, FnMeta>,
    entries: HashMap<String, String>,
    target: NativeTarget,
    exe_wrapper_added: bool,
    /// Whether body lowering declared `mjrt_unhandled_error` (the wrapper
    /// must not redeclare it).
    unhandled_error_declared: bool,
    /// Per-function debug facts for the emission-time DWARF attach.
    debug_table: debug::DebugTable,
}

impl NativeModule {
    /// Canonical Pliron textual IR (byte-stable across compilations; cached
    /// before any executable wrapper is synthesized).
    pub fn plir_text(&self) -> &str {
        &self.canonical_text
    }

    /// Textual LLVM IR of the converted module.
    pub fn llvm_ir(&self, opt: OptLevel) -> Result<String, PlironError> {
        emit::llvm_ir(&self.context, self.module, &self.target, opt)
    }

    /// Write LLVM bitcode to `path`.
    pub fn write_bitcode(
        &self,
        path: &Path,
        opt: OptLevel,
        debug: DebugInfo,
    ) -> Result<(), PlironError> {
        emit::write_bitcode(
            &self.context,
            self.module,
            &self.target,
            path,
            opt,
            self.debug_policy(debug),
        )
    }

    /// Write a relocatable object file to `path`, with its sidecar
    /// `<path>.link.tsv` manifest. The object contains the synthesized C
    /// `main` wrapper, so `link_object` (the CLI `link` verb) can turn it
    /// into an executable with nothing but the manifest and the runtime.
    pub fn write_object(
        &mut self,
        path: &Path,
        opt: OptLevel,
        debug: DebugInfo,
    ) -> Result<(), PlironError> {
        self.ensure_exe_wrapper()?;
        emit::write_object(
            &self.context,
            self.module,
            &self.target,
            path,
            opt,
            self.debug_policy(debug),
        )
    }

    /// Link an executable at `path`. Requires a compiled zero-argument
    /// non-returning `main`; the synthesized C `main` wrapper checks in the
    /// linked `mojito-runtime` (referencing its version symbol), calls
    /// `__toplevel__` (when compiled), then `main`, then returns 0.
    pub fn write_executable(
        &mut self,
        path: &Path,
        opt: OptLevel,
        debug: DebugInfo,
    ) -> Result<(), PlironError> {
        self.ensure_exe_wrapper()?;
        emit::write_executable(
            &self.context,
            self.module,
            &self.target,
            path,
            opt,
            self.debug_policy(debug),
        )
    }

    /// [`NativeModule::write_executable`] instrumented with AddressSanitizer
    /// (and its leak checking) — the sanitizer acceptance lane.
    pub fn write_executable_sanitized(
        &mut self,
        path: &Path,
        opt: OptLevel,
        debug: DebugInfo,
    ) -> Result<(), PlironError> {
        self.ensure_exe_wrapper()?;
        emit::write_executable_sanitized(
            &self.context,
            self.module,
            &self.target,
            path,
            opt,
            self.debug_policy(debug),
        )
    }

    fn debug_policy(&self, level: DebugInfo) -> debug::DebugPolicy<'_> {
        debug::DebugPolicy {
            level,
            table: &self.debug_table,
        }
    }

    /// Attach debug information to the converted module in a scratch file
    /// and report the functions that degraded to subprogram-only
    /// correlation. The corpus test pins this to empty; production emission
    /// degrades identically but silently.
    pub fn debug_degradations(&self) -> Result<Vec<String>, PlironError> {
        emit::debug_degradations(&self.context, self.module, &self.target, &self.debug_table)
    }

    /// JIT-execute a compiled zero-argument value-returning MIR function and
    /// return its typed value. The differential harness's native side.
    pub fn jit_value(&self, entry: &str, opt: OptLevel) -> Result<JitValue, PlironError> {
        self.jit_value_with_symbols(entry, opt, &[])
    }

    /// [`NativeModule::jit_value`] with an explicit mapping from external
    /// symbols the module references (runtime-contract functions such as
    /// `mjrt_trap`) to in-process addresses. The differential harness passes
    /// the linked `mojito-runtime` exports so JIT resolution is deterministic.
    pub fn jit_value_with_symbols(
        &self,
        entry: &str,
        opt: OptLevel,
        symbols: &[(&str, u64)],
    ) -> Result<JitValue, PlironError> {
        if mojito_native::native::target::Triple::host() != Some(self.target.triple) {
            return Err(PlironError {
                function: None,
                kind: PlironErrorKind::Emit(format!(
                    "cannot JIT-execute target '{}' on this host",
                    self.target.triple.name()
                )),
                location: None,
            });
        }
        let concrete_entry = self.entries.get(entry).map_or(entry, String::as_str);
        let Some(meta) = self.functions.get(concrete_entry) else {
            return Err(PlironError {
                function: None,
                kind: PlironErrorKind::Emit(format!("function `{entry}` was not compiled")),
                location: None,
            });
        };
        if meta.outcome.is_some() {
            return Err(PlironError {
                function: Some(entry.to_string()),
                kind: PlironErrorKind::Emit(format!(
                    "cannot JIT-execute raising entry `{entry}` (tagged-outcome signature)"
                )),
                location: None,
            });
        }
        jit::run_value(
            &self.context,
            self.module,
            &self.target,
            &meta.mangled,
            meta.ret,
            opt,
            symbols,
        )
    }

    /// JIT-execute a compiled zero-argument `Int`-returning MIR function at
    /// `O0` and return its value.
    pub fn jit_i64(&self, entry: &str) -> Result<i64, PlironError> {
        match self.jit_value(entry, OptLevel::O0)? {
            JitValue::Int(value) => Ok(value),
            other => Err(PlironError {
                function: None,
                kind: PlironErrorKind::Emit(format!(
                    "entry `{entry}` returned {other:?}, not an Int"
                )),
                location: None,
            }),
        }
    }

    /// The native symbol a MIR function was mangled to, when compiled.
    pub fn mangled_name(&self, mir_name: &str) -> Option<&str> {
        let mir_name = self.entries.get(mir_name).map_or(mir_name, String::as_str);
        self.functions
            .get(mir_name)
            .map(|meta| meta.mangled.as_str())
    }

    /// Synthesize the executable's C `main` wrapper into the module (once;
    /// later calls are no-ops). `write_object` and the executable writers do
    /// this themselves; call it before `llvm_ir` or `write_bitcode` when the
    /// emitted artifact will be linked by hand, so the textual IR or bitcode
    /// resolves `main` exactly as `--emit exe` would. Requires a compiled
    /// zero-argument, non-returning `main` entry.
    pub fn ensure_exe_wrapper(&mut self) -> Result<(), PlironError> {
        if self.exe_wrapper_added {
            return Ok(());
        }
        let mut callees = Vec::new();
        let toplevel_name = self
            .entries
            .get("__toplevel__")
            .map_or("__toplevel__", String::as_str);
        if let Some(toplevel) = self.functions.get(toplevel_name) {
            callees.push((toplevel.mangled.clone(), toplevel.outcome));
        }
        let main_name = self.entries.get("main").map_or("main", String::as_str);
        let Some(main) = self.functions.get(main_name) else {
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
        callees.push((main.mangled.clone(), main.outcome));
        lower::synthesize_exe_wrapper(
            &mut self.context,
            self.module,
            &callees,
            self.unhandled_error_declared,
        )?;
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
    ret: RetKind,
    /// The tagged-outcome storage layout and error-slot offset of a raising
    /// function (the executable wrapper reports a propagated error; the JIT
    /// refuses raising entries).
    outcome: Option<(mojito_native::native::layout::Layout, u64)>,
}

/// A typed result of a JIT-executed entry, tagged by the entry's [`RetKind`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum JitValue {
    Int(i64),
    UInt(u64),
    Float64(f64),
    Bool(bool),
}

/// The native return-value kind of a compiled function, derived from its
/// checked MIR return type. Drives the typed JIT harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetKind {
    /// `Ty::None` — no return value.
    Void,
    /// `Ty::Int` — signed i64.
    I64,
    /// `Ty::UInt` — the i64 bits reinterpret as u64.
    U64,
    /// `Ty::Float64` — f64.
    F64,
    /// `Ty::Bool` — i1 (read as u8, low bit significant).
    Bool,
    /// `Ty::Pointer` — an opaque pointer. Compiles; the value-comparing JIT
    /// harness refuses to read it (a raw address has no VM display analog).
    Ptr,
    /// A width-1 SIMD scalar alias (`Ty::Simd { dtype, width: 1 }`): the
    /// native return is the lane type; the JIT reads it back as the VM's
    /// mathematical lane value (sign/zero-extended integer, f64 view of a
    /// `Float32`).
    Sized(mojito_ast::ast::Dtype),
}

/// A checked runtime trap the native backend guards explicitly. Trap blocks
/// call the runtime's `mjrt_trap` with [`TrapCategory::code`]; the runtime
/// reports on stderr and exits with [`TrapCategory::exit_code`], so a
/// trapping native executable's exit status identifies the category. The
/// scalar categories map onto the VM's `RuntimeError::TypeError` message for
/// differential comparison; the runtime-service categories have no VM analog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrapCategory {
    /// `//` or `%` with a zero divisor on Int/UInt (`runtime::nonzero`).
    DivModZero,
    /// `**` exponent outside `0 ..= u32::MAX` (`runtime::pow_exp`).
    PowExponent,
    /// `mjrt_alloc` exhaustion or invalid alignment.
    AllocFailure,
    /// `mjrt_write_stdout` failure.
    StdoutFailure,
    /// An uncaught `raise` — `mjrt_unhandled_error` reports the message on
    /// stderr and exits with this category's code (no `try` lowering exists
    /// before Stage 4, so every runtime raise is dynamically unhandled).
    UnhandledError,
    /// `mjrt_read_line` stdin read failure (EOF is not a failure).
    StdinFailure,
    /// An uncatchable language-level `abort` with a separately reported
    /// dynamic message.
    Abort,
    PointerDangling,
    PointerUseAfterFree,
    PointerDoubleFree,
    UninitRead,
    UninitTake,
    UninitDestroy,
}

impl TrapCategory {
    const ALL: [TrapCategory; 13] = [
        TrapCategory::DivModZero,
        TrapCategory::PowExponent,
        TrapCategory::AllocFailure,
        TrapCategory::StdoutFailure,
        TrapCategory::UnhandledError,
        TrapCategory::StdinFailure,
        TrapCategory::Abort,
        TrapCategory::PointerDangling,
        TrapCategory::PointerUseAfterFree,
        TrapCategory::PointerDoubleFree,
        TrapCategory::UninitRead,
        TrapCategory::UninitTake,
        TrapCategory::UninitDestroy,
    ];

    /// Stable small code of this category (used in exit codes and manifests),
    /// in lockstep with the `rt_abi` trap constants.
    pub fn code(self) -> u8 {
        use mojito_native::native::rt_abi;
        let code = match self {
            TrapCategory::DivModZero => rt_abi::TRAP_DIV_MOD_ZERO,
            TrapCategory::PowExponent => rt_abi::TRAP_POW_EXPONENT,
            TrapCategory::AllocFailure => rt_abi::TRAP_ALLOC_FAILURE,
            TrapCategory::StdoutFailure => rt_abi::TRAP_STDOUT_FAILURE,
            TrapCategory::UnhandledError => rt_abi::TRAP_UNHANDLED_ERROR,
            TrapCategory::StdinFailure => rt_abi::TRAP_STDIN_FAILURE,
            TrapCategory::Abort => rt_abi::TRAP_ABORT,
            TrapCategory::PointerDangling => rt_abi::TRAP_POINTER_DANGLING,
            TrapCategory::PointerUseAfterFree => rt_abi::TRAP_POINTER_USE_AFTER_FREE,
            TrapCategory::PointerDoubleFree => rt_abi::TRAP_POINTER_DOUBLE_FREE,
            TrapCategory::UninitRead => rt_abi::TRAP_UNINIT_READ,
            TrapCategory::UninitTake => rt_abi::TRAP_UNINIT_TAKE,
            TrapCategory::UninitDestroy => rt_abi::TRAP_UNINIT_DESTROY,
        };
        code as u8
    }

    /// The process exit status a native trap reports: `64 + code`.
    pub fn exit_code(self) -> u8 {
        64 + self.code()
    }

    /// The category a trapping native process reported, if any.
    pub fn from_exit_code(code: i32) -> Option<TrapCategory> {
        TrapCategory::ALL
            .into_iter()
            .find(|category| i32::from(category.exit_code()) == code)
    }

    /// The runtime's stderr text for this category (`trap_message` in
    /// `crates/mojito-runtime`; the scalar categories reuse the VM's
    /// runtime-error text so both backends diagnose identically).
    pub fn runtime_message(self) -> &'static str {
        match self {
            TrapCategory::DivModZero => "integer division or modulo by zero",
            TrapCategory::PowExponent => {
                "'**' exponent must be a non-negative Int that fits in 32 bits"
            }
            TrapCategory::AllocFailure => "allocation failed",
            TrapCategory::StdoutFailure => "stdout write failed",
            TrapCategory::UnhandledError => "unhandled error",
            TrapCategory::StdinFailure => "stdin read failed",
            TrapCategory::Abort => "abort",
            TrapCategory::PointerDangling => "vm: dereference of dangling Pointer",
            TrapCategory::PointerUseAfterFree => "vm: use after Pointer deallocation",
            TrapCategory::PointerDoubleFree => "vm: double free of Pointer allocation",
            TrapCategory::UninitRead => "vm: read of uninitialized MaybeUninit storage",
            TrapCategory::UninitTake => "vm: take of uninitialized MaybeUninit storage",
            TrapCategory::UninitDestroy => "vm: destroy of uninitialized MaybeUninit storage",
        }
    }

    /// The VM `RuntimeError::TypeError` message this trap mirrors, for the
    /// categories with a VM analog.
    pub fn vm_message(self) -> Option<&'static str> {
        matches!(
            self,
            TrapCategory::DivModZero
                | TrapCategory::PowExponent
                | TrapCategory::PointerDangling
                | TrapCategory::PointerUseAfterFree
                | TrapCategory::PointerDoubleFree
                | TrapCategory::UninitRead
                | TrapCategory::UninitTake
                | TrapCategory::UninitDestroy
        )
        .then(|| self.runtime_message())
    }

    /// The category whose VM message `message` carries, if any.
    pub fn from_vm_message(message: &str) -> Option<TrapCategory> {
        [
            TrapCategory::DivModZero,
            TrapCategory::PowExponent,
            TrapCategory::PointerDangling,
            TrapCategory::PointerUseAfterFree,
            TrapCategory::PointerDoubleFree,
            TrapCategory::UninitRead,
            TrapCategory::UninitTake,
            TrapCategory::UninitDestroy,
        ]
        .into_iter()
        .find(|category| {
            category
                .vm_message()
                .is_some_and(|text| message.contains(text))
        })
    }
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
    /// The checked native target. Its triple and pinned data-layout string
    /// stamp every emitted LLVM module; JIT execution requires the host.
    pub target: NativeTarget,
    /// Emit ordered lifecycle-event reports (`mjrt_trace`) at destructor
    /// dispatches, consumes, raises, and catches. Test-lane only: default
    /// emission never traces, and the trace writes to stderr so stdout byte
    /// parity is untouched.
    pub trace_lifecycle: bool,
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

/// The transitive call-graph closure of `entries`: direct `MirInstr::Call`
/// edges, checker-resolved `MethodCall` targets, constructor calls to a
/// declared struct (the exact `__init__` or its unique arity overload — the
/// VM's `overload_name` policy), and lifecycle edges — every struct type a
/// reachable function mentions (transitively through declared field types)
/// contributes its compiled `__deinit__`/`__copyinit__`, because drops and
/// copies execute those bodies without any call instruction naming them.
/// Edges to names with no MIR function (builtins, unknowns) are left for body
/// lowering to reject with per-call context.
fn reachable_set<'p>(
    program: &'p MirProgram,
    entries: &[String],
) -> Result<HashSet<&'p str>, PlironError> {
    let functions: HashMap<&str, &MirFunction> = program
        .functions
        .iter()
        .map(|(name, function)| (name.as_str(), function))
        .collect();
    let structs: HashMap<&str, &MirStructDeclaration> = program
        .declarations
        .structs
        .iter()
        .map(|decl| (decl.name.as_str(), decl))
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
    let mut seen_structs: HashSet<&str> = HashSet::new();
    while let Some(name) = queue.pop_front() {
        let function = functions[name];
        let mut discovered = Vec::new();
        for ty in function
            .param_types
            .iter()
            .chain(function.ret_ty.as_ref())
            .chain(function.var_tys.values())
            .chain(function.reg_types.values())
        {
            collect_struct_types(ty, &structs, &mut seen_structs, &mut discovered);
        }
        for struct_name in discovered {
            for method in ["__deinit__", "__copyinit__"] {
                // The nominal String's copy constructor is bridged natively
                // (its stdlib byte loop needs machinery outside this stage);
                // its `__deinit__` compiles from real MIR and stays an edge.
                if method == "__copyinit__"
                    && mojito_symbol::symbol::is_stdlib_string_struct(struct_name)
                {
                    continue;
                }
                let lifecycle = format!("{struct_name}.{method}");
                if let Some((callee, _)) = functions.get_key_value(lifecycle.as_str())
                    && reachable.insert(*callee)
                {
                    queue.push_back(*callee);
                }
            }
        }
        visit_call_edges(
            &function.blocks,
            function,
            &functions,
            &structs,
            &mut reachable,
            &mut queue,
        );
    }
    Ok(reachable)
}

/// Collect the call edges under `blocks` into the reachability worklist,
/// recursing into `try` sub-regions (call sites inside a body, handler,
/// `else`, or `finally` execute like any other).
fn visit_call_edges<'p>(
    blocks: &'p [mojito_mir::mir::MirBlock],
    function: &'p MirFunction,
    functions: &HashMap<&'p str, &'p MirFunction>,
    structs: &HashMap<&str, &MirStructDeclaration>,
    reachable: &mut HashSet<&'p str>,
    queue: &mut VecDeque<&'p str>,
) {
    for block in blocks {
        for instr in &block.instrs {
            if let MirInstr::Try {
                body,
                handler,
                orelse,
                finalbody,
                ..
            } = instr
            {
                visit_call_edges(body, function, functions, structs, reachable, queue);
                if let Some((_, handler_blocks)) = handler {
                    visit_call_edges(
                        handler_blocks,
                        function,
                        functions,
                        structs,
                        reachable,
                        queue,
                    );
                }
                if let Some(orelse_blocks) = orelse {
                    visit_call_edges(
                        orelse_blocks,
                        function,
                        functions,
                        structs,
                        reachable,
                        queue,
                    );
                }
                if let Some(final_blocks) = finalbody {
                    visit_call_edges(final_blocks, function, functions, structs, reachable, queue);
                }
                continue;
            }
            let mut targets: Vec<&'p str> = Vec::new();
            let push_named = |targets: &mut Vec<&'p str>, name: &str| {
                if let Some((callee, _)) = functions.get_key_value(name) {
                    targets.push(*callee);
                }
            };
            match instr {
                // Intercepted allocation entry points lower as runtime
                // intrinsics; their element-erased stdlib bodies must not
                // be declared.
                MirInstr::Call { func, .. } if lower::intercepted_call(&func.0) => {}
                // `print` or `String(...)` of a nominal struct calls its
                // `write_to` instance in the lowered expansion.
                MirInstr::Call { func, args, .. }
                    if matches!(func.0.as_str(), "print" | "String") =>
                {
                    for arg in args {
                        if let Some(Ty::Struct(name, _)) = function.reg_types.get(&arg.0) {
                            let prefix = format!("{name}.write_to");
                            for (fname, _) in functions.iter() {
                                if fname.starts_with(prefix.as_str()) {
                                    targets.push(*fname);
                                }
                            }
                        }
                    }
                }
                MirInstr::Call {
                    func, args, kwargs, ..
                } => match functions.get_key_value(func.0.as_str()) {
                    Some((callee, _)) => targets.push(*callee),
                    None => {
                        if let Some(callee) =
                            constructor_init_target(functions, structs, &func.0, args.len(), kwargs)
                        {
                            targets.push(callee);
                        }
                    }
                },
                // Pointer-receiver methods dispatch to runtime
                // intrinsics, never to compiled stdlib bodies.
                MirInstr::MethodCall {
                    recv,
                    resolved: Some(resolved),
                    ..
                } => {
                    if !matches!(function.reg_types.get(&recv.0), Some(Ty::Pointer { .. })) {
                        push_named(&mut targets, resolved);
                    }
                }
                // The VM-synthesized `Writer.write` dispatch lowers to
                // `write_string` calls that exist only in the expansion; the
                // builtin-string writer displays nominal arguments through
                // their `write_to` instances.
                MirInstr::MethodCall {
                    recv,
                    method,
                    resolved: None,
                    args,
                    ..
                } if method == "write" => {
                    if let Some(Ty::Struct(name, _)) = function.reg_types.get(&recv.0) {
                        push_named(&mut targets, &format!("{name}.write_string"));
                    }
                    if matches!(function.reg_types.get(&recv.0), Some(Ty::StringLiteral)) {
                        for arg in args {
                            if let Some(Ty::Struct(name, _)) = function.reg_types.get(&arg.0) {
                                let prefix = format!("{name}.write_to");
                                for (fname, _) in functions.iter() {
                                    if fname.starts_with(prefix.as_str()) {
                                        targets.push(*fname);
                                    }
                                }
                            }
                        }
                    }
                }
                // A scalar/literal Hashable leaf lowers to the hasher's
                // compiled `_update_with_simd` (a string literal through the
                // nominal String's `__hash__` instance bound to that hasher).
                MirInstr::MethodCall {
                    recv,
                    method,
                    resolved: None,
                    args,
                    ..
                } if method == "__hash__" && args.len() == 1 => {
                    let hasher = function.reg_types.get(&args[0].0).map(|ty| match ty {
                        Ty::Ref(reference) => &reference.referent,
                        other => other,
                    });
                    if let Some(Ty::Struct(hasher, _)) = hasher {
                        let prefix = format!("{hasher}._update_with_simd");
                        for (fname, _) in functions.iter() {
                            if fname.starts_with(prefix.as_str()) {
                                targets.push(*fname);
                            }
                        }
                        if matches!(function.reg_types.get(&recv.0), Some(Ty::StringLiteral)) {
                            // The nominal String's symbols are module-qualified.
                            for (fname, _) in functions.iter() {
                                if fname.rsplit_once(".__hash__").is_some_and(|(owner, rest)| {
                                    owner
                                        .rsplit('$')
                                        .next()
                                        .is_some_and(mojito_symbol::symbol::is_stdlib_string_struct)
                                        && (rest.is_empty() || rest.starts_with('$'))
                                }) {
                                    targets.push(*fname);
                                }
                            }
                        }
                    }
                }
                // Iterator instructions carry their targets as symbols rather
                // than call edges; monomorphization has already retargeted
                // them to concrete instances.
                MirInstr::GetIter { prepare, .. } => {
                    for step in prepare {
                        push_named(&mut targets, step);
                    }
                }
                MirInstr::HasNext {
                    method: Some(method),
                    ..
                } => push_named(&mut targets, method),
                MirInstr::Next {
                    call: Some(call), ..
                }
                | MirInstr::TryNext { call, .. } => push_named(&mut targets, &call.target),
                // Subscript instructions carry their checker-selected (and
                // mono-retargeted) targets on the instruction.
                MirInstr::Index {
                    call: Some(call), ..
                }
                | MirInstr::Slice {
                    call: Some(call), ..
                }
                | MirInstr::MultiIndex {
                    call: Some(call), ..
                } => push_named(&mut targets, &call.target),
                MirInstr::MultiSet { call, .. } => push_named(&mut targets, &call.target),
                // A retained callable names its lifted body on the
                // instruction; the per-target thunk calls it, so the body
                // must be declared and compiled even when every invocation
                // is indirect.
                MirInstr::MakeClosure { function, .. } => {
                    push_named(&mut targets, function);
                }
                MirInstr::Const {
                    k: mojito_mir::mir::Const::Function(function),
                    ..
                } => {
                    push_named(&mut targets, function);
                }
                // Scalar/SIMD construction over a concrete Intable struct
                // invokes its checker-selected `__int__` in the lowered
                // expansion, so retain that implicit edge.
                MirInstr::MakeSimd { elems, .. } => {
                    for elem in elems {
                        if let Some(Ty::Struct(name, _)) = function.reg_types.get(&elem.0) {
                            push_named(&mut targets, &format!("{name}.__int__"));
                        }
                    }
                }
                // A `^` transfer of a struct with a user `__moveinit__` runs
                // it (the VM's `move_value`); the edge exists only in the
                // lowered expansion.
                MirInstr::UseVar {
                    var,
                    mode: mojito_mir::mir::UseMode::Move,
                    ..
                } => {
                    if let Some(Ty::Struct(name, _)) = function.var_tys.get(var) {
                        push_named(&mut targets, &format!("{name}.__moveinit__"));
                    }
                }
                _ => {}
            }
            for callee in targets {
                if reachable.insert(callee) {
                    queue.push_back(callee);
                }
            }
        }
    }
}

/// The `__init__` a constructor call to struct `name` executes: the exact
/// name when compiled, else the unique overload taking `argc + 1` parameters
/// (counting `out self`) — the VM's `overload_name` policy. The
/// `Type(copy=value)` form runs the fieldwise copy path, not `__init__`.
fn constructor_init_target<'p>(
    functions: &HashMap<&'p str, &'p MirFunction>,
    structs: &HashMap<&str, &MirStructDeclaration>,
    name: &str,
    argc: usize,
    kwargs: &[(String, Reg)],
) -> Option<&'p str> {
    if !structs.contains_key(name) {
        return None;
    }
    // The nominal String constructor is the backend's literal bridge; its
    // stdlib `__init__` bodies never lower.
    if mojito_symbol::symbol::is_stdlib_string_struct(name) {
        return None;
    }
    if argc == 0 && kwargs.len() == 1 && kwargs[0].0 == "copy" {
        return None;
    }
    let init = format!("{name}.__init__");
    if let Some((callee, _)) = functions.get_key_value(init.as_str()) {
        return Some(*callee);
    }
    let mut matches = functions.iter().filter(|(fname, function)| {
        mojito_symbol::symbol::is_overload_of(fname, &init) && function.n_params == argc + 1
    });
    let first = *matches.next()?.0;
    matches.next().is_none().then_some(first)
}

/// Record every declared struct name `ty` mentions — transitively through
/// declared field types — that `seen` has not recorded yet, appending fresh
/// names to `discovered`.
fn collect_struct_types<'p>(
    ty: &'p Ty,
    structs: &HashMap<&'p str, &'p MirStructDeclaration>,
    seen: &mut HashSet<&'p str>,
    discovered: &mut Vec<&'p str>,
) {
    match ty {
        Ty::Struct(name, args) => {
            if let Some((key, decl)) = structs.get_key_value(name.as_str())
                && seen.insert(*key)
            {
                discovered.push(*key);
                for (_, field) in &decl.fields {
                    collect_struct_types(field, structs, seen, discovered);
                }
            }
            for arg in args {
                if let TyArg::Ty(inner) = arg {
                    collect_struct_types(inner, structs, seen, discovered);
                }
            }
        }
        Ty::Tuple(elements) | Ty::RuntimePack(elements) | Ty::Variant(elements) => {
            for element in elements {
                collect_struct_types(element, structs, seen, discovered);
            }
        }
        Ty::Pointer { element, .. } => collect_struct_types(element, structs, seen, discovered),
        Ty::Ref(ref_ty) => collect_struct_types(&ref_ty.referent, structs, seen, discovered),
        _ => {}
    }
}

/// Run the pliron verifier over the whole module.
fn verify_module(context: &Context, module: ModuleOp) -> Result<(), PlironError> {
    pliron::op::verify_op(&module, context).map_err(|error| {
        if std::env::var_os("MOJITO_PLIRON_DUMP_ON_VERIFY_ERROR").is_some() {
            eprintln!("verify debug: {error:?}");
            eprintln!("{}", module.get_operation().disp(context));
        }
        PlironError {
            function: None,
            kind: PlironErrorKind::Verify(error.disp(context).to_string()),
            location: None,
        }
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

    /// Reachability follows constructor calls to their `__init__` overload and
    /// pulls the lifecycle methods (`__deinit__`/`__copyinit__`) of every
    /// struct type a reachable function mentions — those bodies run at drops
    /// and copies without any call instruction naming them.
    #[test]
    fn reachability_follows_constructor_and_lifecycle_edges() {
        use mojito_mir::mir::{FuncRef, MirDeclarations};

        fn test_function(param_types: Vec<Ty>, instrs: Vec<MirInstr>) -> MirFunction {
            let n_params = param_types.len();
            MirFunction {
                blocks: vec![mojito_mir::mir::MirBlock {
                    instrs,
                    term: mojito_mir::mir::MirTerm::Return(None),
                }],
                n_regs: 8,
                n_vars: n_params,
                var_names: (0..n_params).map(|i| format!("v{i}")).collect(),
                n_params,
                param_types,
                owned_params: vec![false; n_params],
                deinit_params: vec![false; n_params],
                ref_params: vec![false; n_params],
                returns_reference: false,
                var_tys: HashMap::new(),
                ret_ty: Some(mojito_types::types::Ty::None),
                raises: false,
                error_ty: None,
                spans: Default::default(),
                reg_types: HashMap::new(),
            }
        }

        let point = Ty::Struct("Point".to_string(), Vec::new());
        let init_symbol = mojito_symbol::symbol::method_symbol(
            "Point",
            "__init__",
            &mojito_symbol::symbol::SignatureKey::from_tys([&Ty::Int]),
        );
        let constructor_call = MirInstr::Call {
            dest: Reg(0),
            func: FuncRef("Point".to_string()),
            raises: None,
            args: vec![Reg(1)],
            kwargs: Vec::new(),
            arg_places: vec![None],
            kwarg_places: Vec::new(),
            capture_accesses: Vec::new(),
            param_arg_regs: Vec::new(),
        };
        let program = MirProgram {
            functions: vec![
                (
                    "main".to_string(),
                    test_function(Vec::new(), vec![constructor_call]),
                ),
                (
                    init_symbol.clone(),
                    test_function(vec![point.clone(), Ty::Int], Vec::new()),
                ),
                (
                    "Point.__deinit__".to_string(),
                    test_function(vec![point.clone()], Vec::new()),
                ),
                (
                    "Point.__copyinit__".to_string(),
                    test_function(vec![point.clone(), point.clone()], Vec::new()),
                ),
                (
                    "unrelated".to_string(),
                    test_function(Vec::new(), Vec::new()),
                ),
            ],
            declarations: MirDeclarations {
                structs: vec![MirStructDeclaration {
                    name: "Point".to_string(),
                    fields: vec![("x".to_string(), Ty::Int)],
                    mut_self_methods: Default::default(),
                    fieldwise_init: false,
                    param_decls: Vec::new(),
                    explicit_destroy_message: None,
                    explicit_destructors: Default::default(),
                }],
                functions: Vec::new(),
            },
            invariant_errors: Vec::new(),
        };

        let reachable = reachable_set(&program, &["main".to_string()])
            .expect("reachability over a well-formed program succeeds");
        assert!(reachable.contains("main"));
        assert!(
            reachable.contains(init_symbol.as_str()),
            "the unique arity overload of the constructor is a call edge"
        );
        assert!(
            reachable.contains("Point.__deinit__"),
            "struct types reachable functions mention pull their destructor"
        );
        assert!(
            reachable.contains("Point.__copyinit__"),
            "struct types reachable functions mention pull their copy constructor"
        );
        assert!(!reachable.contains("unrelated"));
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
            target: NativeTarget::host().expect("supported host"),
            trace_lifecycle: false,
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
