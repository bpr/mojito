//! Register-VM backend and Mojito's sole runtime.
//!
//! The VM executes verified, drop-elaborated [`MirProgram`]s over per-call
//! register and variable frames. Language-level validation belongs to the
//! checker and ownership analysis; this module implements the checked call ABI,
//! places, structured control flow, exceptions, destruction, and runtime
//! primitives. See `docs/features.md` for the supported language surface.

use crate::runtime::RuntimeError;
use crate::runtime::{
    ClosureCapture, RefProjection, Value, apply_infix, apply_prefix, builtin_abs, builtin_convert,
    builtin_divmod, builtin_error, builtin_input, builtin_min_max, builtin_round, read_simd_lane,
    simd_from_values, value_as_index,
};
#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use calls::*;
#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use mojito_ast::call::{ArgSlot, CallVariadics, match_call_slots};
use mojito_checked::checked::{CheckedConst, ComptimeSequence};
use mojito_common::timing;
use mojito_hir::hir::VarId;
use mojito_mir::mir::{
    ConcreteMir, Const, MirBlock, MirCaptureMode, MirFunction, MirInstr, MirIntrinsicSubscript,
    MirPlace, MirProgram, MirSubscriptArg, MirTerm, Proj, Reg,
};
use mojito_types::ct::CtValue;
use mojito_types::param_expr::ParamId;
use mojito_types::types::{
    CallableDefault, ConstraintOperand, GenericConstraint, ParamDecl, Ty, TyArg,
};
#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use places::*;
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
    /// Host state of the `external_call` libc table: descriptors, directory
    /// streams, `errno`, and the environment overlay (see `libc.rs`).
    host: libc::HostState,
    /// The byte buffer the bundled `_int_digits`/`_uint_digits` bodies format
    /// into, allocated on first use and reused — the VM's counterpart to the
    /// native backend's per-function formatting alloca.
    digit_scratch: Option<Value>,
    /// The never-freed UTF-8 byte allocation behind each distinct literal
    /// text, by text — the VM's counterpart to the native backend's interned
    /// `mjstr_<n>` globals, which `StringLiteral.ptr()` addresses.
    static_literals: HashMap<String, u64>,
    /// The never-freed element allocation behind each distinct closed
    /// parameter list, by element type and spelling — the VM's counterpart
    /// to the native backend's constant globals, which
    /// `MirInstr::ParamListAddress` addresses.
    static_param_lists: HashMap<String, u64>,
    /// The erased oracle's position in each running `comptime for`, by frame
    /// and header: the index of the element its slot holds.
    comptime_cursors: HashMap<(FrameId, usize), usize>,
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
            never_written: vec![true; n as usize],
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

    /// The reused digit-formatting buffer, allocated on first use.
    fn digit_scratch(&mut self) -> Result<Value, RuntimeError> {
        if let Some(scratch) = &self.digit_scratch {
            return Ok(scratch.clone());
        }
        let scratch = self.heap_alloc(
            i64::try_from(mojito_symbol::symbol::DIGITS_BUFFER_BYTES)
                .expect("the digit buffer size fits i64"),
            1,
        )?;
        self.digit_scratch = Some(scratch.clone());
        Ok(scratch)
    }

    /// The `length` bytes at `pointer`, read out of the heap arena. Every
    /// byte slot holds a width-1 integer `Simd` value (the representation of
    /// `Byte`), so anything else is a corrupt buffer.
    fn heap_bytes(&self, pointer: &Value, length: i64) -> Result<Vec<u8>, RuntimeError> {
        let Value::Pointer { allocation, offset } = pointer else {
            return Err(RuntimeError::TypeError(
                "vm: byte read requires a Pointer".to_string(),
            ));
        };
        let mut bytes = Vec::with_capacity(length.max(0) as usize);
        for index in 0..length {
            let (arena, slot) = self.heap_index(*allocation, *offset, index)?;
            match self.heap[arena].slots.get(slot) {
                Some(Value::Simd {
                    lanes: crate::runtime::SimdLanes::Int(lanes),
                    ..
                }) if lanes.len() == 1 => bytes.push(lanes[0] as u8),
                other => {
                    return Err(RuntimeError::TypeError(format!(
                        "vm: byte buffer slot is {other:?}, not a byte"
                    )));
                }
            }
        }
        Ok(bytes)
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
        region.never_written.clear();
        Ok(())
    }

    /// Read one initialized heap slot. `Moved` is the VM's raw-storage
    /// tombstone: allocation starts uninitialized and take/destroy restore that
    /// state until an explicit pointer store initializes the slot again. A
    /// slot has three states: never written (a read traps, a take yields the
    /// tombstone, a destroy is a no-op — `unsafe_uninit_length` storage),
    /// taken (read, take, and destroy all trap), and initialized.
    fn heap_read(&self, allocation: u64, base: i64, offset: i64) -> Result<Value, RuntimeError> {
        let (region, slot) = self.heap_index(allocation, base, offset)?;
        match &self.heap[region].slots[slot] {
            Value::Moved => Err(RuntimeError::TypeError(
                "vm: read of uninitialized Pointer storage".to_string(),
            )),
            value => Ok(value.clone()),
        }
    }

    /// Every heap-slot store: the one place the never-written mark is kept
    /// in step with the slot. Storing the tombstone itself (a container's
    /// reallocation forwarding a never-written slot) re-marks the slot.
    fn heap_store(&mut self, region: usize, slot: usize, value: Value) {
        self.heap[region].never_written[slot] = matches!(value, Value::Moved);
        self.heap[region].slots[slot] = value;
    }

    /// Move one initialized raw-storage value out, leaving an uninitialized
    /// tombstone. This intentionally bypasses `__moveinit__`: ownership of the
    /// existing value is transferred rather than constructing another value.
    /// A never-written slot yields the tombstone (so `_realloc` forwards the
    /// state to the new storage) and becomes a taken slot.
    fn heap_take(
        &mut self,
        allocation: u64,
        base: i64,
        offset: i64,
    ) -> Result<Value, RuntimeError> {
        let (region, slot) = self.heap_index(allocation, base, offset)?;
        if std::mem::replace(&mut self.heap[region].never_written[slot], false) {
            return Ok(Value::Moved);
        }
        let value = std::mem::replace(&mut self.heap[region].slots[slot], Value::Moved);
        if matches!(value, Value::Moved) {
            Err(RuntimeError::TypeError(
                "vm: take or destroy of uninitialized Pointer storage".to_string(),
            ))
        } else {
            Ok(value)
        }
    }

    /// Destroy one slot in place; a never-written slot has nothing to destroy.
    fn heap_destroy(
        &mut self,
        prog: &Prog,
        allocation: u64,
        base: i64,
        offset: i64,
    ) -> Result<(), RuntimeError> {
        let (region, slot) = self.heap_index(allocation, base, offset)?;
        if std::mem::replace(&mut self.heap[region].never_written[slot], false) {
            return Ok(());
        }
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
        let fname = resolved.map_or_else(
            || prog.overload_name(&source_fname, args.len().saturating_sub(1)),
            str::to_string,
        );
        let Some(idx) = prog.index_of(&fname) else {
            // `Equatable`'s default `!=`: a struct that declares `__eq__`
            // and no `__ne__` negates its `__eq__`, which an erased body
            // dispatches by name.
            if method == "__ne__"
                && let Value::Bool(equal) = self.call_dunder(prog, sname, "__eq__", args)?
            {
                return Ok(Value::Bool(!equal));
            }
            return Err(RuntimeError::Unsupported(format!(
                "vm: struct '{sname}' has no method '{method}'"
            )));
        };
        self.call_function(prog, idx, args, &[])
    }
}

impl VmBackend {
    /// Run a checked program, entering through `main()` when present. This
    /// executable entry enforces the same pre-drop ownership contract the
    /// production `Compiler` pipeline does, so a stage-composed caller cannot
    /// execute a program the analysis rejects. (The VM-CTFE entry
    /// `run_function_value` deliberately keeps the lighter checked boundary.)
    pub fn run(
        &mut self,
        program: &mojito_checked::checked::CheckedProgram,
    ) -> Result<(), RuntimeError> {
        let lowered = mojito_mir::mir::lower_checked_program(program);
        if !lowered.invariant_errors.is_empty() {
            return Err(RuntimeError::Unsupported(format!(
                "invalid checked program: {}",
                lowered.invariant_errors.join("; ")
            )));
        }
        mojito_analysis::analysis::check_ownership_program(&lowered)
            .map_err(|error| RuntimeError::Unsupported(format!("ownership error: {error}")))?;
        self.run_prog(&build_prog_lowered(lowered)?)
    }

