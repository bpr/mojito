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
}

impl VmBackend {
    pub fn new() -> Self {
        Self::default()
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
                "vm: UnsafePointer.alloc count must be non-negative".to_string(),
            ));
        }
        if alignment <= 0 || !(alignment as u64).is_power_of_two() {
            return Err(RuntimeError::TypeError(
                "vm: UnsafePointer allocation alignment must be a positive power of two"
                    .to_string(),
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
                "vm: dereference of dangling UnsafePointer".to_string(),
            ));
        }
        let allocation_index = usize::try_from(allocation - 1).map_err(|_| {
            RuntimeError::TypeError("vm: invalid UnsafePointer provenance".to_string())
        })?;
        let region = self.heap.get(allocation_index).ok_or_else(|| {
            RuntimeError::TypeError("vm: invalid UnsafePointer provenance".to_string())
        })?;
        if !region.live {
            return Err(RuntimeError::TypeError(
                "vm: use after UnsafePointer.free()".to_string(),
            ));
        }
        let i = base.checked_add(offset).ok_or_else(|| {
            RuntimeError::TypeError("vm: UnsafePointer offset overflow".to_string())
        })?;
        if i < 0 || i as usize >= region.slots.len() {
            return Err(RuntimeError::TypeError(
                "vm: UnsafePointer access out of bounds".to_string(),
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
            .ok_or_else(|| {
                RuntimeError::TypeError("vm: invalid UnsafePointer provenance".to_string())
            })?;
        if !region.live {
            return Err(RuntimeError::TypeError(
                "vm: double free of UnsafePointer allocation".to_string(),
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
                "vm: read of uninitialized UnsafePointer storage".to_string(),
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
                "vm: take or destroy of uninitialized UnsafePointer storage".to_string(),
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

    /// Apply a binary operator, dispatching to a user struct's **dunder** when an
    /// operand is a struct (operator overloading): `a OP b` → `a.__op__(b)` for a
    /// struct left operand; `x in c` / `x not in c` → `c.__contains__(x)` (negated
    /// for `not in`). Primitive operands go through the shared `apply_infix`.
    fn apply_binop(
        &mut self,
        prog: &Prog,
        op: crate::ast::InfixOp,
        l: Value,
        r: Value,
        resolved: Option<&str>,
    ) -> Result<Value, RuntimeError> {
        use crate::ast::InfixOp;
        match (&l, &r, op) {
            (
                Value::Pointer { allocation, offset },
                Value::IntLiteral(delta),
                InfixOp::Add | InfixOp::Sub,
            ) => {
                let delta = delta.wrapping_signed(64).ok_or_else(|| {
                    RuntimeError::TypeError(
                        "vm: UnsafePointer offset cannot materialize as Int".to_string(),
                    )
                })?;
                let offset = if op == InfixOp::Sub {
                    offset.checked_sub(delta)
                } else {
                    offset.checked_add(delta)
                }
                .ok_or_else(|| {
                    RuntimeError::TypeError("vm: UnsafePointer offset overflow".to_string())
                })?;
                return Ok(Value::Pointer {
                    allocation: *allocation,
                    offset,
                });
            }
            (
                Value::Pointer { allocation, offset },
                Value::Int(delta),
                InfixOp::Add | InfixOp::Sub,
            ) => {
                let offset = if op == InfixOp::Sub {
                    offset.checked_sub(*delta)
                } else {
                    offset.checked_add(*delta)
                }
                .ok_or_else(|| {
                    RuntimeError::TypeError("vm: UnsafePointer offset overflow".to_string())
                })?;
                return Ok(Value::Pointer {
                    allocation: *allocation,
                    offset,
                });
            }
            (
                Value::Pointer {
                    allocation: left_allocation,
                    offset: left_offset,
                },
                Value::Pointer {
                    allocation: right_allocation,
                    offset: right_offset,
                },
                InfixOp::Sub,
            ) => {
                if left_allocation != right_allocation {
                    return Err(RuntimeError::TypeError(
                        "vm: cannot subtract pointers with different provenance".to_string(),
                    ));
                }
                return Ok(Value::Int(left_offset - right_offset));
            }
            (
                Value::Pointer {
                    allocation: left_allocation,
                    offset: left_offset,
                },
                Value::Pointer {
                    allocation: right_allocation,
                    offset: right_offset,
                },
                InfixOp::Eq | InfixOp::Ne,
            ) => {
                let equal = left_allocation == right_allocation && left_offset == right_offset;
                return Ok(Value::Bool(if op == InfixOp::Eq { equal } else { !equal }));
            }
            _ => {}
        }
        if matches!(op, InfixOp::In | InfixOp::NotIn) {
            if let Value::Struct { name, .. } = &r {
                let sname = name.clone();
                let res =
                    self.call_resolved_dunder(prog, &sname, "__contains__", vec![r, l], resolved)?;
                return Ok(match (op, res) {
                    (InfixOp::NotIn, Value::Bool(b)) => Value::Bool(!b),
                    (_, v) => v,
                });
            }
        } else if let Value::Struct { name, .. } = &l
            && let Some(dunder) = op.dunder()
        {
            let sname = name.clone();
            return self.call_dunder(prog, &sname, dunder, vec![l, r]);
        }
        apply_infix(op, l, r)
    }

    /// Prefix operator dispatch. A user struct routes through its dunder
    /// (`-x` → `x.__neg__()`, `not x` → `not x.__bool__()`), mirroring
    /// `apply_binop`; scalars use the primitive `apply_prefix`.
    fn apply_prefix(
        &mut self,
        prog: &Prog,
        op: crate::ast::PrefixOp,
        value: Value,
    ) -> Result<Value, RuntimeError> {
        if let Value::Struct { name, .. } = &value {
            let sname = name.clone();
            let result = self.call_dunder(prog, &sname, op.dunder(), vec![value])?;
            return Ok(match (op, result) {
                (crate::ast::PrefixOp::Not, Value::Bool(b)) => Value::Bool(!b),
                (_, v) => v,
            });
        }
        apply_prefix(op, value)
    }

    /// `c[i] = value` where the container `c` (at `parent`) is a user struct →
    /// `c.__setitem__(i, value)`, writing the mutated `self` back to `c`'s place.
    /// The MIR has already evaluated the receiver root, index, and RHS exactly once;
    /// this clones the receiver, runs the `mut self` method, and stores its resulting
    /// `self` (frame slot 0) back — the same write-back a normal `mut self` call uses.
    fn store_index_dunder(
        &mut self,
        prog: &Prog,
        parent: &MirPlace,
        idx: Value,
        value: Value,
        regs: &[Value],
        vars: &mut [Value],
    ) -> Result<(), RuntimeError> {
        let recv = nav_mut(vars, regs, parent)?.clone();
        let Value::Struct { name, .. } = &recv else {
            return Err(RuntimeError::TypeError(
                "__setitem__ dispatch requires a struct container".to_string(),
            ));
        };
        let fname = prog.overload_name(&format!("{name}.__setitem__"), 2);
        let fidx = prog.index_of(&fname).ok_or_else(|| {
            RuntimeError::Unsupported(format!("vm: struct '{name}' has no method '__setitem__'"))
        })?;
        let (_, frame_vars) = self.call_frame(prog, fidx, vec![recv, idx, value], &[])?;
        *nav_mut(vars, regs, parent)? = frame_vars.into_iter().next().unwrap_or(Value::None);
        Ok(())
    }

    /// Store a value through any currently supported place shape. A final index
    /// into an arena pointer writes the heap; a final index into a user struct
    /// dispatches `__setitem__`; ordinary slots, fields, private pack storage,
    /// and SIMD lanes use `store_place`. Shared by MIR stores and mut-self
    /// write-back so `self.buckets[i].append(v)` follows the same path as
    /// `self.buckets[i] = row`.
    fn store_at_place(
        &mut self,
        prog: &Prog,
        place: &MirPlace,
        value: Value,
        regs: &[Value],
        vars: &mut [Value],
    ) -> Result<(), RuntimeError> {
        enum Target {
            Pointer(u64, i64, Reg),
            StructIndex(Box<MirPlace>, Reg),
            Ordinary,
        }
        let target = if let Some((Proj::Index(index), prefix)) = place.proj.split_last() {
            let parent = MirPlace {
                root: place.root,
                root_ty: place.root_ty.clone(),
                proj: prefix.to_vec(),
                projection_tys: place.projection_tys[..prefix.len()].to_vec(),
                ty: if prefix.is_empty() {
                    place.root_ty.clone()
                } else {
                    place.projection_tys.get(prefix.len() - 1).cloned()
                },
                through: place.through,
            };
            match nav_mut(vars, regs, &parent)? {
                Value::Pointer { allocation, offset } => {
                    Target::Pointer(*allocation, *offset, *index)
                }
                Value::Struct { .. } => Target::StructIndex(Box::new(parent), *index),
                _ => Target::Ordinary,
            }
        } else {
            Target::Ordinary
        };
        match target {
            Target::Pointer(allocation, base, index) => {
                let offset = value_as_index(&regs[index.0 as usize])?;
                let (region, slot) = self.heap_index(allocation, base, offset)?;
                self.heap[region].slots[slot] = value;
                Ok(())
            }
            Target::StructIndex(parent, index) => {
                let index = regs[index.0 as usize].clone();
                self.store_index_dunder(prog, &parent, index, value, regs, vars)
            }
            Target::Ordinary => store_place(vars, regs, place, value),
        }
    }

    /// Store through a caller place that may itself be rooted in a reference
    /// parameter. Intrinsic mutators receive a materialized receiver value, so
    /// they update that value and commit it through this handle instead of
    /// asking ordinary frame-place navigation to interpret `Value::Ref`.
    fn store_at_call_place(
        &mut self,
        prog: &Prog,
        frame_id: FrameId,
        place: &MirPlace,
        value: Value,
        regs: &[Value],
        vars: &mut [Value],
    ) -> Result<(), RuntimeError> {
        if !matches!(vars[place.root as usize], Value::Ref { .. }) {
            return self.store_at_place(prog, place, value, regs, vars);
        }
        let handle = Self::reference_to_place_parts(frame_id, regs, vars, place)?;
        self.write_reference(&handle, frame_id, vars, value)
    }

    /// Construct a struct via a hand-written `def __init__(out self, …)`: build an
    /// uninitialized `self` skeleton (fields = `None` placeholders, value parameters
    /// reified), run `__init__(self, args…)`, and return the initialized `self`
    /// (frame slot 0). The checker's definite-init check guarantees every field is
    /// assigned in the body, so no placeholder survives. Arguments are coerced to the
    /// `__init__` parameter types by the normal call ABI.
    fn construct_via_init(
        &mut self,
        prog: &Prog,
        name: &str,
        target: Option<&str>,
        args: Vec<Value>,
        kwargs: Vec<(String, Value)>,
        param_vals: &[Option<Value>],
    ) -> Result<Value, RuntimeError> {
        let def = &prog.structs[name];
        let fields = def
            .fields
            .iter()
            .map(|(f, _)| (f.clone(), Value::None))
            .collect();
        let value_params = reify_value_parameters(&def.param_decls, param_vals);
        let skeleton = Value::Struct {
            name: name.to_string(),
            fields,
            value_params,
        };
        let constructor = target
            .map(str::to_string)
            .unwrap_or_else(|| prog.overload_name(&format!("{name}.__init__"), args.len()));
        let fidx = prog.index_of(&constructor).ok_or_else(|| {
            RuntimeError::Unsupported(format!(
                "vm: checked constructor '{constructor}' is missing from MIR"
            ))
        })?;
        let user_args = match prog.sigs.get(&constructor) {
            Some(signature) => {
                self.bind_for_call(prog, &constructor, signature, args, kwargs)?
                    .0
            }
            None => args,
        };
        let mut bound = Vec::with_capacity(user_args.len() + 1);
        bound.push(skeleton);
        bound.extend(user_args);
        let (_, frame_vars) = self.call_frame(prog, fidx, bound, &[])?;
        Ok(frame_vars.into_iter().next().unwrap_or(Value::None))
    }

    fn construct_via_copy(
        &mut self,
        prog: &Prog,
        name: &str,
        args: Vec<Value>,
        kwargs: Vec<(String, Value)>,
        param_vals: &[Option<Value>],
    ) -> Result<Value, RuntimeError> {
        if !args.is_empty() || kwargs.len() != 1 || kwargs[0].0 != "copy" {
            return Err(RuntimeError::Unsupported(format!(
                "vm: keyword arguments to '{name}' are not supported"
            )));
        }
        let fidx = prog
            .index_of(&format!("{name}.__copyinit__"))
            .ok_or_else(|| {
                RuntimeError::Unsupported(format!("vm: struct '{name}' has no copy constructor"))
            })?;
        let def = &prog.structs[name];
        let value_params = reify_value_parameters(&def.param_decls, param_vals);
        let skeleton = self.struct_skeleton(prog, name, value_params);
        let (_, frame_vars) =
            self.call_frame(prog, fidx, vec![skeleton, kwargs[0].1.clone()], &[])?;
        Ok(frame_vars.into_iter().next().unwrap_or(Value::None))
    }

    /// Produce a semantically-correct **copy** of a value (a `UseVar { Copy }` read,
    /// a by-value argument, or a return). For a struct that defines `__copyinit__`,
    /// run it (so a pointer-owning type deep-copies its storage instead of aliasing);
    /// for a struct without one, recurse into fields (a nested field may define it).
    /// Internal tuple/compile-time storage recurses element-wise. Only reached
    /// when `has_copyinit` is set.
    fn clone_value(&mut self, prog: &Prog, v: &Value) -> Result<Value, RuntimeError> {
        match v {
            Value::Struct {
                name,
                fields,
                value_params,
            } => {
                if let Some(fidx) = prog.index_of(&format!("{name}.__copyinit__")) {
                    let skeleton = self.struct_skeleton(prog, name, value_params.clone());
                    let (_, frame_vars) =
                        self.call_frame(prog, fidx, vec![skeleton, v.clone()], &[])?;
                    Ok(frame_vars.into_iter().next().unwrap_or(Value::None))
                } else {
                    let mut new_fields = Vec::with_capacity(fields.len());
                    for (f, fv) in fields {
                        new_fields.push((f.clone(), self.clone_value(prog, fv)?));
                    }
                    Ok(Value::Struct {
                        name: name.clone(),
                        fields: new_fields,
                        value_params: value_params.clone(),
                    })
                }
            }
            Value::ComptimeList(items) => {
                let mut out = Vec::with_capacity(items.len());
                for it in items {
                    out.push(self.clone_value(prog, it)?);
                }
                Ok(Value::ComptimeList(out))
            }
            Value::Tuple(items) => {
                let mut out = Vec::with_capacity(items.len());
                for it in items {
                    out.push(self.clone_value(prog, it)?);
                }
                Ok(Value::Tuple(out))
            }
            Value::Variant {
                alternatives,
                index,
                value,
            } => Ok(Value::Variant {
                alternatives: alternatives.clone(),
                index: *index,
                value: Box::new(self.clone_value(prog, value)?),
            }),
            Value::Closure { function, captures } => {
                let mut copied = Vec::with_capacity(captures.len());
                for capture in captures {
                    copied.push(ClosureCapture {
                        value: if capture.owned {
                            self.clone_value(prog, &capture.value)?
                        } else {
                            capture.value.clone()
                        },
                        owned: capture.owned,
                    });
                }
                Ok(Value::Closure {
                    function: function.clone(),
                    captures: copied,
                })
            }
            // Scalars alias/copy trivially; a bare pointer copy *aliases* (correct —
            // deep-copy is the owning struct's `__copyinit__` job, handled above).
            other => Ok(other.clone()),
        }
    }

    /// Relocate a **moved** value (a `UseVar { Move }` / `^` transfer). For a struct
    /// that defines `__moveinit__`, run it (`existing` is consumed); otherwise the
    /// default move — the value's slot was already tombstoned — suffices. Only
    /// reached when `has_moveinit` is set.
    fn move_value(&mut self, prog: &Prog, v: Value) -> Result<Value, RuntimeError> {
        if let Value::Struct {
            name, value_params, ..
        } = &v
            && let Some(fidx) = prog.index_of(&format!("{name}.__moveinit__"))
        {
            let skeleton = self.struct_skeleton(prog, name, value_params.clone());
            let (_, frame_vars) = self.call_frame(prog, fidx, vec![skeleton, v], &[])?;
            return Ok(frame_vars.into_iter().next().unwrap_or(Value::None));
        }
        Ok(v)
    }

    /// Materialize an intrinsic result at its checked MIR type boundary. Public
    /// `Tuple` is a nominal library struct even when a primitive operation can
    /// compute its elements most conveniently in private `Value::Tuple` pack
    /// storage (currently `divmod` and `Slice.indices`). The checked destination
    /// type selects the exact concrete Tuple specialization; no runtime element
    /// guessing or source-AST reconstruction is involved.
    fn materialize_checked_result(
        &self,
        prog: &Prog,
        value: Value,
        target: Option<&Ty>,
    ) -> Result<Value, RuntimeError> {
        let Some(target @ Ty::Struct(name, _)) = target else {
            return Ok(match target {
                Some(target) => crate::runtime::coerce_checked(value, target),
                None => value,
            });
        };
        let Some(public_elements) = crate::types::tuple_elements(target) else {
            return Ok(crate::runtime::coerce_checked(value, target));
        };
        let Value::Tuple(items) = value else {
            return Ok(crate::runtime::coerce_checked(value, target));
        };
        // Ordinary generic functions are type-erased: while their body runs,
        // an intrinsic such as `divmod` can have the symbolic checked result
        // `Tuple[T, T]`. There is deliberately no nominal implementation for an
        // open type. Keep the private pack transient through that boundary; the
        // direct-call instruction in the concrete caller carries the fully
        // substituted destination type and materializes its exact generated
        // Tuple specialization below. A closed missing specialization remains a
        // compiler invariant error rather than falling back to runtime guessing.
        if !prog.structs.contains_key(name)
            && public_elements
                .iter()
                .any(|element| vm_type_is_symbolic(element))
        {
            return Ok(Value::Tuple(items));
        }
        let definition = prog.structs.get(name).ok_or_else(|| {
            RuntimeError::Unsupported(format!(
                "vm: checked public Tuple result targets missing specialization '{name}'"
            ))
        })?;
        let [(field, Ty::Tuple(storage_elements))] = definition.fields.as_slice() else {
            return Err(RuntimeError::TypeError(format!(
                "vm: public Tuple specialization '{name}' does not have one private runtime-pack field"
            )));
        };
        if field != "storage"
            || storage_elements.len() != items.len()
            || public_elements.len() != items.len()
            || !public_elements
                .iter()
                .zip(storage_elements)
                // Exact literals can survive on the expression result while
                // specialization deliberately materializes its executable
                // field (`IntLiteral` -> `Int`, for example). This is the same
                // checked, directional coercion used by MIR verification.
                .all(|(public, storage)| crate::checker::value_coerces(public, storage))
        {
            return Err(RuntimeError::TypeError(format!(
                "vm: public Tuple result does not match specialization '{name}' \
                 (public={public_elements:?}, storage={storage_elements:?}, arity={})",
                items.len()
            )));
        }
        let storage = Value::Tuple(
            items
                .into_iter()
                .zip(storage_elements)
                .map(|(item, ty)| crate::runtime::coerce_checked(item, ty))
                .collect(),
        );
        Ok(Value::Struct {
            name: name.clone(),
            fields: vec![(field.clone(), storage)],
            value_params: Vec::new(),
        })
    }

    /// Build an uninitialized `self` skeleton for `name` (fields = `None`), carrying
    /// the given reified `value_params`. Shared by `__init__`/`__copyinit__`/
    /// `__moveinit__` construction.
    fn struct_skeleton(
        &self,
        prog: &Prog,
        name: &str,
        value_params: Vec<(String, Value)>,
    ) -> Value {
        let fields = prog.structs[name]
            .fields
            .iter()
            .map(|(f, _)| (f.clone(), Value::None))
            .collect();
        Value::Struct {
            name: name.to_string(),
            fields,
            value_params,
        }
    }

    /// If `place` is `c[i]` with `c` a user struct or an `UnsafePointer`, read it via
    /// `c.__getitem__(i)` / the heap arena — the read half of `c[i] += e` on such a
    /// container (a projected `LoadPlace`). Returns `None` otherwise, so the caller
    /// uses `load_place` (a slot read or a SIMD-lane read).
    fn load_index_dunder(
        &mut self,
        prog: &Prog,
        place: &MirPlace,
        regs: &[Value],
        vars: &mut [Value],
    ) -> Result<Option<Value>, RuntimeError> {
        let Some((Proj::Index(ireg), prefix)) = place.proj.split_last() else {
            return Ok(None);
        };
        let parent = MirPlace {
            root: place.root,
            root_ty: place.root_ty.clone(),
            proj: prefix.to_vec(),
            projection_tys: place.projection_tys[..prefix.len()].to_vec(),
            ty: if prefix.is_empty() {
                place.root_ty.clone()
            } else {
                place.projection_tys.get(prefix.len() - 1).cloned()
            },
            through: place.through,
        };
        let recv = nav_mut(vars, regs, &parent)?.clone();
        match &recv {
            Value::Struct { name, .. } => {
                let sname = name.clone();
                let idx = regs[ireg.0 as usize].clone();
                Ok(Some(self.call_dunder(
                    prog,
                    &sname,
                    "__getitem__",
                    vec![recv, idx],
                )?))
            }
            Value::Pointer { allocation, offset } => {
                let off = value_as_index(&regs[ireg.0 as usize])?;
                let value = self.heap_read(*allocation, *offset, off)?;
                Ok(Some(if self.has_copyinit {
                    self.clone_value(prog, &value)?
                } else {
                    value
                }))
            }
            _ => Ok(None),
        }
    }

    /// Call a free function that has `mut`/`ref` parameters, writing each one's
    /// final value back to the caller's argument place (`arg_places`). This is the
    /// runtime half of call-scoped reference parameters, performed over the
    /// caller's frame (`regs`/`vars`).
    fn call_with_writeback(
        &mut self,
        prog: &Prog,
        call: WritebackCall<'_>,
        frame: CallerFrame<'_>,
    ) -> Result<Value, RuntimeError> {
        let WritebackCall {
            function_name: name,
            function_index: idx,
            positional_args: argv,
            keyword_args: kwargs,
            argument_places: arg_places,
            keyword_argument_places: kwarg_places,
            value_params,
        } = call;
        let CallerFrame {
            id: frame_id,
            registers: regs,
            variables: vars,
        } = frame;
        // Order the arguments into parameter slots (filling defaults/keywords),
        // keeping the slot map so each parameter's source argument is known.
        let keyword_names: Vec<String> = kwargs.iter().map(|(name, _)| name.clone()).collect();
        let (bound, slots) = match prog.sigs.get(name) {
            Some(sig) => self.bind_for_call(prog, name, sig, argv, kwargs)?,
            None => {
                let slots = (0..argv.len()).map(ArgSlot::Positional).collect();
                (argv, slots)
            }
        };
        let ref_params = prog.mir.functions[idx].1.ref_params.clone();
        let mut reference_inputs = Vec::new();
        for (i, is_ref) in ref_params.iter().enumerate() {
            if !is_ref {
                continue;
            }
            // The reference parameter's caller place: it must have been supplied by
            // a positional argument that is a simple place.
            let place = bound_argument_place(
                slots.get(i),
                prog.sigs
                    .get(name)
                    .and_then(|signature| signature.param_names.get(i))
                    .map(String::as_str),
                0,
                arg_places,
                &keyword_names,
                kwarg_places,
            );
            let place = place.ok_or_else(|| {
                RuntimeError::Unsupported(format!(
                    "vm: a mut/ref argument to '{name}' must be a plain variable or field \
                     (not a temporary, an indexed place, a default, or a forwarded keyword)"
                ))
            })?;
            reference_inputs.push((
                i,
                Self::reference_to_place_parts(frame_id, regs, vars, place)?,
            ));
        }
        let (result, _) = self.call_synchronously_with_references(
            prog,
            SynchronousCall {
                function_index: idx,
                arguments: bound,
                value_params: &value_params,
                reference_inputs: &reference_inputs,
            },
            CallerFrame {
                id: frame_id,
                registers: regs,
                variables: vars,
            },
        )?;
        Ok(result)
    }

    /// Execute a child while its caller is held by a structured MIR region rather
    /// than the continuation-driven frame stack. A temporary mirror with the
    /// caller's real frame identity makes ordinary frame/slot handles work
    /// unchanged: mutations are immediate even on raising paths, and references
    /// nested anywhere in the return value already point at the caller. Every
    /// synchronous call kind uses this one boundary.
    fn call_synchronously_with_references(
        &mut self,
        prog: &Prog,
        call: SynchronousCall<'_>,
        caller: CallerFrame<'_>,
    ) -> Result<(Value, Vec<Value>), RuntimeError> {
        let SynchronousCall {
            function_index,
            mut arguments,
            value_params,
            reference_inputs,
        } = call;
        let CallerFrame {
            id: caller_id,
            registers: caller_registers,
            variables: caller_variables,
        } = caller;
        for (parameter, handle) in reference_inputs {
            let slot = arguments.get_mut(*parameter).ok_or_else(|| {
                RuntimeError::Unsupported(format!(
                    "vm: reference parameter slot {parameter} is outside the call ABI"
                ))
            })?;
            *slot = handle.clone();
        }
        let stack_base = self.push_caller_mirror(caller_id, caller_registers, caller_variables);
        let outcome = self.call_frame(prog, function_index, arguments, value_params);
        self.restore_caller_mirror(stack_base, caller_variables)?;
        outcome
    }

    fn push_caller_mirror(
        &mut self,
        caller_id: FrameId,
        caller_registers: &[Value],
        caller_variables: &[Value],
    ) -> usize {
        let stack_base = self.frames.len();
        self.frames.push(Frame {
            id: caller_id,
            function: usize::MAX,
            registers: caller_registers.to_vec(),
            variables: caller_variables.to_vec(),
            block: 0,
            instruction: 0,
            continuation: None,
        });
        stack_base
    }

    fn restore_caller_mirror(
        &mut self,
        stack_base: usize,
        caller_variables: &mut [Value],
    ) -> Result<(), RuntimeError> {
        let mirrored = self.frames.get(stack_base).ok_or_else(|| {
            RuntimeError::Unsupported(
                "vm: synchronous call lost its mirrored caller frame".to_string(),
            )
        })?;
        caller_variables.clone_from_slice(&mirrored.variables);
        self.frames.truncate(stack_base);
        Ok(())
    }

    fn bind_for_call(
        &mut self,
        prog: &Prog,
        name: &str,
        sig: &FnSig,
        argv: Vec<Value>,
        kwargs: Vec<(String, Value)>,
    ) -> Result<(Vec<Value>, Vec<ArgSlot>), RuntimeError> {
        let mut expanded = Vec::new();
        let mut forwarded = false;
        for (name, value) in kwargs {
            if name == crate::ast::FORWARDED_KWARGS_NAME {
                if forwarded {
                    return Err(RuntimeError::TypeError(
                        "a call may forward only one StringDict".to_string(),
                    ));
                }
                forwarded = true;
                expanded.extend(self.take_forwarded_kwargs(value)?);
            } else {
                expanded.push((name, value));
            }
        }
        let kwargs = expanded;
        let collected: Vec<(String, Value)> = if sig.kw_variadic.is_some() {
            kwargs
                .iter()
                .filter(|(key, _)| !sig.param_names.contains(key))
                .cloned()
                .collect()
        } else {
            Vec::new()
        };
        let (mut bound, slots) = bind_args(name, sig, argv, kwargs)?;
        if let Some(index) = sig.kw_variadic_index {
            bound[index] = self.make_kwargs_dict(prog, collected)?;
        }
        Ok((bound, slots))
    }

    fn make_kwargs_dict(
        &mut self,
        prog: &Prog,
        entries: Vec<(String, Value)>,
    ) -> Result<Value, RuntimeError> {
        let mut dict =
            self.construct_via_init(prog, "StringDict", None, Vec::new(), Vec::new(), &[])?;
        let fname = prog.overload_name("StringDict.__setitem__", 2);
        let fidx = prog.index_of(&fname).ok_or_else(|| {
            RuntimeError::Unsupported("vm: kwargs StringDict has no __setitem__".to_string())
        })?;
        for (key, value) in entries {
            let (_, frame) =
                self.call_frame(prog, fidx, vec![dict, Value::Str(key), value], &[])?;
            dict = frame.into_iter().next().unwrap_or(Value::None);
        }
        Ok(dict)
    }

    /// Consume the self-hosted `StringDict` passed by `**kwargs^` and recover its
    /// insertion-ordered key/value entries. Its `entries` field is a self-hosted
    /// `List[DictEntry[String, V]]`, so taking the pointer slots is a true move:
    /// values are not copied and the transferred dictionary cannot be reused.
    fn take_forwarded_kwargs(
        &mut self,
        value: Value,
    ) -> Result<Vec<(String, Value)>, RuntimeError> {
        let Value::Struct { name, fields, .. } = value else {
            return Err(RuntimeError::TypeError(
                "`**kwargs^` requires a StringDict value".to_string(),
            ));
        };
        if name != "StringDict" {
            return Err(RuntimeError::TypeError(format!(
                "`**kwargs^` requires StringDict, got {name}"
            )));
        }
        let entries = fields
            .into_iter()
            .find_map(|(field, value)| (field == "entries").then_some(value))
            .ok_or_else(|| {
                RuntimeError::TypeError("StringDict has no entries storage".to_string())
            })?;
        let Value::Struct { fields, .. } = entries else {
            return Err(RuntimeError::TypeError(
                "StringDict entries storage is not a List".to_string(),
            ));
        };
        let mut data = None;
        let mut size = None;
        for (field, value) in fields {
            match (field.as_str(), value) {
                ("data", Value::Pointer { allocation, offset }) => {
                    data = Some((allocation, offset));
                }
                ("size", Value::Int(value)) => size = Some(value),
                _ => {}
            }
        }
        let (allocation, base) = data.ok_or_else(|| {
            RuntimeError::TypeError("StringDict entry List has no data pointer".to_string())
        })?;
        let size = size.ok_or_else(|| {
            RuntimeError::TypeError("StringDict entry List has no size".to_string())
        })?;
        let mut result = Vec::with_capacity(size.max(0) as usize);
        for offset in 0..size {
            let entry = self.heap_take(allocation, base, offset)?;
            let Value::Struct { fields, .. } = entry else {
                return Err(RuntimeError::TypeError(
                    "StringDict contains a non-entry value".to_string(),
                ));
            };
            let mut key = None;
            let mut value = None;
            for (field, field_value) in fields {
                match field.as_str() {
                    "key" => key = Some(field_value),
                    "value" => value = Some(field_value),
                    _ => {}
                }
            }
            let Some(Value::Str(key)) = key else {
                return Err(RuntimeError::TypeError(
                    "StringDict entry key is not a String".to_string(),
                ));
            };
            let value = value.ok_or_else(|| {
                RuntimeError::TypeError("StringDict entry has no value".to_string())
            })?;
            result.push((key, value));
        }
        Ok(result)
    }

    /// Dispatch a method call. Nominal values resolve to their mangled
    /// `Type.method` function, with a `mut self` receiver written back; only
    /// primitive and compiler-private storage kinds retain intrinsic branches.
    fn method_call(
        &mut self,
        prog: &Prog,
        invocation: MethodInvocation<'_>,
        frame: CallerFrame<'_>,
    ) -> Result<Value, RuntimeError> {
        let MethodInvocation {
            receiver: recv,
            method,
            resolved_name: resolved,
            arguments: args,
            keyword_arguments: kwargs,
            receiver_place: recv_place,
            argument_places: arg_places,
            keyword_argument_places: kwarg_places,
            parameter_arguments: param_arg_regs,
            parameter_declarations: param_decls,
        } = invocation;
        let CallerFrame {
            id: frame_id,
            registers: regs,
            variables: vars,
        } = frame;
        let keyword_names: Vec<String> = kwargs.iter().map(|(name, _)| name.clone()).collect();
        // Intrinsic dunders on a built-in numeric/hashable value; a struct with
        // its own implementation still dispatches to its method below.
        if !matches!(recv, Value::Struct { .. }) {
            match (method, args.len()) {
                // `Hashable` — `x.__hash__()`.
                ("__hash__", 0) => return self.hash_value(prog, recv).map(Value::UInt),
                // `Floorable`/`Ceilable`/`Truncable` — `x.__floor__()` etc.
                // (roadmap milestone 7).
                ("__floor__" | "__ceil__" | "__trunc__", 0) => {
                    return crate::runtime::builtin_round_dir(method, &recv);
                }
                // `CeilDivable` — `x.__ceildiv__(y)`.
                ("__ceildiv__", 1) => return crate::runtime::builtin_ceildiv(&recv, &args[0]),
                _ => {}
            }
        }
        match &recv {
            Value::UInt(state) if method == "update" && args.len() == 1 => {
                let place = recv_place.as_ref().ok_or_else(|| {
                    RuntimeError::Unsupported("vm: Hasher.update needs a mutable place".into())
                })?;
                let part = self.hash_value(prog, args[0].clone())?;
                self.store_at_call_place(
                    prog,
                    frame_id,
                    place,
                    Value::UInt(state.wrapping_mul(33).wrapping_add(part)),
                    regs,
                    vars,
                )?;
                Ok(Value::None)
            }
            Value::Str(template) if method == "format" => {
                self.format_template(prog, template, &args).map(Value::Str)
            }
            Value::Str(current) if method == "write" => {
                let place = recv_place.as_ref().ok_or_else(|| {
                    RuntimeError::Unsupported("vm: Writer.write needs a mutable place".into())
                })?;
                let mut text = current.clone();
                for argument in args {
                    text.push_str(&self.format_value(prog, argument, false)?);
                }
                self.store_at_call_place(prog, frame_id, place, Value::Str(text), regs, vars)?;
                Ok(Value::None)
            }
            Value::Tuple(_) => Err(RuntimeError::Unsupported(format!(
                "vm: internal tuple-pack storage has no runtime method '{method}'; public Tuple methods require nominal lowering"
            ))),
            Value::Slice {
                start, end, step, ..
            } if method == "indices" && args.len() == 1 => {
                let length = value_as_index(&args[0])?;
                let (start, end, step) =
                    crate::runtime::normalize_slice_bounds(length, *start, *end, *step)?;
                Ok(Value::Tuple(vec![
                    Value::Int(start),
                    Value::Int(end),
                    Value::Int(step),
                ]))
            }
            Value::Slice { kind, .. } => Err(RuntimeError::Unsupported(format!(
                "vm: {} has no method '{method}'",
                kind.type_name()
            ))),
            // `UnsafePointer` methods: `free()` releases the allocation (a no-op in
            // the arena model — the arena never reclaims).
            Value::Pointer { allocation, offset } => match method {
                "free" => {
                    self.heap_free(*allocation, *offset)?;
                    Ok(Value::None)
                }
                _ => Err(RuntimeError::Unsupported(format!(
                    "vm: UnsafePointer has no method '{method}'"
                ))),
            },
            Value::Struct { name, .. }
                if method == "write"
                    && prog.index_of(&format!("{name}.write_string")).is_some() =>
            {
                let place = recv_place.as_ref().ok_or_else(|| {
                    RuntimeError::Unsupported("vm: Writer.write needs a mutable place".into())
                })?;
                let mut writer = recv.clone();
                let index = prog
                    .index_of(&format!("{name}.write_string"))
                    .expect("guard established Writer.write_string");
                for argument in args {
                    let text = self.format_value(prog, argument, false)?;
                    let (_, variables) =
                        self.call_frame(prog, index, vec![writer, Value::Str(text)], &[])?;
                    writer = variables.into_iter().next().unwrap_or(Value::None);
                }
                self.store_at_call_place(prog, frame_id, place, writer, regs, vars)?;
                Ok(Value::None)
            }
            Value::Struct { name, .. } => {
                let method_argc = args.len();
                let source_fname = format!("{name}.{method}");
                let fname = prog.runtime_method_name(name, method, resolved, method_argc);
                let fidx = prog.index_of(&fname).ok_or_else(|| {
                    RuntimeError::Unsupported(format!("vm: unknown method '{fname}'"))
                })?;
                // Bind ordinary arguments through the same signature metadata as
                // free functions, then prepend `self` in frame slot zero.
                let (bound, slots) = match prog.sigs.get(&fname) {
                    Some(signature) => self.bind_for_call(prog, &fname, signature, args, kwargs)?,
                    None => {
                        let slots = (0..args.len()).map(ArgSlot::Positional).collect();
                        (args, slots)
                    }
                };
                let mut call_args = Vec::with_capacity(bound.len() + 1);
                call_args.push(recv.clone());
                call_args.extend(bound);
                let ref_params = &prog.mir.functions[fidx].1.ref_params;
                let mut reference_inputs = Vec::new();
                if ref_params.first().copied().unwrap_or(false) {
                    let place = recv_place.as_ref().ok_or_else(|| {
                        RuntimeError::Unsupported(format!(
                            "vm: reference receiver for method '{fname}' must be a place"
                        ))
                    })?;
                    reference_inputs.push((
                        0,
                        Self::reference_to_place_parts(frame_id, regs, vars, place)?,
                    ));
                }
                for (i, is_ref) in ref_params.iter().enumerate().skip(1) {
                    if !is_ref {
                        continue;
                    }
                    let place = bound_argument_place(
                        slots.get(i - 1),
                        prog.sigs
                            .get(&fname)
                            .and_then(|signature| signature.param_names.get(i - 1))
                            .map(String::as_str),
                        0,
                        arg_places,
                        &keyword_names,
                        kwarg_places,
                    );
                    let place = place.ok_or_else(|| {
                        RuntimeError::Unsupported(format!(
                            "vm: a mut/ref argument to method '{fname}' must be a plain \
                             variable or field (not a temporary or indexed place)"
                        ))
                    })?;
                    reference_inputs.push((
                        i,
                        Self::reference_to_place_parts(frame_id, regs, vars, place)?,
                    ));
                }
                let value_params = prog
                    .sigs
                    .get(&fname)
                    .map(|signature| {
                        let contract = if param_decls.is_empty() {
                            &signature.param_decls
                        } else {
                            param_decls
                        };
                        let supplied = runtime_parameter_arguments(contract, param_arg_regs, regs);
                        let supplied = resolve_value_parameter_slots(contract, &supplied);
                        reify_value_parameters(&signature.param_decls, &supplied)
                    })
                    .unwrap_or_default();
                let (ret, frame_vars) = self.call_synchronously_with_references(
                    prog,
                    SynchronousCall {
                        function_index: fidx,
                        arguments: call_args,
                        value_params: &value_params,
                        reference_inputs: &reference_inputs,
                    },
                    CallerFrame {
                        id: frame_id,
                        registers: regs,
                        variables: vars,
                    },
                )?;
                if prog.mir.functions[fidx].1.returns_reference && !matches!(ret, Value::Ref { .. })
                {
                    return Err(RuntimeError::TypeError(format!(
                        "vm: reference-returning method '{fname}' produced {ret:?}"
                    )));
                }
                // `mut self`: write the (possibly mutated) receiver back.
                let is_mut = prog.structs.get(name).is_some_and(|d| {
                    let key = if fname != source_fname {
                        fname.as_str()
                    } else {
                        method
                    };
                    d.mut_self_methods.contains(key)
                });
                if is_mut
                    && !ref_params.first().copied().unwrap_or(false)
                    && let Some(place) = recv_place
                {
                    self.store_at_call_place(
                        prog,
                        frame_id,
                        place,
                        frame_vars[0].clone(),
                        regs,
                        vars,
                    )?;
                }
                Ok(ret)
            }
            other => Err(RuntimeError::Unsupported(format!(
                "vm backend does not support methods on {} yet",
                crate::runtime::type_name(other)
            ))),
        }
    }

    /// Recursively destroy a value (ASAP drop): run a struct's `__del__` if it
    /// defines one, then drop its fields in reverse declaration order. Internal
    /// tuple/compile-time storage recurses through its elements. Scalars are a
    /// no-op; a destructor-less struct still recursively destroys its fields.
    fn drop_value(&mut self, prog: &Prog, v: Value) -> Result<(), RuntimeError> {
        match v {
            Value::Struct { name, fields, .. } => {
                // A partial aggregate cannot run its whole-value destructor:
                // that method may inspect a field already transferred or
                // explicitly destroyed. Drop only the initialized residual
                // fields. Checked explicit-destroy obligations guarantee that
                // an intact linear value never reaches an automatic DropVar, so
                // this rule does not need to reconstruct generic conditional
                // deletability from the erased runtime struct name.
                if fields
                    .iter()
                    .any(|(_, value)| matches!(value, Value::Moved))
                {
                    for (_, field) in fields.into_iter().rev() {
                        if !matches!(field, Value::Moved) {
                            self.drop_value(prog, field)?;
                        }
                    }
                    return Ok(());
                }
                let del = format!("{name}.__del__");
                if let Some(idx) = prog.index_of(&del) {
                    // `self` is the whole struct; the return value is discarded.
                    let self_val = Value::Struct {
                        name: name.clone(),
                        fields: fields.clone(),
                        value_params: Vec::new(),
                    };
                    self.call_function(prog, idx, vec![self_val], &[])?;
                }
                for (_, fv) in fields.into_iter().rev() {
                    self.drop_value(prog, fv)?;
                }
            }
            Value::ComptimeList(items) => {
                for item in items.into_iter().rev() {
                    self.drop_value(prog, item)?;
                }
            }
            // Private heterogeneous pack storage follows Mojo's element
            // destruction order (left-to-right). Public Tuple is the nominal
            // one-field wrapper handled by the struct branch above.
            Value::Tuple(items) => {
                for item in items {
                    self.drop_value(prog, item)?;
                }
            }
            Value::Variant { value, .. } => self.drop_value(prog, *value)?,
            Value::Closure { captures, .. } => {
                for capture in captures.into_iter().rev() {
                    if capture.owned && !matches!(capture.value, Value::Moved) {
                        self.drop_value(prog, capture.value)?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Materialize one intrinsic Slice bound through the ordinary bundled
    /// `Optional[Int]` constructor. The linker may keep Optional module-qualified,
    /// so accept its unique nominal suffix but never guess between declarations.
    fn slice_bound_optional(
        &mut self,
        prog: &Prog,
        bound: Option<i64>,
    ) -> Result<Value, RuntimeError> {
        let name = if prog.structs.contains_key("Optional") {
            "Optional".to_string()
        } else {
            let mut candidates = prog
                .structs
                .keys()
                .filter(|name| name.ends_with("$Optional"));
            let name = candidates.next().cloned().ok_or_else(|| {
                RuntimeError::Unsupported(
                    "vm: Slice bound access requires the nominal Optional declaration".to_string(),
                )
            })?;
            if candidates.next().is_some() {
                return Err(RuntimeError::Unsupported(
                    "vm: Slice bound access found ambiguous nominal Optional declarations"
                        .to_string(),
                ));
            }
            name
        };
        let arguments = bound
            .map(|value| vec![Value::Int(value), Value::Bool(true)])
            .unwrap_or_default();
        self.call_named(prog, &name, arguments, Vec::new(), &[])
    }

    /// Normalize an `Indexer` to the VM's signed index representation. Int-like
    /// values take the intrinsic path; user conformers execute
    /// `__mlir_index__`, which is the source-level contract even though MIR
    /// represents its result as an `Int` rather than an MLIR index type.
    fn normalize_index(&mut self, prog: &Prog, value: &Value) -> Result<i64, RuntimeError> {
        if let Value::Struct { name, .. } = value {
            let normalized = self.call_dunder(prog, name, "__mlir_index__", vec![value.clone()])?;
            value_as_index(&normalized)
        } else {
            value_as_index(value)
        }
    }

    /// Dispatch a call by name: a built-in intrinsic, a struct constructor, or a
    /// user function (with default/keyword/`*args` slot-matching). `param_vals`
    /// holds the supplied compile-time value-parameter arguments (`Name[...](...)`),
    /// used to reify a constructed struct's `value_params`.
    fn call_named(
        &mut self,
        prog: &Prog,
        name: &str,
        args: Vec<Value>,
        kwargs: Vec<(String, Value)>,
        param_vals: &[Option<Value>],
    ) -> Result<Value, RuntimeError> {
        // Built-ins take positional arguments only, and user functions handle
        // keywords through their signatures below. Struct constructors get a
        // narrow exception for Mojo's lifecycle copy constructor (`copy:`).
        if !kwargs.is_empty() && !prog.sigs.contains_key(name) && !prog.structs.contains_key(name) {
            return Err(RuntimeError::Unsupported(format!(
                "vm: keyword arguments to '{name}' are not supported"
            )));
        }
        if let Some(struct_name) = crate::symbol::init_overload_struct(name)
            && prog.structs.contains_key(struct_name)
        {
            return self.construct_via_init(
                prog,
                struct_name,
                Some(name),
                args,
                kwargs,
                param_vals,
            );
        }
        match name {
            // Unlinked VM-CTFE programs have the checker-known `range` builtin
            // but no nominal std.range declarations. Materialize its finite
            // compile-time sequence explicitly; GetIter/HasNext/Next reserve
            // their method-free path for this `ComptimeList` representation.
            // Normal linked execution resolves the source `range` overloads and
            // never reaches this branch.
            "range" if self.ctfe_fuel.is_some() => {
                if !(1..=3).contains(&args.len()) {
                    return Err(RuntimeError::ArityMismatch {
                        name: name.to_string(),
                        expected: if args.is_empty() { 1 } else { 3 },
                        got: args.len(),
                    });
                }
                let bounds = args
                    .iter()
                    .map(value_as_index)
                    .collect::<Result<Vec<_>, _>>()?;
                let (mut current, stop, step) = match bounds.as_slice() {
                    [stop] => (0, *stop, 1),
                    [start, stop] => (*start, *stop, 1),
                    [start, stop, step] => (*start, *stop, *step),
                    _ => unreachable!("range arity checked above"),
                };
                if step == 0 {
                    return Err(RuntimeError::TypeError(
                        "range() step argument must not be zero".to_string(),
                    ));
                }
                let limit = self.ctfe_fuel.unwrap_or(0);
                let mut values = Vec::new();
                while (step > 0 && current < stop) || (step < 0 && current > stop) {
                    if values.len() >= limit {
                        return Err(RuntimeError::Unsupported(
                            "compile-time execution exceeded the VM CTFE fuel quota".to_string(),
                        ));
                    }
                    values.push(Value::Int(current));
                    current = current.checked_add(step).ok_or_else(|| {
                        RuntimeError::TypeError("range() iteration overflowed Int".to_string())
                    })?;
                }
                Ok(Value::ComptimeList(values))
            }
            "print" => {
                let mut cells = Vec::with_capacity(args.len());
                for value in args {
                    cells.push(self.format_value(prog, value, false)?);
                }
                self.output.push_str(&cells.join(" "));
                self.output.push('\n');
                Ok(Value::None)
            }
            "String" => Ok(Value::Str(match args.into_iter().next() {
                Some(value) => self.format_value(prog, value, false)?,
                None => String::new(),
            })),
            "repr" => match args.into_iter().next() {
                Some(value) => Ok(Value::Str(self.format_value(prog, value, true)?)),
                None => Err(RuntimeError::ArityMismatch {
                    name: "repr".to_string(),
                    expected: 1,
                    got: 0,
                }),
            },
            "hash" => match args.into_iter().next() {
                Some(value) => Ok(Value::UInt(self.hash_value(prog, value)?)),
                None => Err(RuntimeError::ArityMismatch {
                    name: "hash".to_string(),
                    expected: 1,
                    got: 0,
                }),
            },
            // `len(c)` on a user struct dispatches to `c.__len__()`.
            "len" => match args.into_iter().next() {
                Some(Value::Str(s)) => Ok(Value::Int(s.len() as i64)),
                Some(Value::ComptimeList(items)) if self.ctfe_fuel.is_some() => {
                    Ok(Value::Int(items.len() as i64))
                }
                Some(Value::Tuple(items)) => Ok(Value::Int(items.len() as i64)),
                Some(Value::Struct {
                    name,
                    fields,
                    value_params,
                }) => {
                    let recv = Value::Struct {
                        name: name.clone(),
                        fields,
                        value_params,
                    };
                    self.call_dunder(prog, &name, "__len__", vec![recv])
                }
                _ => Err(RuntimeError::Unsupported(
                    "vm: len supports String, internal Tuple storage, and nominal structs with __len__"
                        .into(),
                )),
            },
            "Slice" | "slice" => {
                let optional = |value: &Value| match value {
                    Value::Int(value) => Ok(Some(*value)),
                    Value::IntLiteral(value) => {
                        value.wrapping_signed(64).map(Some).ok_or_else(|| {
                            RuntimeError::TypeError(
                                "slice bound cannot materialize as Int".to_string(),
                            )
                        })
                    }
                    Value::None => Ok(None),
                    other => Err(RuntimeError::TypeError(format!(
                        "slice bound must be Int or None, got {}",
                        crate::runtime::type_name(other)
                    ))),
                };
                let (start, end, step) = match (name, args.as_slice()) {
                    ("slice", [end]) => (None, optional(end)?, None),
                    ("slice" | "Slice", [start, end]) => (optional(start)?, optional(end)?, None),
                    ("slice" | "Slice", [start, end, step]) => {
                        (optional(start)?, optional(end)?, optional(step)?)
                    }
                    _ => {
                        return Err(RuntimeError::ArityMismatch {
                            name: name.to_string(),
                            expected: if name == "Slice" { 2 } else { 1 },
                            got: args.len(),
                        });
                    }
                };
                Ok(Value::Slice {
                    kind: crate::types::SliceKind::Slice,
                    start,
                    end,
                    step,
                })
            }
            // Utility numeric built-ins use the shared runtime value helpers; a
            // struct operand routes through the same dunder the checker resolved.
            "abs" => {
                let value = arg1(name, args)?;
                if let Value::Struct { name: sname, .. } = &value {
                    let sname = sname.clone();
                    return self.call_dunder(prog, &sname, "__abs__", vec![value]);
                }
                builtin_abs(value)
            }
            "min" => {
                let (a, b) = arg2(name, args)?;
                builtin_min_max(true, a, b)
            }
            "max" => {
                let (a, b) = arg2(name, args)?;
                builtin_min_max(false, a, b)
            }
            "round" => {
                let value = arg1(name, args)?;
                if let Value::Struct { name: sname, .. } = &value {
                    let sname = sname.clone();
                    return self.call_dunder(prog, &sname, "__round__", vec![value]);
                }
                builtin_round(value)
            }
            "input" => builtin_input(arg1(name, args)?),
            "Int" | "Float64" | "Bool" => {
                let value = arg1(name, args)?;
                if let Value::Struct { name: sname, .. } = &value {
                    let dunder = match name {
                        "Float64" => "__float__",
                        "Bool" => "__bool__",
                        _ => "__int__",
                    };
                    let sname = sname.clone();
                    return self.call_dunder(prog, &sname, dunder, vec![value]);
                }
                builtin_convert(name, value)
            }
            "Scalar" | "UInt" => builtin_convert(name, arg1(name, args)?),
            "divmod" => {
                let (a, b) = arg2(name, args)?;
                builtin_divmod(a, b)
            }
            "Error" => builtin_error(arg1(name, args)?),
            // A struct constructor. A hand-written `def __init__(out self, …)`
            // takes precedence over the fieldwise constructor: build an uninitialized
            // `self` skeleton, run `__init__`, and return the initialized value.
            _ if prog.structs.contains_key(name) => {
                let init_name = format!("{name}.__init__");
                if (!args.is_empty() || kwargs.len() != 1 || kwargs[0].0 != "copy")
                    && (prog.index_of(&init_name).is_some()
                        || prog
                            .index_of(&prog.overload_name(&init_name, args.len()))
                            .is_some())
                {
                    return self.construct_via_init(prog, name, None, args, kwargs, param_vals);
                }
                if !kwargs.is_empty() {
                    self.construct_via_copy(prog, name, args, kwargs, param_vals)
                } else if prog
                    .index_of(&prog.overload_name(&init_name, args.len()))
                    .is_some()
                {
                    self.construct_via_init(prog, name, None, args, Vec::new(), param_vals)
                } else {
                    construct(&prog.structs[name], name, args, param_vals)
                }
            }
            // `UnsafePointer[T].alloc(n)` — reserve `n` slots in the heap arena and
            // return a pointer to the base (the element type is erased).
            "UnsafePointer.alloc" => {
                let n = crate::runtime::value_as_index(&arg1(name, args)?)?;
                self.heap_alloc(n, std::mem::align_of::<Value>() as i64)
            }
            "UnsafePointer.alloc_aligned" => {
                if args.len() != 2 {
                    return Err(RuntimeError::ArityMismatch {
                        name: name.to_string(),
                        expected: 2,
                        got: args.len(),
                    });
                }
                let n = crate::runtime::value_as_index(&args[0])?;
                let alignment = crate::runtime::value_as_index(&args[1])?;
                self.heap_alloc(n, alignment)
            }
            "UnsafePointer.dangling" => {
                if !args.is_empty() {
                    return Err(RuntimeError::ArityMismatch {
                        name: name.to_string(),
                        expected: 0,
                        got: args.len(),
                    });
                }
                Ok(Value::Pointer {
                    allocation: 0,
                    offset: 0,
                })
            }
            _ => match prog.index_of(name) {
                Some(idx) => {
                    // Match positional + keyword args to the parameter slots (fill
                    // defaults, collect `*args`) when a signature is known; else a
                    // plain positional call.
                    let bound = match prog.sigs.get(name) {
                        Some(sig) => self.bind_for_call(prog, name, sig, args, kwargs)?.0,
                        None => args,
                    };
                    // Reify the function's value parameters (`doubled[21]()`): pair
                    // each declared value parameter with its supplied comptime arg.
                    let value_params: Vec<(String, Value)> = match prog.sigs.get(name) {
                        Some(sig) => reify_value_parameters(&sig.param_decls, param_vals),
                        None => Vec::new(),
                    };
                    self.call_function(prog, idx, bound, &value_params)
                }
                None => Err(RuntimeError::Unsupported(format!(
                    "vm backend does not support the built-in or callee '{name}' yet"
                ))),
            },
        }
    }

    fn format_value(
        &mut self,
        prog: &Prog,
        value: Value,
        repr: bool,
    ) -> Result<String, RuntimeError> {
        if let Value::Variant { value, .. } = value {
            return self.format_value(prog, *value, repr);
        }
        let Value::Struct {
            name,
            fields,
            value_params,
        } = value
        else {
            return Ok(value.to_string());
        };
        let method = if repr { "write_repr_to" } else { "write_to" };
        let source = format!("{name}.{method}");
        if let Some(index) = prog.index_of(&prog.overload_name(&source, 1)) {
            let receiver = Value::Struct {
                name,
                fields,
                value_params,
            };
            let (_, variables) =
                self.call_frame(prog, index, vec![receiver, Value::Str(String::new())], &[])?;
            return match variables.get(1) {
                Some(Value::Str(text)) => Ok(text.clone()),
                other => Err(RuntimeError::TypeError(format!(
                    "{source} did not leave a Writer value, got {}",
                    other
                        .map(crate::runtime::type_name)
                        .unwrap_or_else(|| "missing".to_string())
                ))),
            };
        }
        let mut cells = Vec::with_capacity(fields.len());
        for (field, value) in fields {
            cells.push(format!("{field}={}", self.format_value(prog, value, repr)?));
        }
        Ok(format!("{name}({})", cells.join(", ")))
    }

    fn format_template(
        &mut self,
        prog: &Prog,
        template: &str,
        arguments: &[Value],
    ) -> Result<String, RuntimeError> {
        let chars: Vec<char> = template.chars().collect();
        let mut output = String::new();
        let mut automatic = 0usize;
        let mut cursor = 0usize;
        while cursor < chars.len() {
            if chars[cursor] == '{' {
                if chars.get(cursor + 1) == Some(&'{') {
                    output.push('{');
                    cursor += 2;
                    continue;
                }
                let Some(end_offset) = chars[cursor + 1..].iter().position(|ch| *ch == '}') else {
                    return Err(RuntimeError::TypeError("unclosed format field".to_string()));
                };
                let end = cursor + 1 + end_offset;
                let field: String = chars[cursor + 1..end].iter().collect();
                let repr = field.contains("!r");
                let spec = field.split_once(':').map(|(_, spec)| spec).unwrap_or("");
                let selector = field.split(['!', ':']).next().unwrap_or_default();
                let index = if selector.is_empty() {
                    let index = automatic;
                    automatic += 1;
                    index
                } else {
                    selector.parse::<usize>().map_err(|_| {
                        RuntimeError::TypeError(format!("invalid format field '{{{field}}}'"))
                    })?
                };
                let value = arguments
                    .get(index)
                    .ok_or_else(|| RuntimeError::ArityMismatch {
                        name: "String.format".to_string(),
                        expected: index + 1,
                        got: arguments.len(),
                    })?;
                let rendered = self.format_value(prog, value.clone(), repr)?;
                output.push_str(&apply_format_spec(value, &rendered, spec)?);
                cursor = end + 1;
                continue;
            }
            if chars[cursor] == '}' && chars.get(cursor + 1) == Some(&'}') {
                output.push('}');
                cursor += 2;
                continue;
            }
            output.push(chars[cursor]);
            cursor += 1;
        }
        Ok(output)
    }

    fn hash_value(&mut self, prog: &Prog, value: Value) -> Result<u64, RuntimeError> {
        if let Value::Variant { index, value, .. } = value {
            // Include both discriminant and payload. Equal payload bytes in
            // different alternatives must not collapse to the same Variant hash.
            let tag = crate::runtime::builtin_hash(&Value::UInt(index as u64))?;
            return Ok(5381u64
                .wrapping_mul(33)
                .wrapping_add(tag)
                .wrapping_mul(33)
                .wrapping_add(self.hash_value(prog, *value)?));
        }
        let Value::Struct {
            name,
            fields,
            value_params,
        } = value
        else {
            return crate::runtime::builtin_hash(&value);
        };
        let source = format!("{name}.__hash__");
        if let Some(index) = prog.index_of(&prog.overload_name(&source, 1)) {
            let receiver = Value::Struct {
                name,
                fields,
                value_params,
            };
            let (_, variables) =
                self.call_frame(prog, index, vec![receiver, Value::UInt(5381)], &[])?;
            return match variables.get(1) {
                Some(Value::UInt(hash)) => Ok(*hash),
                other => Err(RuntimeError::TypeError(format!(
                    "{source} did not leave a Hasher value, got {}",
                    other
                        .map(crate::runtime::type_name)
                        .unwrap_or_else(|| "missing".to_string())
                ))),
            };
        }
        let mut state = 5381u64;
        for (_, field) in fields {
            state = state
                .wrapping_mul(33)
                .wrapping_add(self.hash_value(prog, field)?);
        }
        Ok(state)
    }
}

impl VmBackend {
    /// Run a checked program, entering through `main()` when present.
    pub fn run(&mut self, program: &crate::checked::CheckedProgram) -> Result<(), RuntimeError> {
        self.run_prog(build_prog_checked(program)?)
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
        if self.index_of(name).is_some() {
            return name.to_string();
        }
        let expected_params = if name.contains('.') { argc + 1 } else { argc };
        let mut matches = self
            .mir
            .functions
            .iter()
            .filter(|(fname, f)| {
                crate::symbol::is_overload_of(fname, name) && f.n_params == expected_params
            })
            .map(|(fname, _)| fname.clone())
            .collect::<Vec<_>>();
        if matches.len() == 1 {
            matches.remove(0)
        } else {
            name.to_string()
        }
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
        if let Some(selected) = resolved {
            if let Some(retargeted) = crate::symbol::retarget_method_symbol(selected, receiver_type)
                && self.index_of(&retargeted).is_some()
            {
                return retargeted;
            }
            if self.index_of(selected).is_some() {
                return selected.to_string();
            }
        }
        self.overload_name(&format!("{receiver_type}.{method}"), argc)
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
        Ty::Int
        | Ty::UInt
        | Ty::Bool
        | Ty::String
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
    /// Const-evaluated default per regular parameter (`None` = no default, or a
    /// non-constant default the VM can't fold — using such a slot errors).
    defaults: Vec<Option<Value>>,
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
                return None;
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
        CtValue::Type(_) | CtValue::Reflected(_) | CtValue::Param(_) => return None,
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
        CtValue::Int(_)
        | CtValue::UInt(_)
        | CtValue::Float(_)
        | CtValue::IntLiteral(_)
        | CtValue::FloatLiteral(_)
        | CtValue::Bool(_)
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
    let mut mir =
        crate::analysis::elaborate_drops_program(crate::mir::lower_checked_program(checked));
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
        // use, so a struct's `__del__` runs there (Stage 7). A no-op for values
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
                    defaults: declaration
                        .defaults
                        .iter()
                        .map(|default| default.as_ref().map(checked_const_value))
                        .collect(),
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
                _ => {
                    return Err(RuntimeError::TypeError(
                        "mutable index reference did not cross a nominal collection's pointer \
                         storage or private runtime pack"
                            .to_string(),
                    ));
                }
            },
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
}
