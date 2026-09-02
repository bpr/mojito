//! Register-VM backend and Mojito's sole runtime.
//!
//! The VM executes verified, drop-elaborated [`MirProgram`]s over per-call
//! register and variable frames. Language-level validation belongs to the
//! checker and ownership analysis; this module implements the checked call ABI,
//! places, structured control flow, exceptions, destruction, and runtime
//! primitives. See `docs/features.md` for the supported language surface.

use crate::ast::Stmt;
use crate::call::{ArgSlot, CallVariadics, match_call_slots};
use crate::checked::CheckedConst;
use crate::ct::CtValue;
use crate::error::RuntimeError;
use crate::hir::VarId;
use crate::mir::{
    Const, MirBlock, MirCaptureMode, MirInstr, MirIntrinsicSubscript, MirPlace, MirProgram,
    MirSubscriptArg, MirTerm, Proj, Reg,
};
use crate::runtime::{
    ClosureCapture, RefProjection, Value, apply_infix, apply_prefix, builtin_abs, builtin_convert,
    builtin_divmod, builtin_error, builtin_input, builtin_min_max, builtin_round, read_simd_lane,
    simd_from_values, value_as_index,
};
use crate::types::{CallableDefault, ParamDecl, Ty};
use std::collections::HashMap;

#[derive(Default)]
pub struct VmBackend {
    output: String,
    /// The final top-level (`__toplevel__`) variable values, by name — the global
    /// bindings, captured after execution for the CLI `run` dump and tests.
    bindings: Vec<(String, Value)>,
    /// Provenance-bearing allocations. Pointer copies retain an allocation id;
    /// freeing invalidates every alias and allocation bounds are never confused
    /// with adjacent allocations.
    heap: Vec<HeapAllocation>,
    /// Whether the program defines any `__copyinit__` / `__moveinit__`. When false,
    /// a value copy/move is the default (a raw deep `Clone` / a slot transfer) — the
    /// common fast path, keeping non-lifecycle programs unchanged. When true, a
    /// struct copy/move routes through its lifecycle method (`clone_value`/
    /// `move_value`), giving a pointer-owning type correct value semantics.
    has_copyinit: bool,
    has_moveinit: bool,
    /// Optional compile-time execution budget. Runtime VM execution leaves this
    /// `None`; VM-backed CTFE sets it and every function/block/instruction burns
    /// from it so compile-time execution cannot hang the compiler.
    ctfe_fuel: Option<usize>,
    frames: Vec<Frame>,
    next_frame_id: u64,
    /// Test-only ordered lifecycle-event log: destructor dispatches,
    /// consumes, raises, and catches, in execution order. `None` (the
    /// default) records nothing; the native backend's trace lane compares
    /// against this sequence.
    lifecycle_log: Option<Vec<String>>,
    /// Test-only `input()` source override. `None` (the default) reads process
    /// stdin unchanged; `Some` serves `input()` lines from the buffer and
    /// appends prompts to `output` so differential harnesses can feed the VM
    /// and a native executable identical bytes.
    input_override: Option<std::io::Cursor<Vec<u8>>>,
}

impl VmBackend {
    pub fn new() -> Self {
        Self::default()
    }

    /// Enable the test-only ordered lifecycle-event log.
    pub fn enable_lifecycle_log(&mut self) {
        self.lifecycle_log = Some(Vec::new());
    }

    /// The recorded lifecycle events, in execution order.
    pub fn lifecycle_log(&self) -> Option<&[String]> {
        self.lifecycle_log.as_deref()
    }

    /// Serve `input()` from `bytes` instead of process stdin (test-only).
    /// Prompts are appended to the captured output, matching a native
    /// executable that writes prompts to stdout.
    pub fn set_input_override(&mut self, bytes: Vec<u8>) {
        self.input_override = Some(std::io::Cursor::new(bytes));
    }

    pub(super) fn record_lifecycle(&mut self, event: String) {
        if let Some(log) = self.lifecycle_log.as_mut() {
            log.push(event);
        }
    }

    /// Execute a named top-level function and return its value without running the
    /// program's top-level block or `main`. This is the narrow API used by
    /// VM-backed CTFE: the caller has already checked that the function is
    /// compile-time safe and supplied any reified value parameters.
    pub fn run_function_value(
        &mut self,
        program: &[Stmt],
        name: &str,
        args: Vec<Value>,
        value_params: &[(String, Value)],
        fuel: usize,
    ) -> Result<(Value, usize), RuntimeError> {
        let checked = crate::checker::check_program(program).map_err(|error| {
            RuntimeError::TypeError(format!(
                "VM compile-time program failed the checked boundary: {error}"
            ))
        })?;
        let prog = build_prog_checked(&checked)?;
        self.configure_lifecycle(&prog);
        let idx = prog.index_of(name).ok_or_else(|| {
            RuntimeError::Unsupported(format!("vm: unknown compile-time function '{name}'"))
        })?;
        self.ctfe_fuel = Some(fuel);
        let result = self.call_function(&prog, idx, args, value_params);
        let remaining = self.ctfe_fuel.unwrap_or(0);
        self.ctfe_fuel = None;
        result.map(|value| (value, remaining))
    }

    fn configure_lifecycle(&mut self, prog: &Prog) {
        // A program with no lifecycle copy/move methods uses the default (raw clone /
        // slot transfer) path everywhere — so non-lifecycle programs are unchanged.
        self.has_copyinit = prog.defines(".__copyinit__");
        self.has_moveinit = prog.defines(".__moveinit__");
    }

    fn burn_ctfe(&mut self) -> Result<(), RuntimeError> {
        if let Some(fuel) = &mut self.ctfe_fuel {
            *fuel = fuel.checked_sub(1).ok_or_else(|| {
                RuntimeError::Unsupported(
                    "compile-time execution exceeded the VM CTFE fuel quota".to_string(),
                )
            })?;
        }
        Ok(())
    }

    /// Allocate `n` uninitialized slots in the heap arena, returning a
    /// pointer to the base. A negative/absurd count is a runtime error.
    fn heap_alloc(&mut self, n: i64, alignment: i64) -> Result<Value, RuntimeError> {
        if n < 0 {
            return Err(RuntimeError::TypeError(
                "vm: Pointer allocation count must be non-negative".to_string(),
            ));
        }
        if alignment <= 0 || !(alignment as u64).is_power_of_two() {
            return Err(RuntimeError::TypeError(
                "vm: Pointer allocation alignment must be a positive power of two".to_string(),
            ));
        }
        self.heap.push(HeapAllocation {
            slots: vec![Value::Moved; n as usize],
            alignment: alignment as usize,
            live: true,
        });
        Ok(Value::Pointer {
            allocation: self.heap.len() as u64,
            offset: 0,
        })
    }