    /// Run a verified, already drop-elaborated MIR program — what
    /// `mir::text::load_artifact` yields — resolving any parameter it still
    /// names at run time. The loading gate is the artifact's
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
        let prog = {
            let _registry = timing::span("registry_build");
            let structs = build_structs(&mir.declarations);
            let sigs = build_sigs(&mir.declarations);
            Prog { mir, structs, sigs }
        };
        let _run = timing::span("execute");
        self.run_prog(&prog)
    }

    /// Run concrete MIR. [`ConcreteMir`] is built only by concrete
    /// verification, so this entry verifies nothing again.
    pub fn run_concrete(&mut self, mir: ConcreteMir) -> Result<(), RuntimeError> {
        self.run_elaborated(mir.into_program())
    }

    /// Call `name` in a verified concrete fragment for its result, under
    /// `fuel`: the elaborator's compile-time evaluation. The remaining fuel
    /// comes back with the value, which [`Self::freeze`] turns into a
    /// compile-time value while this VM still owns its heap.
    pub fn call_concrete(
        &mut self,
        mir: &ConcreteMir,
        name: &str,
        args: Vec<Value>,
        fuel: usize,
    ) -> Result<(Value, usize), RuntimeError> {
        let mir: &MirProgram = mir;
        let prog = Prog {
            structs: build_structs(&mir.declarations),
            sigs: build_sigs(&mir.declarations),
            mir: mir.clone(),
        };
        self.configure_lifecycle(&prog);
        let index = prog
            .index_of(name)
            .ok_or_else(|| RuntimeError::UndefinedVariable(name.to_string()))?;
        self.ctfe_fuel = Some(fuel);
        let result = self.call_function(&prog, index, args, &[]);
        let remaining = self.ctfe_fuel.take().unwrap_or(0);
        Ok((result?, remaining))
    }

    /// A compile-time evaluation's result as a compile-time value: a nominal
    /// `String` becomes its text, a nominal collection (a display's value)
    /// its frozen elements ([`Self::freeze_collection`]), a nominal `Tuple`
    /// its frozen elements, any other struct its frozen fields (the checker
    /// admits only a fieldwise one where a frozen struct is materialized),
    /// and anything else crosses as [`crate::crossing::vm_to_ct`] admits it.
    pub fn freeze(&self, value: Value) -> Result<CtValue, RuntimeError> {
        if let Some(text) = self.nominal_string_text(&value) {
            return Ok(CtValue::Str(text));
        }
        if let Some(collection) = self.freeze_collection(&value)? {
            return Ok(collection);
        }
        match value {
            Value::Struct {
                name,
                fields,
                value_params,
            } if value_params.is_empty() => {
                if let [(storage, Value::Tuple(elements))] = fields.as_slice()
                    && storage == "storage"
                    && is_nominal_tuple(&name)
                {
                    return elements
                        .iter()
                        .map(|element| self.freeze(element.clone()))
                        .collect::<Result<Vec<_>, _>>()
                        .map(CtValue::Tuple);
                }
                Ok(CtValue::Struct {
                    name,
                    fields: fields
                        .into_iter()
                        .map(|(field, value)| Ok((field, self.freeze(value)?)))
                        .collect::<Result<Vec<_>, RuntimeError>>()?,
                })
            }
            value => crate::crossing::vm_to_ct(value),
        }
    }

    /// Captured standard output.
    pub fn output(&self) -> String {
        self.output.clone()
    }

    /// Final top-level bindings, for the CLI `run` dump.
    pub fn bindings(&self) -> Vec<(String, Value)> {
        self.bindings.clone()
    }

    /// A nominal stdlib collection value whose storage lives in this VM's
    /// heap as the compile-time collection it holds, in insertion order: an
    /// `Array` or a `List` as a list, a `Set` as a set, a `Dict` as a
    /// dictionary. `None` for any other value.
    fn freeze_collection(&self, value: &Value) -> Result<Option<CtValue>, RuntimeError> {
        use mojito_types::types::{ARRAY_TYPE_NAME, DICT_TYPE_NAME, LIST_TYPE_NAME, SET_TYPE_NAME};
        let Value::Struct { name, fields, .. } = value else {
            return Ok(None);
        };
        let template = name
            .split_once("$mono$")
            .map_or(name.as_str(), |(base, _)| base);
        let instance_of = |nominal: &str| {
            template
                .strip_suffix(nominal)
                .is_some_and(|module| module.is_empty() || module.ends_with('$'))
        };
        let field = |wanted: &str| {
            fields
                .iter()
                .find(|(field, _)| field == wanted)
                .map(|(_, value)| value)
        };
        let buffer = |size: &str| match (field("data"), field(size)) {
            (Some(Value::Pointer { allocation, offset }), Some(Value::Int(size))) => (0..*size)
                .map(|index| self.heap_read(*allocation, *offset, index))
                .collect::<Result<Vec<_>, _>>()
                .map(Some),
            _ => Ok(None),
        };
        let frozen = |values: Vec<Value>| {
            values
                .into_iter()
                .map(|element| self.freeze(element))
                .collect::<Result<Vec<_>, _>>()
        };
        if instance_of(ARRAY_TYPE_NAME) {
            return buffer("_size")?
                .map(|elements| frozen(elements).map(CtValue::List))
                .transpose();
        }
        if instance_of(LIST_TYPE_NAME) {
            return buffer("size")?
                .map(|elements| frozen(elements).map(CtValue::List))
                .transpose();
        }
        if instance_of(SET_TYPE_NAME) {
            let Some(CtValue::List(elements)) = field("items")
                .map(|items| self.freeze_collection(items))
                .transpose()?
                .flatten()
            else {
                return Ok(None);
            };
            return Ok(Some(CtValue::set(None, elements)));
        }
        if instance_of(DICT_TYPE_NAME) {
            let Some(Value::Struct { fields: list, .. }) = field("entries") else {
                return Ok(None);
            };
            let (Some(Value::Pointer { allocation, offset }), Some(Value::Int(size))) = (
                list.iter()
                    .find(|(name, _)| name == "data")
                    .map(|(_, value)| value),
                list.iter()
                    .find(|(name, _)| name == "size")
                    .map(|(_, value)| value),
            ) else {
                return Ok(None);
            };
            let mut entries = Vec::new();
            for index in 0..*size {
                let Value::Struct { fields: entry, .. } =
                    self.heap_read(*allocation, *offset, index)?
                else {
                    return Ok(None);
                };
                let part = |wanted: &str| {
                    entry
                        .iter()
                        .find(|(name, _)| name == wanted)
                        .map(|(_, value)| self.freeze(value.clone()))
                        .transpose()
                };
                let (Some(key), Some(value)) = (part("key")?, part("value")?) else {
                    return Ok(None);
                };
                entries.push((key, value));
            }
            return Ok(Some(CtValue::dict(None, entries)));
        }
        Ok(None)
    }

    /// The text of a nominal stdlib `String` value whose bytes live in this
    /// VM's heap, or `None` for any other value.
    pub fn nominal_string_text(&self, value: &Value) -> Option<String> {
        if !matches!(value, Value::Struct { name, .. }
            if mojito_symbol::symbol::is_stdlib_string_struct(name))
        {
            return None;
        }
        match self.string_struct_literal(value) {
            Ok(Value::Str(text)) => Some(text),
            _ => None,
        }
    }
}

impl VmBackend {
    fn run_prog(&mut self, prog: &Prog) -> Result<(), RuntimeError> {
        self.configure_lifecycle(prog);
        // Run module initialization, then `main()`. Capture the top-level frame's
        // user variables (skipping synthetic `$…` temporaries) as the global
        // bindings.
        if let Some(top) = prog.index_of("__toplevel__") {
            let (_, vars) = self.call_frame(prog, top, Vec::new(), &[])?;
            let names = &prog.mir.functions[top].1.var_names;
            self.bindings = names
                .iter()
                .zip(&vars)
                .filter(|(name, _)| !name.starts_with('$'))
                .map(|(name, v)| (name.clone(), v.clone()))
                .collect();
        }
        if let Some(main) = prog.index_of("main") {
            self.call_function(prog, main, Vec::new(), &[])?;
        }
        Ok(())
    }
}

/// A `Variant` operation's alternative. Concrete MIR holds only known
/// ones; the erased oracle reifies a pack's length, not its element types,
/// so a template's `_get_type_index` is out of its reach.
pub(super) fn known_variant_index(
    index: &mojito_types::types::VariantIndex,
) -> Result<usize, RuntimeError> {
    index.known().ok_or_else(|| {
        RuntimeError::Unsupported(format!(
            "Variant alternative `{index}` is not known in erased execution"
        ))
    })
}

/// The values an erased frame reads a parameter expression against, by
/// name: its slots and reified value parameters that have a compile-time
/// reading, and each type pack of the signature by its collector's runtime
/// arity — a tuple of as many placeholder elements, which answers the
/// pack's length query and nothing else, since the frame carries no type
/// argument. The index of a `comptime for` inside its loop is its slot's
/// value under the binder's name, whatever spelling the slot took.
pub(super) fn erased_parameter_values(
    function: &MirFunction,
    variables: &[Value],
    comptime: &[(String, Value)],
) -> HashMap<String, CtValue> {
    let packs = function
        .param_types
        .iter()
        .zip(variables)
        .filter_map(|(ty, value)| {
            let (Ty::VariadicPack(element), Value::Tuple(items)) = (ty, value) else {
                return None;
            };
            let Ty::Param { binder, .. } =
                mojito_types::types::pack_spread(std::slice::from_ref(&**element))?
            else {
                return None;
            };
            let placeholder = CtValue::Type(Box::new(Ty::None));
            Some((
                binder.name.trim_start_matches('*').to_string(),
                CtValue::Tuple(vec![placeholder; items.len()]),
            ))
        });
    function
        .var_names
        .iter()
        .zip(variables)
        .chain(comptime.iter().map(|(name, value)| (name, value)))
        .map(|(name, value)| (name.trim_start_matches('*'), value))
        .chain(live_loop_indices(function, variables))
        .filter_map(|(name, value)| {
            runtime_value_as_ct(value).map(|value| (name.to_string(), value))
        })
        .chain(packs)
        .collect()
}

/// The index of each `comptime for` of `function` whose loop is running,
/// under its binder's name: its slot is cleared outside the loop.
fn live_loop_indices<'a>(
    function: &'a MirFunction,
    variables: &'a [Value],
) -> impl Iterator<Item = (&'a str, &'a Value)> {
    function
        .blocks
        .iter()
        .filter_map(|block| match &block.term {
            MirTerm::ComptimeFor { binder, slot, .. } => {
                let value = variables.get(*slot as usize)?;
                (!matches!(value, Value::None)).then_some((binder.name.as_ref(), value))
            }
            _ => None,
        })
}

