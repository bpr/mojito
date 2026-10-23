//! Explicit call-frame construction and dispatch: `call_frame`, `make_frame`,
//! the `drive_frames` loop over each frame's region cursor, `finish_frame`,
//! and `prepare_direct_call`.
//! Extracted from `backend/vm.rs`; see `docs/symbol-map.md`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

impl VmBackend {
    /// Execute one function, returning its return value plus the final variable
    /// slots (so a `mut self` method call can recover the mutated receiver).
    /// Run a synchronous iterator-protocol call (`__iter__`/`__next__`/`__len__`)
    /// with the caller frame reachable, so a reference the callee holds into the
    /// caller — a user iterator's `ref[origin]` field rooted at the loop frame —
    /// resolves. `call_frame` otherwise drives the child while its caller is a
    /// local popped out of `self.frames` (unlike `prepare_direct_call`, which
    /// keeps the caller pushed). The caller's variables are *moved* into a shadow
    /// frame (no clone) for the duration of the call and moved back after, so the
    /// callee reads the caller's real storage — including any write-through — and
    /// per-iteration cost stays O(1).
    pub(super) fn call_frame_caller_reachable(
        &mut self,
        prog: &Prog,
        fidx: usize,
        args: Vec<Value>,
        frame_id: FrameId,
        function: usize,
        variables: &mut Vec<Value>,
    ) -> Result<(Value, Vec<Value>, FrameId), RuntimeError> {
        let shadow = Frame {
            id: frame_id,
            function,
            registers: Vec::new(),
            variables: std::mem::take(variables),
            cursor: RegionCursor::default(),
            continuation: None,
        };
        self.frames.push(shadow);
        let result = self.call_frame_with_id(prog, fidx, args);
        // `call_frame` truncates to the shadow on error and unwinds its own child
        // on success, leaving the shadow on top either way; move its storage back.
        if let Some(shadow) = self.frames.pop() {
            *variables = shadow.variables;
        }
        result
    }

    pub(super) fn call_frame(
        &mut self,
        prog: &Prog,
        fidx: usize,
        args: Vec<Value>,
    ) -> Result<(Value, Vec<Value>), RuntimeError> {
        self.call_frame_with_id(prog, fidx, args)
            .map(|(value, variables, _)| (value, variables))
    }

    /// Synchronous call result together with the identity of the completed
    /// frame. Reference-result adapters use that identity to distinguish a
    /// handle into the just-returned receiver from an unrelated stale handle.
    pub(super) fn call_frame_with_id(
        &mut self,
        prog: &Prog,
        fidx: usize,
        args: Vec<Value>,
    ) -> Result<(Value, Vec<Value>, FrameId), RuntimeError> {
        let frame = self.make_frame(prog, fidx, args, None)?;
        let target = frame.id;
        let stack_base = self.frames.len();
        self.frames.push(frame);
        let result = self.drive_frames(prog, target);
        if result.is_err() {
            // A raising child may leave suspended callers beneath it. The
            // synchronous caller handles the error from its own retained frame;
            // no abandoned child frame may leak into the next call.
            self.frames.truncate(stack_base);
        }
        result.map(|(value, variables)| (value, variables, target))
    }

    pub(super) fn make_frame(
        &mut self,
        prog: &Prog,
        fidx: usize,
        args: Vec<Value>,
        continuation: Option<ReturnContinuation>,
    ) -> Result<Frame, RuntimeError> {
        self.burn_ctfe()?;
        let f = &prog.mir.functions[fidx].1;
        // The MIR flattens only positional arguments; a mismatched count means a
        // default/keyword/`*args` form (not lowered yet) — refuse rather than
        // silently bind `None`.
        if args.len() != f.n_params {
            return Err(RuntimeError::Unsupported(format!(
                "vm backend does not support default/keyword/variadic arguments yet \
                 (call passed {} args to {}-parameter function '{}')",
                args.len(),
                f.n_params,
                prog.mir.functions[fidx].0,
            )));
        }
        let regs = vec![Value::None; f.n_regs as usize];
        let mut vars = vec![Value::None; f.n_vars];
        for (i, arg) in args.into_iter().enumerate() {
            vars[i] = match f.param_types.get(i) {
                Some(t) => crate::runtime::coerce_checked(arg, t),
                None => arg,
            };
        }
        let id = FrameId(self.next_frame_id);
        self.next_frame_id += 1;
        Ok(Frame {
            id,
            function: fidx,
            registers: regs,
            variables: vars,
            cursor: RegionCursor::default(),
            continuation,
        })
    }