    /// Resolve `base + offset` to an arena index, bounds-checking against the arena
    /// (a truly out-of-arena access errors rather than panicking; an in-arena but
    /// past-allocation access is permitted — `UnsafePointer` is unchecked).
    fn heap_index(
        &self,
        allocation: u64,
        base: i64,
        offset: i64,
    ) -> Result<(usize, usize), RuntimeError> {
        if allocation == 0 {
            return Err(RuntimeError::TypeError(
                "vm: dereference of dangling Pointer".to_string(),
            ));
        }
        let allocation_index = usize::try_from(allocation - 1)
            .map_err(|_| RuntimeError::TypeError("vm: invalid Pointer provenance".to_string()))?;
        let region = self
            .heap
            .get(allocation_index)
            .ok_or_else(|| RuntimeError::TypeError("vm: invalid Pointer provenance".to_string()))?;
        if !region.live {
            return Err(RuntimeError::TypeError(
                "vm: use after Pointer deallocation".to_string(),
            ));
        }
        let i = base
            .checked_add(offset)
            .ok_or_else(|| RuntimeError::TypeError("vm: Pointer offset overflow".to_string()))?;
        if i < 0 || i as usize >= region.slots.len() {
            return Err(RuntimeError::TypeError(
                "vm: Pointer access out of bounds".to_string(),
            ));
        }
        Ok((allocation_index, i as usize))
    }

    fn heap_free(&mut self, allocation: u64, offset: i64) -> Result<(), RuntimeError> {
        if allocation == 0 || offset != 0 {
            return Err(RuntimeError::TypeError(
                "vm: free requires a live allocation-base pointer".to_string(),
            ));
        }
        let region = self
            .heap
            .get_mut((allocation - 1) as usize)
            .ok_or_else(|| RuntimeError::TypeError("vm: invalid Pointer provenance".to_string()))?;
        if !region.live {
            return Err(RuntimeError::TypeError(
                "vm: double free of Pointer allocation".to_string(),
            ));
        }
        region.live = false;
        region.slots.clear();
        Ok(())
    }

    /// Read one initialized heap slot. `Moved` is the VM's raw-storage
    /// tombstone: allocation starts uninitialized and take/destroy restore that
    /// state until an explicit pointer store initializes the slot again.
    fn heap_read(&self, allocation: u64, base: i64, offset: i64) -> Result<Value, RuntimeError> {
        let (region, slot) = self.heap_index(allocation, base, offset)?;
        match &self.heap[region].slots[slot] {
            Value::Moved => Err(RuntimeError::TypeError(
                "vm: read of uninitialized Pointer storage".to_string(),
            )),
            value => Ok(value.clone()),
        }
    }

    /// Move one initialized raw-storage value out, leaving an uninitialized
    /// tombstone. This intentionally bypasses `__moveinit__`: ownership of the
    /// existing value is transferred rather than constructing another value.
    fn heap_take(
        &mut self,
        allocation: u64,
        base: i64,
        offset: i64,
    ) -> Result<Value, RuntimeError> {
        let (region, slot) = self.heap_index(allocation, base, offset)?;
        let value = std::mem::replace(&mut self.heap[region].slots[slot], Value::Moved);
        if matches!(value, Value::Moved) {
            Err(RuntimeError::TypeError(
                "vm: take or destroy of uninitialized Pointer storage".to_string(),
            ))
        } else {
            Ok(value)
        }
    }

    fn heap_destroy(
        &mut self,
        prog: &Prog,
        allocation: u64,
        base: i64,
        offset: i64,
    ) -> Result<(), RuntimeError> {
        let value = self.heap_take(allocation, base, offset)?;
        self.drop_value(prog, value)
    }

    /// Move the payload out of a consumed inline uninit-storage value
    /// (`MaybeUninit`'s field). Upstream leaves this undefined behavior;
    /// the VM traps deterministically, mirroring the heap arena's tombstones.
    pub(super) fn uninit_storage_payload(
        storage: Value,
        operation: &str,
    ) -> Result<Value, RuntimeError> {
        match storage {
            Value::UninitStorage(Some(payload)) => Ok(*payload),
            Value::UninitStorage(None) => Err(RuntimeError::TypeError(format!(
                "vm: {operation} of uninitialized MaybeUninit storage"
            ))),
            other => Err(RuntimeError::TypeError(format!(
                "vm: {operation} requires inline uninit storage, found {}",
                crate::runtime::type_name(&other)
            ))),
        }
    }

    /// Execute a function for its return value only. `value_params` reifies a
    /// value-parameterized generic function's comptime arguments (empty otherwise).
    fn call_function(
        &mut self,
        prog: &Prog,
        fidx: usize,
        args: Vec<Value>,
        value_params: &[(String, Value)],
    ) -> Result<Value, RuntimeError> {
        Ok(self.call_frame(prog, fidx, args, value_params)?.0)
    }

    /// Call a struct dunder `Type.method(args…)` (`args[0]` is the receiver). The
    /// checker has already verified the method exists and its argument types, so a
    /// missing method here is a compiler bug (reported cleanly rather than a panic).
    fn call_dunder(
        &mut self,
        prog: &Prog,
        sname: &str,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        self.call_resolved_dunder(prog, sname, method, args, None)
    }

    fn call_resolved_dunder(
        &mut self,
        prog: &Prog,
        sname: &str,
        method: &str,
        args: Vec<Value>,
        resolved: Option<&str>,
    ) -> Result<Value, RuntimeError> {
        let source_fname = format!("{sname}.{method}");
        let fname = resolved
            .map(str::to_string)
            .unwrap_or_else(|| prog.overload_name(&source_fname, args.len().saturating_sub(1)));
        let idx = prog.index_of(&fname).ok_or_else(|| {
            RuntimeError::Unsupported(format!("vm: struct '{sname}' has no method '{method}'"))
        })?;
        self.call_function(prog, idx, args, &[])
    }
}