/// Bind a nested body's inherited enclosing binders beside the call's own
/// parameters, which shadow an enclosing binder of the same name.
fn inherit_parameters(own: &mut Vec<(String, Value)>, inherited: Vec<(String, Value)>) {
    for (name, value) in inherited {
        if !own.iter().any(|(bound, _)| *bound == name) {
            own.push((name, value));
        }
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

/// The display witness of a specialized instance struct: the one `method`
/// body a specialized program keeps for the instance under a suffixed name
/// (`Wrap$mono$TInt.write_to$mono$…`, the writer-generic method's own
/// instance). `None` when the struct declares the plain method, or when no
/// one body of that name exists.
fn specialized_witness_symbol(prog: &Prog, sname: &str, method: &str) -> Option<String> {
    if prog.index_of(&format!("{sname}.{method}")).is_some() {
        return None;
    }
    let mut witnesses = prog.mir.functions.iter().filter_map(|(name, _)| {
        let member = name.strip_prefix(sname)?.strip_prefix('.')?;
        (member != method && member.split('$').next() == Some(method)).then_some(name)
    });
    let witness = witnesses.next()?;
    witnesses.next().is_none().then(|| witness.clone())
}

/// A static type with its reference layers removed: a place or parameter
/// handle describes the same instance as the value behind it.
fn peel_references(ty: &mojito_types::types::Ty) -> &mojito_types::types::Ty {
    let mut ty = ty;
    while let mojito_types::types::Ty::Ref(reference) = ty {
        ty = &reference.referent;
    }
    ty
}

impl Prog {
    fn index_of(&self, name: &str) -> Option<usize> {
        self.mir.functions.iter().position(|(n, _)| n == name)
    }

    /// The values of the binders of the regions a body nested in them is
    /// declared in and of its enclosing declarations' value binders, which
    /// it reads, taken from the frame of `function` that builds a closure
    /// over it: an erased nested body is not instantiated, so its closure
    /// carries them. A `comptime for` index is its slot's value in the
    /// iteration the closure is built in.
    fn inherited_parameters(
        &self,
        target: &str,
        function: &MirFunction,
        variables: &[Value],
        comptime: &[(String, Value)],
    ) -> Vec<(String, Value)> {
        let mut parameters = Vec::new();
        let target = self.sigs.get(target);
        // A region binder of the target is the index of a loop of
        // `function`, whose slot holds the iteration's value.
        for decl in target.map_or(&[][..], |sig| &sig.region_binders) {
            let binder = decl.binder();
            let value = function.blocks.iter().find_map(|block| match &block.term {
                MirTerm::ComptimeFor {
                    binder: index,
                    slot,
                    ..
                } if index.id == binder.id => variables.get(*slot as usize),
                _ => None,
            });
            if let Some(value) = value.filter(|value| !matches!(value, Value::None)) {
                parameters.push((binder.name.to_string(), value.clone()));
            }
        }
        let mut scopes: Vec<&[ParamDecl]> = vec![target.map_or(&[][..], |sig| &sig.region_binders)];
        let mut link = target.and_then(|sig| sig.enclosing.as_deref());
        while let Some(sig) = link.and_then(|name| self.sigs.get(name)) {
            scopes.push(&sig.param_decls);
            scopes.push(&sig.region_binders);
            link = sig.enclosing.as_deref();
        }
        for decl in scopes.into_iter().flatten() {
            let name = match decl {
                ParamDecl::Value { name, .. }
                | ParamDecl::Type {
                    name,
                    variadic: true,
                    ..
                } => name.trim_start_matches('*'),
                ParamDecl::Type { .. } => continue,
            };
            let pack = matches!(decl, ParamDecl::Type { .. })
                .then(|| erased_parameter_values(function, variables, comptime).remove(name))
                .flatten()
                .and_then(|pack| match pack {
                    // Only the pack's length is read off an erased body.
                    CtValue::Tuple(items) => Some(Value::Tuple(vec![Value::Int(0); items.len()])),
                    _ => None,
                });
            let value = pack.as_ref().or_else(|| {
                comptime
                    .iter()
                    .find(|(bound, _)| bound == name)
                    .map(|(_, value)| value)
                    .or_else(|| {
                        function
                            .var_names
                            .iter()
                            .position(|candidate| candidate == name)
                            .and_then(|slot| variables.get(slot))
                    })
                    .filter(|value| !matches!(value, Value::None))
            });
            if let Some(value) = value
                && !parameters.iter().any(|(bound, _)| bound == name)
            {
                parameters.push((name.to_string(), value.clone()));
            }
        }
        parameters
    }

    /// The `hasher`'s `_update_with_simd` a scalar `__hash__` leaf of type
    /// `leaf` calls: the instance the elaborator minted at the leaf's vector
    /// type, else the template itself, whose dtype and width binders an
    /// erased run reifies from the leaf.
    fn hash_leaf_update(&self, hasher: &str, leaf: &mojito_types::types::Ty) -> String {
        let template = format!("{hasher}._update_with_simd");
        let shape = mojito_types::types::simd_shape(&mojito_types::types::hash_leaf_ty(leaf));
        self.mir
            .functions
            .iter()
            .find(|(fname, function)| {
                fname
                    .strip_prefix(&template)
                    .is_some_and(|rest| rest.starts_with('$'))
                    && function
                        .param_types
                        .get(1)
                        .and_then(|value| mojito_types::types::simd_shape(peel_references(value)))
                        == shape
            })
            .map_or(template, |(fname, _)| fname.clone())
    }

    /// The instance of `receiver_type.method` a specialized program minted
    /// for `hasher`'s struct (`String.__hash__$mono$T…AHasher…`), when the
    /// program declares no template of that name: the instance whose hasher
    /// parameter is that struct. `None` keeps by-name dispatch.
    fn hasher_method_instance(
        &self,
        receiver_type: &str,
        method: &str,
        hasher: &Value,
    ) -> Option<String> {
        let Value::Struct { name: hasher, .. } = hasher else {
            return None;
        };
        let symbol = format!("{receiver_type}.{method}");
        if self.index_of(&symbol).is_some() {
            return None;
        }
        let prefix = format!("{symbol}$mono$");
        let mut instances = self.mir.functions.iter().filter(|(fname, function)| {
            fname.starts_with(&prefix)
                && matches!(
                    function.param_types.get(1).map(peel_references),
                    Some(mojito_types::types::Ty::Struct(name, _)) if name == hasher
                )
        });
        match (instances.next(), instances.next()) {
            (Some((fname, _)), None) => Some(fname.clone()),
            _ => None,
        }
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
    /// The compiled `__init__` a construction of `struct_name` with `argc`
    /// positional arguments executes: the exact name, else the unique
    /// same-arity overload, else the unique variadic overload whose
    /// runtime-pack collector binds any element count (current Tuple's
    /// `__init__(out self, var *args: *Ts)` beside its nullary constructor,
    /// which arity-keyed selection cannot see).
    fn constructor_name(&self, struct_name: &str, argc: usize) -> String {
        let init = format!("{struct_name}.__init__");
        let resolved = self.overload_name(&init, argc);
        if self.index_of(&resolved).is_some() {
            return resolved;
        }
        let mut packs = self.mir.functions.iter().filter(|(fname, _)| {
            mojito_symbol::symbol::is_overload_of(fname, &init)
                && self.sigs.get(fname.as_str()).is_some_and(|signature| {
                    signature.variadic.as_ref().is_some_and(|element| {
                        matches!(element, Ty::RuntimePack(_))
                            || mojito_types::types::pack_spread(std::slice::from_ref(element))
                                .is_some()
                    })
                })
        });
        match (packs.next(), packs.next()) {
            (Some((fname, _)), None) => fname.clone(),
            _ => resolved,
        }
    }

    fn overload_name(&self, name: &str, argc: usize) -> String {
        mojito_symbol::symbol::resolve_callable_symbol(
            self.mir.functions.iter().map(|(name, function)| {
                mojito_symbol::symbol::CallableCandidate {
                    name,
                    n_params: function.n_params,
                }
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
        mojito_symbol::symbol::resolve_method_symbol(
            self.mir.functions.iter().map(|(name, function)| {
                mojito_symbol::symbol::CallableCandidate {
                    name,
                    n_params: function.n_params,
                }
            }),
            receiver_type,
            method,
            resolved,
            argc,
        )
    }
}

struct CallerFrame<'a> {
    id: FrameId,
    /// The caller's function (its compile-time bindings resolve by name).
    function: usize,
    registers: &'a mut [Value],
    variables: &'a mut Vec<Value>,
    /// The caller frame's compile-time bindings (`Frame::comptime`).
    comptime: &'a [(String, Value)],
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
    /// default MIR could not lower — using such a slot errors). A
    /// `CheckedConst::Construct` default is materialized at bind time by running
    /// its converting constructor, an `Evaluate` one by running its default
    /// function (see `bind_for_call`); scalars fold directly.
    defaults: Vec<Option<CheckedConst>>,
    required: Vec<bool>,
    variadic: Option<Ty>,
    /// Where the collected `*args` list belongs among source parameters. For a
    /// signature like `def f(a, *xs, b)`, this is `Some(1)`.
    variadic_index: Option<usize>,
    kw_variadic: Option<Ty>,
    kw_variadic_index: Option<usize>,
    /// The frame slot of an owning positional collector.
    owned_pack_slot: Option<mojito_hir::hir::VarId>,
    /// Indexes into the regular-parameter list.
    positional_only: Option<usize>,
    keyword_only: Option<usize>,
    /// Checker-resolved compile-time parameters. Value parameters become typed
    /// frame locals; type parameters remain erased.
    param_decls: Vec<ParamDecl>,
    /// The declaration a nested function is nested in, whose value binders
    /// its body reads.
    enclosing: Option<String>,
    /// The binders of the regions a nested function is declared in, which
    /// its body reads.
    region_binders: Vec<ParamDecl>,
}

impl FnSig {
    /// The regular parameter at a frame slot, where the collectors
    /// `bind_args` inserts occupy slots of their own;
    /// `None` at a collector's slot.
    fn frame_param_name(&self, frame_slot: usize) -> Option<&str> {
        let mut index = frame_slot;
        if let Some(keyword) = self.kw_variadic_index {
            if index == keyword {
                return None;
            }
            index -= usize::from(index > keyword);
        }
        if self.variadic.is_some() {
            let positional = self.variadic_index.unwrap_or(self.param_names.len());
            if index == positional {
                return None;
            }
            index -= usize::from(index > positional);
        }
        self.param_names.get(index).map(String::as_str)
    }
}

/// The name a solved type argument reifies to in an erased frame: the
/// spelling [`MirInstr::ConstructTypeParam`] constructs from. A type with no
/// runtime constructor by name (a symbolic one, a SIMD vector) reifies to
/// nothing, and the slot stays unsupplied.
fn reified_type_spelling(ty: &Ty) -> Option<String> {
    Some(match ty {
        Ty::Struct(name, _) => name.clone(),
        Ty::Int => "Int".to_string(),
        Ty::UInt => "UInt".to_string(),
        Ty::Bool => "Bool".to_string(),
        Ty::Float64 => "Float64".to_string(),
        Ty::StringLiteral => "StringLiteral".to_string(),
        Ty::None => "NoneType".to_string(),
        _ => return None,
    })
}

/// The erased frame's reification of a type argument: its spelling, or,
/// for a struct instance over value arguments (`AHasher[key]`), a type token
/// — a fieldless `Value::Struct` whose `value_params` carry them — so a
/// construction of the binder (`H()`) builds the instance at its arguments.
fn reified_type_value(prog: &Prog, ty: &Ty) -> Option<Value> {
    let Ty::Struct(name, arguments) = ty else {
        return reified_type_spelling(ty).map(Value::Str);
    };
    let value_params: Vec<(String, Value)> = prog
        .structs
        .get(name)
        .map(|definition| {
            definition
                .param_decls
                .iter()
                .zip(arguments.iter())
                .filter_map(|(declaration, argument)| match (declaration, argument) {
                    (ParamDecl::Value { name, ty, .. }, TyArg::Val(value)) => Some((
                        name.trim_start_matches('*').to_string(),
                        crate::runtime::coerce_checked(
                            ct_value_as_runtime(value.clone())?,
                            ty.as_ref(),
                        ),
                    )),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    Some(if value_params.is_empty() {
        Value::Str(name.clone())
    } else {
        type_token(name, value_params)
    })
}

/// A reified struct type over value arguments (see [`reified_type_value`]).
fn type_token(name: &str, value_params: Vec<(String, Value)>) -> Value {
    Value::Struct {
        name: name.to_string(),
        fields: Vec::new(),
        value_params,
    }
}

/// Reify generic value parameters in declaration order. Missing source
/// arguments are filled from checked scalar/callable defaults; callable aliases
/// can therefore reuse an earlier runtime closure without ever converting its
/// capture payload into `CtValue`.
fn reify_value_parameters(
    prog: &Prog,
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
                let value = match resolved.get(index).cloned().flatten() {
                    Some(value @ (Value::Str(_) | Value::Tuple(_) | Value::Struct { .. })) => value,
                    _ if constructible_type_parameter(declaration) => match default.as_deref() {
                        Some(default @ Ty::Struct(..)) => reified_type_value(prog, default)?,
                        _ => return None,
                    },
                    _ => return None,
                };
                return Some((name.clone(), value));
            };
            let value = resolved
                .get(index)
                .cloned()
                .flatten()
                .unwrap_or(Value::None);
            Some((
                name.trim_start_matches('*').to_string(),
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
        Value::Dtype(dtype) => CtValue::Dtype(*dtype),
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
        Value::Simd { dtype, lanes } => CtValue::Simd {
            dtype: *dtype,
            lanes: match lanes {
                crate::runtime::SimdLanes::Int(lanes) => lanes
                    .iter()
                    .copied()
                    .map(mojito_types::ct::CtLane::Int)
                    .collect(),
                crate::runtime::SimdLanes::Float(lanes) => lanes
                    .iter()
                    .map(|lane| mojito_types::ct::CtLane::Float(lane.to_bits()))
                    .collect(),
                crate::runtime::SimdLanes::Bool(lanes) => lanes
                    .iter()
                    .copied()
                    .map(mojito_types::ct::CtLane::Bool)
                    .collect(),
            },
        },
        // A type token compares by its spelling, as every reified type does.
        Value::Struct {
            name,
            fields,
            value_params,
        } if fields.is_empty() && !value_params.is_empty() => CtValue::Str(name.clone()),
        Value::Struct { name, fields, .. } => CtValue::Struct {
            name: name.clone(),
            fields: fields
                .iter()
                .map(|(field, value)| Some((field.clone(), runtime_value_as_ct(value)?)))
                .collect::<Option<Vec<_>>>()?,
        },
        _ => return None,
    })
}

/// The value parameter `binder` of the struct a static method's
/// receiver-less `self` slot carries, in the caller's frame.
fn static_self_parameter(prog: &Prog, caller: CallerBindings<'_>, binder: &str) -> Option<Value> {
    let function = &prog.mir.functions[caller.function].1;
    let slot = function
        .var_names
        .iter()
        .skip(function.n_params)
        .position(|var| var == "self")?;
    let Value::Struct { value_params, .. } = caller.variables.get(function.n_params + slot)? else {
        return None;
    };
    value_params
        .iter()
        .find(|(name, _)| name == binder)
        .map(|(_, value)| value.clone())
}

/// Whether the struct `name` is an instance of the nominal `Tuple`.
fn is_nominal_tuple(name: &str) -> bool {
    let template = name.split_once("$mono$").map_or(name, |(base, _)| base);
    template
        .strip_suffix(mojito_types::types::TUPLE_TYPE_NAME)
        .is_some_and(|module| module.is_empty() || module.ends_with('$'))
}

fn ct_value_as_runtime(value: CtValue) -> Option<Value> {
    Some(match value {
        CtValue::Int(value) => Value::Int(value),
        CtValue::UInt(value) => Value::UInt(value),
        CtValue::Simd { dtype, lanes } => {
            use mojito_types::ct::CtLane;
            let lanes = match lanes.first()? {
                CtLane::Float(_) => crate::runtime::SimdLanes::Float(
                    lanes
                        .iter()
                        .map(|lane| match lane {
                            CtLane::Float(bits) => Some(f64::from_bits(*bits)),
                            _ => None,
                        })
                        .collect::<Option<Vec<_>>>()?,
                ),
                CtLane::Bool(_) => crate::runtime::SimdLanes::Bool(
                    lanes
                        .iter()
                        .map(|lane| match lane {
                            CtLane::Bool(value) => Some(*value),
                            _ => None,
                        })
                        .collect::<Option<Vec<_>>>()?,
                ),
                CtLane::Int(_) => crate::runtime::SimdLanes::Int(
                    lanes
                        .iter()
                        .map(|lane| match lane {
                            CtLane::Int(value) => Some(*value),
                            _ => None,
                        })
                        .collect::<Option<Vec<_>>>()?,
                ),
            };
            Value::Simd { dtype, lanes }
        }
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
        CtValue::Dtype(dtype) => Value::Dtype(dtype),
        CtValue::Struct { name, fields } => Value::Struct {
            name,
            fields: fields
                .into_iter()
                .map(|(field, value)| Some((field, ct_value_as_runtime(value)?)))
                .collect::<Option<Vec<_>>>()?,
            value_params: Vec::new(),
        },
        CtValue::Dict { .. }
        | CtValue::Set { .. }
        | CtValue::Type(_)
        | CtValue::Reflected(_)
        // No unresolved expression or deferred slot becomes a runtime value.
        | CtValue::Expr(_)
        | CtValue::Deferred(_) | CtValue::Marker(_) => return None,
    })
}

/// A type over the erased frame's value binders, closed by the values the
/// frame reifies: an erased body keyed on a `DType` or lane binder
/// (`Scalar[Self.dtype]` in a range-family member) builds its values at the
/// lane its frame binds. A closed type, or one over a binder the frame does
/// not hold, is returned as it is.
fn erased_closed_ty(
    ty: &Ty,
    function: &MirFunction,
    variables: &[Value],
    comptime: &[(String, Value)],
) -> Ty {
    if comptime.is_empty() || !mojito_types::types::is_symbolic(ty) {
        return ty.clone();
    }
    let named = erased_parameter_values(function, variables, comptime);
    let context = mojito_types::param_expr::ParamContext::detached();
    let bindings = mojito_types::param_expr::ParamBindings::from_named_values(&context, &named);
    mojito_types::types::replace_parameters(&context, ty, &bindings, 0)
        .unwrap_or_else(|_| ty.clone())
}

/// The length of the type list an application spreads (`Tuple[*Ts.reverse()]`,
/// `Tuple[*Self.Ts]`), counted under the erased frame's reified packs.
fn erased_list_length(
    arguments: &[TyArg],
    function: &MirFunction,
    variables: &[Value],
    comptime: &[(String, Value)],
) -> Option<usize> {
    let context = mojito_types::param_expr::ParamContext::detached();
    let named = erased_parameter_values(function, variables, comptime);
    let bindings = mojito_types::param_expr::ParamBindings::from_named_values(&context, &named);
    if let Some(Ty::Param { binder, .. }) = mojito_types::types::pack_spread_argument(arguments) {
        return match named.get(binder.name.trim_start_matches('*')) {
            Some(mojito_types::ct::CtValue::Tuple(elements)) => Some(elements.len()),
            _ => None,
        };
    }
    let list = mojito_types::types::list_spread_argument(arguments)?;
    let length = context.list_length(list).ok()?;
    context
        .replace(&length, &bindings)
        .ok()?
        .as_constant()
        .and_then(mojito_types::param_expr::fold::integer_value)
        .and_then(|length| length.to_i64())
        .and_then(|length| usize::try_from(length).ok())
}

/// Decide a `comptime if` on the erased path from the frame's reified value
/// parameters: a comparison over value binders, constants, and expressions
/// of them. A condition over a type binder has no erased reading, since an
/// erased frame carries no type argument.
/// The block a `comptime for` header hands control to on the erased path,
/// which runs the loop as written: the slot holds the iteration's element,
/// or nothing before the first and after the last, and `cursors` the
/// element's position, keyed by the frame and the header — an empty slot
/// starts the sequence the frame's reified parameters decide, which a range
/// spans as `range` does, and the slot is cleared on exit.
fn comptime_for_next(
    header: &MirTerm,
    function: &MirFunction,
    variables: &mut [Value],
    comptime: &[(String, Value)],
    cursors: &mut HashMap<(FrameId, usize), usize>,
    frame: FrameId,
) -> Result<usize, RuntimeError> {
    let MirTerm::ComptimeFor {
        slot,
        source,
        body,
        exit,
        ..
    } = header
    else {
        return Err(RuntimeError::Unsupported(
            "a compile-time loop header was expected".to_string(),
        ));
    };
    let unsupported = |what: &str| {
        RuntimeError::Unsupported(format!(
            "the erased oracle cannot decide the comptime for {what} `{source}`"
        ))
    };
    let named = erased_parameter_values(function, variables, comptime);
    let evaluate = |expr: &mojito_types::param_expr::ParamExpr, what: &str| {
        expr.evaluate_named(&named).map_err(|_| unsupported(what))
    };
    let key = (frame, std::ptr::from_ref(header) as usize);
    let slot = *slot as usize;
    let position = if matches!(variables[slot], Value::None) {
        0
    } else {
        cursors.get(&key).map_or(0, |position| position + 1)
    };
    let next = match source {
        ComptimeSequence::Range { start, stop, step } => {
            let bound = |expr, what| {
                mojito_types::param_expr::fold::integer_value(&evaluate(expr, what)?)
                    .and_then(|value| value.to_i64())
                    .ok_or_else(|| unsupported(what))
            };
            let (start, stop, step) = (
                bound(start, "start")?,
                bound(stop, "stop")?,
                bound(step, "step")?,
            );
            i64::try_from(position)
                .ok()
                .and_then(|position| position.checked_mul(step))
                .and_then(|offset| start.checked_add(offset))
                .filter(|next| (step > 0 && *next < stop) || (step < 0 && *next > stop))
                .map(Value::Int)
        }
        ComptimeSequence::Elements(elements) => evaluate(elements, "sequence")?
            .comptime_iteration_elements()
            .ok_or_else(|| unsupported("sequence"))?
            .into_iter()
            .nth(position)
            .map(|element| ct_value_as_runtime(element).ok_or_else(|| unsupported("element")))
            .transpose()?,
    };
    if let Some(next) = next {
        variables[slot] = next;
        cursors.insert(key, position);
        Ok(*body)
    } else {
        variables[slot] = Value::None;
        cursors.remove(&key);
        Ok(*exit)
    }
}

fn comptime_branch_holds(
    cond: &GenericConstraint,
    function: &MirFunction,
    variables: &[Value],
    comptime: &[(String, Value)],
) -> Result<bool, RuntimeError> {
    use GenericConstraint::{And, Bool, Eq, Ge, Gt, Le, Lt, Ne, Not, Or, WithMessage};
    use mojito_ast::ast::InfixOp;
    let unsupported = || {
        RuntimeError::Unsupported(format!(
            "the erased oracle cannot decide the comptime if condition `{cond:?}`"
        ))
    };
    let named = erased_parameter_values(function, variables, comptime);
    let operand = |operand: &ConstraintOperand| match operand {
        ConstraintOperand::Param(param) => named.get(param.name.as_ref()).cloned(),
        ConstraintOperand::Value(CtValue::Expr(expr)) | ConstraintOperand::Expr(expr) => {
            expr.evaluate_named(&named).ok()
        }
        ConstraintOperand::Value(value) => Some(value.clone()),
        ConstraintOperand::PackLength(pack) => match named.get(pack.name.trim_start_matches('*')) {
            Some(CtValue::Tuple(elements)) => i64::try_from(elements.len()).ok().map(CtValue::Int),
            _ => None,
        },
        // A type parameter reifies as its type's spelling, so a type operand
        // compares by the same spelling; an element of a reified pack
        // (`Self.Ts[i]`) is its element's spelling.
        ConstraintOperand::Type(Ty::Dependent(dependent)) => {
            let (list, index) = dependent.pack_element()?;
            let pack = list.as_decl_ref()?;
            let CtValue::Tuple(elements) = named.get(pack.name.trim_start_matches('*'))? else {
                return None;
            };
            let index =
                mojito_types::param_expr::fold::integer_value(&index.evaluate_named(&named).ok()?)?
                    .to_i64()?;
            match elements.get(usize::try_from(index).ok()?)? {
                CtValue::Type(ty) => reified_type_spelling(ty).map(CtValue::Str),
                spelling @ CtValue::Str(_) => Some(spelling.clone()),
                _ => None,
            }
        }
        ConstraintOperand::Type(ty) => reified_type_spelling(ty).map(CtValue::Str),
    };
    let compare = |op, left, right| {
        let (left, right) = (
            operand(left).ok_or_else(unsupported)?,
            operand(right).ok_or_else(unsupported)?,
        );
        match (op, &left, &right) {
            (InfixOp::Eq, CtValue::Str(left), CtValue::Str(right)) => Ok(left == right),
            (InfixOp::Eq, CtValue::Bool(left), CtValue::Bool(right)) => Ok(left == right),
            _ => mojito_types::param_expr::fold::compare(op, &left, &right)
                .map_err(|_| unsupported()),
        }
    };
    let holds = |inner| comptime_branch_holds(inner, function, variables, comptime);
    match cond {
        Bool(value) => Ok(*value),
        WithMessage(inner, _) => holds(inner),
        Not(inner) => Ok(!holds(inner)?),
        And(left, right) => Ok(holds(left)? && holds(right)?),
        Or(left, right) => Ok(holds(left)? || holds(right)?),
        Eq(left, right) => compare(InfixOp::Eq, left, right),
        Ne(left, right) => Ok(!compare(InfixOp::Eq, left, right)?),
        Lt(left, right) => compare(InfixOp::Lt, left, right),
        Le(left, right) => compare(InfixOp::Le, left, right),
        Gt(left, right) => compare(InfixOp::Gt, left, right),
        Ge(left, right) => compare(InfixOp::Ge, left, right),
        GenericConstraint::Conforms { .. }
        | GenericConstraint::ConformsPack { .. }
        | GenericConstraint::PackPredicate { .. }
        | GenericConstraint::PackContains { .. }
        | GenericConstraint::Trivial(..) => Err(unsupported()),
    }
}

fn resolve_callable_default(
    default: &CallableDefault,
    runtime: &HashMap<ParamId, Value>,
    comptime: &HashMap<String, CtValue>,
) -> Option<Value> {
    match default {
        CallableDefault::Symbol(symbol) => Some(Value::Function(symbol.clone())),
        CallableDefault::Parameter(parameter) => runtime.get(&parameter.id).cloned(),
        CallableDefault::If {
            condition,
            then_value,
            else_value,
        } => match condition.evaluate_named(comptime).ok()? {
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
pub fn constructible_type_parameter(declaration: &ParamDecl) -> bool {
    mojito_types::types::constructible_type_parameter(declaration)
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
            id,
            name,
            ty,
            default,
            callable_default,
            ..
        } = declaration
        else {
            // A reified type argument passes through as the bound type's
            // name (a type token, for an instance over value arguments; a
            // pack, as its elements' names). The declaration's own bounds do
            // not gate it: a constructor may default-construct `Self.T`
            // under a `where conforms_to(Self.T, Defaultable)` clause on an
            // `AnyType` binder (current Array's nullary `__init__`).
            resolved[index] = supplied.get(index).cloned().flatten().filter(|value| {
                matches!(
                    value,
                    Value::Str(_) | Value::Tuple(_) | Value::Struct { .. }
                )
            });
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
                        .evaluate_named(&comptime)
                        .ok()
                        .and_then(|value| value.materialize_as(ty))
                        .and_then(ct_value_as_runtime)
                })
            })
            .map(|value| match (value, declaration) {
                // A value pack coerces each of its values.
                (Value::Tuple(values), ParamDecl::Value { variadic: true, .. }) => Value::Tuple(
                    values
                        .into_iter()
                        .map(|value| crate::runtime::coerce_checked(value, ty.as_ref()))
                        .collect(),
                ),
                (value, _) => crate::runtime::coerce_checked(value, ty.as_ref()),
            });
        let Some(value) = value else {
            continue;
        };
        runtime.insert(id.clone(), value.clone());
        if let Some(value) = runtime_value_as_ct(&value) {
            comptime.insert(name.clone(), value);
        }
        resolved[index] = Some(value);
    }
    resolved
}

struct Frame {
    id: FrameId,
    function: usize,
    registers: Vec<Value>,
    variables: Vec<Value>,
    block: usize,
    instruction: usize,
    continuation: Option<ReturnContinuation>,
    /// The reified value parameters of an erased generic body, by name:
    /// what a `comptime if` over a value binder reads on the erased path.
    comptime: Vec<(String, Value)>,
}

/// The frame an instruction executes in: its function, its id, and the value
/// parameters it reifies (`Frame::comptime`).
#[derive(Clone, Copy)]
struct FrameScope<'a> {
    function: usize,
    id: FrameId,
    comptime: &'a [(String, Value)],
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
    result_adapter: Option<mojito_checked::checked::CheckedResultAdapter>,
    arguments: Vec<Value>,
    keyword_arguments: Vec<(String, Value)>,
    receiver_place: &'a Option<MirPlace>,
    argument_places: &'a [Option<MirPlace>],
    keyword_argument_places: &'a [Option<MirPlace>],
    parameter_arguments: &'a [mojito_mir::mir::MirParamArg],
    parameter_declarations: &'a [mojito_types::types::ParamDecl],
    /// The method's own compile-time arguments the checker solved
    /// (`MirInstr::MethodCall::instantiated_args`).
    instantiated_arguments: &'a [TyArg],
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
    /// True for a slot no store has ever initialized — distinct from the
    /// `Moved` tombstone a take or destroy leaves. `unsafe_uninit_length`
    /// storage lives here: reading traps, taking yields a tombstone, and
    /// destroying is a no-op, while a taken slot still traps on both.
    never_written: Vec<bool>,
    #[allow(dead_code)]
    alignment: usize,
    live: bool,
}

fn build_prog_lowered(lowered: mojito_mir::mir::MirProgram) -> Result<Prog, RuntimeError> {
    let mut mir = mojito_analysis::analysis::elaborate_drops_program(lowered);
    // The VM executes the drop-elaborated program, so it is re-verified after
    // the DropVar/edge-cleanup rewrite — the elaborated MIR must satisfy the
    // same contract the pre-elaboration program did.
    mir.invariant_errors
        .extend(mojito_mir::mir::verify::verify(&mir));
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
/// an erased type argument still occupies its selected declaration slot. A
/// value pack takes every positional argument from its slot on, as the tuple
/// of its values.
fn align_parameter_arguments(
    declarations: &[ParamDecl],
    arguments: Vec<(Option<String>, Option<Value>)>,
) -> Vec<Option<Value>> {
    // A pack, of values or of types (its elements reified by spelling),
    // collects every positional argument from its position on.
    let value_pack = |index: usize| {
        matches!(
            declarations[index],
            ParamDecl::Value { variadic: true, .. } | ParamDecl::Type { variadic: true, .. }
        )
    };
    let mut aligned: Vec<Option<Value>> = (0..declarations.len())
        .map(|index| value_pack(index).then(|| Value::Tuple(Vec::new())))
        .collect();
    let mut next_positional = 0;
    for (name, value) in arguments {
        let index = if let Some(name) = name {
            declarations
                .iter()
                .position(|declaration| declaration.name().trim_start_matches('*') == name)
        } else {
            while declarations
                .get(next_positional)
                .is_some_and(|declaration| match declaration {
                    ParamDecl::Type { infer_only, .. } | ParamDecl::Value { infer_only, .. } => {
                        *infer_only
                    }
                })
            {
                next_positional += 1;
            }
            let index = (next_positional < declarations.len()).then_some(next_positional);
            next_positional += usize::from(index.is_some_and(|index| !value_pack(index)));
            index
        };
        let Some(index) = index else {
            continue;
        };
        if !value_pack(index) {
            aligned[index] = value;
        } else if let (Some(Value::Tuple(values)), Some(value)) = (&mut aligned[index], value) {
            // A value pack's elements are scalars, so a tuple is a pack
            // spread whole (`f[*vs]()`).
            match value {
                Value::Tuple(spread) if matches!(declarations[index], ParamDecl::Value { .. }) => {
                    values.extend(spread);
                }
                value => values.push(value),
            }
        }
    }
    aligned
}

impl VmBackend {
    /// The bindings a static method call's spelled receiver (`W[5].st()`,
    /// `Box[Int].accepts[Int]()`) gives its frame: each type parameter of the
    /// receiver's struct reified by name, so a `comptime if` on `Self.T`
    /// decides as in an instance method, and the receiver-less `self` slot
    /// of a value-parameterized struct, bound with its value parameters
    /// reified so the body's `Self.k` reads it as an instance method reads
    /// its receiver's. A receiver argument naming an enclosing binder
    /// (`W[Self.k]`) resolves in the caller's frame.
    fn static_receiver_binding(
        &self,
        prog: &Prog,
        caller: CallerBindings<'_>,
        callee: usize,
        receiver: Option<&Ty>,
    ) -> Vec<(String, Value)> {
        let Some(Ty::Struct(name, arguments)) = receiver else {
            return Vec::new();
        };
        let Some(definition) = prog.structs.get(name) else {
            return Vec::new();
        };
        let mut bindings: Vec<(String, Value)> = definition
            .param_decls
            .iter()
            .zip(arguments.iter())
            .filter_map(|(declaration, argument)| {
                let (
                    ParamDecl::Type {
                        name,
                        variadic: false,
                        ..
                    },
                    TyArg::Ty(ty),
                ) = (declaration, argument)
                else {
                    return None;
                };
                let value = match ty {
                    Ty::Param { binder, .. } => self
                        .bound_type_parameter(
                            prog,
                            caller.function,
                            caller.frame,
                            caller.variables,
                            &binder.name,
                        )
                        .or_else(|| caller.comptime_binding(&binder.name))?,
                    ty => reified_type_value(prog, ty)?,
                };
                Some((name.clone(), value))
            })
            .collect();
        bindings.extend(self.static_self_binding(prog, caller, callee, name, arguments));
        bindings
    }

    /// The receiver-less `self` slot of a static method on a
    /// value-parameterized struct ([`Self::static_receiver_binding`]);
    /// `None` when the callee has no such slot.
    fn static_self_binding(
        &self,
        prog: &Prog,
        caller: CallerBindings<'_>,
        callee: usize,
        name: &str,
        arguments: &mojito_types::types::TyArgs,
    ) -> Option<(String, Value)> {
        let function = &prog.mir.functions[callee].1;
        function
            .var_names
            .iter()
            .skip(function.n_params)
            .any(|var| var == "self")
            .then_some(())?;
        let value_params = prog
            .structs
            .get(name)?
            .param_decls
            .iter()
            .zip(arguments)
            .filter_map(|(declaration, argument)| {
                let (ParamDecl::Value { name, ty, .. }, TyArg::Val(value)) =
                    (declaration, argument)
                else {
                    return None;
                };
                let value = match value {
                    CtValue::Expr(expression) => {
                        let binder = &expression.as_decl_ref()?.name;
                        self.bound_type_parameter(
                            prog,
                            caller.function,
                            caller.frame,
                            caller.variables,
                            binder,
                        )
                        .or_else(|| caller.comptime_binding(binder))
                        .or_else(|| static_self_parameter(prog, caller, binder))?
                    }
                    value => ct_value_as_runtime(value.clone())?,
                };
                Some((
                    name.clone(),
                    crate::runtime::coerce_checked(value, ty.as_ref()),
                ))
            })
            .collect();
        Some((
            "self".to_string(),
            Value::Struct {
                name: name.to_string(),
                fields: Vec::new(),
                value_params,
            },
        ))
    }

    /// A compile-time value as a run-time value of the declared type `ty`,
    /// [`Self::freeze`]'s inverse: a string becomes a nominal `String` (a
    /// `StringLiteral` keeps its text), a tuple at a nominal `Tuple` type the
    /// struct holding its thawed elements in `storage`, a struct its fields
    /// thawed at their declared types, and anything else crosses as
    /// [`crate::crossing::ct_to_vm`] admits it.
    fn thaw(&mut self, prog: &Prog, value: &CtValue, ty: &Ty) -> Result<Value, RuntimeError> {
        match (value, ty) {
            (CtValue::Str(text), ty) if *ty != Ty::StringLiteral => {
                self.nominal_string_value(prog, text)
            }
            (CtValue::Tuple(elements), Ty::Struct(name, _)) if is_nominal_tuple(name) => {
                let element_types = mojito_types::types::tuple_elements(ty).unwrap_or_default();
                if element_types.len() != elements.len() {
                    return Err(RuntimeError::Unsupported(format!(
                        "vm: a compile-time tuple does not match its declared type {ty}"
                    )));
                }
                let thawed = elements
                    .iter()
                    .zip(&element_types)
                    .map(|(element, element_ty)| self.thaw(prog, element, element_ty))
                    .collect::<Result<Vec<_>, _>>()?;
                let spellings = element_types
                    .iter()
                    .map(|element| {
                        Value::Str(
                            reified_type_spelling(element).unwrap_or_else(|| element.to_string()),
                        )
                    })
                    .collect();
                Ok(Value::Struct {
                    name: name.clone(),
                    fields: vec![("storage".to_string(), Value::Tuple(thawed))],
                    value_params: vec![("*Ts".to_string(), Value::Tuple(spellings))],
                })
            }
            (CtValue::Struct { name, fields }, ty) => {
                let declared = prog
                    .structs
                    .get(name)
                    .map(|definition| definition.fields.clone())
                    .unwrap_or_default();
                let fields = fields
                    .iter()
                    .map(|(field, value)| {
                        let (_, field_ty) = declared
                            .iter()
                            .find(|(candidate, _)| candidate == field)
                            .ok_or_else(|| {
                                RuntimeError::Unsupported(format!(
                                    "vm: compile-time struct '{name}' has no field '{field}'"
                                ))
                            })?;
                        Ok((field.clone(), self.thaw(prog, value, field_ty)?))
                    })
                    .collect::<Result<Vec<_>, RuntimeError>>()?;
                let value_params = match reified_type_value(prog, ty) {
                    Some(Value::Struct { value_params, .. }) => value_params,
                    _ => Vec::new(),
                };
                Ok(Value::Struct {
                    name: name.clone(),
                    fields,
                    value_params,
                })
            }
            (value, _) => crate::crossing::ct_to_vm(value),
        }
    }

    /// The supplied compile-time arguments of a call, aligned to the callee's
    /// declarations. A reified type argument spelled as the caller's own binder
    /// (`hash[Self.H](key)` in an erased struct body, `Const::Str("H")`)
    /// resolves through the caller frame's reified parameters; a spelling bound
    /// nowhere passes through for the callee's declaration default.
    #[allow(
        clippy::needless_pass_by_value,
        reason = "borrow bundle: `From<&Frame>` builds it at each call site"
    )]
    #[allow(
        clippy::needless_pass_by_value,
        reason = "borrow bundle: `From<&Frame>` builds it at each call site"
    )]
    /// [`Self::runtime_parameter_arguments`] completed by the type arguments
    /// the checker solved for a generic `def` or method call: a type
    /// parameter the brackets spelled no struct name for
    /// (`make[Tuple[Int, Bool]]()`) reifies as the solved struct's name.
    fn supplied_parameter_arguments(
        &mut self,
        prog: &Prog,
        caller: CallerBindings<'_>,
        declarations: &[ParamDecl],
        arguments: &[mojito_mir::mir::MirParamArg],
        instantiated: &[TyArg],
    ) -> Vec<Option<Value>> {
        let mut supplied = self.runtime_parameter_arguments(prog, caller, declarations, arguments);
        for ((slot, argument), declaration) in
            supplied.iter_mut().zip(instantiated).zip(declarations)
        {
            // A forwarded pack (`f[*Ts]`, a spread) replaces the empty
            // element list an unsupplied pack slot starts with.
            let pack = matches!(declaration, ParamDecl::Type { variadic: true, .. });
            match argument {
                TyArg::Ty(ty) if slot.is_none() || pack => {
                    if let Some(value) = reified_type_value(prog, ty) {
                        *slot = Some(value);
                    } else if let Ty::Param { binder, .. } = ty
                        && let Some(value) = self
                            .bound_type_parameter(
                                prog,
                                caller.function,
                                caller.frame,
                                caller.variables,
                                &binder.name,
                            )
                            .or_else(|| caller.comptime_binding(&binder.name))
                    {
                        // A binder of the caller's own forwards the type the
                        // caller's frame reified for it.
                        *slot = Some(value);
                    }
                }
                // A type pack's solution is its element list, which the
                // erased frame keeps for the pack's length.
                // A closed value the call carries as compile-time data
                // (`tup[(1, Tag(8, "x"))]()`, `s[3]()`) has no register; the
                // erased frame holds it thawed at its declared type.
                TyArg::Val(value)
                    if slot.is_none()
                        && value.is_folded_parameter_argument()
                        && let ParamDecl::Value {
                            ty,
                            variadic: false,
                            ..
                        } = declaration =>
                {
                    *slot = self.thaw(prog, value, ty).ok();
                }
                TyArg::Val(CtValue::Tuple(elements))
                    if matches!(declaration, ParamDecl::Type { variadic: true, .. }) =>
                {
                    *slot = Some(Value::Tuple(
                        elements
                            .iter()
                            .map(|element| match element {
                                CtValue::Type(ty) => Value::Str(
                                    reified_type_spelling(ty).unwrap_or_else(|| ty.to_string()),
                                ),
                                _ => Value::None,
                            })
                            .collect(),
                    ));
                }
                _ => {}
            }
        }
        supplied
    }

    /// A constructed instance's compile-time arguments: those the call
    /// supplied, with each unsupplied type parameter reified from the call's
    /// checked result type, so `Cell(9)` builds the same `Cell[Int]` value an
    /// explicit `Cell[Int](9)` does, and its methods' erased frames bind
    /// `Self.T` from the receiver. An argument over the caller's binder
    /// (`Box[T](v)` in a generic `def`) reads what the caller's frame bound.
    fn constructed_parameter_arguments(
        &self,
        prog: &Prog,
        caller: CallerBindings<'_>,
        declarations: &[ParamDecl],
        mut supplied: Vec<Option<Value>>,
        result_ty: Option<&Ty>,
    ) -> Vec<Option<Value>> {
        let Some(Ty::Struct(_, arguments)) = result_ty else {
            return supplied;
        };
        supplied.resize(supplied.len().max(declarations.len()), None);
        let spelling =
            |ty: &Ty| Value::Str(reified_type_spelling(ty).unwrap_or_else(|| ty.to_string()));
        // A pack is its elements' spellings, read off the checked type
        // whatever the call's own arguments reified: the type binds it whole,
        // or, keyed on the pack alone, element by element. A spread of the
        // caller's own pack (`V[*Ts]`) keeps the elements the call forwarded.
        if let [ParamDecl::Type { variadic: true, .. }] = declarations
            && !matches!(
                &arguments[..],
                [TyArg::Val(_) | TyArg::Ty(Ty::RuntimePack(_))]
            )
            && mojito_types::types::pack_spread_argument(arguments).is_none()
        {
            supplied[0] = Some(Value::Tuple(
                arguments
                    .iter()
                    .filter_map(|argument| match argument {
                        TyArg::Ty(ty) => Some(spelling(ty)),
                        _ => None,
                    })
                    .collect(),
            ));
            return supplied;
        }
        for ((slot, declaration), argument) in
            supplied.iter_mut().zip(declarations).zip(arguments.iter())
        {
            match (declaration, argument) {
                (ParamDecl::Type { variadic: true, .. }, TyArg::Val(CtValue::Tuple(elements))) => {
                    *slot = Some(Value::Tuple(
                        elements
                            .iter()
                            .map(|element| match element {
                                CtValue::Type(ty) => spelling(ty),
                                _ => Value::None,
                            })
                            .collect(),
                    ));
                    continue;
                }
                (ParamDecl::Type { variadic: true, .. }, TyArg::Ty(Ty::RuntimePack(elements))) => {
                    *slot = Some(Value::Tuple(elements.iter().map(spelling).collect()));
                    continue;
                }
                _ => {}
            }
            let (
                None,
                ParamDecl::Type {
                    variadic: false, ..
                },
                TyArg::Ty(ty),
            ) = (&slot, declaration, argument)
            else {
                continue;
            };
            *slot = match ty {
                Ty::Param { binder, .. } => self
                    .bound_type_parameter(
                        prog,
                        caller.function,
                        caller.frame,
                        caller.variables,
                        &binder.name,
                    )
                    .or_else(|| caller.comptime_binding(&binder.name)),
                ty => reified_type_value(prog, ty),
            };
        }
        supplied
    }

    fn runtime_parameter_arguments(
        &self,
        prog: &Prog,
        caller: CallerBindings<'_>,
        declarations: &[ParamDecl],
        arguments: &[mojito_mir::mir::MirParamArg],
    ) -> Vec<Option<Value>> {
        let bound = |spelling: &str| {
            self.bound_type_parameter(
                prog,
                caller.function,
                caller.frame,
                caller.variables,
                spelling,
            )
            .or_else(|| caller.comptime_binding(spelling))
        };
        align_parameter_arguments(
            declarations,
            arguments
                .iter()
                .flat_map(|argument| {
                    let value = argument
                        .value
                        .map(|register| caller.registers[register.0 as usize].clone());
                    // A spread of the caller's own pack (`V[*Ts]`) forwards
                    // each element the caller's frame bound for it.
                    if let Some(Value::Str(spelling)) = &value
                        && spelling.starts_with('*')
                        && let Some(Value::Tuple(elements)) = bound(spelling)
                    {
                        return elements
                            .into_iter()
                            .map(|element| (argument.name.clone(), Some(element)))
                            .collect();
                    }
                    let value = value.map(|value| match value {
                        Value::Str(spelling) if !prog.structs.contains_key(&spelling) => {
                            bound(&spelling).unwrap_or(Value::Str(spelling))
                        }
                        other => other,
                    });
                    vec![(argument.name.clone(), value)]
                })
                .collect(),
        )
    }

    /// The runtime binding of the compile-time type parameter `param` in the
    /// frame of `function`, reified as the bound struct's name: a def binds its
    /// reified parameters into the frame local of the same name; a struct method
    /// reads them from `self`'s reified parameters — through the handle when
    /// `self` is a reference (a `mut self` method called on a value still under
    /// construction, `self[k] = v` inside `Dict.__init__`).
    fn bound_type_parameter(
        &self,
        prog: &Prog,
        function: usize,
        frame: FrameId,
        variables: &[Value],
        param: &str,
    ) -> Option<Value> {
        let definition = &prog.mir.functions[function].1;
        let receiver_parameter = |receiver: &Value| match receiver {
            Value::Struct { value_params, .. } => value_params
                .iter()
                .find(|(candidate, _)| candidate == param)
                .map(|(_, value)| value.clone()),
            _ => None,
        };
        definition
            .var_names
            .iter()
            .position(|candidate| candidate == param)
            .map(|slot| variables[slot].clone())
            .filter(|value| !matches!(value, Value::None))
            .or_else(|| match variables.first() {
                Some(receiver @ Value::Struct { .. }) => receiver_parameter(receiver),
                Some(reference @ Value::Ref { .. }) => self
                    .read_reference(reference, frame, variables)
                    .ok()
                    .and_then(|receiver| receiver_parameter(&receiver)),
                _ => None,
            })
            // A method-level type parameter inferred from an argument
            // (`__hash__[H2: Hasher](self, mut hasher: H2)` → `H2()`): the
            // parameter's runtime struct names the bound type. A `mut` parameter
            // holds a reference handle; the caller reads through it.
            .or_else(|| {
                let signature = prog.sigs.get(&prog.mir.functions[function].0)?;
                let parameter = signature
                    .param_names
                    .iter()
                    .zip(&signature.param_types)
                    .find(|(_, ty)| {
                        matches!(ty, Ty::Param { binder, .. } if binder.name.as_ref() == param)
                    })
                    .map(|(name, _)| name)?;
                let slot = definition
                    .var_names
                    .iter()
                    .position(|candidate| candidate == parameter)?;
                match &variables[slot] {
                    Value::Struct {
                        name, value_params, ..
                    } => Some(if value_params.is_empty() {
                        Value::Str(name.clone())
                    } else {
                        type_token(name, value_params.clone())
                    }),
                    reference @ Value::Ref { .. } => Some(reference.clone()),
                    _ => None,
                }
            })
    }
}

#[derive(Clone, Copy)]
struct CallerBindings<'a> {
    function: usize,
    frame: FrameId,
    registers: &'a [Value],
    variables: &'a [Value],
    /// The caller frame's compile-time bindings (`Frame::comptime`).
    comptime: &'a [(String, Value)],
}

impl CallerBindings<'_> {
    /// What the caller's frame binds its compile-time parameter `name` to:
    /// a reified type's spelling or a value parameter's value, including a
    /// struct parameter read off its receiver.
    fn comptime_binding(&self, name: &str) -> Option<Value> {
        self.comptime
            .iter()
            .find(|(bound, _)| bound == name)
            .map(|(_, value)| value.clone())
    }
}