    /// Step the frame stack until the frame `target` returns, running each
    /// frame's straight-line instructions until one moves it. A region op
    /// moves its frame's [`RegionCursor`] into one of its regions, and a
    /// direct call inside any of them pushes a child frame, so neither
    /// grows the host stack.
    pub(super) fn drive_frames(
        &mut self,
        prog: &Prog,
        target: FrameId,
    ) -> Result<(Value, Vec<Value>), RuntimeError> {
        'frames: loop {
            let mut frame = self.frames.pop().ok_or_else(|| {
                RuntimeError::Unsupported("vm: frame stack underflow".to_string())
            })?;
            let blocks = prog.mir.functions[frame.function].1.blocks.as_slice();
            let block = frame.cursor.block(blocks)?;
            while let Some(instruction) = block.instrs.get(frame.cursor.instruction()) {
                self.burn_ctfe()?;
                if frame.cursor.enter(instruction, &mut frame.registers) {
                    self.frames.push(frame);
                    continue 'frames;
                }
                frame.cursor.advance();
                if let Some(child) = self.prepare_direct_call(prog, &frame, instruction)? {
                    self.frames.push(frame);
                    self.frames.push(child);
                    continue 'frames;
                }
                let outcome = self.exec_instr(
                    prog,
                    instruction,
                    FrameScope {
                        function: frame.function,
                        id: frame.id,
                    },
                    &mut frame.registers,
                    &mut frame.variables,
                );
                raise_into_edge(instruction, outcome, &mut frame.registers)?;
            }
            self.burn_ctfe()?;
            let returned = match frame
                .cursor
                .transfer(blocks, &block.term, &mut frame.registers)?
            {
                Transfer::Moved => {
                    self.frames.push(frame);
                    continue;
                }
                Transfer::Return(reg) => {
                    reg.map_or(Value::None, |reg| frame.registers[reg.0 as usize].clone())
                }
                Transfer::Raise {
                    src,
                    label: Some(label),
                    values,
                } => {
                    let error =
                        match self.raise_register(prog, frame.function, &frame.registers, src) {
                            RuntimeError::Raised(error) => error,
                            raised => return Err(raised),
                        };
                    let slot = frame
                        .cursor
                        .catch(blocks, label, values, &mut frame.registers)?;
                    if let Value::Error(message) = &error {
                        self.record_lifecycle(format!("catch {message}"));
                    }
                    if let Some(slot) = slot {
                        frame.variables[slot as usize] = error;
                    }
                    self.frames.push(frame);
                    continue;
                }
                Transfer::Raise {
                    src, label: None, ..
                } => {
                    let raised = self.raise_register(prog, frame.function, &frame.registers, src);
                    match raised {
                        RuntimeError::Raised(error)
                            if frame.id != target
                                && frame
                                    .continuation
                                    .as_ref()
                                    .is_some_and(|continuation| continuation.raises.is_some()) =>
                        {
                            self.raise_into_caller(frame, error)?;
                            continue;
                        }
                        raised => return Err(raised),
                    }
                }
            };
            // A reference returned by projecting through a stored handle (a
            // `ref[origin]` field's element) — or buried inside a returned
            // aggregate's reference fields — is still rooted at this frame;
            // re-root it at the borrowed storage while this frame's variables
            // are live, so it survives the frame's disposal.
            let returned = {
                let mut returned = returned;
                self.canonicalize_value_references(frame.id, &frame.variables, &mut returned);
                returned
            };
            if let Some(done) = self.finish_frame(prog, target, frame, returned)? {
                return Ok(done);
            }
        }
    }

    pub(super) fn finish_frame(
        &mut self,
        prog: &Prog,
        target: FrameId,
        frame: Frame,
        value: Value,
    ) -> Result<Option<(Value, Vec<Value>)>, RuntimeError> {
        if frame.id == target {
            return Ok(Some((value, frame.variables)));
        }
        let continuation = frame.continuation.ok_or_else(|| {
            RuntimeError::Unsupported("vm: child frame has no return continuation".to_string())
        })?;
        let target_ty = self.frames.last().and_then(|caller| {
            prog.mir
                .functions
                .get(caller.function)
                .and_then(|(_, function)| function.reg_types.get(&continuation.dest.0))
        });
        let value = self.materialize_checked_result(prog, value, target_ty)?;
        // A written-back aggregate may carry reference handles rooted at this
        // frame (a `ref` field rebound inside the callee); re-root them at the
        // borrowed storage they project through before the frame is disposed,
        // exactly like the returned-value path above.
        let writebacks: Vec<_> = continuation
            .writebacks
            .into_iter()
            .map(|(parameter, place)| {
                let mut written = frame.variables[parameter].clone();
                self.canonicalize_value_references(frame.id, &frame.variables, &mut written);
                (place, written)
            })
            .collect();
        let caller = self.frames.last_mut().ok_or_else(|| {
            RuntimeError::Unsupported("vm: returning child has no caller frame".to_string())
        })?;
        caller.registers[continuation.dest.0 as usize] = value;
        if let Some(raises) = &continuation.raises {
            caller.registers[raises.flag.0 as usize] = Value::Bool(false);
        }
        for (place, written) in writebacks {
            *nav_mut(&mut caller.variables, &caller.registers, &place)? = written;
        }
        Ok(None)
    }

    #[allow(clippy::too_many_lines, reason = "TODO: split this pass")]
    pub(super) fn prepare_direct_call(
        &mut self,
        prog: &Prog,
        caller: &Frame,
        instruction: &MirInstr,
    ) -> Result<Option<Frame>, RuntimeError> {
        if let MirInstr::CallIndirect {
            dest,
            callee,
            resolved,
            args,
            kwargs,
            callee_place,
            arg_places,
            kwarg_places,
            ..
        } = instruction
        {
            let callable = &caller.registers[callee.0 as usize];
            let mut nominal_receiver = None;
            let (function_name, captured) = match callable {
                Value::Function(function_name) => (function_name.clone(), Vec::new()),
                Value::Closure {
                    function, captures, ..
                } => (
                    function.clone(),
                    Self::closure_capture_arguments(
                        caller.id,
                        &caller.registers,
                        &caller.variables,
                        callee_place.as_deref(),
                        captures,
                    )?,
                ),
                Value::Struct { name, .. } => {
                    nominal_receiver = Some(callable.clone());
                    let index = prog
                        .selected_struct_member(name, "__call__", resolved.as_deref(), args.len())
                        .ok_or_else(|| RuntimeError::NotCallable(format!("{name}.__call__")))?;
                    (prog.mir.functions[index].0.clone(), Vec::new())
                }
                value => {
                    return Err(RuntimeError::NotCallable(crate::runtime::type_name(value)));
                }
            };
            let index = prog
                .index_of(&function_name)
                .ok_or_else(|| RuntimeError::NotCallable(function_name.clone()))?;
            let capture_count = captured.len();
            let mut positional = captured;
            positional.extend(
                args.iter()
                    .map(|register| caller.registers[register.0 as usize].clone()),
            );
            let keywords = kwargs
                .iter()
                .map(|(name, register)| {
                    (name.clone(), caller.registers[register.0 as usize].clone())
                })
                .collect();
            let keyword_names: Vec<String> = kwargs.iter().map(|(name, _)| name.clone()).collect();
            let (mut bound, slots) = match prog.sigs.get(&function_name) {
                Some(signature) => {
                    self.bind_for_call(prog, &function_name, signature, &positional, keywords)?
                }
                None => (
                    positional,
                    (0..capture_count + args.len())
                        .map(ArgSlot::Positional)
                        .collect(),
                ),
            };
            let definition = &prog.mir.functions[index].1;
            if let Some(receiver) = nominal_receiver {
                for parameter in 1..definition.ref_params.len() {
                    let place = bound_argument_place(
                        slots.get(parameter - 1),
                        prog.sigs
                            .get(&function_name)
                            .and_then(|signature| signature.frame_param_name(parameter - 1)),
                        0,
                        arg_places,
                        &keyword_names,
                        kwarg_places,
                    );
                    let Some(place) = caller_handle_place(definition, parameter, place)
                        .map_err(|_| {
                            RuntimeError::Unsupported(format!(
                                "vm: a mut/ref argument to callable '{function_name}' must be a place"
                            ))
                        })?
                    else {
                        continue;
                    };
                    bound[parameter - 1] = self.reference_to_place(caller, place)?;
                }
                // As in the synchronous method path: a borrowed receiver whose
                // result carries reference fields also enters as a caller-place
                // handle, or the result's `ref` fields root in the callee frame.
                let receiver = if definition.ref_params.first().copied().unwrap_or(false) {
                    let place = callee_place.as_ref().ok_or_else(|| {
                        RuntimeError::Unsupported(format!(
                            "vm: reference receiver for callable '{function_name}' must be a place"
                        ))
                    })?;
                    self.reference_to_place(caller, place)?
                } else if let Some(place) = callee_place.as_ref()
                    && !definition.owned_params.first().copied().unwrap_or(false)
                    && !definition.deinit_params.first().copied().unwrap_or(false)
                    && Self::function_result_carries_reference(prog, index)
                {
                    self.reference_to_place(caller, place)?
                } else {
                    receiver
                };
                bound.insert(0, receiver);
            } else {
                for (parameter, is_ref) in definition.ref_params.iter().enumerate() {
                    let captured_argument = match slots.get(parameter) {
                        Some(ArgSlot::Positional(argument)) if *argument < capture_count => {
                            Some(*argument)
                        }
                        _ => None,
                    };
                    if let Some(argument) = captured_argument {
                        if *is_ref && !matches!(bound[parameter], Value::Ref { .. }) {
                            return Err(RuntimeError::TypeError(format!(
                                "vm: reference capture {argument} for '{function_name}' lost its handle"
                            )));
                        }
                        continue;
                    }
                    let place = bound_argument_place(
                        slots.get(parameter),
                        prog.sigs
                            .get(&function_name)
                            .and_then(|signature| signature.frame_param_name(parameter)),
                        capture_count,
                        arg_places,
                        &keyword_names,
                        kwarg_places,
                    );
                    let Some(place) = caller_handle_place(definition, parameter, place)
                        .map_err(|_| {
                            RuntimeError::Unsupported(format!(
                                "vm: a mut/ref argument to callable '{function_name}' must be a place"
                            ))
                        })?
                    else {
                        continue;
                    };
                    bound[parameter] = self.reference_to_place(caller, place)?;
                }
            }
            return self
                .make_frame(
                    prog,
                    index,
                    bound,
                    Some(ReturnContinuation {
                        dest: *dest,
                        writebacks: Vec::new(),
                        raises: instruction.raise_edge().cloned(),
                    }),
                )
                .map(Some);
        }
        if let MirInstr::MethodCall {
            dest,
            recv,
            method,
            resolved,
            result_adapter,
            args,
            kwargs,
            recv_place,
            arg_places,
            kwarg_places,
            spread,
            ..
        } = instruction
            && let Value::Struct { name, .. } = &caller.registers[recv.0 as usize]
        {
            let positional: Vec<Value> = args
                .iter()
                .map(|register| caller.registers[register.0 as usize].clone())
                .collect();
            refuse_pack_spread(*spread)?;
            let Some((function_name, index)) = prog
                .runtime_method_name(name, method, resolved.as_deref(), positional.len())
                .and_then(|function_name| {
                    let index = prog.index_of(&function_name)?;
                    Some((function_name, index))
                })
            else {
                return Ok(None);
            };
            // An adapted abstract result must be materialized while the caller
            // frame is directly available. Let the synchronous instruction path
            // perform the checked read/copy instead of installing the ordinary
            // raw-reference return continuation.
            if result_adapter.is_some() {
                return Ok(None);
            }
            if !prog.mir.functions[index].1.returns_reference {
                return Ok(None);
            }
            let keywords: Vec<(String, Value)> = kwargs
                .iter()
                .map(|(key, register)| (key.clone(), caller.registers[register.0 as usize].clone()))
                .collect();
            let keyword_names: Vec<String> = kwargs.iter().map(|(name, _)| name.clone()).collect();
            let (mut bound, slots) = match prog.sigs.get(&function_name) {
                Some(signature) => {
                    self.bind_for_call(prog, &function_name, signature, &positional, keywords)?
                }
                None => (
                    positional,
                    (0..args.len()).map(ArgSlot::Positional).collect(),
                ),
            };
            let definition = &prog.mir.functions[index].1;
            for parameter in 1..definition.ref_params.len() {
                let place = bound_argument_place(
                    slots.get(parameter - 1),
                    prog.sigs
                        .get(&function_name)
                        .and_then(|signature| signature.frame_param_name(parameter - 1)),
                    0,
                    arg_places,
                    &keyword_names,
                    kwarg_places,
                );
                let Some(place) =
                    caller_handle_place(definition, parameter, place).map_err(|_| {
                        RuntimeError::Unsupported(format!(
                            "vm: a mut/ref argument to method '{function_name}' must be a place"
                        ))
                    })?
                else {
                    continue;
                };
                bound[parameter - 1] = self.reference_to_place(caller, place)?;
            }
            // A ref-returning method's result handle roots in its receiver's
            // storage, so the receiver passes as a reference to the caller's
            // live slot — including a read (immutable) receiver, whose frame
            // value copy would otherwise root the handle in this dying frame.
            // A receiver without a place (an rvalue chain) still passes by
            // value for a read convention.
            let receiver = match recv_place.as_ref() {
                Some(place) => self.reference_to_place(caller, place)?,
                None if !definition.ref_params.first().copied().unwrap_or(false) => {
                    caller.registers[recv.0 as usize].clone()
                }
                None => {
                    return Err(RuntimeError::Unsupported(format!(
                        "vm: mutable receiver for '{function_name}' must be a place"
                    )));
                }
            };
            let mut call_args = Vec::with_capacity(bound.len() + 1);
            call_args.push(receiver);
            call_args.extend(bound);
            return self
                .make_frame(
                    prog,
                    index,
                    call_args,
                    Some(ReturnContinuation {
                        dest: *dest,
                        writebacks: Vec::new(),
                        raises: instruction.raise_edge().cloned(),
                    }),
                )
                .map(Some);
        }
        let MirInstr::Call {
            dest,
            func,
            args,
            kwargs,
            arg_places,
            kwarg_places,
            spread,
            ..
        } = instruction
        else {
            return Ok(None);
        };
        let constructed = prog.declared_struct(
            prog.mir
                .functions
                .get(caller.function)
                .and_then(|(_, function)| function.reg_types.get(&dest.0)),
        );
        if prog.lifecycle_constructor(&func.0, constructed).is_some() {
            return Ok(None);
        }
        let Some(index) = prog.index_of(&func.0) else {
            return Ok(None);
        };
        let positional: Vec<Value> = args
            .iter()
            .map(|reg| caller.registers[reg.0 as usize].clone())
            .collect();
        refuse_pack_spread(*spread)?;
        let keywords: Vec<(String, Value)> = kwargs
            .iter()
            .map(|(name, reg)| (name.clone(), caller.registers[reg.0 as usize].clone()))
            .collect();
        let keyword_names: Vec<String> = kwargs.iter().map(|(name, _)| name.clone()).collect();
        let (mut bound, slots) = match prog.sigs.get(&func.0) {
            Some(signature) => {
                self.bind_for_call(prog, &func.0, signature, &positional, keywords)?
            }
            None => (
                positional,
                (0..args.len()).map(ArgSlot::Positional).collect(),
            ),
        };
        let function = &prog.mir.functions[index].1;
        for (parameter, _) in function.ref_params.iter().enumerate() {
            let place = bound_argument_place(
                slots.get(parameter),
                prog.sigs
                    .get(&func.0)
                    .and_then(|signature| signature.frame_param_name(parameter)),
                0,
                arg_places,
                &keyword_names,
                kwarg_places,
            );
            let Some(place) = caller_handle_place(function, parameter, place).map_err(|_| {
                RuntimeError::Unsupported(format!(
                    "vm: a mut/ref argument to '{}' must be a place",
                    func.0
                ))
            })?
            else {
                continue;
            };
            bound[parameter] = self.reference_to_place(caller, place)?;
        }
        self.make_frame(
            prog,
            index,
            bound,
            Some(ReturnContinuation {
                dest: *dest,
                writebacks: Vec::new(),
                raises: instruction.raise_edge().cloned(),
            }),
        )
        .map(Some)
    }

    /// Pop a child frame a `raise` leaves into its caller, whose call has
    /// a raise edge: set the edge's flag and error, as upstream's throwing
    /// call returns its error flag, and apply the call's write-backs, so
    /// the caller goes on to its error region.
    fn raise_into_caller(&mut self, frame: Frame, error: Value) -> Result<(), RuntimeError> {
        let continuation = frame.continuation.ok_or_else(|| {
            RuntimeError::Unsupported("vm: child frame has no return continuation".to_string())
        })?;
        let writebacks: Vec<_> = continuation
            .writebacks
            .into_iter()
            .map(|(parameter, place)| {
                let mut written = frame.variables[parameter].clone();
                self.canonicalize_value_references(frame.id, &frame.variables, &mut written);
                (place, written)
            })
            .collect();
        let caller = self.frames.last_mut().ok_or_else(|| {
            RuntimeError::Unsupported("vm: raising child has no caller frame".to_string())
        })?;
        if let Some(raises) = &continuation.raises {
            caller.registers[raises.flag.0 as usize] = Value::Bool(true);
            caller.registers[raises.error.0 as usize] = error;
        }
        for (place, written) in writebacks {
            *nav_mut(&mut caller.variables, &caller.registers, &place)? = written;
        }
        Ok(())
    }
}

/// The outcome of `instruction` once its raise edge has taken a raise: a
/// raise sets its error flag and error, as upstream's throwing call returns
/// its flag, and completion clears the flag. An instruction with no raise
/// edge, and an error that is no raise, pass through.
fn raise_into_edge(
    instruction: &MirInstr,
    outcome: Result<(), RuntimeError>,
    registers: &mut [Value],
) -> Result<(), RuntimeError> {
    let Some(raises) = instruction.raise_edge() else {
        return outcome;
    };
    match outcome {
        Err(RuntimeError::Raised(error)) => {
            registers[raises.flag.0 as usize] = Value::Bool(true);
            registers[raises.error.0 as usize] = error;
            Ok(())
        }
        Ok(()) => {
            registers[raises.flag.0 as usize] = Value::Bool(false);
            Ok(())
        }
        Err(other) => Err(other),
    }
}
