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
use mojito_checked::checked::CheckedConst;
use mojito_common::timing;
use mojito_hir::hir::VarId;
use mojito_mir::mir::{
    ConcreteMir, Const, MirBlock, MirCaptureMode, MirInstr, MirIntrinsicSubscript, MirPlace,
    MirProgram, MirSubscriptArg, MirTerm, Proj, Reg,
};
use mojito_types::ct::CtValue;
use mojito_types::types::Ty;
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

    /// Execute a function for its return value only.
    fn call_function(
        &mut self,
        prog: &Prog,
        fidx: usize,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        Ok(self.call_frame(prog, fidx, args)?.0)
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
            // and no `__ne__` negates its `__eq__`.
            if method == "__ne__"
                && let Value::Bool(equal) = self.call_dunder(prog, sname, "__eq__", args)?
            {
                return Ok(Value::Bool(!equal));
            }
            return Err(RuntimeError::Unsupported(format!(
                "vm: struct '{sname}' has no method '{method}'"
            )));
        };
        self.call_function(prog, idx, args)
    }
}

impl VmBackend {
    /// Run concrete MIR, entering through `main()` when present.
    /// [`ConcreteMir`] is built only by concrete verification, so this entry
    /// verifies nothing again: the drop schedule and every instantiation are
    /// already final.
    pub fn run_concrete(&mut self, mir: ConcreteMir) -> Result<(), RuntimeError> {
        let mir: MirProgram = mir.into_program();
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

    /// Call `name` in a verified concrete fragment for its result, under
    /// `fuel`: the elaborator's compile-time evaluation. The remaining fuel
    /// comes back with the value, which [`Self::freeze`] turns into a
    /// compile-time value while this VM still owns its heap.
    pub fn call_concrete(
        &mut self,
        mir: &ConcreteMir,
        name: &str,
        args: &[CtValue],
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
        let param_types = &prog.mir.functions[index].1.param_types;
        if args.len() != param_types.len() {
            return Err(RuntimeError::ArityMismatch {
                name: name.to_string(),
                expected: param_types.len(),
                got: args.len(),
            });
        }
        let args = args
            .iter()
            .zip(param_types)
            .map(|(value, ty)| self.thaw(&prog, value, Some(ty)))
            .collect::<Result<Vec<_>, _>>()?;
        self.ctfe_fuel = Some(fuel);
        let result = self.call_function(&prog, index, args);
        let remaining = self.ctfe_fuel.take().unwrap_or(0);
        Ok((result?, remaining))
    }

    /// Freeze a result while this VM owns its heap. Pointer leaves retain
    /// allocation contents and uninitialized slots; strings retain their text.
    pub fn freeze(&self, value: Value) -> Result<CtValue, RuntimeError> {
        self.freeze_value(value, &mut std::collections::HashSet::new())
    }

    /// Captured standard output.
    pub fn output(&self) -> String {
        self.output.clone()
    }

    /// Final top-level bindings, for the CLI `run` dump.
    pub fn bindings(&self) -> Vec<(String, Value)> {
        self.bindings.clone()
    }

    fn freeze_value(
        &self,
        value: Value,
        active: &mut std::collections::HashSet<u64>,
    ) -> Result<CtValue, RuntimeError> {
        if let Some(text) = self.nominal_string_text(&value) {
            return Ok(CtValue::Str(text));
        }
        match value {
            Value::Pointer { allocation, offset } => {
                let memory = if allocation == 0 {
                    None
                } else {
                    let region = usize::try_from(allocation - 1)
                        .ok()
                        .and_then(|index| self.heap.get(index))
                        .filter(|region| region.live)
                        .ok_or_else(|| {
                            RuntimeError::Unsupported(
                                "cannot freeze a pointer to a dead or invalid allocation"
                                    .to_string(),
                            )
                        })?;
                    if !active.insert(allocation) {
                        return Err(RuntimeError::Unsupported(
                            "cannot freeze cyclic pointer memory".to_string(),
                        ));
                    }
                    let slots = region
                        .slots
                        .iter()
                        .zip(&region.never_written)
                        .map(|(slot, never)| {
                            if *never || matches!(slot, Value::Moved) {
                                Ok(None)
                            } else {
                                self.freeze_value(slot.clone(), active).map(Some)
                            }
                        })
                        .collect::<Result<Vec<_>, RuntimeError>>()?;
                    active.remove(&allocation);
                    Some(slots)
                };
                Ok(CtValue::Pointer { memory, offset })
            }
            Value::Struct { name, fields } => {
                if let [(storage, Value::Tuple(elements))] = fields.as_slice()
                    && storage == "storage"
                    && is_nominal_tuple(&name)
                {
                    return elements
                        .iter()
                        .map(|value| self.freeze_value(value.clone(), active))
                        .collect::<Result<Vec<_>, _>>()
                        .map(CtValue::Tuple);
                }
                Ok(CtValue::Struct {
                    name,
                    fields: fields
                        .into_iter()
                        .map(|(name, value)| Ok((name, self.freeze_value(value, active)?)))
                        .collect::<Result<Vec<_>, RuntimeError>>()?,
                })
            }
            Value::Tuple(elements) => elements
                .into_iter()
                .map(|value| self.freeze_value(value, active))
                .collect::<Result<Vec<_>, _>>()
                .map(CtValue::Tuple),
            Value::ComptimeList(elements) => elements
                .into_iter()
                .map(|value| self.freeze_value(value, active))
                .collect::<Result<Vec<_>, _>>()
                .map(CtValue::List),
            value => crate::crossing::vm_to_ct(value),
        }
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
            let (_, vars) = self.call_frame(prog, top, Vec::new())?;
            let names = &prog.mir.functions[top].1.var_names;
            self.bindings = names
                .iter()
                .zip(&vars)
                .filter(|(name, _)| !name.starts_with('$'))
                .map(|(name, v)| (name.clone(), v.clone()))
                .collect();
        }
        if let Some(main) = prog.index_of("main") {
            self.call_function(prog, main, Vec::new())?;
        }
        Ok(())
    }
}