impl VmBackend {
    /// Run a checked program, entering through `main()` when present. This
    /// executable entry enforces the same pre-drop ownership contract the
    /// production `Compiler` pipeline does, so a stage-composed caller cannot
    /// execute a program the analysis rejects. (The VM-CTFE entry
    /// `run_function_value` deliberately keeps the lighter checked boundary.)
    pub fn run(&mut self, program: &crate::checked::CheckedProgram) -> Result<(), RuntimeError> {
        let lowered = crate::mir::lower_checked_program(program);
        if !lowered.invariant_errors.is_empty() {
            return Err(RuntimeError::Unsupported(format!(
                "invalid checked program: {}",
                lowered.invariant_errors.join("; ")
            )));
        }
        crate::analysis::check_ownership_program(&lowered)
            .map_err(|error| RuntimeError::Unsupported(format!("ownership error: {error}")))?;
        self.run_prog(build_prog_lowered(lowered)?)
    }

    /// Run a verified, already drop-elaborated MIR program — what
    /// `mir::text::load_artifact` yields. The loading gate is the artifact's
    /// semantic gate, so this entry re-runs neither `mir::verify` nor the
    /// pre-drop ownership analysis (meaningless on elaborated MIR), and it
    /// must not re-run drop elaboration: `elaborate_drops_program` is not
    /// idempotent, and the artifact's `drop.var`/cleanup schedule is already
    /// final.
    pub fn run_elaborated(&mut self, mir: MirProgram) -> Result<(), RuntimeError> {
        if !mir.invariant_errors.is_empty() {
            return Err(RuntimeError::Unsupported(format!(
                "invalid MIR program: {}",
                mir.invariant_errors.join("; ")
            )));
        }
        let structs = build_structs(&mir.declarations);
        let sigs = build_sigs(&mir.declarations);
        self.run_prog(Prog { mir, structs, sigs })
    }

    /// Captured standard output.
    pub fn output(&self) -> String {
        self.output.clone()
    }

    /// Final top-level bindings, for the CLI `run` dump.
    pub fn bindings(&self) -> Vec<(String, Value)> {
        self.bindings.clone()
    }
}

impl VmBackend {
    fn run_prog(&mut self, prog: Prog) -> Result<(), RuntimeError> {
        self.configure_lifecycle(&prog);
        // Run module initialization, then `main()`. Capture the top-level frame's
        // user variables (skipping synthetic `$…` temporaries) as the global
        // bindings.
        if let Some(top) = prog.index_of("__toplevel__") {
            let (_, vars) = self.call_frame(&prog, top, Vec::new(), &[])?;
            let names = &prog.mir.functions[top].1.var_names;
            self.bindings = names
                .iter()
                .zip(&vars)
                .filter(|(name, _)| !name.starts_with('$'))
                .map(|(name, v)| (name.clone(), v.clone()))
                .collect();
        }
        if let Some(main) = prog.index_of("main") {
            self.call_function(&prog, main, Vec::new(), &[])?;
        }
        Ok(())
    }
}

/// The whole program the VM executes: the lowered MIR plus the struct and
/// function-signature registries. Immutable during execution, so it threads as
/// `&Prog` beside the mutable output.
struct Prog {
    mir: MirProgram,
    structs: HashMap<String, StructDef>,
    sigs: HashMap<String, FnSig>,
}

impl Prog {
    fn index_of(&self, name: &str) -> Option<usize> {
        self.mir.functions.iter().position(|(n, _)| n == name)
    }

    /// Whether any function name ends with `suffix` (e.g. `.__copyinit__`) — used to
    /// decide whether copy/move needs the lifecycle-method path at all.
    fn defines(&self, suffix: &str) -> bool {
        self.mir.functions.iter().any(|(n, _)| n.ends_with(suffix))
    }

    /// Arity-based overload fallback: resolve a *source* name to the lowered
    /// function it must mean, for the calls the checker records no per-span
    /// target for. Its callers are the VM-synthesized dispatches — operator/
    /// `__str__`/`__hash__` dunders (`call_dunder`), `__setitem__`,
    /// the `for`-loop `__next__` protocol, `__init__` construction reached
    /// without a recorded target, and `runtime_method_name` when `resolved` is
    /// absent or its abstract callable-contract suffix retargets to a plain,
    /// non-overloaded nominal `__call__`. Checker-resolved concrete overloads
    /// carry their exact lowered callee and never depend on this fallback.
    fn overload_name(&self, name: &str, argc: usize) -> String {
        crate::symbol::resolve_callable_symbol(
            self.mir
                .functions
                .iter()
                .map(|(name, function)| crate::symbol::CallableCandidate {
                    name,
                    n_params: function.n_params,
                }),
            name,
            argc,
        )
    }

    /// Resolve a selected method signature against the receiver's concrete
    /// runtime type. Bounded generic calls carry an abstract checker symbol;
    /// retargeting its suffix preserves overload selection even when every
    /// overload has the same positional arity (for example `**kwargs` methods).
    fn runtime_method_name(
        &self,
        receiver_type: &str,
        method: &str,
        resolved: Option<&str>,
        argc: usize,
    ) -> String {
        crate::symbol::resolve_method_symbol(
            self.mir
                .functions
                .iter()
                .map(|(name, function)| crate::symbol::CallableCandidate {
                    name,
                    n_params: function.n_params,
                }),
            receiver_type,
            method,
            resolved,
            argc,
        )
    }
}

fn vm_type_is_symbolic(ty: &Ty) -> bool {
    match ty {
        Ty::Infer | Ty::Param { .. } | Ty::Assoc { .. } | Ty::Dependent(_) | Ty::SelfType => true,
        Ty::Struct(_, arguments) => arguments.iter().any(|argument| match argument {
            crate::types::TyArg::Ty(ty) => vm_type_is_symbolic(ty),
            crate::types::TyArg::Val(value) => vm_ct_value_is_symbolic(value),
            // Origins erase from the runtime ABI, so they never make a type symbolic.
            crate::types::TyArg::Origin(_) => false,
        }),
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
            params.iter().any(vm_type_is_symbolic)
                || vm_type_is_symbolic(ret)
                || variadic.as_deref().is_some_and(vm_type_is_symbolic)
                || kw_variadic.as_deref().is_some_and(vm_type_is_symbolic)
                || error.as_deref().is_some_and(vm_type_is_symbolic)
        }
        Ty::Overload(types) | Ty::Tuple(types) | Ty::RuntimePack(types) | Ty::Variant(types) => {
            types.iter().any(vm_type_is_symbolic)
        }
        Ty::ComptimeList(element) | Ty::VariadicPack(element) | Ty::Pointer { element, .. } => {
            vm_type_is_symbolic(element)
        }
        Ty::Ref(reference) => vm_type_is_symbolic(&reference.referent),
        Ty::Dtype
        | Ty::Int
        | Ty::UInt
        | Ty::Bool
        | Ty::StringLiteral
        | Ty::Float64
        | Ty::None
        | Ty::Never
        | Ty::IntLiteral
        | Ty::FloatLiteral
        | Ty::Simd { .. }
        | Ty::Error => false,
    }
}

