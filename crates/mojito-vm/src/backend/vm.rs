//! Register-VM backend and Mojito's sole runtime.
//!
//! The VM executes verified, drop-elaborated [`MirProgram`]s over per-call
//! register and variable frames. Language-level validation belongs to the
//! checker and ownership analysis; this module implements the checked call ABI,
//! places, structured control flow, exceptions, destruction, and runtime
//! primitives. See `docs/features.md` for the supported language surface.

use crate::runtime::RuntimeError;
use crate::runtime::{
    ClosureCapture, RefProjection, Value, apply_infix, apply_prefix, builtin_abs, builtin_divmod,
    builtin_error, builtin_input, builtin_min_max, builtin_round, simd_from_values, value_as_index,
};
#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use calls::*;
#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use mojito_ast::call::{ArgSlot, CallVariadics, match_call_slots};
use mojito_checked::checked::CheckedConst;
use mojito_common::timing;
use mojito_hir::hir::VarId;
use mojito_mir::mir::{
    ConcreteMir, Const, MirBlock, MirCaptureMode, MirFunction, MirInstr, MirIntrinsicSubscript,
    MirPlace, MirProgram, MirSubscriptArg, MirTerm, Proj, Raises, Reg,
};
use mojito_types::ct::CtValue;
use mojito_types::ct::ScalarConversion;
use mojito_types::types::Ty;
#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use places::*;
use std::collections::{HashMap, HashSet};

/// A verified concrete fragment prepared for compile-time calls, its struct
/// and signature registries built once for every call into it.
pub struct ConcreteProgram(Prog);

impl ConcreteProgram {
    /// Prepare `mir`, whose structured control flow the VM runs as it
    /// stands.
    pub fn new(mir: &ConcreteMir) -> Self {
        let mir: MirProgram = (**mir).clone();
        Self(Prog::new(mir))
    }

    /// The result type of the function `name`, which a value
    /// [`VmBackend::call_concrete`] returned freezes at: an initializer's
    /// is the struct its `out self` receiver constructs.
    pub fn result_ty(&self, name: &str) -> Option<&Ty> {
        let index = self.0.index_of(name)?;
        let function = &self.0.mir.functions[index].1;
        match self.0.constructed(name, &function.param_types) {
            Some(_) => function.param_types.first(),
            None => function.ret_ty.as_ref(),
        }
    }
}

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
    /// The never-freed UTF-8 byte allocation behind each distinct literal
    /// text, by text — the VM's counterpart to the native backend's interned
    /// `mjstr_<n>` globals, which `StringLiteral.ptr()` addresses.
    static_literals: HashMap<String, u64>,
    /// The never-freed element allocation behind each distinct closed
    /// parameter list, by element type and spelling — the VM's counterpart
    /// to the native backend's constant globals, which
    /// `MirInstr::ParamListAddress` addresses.
    static_param_lists: HashMap<String, u64>,
    /// Compile-time memory: the never-destroyed storage behind each
    /// [`MirInstr::StoreToMem`] — upstream's `store_to_mem`, a borrowed
    /// parameter-domain value — addressed by a [`Value::Ref`] rooted at
    /// [`COMPTIME_MEMORY_FRAME`].
    comptime_memory: Vec<Value>,
    /// The heap pointers references into a container's owned interior are
    /// rooted at, interned by allocation and offset and addressed by a
    /// [`Value::Ref`] rooted at [`POINTER_ROOT_FRAME`]: such a reference
    /// designates the heap element, so it outlives the slot (a temporary
    /// `Span`) that held the pointer reaching it.
    pointer_roots: Vec<Value>,
    pointer_root_slots: HashMap<(u64, i64), usize>,
}

/// The reserved frame id of compile-time memory. Frame ids count up from zero,
/// so no live frame ever takes it.
pub(super) const COMPTIME_MEMORY_FRAME: u64 = u64::MAX;