impl<'a> From<&'a Frame> for CallerBindings<'a> {
    fn from(frame: &'a Frame) -> Self {
        CallerBindings {
            function: frame.function,
            frame: frame.id,
            registers: &frame.registers,
            variables: &frame.variables,
            comptime: &frame.comptime,
        }
    }
}

struct ReturnContinuation {
    dest: Reg,
    writebacks: Vec<(usize, MirPlace)>,
}

/// Build the VM registry from declaration metadata carried by MIR.
fn build_structs(declarations: &mojito_mir::mir::MirDeclarations) -> HashMap<String, StructDef> {
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
fn build_sigs(declarations: &mojito_mir::mir::MirDeclarations) -> HashMap<String, FnSig> {
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
                    owned_pack_slot: declaration.owned_pack_slot(),
                    positional_only: declaration.positional_only,
                    keyword_only: declaration.keyword_only,
                    param_decls: declaration.param_decls.clone(),
                    enclosing: declaration.enclosing.clone(),
                    region_binders: declaration.region_binders.clone(),
                },
            )
        })
        .collect()
}

fn navigate_reference_mut<'a>(
    mut value: &'a mut Value,
    projection: &[RefProjection],
) -> Result<&'a mut Value, RuntimeError> {
    for segment in projection {
        value = match segment {
            RefProjection::Field(name) => match value {
                Value::Struct {
                    fields,
                    value_params,
                    ..
                } => fields
                    .iter_mut()
                    .chain(value_params.iter_mut())
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
                            .map_or_else(|| "<invalid>".to_string(), ToString::to_string),
                        alternatives
                            .get(*expected)
                            .map_or_else(|| "<invalid>".to_string(), ToString::to_string)
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
#[allow(
    clippy::needless_pass_by_value,
    reason = "TODO: only the Index arm stores it"
)]
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
    crate::runtime::set_simd_lane(*dtype, lanes, *index as i64, &value)?;
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

impl VmBackend {
    /// The value of a compile-time application an erased frame reads as a
    /// constant (`comptime x = f(n)`): the function called with the
    /// frame's values of its arguments, its compile-time ones bound by name
    /// as an erased call binds them. `None` for any other parameter
    /// constant, or an application of no function of the program.
    fn erased_application(
        &mut self,
        prog: &Prog,
        expr: &mojito_types::param_expr::ParamExpr,
        function: &MirFunction,
        variables: &[Value],
        comptime: &[(String, Value)],
    ) -> Result<Option<Value>, RuntimeError> {
        let mojito_types::param_expr::ParamKind::Apply {
            function: callee,
            args,
        } = expr.kind()
        else {
            return Ok(None);
        };
        let (Some(index), Some(declaration)) = (
            prog.index_of(callee),
            prog.mir
                .declarations
                .functions
                .iter()
                .find(|declaration| declaration.lowered_name == *callee),
        ) else {
            return Ok(None);
        };
        // A static method of a generic struct is applied to its instance
        // first, which an erased call does not pass.
        let on_instance = mojito_symbol::symbol::split_method_symbol(callee)
            .and_then(|(owner, _)| prog.structs.get(owner))
            .is_some_and(|owner| !owner.param_decls.is_empty());
        let args = &args[usize::from(on_instance).min(args.len())..];
        let parameters = erased_parameter_values(function, variables, comptime);
        let value = |arg: &mojito_types::param_expr::ParamExpr| {
            arg.evaluate_named(&parameters)
                .ok()
                .and_then(ct_value_as_runtime)
                .ok_or_else(|| {
                    RuntimeError::Unsupported(format!(
                        "the erased oracle cannot evaluate the argument `{arg}` of `{expr}`"
                    ))
                })
        };
        let (compile_time, runtime) = args.split_at(declaration.param_decls.len().min(args.len()));
        let mut value_params = Vec::new();
        for (decl, arg) in declaration.param_decls.iter().zip(compile_time) {
            if let ParamDecl::Value { name, .. } = decl {
                value_params.push((name.clone(), value(arg)?));
            }
        }
        let runtime = runtime.iter().map(value).collect::<Result<Vec<_>, _>>()?;
        // A compile-time evaluation is fuel-bounded wherever it runs.
        let outermost = self.ctfe_fuel.is_none();
        if outermost {
            self.ctfe_fuel = Some(crate::crossing::CTFE_FUEL);
        }
        let result = self.call_function(prog, index, runtime, &value_params);
        if outermost {
            self.ctfe_fuel = None;
        }
        result.map(Some)
    }
}

/// Whether a branch condition register holds `True`.
fn is_true(v: &Value) -> bool {
    matches!(v, Value::Bool(true))
        || matches!(v, Value::Simd { dtype: mojito_ast::ast::Dtype::Bool, lanes: crate::runtime::SimdLanes::Bool(values) } if values == &[true])
}

/// Materialize a MIR constant into a runtime value. A parameter constant
/// reaches the VM only on the erased path, which reads it against the
/// frame's reified parameters as `comptime_for_next` reads a loop bound.
fn const_value(
    k: &Const,
    function: &MirFunction,
    variables: &[Value],
    comptime: &[(String, Value)],
) -> Result<Value, RuntimeError> {
    Ok(match k {
        Const::Int(n) => Value::Int(*n),
        Const::Float(x) => Value::Float64(*x),
        Const::IntLiteral(value) => Value::IntLiteral(value.clone()),
        Const::FloatLiteral(value) => Value::FloatLiteral(value.clone()),
        Const::Bool(b) => Value::Bool(*b),
        Const::Str(s) => Value::Str(s.clone()),
        Const::Function(name) => Value::Function(name.clone()),
        Const::Dtype(dtype) => Value::Dtype(*dtype),
        Const::None => Value::None,
        Const::Value(value) => ct_value_as_runtime(value.clone()).ok_or_else(|| {
            RuntimeError::Unsupported(format!(
                "no runtime value for the parameter value `{value}`"
            ))
        })?,
        Const::Param(expr) => expr
            .evaluate_named(&erased_parameter_values(function, variables, comptime))
            .ok()
            .and_then(ct_value_as_runtime)
            .ok_or_else(|| {
                RuntimeError::Unsupported(format!(
                    "the erased oracle cannot evaluate the parameter constant `{expr}`"
                ))
            })?,
    })
}

mod calls;

mod exec;

mod frames;

mod references;

mod places;

#[cfg(test)]
mod pointer_storage_tests {
    use super::*;

    fn empty_program() -> Prog {
        Prog {
            mir: MirProgram {
                functions: Vec::new(),
                declarations: mojito_mir::mir::MirDeclarations::default(),
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
        vm.heap_store(region, slot, Value::Int(7));
        assert_eq!(
            vm.heap_take(allocation, offset, 0)
                .expect("initialized take"),
            Value::Int(7)
        );
        assert!(vm.heap_take(allocation, offset, 0).is_err());

        vm.heap_store(
            region,
            slot,
            Value::Tuple(vec![Value::Int(1), Value::Int(2)]),
        );
        vm.heap_destroy(&empty_program(), allocation, offset, 0)
            .expect("initialized destroy");
        assert!(vm.heap_read(allocation, offset, 0).is_err());
    }

    #[test]
    fn never_written_heap_storage_is_destroyable_and_forwards_on_take() {
        let mut vm = VmBackend::default();
        let Value::Pointer { allocation, offset } = vm.heap_alloc(2, 8).expect("allocation") else {
            panic!("allocation did not return a pointer");
        };
        // Never written: a read traps, a destroy is a no-op, a take yields the
        // tombstone once and leaves a taken slot that traps thereafter.
        assert!(vm.heap_read(allocation, offset, 0).is_err());
        vm.heap_destroy(&empty_program(), allocation, offset, 0)
            .expect("destroying a never-written slot is a no-op");
        assert!(
            vm.heap_destroy(&empty_program(), allocation, offset, 0)
                .is_err()
        );
        assert!(matches!(
            vm.heap_take(allocation, offset, 1)
                .expect("take forwards the tombstone"),
            Value::Moved
        ));
        assert!(vm.heap_take(allocation, offset, 1).is_err());
        // Storing the tombstone re-marks the slot; storing a value clears it.
        let (region, slot) = vm.heap_index(allocation, offset, 1).expect("slot");
        vm.heap_store(region, slot, Value::Moved);
        vm.heap_destroy(&empty_program(), allocation, offset, 1)
            .expect("a forwarded never-written slot destroys as a no-op");
        vm.heap_store(region, slot, Value::Int(3));
        assert_eq!(
            vm.heap_read(allocation, offset, 1).expect("read"),
            Value::Int(3)
        );
        assert_eq!(
            vm.heap_take(allocation, offset, 1).expect("take"),
            Value::Int(3)
        );
        assert!(vm.heap_take(allocation, offset, 1).is_err());
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
            mojito_types::types::UNINIT_STORAGE_TYPE_NAME.to_string(),
            vec![mojito_types::types::TyArg::Ty(Ty::Int)].into(),
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

        // The buffer is exhausted: EOF raises, the same as builtin_input on
        // closed stdin and as upstream's `input()`.
        let second = vm
            .input_from_override(Value::Str("Again: ".to_string()))
            .expect_err("EOF raises");
        assert!(
            matches!(&second, RuntimeError::Raised(Value::Error(message)) if message == "EOF"),
            "{second:?}"
        );

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
use dispatch::CallTypes;
mod invoke;
mod libc;
mod values;
use values::ConstructorParameters;