struct CallerFrame<'a> {
    id: FrameId,
    registers: &'a mut [Value],
    variables: &'a mut Vec<Value>,
}

/// Executing-frame storage that must remain reachable while adapting an
/// abstract call result. A concrete reference result may point into either the
/// caller or the just-completed iterator frame while its lifecycle copy runs.
struct ResultAdapterFrames<'a> {
    current: FrameId,
    current_variables: &'a mut Vec<Value>,
    returned: Option<(FrameId, &'a mut Vec<Value>)>,
}

/// Take the single argument of a one-arg built-in (the checker guarantees arity;
/// a mismatch is a defensive clean error, never a panic).
fn arg1(name: &str, args: Vec<Value>) -> Result<Value, RuntimeError> {
    let mut args = args;
    if args.len() != 1 {
        return Err(RuntimeError::ArityMismatch {
            name: name.to_string(),
            expected: 1,
            got: args.len(),
        });
    }
    Ok(args.pop().expect("arity checked above"))
}

/// A free function's calling signature (the MIR doesn't keep it), for matching
/// positional + keyword arguments to parameter slots — filling defaults and
/// collecting a trailing `*args`. Covers only the *regular* parameters;
/// `variadic` is either the homogeneous element type or an explicit
/// `Ty::RuntimePack` sequence for a specialized heterogeneous collector.
struct FnSig {
    param_names: Vec<String>,
    param_types: Vec<Ty>,
    /// Declared default per regular parameter (`None` = no default, or a
    /// non-constant default the VM can't fold — using such a slot errors). A
    /// `CheckedConst::Construct` default is materialized at bind time by running
    /// its converting constructor (see `bind_for_call`); scalars fold directly.
    defaults: Vec<Option<CheckedConst>>,
    required: Vec<bool>,
    variadic: Option<Ty>,
    /// Where the collected `*args` list belongs among source parameters. For a
    /// signature like `def f(a, *xs, b)`, this is `Some(1)`.
    variadic_index: Option<usize>,
    kw_variadic: Option<Ty>,
    kw_variadic_index: Option<usize>,
    /// Indexes into the regular-parameter list.
    positional_only: Option<usize>,
    keyword_only: Option<usize>,
    /// Checker-resolved compile-time parameters. Value parameters become typed
    /// frame locals; type parameters remain erased.
    param_decls: Vec<ParamDecl>,
}

