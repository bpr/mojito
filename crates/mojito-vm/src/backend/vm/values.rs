//! Value construction and mutation: operators, place stores,
//! constructors, strings, cloning, and moves.

use super::*;

impl VmBackend {
    /// Apply a binary operator, dispatching to a user struct's **dunder** when an
    /// operand is a struct (operator overloading): `a OP b` → `a.__op__(b)` for a
    /// struct left operand; `x in c` / `x not in c` → `c.__contains__(x)` (negated
    /// for `not in`). Primitive operands go through the shared `apply_infix`.
    pub(super) fn apply_binop(
        &mut self,
        prog: &Prog,
        op: mojito_ast::ast::InfixOp,
        l: Value,
        r: Value,
        resolved: Option<&str>,
    ) -> Result<Value, RuntimeError> {
        use mojito_ast::ast::InfixOp;
        match (&l, &r, op) {
            (
                Value::Pointer { allocation, offset },
                Value::IntLiteral(delta),
                InfixOp::Add | InfixOp::Sub,
            ) => {
                let delta = delta.wrapping_signed(64).ok_or_else(|| {
                    RuntimeError::TypeError(
                        "vm: Pointer offset cannot materialize as Int".to_string(),
                    )
                })?;
                let offset = if op == InfixOp::Sub {
                    offset.checked_sub(delta)
                } else {
                    offset.checked_add(delta)
                }
                .ok_or_else(|| {
                    RuntimeError::TypeError("vm: Pointer offset overflow".to_string())
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
                    RuntimeError::TypeError("vm: Pointer offset overflow".to_string())
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
            // An overloaded dunder carries the checker-selected symbol (the
            // overload key of `StringSpan.__eq__(rhs: String)`), retargeted
            // to the runtime receiver type like a method call; otherwise
            // arity selects.
            let sname = name.clone();
            let fname = prog.runtime_method_name(&sname, dunder, resolved, 1);
            return self.call_resolved_dunder(prog, &sname, dunder, vec![l, r], Some(&fname));
        }
        apply_infix(op, l, r)
    }

    /// Prefix operator dispatch. A user struct routes through its dunder
    /// (`-x` → `x.__neg__()`, `not x` → `not x.__bool__()`), mirroring
    /// `apply_binop`; scalars use the primitive `apply_prefix`.
    pub(super) fn apply_prefix(
        &mut self,
        prog: &Prog,
        op: mojito_ast::ast::PrefixOp,
        value: Value,
    ) -> Result<Value, RuntimeError> {
        if let Value::Struct { name, .. } = &value {
            let sname = name.clone();
            let result = self.call_dunder(prog, &sname, op.dunder(), vec![value])?;
            return Ok(match (op, result) {
                (mojito_ast::ast::PrefixOp::Not, Value::Bool(b)) => Value::Bool(!b),
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
    pub(super) fn store_index_dunder(
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
    pub(super) fn store_at_place(
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

    /// Store through a caller place that may itself be rooted in — or cross —
    /// a reference handle. Intrinsic mutators and `mut self` write-backs
    /// receive a materialized receiver value, so they update that value and
    /// commit it through the handle instead of asking ordinary frame-place
    /// navigation to interpret `Value::Ref`: a handle at the root, stored in a
    /// `ref`-typed field along the place, or filling the final slot all
    /// re-root the write at the storage the handle designates.
    pub(super) fn store_at_call_place(
        &mut self,
        prog: &Prog,
        frame_id: FrameId,
        place: &MirPlace,
        value: Value,
        regs: &[Value],
        vars: &mut [Value],
    ) -> Result<(), RuntimeError> {
        let handle = Self::reference_to_place_parts(frame_id, regs, vars, place)?;
        let Value::Ref { projection, .. } = &handle else {
            unreachable!("reference_to_place_parts always returns a handle");
        };
        let crosses_reference = matches!(vars[place.root as usize], Value::Ref { .. }) || {
            let mut current = &vars[place.root as usize];
            let mut found = false;
            for segment in projection {
                match references::navigate_reference_step(current, segment) {
                    Some(Value::Ref { .. }) => {
                        found = true;
                        break;
                    }
                    Some(next) => current = next,
                    None => break,
                }
            }
            found
        };
        if !crosses_reference {
            return self.store_at_place(prog, place, value, regs, vars);
        }
        self.write_reference(&handle, frame_id, vars, value)
    }

    /// Construct a struct via a hand-written `def __init__(out self, …)`: build an
    /// uninitialized `self` skeleton (fields = `None` placeholders, value parameters
    /// reified), run `__init__(self, args…)`, and return the initialized `self`
    /// (frame slot 0). The checker's definite-init check guarantees every field is
    /// assigned in the body, so no placeholder survives. Arguments are coerced to the
    /// `__init__` parameter types by the normal call ABI.
    pub(super) fn construct_via_init(
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
        let mut value_params = reify_value_parameters(&def.param_decls, param_vals);
        // A same-type lifecycle constructor (`copy:` / `deinit move:`) always
        // produces its argument's exact type; when the call site supplied no
        // parameter arguments (a generic template body cannot), inherit the
        // reified parameters from the source value.
        if value_params
            .iter()
            .all(|(_, value)| matches!(value, Value::None))
            && let Some(source) = args
                .iter()
                .chain(kwargs.iter().map(|(_, value)| value))
                .find_map(|value| match value {
                    Value::Struct {
                        name: source_name,
                        value_params,
                        ..
                    } if source_name == name && !value_params.is_empty() => Some(value_params),
                    _ => None,
                })
        {
            value_params = source.clone();
        }
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
        // Literal-to-struct bridge: the nominal stdlib String's literal
        // constructor never executes its body — the byte buffer is filled
        // from the literal's UTF-8 bytes here instead.
        if mojito_symbol::symbol::is_stdlib_string_struct(name)
            && let [Value::Str(literal)] = user_args.as_slice()
        {
            let literal = literal.clone();
            return self.materialize_string_struct(skeleton, &literal);
        }
        let mut bound = Vec::with_capacity(user_args.len() + 1);
        bound.push(skeleton);
        bound.extend(user_args);
        let (_, frame_vars) = self.call_frame(prog, fidx, bound, &[])?;
        Ok(frame_vars.into_iter().next().unwrap_or(Value::None))
    }

    /// `input()` under [`Self::set_input_override`]: append the prompt to the
    /// captured output (a native executable writes prompts to stdout, which the
    /// differential compares byte-for-byte), then serve one line from the
    /// override buffer — trailing `\n` then `\r` stripped, EOF → `""`.
    pub(super) fn input_from_override(&mut self, prompt: Value) -> Result<Value, RuntimeError> {
        let Value::Str(prompt) = prompt else {
            return Err(RuntimeError::TypeError(format!(
                "input() expects a String prompt, got {}",
                crate::runtime::type_name(&prompt)
            )));
        };
        self.output.push_str(&prompt);
        let cursor = self
            .input_override
            .as_mut()
            .expect("input_from_override requires an installed override");
        let mut line = String::new();
        std::io::BufRead::read_line(cursor, &mut line).map_err(|e| {
            RuntimeError::Unsupported(format!("input(): failed to read stdin: {e}"))
        })?;
        if line.ends_with('\n') {
            line.pop();
            if line.ends_with('\r') {
                line.pop();
            }
        }
        Ok(Value::Str(line))
    }

    /// Read a nominal `String` value's byte buffer back into a builtin
    /// string value — the reverse of [`Self::materialize_string_struct`].
    pub(super) fn string_struct_literal(&self, value: &Value) -> Result<Value, RuntimeError> {
        let Value::Struct { fields, .. } = value else {
            unreachable!("string bridge receiver is a struct");
        };
        let field = |name: &str| {
            fields
                .iter()
                .find(|(field, _)| field == name)
                .map(|(_, value)| value)
        };
        let (Some(Value::Pointer { allocation, offset }), Some(Value::Int(size))) =
            (field("data"), field("size"))
        else {
            return Err(RuntimeError::TypeError(
                "vm: nominal String value is missing its byte buffer".to_string(),
            ));
        };
        let mut bytes = Vec::with_capacity(*size as usize);
        for index in 0..*size {
            let (arena, slot) = self.heap_index(*allocation, *offset, index)?;
            match self.heap[arena].slots.get(slot) {
                Some(Value::Simd {
                    lanes: crate::runtime::SimdLanes::Int(lanes),
                    ..
                }) if lanes.len() == 1 => bytes.push(lanes[0] as u8),
                other => {
                    return Err(RuntimeError::TypeError(format!(
                        "vm: nominal String buffer slot is {other:?}, not a byte"
                    )));
                }
            }
        }
        // Lossy, matching the builtin literal slice: byte-wise slicing may
        // leave a split multibyte sequence in the buffer.
        Ok(Value::Str(
            std::string::String::from_utf8_lossy(&bytes).into_owned(),
        ))
    }

    /// Materialize a builtin string as a nominal stdlib `String` value,
    /// falling back to the literal value when the struct is not linked (a
    /// seam program without the prelude).
    pub(super) fn nominal_string_value(
        &mut self,
        prog: &Prog,
        text: &str,
    ) -> Result<Value, RuntimeError> {
        let Some(def) = prog
            .structs
            .get(mojito_symbol::symbol::STDLIB_STRING_STRUCT)
        else {
            return Ok(Value::Str(text.to_string()));
        };
        let fields = def
            .fields
            .iter()
            .map(|(field, _)| (field.clone(), Value::None))
            .collect();
        let skeleton = Value::Struct {
            name: mojito_symbol::symbol::STDLIB_STRING_STRUCT.to_string(),
            fields,
            value_params: Vec::new(),
        };
        self.materialize_string_struct(skeleton, text)
    }

    /// Fill a nominal `String` skeleton from a literal's UTF-8 bytes: one
    /// heap allocation holding width-1 `UInt8` scalars, sized exactly.
    pub(super) fn materialize_string_struct(
        &mut self,
        skeleton: Value,
        literal: &str,
    ) -> Result<Value, RuntimeError> {
        let bytes = literal.as_bytes();
        let pointer = self.heap_alloc(bytes.len() as i64, 1)?;
        let Value::Pointer { allocation, .. } = pointer else {
            unreachable!("heap_alloc returns a pointer");
        };
        {
            let slots = &mut self.heap[(allocation - 1) as usize].slots;
            for (index, byte) in bytes.iter().enumerate() {
                slots[index] = Value::Simd {
                    dtype: mojito_ast::ast::Dtype::UInt8,
                    lanes: crate::runtime::SimdLanes::Int(vec![i128::from(*byte)]),
                };
            }
        }
        let Value::Struct {
            name,
            mut fields,
            value_params,
        } = skeleton
        else {
            unreachable!("string construction starts from a struct skeleton");
        };
        for (field, slot) in &mut fields {
            *slot = match field.as_str() {
                "data" => Value::Pointer {
                    allocation,
                    offset: 0,
                },
                "size" => Value::Int(bytes.len() as i64),
                "cap" => Value::Int(bytes.len() as i64),
                other => unreachable!("unexpected String field '{other}'"),
            };
        }
        Ok(Value::Struct {
            name,
            fields,
            value_params,
        })
    }

    /// Invoke a callable *value* (a plain function or a closure) with owned
    /// positional arguments — the narrow synchronous channel used by the
    /// Variant owning operations (`set(init_with=…)`, `deinit_with`), whose
    /// handlers were checked to be non-raising with owned parameters only.
    /// Nominal callable structs are not accepted here: their `__call__`
    /// dispatch needs the full indirect-call contract.
    pub(super) fn invoke_callable_value(
        &mut self,
        prog: &Prog,
        callable: Value,
        arguments: Vec<Value>,
        caller: (FrameId, usize, &mut Vec<Value>),
    ) -> Result<Value, RuntimeError> {
        let (function, captures) = match &callable {
            Value::Function(function) => (function.clone(), Vec::new()),
            Value::Closure { function, captures } => {
                let mut materialized = Vec::with_capacity(captures.len());
                for capture in captures {
                    if capture.owned {
                        return Err(RuntimeError::Unsupported(
                            "vm: an owned-capture closure is not supported as a Variant \
                             owning-operation handler"
                                .to_string(),
                        ));
                    }
                    materialized.push(capture.value.clone());
                }
                (function.clone(), materialized)
            }
            value => {
                return Err(RuntimeError::NotCallable(crate::runtime::type_name(value)));
            }
        };
        let mut positional = captures;
        positional.extend(arguments);
        let index = prog
            .index_of(&function)
            .ok_or_else(|| RuntimeError::NotCallable(function.clone()))?;
        // The caller-reachable channel keeps captured references into the
        // invoking frame valid for the child call, exactly like the checked
        // iterator calls.
        let (frame_id, function_index, variables) = caller;
        let (value, _, _) = self.call_frame_caller_reachable(
            prog,
            index,
            positional,
            frame_id,
            function_index,
            variables,
        )?;
        Ok(value)
    }

    pub(super) fn construct_via_copy(
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
        let mut value_params = reify_value_parameters(&def.param_decls, param_vals);
        // Copy construction produces the source's exact type; inherit its
        // reified parameters when the call site supplied none.
        if value_params
            .iter()
            .all(|(_, value)| matches!(value, Value::None))
            && let Value::Struct {
                name: source_name,
                value_params: source_params,
                ..
            } = &kwargs[0].1
            && source_name == name
            && !source_params.is_empty()
        {
            value_params = source_params.clone();
        }
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
    pub(super) fn clone_value(&mut self, prog: &Prog, v: &Value) -> Result<Value, RuntimeError> {
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

    /// Run lifecycle copying while reference handles embedded in the value can
    /// still resolve against the executing caller and (for iterator results)
    /// the just-completed callee. The VM normally pops the executing frame while
    /// interpreting one instruction; moving its real slots into temporary frame
    /// views preserves both reads and any observable write-through performed by
    /// user copy code. Storage is restored on success or failure.
    pub(super) fn clone_value_with_reachable_frames(
        &mut self,
        prog: &Prog,
        value: &Value,
        current: FrameId,
        current_variables: &mut Vec<Value>,
        returned_frame: Option<(FrameId, &mut Vec<Value>)>,
    ) -> Result<Value, RuntimeError> {
        let stack_base = self.frames.len();
        debug_assert!(self.frames.iter().all(|frame| frame.id != current));
        self.frames.push(Frame {
            id: current,
            function: 0,
            registers: Vec::new(),
            variables: std::mem::take(current_variables),
            block: 0,
            instruction: 0,
            continuation: None,
        });

        let mut returned_variables = returned_frame.map(|(id, variables)| {
            debug_assert!(id != current);
            debug_assert!(self.frames.iter().all(|frame| frame.id != id));
            self.frames.push(Frame {
                id,
                function: 0,
                registers: Vec::new(),
                variables: std::mem::take(variables),
                block: 0,
                instruction: 0,
                continuation: None,
            });
            variables
        });

        let result = self.clone_value(prog, value);
        if let Some(variables) = returned_variables.as_mut() {
            let frame = self
                .frames
                .pop()
                .expect("copy lifecycle retained returned frame");
            **variables = frame.variables;
        }
        let frame = self
            .frames
            .pop()
            .expect("copy lifecycle retained current frame");
        *current_variables = frame.variables;
        debug_assert_eq!(self.frames.len(), stack_base);
        result
    }

    /// Relocate a **moved** value (a `UseVar { Move }` / `^` transfer). For a struct
    /// that defines `__moveinit__`, run it (`existing` is consumed); otherwise the
    /// default move — the value's slot was already tombstoned — suffices. Only
    /// reached when `has_moveinit` is set.
    pub(super) fn move_value(&mut self, prog: &Prog, v: Value) -> Result<Value, RuntimeError> {
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
}
