//! Call machinery: writeback/synchronous calls, caller mirrors,
//! argument binding, kwargs, and method dispatch.

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
        let function = &prog.mir.functions[idx].1;
        let ref_params = function.ref_params.clone();
        let mut reference_inputs = Vec::new();
        for (i, is_ref) in ref_params.iter().enumerate() {
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
            if !is_ref {
                // A shared-read place lent to a borrowing-view result binds
                // the caller's storage too (see the continuation path).
                if let Some(place) = place
                    && !function.owned_params.get(i).copied().unwrap_or(false)
                    && !function.deinit_params.get(i).copied().unwrap_or(false)
                {
                    reference_inputs.push((
                        i,
                        Self::reference_to_place_parts(frame_id, regs, vars, place)?,
                    ));
                }
                continue;
            }
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
        let (result, _, _) = self.call_synchronously_with_references(
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
        let outcome = self.call_frame_with_id(prog, function_index, arguments, value_params);
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
            block: 0,
            instruction: 0,
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
        argv: Vec<Value>,
        kwargs: Vec<(String, Value)>,
    ) -> Result<(Vec<Value>, Vec<ArgSlot>), RuntimeError> {
        let mut expanded = Vec::new();
        let mut forwarded = false;
        for (name, value) in kwargs {
            if name == mojito_ast::ast::FORWARDED_KWARGS_NAME {
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
        // Materialize omitted defaults. A `Construct` default runs its
        // converting constructor (e.g. the empty `Optional[T]` for a `None`
        // default) through the same path an explicit `f(arg=None)` takes;
        // scalars fold directly; a non-constant default without a construction
        // errors only when its slot is actually taken.
        let make_default = |i: usize| -> Result<Value, RuntimeError> {
            match &sig.defaults[i] {
                Some(CheckedConst::Construct { target, arg }) => self.call_named(
                    prog,
                    target,
                    vec![checked_const_value(arg)],
                    vec![],
                    &[],
                    &[],
                ),
                Some(other) => Ok(checked_const_value(other)),
                None => Err(RuntimeError::Unsupported(format!(
                    "vm: non-constant default for parameter '{}' of '{name}'",
                    sig.param_names[i]
                ))),
            }
        };
        let (mut bound, slots) = bind_args(name, sig, argv, kwargs, make_default)?;
        if let Some(index) = sig.kw_variadic_index {
            bound[index] = self.make_kwargs_dict(prog, collected)?;
        }
        Ok((bound, slots))
    }

    pub(super) fn make_kwargs_dict(
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
    pub(super) fn take_forwarded_kwargs(
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
            parameter_arguments: param_arg_regs,
            parameter_declarations: param_decls,
            argument_types: arg_types,
        } = invocation;
        let CallerFrame {
            id: frame_id,
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
        // Trait-dispatched `Copyable.copy` on a non-struct value (a scalar or
        // built-in aggregate reaching `__trait_dispatch.copy` through a
        // generic body) is the value itself; concrete built-in receivers never
        // reach here (the checker resolves them to the value read).
        if method == "copy" && !matches!(recv, Value::Struct { .. }) {
            return Ok(recv.clone());
        }
        // Struct-to-literal bridge: the nominal String's `_as_string_literal`
        // reads the byte buffer back into a compile-time string value; the
        // declared body never executes.
        if method == "_as_string_literal"
            && let Value::Struct { name, .. } = &recv
            && mojito_symbol::symbol::is_stdlib_string_struct(name)
        {
            return self.string_struct_literal(&recv);
        }
        // `format` on a nominal String receiver reads the template back
        // through the bridge and runs the builtin template formatter; the
        // checker's recorded wrap materializes the literal result.
        if method == "format"
            && let Value::Struct { name, .. } = &recv
            && mojito_symbol::symbol::is_stdlib_string_struct(name)
        {
            let Value::Str(template) = self.string_struct_literal(&recv)? else {
                unreachable!("the string bridge reads back a literal");
            };
            return self.format_template(prog, &template, &args).map(Value::Str);
        }
        let keyword_names: Vec<String> = kwargs.iter().map(|(name, _)| name.clone()).collect();
        // Intrinsic dunders on a built-in numeric/hashable value; a struct with
        // its own implementation still dispatches to its method below.
        if !matches!(recv, Value::Struct { .. }) {
            match (method, args.len()) {
                // Hashable scalar leaf: normalize its bits and contribute them
                // to the caller-owned hasher through `_update_with_simd`.
                ("__hash__", 1) => {
                    // A string literal hashes as the nominal `String` it
                    // materializes to, so literal and nominal keys agree.
                    if let Value::Str(text) = &recv {
                        let text = text.clone();
                        let materialized = self.nominal_string_value(prog, &text)?;
                        if matches!(materialized, Value::Struct { .. }) {
                            return self.method_call(
                                prog,
                                MethodInvocation {
                                    receiver: materialized,
                                    method,
                                    resolved_name: None,
                                    result_adapter,
                                    arguments: args,
                                    keyword_arguments: kwargs,
                                    receiver_place: recv_place,
                                    argument_places: arg_places,
                                    keyword_argument_places: kwarg_places,
                                    parameter_arguments: param_arg_regs,
                                    parameter_declarations: param_decls,
                                    argument_types: Vec::new(),
                                },
                                CallerFrame {
                                    id: frame_id,
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
                    let fname = prog.runtime_method_name(name, "_update_with_simd", None, 1);
                    let fidx = prog.index_of(&fname).ok_or_else(|| {
                        RuntimeError::Unsupported(format!(
                            "vm: Hasher implementation has no '{fname}'"
                        ))
                    })?;
                    let bits = crate::runtime::hash_bits(&recv)?;
                    let contribution = Value::Simd {
                        dtype: mojito_ast::ast::Dtype::UInt64,
                        lanes: crate::runtime::SimdLanes::Int(vec![i128::from(bits)]),
                    };
                    let (_, variables) =
                        self.call_frame(prog, fidx, vec![hasher, contribution], &[])?;
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
                _ => {}
            }
        }
        match &recv {
            Value::Str(template) if method == "format" => {
                self.format_template(prog, template, &args).map(Value::Str)
            }
            Value::Str(current) if method == "write" => {
                let place = recv_place.as_ref().ok_or_else(|| {
                    RuntimeError::Unsupported("vm: Writer.write needs a mutable place".into())
                })?;
                let mut text = current.clone();
                for (index, argument) in args.into_iter().enumerate() {
                    let static_ty = arg_types.get(index).and_then(Option::as_ref);
                    text.push_str(&self.format_value(prog, argument, false, static_ty)?);
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
                // Always the three normalized bounds: the checked destination
                // type (`materialize_checked_result`) trims the private pack to
                // `ContiguousSlice.indices`' two-element `(start, end)`, since
                // the runtime kind of a widened literal is not the receiver's
                // checked descriptor type.
                Ok(Value::Tuple(vec![
                    Value::Int(start),
                    Value::Int(end),
                    Value::Int(step),
                ]))
            }
            Value::Slice { .. } if matches!(method, "__eq__" | "__ne__") && args.len() == 1 => {
                let equal = crate::runtime::values_equal(&recv, &args[0])?;
                Ok(Value::Bool(if method == "__eq__" { equal } else { !equal }))
            }
            Value::Slice { kind, .. } => Err(RuntimeError::Unsupported(format!(
                "vm: {} has no method '{method}'",
                kind.type_name()
            ))),
            Value::Simd { dtype, lanes } => {
                crate::runtime::simd_method(*dtype, lanes, method, &args)
            }
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
                // A `write_string` declaring the nominal String receives a
                // materialized struct; the literal spelling keeps `Value::Str`.
                let nominal_payload = prog
                    .sigs
                    .get(&format!("{name}.write_string"))
                    .and_then(|signature| signature.param_types.first())
                    .is_some_and(|ty| {
                        matches!(ty, Ty::Struct(payload, args)
                        if args.is_empty() && mojito_symbol::symbol::is_stdlib_string_struct(payload))
                    });
                for (position, argument) in args.into_iter().enumerate() {
                    let static_ty = arg_types.get(position).and_then(Option::as_ref);
                    let text = self.format_value(prog, argument, false, static_ty)?;
                    let payload = if nominal_payload {
                        self.nominal_string_value(prog, &text)?
                    } else {
                        Value::Str(text)
                    };
                    let (_, variables) =
                        self.call_frame(prog, index, vec![writer, payload], &[])?;
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
                let function = &prog.mir.functions[fidx].1;
                let ref_params = &function.ref_params;
                // A borrowed receiver whose result carries reference fields
                // must also enter as a caller-place handle: a value copy would
                // root the result's `ref` fields in the callee frame, and
                // return-time canonicalization has no caller storage to re-root
                // them through. Owned/deinit receivers keep the value transfer.
                let receiver_as_reference = ref_params.first().copied().unwrap_or(false)
                    || (recv_place.is_some()
                        && !function.owned_params.first().copied().unwrap_or(false)
                        && !function.deinit_params.first().copied().unwrap_or(false)
                        && Self::function_result_carries_reference(prog, fidx));
                let mut reference_inputs = Vec::new();
                if receiver_as_reference {
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
                    if !is_ref {
                        // A shared-read place lent to a borrowing-view result
                        // binds the caller's storage (as for free functions).
                        if let Some(place) = place
                            && !function.owned_params.get(i).copied().unwrap_or(false)
                            && !function.deinit_params.get(i).copied().unwrap_or(false)
                        {
                            reference_inputs.push((
                                i,
                                Self::reference_to_place_parts(frame_id, regs, vars, place)?,
                            ));
                        }
                        continue;
                    }
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
                let (ret, mut frame_vars, returned_frame_id) = self
                    .call_synchronously_with_references(
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
                let returns_reference = prog.mir.functions[fidx].1.returns_reference;
                if returns_reference && !matches!(ret, Value::Ref { .. }) {
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
                    let key = if fname != source_fname {
                        fname.as_str()
                    } else {
                        method
                    };
                    d.mut_self_methods.contains(key)
                });
                // A named destructor (`deinit self`) also writes its final
                // receiver state back: the caller's trailing consumption then
                // destroys exactly the residual fields the body left, instead
                // of a stale pre-call clone (which would re-drop moved fields
                // and double-free drained pointer-backed containers).
                let is_named_destructor = prog.mir.functions[fidx]
                    .1
                    .deinit_params
                    .first()
                    .copied()
                    .unwrap_or(false);
                if (is_mut || is_named_destructor)
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