/// Reify generic value parameters in declaration order. Missing source
/// arguments are filled from checked scalar/callable defaults; callable aliases
/// can therefore reuse an earlier runtime closure without ever converting its
/// capture payload into `CtValue`.
fn reify_value_parameters(
    declarations: &[ParamDecl],
    supplied: &[Option<Value>],
) -> Vec<(String, Value)> {
    let resolved = resolve_value_parameter_slots(declarations, supplied);
    declarations
        .iter()
        .enumerate()
        .filter_map(|(index, declaration)| {
            let ParamDecl::Value { name, ty, .. } = declaration else {
                // A constructible type parameter is reified as the bound
                // struct's name (supplied argument, else the declared default).
                let ParamDecl::Type { name, default, .. } = declaration else {
                    return None;
                };
                if !constructible_type_parameter(declaration) {
                    return None;
                }
                let value = match resolved.get(index).cloned().flatten() {
                    Some(value @ Value::Str(_)) => value,
                    _ => match default.as_deref() {
                        Some(Ty::Struct(struct_name, _)) => Value::Str(struct_name.clone()),
                        _ => return None,
                    },
                };
                return Some((name.clone(), value));
            };
            let value = resolved
                .get(index)
                .cloned()
                .flatten()
                .unwrap_or(Value::None);
            Some((
                name.clone(),
                crate::runtime::coerce_checked(value, ty.as_ref()),
            ))
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct FrameId(u64);

struct SynchronousCall<'a> {
    function_index: usize,
    arguments: Vec<Value>,
    value_params: &'a [(String, Value)],
    reference_inputs: &'a [(usize, Value)],
}

/// A struct type's runtime shape, gathered from the program AST (the MIR doesn't
/// keep field layout): field names + types (for constructor coercion), and which
/// methods take `mut self` (so their receiver is written back).
struct StructDef {
    fields: Vec<(String, Ty)>,
    mut_self_methods: std::collections::HashSet<String>,
    fieldwise_init: bool,
    /// Checker-resolved compile-time parameters. Type parameters are erased;
    /// value parameters are materialized to their declared type on reification.
    param_decls: Vec<ParamDecl>,
}

fn runtime_value_as_ct(value: &Value) -> Option<CtValue> {
    Some(match value {
        Value::Int(value) => CtValue::Int(*value),
        Value::UInt(value) => CtValue::UInt(*value),
        Value::Float64(value) => CtValue::Float(value.to_bits()),
        Value::IntLiteral(value) => CtValue::IntLiteral(value.clone()),
        Value::FloatLiteral(value) => CtValue::FloatLiteral(value.clone()),
        Value::Bool(value) => CtValue::Bool(*value),
        Value::Str(value) => CtValue::Str(value.clone()),
        Value::Tuple(values) => CtValue::Tuple(
            values
                .iter()
                .map(runtime_value_as_ct)
                .collect::<Option<Vec<_>>>()?,
        ),
        Value::ComptimeList(values) => CtValue::List(
            values
                .iter()
                .map(runtime_value_as_ct)
                .collect::<Option<Vec<_>>>()?,
        ),
        _ => return None,
    })
}

fn ct_value_as_runtime(value: CtValue) -> Option<Value> {
    Some(match value {
        CtValue::Int(value) => Value::Int(value),
        CtValue::UInt(value) => Value::UInt(value),
        CtValue::Float(bits) => Value::Float64(f64::from_bits(bits)),
        CtValue::IntLiteral(value) => Value::IntLiteral(value),
        CtValue::FloatLiteral(value) => Value::FloatLiteral(value),
        CtValue::Bool(value) => Value::Bool(value),
        CtValue::Str(value) => Value::Str(value),
        CtValue::Tuple(values) => Value::Tuple(
            values
                .into_iter()
                .map(ct_value_as_runtime)
                .collect::<Option<Vec<_>>>()?,
        ),
        CtValue::List(values) => Value::ComptimeList(
            values
                .into_iter()
                .map(ct_value_as_runtime)
                .collect::<Option<Vec<_>>>()?,
        ),
        CtValue::Dtype(_)
        | CtValue::Struct { .. }
        | CtValue::Type(_)
        | CtValue::Reflected(_)
        | CtValue::Param(_) => return None,
    })
}

fn resolve_callable_default(
    default: &CallableDefault,
    runtime: &HashMap<String, Value>,
    comptime: &HashMap<String, CtValue>,
) -> Option<Value> {
    match default {
        CallableDefault::Symbol(symbol) => Some(Value::Function(symbol.clone())),
        CallableDefault::Parameter(name) => runtime.get(name).cloned(),
        CallableDefault::If {
            condition,
            then_value,
            else_value,
        } => match condition.evaluate(comptime)? {
            CtValue::Bool(true) => resolve_callable_default(then_value, runtime, comptime),
            CtValue::Bool(false) => resolve_callable_default(else_value, runtime, comptime),
            _ => None,
        },
    }
}

/// Take the two arguments of a two-arg built-in (`min`/`max`).
fn arg2(name: &str, args: Vec<Value>) -> Result<(Value, Value), RuntimeError> {
    if args.len() != 2 {
        return Err(RuntimeError::ArityMismatch {
            name: name.to_string(),
            expected: 2,
            got: args.len(),
        });
    }
    let mut it = args.into_iter();
    Ok((
        it.next().expect("arity checked above"),
        it.next().expect("arity checked above"),
    ))
}

/// Resolve every supplied or defaulted value in declaration order. This is
/// separate from frame-local naming so an indirect call can resolve the
/// anonymous contract's defaults, then reify those concrete values under the
/// implementation's (alpha-equivalent) declaration names.
/// Whether a type parameter must be reified at runtime: its bound admits
/// default construction (`H()`), which an erased body performs by name.
pub(crate) fn constructible_type_parameter(declaration: &ParamDecl) -> bool {
    crate::types::constructible_type_parameter(declaration)
}

fn resolve_value_parameter_slots(
    declarations: &[ParamDecl],
    supplied: &[Option<Value>],
) -> Vec<Option<Value>> {
    let mut resolved = vec![None; declarations.len()];
    let mut runtime = HashMap::new();
    let mut comptime = HashMap::new();
    for (index, declaration) in declarations.iter().enumerate() {
        let ParamDecl::Value {
            name,
            ty,
            default,
            callable_default,
            ..
        } = declaration
        else {
            // A reified type argument passes through as the bound struct name.
            if constructible_type_parameter(declaration) {
                resolved[index] = supplied.get(index).cloned().flatten();
            }
            continue;
        };
        let value = supplied
            .get(index)
            .cloned()
            .flatten()
            .or_else(|| {
                callable_default
                    .as_ref()
                    .and_then(|default| resolve_callable_default(default, &runtime, &comptime))
            })
            .or_else(|| {
                default.as_ref().and_then(|default| {
                    default
                        .evaluate(&comptime)
                        .and_then(|value| value.materialize_as(ty))
                        .and_then(ct_value_as_runtime)
                })
            })
            .map(|value| crate::runtime::coerce_checked(value, ty.as_ref()));
        let Some(value) = value else {
            continue;
        };
        runtime.insert(name.clone(), value.clone());
        if let Some(value) = runtime_value_as_ct(&value) {
            comptime.insert(name.clone(), value);
        }
        resolved[index] = Some(value);
    }
    resolved
}

fn vm_ct_value_is_symbolic(value: &CtValue) -> bool {
    match value {
        CtValue::Param(_) => true,
        CtValue::Tuple(values) | CtValue::List(values) => {
            values.iter().any(vm_ct_value_is_symbolic)
        }
        CtValue::Type(ty) | CtValue::Reflected(ty) => vm_type_is_symbolic(ty),
        CtValue::Struct { fields, .. } => fields
            .iter()
            .any(|(_, value)| vm_ct_value_is_symbolic(value)),
        CtValue::Int(_)
        | CtValue::UInt(_)
        | CtValue::Float(_)
        | CtValue::IntLiteral(_)
        | CtValue::FloatLiteral(_)
        | CtValue::Bool(_)
        | CtValue::Dtype(_)
        | CtValue::Str(_) => false,
    }
}

struct Frame {
    id: FrameId,
    function: usize,
    registers: Vec<Value>,
    variables: Vec<Value>,
    block: usize,
    instruction: usize,
    continuation: Option<ReturnContinuation>,
}

struct WritebackCall<'a> {
    function_name: &'a str,
    function_index: usize,
    positional_args: Vec<Value>,
    keyword_args: Vec<(String, Value)>,
    argument_places: &'a [Option<MirPlace>],
    keyword_argument_places: &'a [Option<MirPlace>],
    value_params: Vec<(String, Value)>,
}

struct MethodInvocation<'a> {
    receiver: Value,
    method: &'a str,
    resolved_name: Option<&'a str>,
    result_adapter: Option<crate::checked::CheckedResultAdapter>,
    arguments: Vec<Value>,
    keyword_arguments: Vec<(String, Value)>,
    receiver_place: &'a Option<MirPlace>,
    argument_places: &'a [Option<MirPlace>],
    keyword_argument_places: &'a [Option<MirPlace>],
    parameter_arguments: &'a [crate::mir::MirParamArg],
    parameter_declarations: &'a [crate::types::ParamDecl],
}

/// Recover the retained caller place selected for one bound parameter. Keyword
/// slots are deliberately matched by parameter name: `bind_for_call` expands
/// `**kwargs^`, so its internal keyword index is not necessarily an index into
/// the original MIR keyword vectors. A forwarded entry has no retained source
/// place and therefore correctly returns `None` here.
fn bound_argument_place<'a>(
    slot: Option<&ArgSlot>,
    parameter_name: Option<&str>,
    positional_offset: usize,
    argument_places: &'a [Option<MirPlace>],
    keyword_names: &[String],
    keyword_argument_places: &'a [Option<MirPlace>],
) -> Option<&'a MirPlace> {
    match slot? {
        ArgSlot::Positional(argument) => argument
            .checked_sub(positional_offset)
            .and_then(|argument| argument_places.get(argument))
            .and_then(Option::as_ref),
        ArgSlot::Keyword(_) => parameter_name
            .and_then(|name| keyword_names.iter().position(|candidate| candidate == name))
            .and_then(|argument| keyword_argument_places.get(argument))
            .and_then(Option::as_ref),
        ArgSlot::Default => None,
    }
}