/// The reserved frame id of the interned heap pointers
/// ([`VmBackend::pointer_root`]), beside [`COMPTIME_MEMORY_FRAME`].
pub(super) const POINTER_ROOT_FRAME: u64 = u64::MAX - 1;

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
        self.has_copyinit = prog.owns_member("__copyinit__");
        self.has_moveinit = prog.owns_member("__moveinit__");
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

    /// Destroy one slot of `element` values in place by `destructor`, the
    /// `__deinit__` the storage destroy names ([`Self::drop_named`]); a
    /// never-written slot has nothing to destroy.
    fn heap_destroy(
        &mut self,
        prog: &Prog,
        allocation: u64,
        base: i64,
        offset: i64,
        element: Option<&Ty>,
        destructor: Option<&str>,
    ) -> Result<(), RuntimeError> {
        let (region, slot) = self.heap_index(allocation, base, offset)?;
        if std::mem::replace(&mut self.heap[region].never_written[slot], false) {
            return Ok(());
        }
        let value = self.heap_take(allocation, base, offset)?;
        self.drop_named(prog, value, element, destructor)
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
        let argc = args.len().saturating_sub(1);
        // A member the backends reach from a value's type is the instance
        // the struct's member table names.
        let idx = match resolved {
            Some(resolved) => prog.index_of(resolved),
            None => prog.struct_member(sname, method, argc),
        };
        let Some(idx) = idx else {
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
    /// already final. Its structured control flow runs as it stands, each
    /// frame's region cursor stepping the region ops.
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
            Prog::new(mir)
        };
        let _run = timing::span("execute");
        self.run_prog(&prog)
    }

    /// Call `name` in a prepared concrete fragment for its result, under
    /// `fuel`: the elaborator's compile-time evaluation. The remaining fuel
    /// comes back with the value, which [`Self::freeze`] turns into a
    /// compile-time value while this VM still owns its heap.
    pub fn call_concrete(
        &mut self,
        program: &ConcreteProgram,
        name: &str,
        args: &[CtValue],
        fuel: usize,
    ) -> Result<(Value, usize), RuntimeError> {
        let prog = &program.0;
        self.configure_lifecycle(prog);
        let index = prog
            .index_of(name)
            .ok_or_else(|| RuntimeError::UndefinedVariable(name.to_string()))?;
        let param_types = &prog.mir.functions[index].1.param_types;
        // An initializer's `out self` is its result, not an argument: it
        // enters as the skeleton a construction builds.
        let constructed = prog.constructed(name, param_types);
        let declared = &param_types[usize::from(constructed.is_some())..];
        if args.len() != declared.len() {
            return Err(RuntimeError::ArityMismatch {
                name: name.to_string(),
                expected: declared.len(),
                got: args.len(),
            });
        }
        let args = args
            .iter()
            .zip(declared)
            .map(|(value, ty)| self.thaw(prog, value, Some(ty)))
            .collect::<Result<Vec<_>, _>>()?;
        self.ctfe_fuel = Some(fuel);
        let result = match constructed {
            Some(Constructed::Struct(constructed)) => {
                self.construct_bound(prog, constructed, name, args)
            }
            Some(Constructed::Value(skeleton)) => {
                self.construct_entered(prog, skeleton, name, args)
            }
            None => self.call_function(prog, index, args),
        };
        let remaining = self.ctfe_fuel.take().unwrap_or(0);
        Ok((result?, remaining))
    }

    /// Freeze a result of the static type `ty`, when known, which
    /// `program` ran, while this VM owns its heap. Pointer leaves retain allocation contents and
    /// uninitialized slots; strings retain their text. A held struct
    /// ([`StructDef::register_field`]) freezes as the struct it is, so the
    /// compile-time value stays nominal.
    pub fn freeze(
        &self,
        program: &ConcreteProgram,
        value: Value,
        ty: Option<&Ty>,
    ) -> Result<CtValue, RuntimeError> {
        self.freeze_value(&program.0, value, ty, &mut HashSet::new())
    }

    /// Captured standard output.
    pub fn output(&self) -> String {
        self.output.clone()
    }

    /// Final top-level bindings, for the CLI `run` dump.
    pub fn bindings(&self) -> Vec<(String, Value)> {
        self.bindings.clone()
    }

    /// The slot of [`POINTER_ROOT_FRAME`] holding the heap pointer
    /// `allocation`+`offset`, interned on first use.
    pub(super) fn pointer_root(&mut self, allocation: u64, offset: i64) -> usize {
        *self
            .pointer_root_slots
            .entry((allocation, offset))
            .or_insert_with(|| {
                self.pointer_roots
                    .push(Value::Pointer { allocation, offset });
                self.pointer_roots.len() - 1
            })
    }

    /// Move `value` into compile-time memory, which is never destroyed, and
    /// return the immutable reference to it: upstream's `store_to_mem`.
    pub(super) fn store_to_comptime_memory(&mut self, value: Value) -> Value {
        self.comptime_memory.push(value);
        Value::Ref {
            frame: COMPTIME_MEMORY_FRAME,
            slot: self.comptime_memory.len() - 1,
            projection: Vec::new(),
        }
    }

    fn freeze_value(
        &self,
        prog: &Prog,
        value: Value,
        ty: Option<&Ty>,
        active: &mut HashSet<u64>,
    ) -> Result<CtValue, RuntimeError> {
        use mojito_types::types::TyKind;
        let value = prog.boxed(value, ty);
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
                    let element = match ty.map(Ty::kind) {
                        Some(TyKind::Pointer { element, .. }) => Some(element),
                        _ => None,
                    };
                    let slots = region
                        .slots
                        .iter()
                        .zip(&region.never_written)
                        .map(|(slot, never)| {
                            if *never || matches!(slot, Value::Moved) {
                                Ok(None)
                            } else {
                                self.freeze_value(prog, slot.clone(), element, active)
                                    .map(Some)
                            }
                        })
                        .collect::<Result<Vec<_>, RuntimeError>>()?;
                    active.remove(&allocation);
                    Some(slots)
                };
                Ok(CtValue::Pointer { memory, offset })
            }
            Value::Struct { name, fields } => {
                let declared = prog.structs.get(&name).map(|definition| &definition.fields);
                let field_ty = |field: &str| {
                    declared.and_then(|declared| {
                        declared
                            .iter()
                            .find(|(name, _)| name == field)
                            .map(|(_, ty)| ty)
                    })
                };
                if let [(storage, Value::Tuple(elements))] = fields.as_slice()
                    && storage == "storage"
                    && crate::crossing::is_nominal_tuple(&name)
                {
                    let types = match field_ty(storage).map(Ty::kind) {
                        Some(TyKind::Tuple(types)) => types.as_slice(),
                        _ => &[],
                    };
                    return elements
                        .iter()
                        .enumerate()
                        .map(|(at, value)| {
                            self.freeze_value(prog, value.clone(), types.get(at), active)
                        })
                        .collect::<Result<Vec<_>, _>>()
                        .map(CtValue::Tuple);
                }
                let fields = fields
                    .into_iter()
                    .map(|(field, value)| {
                        let frozen = self.freeze_value(prog, value, field_ty(&field), active)?;
                        Ok((field, frozen))
                    })
                    .collect::<Result<Vec<_>, RuntimeError>>()?;
                Ok(CtValue::Struct { name, fields })
            }
            Value::Tuple(elements) => {
                let types = match ty.map(Ty::kind) {
                    Some(TyKind::Tuple(types) | TyKind::RuntimePack(types)) => types.as_slice(),
                    _ => &[],
                };
                elements
                    .into_iter()
                    .enumerate()
                    .map(|(at, value)| self.freeze_value(prog, value, types.get(at), active))
                    .collect::<Result<Vec<_>, _>>()
                    .map(CtValue::Tuple)
            }
            Value::Variant {
                alternatives,
                index,
                value,
            } => {
                let payload = match *value {
                    Value::None => None,
                    value => Some(Box::new(self.freeze_value(
                        prog,
                        value,
                        alternatives.get(index),
                        active,
                    )?)),
                };
                Ok(CtValue::Variant {
                    alternatives,
                    index,
                    payload,
                })
            }
            Value::ComptimeList(elements) => {
                let element = match ty.map(Ty::kind) {
                    Some(TyKind::ComptimeList(element)) => Some(element),
                    _ => None,
                };
                elements
                    .into_iter()
                    .map(|value| self.freeze_value(prog, value, element, active))
                    .collect::<Result<Vec<_>, _>>()
                    .map(CtValue::List)
            }
            Value::Ref {
                frame: COMPTIME_MEMORY_FRAME,
                ..
            } => {
                let referent = self.read_reference(
                    &value,
                    FrameId(COMPTIME_MEMORY_FRAME),
                    &self.comptime_memory,
                )?;
                let referent_ty = match ty.map(Ty::kind) {
                    Some(TyKind::Ref(reference)) => Some(&reference.referent),
                    Some(TyKind::Pointer { element, .. }) => Some(element),
                    _ => None,
                };
                Ok(CtValue::StoreToMem(Box::new(self.freeze_value(
                    prog,
                    referent,
                    referent_ty,
                    active,
                )?)))
            }
            Value::Ref { .. } => Err(RuntimeError::Unsupported(
                "cannot freeze a reference to run-time storage".to_string(),
            )),
            // The VM holds an `index` as an `Int`; its type says which it is.
            Value::Int(n) if ty.is_some_and(|ty| matches!(ty.kind(), TyKind::Index)) => {
                Ok(CtValue::Index(n))
            }
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
            let function = &prog.mir.functions[top].1;
            self.bindings = function
                .var_names
                .iter()
                .zip(vars)
                .enumerate()
                .filter(|(_, (name, _))| !name.starts_with('$'))
                .map(|(slot, (name, value))| {
                    let ty = function.var_tys.get(&(slot as VarId));
                    (name.clone(), prog.boxed(value, ty))
                })
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
    /// Per function, the error registers of its raise edges: a `raise` of
    /// one propagates the error its instruction raised, as upstream's
    /// `lit.try.raise` carries no new error, so the trace records no second
    /// raise.
    edge_errors: Vec<HashSet<u32>>,
    /// Per function, the error slots of its `try`s: a store into one is a
    /// raise's, staging the error as the raise stages it
    /// ([`VmBackend::stage_error`]).
    error_slots: Vec<HashSet<VarId>>,
}

