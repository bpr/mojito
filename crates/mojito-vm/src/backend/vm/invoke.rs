//! Call machinery: writeback/synchronous calls, caller mirrors,
//! argument binding, kwargs, and method dispatch.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

impl VmBackend {
    /// Call a free function that has `mut`/`ref` parameters, writing each one's
    /// final value back to the caller's argument place (`arg_places`). This is the
    /// runtime half of call-scoped reference parameters, performed over the
    /// caller's frame (`regs`/`vars`).
    pub(super) fn call_with_writeback(
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
        } = call;
        let CallerFrame {
            id: frame_id,
            function: caller_function,
            registers: regs,
            variables: vars,
        } = frame;
        // Order the arguments into parameter slots (filling defaults/keywords),
        // keeping the slot map so each parameter's source argument is known.
        let keyword_names: Vec<String> = kwargs.iter().map(|(name, _)| name.clone()).collect();
        let (bound, slots) = if let Some(sig) = prog.sigs.get(name) {
            self.bind_for_call(prog, name, sig, &argv, &kwargs)?
        } else {
            let slots = (0..argv.len()).map(ArgSlot::Positional).collect();
            (argv, slots)
        };
        let function = &prog.mir.functions[idx].1;
        let mut reference_inputs = Vec::new();
        for i in 0..function.ref_params.len() {
            let place = bound_argument_place(
                slots.get(i),
                prog.sigs
                    .get(name)
                    .and_then(|signature| signature.frame_param_name(i)),
                0,
                arg_places,
                &keyword_names,
                kwarg_places,
            );
            let Some(place) = caller_handle_place(function, i, place).map_err(|_| {
                RuntimeError::Unsupported(format!(
                    "vm: a mut/ref argument to '{name}' must be a plain variable or field \
                     (not a temporary, an indexed place, a default, or a forwarded keyword)"
                ))
            })?
            else {
                continue;
            };
            reference_inputs.push((i, self.reference_argument(frame_id, regs, vars, place)?));
        }
        let (result, _, _) = self.call_synchronously_with_references(
            prog,
            SynchronousCall {
                function_index: idx,
                arguments: bound,
                reference_inputs: &reference_inputs,
            },
            CallerFrame {
                id: frame_id,
                function: caller_function,
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
    /// nested anywhere in the return value already point at the caller. The
    /// completed child identity is retained for references into its own receiver.
    /// Every synchronous call kind uses this one boundary.
    pub(super) fn call_synchronously_with_references(
        &mut self,
        prog: &Prog,
        call: SynchronousCall<'_>,
        caller: CallerFrame<'_>,
    ) -> Result<(Value, Vec<Value>, FrameId), RuntimeError> {
        let SynchronousCall {
            function_index,
            mut arguments,
            reference_inputs,
        } = call;
        let CallerFrame {
            id: caller_id,
            function: _,
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
        let outcome = self.call_frame_with_id(prog, function_index, arguments);
        self.restore_caller_mirror(stack_base, caller_variables)?;
        outcome
    }

    pub(super) fn push_caller_mirror(
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
            cursor: RegionCursor::default(),
            continuation: None,
        });
        stack_base
    }

    pub(super) fn restore_caller_mirror(
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

    pub(super) fn bind_for_call(
        &mut self,
        prog: &Prog,
        name: &str,
        sig: &FnSig,
        argv: &[Value],
        kwargs: &[(String, Value)],
    ) -> Result<(Vec<Value>, Vec<ArgSlot>), RuntimeError> {
        // Materialize omitted defaults. A `Construct` default runs its
        // converting constructor (e.g. the empty `Optional[T]` for a `None`
        // default) through the same path an explicit `f(arg=None)` takes;
        // scalars fold directly. An `Evaluate` default is the call's to run
        // (`mir::fill_omitted_arguments`), so a slot still omitting one is
        // left to the callee by a call shape the fill does not name.
        let make_default = |i: usize| -> Result<Value, RuntimeError> {
            match &sig.defaults[i] {
                Some(CheckedConst::Construct { target, arg }) => self.call_named(
                    prog,
                    target,
                    vec![checked_const_value(arg)],
                    vec![],
                    &CallTypes {
                        constructed: prog.declared_struct(sig.param_types.get(i)),
                        ..CallTypes::default()
                    },
                ),
                Some(CheckedConst::Evaluate { function, .. }) => {
                    Err(RuntimeError::Unsupported(format!(
                        "vm: the default '{function}' of parameter '{}' of '{name}' is left to \
                         the callee, where only its call evaluates it",
                        sig.param_names[i]
                    )))
                }
                Some(other) => Ok(checked_const_value(other)),
                None => Err(RuntimeError::Unsupported(format!(
                    "vm: non-constant default for parameter '{}' of '{name}'",
                    sig.param_names[i]
                ))),
            }
        };
        bind_args(name, sig, argv, kwargs, make_default)
    }

    /// [`Self::bind_for_call`] of a call of a closure over `name`, whose
    /// first `captures` arguments its captures fill. A closure over a
    /// method passes its first capture, the closure's storage, as the
    /// receiver its signature does not list, in frame slot zero.
    pub(super) fn bind_for_closure_call(
        &mut self,
        prog: &Prog,
        name: &str,
        sig: &FnSig,
        argv: &[Value],
        kwargs: &[(String, Value)],
        captures: usize,
    ) -> Result<(Vec<Value>, Vec<ArgSlot>), RuntimeError> {
        let Some((receiver, arguments)) = argv
            .split_first()
            .filter(|_| sig.has_receiver && captures > 0)
        else {
            return self.bind_for_call(prog, name, sig, argv, kwargs);
        };
        let (bound, slots) = self.bind_for_call(prog, name, sig, arguments, kwargs)?;
        Ok((
            std::iter::once(receiver.clone()).chain(bound).collect(),
            std::iter::once(ArgSlot::Positional(0))
                .chain(slots.into_iter().map(|slot| match slot {
                    ArgSlot::Positional(argument) => ArgSlot::Positional(argument + 1),
                    slot => slot,
                }))
                .collect(),
        ))
    }

    /// Dispatch a method call. Nominal values resolve to their mangled
    /// `Type.method` function, with a `mut self` receiver written back; only
    /// primitive and compiler-private storage kinds retain intrinsic branches.
    #[allow(clippy::too_many_lines, reason = "TODO: split this pass")]
    pub(super) fn method_call(
        &mut self,
        prog: &Prog,
        invocation: MethodInvocation<'_>,
        frame: CallerFrame<'_>,
    ) -> Result<Value, RuntimeError> {
        let MethodInvocation {
            receiver: recv,
            method,
            resolved_name: resolved,
            result_adapter,
            arguments: args,
            keyword_arguments: kwargs,
            receiver_place: recv_place,
            argument_places: arg_places,
            keyword_argument_places: kwarg_places,
        } = invocation;
        let CallerFrame {
            id: frame_id,
            function: caller_function,
            registers: regs,
            variables: vars,
        } = frame;
        // A receiver read out of a `ref`-typed field arrives as a reference
        // handle: dispatch on its referent. A `mut self` write-back re-enters
        // the storage through the receiver place, which chases the same
        // handle.
        let recv = if matches!(recv, Value::Ref { .. }) {
            self.read_reference(&recv, frame_id, vars)?
        } else {
            recv
        };
        let keyword_names: Vec<String> = kwargs.iter().map(|(name, _)| name.clone()).collect();
        // Intrinsic dunders on a built-in numeric/hashable value; a struct with
        // its own implementation still dispatches to its method below.
        if !matches!(recv, Value::Struct { .. }) {
            match (method, args.len()) {
                // Hashable scalar leaf: contribute the value itself (`-0.0`
                // folded, as upstream's `SIMD.__hash__`) to the caller-owned
                // hasher through its `_update_with_simd` instance at the leaf's
                // own vector type.
                ("__hash__", 1) => {
                    // A string literal hashes as the nominal `String` it
                    // materializes to, so literal and nominal keys agree.
                    if let Value::Str(text) = &recv {
                        let text = text.clone();
                        let materialized = self.nominal_string_value(prog, &text)?;
                        if let Value::Struct { name, .. } = &materialized {
                            let instance = prog.hasher_method_instance(name, method, &args[0]);
                            return self.method_call(
                                prog,
                                MethodInvocation {
                                    receiver: materialized,
                                    method,
                                    resolved_name: instance.as_deref(),
                                    result_adapter,
                                    arguments: args,
                                    keyword_arguments: kwargs,
                                    receiver_place: recv_place,
                                    argument_places: arg_places,
                                    keyword_argument_places: kwarg_places,
                                },
                                CallerFrame {
                                    id: frame_id,
                                    function: caller_function,
                                    registers: regs,
                                    variables: vars,
                                },
                            );
                        }
                    }
                    let place = arg_places.first().and_then(Option::as_ref).ok_or_else(|| {
                        RuntimeError::Unsupported(
                            "vm: Hashable.__hash__ needs a mutable hasher place".into(),
                        )
                    })?;
                    let hasher = args[0].clone();
                    let Value::Struct { name, .. } = &hasher else {
                        return Err(RuntimeError::TypeError(format!(
                            "Hashable.__hash__ expected a Hasher, got {}",
                            crate::runtime::type_name(&hasher)
                        )));
                    };
                    let leaf = crate::runtime::hash_leaf_ty(&recv).ok_or_else(|| {
                        RuntimeError::TypeError(format!(
                            "cannot hash {} as a scalar leaf",
                            crate::runtime::type_name(&recv)
                        ))
                    })?;
                    let fidx = prog.hash_leaf_update(name, &leaf).ok_or_else(|| {
                        RuntimeError::Unsupported(format!(
                            "vm: Hasher `{name}` has no `_update_with_simd` for {leaf}"
                        ))
                    })?;
                    let arguments = vec![hasher, crate::runtime::hash_leaf_value(recv.clone())];
                    let (_, variables) = self.call_frame(prog, fidx, arguments)?;
                    let updated = variables.into_iter().next().unwrap_or(Value::None);
                    self.store_at_call_place(prog, frame_id, place, updated, regs, vars)?;
                    return Ok(Value::None);
                }
                // `Floorable`/`Ceilable`/`Truncable` — `x.__floor__()` etc.
                // (roadmap milestone 7).
                ("__floor__" | "__ceil__" | "__trunc__", 0) => {
                    return crate::runtime::builtin_round_dir(method, &recv);
                }
                // `CeilDivable` — `x.__ceildiv__(y)`.
                ("__ceildiv__", 1) => return crate::runtime::builtin_ceildiv(&recv, &args[0]),
                // `Absable`, `Roundable`, and `DivModable` on a scalar: its
                // builtin (the elaborator makes `__pow__` its `**`).
                ("__abs__", 0) => return crate::runtime::builtin_abs(recv),
                ("__round__", 0) => return crate::runtime::builtin_round(&recv),
                ("__divmod__", 1) => {
                    return crate::runtime::builtin_divmod(recv, args[0].clone());
                }
                // A float range element's fused `self * b + c`.
                ("__fma__", 2) => {
                    return crate::runtime::builtin_fma(&recv, &args[0], &args[1]);
                }
                // `SIMD.__len__`: the lane count, one for each scalar it models.
                ("__len__", 0) => match &recv {
                    Value::Int(_) | Value::UInt(_) | Value::Float64(_) => return Ok(Value::Int(1)),
                    Value::Simd { lanes, .. } => return Ok(Value::Int(lanes.width() as i64)),
                    _ => {}
                },
                // `Equatable` and `Comparable` on a scalar: its operator.
                ("__eq__" | "__ne__" | "__lt__" | "__le__" | "__gt__" | "__ge__", 1) if !matches!(&recv, Value::Simd { lanes, .. } if lanes.width() != 1) =>
                {
                    use mojito_ast::ast::InfixOp::{Eq, Ge, Gt, Le, Lt, Ne};
                    let op = [Eq, Ne, Lt, Le, Gt, Ge]
                        .into_iter()
                        .find(|op| op.dunder() == Some(method))
                        .unwrap_or(Ge);
                    return crate::runtime::apply_infix(op, recv, args[0].clone());
                }
                // `Boolable`, `Intable`, `Floatable`, and `Indexer` on a
                // scalar: its conversion. A string literal is true when
                // non-empty.
                ("__bool__", 0) if matches!(&recv, Value::Str(_)) => {
                    return Ok(Value::Bool(
                        matches!(&recv, Value::Str(text) if !text.is_empty()),
                    ));
                }
                ("__bool__" | "__int__" | "__float__" | "__mlir_index__", 0) => {
                    let target = match method {
                        "__bool__" => "Bool",
                        "__float__" => "Float64",
                        _ => "Int",
                    };
                    return crate::runtime::builtin_convert(target, recv);
                }
                _ => {}
            }
        }
        match &recv {
            // The builtin text writer's `Writer.write_string`: append the
            // view's bytes.
            Value::Str(current) if method == "write_string" && args.len() == 1 => {
                let place = recv_place.as_ref().ok_or_else(|| {
                    RuntimeError::Unsupported(
                        "vm: Writer.write_string needs a mutable place".into(),
                    )
                })?;
                let text = format!("{current}{}", self.string_span_text(&args[0])?);
                self.store_at_call_place(prog, frame_id, place, Value::Str(text), regs, vars)?;
                Ok(Value::None)
            }
            Value::Str(text) if method == "byte_length" && args.is_empty() => {
                Ok(Value::Int(text.len() as i64))
            }
            Value::Str(text) if matches!(method, "ptr" | "unsafe_ptr") && args.is_empty() => {
                let allocation = self.static_literal_bytes(text)?;
                Ok(Value::Pointer {
                    allocation,
                    offset: 0,
                })
            }
            Value::Str(current)
                if matches!(method, "write" | mojito_symbol::symbol::WRITE_FORMATTED) =>
            {
                let place = recv_place.as_ref().ok_or_else(|| {
                    RuntimeError::Unsupported("vm: Writer.write needs a mutable place".into())
                })?;
                let mut text = current.clone();
                // A formatted argument's `write_to` runs in a nested frame
                // that may read references into this caller (a temporary
                // holding `Pointer(to=local)`): keep the caller reachable as
                // a synchronous call does.
                let stack_base = self.push_caller_mirror(frame_id, regs, vars);
                let formatted = (|| {
                    for argument in args {
                        text.push_str(&self.format_value(prog, argument, false)?);
                    }
                    Ok::<_, RuntimeError>(())
                })();
                self.restore_caller_mirror(stack_base, vars)?;
                formatted?;
                self.store_at_call_place(prog, frame_id, place, Value::Str(text), regs, vars)?;
                Ok(Value::None)
            }
            // A specialized pack's storage answers `Sized` where it stands:
            // `b.__len__()` is the element count `len(b)` reads, and the
            // clone has no nominal Tuple to lower the spelling onto.
            Value::Tuple(items) if method == "__len__" && args.is_empty() => {
                Ok(Value::Int(items.len() as i64))
            }
            Value::Tuple(_) => Err(RuntimeError::Unsupported(format!(
                "vm: internal tuple-pack storage has no runtime method '{method}'; public Tuple methods require nominal lowering"
            ))),
            Value::Simd { dtype, lanes } => {
                crate::runtime::simd_method(*dtype, lanes, method, &args)
            }
            Value::Dtype(dtype) => dtype
                .predicate(method)
                .filter(|_| args.is_empty())
                .map(Value::Bool)
                .ok_or_else(|| {
                    RuntimeError::Unsupported(format!("vm: DType has no method '{method}'"))
                }),
            // `Pointer` methods: `free()` releases the allocation (a no-op in
            // the arena model — the arena never reclaims).
            Value::Pointer { allocation, offset } => match method {
                "free" | "unsafe_free" => {
                    self.heap_free(*allocation, *offset)?;
                    Ok(Value::None)
                }
                _ => Err(RuntimeError::Unsupported(format!(
                    "vm: Pointer has no method '{method}'"
                ))),
            },
            Value::Struct { name, .. }
                if method == mojito_symbol::symbol::WRITE_FORMATTED
                    && let Some(index) = prog.struct_member(name, "write_string", 1) =>
            {
                let place = recv_place.as_ref().ok_or_else(|| {
                    RuntimeError::Unsupported("vm: Writer.write needs a mutable place".into())
                })?;
                let mut writer = recv.clone();
                // The formatted arguments' `write_to` frames may read
                // references into this caller (see the builtin-string arm).
                let stack_base = self.push_caller_mirror(frame_id, regs, vars);
                let written = (|| {
                    for argument in args {
                        let text = self.format_value(prog, argument, false)?;
                        // `write_string` borrows a view over a temporary buffer
                        // that lives exactly as long as the call.
                        let (payload, allocation) = self.temporary_string_span(prog, &text)?;
                        let (_, variables) =
                            self.call_frame(prog, index, vec![writer.clone(), payload])?;
                        self.heap_free(allocation, 0)?;
                        writer = variables.into_iter().next().unwrap_or(Value::None);
                    }
                    Ok::<_, RuntimeError>(())
                })();
                self.restore_caller_mirror(stack_base, vars)?;
                written?;
                self.store_at_call_place(prog, frame_id, place, writer, regs, vars)?;
                Ok(Value::None)
            }
            Value::Struct { name, .. } => {
                let method_argc = args.len();
                let source_fname = format!("{name}.{method}");
                let (fname, fidx) = prog
                    .runtime_method_name(name, method, resolved, method_argc)
                    .and_then(|fname| {
                        let fidx = prog.index_of(&fname)?;
                        Some((fname, fidx))
                    })
                    .ok_or_else(|| {
                        RuntimeError::Unsupported(format!("vm: unknown method '{source_fname}'"))
                    })?;
                // Bind ordinary arguments through the same signature metadata as
                // free functions, then prepend `self` in frame slot zero.
                let (bound, slots) = if let Some(signature) = prog.sigs.get(&fname) {
                    self.bind_for_call(prog, &fname, signature, &args, &kwargs)?
                } else {
                    let slots = (0..args.len()).map(ArgSlot::Positional).collect();
                    (args, slots)
                };
                let mut call_args = Vec::with_capacity(bound.len() + 1);
                call_args.push(recv.clone());
                call_args.extend(bound);
                let function = &prog.mir.functions[fidx].1;
                let ref_params = &function.ref_params;
                // A borrowed receiver whose result carries reference fields
                // must also enter as a caller-place handle: a value copy would
                // root the result's `ref` fields in the callee frame, and
                // return-time canonicalization has no caller storage to re-root
                // them through. Owned/deinit receivers keep the value transfer.
                let deinit_receiver = function.deinit_params.first().copied().unwrap_or(false);
                let receiver_as_reference = ref_params.first().copied().unwrap_or(false)
                    || (recv_place.is_some()
                        && !function.owned_params.first().copied().unwrap_or(false)
                        && !deinit_receiver
                        && Self::function_result_carries_reference(prog, fidx));
                // The call consumes its `deinit` receiver's place, as
                // upstream's call consumes a `DeinitMem` operand: the
                // callee's elaborated drops destroy its fields, and the place
                // holds the VM's ended-place tombstone from the call on, as a
                // move leaves one.
                if deinit_receiver
                    && !receiver_as_reference
                    && let Some(place) = recv_place
                {
                    self.store_at_call_place(prog, frame_id, place, Value::Moved, regs, vars)?;
                }
                let mut reference_inputs = Vec::new();
                if receiver_as_reference {
                    let place = recv_place.as_ref().ok_or_else(|| {
                        RuntimeError::Unsupported(format!(
                            "vm: reference receiver for method '{fname}' must be a place"
                        ))
                    })?;
                    reference_inputs
                        .push((0, self.reference_argument(frame_id, regs, vars, place)?));
                }
                for i in 1..ref_params.len() {
                    let place = bound_argument_place(
                        slots.get(i - 1),
                        prog.sigs
                            .get(&fname)
                            .and_then(|signature| signature.frame_param_name(i - 1)),
                        0,
                        arg_places,
                        &keyword_names,
                        kwarg_places,
                    );
                    let Some(place) = caller_handle_place(function, i, place).map_err(|_| {
                        RuntimeError::Unsupported(format!(
                            "vm: a mut/ref argument to method '{fname}' must be a plain \
                             variable or field (not a temporary or indexed place)"
                        ))
                    })?
                    else {
                        continue;
                    };
                    reference_inputs
                        .push((i, self.reference_argument(frame_id, regs, vars, place)?));
                }
                let (ret, mut frame_vars, returned_frame_id) = self
                    .call_synchronously_with_references(
                        prog,
                        SynchronousCall {
                            function_index: fidx,
                            arguments: call_args,
                            reference_inputs: &reference_inputs,
                        },
                        CallerFrame {
                            id: frame_id,
                            function: caller_function,
                            registers: regs,
                            variables: vars,
                        },
                    )?;
                let returns_reference = prog.mir.functions[fidx].1.returns_reference;
                // A static-origin reference may arrive as its heap
                // element's pointer (`returned_value`).
                if returns_reference && !matches!(ret, Value::Ref { .. } | Value::Pointer { .. }) {
                    return Err(RuntimeError::TypeError(format!(
                        "vm: reference-returning method '{fname}' produced {ret:?}"
                    )));
                }
                // Adapt while the completed method frame is still available.
                // A read-only or consuming receiver may return a reference into
                // that temporary frame, and adapter-time lifecycle code may
                // write through handles nested in the result. Receiver write-back
                // therefore follows adaptation rather than preceding it.
                let ret = self.apply_checked_result_adapter(
                    prog,
                    ret,
                    result_adapter,
                    returns_reference,
                    ResultAdapterFrames {
                        current: frame_id,
                        current_variables: vars,
                        returned: Some((returned_frame_id, &mut frame_vars)),
                    },
                )?;
                // `mut self`: write the (possibly mutated) receiver back.
                let is_mut = prog.structs.get(name).is_some_and(|d| {
                    let key = if fname == source_fname {
                        method
                    } else {
                        fname.as_str()
                    };
                    d.mut_self_methods.contains(key)
                });
                if is_mut
                    && !receiver_as_reference
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
}