#[derive(Default)]
struct HeapAllocation {
    slots: Vec<Value>,
    #[allow(dead_code)]
    alignment: usize,
    live: bool,
}

fn build_prog_checked(checked: &crate::checked::CheckedProgram) -> Result<Prog, RuntimeError> {
    build_prog_lowered(crate::mir::lower_checked_program(checked))
}

fn build_prog_lowered(lowered: crate::mir::MirProgram) -> Result<Prog, RuntimeError> {
    let mut mir = crate::analysis::elaborate_drops_program(lowered);
    // The VM executes the drop-elaborated program, so it is re-verified after
    // the DropVar/edge-cleanup rewrite — the elaborated MIR must satisfy the
    // same contract the pre-elaboration program did.
    mir.invariant_errors
        .extend(crate::mir::verify::verify(&mir));
    if !mir.invariant_errors.is_empty() {
        return Err(RuntimeError::Unsupported(format!(
            "invalid checked program: {}",
            mir.invariant_errors.join("; ")
        )));
    }
    let structs = build_structs(&mir.declarations);
    let sigs = build_sigs(&mir.declarations);
    Ok(Prog {
        // Elaborate ASAP drops: splice a `DropVar` after each variable's last
        // use, so a struct's `__deinit__` runs there (Stage 7). A no-op for values
        // without a destructor.
        mir,
        structs,
        sigs,
    })
}

/// Bind source-ordered compile-time arguments to their checked declarations.
/// Keyword arguments may skip defaults or appear out of declaration order, and
/// an erased type argument still occupies its selected declaration slot.
fn align_parameter_arguments<T>(
    declarations: &[ParamDecl],
    arguments: Vec<(Option<String>, Option<T>)>,
) -> Vec<Option<T>> {
    let mut aligned: Vec<Option<T>> = (0..declarations.len()).map(|_| None).collect();
    let mut next_positional = 0;
    for (name, value) in arguments {
        let index = match name {
            Some(name) => declarations
                .iter()
                .position(|declaration| declaration.name().trim_start_matches('*') == name),
            None => {
                while declarations
                    .get(next_positional)
                    .is_some_and(|declaration| match declaration {
                        ParamDecl::Type { infer_only, .. }
                        | ParamDecl::Value { infer_only, .. } => *infer_only,
                    })
                {
                    next_positional += 1;
                }
                let index = (next_positional < declarations.len()).then_some(next_positional);
                next_positional += usize::from(index.is_some());
                index
            }
        };
        if let Some(index) = index {
            aligned[index] = value;
        }
    }
    aligned
}

fn runtime_parameter_arguments(
    declarations: &[ParamDecl],
    arguments: &[crate::mir::MirParamArg],
    registers: &[Value],
) -> Vec<Option<Value>> {
    align_parameter_arguments(
        declarations,
        arguments
            .iter()
            .map(|argument| {
                (
                    argument.name.clone(),
                    argument
                        .value
                        .map(|register| registers[register.0 as usize].clone()),
                )
            })
            .collect(),
    )
}

struct ReturnContinuation {
    dest: Reg,
    writebacks: Vec<(usize, MirPlace)>,
}

/// Build the VM registry from declaration metadata carried by MIR.
fn build_structs(declarations: &crate::mir::MirDeclarations) -> HashMap<String, StructDef> {
    declarations
        .structs
        .iter()
        .map(|declaration| {
            (
                declaration.name.clone(),
                StructDef {
                    fields: declaration.fields.clone(),
                    mut_self_methods: declaration.mut_self_methods.clone(),
                    fieldwise_init: declaration.fieldwise_init,
                    param_decls: declaration.param_decls.clone(),
                },
            )
        })
        .collect()
}

/// Build the VM calling registry from declaration metadata carried by MIR.
fn build_sigs(declarations: &crate::mir::MirDeclarations) -> HashMap<String, FnSig> {
    declarations
        .functions
        .iter()
        .map(|declaration| {
            (
                declaration.lowered_name.clone(),
                FnSig {
                    param_names: declaration.param_names.clone(),
                    param_types: declaration.param_types.clone(),
                    defaults: declaration.defaults.clone(),
                    required: declaration.required.clone(),
                    variadic: declaration.variadic.clone(),
                    variadic_index: declaration.variadic_index,
                    kw_variadic: declaration.kw_variadic.clone(),
                    kw_variadic_index: declaration.kw_variadic_index,
                    positional_only: declaration.positional_only,
                    keyword_only: declaration.keyword_only,
                    param_decls: declaration.param_decls.clone(),
                },
            )
        })
        .collect()
}

fn apply_format_spec(value: &Value, rendered: &str, spec: &str) -> Result<String, RuntimeError> {
    if spec.is_empty() {
        return Ok(rendered.to_string());
    }
    if let Some(precision) = spec
        .strip_prefix('.')
        .and_then(|tail| tail.strip_suffix('f'))
        .and_then(|digits| digits.parse::<usize>().ok())
        && let Value::Float64(number) = value
    {
        return Ok(format!("{number:.precision$}"));
    }
    let (alignment, width_text) = match spec.chars().next() {
        Some(character @ ('<' | '>' | '^')) => (character, &spec[1..]),
        _ => ('>', spec),
    };
    let width = width_text.parse::<usize>().map_err(|_| {
        RuntimeError::TypeError(format!("unsupported format specification '{spec}'"))
    })?;
    if rendered.len() >= width {
        return Ok(rendered.to_string());
    }
    let padding = width - rendered.len();
    let (left, right) = match alignment {
        '<' => (0, padding),
        '^' => (padding / 2, padding - padding / 2),
        _ => (padding, 0),
    };
    Ok(format!(
        "{}{}{}",
        " ".repeat(left),
        rendered,
        " ".repeat(right)
    ))
}