/// What an initializer's `out self` receiver enters as ([`Prog::constructed`]).
enum Constructed<'s> {
    /// A declared struct's skeleton, built by [`VmBackend::construct_bound`].
    Struct(&'s str),
    /// The unwritten storage of a receiver the VM holds as a value.
    Value(Value),
}

/// A static type with its reference layers removed: a place or parameter
/// handle describes the same instance as the value behind it.
fn peel_references(ty: &mojito_types::types::Ty) -> &mojito_types::types::Ty {
    let mut ty = ty;
    while let mojito_types::types::TyKind::Ref(reference) = ty.kind() {
        ty = &reference.referent;
    }
    ty
}

/// The struct `ty` is and the field it is held as, among `structs`.
fn held_field<'s>(structs: &'s HashMap<String, StructDef>, ty: &Ty) -> Option<(&'s str, &'s str)> {
    let mojito_types::types::TyKind::Struct(name, _) = ty.kind() else {
        return None;
    };
    let (name, declaration) = structs.get_key_value(name)?;
    Some((name.as_str(), declaration.register_field.as_deref()?))
}

/// Lower every field step whose base is a held struct to [`Proj::Rebind`]:
/// the held field is the struct's whole storage, as upstream's
/// `LowerKGENToLLVM` makes a one-element struct's `struct_gep` the
/// struct's own address.
fn hold_register_fields(structs: &HashMap<String, StructDef>, blocks: &mut [MirBlock]) {
    for block in blocks {
        for instruction in &mut block.instrs {
            for place in mojito_mir::mir::verify::instruction_places_mut(instruction) {
                for position in 0..place.proj.len() {
                    let base = match position {
                        0 => place.root_ty.as_ref(),
                        _ => place.projection_tys.get(position - 1),
                    };
                    if matches!(place.proj[position], Proj::Field(_))
                        && base.is_some_and(|base| {
                            held_field(structs, peel_references(base)).is_some()
                        })
                    {
                        place.proj[position] = Proj::Rebind;
                    }
                }
            }
            for region in mojito_mir::mir::instruction_regions_mut(instruction) {
                hold_register_fields(structs, region);
            }
        }
    }
}

impl Prog {
    fn new(mir: MirProgram) -> Self {
        let edge_errors = mir
            .functions
            .iter()
            .map(|(_, function)| {
                let mut errors = HashSet::new();
                edge_errors(&function.blocks, &mut errors);
                errors
            })
            .collect();
        let error_slots = mir
            .functions
            .iter()
            .map(|(_, function)| {
                mojito_mir::mir::nested_instructions(&function.blocks)
                    .into_iter()
                    .filter_map(|instruction| match instruction {
                        MirInstr::Try { error, .. } => *error,
                        _ => None,
                    })
                    .collect()
            })
            .collect();
        let structs = build_structs(&mir.declarations);
        let mut mir = mir;
        for (_, function) in &mut mir.functions {
            hold_register_fields(&structs, &mut function.blocks);
        }
        Self {
            structs,
            sigs: build_sigs(&mir.declarations),
            mir,
            edge_errors,
            error_slots,
        }
    }

