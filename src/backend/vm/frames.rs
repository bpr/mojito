//! Explicit call-frame construction and dispatch: `call_frame`, `make_frame`,
//! the `drive_frames` loop, `finish_frame`, and `prepare_direct_call`.
//! Extracted from `backend/vm.rs`; see `docs/symbol-map.md`.

use super::*;

impl VmBackend {
    /// Execute one function, returning its return value plus the final variable
    /// slots (so a `mut self` method call can recover the mutated receiver).
    pub(super) fn call_frame(
        &mut self,
        prog: &Prog,
        fidx: usize,
        args: Vec<Value>,
        value_params: &[(String, Value)],
    ) -> Result<(Value, Vec<Value>), RuntimeError> {
        let frame = self.make_frame(prog, fidx, args, value_params, None)?;
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
        result
    }

    pub(super) fn make_frame(
        &mut self,
        prog: &Prog,
        fidx: usize,
        args: Vec<Value>,
        value_params: &[(String, Value)],
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
        // Bind reified value parameters (a value-parameterized generic function's
        // comptime `Int` params) into their body var slots, resolved by name — the
        // body reads them as ordinary `Int` locals (`return n * 2`).
        for (pname, val) in value_params {
            if let Some(slot) = f.var_names.iter().position(|n| n == pname) {
                vars[slot] = match f.var_tys.get(&(slot as u32)) {
                    Some(ty) => crate::runtime::coerce_checked(val.clone(), ty),
                    None => val.clone(),
                };
            }
        }

        let id = FrameId(self.next_frame_id);
        self.next_frame_id += 1;
        Ok(Frame {
            id,
            function: fidx,
            registers: regs,
            variables: vars,
            block: 0,
            instruction: 0,
            continuation,
        })
    }