fn navigate_reference_mut<'a>(
    mut value: &'a mut Value,
    projection: &[RefProjection],
) -> Result<&'a mut Value, RuntimeError> {
    for segment in projection {
        value = match segment {
            RefProjection::Field(name) => match value {
                Value::Struct { fields, .. } => fields
                    .iter_mut()
                    .find(|(field, _)| field == name)
                    .map(|(_, value)| value)
                    .ok_or_else(|| RuntimeError::TypeError(format!("no field '{name}'")))?,
                _ => {
                    return Err(RuntimeError::TypeError(
                        "invalid reference field".to_string(),
                    ));
                }
            },
            RefProjection::Index(index) => match value {
                // Public Tuple's checked dependent accessor returns a handle
                // into its compiler-private runtime-pack field. Cross-frame
                // writes (the accessor frame returning to its caller) must be
                // able to follow that typed pack projection just as local
                // `write_reference_projection` already does.
                Value::Tuple(items) => items.get_mut(*index).ok_or_else(|| {
                    RuntimeError::TypeError("reference index out of bounds".to_string())
                })?,
                // Offset-0 identity deref of an origin-erased single-pointee
                // `to=place` pointer written through a mutable origin: the handle
                // was re-rooted at the pointee itself, so `Index(0)` targets that
                // value in place (see `read_reference_projection`).
                _ if *index == 0 => value,
                _ => {
                    return Err(RuntimeError::TypeError(
                        "mutable index reference did not cross a nominal collection's pointer \
                         storage or private runtime pack"
                            .to_string(),
                    ));
                }
            },
            // The single-pointee dereference: the handle reached here already
            // designates the pointee.
            RefProjection::Deref => value,
            RefProjection::Variant(expected) => match value {
                Value::Variant {
                    index,
                    value,
                    alternatives,
                } if index == expected => value.as_mut(),
                Value::Variant {
                    index,
                    alternatives,
                    ..
                } => {
                    return Err(RuntimeError::TypeError(format!(
                        "Variant holds '{}', not '{}'",
                        alternatives
                            .get(*index)
                            .map(ToString::to_string)
                            .unwrap_or_else(|| "<invalid>".to_string()),
                        alternatives
                            .get(*expected)
                            .map(ToString::to_string)
                            .unwrap_or_else(|| "<invalid>".to_string())
                    )));
                }
                _ => {
                    return Err(RuntimeError::TypeError(
                        "invalid mutable Variant reference projection".to_string(),
                    ));
                }
            },
            RefProjection::Capture(index) => match value {
                Value::Closure { captures, .. } => captures
                    .get_mut(*index)
                    .map(|capture| &mut capture.value)
                    .ok_or_else(|| {
                        RuntimeError::TypeError("closure capture index out of bounds".to_string())
                    })?,
                _ => {
                    return Err(RuntimeError::TypeError(
                        "invalid closure capture projection".to_string(),
                    ));
                }
            },
            // References into inline uninit storage come only from
            // `unsafe_assume_init(ref self)`, which asserts initialization —
            // an uninitialized payload traps rather than lazily initializing.
            RefProjection::UninitPayload => match value {
                Value::UninitStorage(Some(payload)) => payload.as_mut(),
                Value::UninitStorage(None) => {
                    return Err(RuntimeError::TypeError(
                        "vm: read of uninitialized MaybeUninit storage".to_string(),
                    ));
                }
                _ => {
                    return Err(RuntimeError::TypeError(
                        "payload projection on a non-uninit-storage value".to_string(),
                    ));
                }
            },
        };
    }
    Ok(value)
}

/// The control-flow outcome of executing an instruction or a `try` sub-region.
/// Most execution is `Normal`; a `return` that crosses a `try` boundary surfaces
/// as `Return`, so `finally` can run before control leaves the function. (A
/// `raise` propagates separately as `RuntimeError::Raised`; `break`/`continue`
/// crossing a `try` are refused at lowering — the mini-CFG region can't name the
/// outer loop's target block.)
enum Flow {
    Normal,
    Return {
        value: Value,
        cleanup: Vec<VarId>,
    },
    /// A `break`/`continue` that crossed a `try` boundary, already resolved to the
    /// target loop block in the enclosing **function** CFG. Propagates out of the
    /// `try` (running each `finally`) until the function driver jumps there.
    Jump(usize),
}

struct TryRegions<'a> {
    body: &'a [MirBlock],
    handler: &'a Option<(Option<VarId>, Vec<MirBlock>)>,
    orelse: &'a Option<Vec<MirBlock>>,
    finalbody: &'a Option<Vec<MirBlock>>,
    cleanup: &'a [VarId],
}

struct ReferencePointerBoundary<'a> {
    allocation: u64,
    offset: i64,
    index: usize,
    suffix: &'a [RefProjection],
}

/// Read a struct field (or a reified value parameter, e.g. `Self.n`) by name.
fn get_field(base: &Value, field: &str) -> Result<Value, RuntimeError> {
    match base {
        Value::Struct {
            fields,
            value_params,
            ..
        } => fields
            .iter()
            .chain(value_params.iter())
            .find(|(f, _)| f == field)
            .map(|(_, v)| v.clone())
            .ok_or_else(|| RuntimeError::TypeError(format!("no field '{field}'"))),
        other => Err(RuntimeError::TypeError(format!(
            "field access on non-struct {}",
            crate::runtime::type_name(other)
        ))),
    }
}

/// Store through a flattened reference whose final projection selects a SIMD
/// lane. SIMD lanes are packed scalars rather than independent `Value` slots,
/// so the ordinary mutable projection navigator cannot return one by address.
fn write_simd_reference_lane(
    root: &mut Value,
    projection: &[RefProjection],
    value: Value,
) -> Result<bool, RuntimeError> {
    let Some((RefProjection::Index(index), prefix)) = projection.split_last() else {
        return Ok(false);
    };
    let parent = navigate_reference_mut(root, prefix)?;
    let Value::Simd { dtype, lanes } = parent else {
        return Ok(false);
    };
    crate::runtime::set_simd_lane(*dtype, lanes, *index as i64, value)?;
    Ok(true)
}

/// Store through a flattened reference whose final projection is the payload
/// of inline uninit storage: the write initializes-or-overwrites the payload
/// raw — no destructor runs and no initialization is required (`unsafe_write`
/// leaks a previous payload by design). Interior (non-final) payload steps
/// keep the ordinary navigator's initialized-payload requirement.
fn write_uninit_payload(
    root: &mut Value,
    projection: &[RefProjection],
    value: Value,
) -> Result<bool, RuntimeError> {
    let Some((RefProjection::UninitPayload, prefix)) = projection.split_last() else {
        return Ok(false);
    };
    let parent = navigate_reference_mut(root, prefix)?;
    let Value::UninitStorage(payload) = parent else {
        return Ok(false);
    };
    *payload = Some(Box::new(value));
    Ok(true)
}