/// A `Variant` operation's alternative. Concrete MIR holds only known ones.
pub(super) fn known_variant_index(
    index: &mojito_types::types::VariantIndex,
) -> Result<usize, RuntimeError> {
    index.known().ok_or_else(|| {
        RuntimeError::Unsupported(format!(
            "concrete MIR carries no symbolic Variant alternative `{index}`"
        ))
    })
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

    /// The `hasher`'s `_update_with_simd` a scalar `__hash__` leaf of type
    /// `leaf` calls: the instance the elaborator minted at the leaf's vector
    /// type, else the symbol as named.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct FrameId(u64);

struct SynchronousCall<'a> {
    function_index: usize,
    arguments: Vec<Value>,
    reference_inputs: &'a [(usize, Value)],
}

/// A struct type's runtime shape, gathered from the program AST (the MIR doesn't
/// keep field layout): field names + types (for constructor coercion), and which
/// methods take `mut self` (so their receiver is written back).
struct StructDef {
    fields: Vec<(String, Ty)>,
    mut_self_methods: std::collections::HashSet<String>,
    fieldwise_init: bool,
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
        },
        CtValue::Pointer { .. }
        | CtValue::Dict { .. }
        | CtValue::Set { .. }
        | CtValue::Type(_)
        | CtValue::Reflected(_)
        // No unresolved expression or deferred slot becomes a runtime value.
        | CtValue::Expr(_)
        | CtValue::Deferred(_) | CtValue::Marker(_) => return None,
    })
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

struct Frame {
    id: FrameId,
    function: usize,
    registers: Vec<Value>,
    variables: Vec<Value>,
    block: usize,
    instruction: usize,
    continuation: Option<ReturnContinuation>,
}

/// The frame an instruction executes in: its function and its id.
#[derive(Clone, Copy)]
struct FrameScope {
    function: usize,
    id: FrameId,
}

struct WritebackCall<'a> {
    function_name: &'a str,
    function_index: usize,
    positional_args: Vec<Value>,
    keyword_args: Vec<(String, Value)>,
    argument_places: &'a [Option<MirPlace>],
    keyword_argument_places: &'a [Option<MirPlace>],
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

