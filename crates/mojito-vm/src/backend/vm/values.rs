//! Value construction and mutation: operators, place stores,
//! constructors, strings, cloning, and moves.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::TyKind;

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
            (Value::Ref { .. }, Value::Int(delta), InfixOp::Add | InfixOp::Sub) => {
                let delta = if op == InfixOp::Sub { -delta } else { *delta };
                return super::references::offset_place_handle(l, delta);
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
            // A struct operator carries the checker-selected dunder symbol
            // (`S.__add__`, or the overload key of `StringSpan.__eq__(rhs:
            // String)`), retargeted to the runtime receiver type like a
            // method call; arity selects only for an operator naming none.
            let sname = name.clone();
            let fname = prog.runtime_method_name(&sname, dunder, resolved, 1);
            return self.call_resolved_dunder(prog, &sname, dunder, vec![l, r], fname.as_deref());
        }
        apply_infix(op, &l, &r)
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

    /// Store a value through any currently supported place shape. A final index
    /// into an arena pointer writes the heap; ordinary slots, fields, private
    /// pack storage, and SIMD lanes use `store_place`. Shared by MIR stores and mut-self
    /// write-back so `self.buckets[i].append(v)` follows the same path as
    /// `self.buckets[i] = row`.
    pub(super) fn store_at_place(
        &mut self,
        frame_id: FrameId,
        place: &MirPlace,
        value: Value,
        regs: &[Value],
        vars: &mut [Value],
    ) -> Result<(), RuntimeError> {
        enum Target {
            Pointer(u64, i64, Reg),
            Ordinary,
        }
        let place = &*place.storage();
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
            // A parent reached through a stored handle (`self.src.entries._storage`
            // through a `ref` field) is read through the reference walk, as
            // a load of it is.
            let crossing = references::place_crosses_reference(&parent)
                .then(|| Self::reference_to_place_parts(frame_id, regs, vars, &parent))
                .transpose()?
                .map(|handle| self.read_reference(&handle, frame_id, vars))
                .transpose()?;
            let parent_value = match &crossing {
                Some(value) => value,
                None => nav_mut(vars, regs, &parent)?,
            };
            match parent_value {
                Value::Pointer { allocation, offset } => {
                    Target::Pointer(*allocation, *offset, *index)
                }
                _ => Target::Ordinary,
            }
        } else {
            Target::Ordinary
        };
        match target {
            Target::Pointer(allocation, base, index) => {
                let offset = value_as_index(&regs[index.0 as usize])?;
                let (region, slot) = self.heap_index(allocation, base, offset)?;
                self.heap_store(region, slot, value);
                Ok(())
            }
            Target::Ordinary => store_place(vars, regs, place, value),
        }
    }

    /// Write a value through the handle a place designates. A `ref`-typed
    /// place holds *another* handle, and the assignment reaches its referent —
    /// the store twin of `LoadPlace`'s second dereference — rather than
    /// overwriting the handle slot. The write can land in another frame's
    /// storage, so handles inside the written value that root in this frame
    /// are re-rooted first or they dangle once this frame is disposed.
    pub(super) fn store_through_handle(
        &mut self,
        frame_id: FrameId,
        place: &MirPlace,
        handle: Value,
        mut value: Value,
        vars: &mut [Value],
    ) -> Result<(), RuntimeError> {
        let target = if matches!(place.ty.as_ref().map(Ty::kind), Some(TyKind::Ref(_))) {
            match self.read_reference(&handle, frame_id, vars)? {
                inner @ Value::Ref { .. } => inner,
                _ => handle,
            }
        } else {
            handle
        };
        self.canonicalize_value_references(frame_id, vars, &mut value);
        self.write_reference(&target, frame_id, vars, value)
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
            return self.store_at_place(frame_id, place, value, regs, vars);
        }
        self.write_reference(&handle, frame_id, vars, value)
    }

    /// Construct a struct through the compiled `constructor` a call names:
    /// build an uninitialized `self` skeleton (fields = `None` placeholders, value parameters
    /// reified), run `__init__(self, args…)`, and return the initialized `self`
    /// (frame slot 0). The checker's definite-init check guarantees every field is
    /// assigned in the body, so no placeholder survives. Arguments are coerced to the
    /// `__init__` parameter types by the normal call ABI.
    pub(super) fn construct_via_init(
        &mut self,
        prog: &Prog,
        name: &str,
        constructor: &str,
        args: Vec<Value>,
        kwargs: &[(String, Value)],
    ) -> Result<Value, RuntimeError> {
        let user_args = match prog.sigs.get(constructor) {
            Some(signature) => {
                self.bind_for_call(prog, constructor, signature, &args, kwargs)?
                    .0
            }
            None => args,
        };
        self.construct_bound(prog, name, constructor, user_args)
    }

    /// [`Self::construct_via_init`] over `user_args` already bound to the
    /// constructor's parameters in declared order, as a compile-time
    /// application passes them.
    pub(super) fn construct_bound(
        &mut self,
        prog: &Prog,
        name: &str,
        constructor: &str,
        user_args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        let def = prog.structs.get(name).ok_or_else(|| {
            RuntimeError::Unsupported(format!(
                "vm: constructed struct '{name}' is missing from MIR"
            ))
        })?;
        let skeleton = Value::Struct {
            name: name.to_string(),
            fields: uninitialized_fields(&def.fields),
        };
        self.construct_entered(prog, skeleton, constructor, user_args)
    }

    /// Run the initializer `constructor` over its `out self` receiver
    /// entering as `skeleton` and `user_args` bound in declared order, for
    /// the initialized receiver (frame slot 0).
    pub(super) fn construct_entered(
        &mut self,
        prog: &Prog,
        skeleton: Value,
        constructor: &str,
        user_args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        let fidx = prog.index_of(constructor).ok_or_else(|| {
            RuntimeError::Unsupported(format!(
                "vm: checked constructor '{constructor}' is missing from MIR"
            ))
        })?;
        let mut bound = Vec::with_capacity(user_args.len() + 1);
        bound.push(skeleton);
        bound.extend(user_args);
        let (_, frame_vars) = self.call_frame(prog, fidx, bound)?;
        Ok(frame_vars.into_iter().next().unwrap_or(Value::None))
    }

    /// The unwritten storage of a value of `ty`: a struct's skeleton, as an
    /// initializer's receiver enters with, the storage a named result marked
    /// initialized whole has before pointer writes fill it, or the
    /// placeholder that skeleton holds for a field of compiler-private tuple
    /// storage ([`uninitialized_field`]), which a split struct's field slot
    /// holds whole, or a known `SIMD`'s zero vector. `None` for a struct MIR
    /// does not declare and any other type.
    pub(super) fn uninitialized_value(prog: &Prog, ty: &Ty) -> Option<Value> {
        match ty.kind() {
            // A held struct's storage is its one field's.
            struct_ty
                if let Some((name, _)) = struct_ty.as_non_simd_struct()
                    && let Some(definition) = prog.structs.get(name)
                    && definition.register_field.is_some()
                    && let [(_, field)] = definition.fields.as_slice() =>
            {
                Some(uninitialized_field(field))
            }
            struct_ty if let Some((name, _)) = struct_ty.as_non_simd_struct() => {
                Some(Value::Struct {
                    name: name.clone(),
                    fields: uninitialized_fields(&prog.structs.get(name)?.fields),
                })
            }
            TyKind::Tuple(_) | TyKind::PopArray { .. } => Some(uninitialized_field(ty)),
            // Upstream's `undef` vector, which `SIMD`'s element list marks
            // initialized before writing it lane by lane: the zero vector.
            kind if let Some((dtype, width)) = kind.as_simd() => {
                let dtype = dtype.known()?;

                let width = usize::try_from(width.known()?).ok()?;
                let zero = if dtype == mojito_ast::ast::Dtype::Bool {
                    Value::Bool(false)
                } else {
                    Value::Int(0)
                };
                crate::runtime::simd_from_values(dtype, width, &[zero]).ok()
            }
            _ => None,
        }
    }

    /// The value of a never-stored slot of `ty` that `mem-2-reg` reads as
    /// `uninitmem`, upstream's `undef`: a struct whose fields hold no
    /// element at any depth (none, or only empty Tuple storage, as an empty
    /// `Tuple`'s) is whole with nothing stored, so it is that struct, a
    /// `!pop.array` is its
    /// elements' placeholders, which stores through `pop.array.gep` fill
    /// one by one, and a vector or its `!kgen.simd` storage is one of the
    /// vector's type, whose lanes `pop.simd.insertelement` writes one by
    /// one; anything else is uninitialized storage.
    pub(super) fn uninit_mem_value(prog: &Prog, ty: Option<&Ty>) -> Value {
        let ty = ty.map(mojito_types::types::storage_value_ty);
        let ty = ty.as_ref();
        let elements = ty
            .is_some_and(|ty| matches!(ty.kind(), TyKind::PopArray { .. }) || ty.kind().is_simd());
        ty.and_then(|ty| Self::uninitialized_value(prog, ty))
            .filter(|value| {
                elements || (matches!(value, Value::Struct { .. }) && is_empty_value(value))
            })
            .unwrap_or(Value::UninitStorage(None))
    }

    /// `input()` under [`Self::set_input_override`]: append the prompt to the
    /// captured output (a native executable writes prompts to stdout, which the
    /// differential compares byte-for-byte), then serve one line from the
    /// override buffer — trailing `\n` then `\r` stripped; EOF raises `EOF`.
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
        let read = std::io::BufRead::read_line(cursor, &mut line).map_err(|e| {
            RuntimeError::Unsupported(format!("input(): failed to read stdin: {e}"))
        })?;
        if read == 0 {
            return Err(crate::runtime::input_eof());
        }
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
        let bytes = self.heap_bytes(
            &Value::Pointer {
                allocation: *allocation,
                offset: *offset,
            },
            *size,
        )?;
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
        };
        self.materialize_string_struct(skeleton, text)
    }

    /// The library `Error` over `message`, as its `String` initializer
    /// builds it: the struct holding the nominal `String` of the text.
    pub(super) fn error_value(
        &mut self,
        prog: &Prog,
        message: &str,
    ) -> Result<Value, RuntimeError> {
        let error = mojito_types::types::ERROR_TYPE_NAME;
        let Some(def) = prog.structs.get(error) else {
            return Err(RuntimeError::Unsupported(
                "vm: raising an `Error` needs the linked `std.builtin.error`".to_string(),
            ));
        };
        let fields = def
            .fields
            .iter()
            .map(|(field, _)| {
                Ok((
                    field.clone(),
                    match field.as_str() {
                        ERROR_MESSAGE_FIELD => self.nominal_string_value(prog, message)?,
                        other => unreachable!("unexpected Error field '{other}'"),
                    },
                ))
            })
            .collect::<Result<_, RuntimeError>>()?;
        Ok(Value::Struct {
            name: error.to_string(),
            fields,
        })
    }

    /// The message of a library `Error` value, or `None` for any other
    /// value.
    pub(super) fn error_text(&self, value: &Value) -> Option<String> {
        let Value::Struct { name, fields } = value else {
            return None;
        };
        if name != mojito_types::types::ERROR_TYPE_NAME {
            return None;
        }
        fields
            .iter()
            .find(|(field, _)| field == ERROR_MESSAGE_FIELD)
            .and_then(|(_, message)| self.nominal_string_text(message))
    }

    /// Fill a nominal `String` skeleton from a literal's UTF-8 bytes: one
    /// heap allocation holding width-1 `UInt8` scalars, sized exactly.
    pub(super) fn materialize_string_struct(
        &mut self,
        skeleton: Value,
        literal: &str,
    ) -> Result<Value, RuntimeError> {
        let bytes = literal.as_bytes();
        let allocation = self.alloc_utf8_bytes(bytes)?;
        let Value::Struct { name, mut fields } = skeleton else {
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
        Ok(Value::Struct { name, fields })
    }

    /// Produce a semantically-correct **copy** of `v`, a value of the
    /// static type `ty` when known (a `UseVar { Copy }` read, a by-value
    /// argument, or a return), as upstream copies a value by its type. For a
    /// struct that defines `__copyinit__`, run it (so a pointer-owning type
    /// deep-copies its storage instead of aliasing); for a struct without
    /// one, recurse into its fields at their declared types (a nested field
    /// may define it). A held struct ([`Prog::boxed`]) is copied as its box
    /// and keeps its unboxed shape. A tuple's elements copy at their element
    /// types, a variant's payload at its alternative's, and internal
    /// compile-time storage element by element. Only reached when
    /// `has_copyinit` is set, for a copy naming no constructor
    /// ([`Self::copy_named`] runs a named one) and for the fields, captures,
    /// and payloads a copy reaches.
    pub(super) fn clone_value(
        &mut self,
        prog: &Prog,
        v: &Value,
        ty: Option<&Ty>,
    ) -> Result<Value, RuntimeError> {
        if dispatch::holds_unboxed(prog, v, ty) {
            let copy = self.clone_value(prog, &prog.boxed(v.clone(), ty), ty)?;
            return Ok(prog.unboxed(copy, ty));
        }
        match v {
            Value::Struct { name, fields } => {
                if let Some(fidx) = prog.lifecycle_member(name, "__copyinit__") {
                    let skeleton = Self::struct_skeleton(prog, name);
                    let (_, frame_vars) = self.call_frame(prog, fidx, vec![skeleton, v.clone()])?;
                    Ok(frame_vars.into_iter().next().unwrap_or(Value::None))
                } else {
                    let declared = prog.structs.get(name);
                    let mut new_fields = Vec::with_capacity(fields.len());
                    for (f, fv) in fields {
                        let field_ty = declared.and_then(|declared| {
                            declared
                                .fields
                                .iter()
                                .find_map(|(declared, ty)| (declared == f).then_some(ty))
                        });
                        new_fields.push((f.clone(), self.clone_value(prog, fv, field_ty)?));
                    }
                    Ok(Value::Struct {
                        name: name.clone(),
                        fields: new_fields,
                    })
                }
            }
            Value::ComptimeList(items) => {
                let mut out = Vec::with_capacity(items.len());
                for it in items {
                    out.push(self.clone_value(prog, it, None)?);
                }
                Ok(Value::ComptimeList(out))
            }
            Value::Tuple(items) => {
                let types = dispatch::element_types(ty, items.len());
                let mut out = Vec::with_capacity(items.len());
                for (at, it) in items.iter().enumerate() {
                    out.push(self.clone_value(prog, it, types.and_then(|types| types.get(at)))?);
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
                value: Box::new(self.clone_value(prog, value, alternatives.get(*index))?),
            }),
            // Scalars alias/copy trivially; a bare pointer copy *aliases* (correct —
            // deep-copy is the owning struct's `__copyinit__` job, handled above).
            other => Ok(other.clone()),
        }
    }

    /// Run lifecycle copying of `value`, at the result type and by the
    /// constructor `adapter` names, as [`Self::copy_named`], while reference handles embedded in the value can
    /// still resolve against the executing caller and (for iterator results)
    /// the just-completed callee. The VM normally pops the executing frame while
    /// interpreting one instruction; moving its real slots into temporary frame
    /// views preserves both reads and any observable write-through performed by
    /// user copy code. Storage is restored on success or failure.
    pub(super) fn clone_value_with_reachable_frames(
        &mut self,
        prog: &Prog,
        value: &Value,
        adapter: ResultAdapter<'_>,
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
            cursor: RegionCursor::default(),
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
                cursor: RegionCursor::default(),
                continuation: None,
            });
            variables
        });

        let result = self.copy_named(prog, value, adapter.ty, adapter.constructor);
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

    /// Relocate a **moved** value `v` of the static type `ty` when known (a
    /// `UseVar { Move }` / `^` transfer). For a struct that defines
    /// `__moveinit__`, run it (`existing` is consumed); otherwise the default
    /// move — the value's slot was already tombstoned — suffices. A held
    /// struct ([`Prog::boxed`]) moves as its box and keeps its unboxed
    /// shape. Only reached when `has_moveinit` is set.
    pub(super) fn move_typed_value(
        &mut self,
        prog: &Prog,
        v: Value,
        ty: Option<&Ty>,
    ) -> Result<Value, RuntimeError> {
        if dispatch::holds_unboxed(prog, &v, ty) {
            let moved = self.move_typed_value(prog, prog.boxed(v, ty), ty)?;
            return Ok(prog.unboxed(moved, ty));
        }
        if let Value::Struct { name, .. } = &v
            && let Some(fidx) = prog.lifecycle_member(name, "__moveinit__")
        {
            let skeleton = Self::struct_skeleton(prog, name);
            let (_, frame_vars) = self.call_frame(prog, fidx, vec![skeleton, v])?;
            return Ok(frame_vars.into_iter().next().unwrap_or(Value::None));
        }
        Ok(v)
    }

    /// The program-lifetime allocation holding a closed parameter list's
    /// elements, each at `element`, allocated once per distinct list and
    /// never freed.
    pub(super) fn static_param_list(
        &mut self,
        prog: &Prog,
        element: &Ty,
        values: &[mojito_types::ct::CtValue],
    ) -> Result<u64, RuntimeError> {
        let key = format!("{element}{values:?}");
        if let Some(allocation) = self.static_param_lists.get(&key) {
            return Ok(*allocation);
        }
        let pointer = self.heap_alloc(values.len() as i64, 1)?;
        let Value::Pointer { allocation, .. } = pointer else {
            unreachable!("heap_alloc returns a pointer");
        };
        let region = (allocation - 1) as usize;
        for (index, value) in values.iter().enumerate() {
            let value = self.thaw(prog, value, Some(element))?;
            self.heap_store(region, index, value);
        }
        self.static_param_lists.insert(key, allocation);
        Ok(allocation)
    }

    /// One heap allocation holding `bytes` as width-1 `UInt8` scalars, sized
    /// exactly; returns its allocation id (offset 0 is the first byte).
    fn alloc_utf8_bytes(&mut self, bytes: &[u8]) -> Result<u64, RuntimeError> {
        let pointer = self.heap_alloc(bytes.len() as i64, 1)?;
        let Value::Pointer { allocation, .. } = pointer else {
            unreachable!("heap_alloc returns a pointer");
        };
        let region = (allocation - 1) as usize;
        for (index, byte) in bytes.iter().enumerate() {
            self.heap_store(
                region,
                index,
                Value::Simd {
                    dtype: mojito_ast::ast::Dtype::UInt8,
                    lanes: crate::runtime::SimdLanes::Int(vec![i128::from(*byte)]),
                },
            );
        }
        Ok(allocation)
    }
}

