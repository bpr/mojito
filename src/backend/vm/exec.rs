//! The `exec_instr` instruction dispatcher plus `try`-region execution
//! (`exec_try`/`run_cleanup`/`run_region`).
//! Extracted from `backend/vm.rs`; see `docs/symbol-map.md`.

use super::*;

impl VmBackend {
    /// Execute one straight-line MIR instruction against the current frame.
    /// Returns the control-flow outcome — `Normal`, or `Return` when a `return`
    /// inside a nested `try` region crosses out (all other instructions are
    /// `Normal`).
    pub(super) fn exec_instr(
        &mut self,
        prog: &Prog,
        i: &MirInstr,
        function: usize,
        frame_id: FrameId,
        regs: &mut [Value],
        vars: &mut Vec<Value>,
    ) -> Result<Flow, RuntimeError> {
        self.burn_ctfe()?;
        match i {
            MirInstr::EstablishLoans { .. } | MirInstr::InvalidateInteriors { .. } => {}
            MirInstr::MakeRef { dest, place } => {
                let root = vars[place.root as usize].clone();
                let (frame, slot, mut projection) = match root {
                    Value::Ref {
                        frame,
                        slot,
                        projection,
                    } => (frame, slot, projection),
                    _ => (frame_id.0, place.root as usize, Vec::new()),
                };
                projection.extend(super::references::place_projection_segments(place, regs)?);
                regs[dest.0 as usize] = Value::Ref {
                    frame,
                    slot,
                    projection,
                };
            }
            MirInstr::ReadRef { dest, reference } => {
                let handle = regs[reference.0 as usize].clone();
                regs[dest.0 as usize] =
                    self.read_reference(&handle, frame_id, vars)
                        .map_err(|error| {
                            RuntimeError::TypeError(format!(
                                "vm: ReadRef r{} received {handle:?}: {error}",
                                reference.0
                            ))
                        })?;
            }
            MirInstr::CopyValue { dest, value } => {
                let value = regs[value.0 as usize].clone();
                regs[dest.0 as usize] = if self.has_copyinit {
                    self.clone_value(prog, &value)?
                } else {
                    value
                };
            }
            MirInstr::WriteRef { reference, value } => {
                let handle = regs[reference.0 as usize].clone();
                self.write_reference(&handle, frame_id, vars, regs[value.0 as usize].clone())?;
            }
            MirInstr::MakeClosure {
                dest,
                function,
                captures,
            } => {
                let mut environment = Vec::with_capacity(captures.len());
                for capture in captures {
                    let reference =
                        Self::reference_to_place_parts(frame_id, regs, vars, &capture.place)?;
                    let value = match capture.mode {
                        MirCaptureMode::Reference => reference,
                        MirCaptureMode::Copy => {
                            let value = self.read_reference(&reference, frame_id, vars)?;
                            if self.has_copyinit {
                                self.clone_value(prog, &value)?
                            } else {
                                value
                            }
                        }
                        MirCaptureMode::Move => {
                            let value = self.read_reference(&reference, frame_id, vars)?;
                            self.write_reference(&reference, frame_id, vars, Value::Moved)?;
                            if self.has_moveinit {
                                self.move_value(prog, value)?
                            } else {
                                value
                            }
                        }
                    };
                    if matches!(value, Value::Moved) {
                        return Err(RuntimeError::TypeError(
                            "vm: closure captured an already-moved value".to_string(),
                        ));
                    }
                    environment.push(ClosureCapture {
                        value,
                        owned: !matches!(capture.mode, MirCaptureMode::Reference),
                    });
                }
                regs[dest.0 as usize] = Value::Closure {
                    function: function.clone(),
                    captures: environment,
                };
            }
            MirInstr::KeepAlive { .. } => {}
            MirInstr::Const { dest, k } => regs[dest.0 as usize] = const_value(k),
            MirInstr::ConstructTypeParam { dest, param } => {
                // A constructible type parameter is reified at runtime as the
                // bound struct's name: a def binds it into the frame local of
                // the same name; a struct method reads it from `self`'s
                // reified parameters.
                let definition = &prog.mir.functions[function].1;
                let bound = definition
                    .var_names
                    .iter()
                    .position(|candidate| candidate == param)
                    .map(|slot| vars[slot].clone())
                    .filter(|value| !matches!(value, Value::None))
                    .or_else(|| match vars.first() {
                        Some(Value::Struct { value_params, .. }) => value_params
                            .iter()
                            .find(|(candidate, _)| candidate == param)
                            .map(|(_, value)| value.clone()),
                        _ => None,
                    });
                let Some(Value::Str(type_name)) = bound else {
                    return Err(RuntimeError::Unsupported(format!(
                        "vm: constructing type parameter '{param}' requires a reified type argument"
                    )));
                };
                // A reified argument can pass through an enclosing abstract
                // binder's spelling rather than a concrete struct name; fall
                // back to the declaration default there, as an unsupplied
                // slot would.
                let type_name = if prog.structs.contains_key(&type_name) {
                    type_name
                } else {
                    prog.sigs
                        .get(&prog.mir.functions[function].0)
                        .and_then(|signature| {
                            signature
                                .param_decls
                                .iter()
                                .find(|declaration| declaration.name() == param)
                        })
                        .and_then(|declaration| match declaration {
                            crate::types::ParamDecl::Type {
                                default: Some(default),
                                ..
                            } => match default.as_ref() {
                                Ty::Struct(struct_name, _) => Some(struct_name.clone()),
                                _ => None,
                            },
                            _ => None,
                        })
                        .unwrap_or(type_name)
                };
                regs[dest.0 as usize] =
                    self.call_named(prog, &type_name, Vec::new(), Vec::new(), &[])?;
            }
            MirInstr::SizeOf { dest, ty } => {
                let target = crate::native::target::NativeTarget::new(
                    crate::native::target::Triple::X86_64UnknownLinuxGnu,
                );
                let structs = crate::mir::struct_field_index(&prog.mir.declarations);
                let size = crate::native::layout::LayoutCx {
                    target: &target,
                    structs: &structs,
                }
                .layout_of(ty)
                .map_err(|error| RuntimeError::Unsupported(error.to_string()))?
                .size;
                regs[dest.0 as usize] = Value::Int(size as i64);
            }
            MirInstr::MaterializeLiteral {
                dest,
                value,
                target,
            } => {
                regs[dest.0 as usize] =
                    crate::runtime::materialize_literal(regs[value.0 as usize].clone(), target)?;
            }
            MirInstr::UseVar { dest, var, mode } => {
                let slot = *var as usize;
                // A `^` move **transfers** the value out of the source slot, leaving
                // a `Moved` tombstone; any other use copies. Either way, touching an
                // already-moved slot is a use-after-move — a loud runtime error (the
                // ownership analysis rejects this statically, so this only fires on a
                // compiler bug).
                // A moved owned generic `T` may itself be a reference value;
                // transfer that handle intact. Non-moving UseVar remains the
                // compatibility read path for out/ref receiver slots whose
                // handle ABI predates explicit LoadPlace/ReadRef. Checked
                // reference-valued storage sites use Borrow adjustments and do
                // not rely on that compatibility path.
                let value = if matches!(mode, crate::mir::UseMode::Move) {
                    let moved = std::mem::replace(&mut vars[slot], Value::Moved);
                    if self.has_moveinit {
                        self.move_value(prog, moved)?
                    } else {
                        moved
                    }
                } else if let Value::Ref { .. } = &vars[slot] {
                    self.read_reference(&vars[slot], frame_id, vars)?
                } else {
                    match mode {
                        crate::mir::UseMode::Move => {
                            unreachable!("moving uses are handled before reference compatibility")
                        }
                        crate::mir::UseMode::BorrowShared | crate::mir::UseMode::BorrowMut => {
                            vars[slot].clone()
                        }
                        // A copy runs `__copyinit__` (deep copy) for a lifecycle type;
                        // otherwise a plain deep `Clone`.
                        crate::mir::UseMode::Copy if self.has_copyinit => {
                            self.clone_value(prog, &vars[slot])?
                        }
                        crate::mir::UseMode::Copy => vars[slot].clone(),
                    }
                };
                if matches!(value, Value::Moved) {
                    return Err(RuntimeError::TypeError(format!(
                        "vm: use of variable slot {slot} after it was moved"
                    )));
                }
                regs[dest.0 as usize] = value;
            }
            MirInstr::DefVar {
                var,
                src,
                binding_ty,
            } => {
                let v = regs[src.0 as usize].clone();
                let slot = *var as usize;
                if let Some(ty) = binding_ty {
                    // A typed DefVar establishes fresh binding storage. In
                    // particular, rebinding a per-iteration reference target
                    // replaces the previous handle instead of assigning
                    // through the referent it designated.
                    vars[slot] = crate::runtime::coerce_checked(v, ty);
                } else {
                    // An untyped DefVar is assignment to an established
                    // binding. Reference bindings therefore write through;
                    // ordinary bindings retain their existing runtime shape.
                    let current = if let Value::Ref { .. } = &vars[slot] {
                        self.read_reference(&vars[slot], frame_id, vars)?
                    } else {
                        vars[slot].clone()
                    };
                    let assigned = crate::runtime::coerce_like(v, &current);
                    if let Value::Ref { .. } = &vars[slot] {
                        let handle = vars[slot].clone();
                        self.write_reference(&handle, frame_id, vars, assigned)?;
                    } else {
                        vars[slot] = assigned;
                    }
                }
            }
            MirInstr::UnOp { op, dest, a } => {
                regs[dest.0 as usize] = self.apply_prefix(prog, *op, regs[a.0 as usize].clone())?;
            }
            MirInstr::BinOp {
                op,
                dest,
                a,
                b,
                resolved,
            } => {
                let l = regs[a.0 as usize].clone();
                let r = regs[b.0 as usize].clone();
                regs[dest.0 as usize] = self.apply_binop(prog, *op, l, r, resolved.as_deref())?;
            }
            MirInstr::Call {
                dest,
                func,
                args,
                kwargs,
                arg_places,
                kwarg_places,
                param_arg_regs,
                ..
            } => {
                let mut argv: Vec<Value> =
                    args.iter().map(|r| regs[r.0 as usize].clone()).collect();
                let mut kw: Vec<(String, Value)> = kwargs
                    .iter()
                    .map(|(n, r)| (n.clone(), regs[r.0 as usize].clone()))
                    .collect();
                // The supplied compile-time value-parameter arguments (a type
                // parameter is `None`), used to reify a constructed struct's
                // `value_params`.
                let pvals = prog
                    .sigs
                    .get(&func.0)
                    .map(|signature| {
                        runtime_parameter_arguments(&signature.param_decls, param_arg_regs, regs)
                    })
                    .unwrap_or_else(|| {
                        param_arg_regs
                            .iter()
                            .map(|argument| {
                                argument
                                    .value
                                    .map(|register| regs[register.0 as usize].clone())
                            })
                            .collect()
                    });
                // A handwritten constructor receives reference arguments as
                // caller-frame handles, just like an ordinary ref-parameter call.
                // Its synthetic `self` occupies parameter slot zero.
                let constructor_index =
                    if let Some(struct_name) = crate::symbol::init_overload_struct(&func.0) {
                        prog.structs
                            .contains_key(struct_name)
                            .then(|| prog.index_of(&func.0))
                            .flatten()
                    } else if prog.structs.contains_key(&func.0) {
                        let init_name = format!("{}.__init__", func.0);
                        prog.index_of(&init_name)
                            .or_else(|| prog.index_of(&prog.overload_name(&init_name, args.len())))
                    } else {
                        None
                    };
                if let Some(index) = constructor_index {
                    let reference_parameters = &prog.mir.functions[index].1.ref_params;
                    for (argument, value) in argv.iter_mut().enumerate() {
                        if !reference_parameters
                            .get(argument + 1)
                            .copied()
                            .unwrap_or(false)
                        {
                            continue;
                        }
                        let place = arg_places
                            .get(argument)
                            .and_then(Option::as_ref)
                            .ok_or_else(|| {
                                RuntimeError::Unsupported(format!(
                                    "vm: reference constructor argument {} to '{}' must be a place",
                                    argument + 1,
                                    func.0
                                ))
                            })?;
                        *value = Self::reference_to_place_parts(frame_id, regs, vars, place)?;
                    }
                    if let Some(signature) = prog
                        .sigs
                        .get(&prog.mir.functions[index].0)
                        .or_else(|| prog.sigs.get(&func.0))
                    {
                        for (argument, (name, value)) in kw.iter_mut().enumerate() {
                            let Some(parameter) = signature
                                .param_names
                                .iter()
                                .position(|candidate| candidate == name)
                            else {
                                continue;
                            };
                            if !reference_parameters
                                .get(parameter + 1)
                                .copied()
                                .unwrap_or(false)
                            {
                                continue;
                            }
                            let place = kwarg_places
                                .get(argument)
                                .and_then(Option::as_ref)
                                .ok_or_else(|| {
                                    RuntimeError::Unsupported(format!(
                                        "vm: reference constructor keyword '{name}' to '{}' must be a place",
                                        func.0
                                    ))
                                })?;
                            *value = Self::reference_to_place_parts(frame_id, regs, vars, place)?;
                        }
                    }
                }
                // A free function with `mut`/`ref` parameters — or a retained
                // shared-read place lent to a borrowing-view result — binds
                // those parameters to caller-place handles.
                let retains_place = arg_places
                    .iter()
                    .chain(kwarg_places.iter())
                    .any(Option::is_some);
                let writeback = constructor_index
                    .is_none()
                    .then(|| {
                        prog.index_of(&func.0).filter(|&idx| {
                            retains_place || prog.mir.functions[idx].1.ref_params.iter().any(|&r| r)
                        })
                    })
                    .flatten();
                let runtime_value_params = prog
                    .sigs
                    .get(&func.0)
                    .map(|signature| reify_value_parameters(&signature.param_decls, &pvals))
                    .unwrap_or_default();
                let result = match writeback {
                    Some(idx) => self.call_with_writeback(
                        prog,
                        WritebackCall {
                            function_name: &func.0,
                            function_index: idx,
                            positional_args: argv,
                            keyword_args: kw,
                            argument_places: arg_places,
                            keyword_argument_places: kwarg_places,
                            value_params: runtime_value_params,
                        },
                        CallerFrame {
                            id: frame_id,
                            registers: regs,
                            variables: vars,
                        },
                    )?,
                    None => {
                        // Reaching the instruction interpreter means this call
                        // is executing synchronously (for example inside a
                        // structured try region); the continuation path handles
                        // ordinary direct calls earlier. Keep the caller's real
                        // frame identity reachable even without direct ref
                        // parameters because by-value arguments may contain
                        // nested reference handles.
                        let stack_base = self.push_caller_mirror(frame_id, regs, vars);
                        let outcome = self.call_named(prog, &func.0, argv, kw, &pvals);
                        self.restore_caller_mirror(stack_base, vars)?;
                        outcome?
                    }
                };
                let target = prog.mir.functions[function].1.reg_types.get(&dest.0);
                regs[dest.0 as usize] = self.materialize_checked_result(prog, result, target)?;
            }
            MirInstr::CallIndirect {
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
            } => {
                let callable = regs[callee.0 as usize].clone();
                let mut nominal_receiver = None;
                let (function, captures) = match &callable {
                    Value::Function(function) => (function.clone(), Vec::new()),
                    Value::Closure { function, captures } => (
                        function.clone(),
                        Self::closure_capture_arguments(
                            frame_id,
                            regs,
                            vars,
                            callee_place.as_ref(),
                            captures,
                        )?,
                    ),
                    Value::Struct { name, .. } => {
                        nominal_receiver = Some(callable.clone());
                        (
                            prog.runtime_method_name(
                                name,
                                "__call__",
                                resolved.as_deref(),
                                args.len(),
                            ),
                            Vec::new(),
                        )
                    }
                    value => {
                        return Err(RuntimeError::NotCallable(crate::runtime::type_name(value)));
                    }
                };
                let capture_count = captures.len();
                let mut positional = captures;
                positional.extend(
                    args.iter()
                        .map(|register| regs[register.0 as usize].clone()),
                );
                let keywords = kwargs
                    .iter()
                    .map(|(name, register)| (name.clone(), regs[register.0 as usize].clone()))
                    .collect();
                let keyword_names: Vec<String> =
                    kwargs.iter().map(|(name, _)| name.clone()).collect();
                let index = prog
                    .index_of(&function)
                    .ok_or_else(|| RuntimeError::NotCallable(function.clone()))?;
                let (mut bound, slots) = match prog.sigs.get(&function) {
                    Some(signature) => {
                        self.bind_for_call(prog, &function, signature, positional, keywords)?
                    }
                    None => (
                        positional,
                        (0..capture_count + args.len())
                            .map(ArgSlot::Positional)
                            .collect(),
                    ),
                };
                // A `try` region executes synchronously instead of through the
                // explicit frame driver. Retain each indirect reference input as
                // its real caller handle; the shared caller mirror keeps those
                // handles valid through the child call.
                let definition = &prog.mir.functions[index].1;
                let value_params = prog
                    .sigs
                    .get(&function)
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
                let mut reference_inputs: Vec<(usize, Value)> = Vec::new();
                if let Some(receiver) = nominal_receiver {
                    for parameter in 1..definition.ref_params.len() {
                        if !definition.ref_params[parameter] {
                            continue;
                        }
                        let place = bound_argument_place(
                            slots.get(parameter - 1),
                            prog.sigs
                                .get(&function)
                                .and_then(|signature| signature.param_names.get(parameter - 1))
                                .map(String::as_str),
                            0,
                            arg_places,
                            &keyword_names,
                            kwarg_places,
                        )
                        .ok_or_else(|| {
                            RuntimeError::Unsupported(format!(
                                "vm: a mut/ref argument to callable '{function}' must be a place"
                            ))
                        })?;
                        let handle = Self::reference_to_place_parts(frame_id, regs, vars, place)?;
                        reference_inputs.push((parameter, handle));
                    }
                    if definition.ref_params.first().copied().unwrap_or(false) {
                        let place = callee_place.as_ref().ok_or_else(|| {
                            RuntimeError::Unsupported(format!(
                                "vm: reference receiver for callable '{function}' must be a place"
                            ))
                        })?;
                        reference_inputs.push((
                            0,
                            Self::reference_to_place_parts(frame_id, regs, vars, place)?,
                        ));
                    }
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
                        let handle = if let Some(argument) = captured_argument {
                            let handle = bound[parameter].clone();
                            if !matches!(handle, Value::Ref { .. }) {
                                return Err(RuntimeError::TypeError(format!(
                                    "vm: reference capture {argument} for '{function}' lost its handle"
                                )));
                            }
                            handle
                        } else {
                            let place = bound_argument_place(
                                slots.get(parameter),
                                prog.sigs
                                    .get(&function)
                                    .and_then(|signature| signature.param_names.get(parameter))
                                    .map(String::as_str),
                                capture_count,
                                arg_places,
                                &keyword_names,
                                kwarg_places,
                            )
                                .ok_or_else(|| {
                                    RuntimeError::Unsupported(format!(
                                        "vm: a mut/ref argument to callable '{function}' must be a place"
                                    ))
                                })?;
                            Self::reference_to_place_parts(frame_id, regs, vars, place)?
                        };
                        reference_inputs.push((parameter, handle));
                    }
                }
                let (result, _, _) = self.call_synchronously_with_references(
                    prog,
                    SynchronousCall {
                        function_index: index,
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
                regs[dest.0 as usize] = result;
            }
            MirInstr::MethodCall {
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
                param_arg_regs,
                param_decls,
                ..
            } => {
                let recv_val = regs[recv.0 as usize].clone();
                let argv: Vec<Value> = args.iter().map(|r| regs[r.0 as usize].clone()).collect();
                let kw: Vec<(String, Value)> = kwargs
                    .iter()
                    .map(|(name, reg)| (name.clone(), regs[reg.0 as usize].clone()))
                    .collect();
                let result = self.method_call(
                    prog,
                    MethodInvocation {
                        receiver: recv_val,
                        method,
                        resolved_name: resolved.as_deref(),
                        result_adapter: *result_adapter,
                        arguments: argv,
                        keyword_arguments: kw,
                        receiver_place: recv_place,
                        argument_places: arg_places,
                        keyword_argument_places: kwarg_places,
                        parameter_arguments: param_arg_regs,
                        parameter_declarations: param_decls,
                    },
                    CallerFrame {
                        id: frame_id,
                        registers: regs,
                        variables: vars,
                    },
                )?;
                let target = prog.mir.functions[function].1.reg_types.get(&dest.0);
                regs[dest.0 as usize] = self.materialize_checked_result(prog, result, target)?;
            }
            MirInstr::PointerStorageTake {
                dest,
                pointer,
                index,
                ..
            } => {
                let Value::Pointer { allocation, offset } = regs[pointer.0 as usize] else {
                    return Err(RuntimeError::TypeError(
                        "vm: compiler-private storage take requires Pointer".to_string(),
                    ));
                };
                let index = value_as_index(&regs[index.0 as usize])?;
                regs[dest.0 as usize] = self.heap_take(allocation, offset, index)?;
            }
            MirInstr::PointerStorageDestroy {
                dest,
                pointer,
                index,
                ..
            } => {
                let Value::Pointer { allocation, offset } = regs[pointer.0 as usize] else {
                    return Err(RuntimeError::TypeError(
                        "vm: compiler-private storage destroy requires Pointer".to_string(),
                    ));
                };
                let index = value_as_index(&regs[index.0 as usize])?;
                self.heap_destroy(prog, allocation, offset, index)?;
                regs[dest.0 as usize] = Value::None;
            }
            MirInstr::UninitStorage { dest, init } => {
                let payload = init
                    .map(|register| std::mem::replace(&mut regs[register.0 as usize], Value::Moved))
                    .map(Box::new);
                regs[dest.0 as usize] = Value::UninitStorage(payload);
            }
            MirInstr::UninitStorageTake { dest, storage, .. } => {
                let storage = std::mem::replace(&mut regs[storage.0 as usize], Value::Moved);
                regs[dest.0 as usize] = Self::uninit_storage_payload(storage, "take")?;
            }
            MirInstr::UninitStorageDestroy { dest, storage, .. } => {
                let storage = std::mem::replace(&mut regs[storage.0 as usize], Value::Moved);
                let payload = Self::uninit_storage_payload(storage, "destroy")?;
                self.drop_value(prog, payload)?;
                regs[dest.0 as usize] = Value::None;
            }
            MirInstr::GetField { dest, base, field } => {
                let base = regs[base.0 as usize].clone();
                // A ref-typed base register holds a handle; field access
                // addresses its referent (another read twin of the ref-field
                // second dereference).
                let base = if matches!(base, Value::Ref { .. }) {
                    self.read_reference(&base, frame_id, vars)?
                } else {
                    base
                };
                regs[dest.0 as usize] = match &base {
                    Value::Slice {
                        start, end, step, ..
                    } => {
                        let bound = match field.as_str() {
                            "start" => *start,
                            "end" => *end,
                            "step" => *step,
                            _ => {
                                return Err(RuntimeError::TypeError(format!(
                                    "Slice has no field '{field}'"
                                )));
                            }
                        };
                        self.slice_bound_optional(prog, bound)?
                    }
                    _ => get_field(&base, field)?,
                };
            }
            MirInstr::Index {
                dest,
                base,
                index,
                base_place,
                index_place,
                call,
                intrinsic,
            } => {
                let base_value = regs[base.0 as usize].clone();
                // A ref-typed subscript base register holds a handle; the
                // checked receiver is its referent (the read twin of the
                // ref-field store's second dereference).
                let base_value = if matches!(base_value, Value::Ref { .. }) {
                    self.read_reference(&base_value, frame_id, vars)?
                } else {
                    base_value
                };
                if let Some(call) = call {
                    // A user struct with `__getitem__` is subscriptable: `c[i]` →
                    // `c.__getitem__(i)` (index passed as-is, not coerced to Int).
                    // A checker-resolved implementation dispatches exactly. A
                    // specialized Tuple accessor takes only `self`, while an
                    // ordinary overloaded `__getitem__` (for example List's
                    // Int versus Slice overloads) still takes the runtime index.
                    // Use the checked argument contract, not the mere presence
                    // of a selected symbol, to distinguish those ABIs.
                    let recv = match base_value {
                        recv @ Value::Struct { .. } => recv,
                        other => {
                            return Err(RuntimeError::TypeError(format!(
                                "vm: checked nominal subscript receiver is {}",
                                crate::runtime::type_name(&other)
                            )));
                        }
                    };
                    let idx = regs[index.0 as usize].clone();
                    let parameterless = call.arguments.is_empty();
                    let arguments = if parameterless { Vec::new() } else { vec![idx] };
                    let argument_places = if parameterless {
                        Vec::new()
                    } else {
                        vec![index_place.clone()]
                    };
                    let result = self.method_call(
                        prog,
                        MethodInvocation {
                            receiver: recv,
                            method: "__getitem__",
                            resolved_name: Some(&call.target),
                            result_adapter: None,
                            arguments,
                            keyword_arguments: Vec::new(),
                            receiver_place: base_place,
                            argument_places: &argument_places,
                            keyword_argument_places: &[],
                            parameter_arguments: &call.param_arg_regs,
                            parameter_declarations: &call.param_decls,
                        },
                        CallerFrame {
                            id: frame_id,
                            registers: regs,
                            variables: vars,
                        },
                    )?;
                    let target = prog.mir.functions[function].1.reg_types.get(&dest.0);
                    regs[dest.0 as usize] =
                        self.materialize_checked_result(prog, result, target)?;
                } else {
                    let intrinsic = intrinsic.ok_or_else(|| {
                        RuntimeError::TypeError(
                            "vm: call-less index lacks an intrinsic dispatch kind".to_string(),
                        )
                    })?;
                    regs[dest.0 as usize] = match (intrinsic, base_value) {
                        // `ptr[i]` loads the pointee at `base + i` from the heap arena.
                        (MirIntrinsicSubscript::Pointer, Value::Pointer { allocation, offset }) => {
                            let off = self.normalize_index(prog, &regs[index.0 as usize])?;
                            let value = self.heap_read(allocation, offset, off)?;
                            if self.has_copyinit {
                                self.clone_value(prog, &value)?
                            } else {
                                value
                            }
                        }
                        (MirIntrinsicSubscript::ComptimeList, Value::ComptimeList(items))
                            if self.ctfe_fuel.is_some() =>
                        {
                            let idx = self.normalize_index(prog, &regs[index.0 as usize])?;
                            let index = crate::runtime::bounds_check(
                                idx,
                                items.len(),
                                "compile-time list index",
                            )?;
                            items[index].clone()
                        }
                        (
                            MirIntrinsicSubscript::TupleStorage
                            | MirIntrinsicSubscript::VariadicStorage
                            | MirIntrinsicSubscript::Simd,
                            value,
                        ) => {
                            let idx = self.normalize_index(prog, &regs[index.0 as usize])?;
                            index_value(&value, idx)?
                        }
                        (kind, value) => {
                            return Err(RuntimeError::TypeError(format!(
                                "vm: intrinsic {kind:?} cannot index {}",
                                crate::runtime::type_name(&value)
                            )));
                        }
                    };
                }
            }
            MirInstr::Slice {
                dest,
                object,
                kind,
                lower,
                upper,
                step,
                object_place,
                arg_places,
                call,
                intrinsic,
            } => {
                let bound = |b: &Option<Reg>| -> Result<Option<i64>, RuntimeError> {
                    match b {
                        Some(r) => Ok(Some(value_as_index(&regs[r.0 as usize])?)),
                        None => Ok(None),
                    }
                };
                let (lo, hi, st) = (bound(lower)?, bound(upper)?, bound(step)?);
                let receiver = regs[object.0 as usize].clone();
                regs[dest.0 as usize] = if let Some(call) = call {
                    let slice = Value::Slice {
                        kind: *kind,
                        start: lo,
                        end: hi,
                        step: st,
                    };
                    let result = self.method_call(
                        prog,
                        MethodInvocation {
                            receiver,
                            method: "__getitem__",
                            resolved_name: Some(&call.target),
                            result_adapter: None,
                            arguments: vec![slice],
                            keyword_arguments: Vec::new(),
                            receiver_place: object_place,
                            argument_places: arg_places,
                            keyword_argument_places: &[],
                            parameter_arguments: &call.param_arg_regs,
                            parameter_declarations: &call.param_decls,
                        },
                        CallerFrame {
                            id: frame_id,
                            registers: regs,
                            variables: vars,
                        },
                    )?;
                    let target = prog.mir.functions[function].1.reg_types.get(&dest.0);
                    self.materialize_checked_result(prog, result, target)?
                } else {
                    match intrinsic {
                        Some(kind) => {
                            return Err(RuntimeError::TypeError(format!(
                                "vm: intrinsic {kind:?} does not support slicing"
                            )));
                        }
                        None => {
                            return Err(RuntimeError::TypeError(
                                "vm: call-less slice lacks an intrinsic dispatch kind".to_string(),
                            ));
                        }
                    }
                };
            }
            MirInstr::MultiIndex {
                dest,
                object,
                args,
                object_place,
                arg_places,
                kwargs,
                kwarg_places,
                call,
            } => {
                let bound = |bound: &Option<Reg>| -> Result<Option<i64>, RuntimeError> {
                    bound
                        .map(|register| value_as_index(&regs[register.0 as usize]))
                        .transpose()
                };
                let subscript_value = |argument: &MirSubscriptArg| -> Result<Value, RuntimeError> {
                    Ok(match argument {
                        MirSubscriptArg::Index(register) => regs[register.0 as usize].clone(),
                        MirSubscriptArg::Slice {
                            kind,
                            lower,
                            upper,
                            step,
                        } => Value::Slice {
                            kind: *kind,
                            start: bound(lower)?,
                            end: bound(upper)?,
                            step: bound(step)?,
                        },
                    })
                };
                let mut arguments = Vec::with_capacity(args.len());
                for argument in args {
                    arguments.push(subscript_value(argument)?);
                }
                let call = call.as_ref().ok_or_else(|| {
                    RuntimeError::TypeError(
                        "vm: nominal multi-index lacks a checked call contract".to_string(),
                    )
                })?;
                let keyword_arguments = kwargs
                    .iter()
                    .map(|(name, argument)| Ok((name.clone(), subscript_value(argument)?)))
                    .collect::<Result<Vec<_>, RuntimeError>>()?;
                let result = self.method_call(
                    prog,
                    MethodInvocation {
                        receiver: regs[object.0 as usize].clone(),
                        method: "__getitem__",
                        resolved_name: Some(&call.target),
                        result_adapter: None,
                        arguments,
                        keyword_arguments,
                        receiver_place: object_place,
                        argument_places: arg_places,
                        keyword_argument_places: kwarg_places,
                        parameter_arguments: &call.param_arg_regs,
                        parameter_declarations: &call.param_decls,
                    },
                    CallerFrame {
                        id: frame_id,
                        registers: regs,
                        variables: vars,
                    },
                )?;
                let target = prog.mir.functions[function].1.reg_types.get(&dest.0);
                regs[dest.0 as usize] = self.materialize_checked_result(prog, result, target)?;
            }
            MirInstr::MultiSet {
                receiver,
                receiver_place,
                args,
                arg_places,
                value,
                value_place,
                value_keyword,
                call,
            } => {
                let bound = |bound: &Option<Reg>| -> Result<Option<i64>, RuntimeError> {
                    bound
                        .map(|register| value_as_index(&regs[register.0 as usize]))
                        .transpose()
                };
                let mut arguments = Vec::with_capacity(args.len() + usize::from(!value_keyword));
                for argument in args {
                    arguments.push(match argument {
                        MirSubscriptArg::Index(register) => regs[register.0 as usize].clone(),
                        MirSubscriptArg::Slice {
                            kind,
                            lower,
                            upper,
                            step,
                        } => Value::Slice {
                            kind: *kind,
                            start: bound(lower)?,
                            end: bound(upper)?,
                            step: bound(step)?,
                        },
                    });
                }
                let keyword_arguments = if *value_keyword {
                    vec![("value".to_string(), regs[value.0 as usize].clone())]
                } else {
                    arguments.push(regs[value.0 as usize].clone());
                    Vec::new()
                };
                let mut argument_places = arg_places.clone();
                let keyword_argument_places = if *value_keyword {
                    vec![value_place.clone()]
                } else {
                    argument_places.push(value_place.clone());
                    Vec::new()
                };
                let _ = self.method_call(
                    prog,
                    MethodInvocation {
                        receiver: regs[receiver.0 as usize].clone(),
                        method: "__setitem__",
                        resolved_name: Some(&call.target),
                        result_adapter: None,
                        arguments,
                        keyword_arguments,
                        receiver_place,
                        argument_places: &argument_places,
                        keyword_argument_places: &keyword_argument_places,
                        parameter_arguments: &call.param_arg_regs,
                        parameter_declarations: &call.param_decls,
                    },
                    CallerFrame {
                        id: frame_id,
                        registers: regs,
                        variables: vars,
                    },
                )?;
            }
            MirInstr::MakeTuple {
                dest,
                elems,
                element_types,
            } => {
                let raw: Vec<Value> = elems.iter().map(|r| regs[r.0 as usize].clone()).collect();
                let items = match element_types {
                    Some(types) if types.len() == raw.len() => raw
                        .into_iter()
                        .zip(types)
                        .map(|(value, ty)| crate::runtime::coerce_checked(value, ty))
                        .collect(),
                    _ => raw,
                };
                regs[dest.0 as usize] = Value::Tuple(items);
            }
            MirInstr::MakeVariant {
                dest,
                alternatives,
                index,
                value,
            } => {
                let selected = alternatives.get(*index).ok_or_else(|| {
                    RuntimeError::TypeError("Variant construction has an invalid tag".to_string())
                })?;
                // The variant owns an independent payload: a lifecycle
                // struct (a nominal String) deep-copies so the argument
                // temporary's drop cannot free the stored buffer.
                let payload = self.clone_value(
                    prog,
                    &crate::runtime::coerce_checked(regs[value.0 as usize].clone(), selected),
                )?;
                regs[dest.0 as usize] = Value::Variant {
                    alternatives: alternatives.clone(),
                    index: *index,
                    value: Box::new(payload),
                };
            }
            MirInstr::VariantIs {
                dest,
                variant,
                index,
            } => {
                let Value::Variant {
                    alternatives,
                    index: active,
                    ..
                } = &regs[variant.0 as usize]
                else {
                    return Err(RuntimeError::TypeError(format!(
                        "Variant.isa applied to {}",
                        crate::runtime::type_name(&regs[variant.0 as usize])
                    )));
                };
                if *index >= alternatives.len() {
                    return Err(RuntimeError::TypeError(
                        "Variant.isa has an invalid checked tag".to_string(),
                    ));
                }
                regs[dest.0 as usize] = Value::Bool(active == index);
            }
            MirInstr::VariantGet {
                dest,
                variant,
                index,
            } => {
                let Value::Variant {
                    alternatives,
                    index: active,
                    value,
                } = &regs[variant.0 as usize]
                else {
                    return Err(RuntimeError::TypeError(format!(
                        "typed Variant projection applied to {}",
                        crate::runtime::type_name(&regs[variant.0 as usize])
                    )));
                };
                let expected = alternatives.get(*index).ok_or_else(|| {
                    RuntimeError::TypeError(
                        "typed Variant projection has an invalid checked tag".to_string(),
                    )
                })?;
                if active != index {
                    let found = alternatives
                        .get(*active)
                        .map(ToString::to_string)
                        .unwrap_or_else(|| "<invalid>".to_string());
                    return Err(RuntimeError::TypeError(format!(
                        "Variant holds '{found}', not '{expected}'"
                    )));
                }
                regs[dest.0 as usize] = value.as_ref().clone();
            }
            MirInstr::VariantTake {
                dest,
                variant,
                index,
                checked,
            } => {
                let Value::Variant {
                    alternatives,
                    index: active,
                    value,
                } = &regs[variant.0 as usize]
                else {
                    return Err(RuntimeError::TypeError(format!(
                        "Variant.take applied to {}",
                        crate::runtime::type_name(&regs[variant.0 as usize])
                    )));
                };
                let expected = alternatives.get(*index).ok_or_else(|| {
                    RuntimeError::TypeError("Variant.take has an invalid checked tag".to_string())
                })?;
                if *checked && active != index {
                    let found = alternatives
                        .get(*active)
                        .map(ToString::to_string)
                        .unwrap_or_else(|| "<invalid>".to_string());
                    return Err(RuntimeError::TypeError(format!(
                        "Variant holds '{found}', not '{expected}'"
                    )));
                }
                // The receiver place was moved to a tombstone before this
                // instruction, so ownership of the payload is transferred out.
                regs[dest.0 as usize] = value.as_ref().clone();
            }
            MirInstr::VariantSetInitWith {
                dest,
                place,
                index,
                factory,
            } => {
                let old = load_place(vars, regs, place)?;
                let Value::Variant { alternatives, .. } = &old else {
                    return Err(RuntimeError::TypeError(format!(
                        "Variant.set applied to {}",
                        crate::runtime::type_name(&old)
                    )));
                };
                let selected = alternatives.get(*index).cloned().ok_or_else(|| {
                    RuntimeError::TypeError("Variant.set has an invalid checked tag".to_string())
                })?;
                let factory = regs[factory.0 as usize].clone();
                let produced = self.invoke_callable_value(
                    prog,
                    factory,
                    Vec::new(),
                    (frame_id, function, vars),
                )?;
                let payload = crate::runtime::coerce_checked(produced, &selected);
                let replacement = Value::Variant {
                    alternatives: alternatives.clone(),
                    index: *index,
                    value: Box::new(payload),
                };
                self.store_at_place(prog, place, replacement, regs, vars)?;
                self.drop_value(prog, old)?;
                regs[dest.0 as usize] = Value::None;
            }
            MirInstr::VariantDeinitWith {
                dest,
                variant,
                handler,
                index: expected,
            } => {
                let Value::Variant { index, value, .. } = &regs[variant.0 as usize] else {
                    return Err(RuntimeError::TypeError(format!(
                        "Variant.deinit_with applied to {}",
                        crate::runtime::type_name(&regs[variant.0 as usize])
                    )));
                };
                // The handler is monomorphic for one alternative; upstream
                // aborts when the runtime tag holds a different type.
                if index != expected {
                    return Err(RuntimeError::Abort(
                        "Variant.deinit_with: wrong variant type".to_string(),
                    ));
                }
                // The receiver place was moved to a tombstone before this
                // instruction; the handler consumes the payload.
                let payload = value.as_ref().clone();
                let handler = regs[handler.0 as usize].clone();
                self.invoke_callable_value(
                    prog,
                    handler,
                    vec![payload],
                    (frame_id, function, vars),
                )?;
                regs[dest.0 as usize] = Value::None;
            }
            MirInstr::VariantSet {
                dest,
                place,
                index,
                value,
            } => {
                let old = load_place(vars, regs, place)?;
                let Value::Variant { alternatives, .. } = &old else {
                    return Err(RuntimeError::TypeError(format!(
                        "Variant.set applied to {}",
                        crate::runtime::type_name(&old)
                    )));
                };
                let selected = alternatives.get(*index).ok_or_else(|| {
                    RuntimeError::TypeError("Variant.set has an invalid checked tag".to_string())
                })?;
                let payload = self.clone_value(
                    prog,
                    &crate::runtime::coerce_checked(regs[value.0 as usize].clone(), selected),
                )?;
                let replacement = Value::Variant {
                    alternatives: alternatives.clone(),
                    index: *index,
                    value: Box::new(payload),
                };
                self.store_at_place(prog, place, replacement, regs, vars)?;
                self.drop_value(prog, old)?;
                regs[dest.0 as usize] = Value::None;
            }
            MirInstr::VariantReplace {
                dest,
                place,
                input_index,
                output_index,
                value,
                checked,
            } => {
                let old = load_place(vars, regs, place)?;
                let Value::Variant {
                    alternatives,
                    index: active,
                    value: old_payload,
                } = old
                else {
                    return Err(RuntimeError::TypeError(format!(
                        "Variant.replace applied to {}",
                        crate::runtime::type_name(&old)
                    )));
                };
                let input = alternatives.get(*input_index).cloned().ok_or_else(|| {
                    RuntimeError::TypeError("Variant.replace has an invalid input tag".to_string())
                })?;
                let output = alternatives.get(*output_index).cloned().ok_or_else(|| {
                    RuntimeError::TypeError("Variant.replace has an invalid output tag".to_string())
                })?;
                if *checked && active != *output_index {
                    let found = alternatives
                        .get(active)
                        .map(ToString::to_string)
                        .unwrap_or_else(|| "<invalid>".to_string());
                    return Err(RuntimeError::TypeError(format!(
                        "Variant holds '{found}', not '{output}'"
                    )));
                }
                let payload = self.clone_value(
                    prog,
                    &crate::runtime::coerce_checked(regs[value.0 as usize].clone(), &input),
                )?;
                let replacement = Value::Variant {
                    alternatives,
                    index: *input_index,
                    value: Box::new(payload),
                };
                self.store_at_place(prog, place, replacement, regs, vars)?;
                regs[dest.0 as usize] = *old_payload;
            }
            MirInstr::MakeSimd {
                dest,
                dtype,
                width,
                elems,
            } => {
                let mut vals: Vec<Value> =
                    elems.iter().map(|r| regs[r.0 as usize].clone()).collect();
                // An `Intable`-bounded element arrives erased as its concrete
                // struct; normalize it through `__int__` before lane build.
                for val in &mut vals {
                    if let Value::Struct { name, .. } = val {
                        let name = name.clone();
                        let receiver = std::mem::replace(val, Value::None);
                        *val = self.call_dunder(prog, &name, "__int__", vec![receiver])?;
                    }
                }
                regs[dest.0 as usize] = simd_from_values(*dtype, *width, &vals)?;
            }
            MirInstr::SimdCast {
                dest, value, dtype, ..
            } => {
                regs[dest.0 as usize] = crate::runtime::simd_cast(*dtype, &regs[value.0 as usize])?;
            }
            MirInstr::SimdShuffle { dest, value, mask } => {
                regs[dest.0 as usize] =
                    crate::runtime::simd_shuffle(&regs[value.0 as usize], mask)?;
            }
            MirInstr::Store { place, src } => {
                let mut v = regs[src.0 as usize].clone();
                if let Some(reference) =
                    self.extend_reference(&vars[place.root as usize], place, regs)?
                {
                    // A `ref`-typed field behind an aliased root holds another
                    // handle; the assignment writes through it into the
                    // referent — the store twin of LoadPlace's second
                    // dereference — never over the handle slot itself.
                    let target = if matches!(place.ty, Some(Ty::Ref(_))) {
                        match self.read_reference(&reference, frame_id, vars)? {
                            handle @ Value::Ref { .. } => handle,
                            _ => reference,
                        }
                    } else {
                        reference
                    };
                    // The write lands through an alias in another frame's
                    // storage; handles inside the written value that root in
                    // this frame (a reference-bearing aggregate) must be
                    // re-rooted at the storage they project through first, or
                    // they dangle when this frame is disposed.
                    self.canonicalize_value_references(frame_id, vars, &mut v);
                    self.write_reference(&target, frame_id, vars, v)?;
                } else if matches!(place.ty, Some(Ty::Ref(_))) {
                    let reference = load_place(vars, regs, place)?.clone();
                    self.canonicalize_value_references(frame_id, vars, &mut v);
                    self.write_reference(&reference, frame_id, vars, v)?;
                } else {
                    self.store_at_place(prog, place, v, regs, vars)?;
                }
            }
            MirInstr::StoreRef { place, reference } => {
                let handle = regs[reference.0 as usize].clone();
                if !matches!(handle, Value::Ref { .. }) {
                    return Err(RuntimeError::TypeError(
                        "vm: reference storage requires a reference handle".to_string(),
                    ));
                }
                self.store_at_place(prog, place, handle, regs, vars)?;
            }
            MirInstr::LoadPlace { dest, place } => {
                // The read half of `c[i] += e` on a user struct goes through
                // `c.__getitem__(i)`; any other place reads its slot / SIMD lane.
                regs[dest.0 as usize] = if let Some(reference) =
                    self.extend_reference(&vars[place.root as usize], place, regs)?
                {
                    // Reading through a reference-alias root (a `mut`/`ref self`
                    // receiver) reaches the referent in one handle read. A
                    // `ref[origin]`-typed field stored behind that alias yields
                    // *another* handle, which needs the same second dereference the
                    // plain-root branch applies — but only then: an alias that
                    // already reaches its referent (a directly aliased value) must
                    // keep it, so gate on the loaded value still being a handle.
                    let value = self.read_reference(&reference, frame_id, vars)?;
                    if matches!(value, Value::Ref { .. }) && matches!(place.ty, Some(Ty::Ref(_))) {
                        self.read_reference(&value, frame_id, vars)?
                    } else {
                        value
                    }
                } else {
                    match self.load_index_dunder(prog, place, regs, vars, frame_id)? {
                        Some(v) => v,
                        None => {
                            // A projection crossing an INTERMEDIATE ref-typed
                            // segment (`self.value.items` through a `ref`
                            // field) cannot walk raw storage — route it
                            // through the reference walk, which chases stored
                            // handles mid-projection.
                            // A single-pointee pointer field mid-projection
                            // (`self.src[]` on `Pointer[List[T], o]` storage)
                            // stores a handle the same way.
                            let crosses_reference =
                                place.projection_tys.iter().rev().skip(1).any(|ty| {
                                    matches!(ty, Ty::Ref(_))
                                        || super::references::single_pointee_pointer(Some(ty))
                                });
                            if crosses_reference {
                                let composed = Value::Ref {
                                    frame: frame_id.0,
                                    slot: place.root as usize,
                                    projection: Vec::new(),
                                };
                                let composed = self
                                    .extend_reference(&composed, place, regs)?
                                    .expect("a composed root handle extends");
                                let value = self.read_reference(&composed, frame_id, vars)?;
                                if matches!(value, Value::Ref { .. })
                                    && matches!(place.ty, Some(Ty::Ref(_)))
                                {
                                    self.read_reference(&value, frame_id, vars)?
                                } else {
                                    value
                                }
                            } else {
                                let value = load_place(vars, regs, place)?;
                                if matches!(place.ty, Some(Ty::Ref(_))) {
                                    self.read_reference(&value, frame_id, vars)?
                                } else {
                                    value.clone()
                                }
                            }
                        }
                    }
                };
            }
            MirInstr::MovePlace { dest, place } => {
                // A partial move `p.a^`: transfer the field's value out, leaving a
                // `Moved` tombstone so a later drop of the whole struct skips it (no
                // double-drop) and any stray use fails loudly. The ownership analysis
                // has already proven the moved field is not read again.
                let reference = self.extend_reference(&vars[place.root as usize], place, regs)?;
                let value = if let Some(reference) = reference {
                    let old = self.read_reference(&reference, frame_id, vars)?;
                    self.write_reference(&reference, frame_id, vars, Value::Moved)?;
                    old
                } else {
                    std::mem::replace(nav_mut(vars, regs, place)?, Value::Moved)
                };
                if matches!(value, Value::Moved) {
                    return Err(RuntimeError::TypeError(
                        "vm: partial use of an already-moved place".into(),
                    ));
                }
                regs[dest.0 as usize] = value;
            }
            // Iterator protocol (`for`): execute the nominal normalization chain
            // chosen by the checker. Only VM-CTFE's explicit ComptimeList and
            // the compiler-private heterogeneous runtime-pack carrier have a
            // method-free fallback in HasNext/Next below; public Tuple values
            // are nominal structs and must use their checked methods.
            MirInstr::GetIter {
                source,
                dest,
                mode: _,
                prepare,
            } => {
                // Execute the exact normalization chain chosen by the checker,
                // starting from the source and producing the iterator in `dest`.
                // When `dest != source` the source stays live in its own slot (a
                // borrowing iterator must not clobber its only owner); when they
                // are the same slot this normalizes in place, unchanged.
                let dynamic_prepare = prepare
                    .iter()
                    .find(|symbol| symbol.starts_with("__trait_dispatch."))
                    .cloned();
                let mut current = vars[*source as usize].clone();
                // A borrowed named source binds the source slot to a reference;
                // follow it to the underlying source struct for method resolution.
                // A borrowed `__iter__(ref self)` re-roots at the source below via
                // `reference_to_place_parts`, so this deref is only for the name.
                if matches!(current, Value::Ref { .. }) {
                    current = self.read_reference(&current, frame_id, vars)?;
                }
                for (step, selected) in prepare.iter().enumerate() {
                    let Value::Struct { name, .. } = &current else {
                        return Err(RuntimeError::TypeError(format!(
                            "vm: checked iterator preparation applied to {}",
                            crate::runtime::type_name(&current)
                        )));
                    };
                    let sname = name.clone();
                    let mut target =
                        prog.runtime_method_name(&sname, "__iter__", Some(selected.as_str()), 0);
                    // A borrowed conformer may spell its receiver `self` (Read)
                    // or `ref self` (Ref); the checker pinned one spelling in
                    // the abstract dispatch symbol, so probe the sibling before
                    // giving up.
                    if prog.index_of(&target).is_none()
                        && let Some(alternate) =
                            crate::symbol::borrowed_iterator_dispatch_alternate(selected)
                    {
                        target = prog.runtime_method_name(
                            &sname,
                            "__iter__",
                            Some(alternate.as_str()),
                            0,
                        );
                    }
                    let fidx = prog.index_of(&target).ok_or_else(|| {
                        RuntimeError::Unsupported(format!(
                            "vm: checked iterator method '{target}' is missing from MIR"
                        ))
                    })?;
                    // SPIKE seam A: a borrowed `__iter__(ref self)` receives a handle
                    // to the source slot so the iterator's borrow roots at the loop
                    // frame; caller-reachable so that root resolves inside `__iter__`.
                    let borrowed = step == 0
                        && prog.mir.functions[fidx]
                            .1
                            .ref_params
                            .first()
                            .copied()
                            .unwrap_or(false);
                    let arg = if borrowed {
                        Self::reference_to_place_parts(
                            frame_id,
                            regs,
                            vars,
                            &MirPlace::root(*source, None),
                        )?
                    } else {
                        current.clone()
                    };
                    let (value, _, _) = self.call_frame_caller_reachable(
                        prog,
                        fidx,
                        vec![arg],
                        frame_id,
                        function,
                        vars,
                    )?;
                    current = value;
                }
                // A bounded `Iterable` may expose another iterable as its
                // associated `Iter` (the self-hosted Set yields a List). Its
                // concrete normalization depth is known only after generic
                // specialization, so repeat the checked trait operation until
                // the runtime type is an iterator. Concrete source types carry
                // their complete static `prepare` chain and skip this path.
                if let Some(selected) = dynamic_prepare {
                    for _ in 0..8 {
                        let Value::Struct { name, .. } = &current else {
                            break;
                        };
                        let sname = name.clone();
                        let next = prog.runtime_method_name(&sname, "__next__", None, 0);
                        if prog.index_of(&next).is_some() {
                            break;
                        }
                        let target = prog.runtime_method_name(
                            &sname,
                            "__iter__",
                            Some(selected.as_str()),
                            0,
                        );
                        let fidx = prog.index_of(&target).ok_or_else(|| {
                            RuntimeError::Unsupported(format!(
                                "vm: checked iterator method '{target}' is missing from MIR"
                            ))
                        })?;
                        current = self.call_function(prog, fidx, vec![current.clone()], &[])?;
                    }
                }
                vars[*dest as usize] = current;
            }
            MirInstr::HasNext { dest, iter, method } => {
                let slot = *iter as usize;
                // A struct iterator reports remaining length via `__len__` (bounded
                // iteration): more elements iff `len(it) > 0`. Compiler-private
                // runtime-pack storage remains a method-free tuple; public Tuple
                // values are nominal structs and cannot reach this fallback.
                let has = if let Some(selected) = method {
                    let Value::Struct { name, .. } = &vars[slot] else {
                        return Err(RuntimeError::TypeError(format!(
                            "vm: checked iterator length applied to {}",
                            crate::runtime::type_name(&vars[slot])
                        )));
                    };
                    let sname = name.clone();
                    let target =
                        prog.runtime_method_name(&sname, "__len__", Some(selected.as_str()), 0);
                    let fidx = prog.index_of(&target).ok_or_else(|| {
                        RuntimeError::Unsupported(format!(
                            "vm: checked iterator method '{target}' is missing from MIR"
                        ))
                    })?;
                    match self
                        .call_frame_caller_reachable(
                            prog,
                            fidx,
                            vec![vars[slot].clone()],
                            frame_id,
                            function,
                            vars,
                        )?
                        .0
                    {
                        Value::Int(n) => n > 0,
                        other => {
                            return Err(RuntimeError::TypeError(format!(
                                "vm: iterator __len__ must return Int, got {}",
                                crate::runtime::type_name(&other)
                            )));
                        }
                    }
                } else {
                    match &vars[slot] {
                        Value::ComptimeList(items) if self.ctfe_fuel.is_some() => !items.is_empty(),
                        Value::Tuple(items) => !items.is_empty(),
                        other => {
                            return Err(RuntimeError::Unsupported(format!(
                                "vm: native iterator fallback reached {}; runtime collections must use nominal __iter__",
                                crate::runtime::type_name(other)
                            )));
                        }
                    }
                };
                regs[dest.0 as usize] = Value::Bool(has);
            }
            MirInstr::Next { dest, iter, call } => {
                let slot = *iter as usize;
                // A struct iterator advances via `__next__(mut self)`: the element is
                // the return value, and the advanced iterator (frame slot 0) is
                // written back into the iterator variable.
                if let Some(call) = call {
                    let Value::Struct { name, .. } = &vars[slot] else {
                        return Err(RuntimeError::TypeError(format!(
                            "vm: checked iterator next applied to {}",
                            crate::runtime::type_name(&vars[slot])
                        )));
                    };
                    let sname = name.clone();
                    let target =
                        prog.runtime_method_name(&sname, "__next__", Some(call.target.as_str()), 0);
                    let fidx = prog.index_of(&target).ok_or_else(|| {
                        RuntimeError::Unsupported(format!(
                            "vm: checked iterator method '{target}' is missing from MIR"
                        ))
                    })?;
                    let concrete_returns_reference = prog.mir.functions[fidx].1.returns_reference;
                    let (ret, mut frame_vars, returned_frame_id) = self
                        .call_frame_caller_reachable(
                            prog,
                            fidx,
                            vec![vars[slot].clone()],
                            frame_id,
                            function,
                            vars,
                        )?;
                    let adapted = self.apply_checked_result_adapter(
                        prog,
                        ret,
                        call.result_adapter,
                        concrete_returns_reference,
                        ResultAdapterFrames {
                            current: frame_id,
                            current_variables: vars,
                            returned: Some((returned_frame_id, &mut frame_vars)),
                        },
                    )?;
                    let adapted = self.rebase_iterator_result(
                        adapted,
                        returned_frame_id,
                        &frame_vars,
                        frame_id,
                        *iter,
                    );
                    vars[slot] = frame_vars.into_iter().next().unwrap_or(Value::None);
                    regs[dest.0 as usize] = adapted;
                } else {
                    match &mut vars[slot] {
                        Value::ComptimeList(items) if self.ctfe_fuel.is_some() => {
                            regs[dest.0 as usize] = items.remove(0);
                        }
                        Value::Tuple(items) => {
                            regs[dest.0 as usize] = items.remove(0);
                        }
                        ref other => {
                            return Err(RuntimeError::Unsupported(format!(
                                "vm: native iterator fallback reached {}; runtime collections must use nominal __next__",
                                crate::runtime::type_name(other)
                            )));
                        }
                    }
                }
            }
            MirInstr::TryNext {
                dest,
                yielded,
                iter,
                call,
                exhaustion,
            } => {
                let slot = *iter as usize;
                let Value::Struct { name, .. } = &vars[slot] else {
                    return Err(RuntimeError::TypeError(format!(
                        "vm: checked raising iterator next applied to {}",
                        crate::runtime::type_name(&vars[slot])
                    )));
                };
                let receiver_name = name.clone();
                let target = prog.runtime_method_name(
                    &receiver_name,
                    "__next__",
                    Some(call.target.as_str()),
                    0,
                );
                let fidx = prog.index_of(&target).ok_or_else(|| {
                    RuntimeError::Unsupported(format!(
                        "vm: checked iterator method '{target}' is missing from MIR"
                    ))
                })?;
                let concrete_returns_reference = prog.mir.functions[fidx].1.returns_reference;
                match self.call_frame_caller_reachable(
                    prog,
                    fidx,
                    vec![vars[slot].clone()],
                    frame_id,
                    function,
                    vars,
                ) {
                    Ok((element, mut frame_vars, returned_frame_id)) => {
                        let adapted = self.apply_checked_result_adapter(
                            prog,
                            element,
                            call.result_adapter,
                            concrete_returns_reference,
                            ResultAdapterFrames {
                                current: frame_id,
                                current_variables: vars,
                                returned: Some((returned_frame_id, &mut frame_vars)),
                            },
                        )?;
                        let adapted = self.rebase_iterator_result(
                            adapted,
                            returned_frame_id,
                            &frame_vars,
                            frame_id,
                            *iter,
                        );
                        vars[slot] = frame_vars.into_iter().next().unwrap_or(Value::None);
                        regs[dest.0 as usize] = adapted;
                        regs[yielded.0 as usize] = Value::Bool(true);
                    }
                    Err(RuntimeError::Raised(error)) => {
                        let catches_exhaustion = matches!(
                            (&error, exhaustion),
                            (
                                Value::Struct { name, .. },
                                Ty::Struct(expected, arguments)
                            ) if arguments.is_empty() && name == expected
                        );
                        if !catches_exhaustion {
                            return Err(RuntimeError::Raised(error));
                        }
                        regs[dest.0 as usize] = Value::None;
                        regs[yielded.0 as usize] = Value::Bool(false);
                    }
                    Err(other) => return Err(other),
                }
            }
            // ASAP destruction (Stage 7): drop the value at the variable's last
            // use, running its `__deinit__` if it has one.
            MirInstr::DropVar { var } => {
                let v = std::mem::replace(&mut vars[*var as usize], Value::None);
                self.drop_value(prog, v)?;
            }
            MirInstr::ConsumeVar { var } => {
                let value = std::mem::replace(&mut vars[*var as usize], Value::Moved);
                if let Value::Struct { name, .. } = &value {
                    self.record_lifecycle(format!("consume {name}"));
                }
                if let Value::Struct { fields, .. } = value {
                    // The named explicit destructor has already consumed the
                    // aggregate. Its fields still receive their ordinary
                    // reverse-order destruction after that call succeeds.
                    for (_, field) in fields.into_iter().rev() {
                        self.drop_value(prog, field)?;
                    }
                }
            }
            MirInstr::ConsumePlace { place, .. } => {
                let value = std::mem::replace(nav_mut(vars, regs, place)?, Value::Moved);
                if let Value::Struct { fields, .. } = value {
                    for (_, field) in fields.into_iter().rev() {
                        self.drop_value(prog, field)?;
                    }
                }
            }
            MirInstr::Unsupported(what) => {
                return Err(RuntimeError::Unsupported(format!(
                    "vm backend does not support {what} yet"
                )));
            }
            MirInstr::Raise { src } => {
                // Raise an error, propagating as `Raised` — the nearest enclosing
                // `Try` (if any) intercepts it; otherwise it unwinds the frame.
                let error = match &regs[src.0 as usize] {
                    Value::Str(message) => Value::Error(message.clone()),
                    // A raised nominal String reads its buffer back through
                    // the struct-to-literal bridge.
                    value @ Value::Struct { name, .. }
                        if crate::symbol::is_stdlib_string_struct(name) =>
                    {
                        match self.string_struct_literal(value)? {
                            Value::Str(message) => Value::Error(message),
                            other => other,
                        }
                    }
                    other => other.clone(),
                };
                if let Value::Error(message) = &error {
                    self.record_lifecycle(format!("raise {message}"));
                }
                return Err(RuntimeError::Raised(error));
            }
            MirInstr::Try {
                body,
                handler,
                orelse,
                finalbody,
                cleanup,
            } => {
                // A `try` may complete with a `return` that crossed its boundary;
                // propagate that outcome to the block driver.
                return self.exec_try(
                    prog,
                    function,
                    frame_id,
                    TryRegions {
                        body,
                        handler,
                        orelse,
                        finalbody,
                        cleanup,
                    },
                    CallerFrame {
                        id: frame_id,
                        registers: regs,
                        variables: vars,
                    },
                );
            }
            MirInstr::Drop { .. } => {
                return Err(RuntimeError::Unsupported(format!(
                    "vm backend does not support this operation yet: {i:?}"
                )));
            }
        }
        Ok(Flow::Normal)
    }

    /// Execute a `try`/`except`/`else`/`finally` region. Each sub-part runs as a
    /// mini-CFG in the current frame; a raise in
    /// the body unwinds to `handler` (after running the `cleanup` drops), `else` runs
    /// on normal completion, and `finally` always runs (its raise wins).
    pub(super) fn exec_try(
        &mut self,
        prog: &Prog,
        function: usize,
        frame_id: FrameId,
        regions: TryRegions<'_>,
        frame: CallerFrame<'_>,
    ) -> Result<Flow, RuntimeError> {
        let TryRegions {
            body,
            handler,
            orelse,
            finalbody,
            cleanup,
        } = regions;
        let CallerFrame {
            id: _,
            registers: regs,
            variables: vars,
        } = frame;
        let outcome = match self.run_region(prog, function, frame_id, body, regs, vars) {
            // The body raised: run the exceptional-edge cleanup (destroy the body's
            // locals as they go out of scope), then dispatch to the handler or
            // re-propagate.
            Err(RuntimeError::Raised(error)) => {
                self.run_cleanup(prog, cleanup, vars)?;
                match handler {
                    Some((err_slot, hblocks)) => {
                        if let Value::Error(message) = &error {
                            self.record_lifecycle(format!("catch {message}"));
                        }
                        if let Some(slot) = err_slot {
                            vars[*slot as usize] = error.clone();
                        }
                        self.run_region(prog, function, frame_id, hblocks, regs, vars)
                    }
                    None => Err(RuntimeError::Raised(error)),
                }
            }
            // A non-raised runtime error propagates untouched.
            Err(other) => Err(other),
            // The body completed (normally, or via a `return` that crossed out): its
            // locals go out of scope here too. `else` runs only on *normal*
            // completion; a `return` from the body skips `else` and carries out.
            Ok(flow) => {
                self.run_cleanup(prog, cleanup, vars)?;
                match flow {
                    Flow::Normal => match orelse {
                        Some(eblocks) => {
                            self.run_region(prog, function, frame_id, eblocks, regs, vars)
                        }
                        None => Ok(Flow::Normal),
                    },
                    ret => Ok(ret),
                }
            }
        };
        // `finally` always runs; if it raises or itself transfers control
        // (`return`/`break`/`continue`), that outcome wins over the pending one
        // (Python/Mojo semantics).
        if let Some(fblocks) = finalbody {
            let pending_cleanup = match &outcome {
                Ok(Flow::Return { cleanup, .. }) => cleanup.clone(),
                _ => Vec::new(),
            };
            match self.run_region(prog, function, frame_id, fblocks, regs, vars) {
                Ok(Flow::Normal) => {}
                Ok(Flow::Return {
                    value,
                    cleanup: mut overriding_cleanup,
                }) => {
                    // A return in `finally` wins, but values owned by the return
                    // it overrides still leave scope. Preserve the overriding
                    // return's inner-to-outer order and append only distinct
                    // pending roots.
                    for variable in pending_cleanup {
                        if !overriding_cleanup.contains(&variable) {
                            overriding_cleanup.push(variable);
                        }
                    }
                    return Ok(Flow::Return {
                        value,
                        cleanup: overriding_cleanup,
                    });
                }
                Ok(non_normal) => {
                    self.run_cleanup(prog, &pending_cleanup, vars)?;
                    return Ok(non_normal);
                }
                Err(error) => {
                    self.run_cleanup(prog, &pending_cleanup, vars)?;
                    return Err(error);
                }
            }
        }
        outcome
    }

    /// Destroy a `try` body's local variables as they leave scope — a `DropVar` on
    /// each cleanup slot (a no-op on an already-emptied/`None` slot, so it is safe
    /// whether or not the value was already dropped before a raise).
    pub(super) fn run_cleanup(
        &mut self,
        prog: &Prog,
        cleanup: &[VarId],
        vars: &mut [Value],
    ) -> Result<(), RuntimeError> {
        for &v in cleanup {
            let old = std::mem::replace(&mut vars[v as usize], Value::None);
            self.drop_value(prog, old)?;
        }
        Ok(())
    }

    /// Run a `try` sub-region's mini-CFG (block ids local, entry = 0) in the current
    /// frame. Returns the control-flow outcome — `Flow::Normal` on normal completion
    /// (`FallOff`), or `Flow::Return` when a `return` inside the region crosses out —
    /// and propagates a raise as `RuntimeError::Raised`. (`break`/`continue` crossing
    /// the region are refused at lowering, so no such terminator reaches here.)
    pub(super) fn run_region(
        &mut self,
        prog: &Prog,
        function: usize,
        frame_id: FrameId,
        blocks: &[MirBlock],
        regs: &mut [Value],
        vars: &mut Vec<Value>,
    ) -> Result<Flow, RuntimeError> {
        let mut block = 0usize;
        loop {
            let b = &blocks[block];
            for instr in &b.instrs {
                // A non-`Normal` outcome from a nested `try` (a `return`, or a
                // `break`/`continue` escaping to an outer loop) leaves this region
                // carrying that outcome.
                match self.exec_instr(prog, instr, function, frame_id, regs, vars)? {
                    Flow::Normal => {}
                    non_normal => return Ok(non_normal),
                }
            }
            match &b.term {
                MirTerm::Jump(t) => block = *t,
                MirTerm::Branch {
                    cond,
                    then_b,
                    else_b,
                } => {
                    block = if is_true(&regs[cond.0 as usize]) {
                        *then_b
                    } else {
                        *else_b
                    };
                }
                MirTerm::Return(r) => {
                    let v = r
                        .as_ref()
                        .map(|r| regs[r.0 as usize].clone())
                        .unwrap_or(Value::None);
                    return Ok(Flow::Return {
                        value: v,
                        cleanup: Vec::new(),
                    });
                }
                MirTerm::ReturnWithCleanup { value, cleanup } => {
                    let value = value
                        .as_ref()
                        .map(|register| regs[register.0 as usize].clone())
                        .unwrap_or(Value::None);
                    return Ok(Flow::Return {
                        value,
                        cleanup: cleanup.clone(),
                    });
                }
                MirTerm::FallOff => return Ok(Flow::Normal),
                // A `break`/`continue` targeting an outer function loop: run this
                // region's escape-edge cleanup (values that die leaving the region),
                // then carry the resolved target out as a `Flow::Jump`.
                MirTerm::EscapeJump { target, cleanup } => {
                    self.run_cleanup(prog, cleanup, vars)?;
                    return Ok(Flow::Jump(*target));
                }
            }
        }
    }

    /// Preserve a reference yielded directly from an iterator receiver after
    /// the temporary `__next__` frame is discarded. Stored handles are first
    /// canonicalized while that frame remains available. A handle still rooted
    /// at receiver slot zero then names the caller's write-back iterator slot,
    /// which contains the same advanced receiver after the call completes.
    fn rebase_iterator_result(
        &self,
        value: Value,
        returned_frame: FrameId,
        returned_variables: &[Value],
        caller_frame: FrameId,
        iterator: VarId,
    ) -> Value {
        let Value::Ref {
            frame,
            slot,
            projection,
        } = value
        else {
            return value;
        };
        let (frame, slot, projection) = self.canonical_reference_parts(
            returned_frame,
            returned_variables,
            frame,
            slot,
            projection,
        );
        if frame == returned_frame.0 && slot == 0 {
            Value::Ref {
                frame: caller_frame.0,
                slot: iterator as usize,
                projection,
            }
        } else {
            Value::Ref {
                frame,
                slot,
                projection,
            }
        }
    }
}
