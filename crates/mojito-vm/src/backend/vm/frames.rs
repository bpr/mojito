//! Explicit call-frame construction and dispatch: `call_frame`, `make_frame`,
//! the `drive_frames` loop over each frame's region cursor, `finish_frame`,
//! and `prepare_direct_call`.
//! Extracted from `backend/vm.rs`; see `docs/symbol-map.md`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::TyKind;

impl VmBackend {
    /// Execute one function, returning its return value plus the final variable
    /// slots (so a `mut self` method call can recover the mutated receiver).
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
        // A boxed operand of a held struct parameter (a reboxed receiver,
        // an initializer's skeleton) enters as its field.
        for (i, arg) in args.into_iter().enumerate() {
            vars[i] = match f.param_types.get(i) {
                Some(t) => prog.unboxed(crate::runtime::coerce_checked(arg, t), Some(t)),
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
            let body = &prog.mir.functions[frame.function].1.body;
            let block = frame.cursor.block(body)?;
            while let Some(instruction) = block.instrs.get(frame.cursor.instruction()) {
                self.burn_ctfe()?;
                if frame.cursor.enter(instruction, &mut frame.registers) {
                    self.frames.push(frame);
                    continue 'frames;
                }
                frame.cursor.advance();
                if let Some(child) = self.prepare_direct_call(prog, &mut frame, instruction)? {
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
                .transfer(body, &block.term, &mut frame.registers)?
            {
                Transfer::Moved => {
                    self.frames.push(frame);
                    continue;
                }
                Transfer::Return(reg) => {
                    reg.map_or(Value::None, |reg| frame.registers[reg.0 as usize].clone())
                }
                Transfer::TryRaise { label, values } => {
                    frame
                        .cursor
                        .catch(body, label, values, &mut frame.registers)?;
                    self.frames.push(frame);
                    continue;
                }
                Transfer::ErrorReturn => {
                    let raised = prog.mir.functions[frame.function]
                        .1
                        .error_slot
                        .and_then(|slot| frame.variables.get_mut(slot as usize))
                        .map_or_else(
                            || {
                                RuntimeError::Unsupported(
                                    "vm: a raise out of a function with no error slot".to_string(),
                                )
                            },
                            |slot| RuntimeError::Raised(std::mem::replace(slot, Value::Moved)),
                        );
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
        mut frame: Frame,
        value: Value,
    ) -> Result<Option<(Value, Vec<Value>)>, RuntimeError> {
        if frame.id == target {
            return Ok(Some((value, frame.variables)));
        }
        let mut continuation = frame.continuation.take().ok_or_else(|| {
            RuntimeError::Unsupported("vm: child frame has no return continuation".to_string())
        })?;
        let value = match continuation.result_adapter.take() {
            Some(adapter) => {
                self.adapt_returned_result(prog, &mut frame, continuation.dest, &adapter, value)?
            }
            None => value,
        };
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
        caller: &mut Frame,
        instruction: &MirInstr,
    ) -> Result<Option<Frame>, RuntimeError> {
        if let MirInstr::CallIndirect {
            dest,
            callee,
            args,
            kwargs,
            arg_places,
            kwarg_places,
            ..
        } = instruction
        {
            let callable = &caller.registers[callee.0 as usize];
            let (function_name, captured) = match callable {
                Value::Function(function_name) => (function_name.clone(), Vec::new()),
                Value::Closure {
                    function, captures, ..
                } => (function.clone(), captures.clone()),
                value => {
                    return Err(RuntimeError::NotCallable(crate::runtime::type_name(value)));
                }
            };
            let index = prog
                .index_of(&function_name)
                .ok_or_else(|| RuntimeError::NotCallable(function_name.clone()))?;
            // An initializer's result is its receiver, which the synchronous
            // path reads back.
            if Self::initializer_receiver(prog, index).is_some() {
                return Ok(None);
            }
            let capture_count = captured.len();
            let mut positional = captured;
            positional.extend(
                args.iter()
                    .map(|register| caller.registers[register.0 as usize].clone()),
            );
            let keywords: Vec<(String, Value)> = kwargs
                .iter()
                .map(|(name, register)| {
                    (name.clone(), caller.registers[register.0 as usize].clone())
                })
                .collect();
            let keyword_names: Vec<String> = kwargs.iter().map(|(name, _)| name.clone()).collect();
            let (mut bound, slots) = match prog.sigs.get(&function_name) {
                Some(signature) => self.bind_for_closure_call(
                    prog,
                    &function_name,
                    signature,
                    &positional,
                    &keywords,
                )?,
                None => (
                    positional,
                    (0..capture_count + args.len())
                        .map(ArgSlot::Positional)
                        .collect(),
                ),
            };
            let definition = &prog.mir.functions[index].1;
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
                    prog.sigs.get(&function_name).and_then(|signature| {
                        // A method's receiver, which its signature does
                        // not list, is the value's first argument.
                        signature.frame_param_name(
                            parameter.checked_sub(usize::from(signature.has_receiver))?,
                        )
                    }),
                    capture_count,
                    arg_places,
                    &keyword_names,
                    kwarg_places,
                );
                let Some(place) =
                    caller_handle_place(definition, parameter, place).map_err(|_| {
                        RuntimeError::Unsupported(format!(
                            "vm: a mut/ref argument to callable '{function_name}' must be a place"
                        ))
                    })?
                else {
                    continue;
                };
                bound[parameter] = self.reference_argument(
                    caller.id,
                    &caller.registers,
                    &caller.variables,
                    place,
                )?;
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
                        result_adapter: None,
                    }),
                )
                .map(Some);
        }
        if matches!(instruction, MirInstr::MethodCall { .. }) {
            return self.prepare_method_call(prog, caller, instruction);
        }
        let MirInstr::Call {
            dest,
            func,
            args,
            kwargs,
            arg_places,
            kwarg_places,
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
        // An initializer of a receiver held as a value returns its `out self`
        // slot, which only the synchronous construction reads.
        if matches!(
            prog.constructed(&func.0, &prog.mir.functions[index].1.param_types),
            Some(Constructed::Value(_))
        ) {
            return Ok(None);
        }
        let positional: Vec<Value> = args
            .iter()
            .map(|reg| caller.registers[reg.0 as usize].clone())
            .collect();
        let keywords: Vec<(String, Value)> = kwargs
            .iter()
            .map(|(name, reg)| (name.clone(), caller.registers[reg.0 as usize].clone()))
            .collect();
        let keyword_names: Vec<String> = kwargs.iter().map(|(name, _)| name.clone()).collect();
        let (mut bound, slots) = match prog.sigs.get(&func.0) {
            Some(signature) => {
                self.bind_for_call(prog, &func.0, signature, &positional, &keywords)?
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
            bound[parameter] =
                self.reference_argument(caller.id, &caller.registers, &caller.variables, place)?;
        }
        self.make_frame(
            prog,
            index,
            bound,
            Some(ReturnContinuation {
                dest: *dest,
                writebacks: Vec::new(),
                raises: instruction.raise_edge().cloned(),
                result_adapter: None,
            }),
        )
        .map(Some)
    }

    /// Push a frame for a method call whose receiver is a declared struct's
    /// value, as upstream lowers the call to an ordinary `kgen.call` of the
    /// method's function: the receiver enters by the rule
    /// [`Self::method_call`] applies, and the call's result adapter runs
    /// when the frame returns. `None` leaves the call to the synchronous
    /// path: an intrinsic receiver (a place pointer's `copy`, a pack, a
    /// heap pointer) and a `mut self` receiver held by value.
    #[allow(clippy::too_many_lines, reason = "TODO: split this pass")]
    fn prepare_method_call(
        &mut self,
        prog: &Prog,
        caller: &mut Frame,
        instruction: &MirInstr,
    ) -> Result<Option<Frame>, RuntimeError> {
        let MirInstr::MethodCall {
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
            ..
        } = instruction
        else {
            return Ok(None);
        };
        let reg_types = &prog.mir.functions[caller.function].1.reg_types;
        let receiver_ty = reg_types.get(&recv.0);
        let pointer_copy = method == "copy"
            && resolved.is_none()
            && args.is_empty()
            && kwargs.is_empty()
            && matches!(receiver_ty.map(Ty::kind), Some(TyKind::Pointer { .. }));
        if pointer_copy {
            return Ok(None);
        }
        let raw = caller.registers[recv.0 as usize].clone();
        let receiver = if prog.held_field(receiver_ty).is_some() {
            prog.boxed(raw, receiver_ty)
        } else if matches!(raw, Value::Ref { .. }) {
            let referent = self.read_reference(&raw, caller.id, &caller.variables)?;
            prog.boxed(referent, receiver_ty.map(peel_references))
        } else {
            raw
        };
        let owner = match &receiver {
            Value::Struct { name, .. } => name.clone(),
            Value::Tuple(_) | Value::Pointer { .. } => return Ok(None),
            _ => match resolved
                .as_deref()
                .filter(|selected| prog.index_of(selected).is_some())
                .and_then(mojito_symbol::symbol::split_method_symbol)
            {
                Some((owner, _)) => owner.to_string(),
                None => return Ok(None),
            },
        };
        let Some((function_name, index)) = prog
            .runtime_method_name(&owner, method, resolved.as_deref(), args.len())
            .and_then(|function_name| {
                let index = prog.index_of(&function_name)?;
                Some((function_name, index))
            })
        else {
            return Ok(None);
        };
        let definition = &prog.mir.functions[index].1;
        let returns_reference = definition.returns_reference;
        let reference_receiver = definition.ref_params.first().copied().unwrap_or(false);
        let deinit_receiver = definition.deinit_params.first().copied().unwrap_or(false);
        // A ref-returning method's result handle roots in its receiver's
        // storage, so a receiver with a place passes as a reference to the
        // caller's live slot — including a read receiver, whose frame value
        // copy would otherwise root the handle in this dying frame. So does
        // a borrowed receiver whose result carries reference fields; an
        // owned or deinit receiver otherwise keeps its value transfer.
        let receiver_as_reference = if returns_reference && result_adapter.is_none() {
            recv_place.is_some()
        } else {
            reference_receiver
                || (recv_place.is_some()
                    && !definition.owned_params.first().copied().unwrap_or(false)
                    && !deinit_receiver
                    && Self::function_result_carries_reference(prog, index))
        };
        // A `mut self` the method does not take as a reference parameter
        // is written back from its returned value by the synchronous path.
        let source_name = format!("{owner}.{method}");
        let mut_receiver = prog.structs.get(&owner).is_some_and(|declaration| {
            let key = if function_name == source_name {
                method.as_str()
            } else {
                function_name.as_str()
            };
            declaration.mut_self_methods.contains(key)
        });
        if mut_receiver && !receiver_as_reference {
            return Ok(None);
        }
        let operand = |register: &Reg| {
            prog.boxed(
                caller.registers[register.0 as usize].clone(),
                reg_types.get(&register.0),
            )
        };
        let positional: Vec<Value> = args.iter().map(operand).collect();
        let keywords: Vec<(String, Value)> = kwargs
            .iter()
            .map(|(key, register)| (key.clone(), operand(register)))
            .collect();
        let keyword_names: Vec<String> = kwargs.iter().map(|(name, _)| name.clone()).collect();
        let (mut bound, slots) = match prog.sigs.get(&function_name) {
            Some(signature) => {
                self.bind_for_call(prog, &function_name, signature, &positional, &keywords)?
            }
            None => (
                positional,
                (0..args.len()).map(ArgSlot::Positional).collect(),
            ),
        };
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
            let Some(place) = caller_handle_place(definition, parameter, place).map_err(|_| {
                RuntimeError::Unsupported(format!(
                    "vm: a mut/ref argument to method '{function_name}' must be a place"
                ))
            })?
            else {
                continue;
            };
            bound[parameter - 1] =
                self.reference_argument(caller.id, &caller.registers, &caller.variables, place)?;
        }
        let receiver = if receiver_as_reference {
            let place = recv_place.as_deref().ok_or_else(|| {
                RuntimeError::Unsupported(format!(
                    "vm: reference receiver for method '{function_name}' must be a place"
                ))
            })?;
            self.reference_argument(caller.id, &caller.registers, &caller.variables, place)?
        } else {
            // The call consumes its `deinit` receiver's place, as upstream's
            // call consumes a `DeinitMem` operand: the place holds the
            // ended-place tombstone from the call on.
            if deinit_receiver && let Some(place) = recv_place.as_deref() {
                self.store_at_call_place(
                    caller.id,
                    place,
                    Value::Moved,
                    &caller.registers,
                    &mut caller.variables,
                )?;
            }
            receiver
        };
        let mut call_args = Vec::with_capacity(bound.len() + 1);
        call_args.push(receiver);
        call_args.extend(bound);
        self.make_frame(
            prog,
            index,
            call_args,
            Some(ReturnContinuation {
                dest: *dest,
                writebacks: Vec::new(),
                raises: instruction.raise_edge().cloned(),
                result_adapter: result_adapter
                    .as_deref()
                    .map(|adapter| ContinuationAdapter {
                        adapter: adapter.clone(),
                        returns_reference,
                    }),
            }),
        )
        .map(Some)
    }

    /// Apply a returning method frame's result adapter, as the synchronous
    /// method call does: the caller's variables are the current frame, and
    /// a reference into the returned receiver reads from `frame`'s
    /// variables before they are discarded.
    fn adapt_returned_result(
        &mut self,
        prog: &Prog,
        frame: &mut Frame,
        dest: Reg,
        adapter: &ContinuationAdapter,
        value: Value,
    ) -> Result<Value, RuntimeError> {
        let mut caller = self.frames.pop().ok_or_else(|| {
            RuntimeError::Unsupported("vm: returning child has no caller frame".to_string())
        })?;
        let ty = prog
            .mir
            .functions
            .get(caller.function)
            .and_then(|(_, function)| function.reg_types.get(&dest.0));
        let adapted = self.apply_checked_result_adapter(
            prog,
            value,
            Some(ResultAdapter {
                kind: adapter.adapter.kind,
                constructor: adapter.adapter.constructor.as_deref(),
                ty,
            }),
            adapter.returns_reference,
            ResultAdapterFrames {
                current: caller.id,
                current_variables: &mut caller.variables,
                returned: Some((frame.id, &mut frame.variables)),
            },
        );
        self.frames.push(caller);
        adapted
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