    /// The struct a value of `ty` is and the field it is held as, when
    /// `ty` is a struct held as its one field ([`StructDef::register_field`]).
    fn held_field(&self, ty: Option<&Ty>) -> Option<(&str, &str)> {
        held_field(&self.structs, ty?)
    }

    /// `value`, a value of `ty`, as the [`Value::Struct`] the VM's
    /// runtime-shape dispatch reads, where `ty` is a held struct: the
    /// operand of a builtin, a method receiver, or a frozen result. Any
    /// other value, a boxed one included, is returned as it is.
    fn boxed(&self, value: Value, ty: Option<&Ty>) -> Value {
        match self.held_field(ty) {
            Some((name, _)) if matches!(&value, Value::Struct { name: held, .. } if held == name) => {
                value
            }
            Some((name, field)) => Value::Struct {
                name: name.to_string(),
                fields: vec![(field.to_string(), value)],
            },
            None => value,
        }
    }

    /// The struct `value`, a value of `ty`, is to runtime-shape dispatch:
    /// the held struct `ty` names, else its box's name.
    fn struct_name<'a>(&'a self, value: &'a Value, ty: Option<&Ty>) -> Option<&'a str> {
        self.held_field(ty).map(|(name, _)| name).or(match value {
            Value::Struct { name, .. } => Some(name.as_str()),
            _ => None,
        })
    }

    /// `value`, a value of `ty`, as the field it is held as where `ty` is a
    /// held struct and `value` is its box: [`Self::boxed`]'s inverse, which
    /// a frame's entry and a call's result apply.
    fn unboxed(&self, value: Value, ty: Option<&Ty>) -> Value {
        match (self.held_field(ty), value) {
            (
                Some((name, _)),
                Value::Struct {
                    name: held,
                    mut fields,
                },
            ) if held == name && fields.len() == 1 => {
                fields.pop().map_or(Value::None, |(_, value)| value)
            }
            (_, value) => value,
        }
    }

    fn index_of(&self, name: &str) -> Option<usize> {
        self.mir.functions.iter().position(|(n, _)| n == name)
    }