/// One placeholder per declared field ([`uninitialized_field`]).
fn uninitialized_fields(fields: &[(String, Ty)]) -> Vec<(String, Value)> {
    fields
        .iter()
        .map(|(field, ty)| (field.clone(), uninitialized_field(ty)))
        .collect()
}

/// The placeholder an `out self` skeleton holds for a field of type `ty`
/// until the initializer assigns it. Compiler-private Tuple storage and a
/// `!pop.array` are built one element at a time, so each starts as one
/// placeholder per element.
fn uninitialized_field(ty: &Ty) -> Value {
    match ty.kind() {
        TyKind::Tuple(elements) => Value::Tuple(vec![Value::None; elements.len()]),
        TyKind::PopArray { size, .. } => Value::Tuple(vec![
            Value::None;
            size.known()
                .and_then(|size| usize::try_from(size).ok())
                .unwrap_or_default()
        ]),
        _ => Value::None,
    }
}

/// Whether `value`, an uninitialized skeleton, holds no element at any
/// depth: a struct or Tuple storage whose every part is one such.
fn is_empty_value(value: &Value) -> bool {
    match value {
        Value::Struct { fields, .. } => fields.iter().all(|(_, field)| is_empty_value(field)),
        Value::Tuple(items) => items.iter().all(is_empty_value),
        _ => false,
    }
}

/// The message field of upstream's `Error`.
const ERROR_MESSAGE_FIELD: &str = "_error";