/// Read a struct field by name.
fn get_field(base: &Value, field: &str) -> Result<Value, RuntimeError> {
    match base {
        Value::Struct { fields, .. } => fields
            .iter()
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

/// Whether a branch condition register holds `True`.
fn is_true(v: &Value) -> bool {
    matches!(v, Value::Bool(true))
        || matches!(v, Value::Simd { dtype: mojito_ast::ast::Dtype::Bool, lanes: crate::runtime::SimdLanes::Bool(values) } if values == &[true])
}

/// The error for a parametric instruction, terminator, or constant the VM
/// met: concrete verification rejects every one before a program runs, so
/// meeting `what` here is a compiler invariant violation.
fn parametric_instruction(what: &str) -> RuntimeError {
    RuntimeError::Unsupported(format!("concrete MIR carries no {what}"))
}

/// Materialize a MIR constant into a runtime value. A parameter constant is
/// a verifier finding on concrete MIR, never a runtime read.
fn const_value(k: &Const) -> Result<Value, RuntimeError> {
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
        Const::Param(expr) => {
            return Err(parametric_instruction(&format!(
                "the parameter constant `{expr}`"
            )));
        }
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
    fn frozen_pointer_round_trip_preserves_holes_offsets_and_independence() {
        let mut vm = VmBackend::default();
        let source = vm.heap_alloc(3, 8).expect("allocation");
        let Value::Pointer { allocation, .. } = source else {
            unreachable!("heap allocator returns a pointer")
        };
        vm.heap_store(allocation as usize - 1, 1, Value::Int(7));
        let frozen = vm
            .freeze(Value::Pointer {
                allocation,
                offset: 1,
            })
            .expect("freeze");
        assert_eq!(
            frozen,
            CtValue::Pointer {
                memory: Some(vec![None, Some(CtValue::Int(7)), None]),
                offset: 1
            }
        );
        let ty = Ty::Pointer {
            element: Box::new(Ty::Int),
            origin: mojito_types::origin::PointerOrigin::Untracked { mutable: true },
        };
        let first = vm.thaw(&empty_program(), &frozen, Some(&ty)).expect("thaw");
        let second = vm
            .thaw(&empty_program(), &frozen, Some(&ty))
            .expect("thaw again");
        assert_ne!(first, second);
        assert_ne!(first, source);
        let Value::Pointer {
            allocation: first_id,
            offset,
        } = first
        else {
            unreachable!("heap allocator returns a pointer")
        };
        assert_eq!(offset, 1);
        assert_eq!(
            vm.heap_read(first_id, offset, 0).expect("initialized"),
            Value::Int(7)
        );
        assert!(vm.heap_read(first_id, 0, 0).is_err());
        assert!(vm.heap_read(first_id, 0, 2).is_err());
        vm.heap_store(first_id as usize - 1, 1, Value::Int(9));
        assert_eq!(vm.freeze(second).expect("independent allocation"), frozen);
        vm.heap_free(allocation, 0).expect("free source");
        assert!(vm.freeze(source).is_err());
        let copy = vm
            .thaw(&empty_program(), &frozen, Some(&ty))
            .expect("survives source");
        assert_eq!(vm.freeze(copy).expect("freeze copy"), frozen);
    }

    #[test]
    fn frozen_pointer_distinguishes_dangling_and_empty_and_rejects_cycles() {
        let mut vm = VmBackend::default();
        assert_eq!(
            vm.freeze(Value::Pointer {
                allocation: 0,
                offset: 0
            })
            .expect("dangling"),
            CtValue::Pointer {
                memory: None,
                offset: 0
            }
        );
        let empty = vm.heap_alloc(0, 8).expect("empty allocation");
        assert_eq!(
            vm.freeze(empty).expect("empty"),
            CtValue::Pointer {
                memory: Some(vec![]),
                offset: 0
            }
        );
        let cycle = vm.heap_alloc(1, 8).expect("allocation");
        let Value::Pointer { allocation, .. } = cycle else {
            unreachable!("heap allocator returns a pointer")
        };
        vm.heap_store(allocation as usize - 1, 0, cycle.clone());
        assert!(
            vm.freeze(cycle)
                .expect_err("cycle")
                .to_string()
                .contains("cyclic pointer memory")
        );
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