    /// What the `__init__` `name`, whose parameters are `param_types`,
    /// constructs: its `out self` receiver's struct, or the unwritten
    /// storage a receiver the VM holds as a value rather than a declared
    /// struct (a `SIMD`) enters as. `None` for any other function.
    fn constructed<'s>(&'s self, name: &'s str, param_types: &'s [Ty]) -> Option<Constructed<'s>> {
        let receiver = param_types.first();
        let declared = match receiver.map(Ty::kind) {
            Some(mojito_types::types::TyKind::Struct(receiver, _)) => Some(receiver.as_str()),
            _ => None,
        };
        if let Some((constructed, member)) = self.lifecycle_constructor(name, declared) {
            return (member == "__init__").then_some(Constructed::Struct(constructed));
        }
        let receiver = receiver.filter(|ty| mojito_types::types::nominal_parts(ty).is_some())?;
        let initializes = mojito_symbol::symbol::lifecycle_constructor(name)
            .is_some_and(|(_, member)| member == "__init__")
            && self
                .sigs
                .get(name)
                .is_some_and(|signature| signature.has_receiver);
        initializes
            .then(|| VmBackend::uninitialized_value(self, receiver))
            .flatten()
            .map(Constructed::Value)
    }

    /// Per argument a closure's captures fill in a call of its body
    /// `name`, whether the body may write it: a lifted body's storage
    /// when it owns a moved capture. A method's receiver, a closure's
    /// storage, leads, written when its struct lists the method among
    /// those writing `self`.
    fn capture_writes(&self, name: &str) -> Vec<bool> {
        let Some(declaration) = self.mir.declarations.function(name) else {
            return Vec::new();
        };
        let receiver = declaration.has_receiver.then(|| {
            mojito_symbol::symbol::split_method_symbol(name).is_some_and(|(owner, method)| {
                self.mir
                    .declarations
                    .structure(owner)
                    .is_some_and(|owner| owner.mut_self_methods.contains(method))
            })
        });
        receiver
            .into_iter()
            .chain(declaration.param_writes.iter().copied())
            .collect()
    }

    /// Whether the function `name` is one the compiler synthesized, whose
    /// lifecycle events the trace leaves out.
    fn synthesized(&self, name: &str) -> bool {
        self.mir
            .declarations
            .function(name)
            .is_some_and(mojito_mir::mir::MirFunctionDeclaration::is_synthesized)
    }

    /// The struct a lifecycle constructor `symbol` builds and the lifecycle
    /// name it runs, when the program declares that struct and the symbol's
    /// declaration takes a receiver: a `def` nested in a constructor is
    /// lifted under the constructor's name but builds nothing. An `__init__`
    /// or a called `__copyinit__` builds its call's result type, `constructed`, when that is a declared
    /// struct: an owner-less instance (`Box.__init__$mono`) names no struct
    /// the program declares, so the symbol's own struct is only the fallback.
    fn lifecycle_constructor<'s>(
        &self,
        symbol: &'s str,
        constructed: Option<&'s str>,
    ) -> Option<(&'s str, &'s str)> {
        let (parsed, member) = mojito_symbol::symbol::lifecycle_constructor(symbol)?;
        let struct_name = constructed
            .filter(|name| {
                matches!(member, "__init__" | "__copyinit__" | "__moveinit__")
                    && self.structs.contains_key(*name)
            })
            .unwrap_or(parsed);
        (self.structs.contains_key(struct_name)
            && self
                .sigs
                .get(symbol)
                .is_none_or(|signature| signature.has_receiver))
        .then_some((struct_name, member))
    }

    /// The declared struct a value of type `ty` is, through references.
    fn declared_struct<'t>(&self, ty: Option<&'t mojito_types::types::Ty>) -> Option<&'t str> {
        match peel_references(ty?).kind() {
            mojito_types::types::TyKind::Struct(name, _) if self.structs.contains_key(name) => {
                Some(name.as_str())
            }
            _ => None,
        }
    }

    /// The `hasher`'s `_update_with_simd` a scalar `__hash__` leaf of type
    /// `leaf` calls: the instance in the hasher's member table at the leaf's
    /// vector type.
    fn hash_leaf_update(&self, hasher: &str, leaf: &mojito_types::types::Ty) -> Option<usize> {
        let shape = mojito_types::types::simd_shape(&mojito_types::types::hash_leaf_ty(leaf));
        self.struct_member_instances(hasher, "_update_with_simd")
            .find(|&(index, _)| {
                self.mir.functions[index]
                    .1
                    .param_types
                    .get(1)
                    .and_then(|value| mojito_types::types::simd_shape(peel_references(value)))
                    == shape
            })
            .map(|(index, _)| index)
    }

    /// The instance of `receiver_type.method` in its member table whose
    /// hasher parameter is `hasher`'s struct (`String.__hash__$mono$T…AHasher…`),
    /// when exactly one is. `None` keeps by-name dispatch.
    fn hasher_method_instance(
        &self,
        receiver_type: &str,
        method: &str,
        hasher: &Value,
    ) -> Option<String> {
        let Value::Struct { name: hasher, .. } = hasher else {
            return None;
        };
        let mut instances = self
            .struct_member_instances(receiver_type, method)
            .filter(|&(index, _)| {
                matches!(
                    (self.mir.functions[index].1.param_types.get(1).map(peel_references)).map(Ty::kind),
                    Some(mojito_types::types::TyKind::Struct(name, _)) if name == hasher
                )
            });
        match (instances.next(), instances.next()) {
            (Some((_, symbol)), None) => Some(symbol.to_string()),
            _ => None,
        }
    }

    /// The function index of the member `method` the struct `name` runs for
    /// a dispatch with `argc` arguments besides the receiver, from its
    /// declaration's member table: the one instance under the key callable
    /// resolution selects among the member's overloads.
    fn struct_member(&self, name: &str, method: &str, argc: usize) -> Option<usize> {
        let candidates = mojito_symbol::symbol::MemberCandidates::new(
            name,
            self.structs
                .get(name)?
                .members
                .iter()
                .filter(|(key, _)| key.split('$').next() == Some(method))
                .filter_map(|(key, symbol)| {
                    let index = self.index_of(symbol)?;
                    Some((
                        key.as_str(),
                        symbol.as_str(),
                        self.mir.functions[index].1.n_params,
                    ))
                }),
        );
        self.index_of(candidates.resolve(method, argc)?)
    }

    /// The function index of the type-reached member `method` a value of the
    /// struct `name` runs for a call whose checker selection is `resolved`:
    /// the instance it names, else the table instance under the method
    /// spelling it selects (a `__trait_dispatch` contract's), else
    /// [`Self::struct_member`]'s.
    fn selected_struct_member(
        &self,
        name: &str,
        method: &str,
        resolved: Option<&str>,
        argc: usize,
    ) -> Option<usize> {
        let selected = resolved.and_then(|resolved| {
            self.index_of(resolved)
                .or_else(|| self.keyed_struct_member(name, resolved))
        });
        selected.or_else(|| self.struct_member(name, method, argc))
    }

    /// The function index of the one instance the struct `name`'s member
    /// table records under the method spelling of the selected symbol
    /// `selected` (`__eq__$ov$Self` for `__trait_dispatch.__eq__$ov$Self`),
    /// as a conformance's witness table answers a requirement.
    fn keyed_struct_member(&self, name: &str, selected: &str) -> Option<usize> {
        let (_, key) = mojito_symbol::symbol::split_method_symbol(selected)?;
        let mut instances = self
            .structs
            .get(name)?
            .members
            .iter()
            .filter(|(known, _)| known == key);
        match (instances.next(), instances.next()) {
            (Some((_, symbol)), None) => self.index_of(symbol),
            _ => None,
        }
    }

    /// Each instance of the member `method` in the struct `name`'s member
    /// table, its overloads included, with its function index.
    fn struct_member_instances<'p>(
        &'p self,
        name: &str,
        method: &'p str,
    ) -> impl Iterator<Item = (usize, &'p str)> + 'p {
        self.structs
            .get(name)
            .into_iter()
            .flat_map(|declaration| &declaration.members)
            .filter(move |(key, _)| key.split('$').next() == Some(method))
            .filter_map(|(_, symbol)| Some((self.index_of(symbol)?, symbol.as_str())))
    }

    /// Whether any struct owns the lifecycle member `member` (e.g.
    /// `__copyinit__`) — used to decide whether copy/move needs the
    /// lifecycle-method path at all.
    fn owns_member(&self, member: &str) -> bool {
        self.structs
            .values()
            .any(|declaration| declaration.members.iter().any(|(name, _)| name == member))
    }

    /// The function index of the lifecycle member `member` (`__deinit__`,
    /// `__copyinit__`, `__moveinit__`) the struct `name` owns, from its
    /// declaration's member table.
    pub(super) fn lifecycle_member(&self, name: &str, member: &str) -> Option<usize> {
        let (_, symbol) = self
            .structs
            .get(name)?
            .members
            .iter()
            .find(|(known, _)| known == member)?;
        self.index_of(symbol)
    }

    /// Resolve a selected method signature against the receiver's concrete
    /// runtime type. Bounded generic calls carry an abstract checker symbol;
    /// retargeting its suffix preserves overload selection even when every
    /// overload has the same positional arity (for example `**kwargs` methods).
    /// A selected instance of the receiver's own struct runs as named, one
    /// spelled for its template too: a method generator that removed its
    /// struct's parameters names no owner instance. Any
    /// other member the receiver's type reaches is the one its struct's
    /// member table names, as a conformance's witness table answers a
    /// requirement: under the selected method spelling, else by callable
    /// resolution among the member's entries. A selected instance of
    /// another owner runs as named when the table names none. `None` when
    /// nothing does: a dispatch never composes `{struct}.{method}`.
    fn runtime_method_name(
        &self,
        receiver_type: &str,
        method: &str,
        resolved: Option<&str>,
        argc: usize,
    ) -> Option<String> {
        let defined = resolved.filter(|selected| self.index_of(selected).is_some());
        if let Some(selected) = defined.filter(|selected| {
            mojito_symbol::symbol::split_method_symbol(selected).is_some_and(|(owner, _)| {
                owner == receiver_type
                    || owner == mojito_types::types::struct_template(receiver_type)
            })
        }) {
            return Some(selected.to_string());
        }
        resolved
            .and_then(|selected| self.keyed_struct_member(receiver_type, selected))
            .or_else(|| self.struct_member(receiver_type, method, argc))
            .map(|index| self.mir.functions[index].0.clone())
            .or_else(|| defined.map(str::to_string))
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

/// The adaptation a method or iterator call applies to its result: the
/// checked kind, and the `__copyinit__` the adapter's copy names.
#[derive(Clone, Copy)]
struct ResultAdapter<'a> {
    kind: mojito_checked::checked::CheckedResultAdapter,
    constructor: Option<&'a str>,
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
/// `TyKind::RuntimePack` sequence for a specialized heterogeneous collector.
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
    /// Indexes into the regular-parameter list.
    positional_only: Option<usize>,
    keyword_only: Option<usize>,
    /// Whether the callable takes a receiver (a method or a lifecycle
    /// constructor), as opposed to a free or nested `def`.
    has_receiver: bool,
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
/// keep field layout): field names + types (for `struct.create` coercion), and which
/// methods take `mut self` (so their receiver is written back).
struct StructDef {
    fields: Vec<(String, Ty)>,
    mut_self_methods: std::collections::HashSet<String>,
    /// The lifecycle members the struct owns, with their symbols.
    members: Vec<(String, String)>,
    /// The one field a value of the struct is held as
    /// ([`mojito_mir::mir::MirDeclarations::register_field`]), but for a
    /// reference or pointer field: a reference, and a pointer to a place,
    /// are a [`Value::Ref`], which a frame slot holds only as a reference
    /// alias to the VM, so a struct holding one stays boxed.
    register_field: Option<String>,
}

fn ct_value_as_runtime(value: CtValue) -> Option<Value> {
    Some(match value {
        CtValue::Int(value) | CtValue::Index(value) => Value::Int(value),
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
        value @ (CtValue::MlirAttr(_) | CtValue::KgenString(_) | CtValue::KgenTarget(_)) => {
            Value::Attribute(value)
        }
        CtValue::Struct { name, fields } => Value::Struct {
            name,
            fields: fields
                .into_iter()
                .map(|(field, value)| Some((field, ct_value_as_runtime(value)?)))
                .collect::<Option<Vec<_>>>()?,
        },
        CtValue::Variant {
            alternatives,
            index,
            payload,
        } => Value::Variant {
            value: Box::new(match payload {
                Some(payload) => {
                    crate::crossing::thawed_payload(
                        alternatives.get(index),
                        ct_value_as_runtime(*payload)?,
                    )
                }
                None => Value::None,
            }),
            alternatives,
            index,
        },
        CtValue::Pointer { .. }
        | CtValue::StoreToMem(_)
        | CtValue::Dict { .. }
        | CtValue::Set { .. }
        | CtValue::Type(_)
        | CtValue::Trait(_)
        | CtValue::Generator(_)
        // No unresolved expression or deferred slot becomes a runtime value.
        | CtValue::Expr(_)
        | CtValue::Deferred(_) | CtValue::Marker(_) | CtValue::RemovedArg => return None,
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
    cursor: RegionCursor,
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
    /// The receiver operand's static type, which names a held struct's
    /// method table ([`Prog::boxed`]).
    receiver_ty: Option<&'a Ty>,
    method: &'a str,
    resolved_name: Option<&'a str>,
    result_adapter: Option<ResultAdapter<'a>>,
    arguments: Vec<Value>,
    keyword_arguments: Vec<(String, Value)>,
    receiver_place: Option<&'a MirPlace>,
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

/// The caller place one parameter slot binds as a handle. A `mut`/`ref`
/// slot requires its retained place (`Err` when the argument retained none).
/// A read-convention slot binds a retained place too — the call's result
/// borrows that argument, so the callee must see the caller's storage rather
/// than a frame copy the returned handle would outlive — while an owned or
/// deinit slot keeps its value transfer.
fn caller_handle_place<'a>(
    function: &MirFunction,
    parameter: usize,
    place: Option<&'a MirPlace>,
) -> Result<Option<&'a MirPlace>, MissingCallerPlace> {
    let flag = |flags: &[bool]| flags.get(parameter).copied().unwrap_or(false);
    if flag(&function.ref_params) {
        return place.map(Some).ok_or(MissingCallerPlace);
    }
    Ok(place.filter(|_| !flag(&function.owned_params) && !flag(&function.deinit_params)))
}

/// A `mut`/`ref` parameter whose argument retained no caller place.
struct MissingCallerPlace;

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
    /// The raise edge of the call: a `raise` leaving the child sets its
    /// flag and error in the caller, which goes on to its error region.
    raises: Option<Raises>,
}

/// Add to `errors` the error register of every raise edge in `blocks`, at
/// any depth.
fn edge_errors(blocks: &[MirBlock], errors: &mut HashSet<u32>) {
    for block in blocks {
        for instruction in &block.instrs {
            if let Some(raises) = instruction.raise_edge() {
                errors.insert(raises.error.0);
            }
            for region in mojito_mir::mir::instruction_regions(instruction) {
                edge_errors(region, errors);
            }
        }
    }
}

/// Build the VM registry from declaration metadata carried by MIR. Every
/// instance of a template is held or none is: an erased instance
/// (`B$mono$V$REMOVED$ARG`) lays out as every instance agreeing on its
/// kept arguments, though a conditional `TrivialRegisterPassable` row it
/// cannot decide drops from its declaration.
fn build_structs(declarations: &mojito_mir::mir::MirDeclarations) -> HashMap<String, StructDef> {
    let mut structs: HashMap<String, StructDef> = declarations
        .structs
        .iter()
        .map(|declaration| {
            (
                declaration.name.clone(),
                StructDef {
                    fields: declaration.fields.clone(),
                    mut_self_methods: declaration.mut_self_methods.clone(),
                    members: declaration
                        .members
                        .iter()
                        .map(|entry| (entry.member.clone(), entry.symbol.clone()))
                        .collect(),
                    register_field: declarations
                        .register_field(&declaration.name)
                        .filter(|(_, ty)| {
                            !matches!(
                                ty.kind(),
                                mojito_types::types::TyKind::Ref(_)
                                    | mojito_types::types::TyKind::Pointer { .. }
                            )
                        })
                        .map(|(field, _)| field.clone()),
                },
            )
        })
        .collect();
    let mut held: HashMap<&str, (bool, bool)> = HashMap::new();
    for (name, declaration) in &structs {
        let agreement = held
            .entry(mojito_types::types::struct_template(name))
            .or_insert((false, false));
        if declaration.register_field.is_some() {
            agreement.0 = true;
        } else {
            agreement.1 = true;
        }
    }
    let mixed: HashSet<String> = held
        .into_iter()
        .filter(|(_, (some, none))| *some && *none)
        .map(|(template, _)| template.to_string())
        .collect();
    for (name, declaration) in &mut structs {
        if mixed.contains(mojito_types::types::struct_template(name)) {
            declaration.register_field = None;
        }
    }
    structs
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
                    positional_only: declaration.positional_only,
                    keyword_only: declaration.keyword_only,
                    has_receiver: declaration.has_receiver,
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

/// Index internal tuple-pack storage. Nominal collections route through
/// their checked `__getitem__` implementation before reaching here.
fn index_value(base: &Value, idx: i64) -> Result<Value, RuntimeError> {
    match base {
        Value::Tuple(items) => {
            let i = crate::runtime::bounds_check(idx, items.len(), "tuple index")?;
            Ok(items[i].clone())
        }
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
        Const::UninitMem => Value::UninitStorage(None),
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
        Const::FunctionInstance { function, .. } => {
            return Err(parametric_instruction(&format!(
                "the unelaborated instance of `{function}`"
            )));
        }
    })
}

mod calls;

mod exec;

mod frames;

mod references;

mod places;

mod regions;
use regions::{RegionCursor, Transfer};

#[cfg(test)]
mod held_struct_tests {
    use super::*;
    use mojito_types::types::TyKind;

    /// A program declaring `L(trait_name)` of `fields` `Int` fields, and
    /// `make() -> L` building one from sevens.
    fn program(trait_name: &str, fields: usize) -> Prog {
        let declaration = mojito_mir::mir::MirStructDeclaration {
            name: "L".into(),
            fields: (0..fields)
                .map(|at| (format!("f{at}"), Ty::int()))
                .collect(),
            mut_self_methods: HashSet::new(),
            fieldwise_init: true,
            param_decls: Vec::new(),
            explicit_destructors: HashMap::new(),
            explicit_destroy: None,
            conformances: Vec::new(),
            declared: vec![mojito_checked::checked::StructConformance {
                trait_name: trait_name.into(),
                conditions: vec![mojito_types::types::GenericConstraint::Bool(true)],
            }],
            associated_types: Vec::new(),
            parameterized_associated_types: Vec::new(),
            associated_values: Vec::new(),
            parameterized_associated_values: Vec::new(),
            witness_tables: Vec::new(),
            members: Vec::new(),
        };
        let built = Reg(fields as u32);
        let mut instrs: Vec<MirInstr> = (0..fields as u32)
            .map(|at| MirInstr::Const {
                dest: Reg(at),
                k: Const::Int(7),
            })
            .collect();
        instrs.push(MirInstr::MakeStruct {
            dest: built,
            fields: (0..fields as u32).map(Reg).collect(),
        });
        let mut reg_types: HashMap<u32, Ty> =
            (0..fields as u32).map(|at| (at, Ty::int())).collect();
        reg_types.insert(built.0, struct_ty());
        let make = MirFunction {
            blocks: vec![MirBlock {
                instrs,
                term: MirTerm::Return(Some(built)),
            }],
            n_regs: built.0 + 1,
            ret_ty: Some(struct_ty()),
            reg_types,
            ..MirFunction::default()
        };
        Prog::new(MirProgram {
            functions: vec![("make".to_string(), make)],
            declarations: mojito_mir::mir::MirDeclarations {
                structs: vec![declaration],
                ..mojito_mir::mir::MirDeclarations::default()
            },
            invariant_errors: Vec::new(),
        })
    }

    fn struct_ty() -> Ty {
        Ty::new(TyKind::Struct("L".into(), Vec::new()))
    }

    #[test]
    fn held_one_field_struct_is_its_field() {
        let prog = program("TrivialRegisterPassable", 1);
        let mut vm = VmBackend::default();
        let value = vm.call_function(&prog, 0, Vec::new()).expect("make");
        assert_eq!(value, Value::Int(7));
        let frozen = vm
            .freeze_value(&prog, value, Some(&struct_ty()), &mut HashSet::new())
            .expect("freeze");
        assert_eq!(
            frozen,
            CtValue::Struct {
                name: "L".into(),
                fields: vec![("f0".into(), CtValue::Int(7))],
            }
        );
        assert_eq!(
            vm.thaw(&prog, &frozen, Some(&struct_ty())).expect("thaw"),
            Value::Int(7)
        );
    }

    #[test]
    fn boxed_struct_stays_a_struct() {
        for (trait_name, fields) in [("TrivialRegisterPassable", 2), ("Copyable", 1)] {
            let prog = program(trait_name, fields);
            let value = VmBackend::default()
                .call_function(&prog, 0, Vec::new())
                .expect("make");
            assert!(
                matches!(&value, Value::Struct { name, fields: built } if name == "L" && built.len() == fields),
                "{trait_name}: {value:?}"
            );
        }
    }
}

#[cfg(test)]
mod pointer_storage_tests {
    use super::*;
    use mojito_types::types::TyKind;

    fn empty_program() -> Prog {
        Prog {
            mir: MirProgram {
                functions: Vec::new(),
                declarations: mojito_mir::mir::MirDeclarations::default(),
                invariant_errors: Vec::new(),
            },
            structs: HashMap::new(),
            sigs: HashMap::new(),
            edge_errors: Vec::new(),
            error_slots: Vec::new(),
        }
    }

    /// Freeze `value` with no static type, as a program with no held
    /// struct does.
    fn freeze(vm: &VmBackend, value: Value) -> Result<CtValue, RuntimeError> {
        vm.freeze_value(&empty_program(), value, None, &mut HashSet::new())
    }

    #[test]
    fn frozen_pointer_round_trip_preserves_holes_offsets_and_independence() {
        let mut vm = VmBackend::default();
        let source = vm.heap_alloc(3, 8).expect("allocation");
        let Value::Pointer { allocation, .. } = source else {
            unreachable!("heap allocator returns a pointer")
        };
        vm.heap_store(allocation as usize - 1, 1, Value::Int(7));
        let frozen = freeze(
            &vm,
            Value::Pointer {
                allocation,
                offset: 1,
            },
        )
        .expect("freeze");
        assert_eq!(
            frozen,
            CtValue::Pointer {
                memory: Some(vec![None, Some(CtValue::Int(7)), None]),
                offset: 1
            }
        );
        let ty = Ty::new(TyKind::Pointer {
            element: Ty::int(),
            origin: mojito_types::origin::PointerOrigin::Untracked { mutable: true },
        });
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
        assert_eq!(freeze(&vm, second).expect("independent allocation"), frozen);
        vm.heap_free(allocation, 0).expect("free source");
        assert!(freeze(&vm, source).is_err());
        let copy = vm
            .thaw(&empty_program(), &frozen, Some(&ty))
            .expect("survives source");
        assert_eq!(freeze(&vm, copy).expect("freeze copy"), frozen);
    }

    #[test]
    fn frozen_comptime_reference_round_trips_with_projection() {
        let mut vm = VmBackend::default();
        let pair = Value::Struct {
            name: "Pair".to_string(),
            fields: vec![
                ("a".to_string(), Value::Int(1)),
                ("b".to_string(), Value::Int(2)),
            ],
        };
        let Value::Ref { frame, slot, .. } = vm.store_to_comptime_memory(pair) else {
            unreachable!("compile-time memory returns a handle")
        };
        let field = Value::Ref {
            frame,
            slot,
            projection: vec![crate::runtime::RefProjection::Field("b".to_string())],
        };
        let frozen = freeze(&vm, field).expect("freeze");
        assert_eq!(frozen, CtValue::StoreToMem(Box::new(CtValue::Int(2))));
        let ty = Ty::new(TyKind::Ref(mojito_types::origin::RefTy {
            referent: Ty::int(),
            origin: mojito_types::origin::Origin::Static,
            mutability: mojito_types::origin::Mutability::Immutable,
        }));
        let thawed = vm.thaw(&empty_program(), &frozen, Some(&ty)).expect("thaw");
        assert!(matches!(
            thawed,
            Value::Ref { frame: COMPTIME_MEMORY_FRAME, ref projection, .. } if projection.is_empty()
        ));
        assert_eq!(
            vm.read_reference(&thawed, FrameId(0), &[]).expect("read"),
            Value::Int(2)
        );
        assert_eq!(freeze(&vm, thawed).expect("freeze again"), frozen);
    }

    #[test]
    fn freezing_a_reference_to_frame_storage_is_rejected() {
        let vm = VmBackend::default();
        let error = freeze(
            &vm,
            Value::Ref {
                frame: 0,
                slot: 0,
                projection: Vec::new(),
            },
        )
        .expect_err("a frame reference has no compile-time form");
        assert!(error.to_string().contains("run-time storage"), "{error}");
    }

    #[test]
    fn frozen_pointer_distinguishes_dangling_and_empty_and_rejects_cycles() {
        let mut vm = VmBackend::default();
        assert_eq!(
            freeze(
                &vm,
                Value::Pointer {
                    allocation: 0,
                    offset: 0
                }
            )
            .expect("dangling"),
            CtValue::Pointer {
                memory: None,
                offset: 0
            }
        );
        let empty = vm.heap_alloc(0, 8).expect("empty allocation");
        assert_eq!(
            freeze(&vm, empty).expect("empty"),
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
            freeze(&vm, cycle)
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
        vm.heap_destroy(&empty_program(), allocation, offset, 0, None, None)
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
        vm.heap_destroy(&empty_program(), allocation, offset, 0, None, None)
            .expect("destroying a never-written slot is a no-op");
        assert!(
            vm.heap_destroy(&empty_program(), allocation, offset, 0, None, None)
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
        vm.heap_destroy(&empty_program(), allocation, offset, 1, None, None)
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
            place.project(Proj::UninitPayload, Ty::int());
            place
        };
        let storage_ty = Ty::new(TyKind::Struct(
            mojito_types::types::UNINIT_STORAGE_TYPE_NAME.to_string(),
            vec![mojito_types::types::TyArg::Ty(Ty::int())],
        ));
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