    pub(super) fn drive_frames(
        &mut self,
        prog: &Prog,
        target: FrameId,
    ) -> Result<(Value, Vec<Value>), RuntimeError> {
        loop {
            let mut frame = self.frames.pop().ok_or_else(|| {
                RuntimeError::Unsupported("vm: frame stack underflow".to_string())
            })?;
            self.burn_ctfe()?;
            let function = &prog.mir.functions[frame.function].1;
            let block = &function.blocks[frame.block];
            if frame.instruction < block.instrs.len() {
                let instruction = block.instrs[frame.instruction].clone();
                frame.instruction += 1;
                if let Some(child) = self.prepare_direct_call(prog, &frame, &instruction)? {
                    self.frames.push(frame);
                    self.frames.push(child);
                    continue;
                }
                match self.exec_instr(
                    prog,
                    &instruction,
                    frame.function,
                    frame.id,
                    &mut frame.registers,
                    &mut frame.variables,
                )? {
                    Flow::Normal => {
                        self.frames.push(frame);
                        continue;
                    }
                    Flow::Jump(block) => {
                        frame.block = block;
                        frame.instruction = 0;
                        self.frames.push(frame);
                        continue;
                    }
                    Flow::Return { value, cleanup } => {
                        self.run_cleanup(prog, &cleanup, &mut frame.variables)?;
                        if let Some(done) = self.finish_frame(prog, target, frame, value)? {
                            return Ok(done);
                        }
                        continue;
                    }
                }
            }
            let returned = match &block.term {
                MirTerm::Jump(next) => {
                    frame.block = *next;
                    frame.instruction = 0;
                    self.frames.push(frame);
                    continue;
                }
                MirTerm::Branch {
                    cond,
                    then_b,
                    else_b,
                } => {
                    frame.block = if is_true(&frame.registers[cond.0 as usize]) {
                        *then_b
                    } else {
                        *else_b
                    };
                    frame.instruction = 0;
                    self.frames.push(frame);
                    continue;
                }
                MirTerm::Return(reg) => reg
                    .as_ref()
                    .map(|reg| frame.registers[reg.0 as usize].clone())
                    .unwrap_or(Value::None),
                MirTerm::ReturnWithCleanup { value, cleanup } => {
                    let returned = value
                        .as_ref()
                        .map(|reg| frame.registers[reg.0 as usize].clone())
                        .unwrap_or(Value::None);
                    self.run_cleanup(prog, cleanup, &mut frame.variables)?;
                    returned
                }
                MirTerm::FallOff | MirTerm::EscapeJump { .. } => Value::None,
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
        let caller = self.frames.last_mut().ok_or_else(|| {
            RuntimeError::Unsupported("vm: returning child has no caller frame".to_string())
        })?;
        caller.registers[continuation.dest.0 as usize] = value;
        for (parameter, place) in continuation.writebacks {
            *nav_mut(&mut caller.variables, &caller.registers, &place)? =
                frame.variables[parameter].clone();
        }
        Ok(None)
    }

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
            param_arg_regs,
            param_decls,
            ..
        } = instruction
        {
            let callable = &caller.registers[callee.0 as usize];
            let mut nominal_receiver = None;
            let (function_name, captured) = match callable {
                Value::Function(function_name) => (function_name.clone(), Vec::new()),
                Value::Closure { function, captures } => (
                    function.clone(),
                    Self::closure_capture_arguments(
                        caller.id,
                        &caller.registers,
                        &caller.variables,
                        callee_place.as_ref(),
                        captures,
                    )?,
                ),
                Value::Struct { name, .. } => {
                    nominal_receiver = Some(callable.clone());
                    (
                        prog.runtime_method_name(name, "__call__", resolved.as_deref(), args.len()),
                        Vec::new(),
                    )
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
                    self.bind_for_call(prog, &function_name, signature, positional, keywords)?
                }
                None => (
                    positional,
                    (0..capture_count + args.len())
                        .map(ArgSlot::Positional)
                        .collect(),
                ),
            };
            let definition = &prog.mir.functions[index].1;
            let value_params = prog
                .sigs
                .get(&function_name)
                .map(|signature| {
                    let contract = if param_decls.is_empty() {
                        &signature.param_decls
                    } else {
                        param_decls
                    };
                    let supplied =
                        runtime_parameter_arguments(contract, param_arg_regs, &caller.registers);
                    let supplied = resolve_value_parameter_slots(contract, &supplied);
                    reify_value_parameters(&signature.param_decls, &supplied)
                })
                .unwrap_or_default();
            if let Some(receiver) = nominal_receiver {
                for parameter in 1..definition.ref_params.len() {
                    if !definition.ref_params[parameter] {
                        continue;
                    }
                    let place = bound_argument_place(
                        slots.get(parameter - 1),
                        prog.sigs
                            .get(&function_name)
                            .and_then(|signature| signature.param_names.get(parameter - 1))
                            .map(String::as_str),
                        0,
                        arg_places,
                        &keyword_names,
                        kwarg_places,
                    )
                    .ok_or_else(|| {
                        RuntimeError::Unsupported(format!(
                            "vm: a mut/ref argument to callable '{function_name}' must be a place"
                        ))
                    })?;
                    bound[parameter - 1] = self.reference_to_place(caller, place)?;
                }
                let receiver = if definition.ref_params.first().copied().unwrap_or(false) {
                    let place = callee_place.as_ref().ok_or_else(|| {
                        RuntimeError::Unsupported(format!(
                            "vm: reference receiver for callable '{function_name}' must be a place"
                        ))
                    })?;
                    self.reference_to_place(caller, place)?
                } else {
                    receiver
                };
                bound.insert(0, receiver);
            } else {
                for (parameter, is_ref) in definition.ref_params.iter().enumerate() {
                    if !is_ref {
                        continue;
                    }
                    let captured_argument = match slots.get(parameter) {
                        Some(ArgSlot::Positional(argument)) if *argument < capture_count => {
                            Some(*argument)
                        }
                        _ => None,
                    };
                    if let Some(argument) = captured_argument {
                        if !matches!(bound[parameter], Value::Ref { .. }) {
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
                            .and_then(|signature| signature.param_names.get(parameter))
                            .map(String::as_str),
                        capture_count,
                        arg_places,
                        &keyword_names,
                        kwarg_places,
                    )
                    .ok_or_else(|| {
                        RuntimeError::Unsupported(format!(
                            "vm: a mut/ref argument to callable '{function_name}' must be a place"
                        ))
                    })?;
                    bound[parameter] = self.reference_to_place(caller, place)?;
                }
            }
            return self
                .make_frame(
                    prog,
                    index,
                    bound,
                    &value_params,
                    Some(ReturnContinuation {
                        dest: *dest,
                        writebacks: Vec::new(),
                    }),
                )
                .map(Some);
        }
        if let MirInstr::MethodCall {
            dest,
            recv,
            method,
            resolved,
            args,
            kwargs,
            recv_place,
            arg_places,
            kwarg_places,
            param_arg_regs,
            param_decls,
            ..
        } = instruction
            && let Value::Struct { name, .. } = &caller.registers[recv.0 as usize]
        {
            let function_name =
                prog.runtime_method_name(name, method, resolved.as_deref(), args.len());
            let Some(index) = prog.index_of(&function_name) else {
                return Ok(None);
            };
            if !prog.mir.functions[index].1.returns_reference {
                return Ok(None);
            }
            let positional: Vec<Value> = args
                .iter()
                .map(|register| caller.registers[register.0 as usize].clone())
                .collect();
            let keywords: Vec<(String, Value)> = kwargs
                .iter()
                .map(|(key, register)| (key.clone(), caller.registers[register.0 as usize].clone()))
                .collect();
            let keyword_names: Vec<String> = kwargs.iter().map(|(name, _)| name.clone()).collect();
            let (mut bound, slots) = match prog.sigs.get(&function_name) {
                Some(signature) => {
                    self.bind_for_call(prog, &function_name, signature, positional, keywords)?
                }
                None => (
                    positional,
                    (0..args.len()).map(ArgSlot::Positional).collect(),
                ),
            };
            let definition = &prog.mir.functions[index].1;
            for parameter in 1..definition.ref_params.len() {
                if !definition.ref_params[parameter] {
                    continue;
                }
                let place = bound_argument_place(
                    slots.get(parameter - 1),
                    prog.sigs
                        .get(&function_name)
                        .and_then(|signature| signature.param_names.get(parameter - 1))
                        .map(String::as_str),
                    0,
                    arg_places,
                    &keyword_names,
                    kwarg_places,
                )
                .ok_or_else(|| {
                    RuntimeError::Unsupported(format!(
                        "vm: a mut/ref argument to method '{function_name}' must be a place"
                    ))
                })?;
                bound[parameter - 1] = self.reference_to_place(caller, place)?;
            }
            let receiver = if definition.ref_params.first().copied().unwrap_or(false) {
                let place = recv_place.as_ref().ok_or_else(|| {
                    RuntimeError::Unsupported(format!(
                        "vm: mutable receiver for '{function_name}' must be a place"
                    ))
                })?;
                self.reference_to_place(caller, place)?
            } else {
                caller.registers[recv.0 as usize].clone()
            };
            let mut call_args = Vec::with_capacity(bound.len() + 1);
            call_args.push(receiver);
            call_args.extend(bound);
            let value_params = prog
                .sigs
                .get(&function_name)
                .map(|signature| {
                    let contract = if param_decls.is_empty() {
                        &signature.param_decls
                    } else {
                        param_decls
                    };
                    let supplied =
                        runtime_parameter_arguments(contract, param_arg_regs, &caller.registers);
                    let supplied = resolve_value_parameter_slots(contract, &supplied);
                    reify_value_parameters(&signature.param_decls, &supplied)
                })
                .unwrap_or_default();
            return self
                .make_frame(
                    prog,
                    index,
                    call_args,
                    &value_params,
                    Some(ReturnContinuation {
                        dest: *dest,
                        writebacks: Vec::new(),
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
            param_arg_regs,
            ..
        } = instruction
        else {
            return Ok(None);
        };
        if crate::symbol::init_overload_struct(&func.0).is_some() {
            return Ok(None);
        }
        let Some(index) = prog.index_of(&func.0) else {
            return Ok(None);
        };
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
                self.bind_for_call(prog, &func.0, signature, positional, keywords)?
            }
            None => (
                positional,
                (0..args.len()).map(ArgSlot::Positional).collect(),
            ),
        };
        let value_params = prog
            .sigs
            .get(&func.0)
            .map(|signature| {
                let supplied = runtime_parameter_arguments(
                    &signature.param_decls,
                    param_arg_regs,
                    &caller.registers,
                );
                reify_value_parameters(&signature.param_decls, &supplied)
            })
            .unwrap_or_default();
        for (parameter, is_ref) in prog.mir.functions[index].1.ref_params.iter().enumerate() {
            if !is_ref {
                continue;
            }
            let place = bound_argument_place(
                slots.get(parameter),
                prog.sigs
                    .get(&func.0)
                    .and_then(|signature| signature.param_names.get(parameter))
                    .map(String::as_str),
                0,
                arg_places,
                &keyword_names,
                kwarg_places,
            )
            .ok_or_else(|| {
                RuntimeError::Unsupported(format!(
                    "vm: a mut/ref argument to '{}' must be a place",
                    func.0
                ))
            })?;
            bound[parameter] = self.reference_to_place(caller, place)?;
        }
        self.make_frame(
            prog,
            index,
            bound,
            &value_params,
            Some(ReturnContinuation {
                dest: *dest,
                writebacks: Vec::new(),
            }),
        )
        .map(Some)
    }
}
