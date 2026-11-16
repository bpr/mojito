//! Explicit call-frame construction and dispatch: `call_frame`, `make_frame`,
//! the `drive_frames` loop over each frame's region cursor, `finish_frame`,
//! and `prepare_direct_call`.
//! Extracted from `backend/vm.rs`; see `docs/symbol-map.md`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

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
                self.raise_into_edge(
                    prog,
                    frame.function,
                    instruction,
                    outcome,
                    &mut frame.registers,
                    &mut frame.variables,
                )?;
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
                            self.raise_into_caller(prog, frame, error)?;
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
        let caller = self.frames.last_mut().ok_or_else(|| {
            RuntimeError::Unsupported("vm: returning child has no caller frame".to_string())
        })?;
        caller.registers[continuation.dest.0 as usize] = value;
        if let Some(raises) = &continuation.raises {
            caller.registers[raises.flag.0 as usize] = Value::Bool(false);
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
            let mut operand = |register: &Reg| {
                self.call_operand(
                    prog,
                    caller.function,
                    (caller.id, &caller.registers, &caller.variables),
                    *register,
                )
            };
            let mut positional = captured;
            positional.extend(args.iter().map(&mut operand));
            let keywords: Vec<(String, Value)> = kwargs
                .iter()
                .map(|(name, register)| (name.clone(), operand(register)))
                .collect();
            let (bound, _) = match prog.sigs.get(&function_name) {
                Some(signature) => self.bind_for_closure_call(
                    prog,
                    &function_name,
                    signature,
                    &positional,
                    &keywords,
                )?,
                None => (positional, Vec::new()),
            };
            let definition = &prog.mir.functions[index].1;
            check_reference_arguments(definition, &function_name, &bound)?;
            return self
                .make_frame(
                    prog,
                    index,
                    bound,
                    Some(ReturnContinuation {
                        dest: *dest,
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
        let mut operand = |register: &Reg| {
            self.call_operand(
                prog,
                caller.function,
                (caller.id, &caller.registers, &caller.variables),
                *register,
            )
        };
        let positional: Vec<Value> = args.iter().map(&mut operand).collect();
        let keywords: Vec<(String, Value)> = kwargs
            .iter()
            .map(|(name, register)| (name.clone(), operand(register)))
            .collect();
        let bound = match prog.sigs.get(&func.0) {
            Some(signature) => {
                self.bind_for_call(prog, &func.0, signature, &positional, &keywords)?
                    .0
            }
            None => positional,
        };
        check_reference_arguments(&prog.mir.functions[index].1, &func.0, &bound)?;
        self.make_frame(
            prog,
            index,
            bound,
            Some(ReturnContinuation {
                dest: *dest,
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
    /// heap pointer) and a by-value receiver its method writes back.
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
            && receiver_ty.is_some_and(Ty::is_pointer);
        if pointer_copy {
            return Ok(None);
        }
        let (raw, receiver_ty, receiver_operand) = self.receiver_operand(
            prog,
            caller.function,
            (caller.id, &caller.registers, &caller.variables),
            *recv,
        )?;
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
        let mut operand = |register: &Reg| {
            prog.boxed(
                self.call_operand(
                    prog,
                    caller.function,
                    (caller.id, &caller.registers, &caller.variables),
                    *register,
                ),
                reg_types.get(&register.0),
            )
        };
        let positional: Vec<Value> = args.iter().map(&mut operand).collect();
        let keywords: Vec<(String, Value)> = kwargs
            .iter()
            .map(|(key, register)| (key.clone(), operand(register)))
            .collect();
        let bound = match prog.sigs.get(&function_name) {
            Some(signature) => {
                self.bind_for_call(prog, &function_name, signature, &positional, &keywords)?
                    .0
            }
            None => positional,
        };
        // The receiver enters by the rule [`Self::method_call`] applies: a
        // by-address receiver as its reference, an owned one as its value,
        // and the call consumes a `deinit` receiver's place, as upstream's
        // call consumes a `DeinitMem` operand: the place holds the
        // ended-place tombstone from the call on.
        let deinit_receiver = definition.deinit_params.first().copied().unwrap_or(false);
        let owned_receiver =
            deinit_receiver || definition.owned_params.first().copied().unwrap_or(false);
        let receiver = match receiver_operand {
            Some(reference) if !owned_receiver => reference,
            _ => receiver,
        };
        if deinit_receiver && let Some(place) = recv_place.as_deref() {
            self.store_at_call_place(
                caller.id,
                place,
                Value::Moved,
                &caller.registers,
                &mut caller.variables,
            )?;
        }
        let mut call_args = Vec::with_capacity(bound.len() + 1);
        call_args.push(receiver);
        call_args.extend(bound);
        check_reference_arguments(definition, &function_name, &call_args)?;
        self.make_frame(
            prog,
            index,
            call_args,
            Some(ReturnContinuation {
                dest: *dest,
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
    /// a raise edge: set the edge's flag and write its error slot, as
    /// upstream's throwing call returns its error flag having written its
    /// `ByRefError` argument, so the caller goes on to its error region.
    fn raise_into_caller(
        &mut self,
        prog: &Prog,
        frame: Frame,
        error: Value,
    ) -> Result<(), RuntimeError> {
        let continuation = frame.continuation.ok_or_else(|| {
            RuntimeError::Unsupported("vm: child frame has no return continuation".to_string())
        })?;
        let mut caller = self.frames.pop().ok_or_else(|| {
            RuntimeError::Unsupported("vm: raising child has no caller frame".to_string())
        })?;
        if let Some(raises) = &continuation.raises {
            self.write_edge_error(
                prog,
                caller.function,
                raises,
                error,
                &mut caller.registers,
                &mut caller.variables,
            );
        }
        self.frames.push(caller);
        Ok(())
    }

    /// The outcome of `instruction` once its raise edge has taken a raise:
    /// a raise sets its error flag and writes its error slot
    /// ([`Self::write_edge_error`]), as upstream's throwing call returns
    /// its flag, and completion clears the flag. An instruction with no
    /// raise edge, and an error that is no raise, pass through.
    fn raise_into_edge(
        &mut self,
        prog: &Prog,
        function: usize,
        instruction: &MirInstr,
        outcome: Result<(), RuntimeError>,
        registers: &mut [Value],
        variables: &mut [Value],
    ) -> Result<(), RuntimeError> {
        let Some(raises) = instruction.raise_edge() else {
            return outcome;
        };
        match outcome {
            Err(RuntimeError::Raised(error)) => {
                self.write_edge_error(prog, function, raises, error, registers, variables);
                Ok(())
            }
            Ok(()) => {
                registers[raises.flag.0 as usize] = Value::Bool(false);
                Ok(())
            }
            Err(other) => Err(other),
        }
    }

    /// Take the raise `error` along the raise edge `raises` of `function`:
    /// set its flag and write the error into its slot, which a `try`'s
    /// slot catches.
    fn write_edge_error(
        &mut self,
        prog: &Prog,
        function: usize,
        raises: &Raises,
        error: Value,
        registers: &mut [Value],
        variables: &mut [Value],
    ) {
        if prog.error_slots[function].contains(&raises.error) {
            self.record_catch(&error);
        }
        registers[raises.flag.0 as usize] = Value::Bool(true);
        variables[raises.error as usize] = error;
    }
}