/// Index internal tuple-pack storage or a SIMD value. Nominal collections route
/// through their checked `__getitem__` implementation before reaching here.
fn index_value(base: &Value, idx: i64) -> Result<Value, RuntimeError> {
    match base {
        Value::Tuple(items) => {
            let i = crate::runtime::bounds_check(idx, items.len(), "tuple index")?;
            Ok(items[i].clone())
        }
        // A SIMD lane read returns the width-1 scalar (a width-1 `float64` lane is
        // a `Float64`, per the SIMD/Float64 unification).
        Value::Simd { dtype, lanes } => read_simd_lane(*dtype, lanes, idx),
        other => Err(RuntimeError::TypeError(format!(
            "cannot index {}",
            crate::runtime::type_name(other)
        ))),
    }
}

/// Whether a branch condition register holds `True`.
fn is_true(v: &Value) -> bool {
    matches!(v, Value::Bool(true))
        || matches!(v, Value::Simd { dtype: crate::ast::Dtype::Bool, lanes: crate::runtime::SimdLanes::Bool(values) } if values == &[true])
}

/// Materialize a MIR constant into a runtime value.
fn const_value(k: &Const) -> Value {
    match k {
        Const::Int(n) => Value::Int(*n),
        Const::Float(x) => Value::Float64(*x),
        Const::IntLiteral(value) => Value::IntLiteral(value.clone()),
        Const::FloatLiteral(value) => Value::FloatLiteral(value.clone()),
        Const::Bool(b) => Value::Bool(*b),
        Const::Str(s) => Value::Str(s.clone()),
        Const::Function(name) => Value::Function(name.clone()),
        Const::None => Value::None,
    }
}

mod calls;

mod exec;

mod frames;

mod references;

use calls::*;

mod places;

use places::*;

#[cfg(test)]
mod pointer_storage_tests {
    use super::*;

    fn empty_program() -> Prog {
        Prog {
            mir: MirProgram {
                functions: Vec::new(),
                declarations: Default::default(),
                invariant_errors: Vec::new(),
            },
            structs: HashMap::new(),
            sigs: HashMap::new(),
        }
    }

    #[test]
    fn take_and_destroy_restore_uninitialized_heap_storage() {
        let mut vm = VmBackend::default();
        let Value::Pointer { allocation, offset } = vm.heap_alloc(1, 8).expect("allocation") else {
            panic!("allocation did not return a pointer");
        };

        assert!(vm.heap_read(allocation, offset, 0).is_err());
        let (region, slot) = vm.heap_index(allocation, offset, 0).expect("slot");
        vm.heap[region].slots[slot] = Value::Int(7);
        assert_eq!(
            vm.heap_take(allocation, offset, 0)
                .expect("initialized take"),
            Value::Int(7)
        );
        assert!(vm.heap_take(allocation, offset, 0).is_err());

        vm.heap[region].slots[slot] = Value::Tuple(vec![Value::Int(1), Value::Int(2)]);
        vm.heap_destroy(&empty_program(), allocation, offset, 0)
            .expect("initialized destroy");
        assert!(vm.heap_read(allocation, offset, 0).is_err());
    }

    #[test]
    fn uninit_storage_payload_take_and_traps() {
        assert_eq!(
            VmBackend::uninit_storage_payload(
                Value::UninitStorage(Some(Box::new(Value::Int(7)))),
                "take"
            )
            .expect("initialized take"),
            Value::Int(7)
        );
        let uninitialized = VmBackend::uninit_storage_payload(Value::UninitStorage(None), "take");
        assert!(
            uninitialized
                .as_ref()
                .is_err_and(|error| error.to_string().contains("uninitialized MaybeUninit")),
            "expected uninitialized trap, got {uninitialized:?}"
        );
        assert!(VmBackend::uninit_storage_payload(Value::Int(1), "take").is_err());
    }

    #[test]
    fn uninit_payload_store_initializes_and_overwrites_without_drop() {
        let place = |root_ty: Ty| {
            let mut place = MirPlace::root(0, Some(root_ty));
            place.project(Proj::UninitPayload, Ty::Int);
            place
        };
        let storage_ty = Ty::Struct(
            crate::types::UNINIT_STORAGE_TYPE_NAME.to_string(),
            vec![crate::types::TyArg::Ty(Ty::Int)],
        );
        let mut vars = vec![Value::UninitStorage(None)];

        // Reading the payload of uninitialized storage traps.
        assert!(load_place(&mut vars, &[], &place(storage_ty.clone())).is_err());

        // A final payload store initializes the slot...
        store_place(&mut vars, &[], &place(storage_ty.clone()), Value::Int(1))
            .expect("initializing store");
        assert_eq!(
            load_place(&mut vars, &[], &place(storage_ty.clone())).expect("initialized read"),
            Value::Int(1)
        );
        // ...and a second store overwrites raw, without touching the old payload.
        store_place(&mut vars, &[], &place(storage_ty), Value::Int(2)).expect("raw overwrite");
        assert_eq!(vars[0], Value::UninitStorage(Some(Box::new(Value::Int(2)))));
    }

    #[test]
    fn uninit_storage_drops_as_a_leaky_no_op() {
        // Discarding storage that still holds a payload must not run any
        // destructor: upstream MaybeUninit leaks by design.
        let mut vm = VmBackend::default();
        vm.drop_value(
            &empty_program(),
            Value::UninitStorage(Some(Box::new(Value::Str("leaked".to_string())))),
        )
        .expect("no-op drop");
        vm.drop_value(&empty_program(), Value::UninitStorage(None))
            .expect("no-op drop of uninitialized storage");
    }
}

#[cfg(test)]
mod input_override_tests {
    use super::*;

    #[test]
    fn input_override_serves_lines_and_echoes_prompts_to_output() {
        let mut vm = VmBackend::default();
        vm.set_input_override(b"World\r\n".to_vec());

        let first = vm
            .input_from_override(Value::Str("Name: ".to_string()))
            .expect("first injected line");
        assert_eq!(first, Value::Str("World".to_string()));

        // The buffer is exhausted: EOF reads back as the empty string, the
        // same as builtin_input on closed stdin.
        let second = vm
            .input_from_override(Value::Str("Again: ".to_string()))
            .expect("EOF read");
        assert_eq!(second, Value::Str(String::new()));

        // Prompts land in the captured output byte-for-byte (no newline),
        // matching a native executable writing prompts to stdout.
        assert_eq!(vm.output(), "Name: Again: ");
    }

    #[test]
    fn input_override_rejects_a_non_string_prompt() {
        let mut vm = VmBackend::default();
        vm.set_input_override(Vec::new());
        assert!(vm.input_from_override(Value::Int(3)).is_err());
    }
}

mod adapters;
mod dispatch;
mod invoke;
mod values;
