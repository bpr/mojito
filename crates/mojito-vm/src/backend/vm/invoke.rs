//! Call machinery: synchronous calls, caller mirrors,
//! argument binding, kwargs, and method dispatch.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

impl VmBackend {
    /// Execute a child while its caller is held by a structured MIR region rather
    /// than the continuation-driven frame stack. A temporary mirror with the
    /// caller's real frame identity makes ordinary frame/slot handles work
    /// unchanged: mutations are immediate even on raising paths, and references
    /// nested anywhere in the return value already point at the caller. The
    /// completed child identity is retained for references into its own receiver.
    /// Every synchronous call kind uses this one boundary.
    pub(super) fn call_synchronously(
        &mut self,
        prog: &Prog,
        function_index: usize,
        arguments: Vec<Value>,
        caller: CallerFrame<'_>,
    ) -> Result<(Value, Vec<Value>, FrameId), RuntimeError> {
        let CallerFrame {
            id: caller_id,
            function: _,
            registers: caller_registers,
            variables: caller_variables,
        } = caller;
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
                // A literal-filled `String` default reads as its text, as
                // the native backend's does: its constructor is a generator
                // over the literal, whose instance no call names.
                Some(CheckedConst::Construct { arg, .. })
                    if let CheckedConst::String(text) = &**arg
                        && prog
                            .declared_struct(sig.param_types.get(i))
                            .is_some_and(mojito_symbol::symbol::is_stdlib_string_struct) =>
                {
                    self.nominal_string_value(prog, text)
                }
                Some(CheckedConst::Construct {
                    target,
                    arg,
                    parameter,
                }) => self.call_named(
                    prog,
                    target,
                    vec![checked_const_value(arg, parameter.as_ref())?],
                    &[],
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
                Some(other) => checked_const_value(other, sig.param_types.get(i)),
                None => Err(RuntimeError::Unsupported(format!(
                    "vm: non-constant default for parameter '{}' of '{name}'",
                    sig.param_names[i]
                ))),
            }
        };
        bind_args(name, sig, argv, kwargs, make_default)
    }

    /// [`Self::bind_for_call`] of a call through a function value of
    /// `name`, whose leading arguments its captures fill. A value of a
    /// method passes the receiver its signature does not list first, in
    /// frame slot zero: a closure's storage, an initializer's `out self`,
    /// or the `self` argument of an instance method read through its type.
    pub(super) fn bind_for_closure_call(
        &mut self,
        prog: &Prog,
        name: &str,
        sig: &FnSig,
        argv: &[Value],
        kwargs: &[(String, Value)],
    ) -> Result<(Vec<Value>, Vec<ArgSlot>), RuntimeError> {
        let Some((receiver, arguments)) = argv.split_first().filter(|_| sig.has_receiver) else {
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
    /// `Type.method` function, which binds a by-address receiver as the
    /// reference its operand is; only primitive and compiler-private storage
    /// kinds retain intrinsic branches.
    #[allow(clippy::too_many_lines, reason = "TODO: split this pass")]
    pub(super) fn method_call(
        &mut self,
        prog: &Prog,
        invocation: MethodInvocation<'_>,
        frame: CallerFrame<'_>,
    ) -> Result<Value, RuntimeError> {
        let MethodInvocation {
            receiver: recv,
            receiver_ty,
            method,
            resolved_name: resolved,
            result_adapter,
            arguments: args,
            keyword_arguments: kwargs,
            receiver_operand,
            receiver_place,
        } = invocation;
        let CallerFrame {
            id: frame_id,
            function: caller_function,
            registers: regs,
            variables: vars,
        } = frame;
        // A receiver read out of a `ref`-typed field arrives as a reference
        // handle: dispatch on its referent.
        // A held struct receiver dispatches as its box, which the method's
        // frame entry unboxes.
        let recv = if prog.held_field(receiver_ty).is_some() {
            prog.boxed(recv, receiver_ty)
        } else if matches!(recv, Value::Ref { .. }) {
            let referent = self.read_reference(&recv, frame_id, vars)?;
            prog.boxed(referent, receiver_ty.map(peel_references))
        } else {
            recv
        };
        // The declared struct whose method the call runs: a struct value's
        // own, and for a value the VM holds as a builtin (a `SIMD` or a
        // `Bool`), the owner of the declared member the elaborator selected.
        let declared_owner = match &recv {
            Value::Struct { name, .. } => Some(name.clone()),
            _ => resolved
                .filter(|selected| prog.index_of(selected).is_some())
                .and_then(mojito_symbol::symbol::split_method_symbol)
                .map(|(owner, _)| owner.to_string()),
        };
        // `Floorable`/`Ceilable`/`Truncable` on a whole number; a struct with
        // its own implementation still dispatches to its method below.
        if declared_owner.is_none()
            && matches!(method, "__floor__" | "__ceil__" | "__trunc__")
            && args.is_empty()
        {
            return crate::runtime::builtin_round_dir(method, &recv);
        }
        match &recv {
            // A specialized pack's storage answers `Sized` where it stands:
            // `b.__len__()` is the element count `len(b)` reads, and the
            // clone has no nominal Tuple to lower the spelling onto.
            Value::Tuple(items) if method == "__len__" && args.is_empty() => {
                Ok(Value::Int(items.len() as i64))
            }
            Value::Tuple(_) => Err(RuntimeError::Unsupported(format!(
                "vm: internal tuple-pack storage has no runtime method '{method}'; public Tuple methods require nominal lowering"
            ))),
            // `Pointer` methods: `free()` releases the allocation (a no-op in
            // the arena model — the arena never reclaims), and `__int__()` is
            // the synthetic address.
            Value::Pointer { allocation, offset } => match method {
                "free" | "unsafe_free" => {
                    self.heap_free(*allocation, *offset)?;
                    Ok(Value::None)
                }
                "__int__" if args.is_empty() => Ok(Value::Int(crate::runtime::pointer_address(
                    *allocation,
                    *offset,
                ))),
                _ => Err(RuntimeError::Unsupported(format!(
                    "vm: Pointer has no method '{method}'"
                ))),
            },
            _ if let Some(name) = &declared_owner => {
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
                let bound = if let Some(signature) = prog.sigs.get(&fname) {
                    self.bind_for_call(prog, &fname, signature, &args, &kwargs)?
                        .0
                } else {
                    args
                };
                let function = &prog.mir.functions[fidx].1;
                // A by-address receiver enters as the reference its operand
                // is, as upstream's method binds its `!lit.ref` `self`; an
                // owned or `deinit` receiver is the value the call transfers,
                // and the call consumes a `deinit` receiver's place, as
                // upstream's call consumes a `DeinitMem` operand: the
                // callee's elaborated drops destroy its fields, and the
                // place holds the VM's ended-place tombstone from the call on.
                let deinit_receiver = function.deinit_params.first().copied().unwrap_or(false);
                let owned_receiver =
                    deinit_receiver || function.owned_params.first().copied().unwrap_or(false);
                let receiver = match receiver_operand {
                    Some(reference) if !owned_receiver => reference,
                    _ => recv.clone(),
                };
                if deinit_receiver && let Some(place) = receiver_place {
                    self.store_at_call_place(frame_id, place, Value::Moved, regs, vars)?;
                }
                let mut call_args = Vec::with_capacity(bound.len() + 1);
                call_args.push(receiver);
                call_args.extend(bound);
                check_reference_arguments(function, &fname, &call_args)?;
                let (ret, mut frame_vars, returned_frame_id) = self.call_synchronously(
                    prog,
                    fidx,
                    call_args,
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
                // write through handles nested in the result.
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
                Ok(ret)
            }
            other => Err(RuntimeError::Unsupported(format!(
                "vm backend does not support methods on {} yet",
                crate::runtime::type_name(other)
            ))),
        }
    }
}
