//! The `exec_instr` instruction dispatcher and the error a `raise`
//! propagates as.
//! Extracted from `backend/vm.rs`; see `docs/symbol-map.md`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_mir::mir::verify::instruction_places;
use mojito_types::types::TyKind;

impl VmBackend {
    /// Execute one straight-line MIR instruction against the current frame.
    #[allow(
        clippy::cognitive_complexity,
        clippy::too_many_lines,
        reason = "TODO: split this pass"
    )]
    pub(super) fn exec_instr(
        &mut self,
        prog: &Prog,
        i: &MirInstr,
        scope: FrameScope,
        regs: &mut [Value],
        vars: &mut Vec<Value>,
    ) -> Result<(), RuntimeError> {
        let FrameScope {
            function,
            id: frame_id,
        } = scope;
        // The executing frame's function, before any callee shadows the name.
        let caller_function = function;
        self.burn_ctfe()?;
        give_storage(prog, function, i, vars);
        match i {
            MirInstr::EstablishLoans { .. } => {
                return Err(parametric_instruction("loan marker"));
            }
            MirInstr::MarkInitialized { .. } => {
                return Err(parametric_instruction("initialization mark"));
            }
            MirInstr::MarkDestroyed { .. } => {
                return Err(parametric_instruction("destruction mark"));
            }
            MirInstr::InvalidateInteriors { .. } => {
                return Err(parametric_instruction("interior invalidation"));
            }
            MirInstr::MakeRef { dest, place } => {
                let root = vars[place.root as usize].clone();
                // A tracked pointer variable holding a heap pointer (a
                // constructor's `Pointer[T, Self.origin]` parameter bound to
                // `xs.unsafe_ptr()`) is its own handle: forward the pointer,
                // applying an element projection as pointer arithmetic (a
                // single-pointee deref designates the pointee the pointer
                // already addresses).
                if let Value::Pointer { allocation, offset } = root
                    && prog
                        .held_field(place.root_ty.as_ref().map(peel_references))
                        .is_none()
                {
                    // A reference within the pointee (`q[i]._data[k]`, a
                    // `Span` member's element reference inlined at the
                    // pointee place `q[i]`) is the heap address the pointee's
                    // own pointer gives it; one to any other part of a heap
                    // element (`q[i].field`) is rooted at the pointer to the
                    // element ([`Self::pointer_root`]).
                    if let Some((element, suffix)) =
                        super::references::pointer_element(&root, place, regs)?
                        && let Value::Pointer {
                            allocation: element_allocation,
                            offset: element_offset,
                        } = element
                    {
                        let referent = self.read_reference(&element, frame_id, vars)?;
                        let boundary = self.reference_pointer_boundary(&referent, &suffix)?;
                        regs[dest.0 as usize] = if let Some(ReferencePointerBoundary {
                            allocation,
                            offset,
                            index,
                            suffix: [],
                        }) = boundary
                        {
                            Value::Pointer {
                                allocation,
                                offset: offset + index as i64,
                            }
                        } else {
                            Value::Ref {
                                frame: POINTER_ROOT_FRAME,
                                slot: self.pointer_root(element_allocation, element_offset),
                                projection: std::iter::once(RefProjection::Index(0))
                                    .chain(suffix)
                                    .collect(),
                            }
                        };
                        return Ok(());
                    }
                    let mut offset = offset;
                    for segment in super::references::place_projection_segments(place, regs)? {
                        match segment {
                            RefProjection::Deref => {}
                            RefProjection::Index(index) => offset += index as i64,
                            other => {
                                return Err(RuntimeError::TypeError(format!(
                                    "vm: cannot project {other:?} through a heap pointer"
                                )));
                            }
                        }
                    }
                    regs[dest.0 as usize] = Value::Pointer { allocation, offset };
                    return Ok(());
                }
                let mut segments = super::references::place_projection_segments(place, regs)?;
                // An element index into a pointer the root holds moves the
                // place pointer the root holds (`elements[unsafe_offset=i]`).
                let root = match (root, place.root_ty.as_ref().map(Ty::kind), segments.first()) {
                    (
                        root @ Value::Ref { .. },
                        Some(TyKind::Pointer { .. }),
                        Some(&RefProjection::Index(index)),
                    ) => {
                        segments.remove(0);
                        super::references::offset_place_handle(root, index as i64)?
                    }
                    (root, ..) => root,
                };
                let (frame, slot, mut projection) = match root {
                    Value::Ref {
                        frame,
                        slot,
                        projection,
                    } => (frame, slot, projection),
                    _ => (frame_id.0, place.root as usize, Vec::new()),
                };
                projection.extend(segments);
                // A place reached through a handle a slot stores (a `to=`
                // pointer field, a `ref` field) designates the handle's
                // referent, as the native address does: the reference is
                // rooted there, so it outlives the slot holding the handle.
                let (frame, slot, projection) =
                    self.canonical_reference_parts(frame_id, vars, frame, slot, projection);
                let handle = Value::Ref {
                    frame,
                    slot,
                    projection,
                };
                let target = prog.mir.functions[function].1.reg_types.get(&dest.0);
                // A projected place ending at a stored reference forwards
                // the stored handle when the destination is typed as that
                // handle (`ref s = self.src` reborrows), as the native
                // lowering loads it.
                if !place.proj.is_empty()
                    && let (Some(TyKind::Ref(stored)), Some(TyKind::Ref(made))) =
                        (place.ty.as_ref().map(Ty::kind), target.map(Ty::kind))
                    && made.referent == stored.referent
                    && let stored @ (Value::Ref { .. } | Value::Pointer { .. }) =
                        self.read_reference(&handle, frame_id, vars)?
                {
                    regs[dest.0 as usize] = stored;
                    return Ok(());
                }
                // A reference to an element reached through a pointer the
                // place holds (`span._data[i]`) designates the heap element,
                // as such a call result does: it outlives the storage
                // holding the pointer.
                regs[dest.0 as usize] = self.heap_reference_result(handle, target, frame_id, vars);
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
            MirInstr::Rebind { .. } => {
                return Err(parametric_instruction("a value rebind"));
            }
            MirInstr::ComptimeAssert { .. } => {
                return Err(parametric_instruction("a compile-time assertion"));
            }
            MirInstr::If { .. }
            | MirInstr::Loop { .. }
            | MirInstr::Match { .. }
            | MirInstr::Try { .. } => {
                return Err(RuntimeError::Unsupported(format!(
                    "vm: `{}` outside a region cursor",
                    mojito_mir::mir::text::instruction_mnemonic(i)
                )));
            }
            MirInstr::ComptimeIf { .. } | MirInstr::ComptimeFor { .. } => {
                return Err(parametric_instruction(&format!(
                    "structured control flow `{}`",
                    mojito_mir::mir::text::instruction_mnemonic(i)
                )));
            }
            MirInstr::CopyValue {
                dest,
                value,
                constructor,
            } => {
                let source = regs[value.0 as usize].clone();
                regs[dest.0 as usize] = if self.has_copyinit {
                    self.copy_named(
                        prog,
                        &source,
                        prog.mir.functions[function].1.reg_types.get(&dest.0),
                        constructor.as_deref(),
                    )?
                } else {
                    source
                };
            }
            MirInstr::WriteRef { reference, value } => {
                let handle = regs[reference.0 as usize].clone();
                self.write_reference(&handle, frame_id, vars, regs[value.0 as usize].clone())?;
            }
            MirInstr::MakeClosure {
                dest,
                function: target,
                captures,
            } => {
                let mut environment = Vec::with_capacity(captures.len());
                for capture in captures {
                    let reference =
                        Self::reference_to_place_parts(frame_id, regs, vars, &capture.place)?;
                    // A moved capture moves by the constructor it names, as
                    // a `var.use` does.
                    let ty = capture
                        .place
                        .storage_ty(&prog.mir.functions[function].1.var_tys);
                    let constructor = capture.constructor.as_deref();
                    let value = match capture.mode {
                        MirCaptureMode::Reference => reference,
                        MirCaptureMode::Move => {
                            let value = self.read_reference(&reference, frame_id, vars)?;
                            self.write_reference(&reference, frame_id, vars, Value::Moved)?;
                            if self.has_moveinit {
                                self.move_named(prog, value, ty, constructor)?
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
                        destructor: capture.destructor.clone(),
                    });
                }
                regs[dest.0 as usize] = Value::Closure {
                    function: target.clone(),
                    captures: environment,
                };
            }
            MirInstr::KeepAlive { .. } => {
                return Err(parametric_instruction("keep-alive marker"));
            }
            MirInstr::Const { dest, k } => {
                let value = match k {
                    Const::Value(value) => self.thaw(
                        prog,
                        value,
                        prog.mir.functions[function].1.reg_types.get(&dest.0),
                    )?,
                    Const::UninitMem => Self::uninit_mem_value(
                        prog,
                        prog.mir.functions[function].1.reg_types.get(&dest.0),
                    ),
                    _ => const_value(k)?,
                };
                regs[dest.0 as usize] = value;
            }
            MirInstr::ParamListAddress { dest, values } => {
                let Const::Value(mojito_types::ct::CtValue::Tuple(items)) = values else {
                    return Err(RuntimeError::Unsupported(format!(
                        "the parameter list `{values:?}` has no elements to address"
                    )));
                };
                let Some(TyKind::Pointer { element, .. }) =
                    (prog.mir.functions[function].1.reg_types.get(&dest.0)).map(Ty::kind)
                else {
                    return Err(RuntimeError::Unsupported(
                        "a parameter list's address needs a pointer register".to_string(),
                    ));
                };
                let allocation = self.static_param_list(prog, element, items)?;
                regs[dest.0 as usize] = Value::Pointer {
                    allocation,
                    offset: 0,
                };
            }
            // A register is a value its readers copy, which `cse` may give
            // more readers than this one.
            MirInstr::StoreToMem { dest, src } => {
                let value = regs[src.0 as usize].clone();
                regs[dest.0 as usize] = self.store_to_comptime_memory(value);
            }
            MirInstr::ConstructTypeParam { param, .. } => {
                return Err(parametric_instruction(&format!(
                    "a construction of the type parameter `{}`",
                    param.name
                )));
            }
            MirInstr::TypeName { ty, .. } => {
                return Err(parametric_instruction(&format!(
                    "the name of the type `{ty}`"
                )));
            }
            MirInstr::ConstructType { ty, .. } => {
                return Err(parametric_instruction(&format!(
                    "a construction of the type `{ty}`"
                )));
            }
            MirInstr::SizeOf { ty, .. } => {
                return Err(parametric_instruction(&format!(
                    "the layout query of `{ty}`"
                )));
            }
            MirInstr::MaterializeLiteral {
                dest,
                value,
                target,
            } => {
                regs[dest.0 as usize] =
                    crate::runtime::materialize_literal(regs[value.0 as usize].clone(), target)?;
            }
            MirInstr::ScalarCast {
                dest,
                value,
                target,
            } => {
                if self.ctfe_fuel.is_some() {
                    let integral = match target {
                        ScalarConversion::Int => Some(mojito_ast::ast::Dtype::Int),
                        ScalarConversion::UInt => Some(mojito_ast::ast::Dtype::UInt),
                        ScalarConversion::Float64 | ScalarConversion::Bool => None,
                    };
                    if let Some(integral) = integral {
                        crate::runtime::ctfe_float_to_int(integral, &regs[value.0 as usize])?;
                    }
                }
                regs[dest.0 as usize] =
                    crate::runtime::builtin_convert(*target, regs[value.0 as usize].clone())?;
            }
            // A `Pointer`, its `!kgen.pointer`, and a `!lit.ref` are one
            // address.
            MirInstr::PointerConvert { dest, value } => {
                regs[dest.0 as usize] = regs[value.0 as usize].clone();
            }
            // An `index` and its `Int` storage are one value; an `i1` is
            // held as a `Bool`, and its `!kgen.scalar<bool>` storage as the
            // one-lane `bool` vector.
            MirInstr::BuiltinCast { dest, value } => {
                regs[dest.0 as usize] = match &regs[value.0 as usize] {
                    bit @ Value::Bool(_) => simd_from_values(
                        mojito_ast::ast::Dtype::Bool,
                        1,
                        std::slice::from_ref(bit),
                    )?,
                    lane @ Value::Simd { .. } => {
                        crate::runtime::builtin_convert(ScalarConversion::Bool, lane.clone())?
                    }
                    value => value.clone(),
                };
            }
            MirInstr::IsRunInComptimeInterpreter { dest } => {
                regs[dest.0 as usize] = Value::Bool(self.ctfe_fuel.is_some());
            }
            MirInstr::Pop { dest, op, operands } if op.is_string_read() => {
                let Some(Value::Attribute(string)) =
                    operands.first().map(|operand| &regs[operand.0 as usize])
                else {
                    return Err(RuntimeError::TypeError(format!(
                        "'pop.{}' reads a '{}'",
                        op.mnemonic(),
                        mojito_types::types::KGEN_STRING_TYPE_SPELLING
                    )));
                };
                let unsupported = |error: mojito_types::param_expr::ParamError| {
                    RuntimeError::Unsupported(error.to_string())
                };
                regs[dest.0 as usize] = if *op == mojito_types::mlir::PopOp::StringSize {
                    let size = mojito_types::param_expr::fold::fold_string_size(string)
                        .map_err(unsupported)?;
                    super::ct_value_as_runtime(size).ok_or_else(|| {
                        RuntimeError::Unsupported("'pop.string.size' gives no index".to_string())
                    })?
                } else {
                    let address = mojito_types::param_expr::fold::fold_string_address(string)
                        .map_err(unsupported)?;
                    self.thaw(
                        prog,
                        &address,
                        Some(&mojito_types::types::static_string_address_ty()),
                    )?
                };
            }
            MirInstr::Pop { dest, op, operands } => {
                let operands: Vec<Value> = operands
                    .iter()
                    .map(|operand| regs[operand.0 as usize].clone())
                    .collect();
                regs[dest.0 as usize] = crate::runtime::pop_op(*op, &operands)?;
            }
            MirInstr::CallLlvmIntrinsic {
                dest,
                intrinsic,
                args,
                ..
            } => {
                let called = intrinsic.llvm_intrinsic().ok_or_else(|| {
                    RuntimeError::Unsupported(format!(
                        "the LLVM intrinsic `{intrinsic:?}` is outside the subset"
                    ))
                })?;
                if self.ctfe_fuel.is_some()
                    && let Some(reason) = called.compile_time_error()
                {
                    return Err(RuntimeError::Unsupported(reason));
                }
                let operands: Vec<Value> = args
                    .iter()
                    .map(|operand| regs[operand.0 as usize].clone())
                    .collect();
                let result = prog.mir.functions[function]
                    .1
                    .reg_types
                    .get(&dest.0)
                    .map(|ty| prog.mir.declarations.translated_type(ty));
                regs[dest.0 as usize] =
                    crate::runtime::llvm_intrinsic(called, result.as_ref(), &operands)?;
            }
            MirInstr::UseVar {
                dest,
                var,
                mode,
                constructor,
            } => {
                let slot = *var as usize;
                // A `^` move **transfers** the value out of the source slot, leaving
                // a `Moved` tombstone, and runs the move initializer unless it
                // forwards the slot to an owned parameter; any other use copies. Either way, touching an
                // already-moved slot is a use-after-move — a loud runtime error (the
                // ownership analysis rejects this statically, so this only fires on a
                // compiler bug).
                // A moved owned generic `T` may itself be a reference value;
                // transfer that handle intact. Non-moving UseVar remains the
                // compatibility read path for out/ref receiver slots whose
                // handle ABI predates explicit LoadPlace/ReadRef. Checked
                // reference-valued storage sites use Borrow adjustments and do
                // not rely on that compatibility path.
                let value = if mode.moves() {
                    let moved = std::mem::replace(&mut vars[slot], Value::Moved);
                    if self.has_moveinit && *mode == mojito_mir::mir::UseMode::Move {
                        self.move_named(
                            prog,
                            moved,
                            prog.mir.functions[function].1.var_tys.get(var),
                            constructor.as_deref(),
                        )?
                    } else {
                        moved
                    }
                } else if let Value::Ref { .. } = &vars[slot]
                    && !matches!(
                        (prog.mir.functions[function].1.var_tys.get(var)).map(Ty::kind),
                        Some(
                            TyKind::Pointer { .. } | TyKind::KgenPointer(_) | TyKind::LitRef { .. }
                        )
                    )
                {
                    // A pointer-typed variable holding a place handle (a
                    // `Pointer(to=x)` local, a bound pointer parameter, or
                    // its `!kgen.pointer` or `!lit.ref`) is the handle
                    // itself; only reference bindings read through their
                    // stored handle.
                    self.read_reference(&vars[slot], frame_id, vars)?
                } else {
                    match mode {
                        mojito_mir::mir::UseMode::Move | mojito_mir::mir::UseMode::Forward => {
                            unreachable!("moving uses are handled before reference compatibility")
                        }
                        mojito_mir::mir::UseMode::BorrowShared
                        | mojito_mir::mir::UseMode::BorrowMut => vars[slot].clone(),
                        // A copy runs `__copyinit__` (deep copy) for a lifecycle type;
                        // otherwise a plain deep `Clone`.
                        mojito_mir::mir::UseMode::Copy if self.has_copyinit => self.copy_named(
                            prog,
                            &vars[slot],
                            prog.mir.functions[function].1.var_tys.get(var),
                            constructor.as_deref(),
                        )?,
                        mojito_mir::mir::UseMode::Copy => vars[slot].clone(),
                    }
                };
                if matches!(value, Value::Moved) {
                    return Err(RuntimeError::TypeError(format!(
                        "vm: use of variable slot {slot} after it was moved"
                    )));
                }
                regs[dest.0 as usize] = value;
            }
            MirInstr::DefVar { var, src, .. } if prog.error_slots[function].contains(var) => {
                vars[*var as usize] = self.stage_error(prog, function, regs, *src)?;
            }
            MirInstr::DefVar { var, src, .. }
                if prog.mir.functions[function].1.error_slot == Some(*var) =>
            {
                vars[*var as usize] = match self.raise_register(prog, function, regs, *src) {
                    RuntimeError::Raised(error) => error,
                    raised => return Err(raised),
                };
            }
            MirInstr::DefVar {
                var,
                src,
                binding_ty,
                ..
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
                    // A pointer-typed slot stores its handle *as* the pointer
                    // value (`UseVar` reads it intact), so assigning to it —
                    // `p = q` on a local or on a `mut p: Pointer[...]`
                    // parameter — replaces the slot instead.
                    let writes_through = matches!(&vars[slot], Value::Ref { .. })
                        && !matches!(
                            (prog.mir.functions[function].1.var_tys.get(var)).map(Ty::kind),
                            Some(TyKind::Pointer { .. })
                        );
                    let current = if writes_through {
                        self.read_reference(&vars[slot], frame_id, vars)?
                    } else {
                        vars[slot].clone()
                    };
                    let assigned = crate::runtime::coerce_like(v, &current);
                    if writes_through {
                        let handle = vars[slot].clone();
                        self.write_reference(&handle, frame_id, vars, assigned)?;
                    } else {
                        vars[slot] = assigned;
                    }
                }
            }
            MirInstr::UnOp { op, dest, a, .. } => {
                regs[dest.0 as usize] = self.apply_prefix(prog, *op, regs[a.0 as usize].clone())?;
            }
            MirInstr::BinOp {
                op,
                dest,
                a,
                b,
                resolved,
                ..
            } => {
                let l = regs[a.0 as usize].clone();
                let r = regs[b.0 as usize].clone();
                let value = self.apply_binop(prog, *op, l, r, resolved.as_deref())?;
                // A generic instance types a comparison over its parameter
                // `Bool`, as its bound declares; sized scalars yield a mask.
                let declared_bool = matches!(value, Value::Simd { .. })
                    && prog.mir.functions[function].1.reg_types.get(&dest.0) == Some(&Ty::bool());
                regs[dest.0 as usize] = if declared_bool {
                    crate::runtime::builtin_convert(ScalarConversion::Bool, value)?
                } else {
                    value
                };
            }
            MirInstr::Call {
                dest,
                func,
                args,
                kwargs,
                arg_places,
                kwarg_places,
                instantiated_args,
                ..
            } => {
                let mut argv: Vec<Value> =
                    args.iter().map(|r| regs[r.0 as usize].clone()).collect();
                let mut kw: Vec<(String, Value)> = kwargs
                    .iter()
                    .map(|(n, r)| (n.clone(), regs[r.0 as usize].clone()))
                    .collect();
                // A handwritten constructor receives reference arguments as
                // caller-frame handles, just like an ordinary ref-parameter call.
                // Its synthetic `self` occupies parameter slot zero.
                let constructed =
                    prog.declared_struct(prog.mir.functions[function].1.reg_types.get(&dest.0));
                let constructor_index = prog
                    .lifecycle_constructor(&func.0, constructed)
                    .and_then(|_| prog.index_of(&func.0));
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
                        *value = self.reference_argument(frame_id, regs, vars, place)?;
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
                            *value = self.reference_argument(frame_id, regs, vars, place)?;
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
                let result = if let Some(idx) = writeback {
                    self.call_with_writeback(
                        prog,
                        WritebackCall {
                            function_name: &func.0,
                            function_index: idx,
                            positional_args: argv,
                            keyword_args: kw,
                            argument_places: arg_places,
                            keyword_argument_places: kwarg_places,
                        },
                        CallerFrame {
                            id: frame_id,
                            function: caller_function,
                            registers: regs,
                            variables: vars,
                        },
                    )?
                } else {
                    // Reaching the instruction interpreter means this call
                    // is executing synchronously (for example inside a
                    // structured try region); the continuation path handles
                    // ordinary direct calls earlier. Keep the caller's real
                    // frame identity reachable even without direct ref
                    // parameters because by-value arguments may contain
                    // nested reference handles.
                    // A builtin (`print`, `String`, `repr`) dispatches its
                    // operands by runtime shape, so a held struct operand
                    // passes as its box; a function's frame entry unboxes it.
                    let reg_types = &prog.mir.functions[function].1.reg_types;
                    let argv: Vec<Value> = argv
                        .into_iter()
                        .zip(args)
                        .map(|(value, register)| prog.boxed(value, reg_types.get(&register.0)))
                        .collect();
                    let kw: Vec<(String, Value)> = kw
                        .into_iter()
                        .zip(kwargs)
                        .map(|((name, value), (_, register))| {
                            (name, prog.boxed(value, reg_types.get(&register.0)))
                        })
                        .collect();
                    let stack_base = self.push_caller_mirror(frame_id, regs, vars);
                    let arg_types: Vec<Option<mojito_types::types::Ty>> = args
                        .iter()
                        .map(|reg| {
                            prog.mir.functions[function]
                                .1
                                .reg_types
                                .get(&reg.0)
                                .cloned()
                        })
                        .collect();
                    let outcome = self.call_named(
                        prog,
                        &func.0,
                        argv,
                        kw,
                        &CallTypes {
                            instantiated: instantiated_args,
                            arg_types: &arg_types,
                            constructed,
                        },
                    );
                    self.restore_caller_mirror(stack_base, vars)?;
                    outcome?
                };
                let target = prog.mir.functions[function].1.reg_types.get(&dest.0);
                regs[dest.0 as usize] = self.materialize_checked_result(prog, result, target)?;
            }
            MirInstr::CallIndirect {
                dest,
                callee,
                args,
                kwargs,
                callee_place,
                arg_places,
                kwarg_places,
                ..
            } => {
                let callable = regs[callee.0 as usize].clone();
                let (function, captures) = match &callable {
                    Value::Function(function) => (function.clone(), Vec::new()),
                    Value::Closure {
                        function, captures, ..
                    } => (
                        function.clone(),
                        Self::closure_capture_arguments(
                            frame_id,
                            regs,
                            vars,
                            callee_place.as_deref(),
                            captures,
                            &prog.capture_writes(function),
                        )?,
                    ),
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
                let keywords: Vec<(String, Value)> = kwargs
                    .iter()
                    .map(|(name, register)| (name.clone(), regs[register.0 as usize].clone()))
                    .collect();
                let keyword_names: Vec<String> =
                    kwargs.iter().map(|(name, _)| name.clone()).collect();
                let index = prog
                    .index_of(&function)
                    .ok_or_else(|| RuntimeError::NotCallable(function.clone()))?;
                let (bound, slots) = match prog.sigs.get(&function) {
                    Some(signature) => self.bind_for_closure_call(
                        prog,
                        &function,
                        signature,
                        &positional,
                        &keywords,
                        capture_count,
                    )?,
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
                let mut reference_inputs: Vec<(usize, Value)> = Vec::new();
                for (parameter, is_ref) in definition.ref_params.iter().enumerate() {
                    let captured_argument = match slots.get(parameter) {
                        Some(ArgSlot::Positional(argument)) if *argument < capture_count => {
                            Some(*argument)
                        }
                        _ => None,
                    };
                    let handle = if let Some(argument) = captured_argument {
                        if !is_ref {
                            continue;
                        }
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
                            prog.sigs.get(&function).and_then(|signature| {
                                // A closure's storage is a method's receiver,
                                // which its signature does not list.
                                signature.frame_param_name(parameter.checked_sub(usize::from(
                                    signature.has_receiver && capture_count > 0,
                                ))?)
                            }),
                            capture_count,
                            arg_places,
                            &keyword_names,
                            kwarg_places,
                        );
                        let Some(place) = caller_handle_place(definition, parameter, place)
                            .map_err(|_| {
                                RuntimeError::Unsupported(format!(
                                    "vm: a mut/ref argument to callable '{function}' must be a place"
                                ))
                            })?
                        else {
                            continue;
                        };
                        self.reference_argument(frame_id, regs, vars, place)?
                    };
                    reference_inputs.push((parameter, handle));
                }
                let (result, _, _) = self.call_synchronously_with_references(
                    prog,
                    SynchronousCall {
                        function_index: index,
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
                ..
            } => {
                let recv_val = regs[recv.0 as usize].clone();
                // A place pointer is a reference handle here, and its
                // trait-dispatched `copy` is the handle, not its pointee's.
                let pointer_copy = method == "copy"
                    && resolved.is_none()
                    && args.is_empty()
                    && kwargs.is_empty()
                    && matches!(
                        (prog.mir.functions[function].1.reg_types.get(&recv.0)).map(Ty::kind),
                        Some(TyKind::Pointer { .. })
                    );
                if pointer_copy {
                    regs[dest.0 as usize] = recv_val;
                } else {
                    // A builtin method (`Writer.write` on the builtin
                    // string writer) formats its operands by runtime shape,
                    // so a held struct operand passes as its box.
                    let reg_types = &prog.mir.functions[function].1.reg_types;
                    let operand = |register: &Reg| {
                        prog.boxed(
                            regs[register.0 as usize].clone(),
                            reg_types.get(&register.0),
                        )
                    };
                    let argv: Vec<Value> = args.iter().map(operand).collect();
                    let kw: Vec<(String, Value)> = kwargs
                        .iter()
                        .map(|(name, reg)| (name.clone(), operand(reg)))
                        .collect();
                    let result = self.method_call(
                        prog,
                        MethodInvocation {
                            receiver: recv_val,
                            receiver_ty: reg_types.get(&recv.0),
                            method,
                            resolved_name: resolved.as_deref(),
                            result_adapter: result_adapter.as_deref().map(|adapter| {
                                ResultAdapter {
                                    kind: adapter.kind,
                                    constructor: adapter.constructor.as_deref(),
                                }
                            }),
                            arguments: argv,
                            keyword_arguments: kw,
                            receiver_place: recv_place.as_deref(),
                            argument_places: arg_places,
                            keyword_argument_places: kwarg_places,
                        },
                        CallerFrame {
                            id: frame_id,
                            function: caller_function,
                            registers: regs,
                            variables: vars,
                        },
                    )?;
                    let target = prog.mir.functions[function].1.reg_types.get(&dest.0);
                    regs[dest.0 as usize] =
                        self.materialize_checked_result(prog, result, target)?;
                }
            }
            MirInstr::PointerStorageTake {
                dest,
                pointer,
                index,
                ..
            } => {
                let index = value_as_index(&regs[index.0 as usize])?;
                regs[dest.0 as usize] = match &regs[pointer.0 as usize] {
                    Value::Pointer { allocation, offset } => {
                        self.heap_take(*allocation, *offset, index)?
                    }
                    // A place pointer (`Pointer(to=x)`) designates one value
                    // whose element the checker proved trivially destructible:
                    // taking it reads the pointee.
                    handle @ Value::Ref { .. } if index == 0 => {
                        self.read_reference(&handle.clone(), frame_id, vars)?
                    }
                    _ => {
                        return Err(RuntimeError::TypeError(
                            "vm: compiler-private storage take requires Pointer".to_string(),
                        ));
                    }
                };
            }
            MirInstr::PointerStorageDestroy {
                dest,
                pointer,
                index,
                element,
                destructor,
            } => {
                let index = value_as_index(&regs[index.0 as usize])?;
                match regs[pointer.0 as usize] {
                    Value::Pointer { allocation, offset } => {
                        self.heap_destroy(
                            prog,
                            allocation,
                            offset,
                            index,
                            Some(element),
                            destructor.as_deref(),
                        )?;
                    }
                    // A place pointer (`Pointer(to=x)`) designates one value:
                    // destroying it runs the pointee's destructor, which only
                    // a bundled module that then ends its storage
                    // (`lit.ownership.mark_destroyed`) names for a value that
                    // is not trivially destructible.
                    ref handle @ Value::Ref { .. } if index == 0 => {
                        let pointee = self.read_reference(&handle.clone(), frame_id, vars)?;
                        self.drop_named(prog, pointee, Some(element), destructor.as_deref())?;
                    }
                    _ => {
                        return Err(RuntimeError::TypeError(
                            "vm: compiler-private storage destroy requires Pointer".to_string(),
                        ));
                    }
                }
                regs[dest.0 as usize] = Value::None;
            }
            // The initializer register keeps its value, as `StoreToMem`'s does.
            MirInstr::UninitStorage { dest, init } => {
                let payload = init
                    .map(|register| regs[register.0 as usize].clone())
                    .map(Box::new);
                regs[dest.0 as usize] = Value::UninitStorage(payload);
            }
            MirInstr::UninitStorageTake { dest, storage, .. } => {
                let storage = std::mem::replace(&mut regs[storage.0 as usize], Value::Moved);
                regs[dest.0 as usize] = Self::uninit_storage_payload(storage, "take")?;
            }
            MirInstr::UninitStorageDestroy {
                dest,
                storage,
                element,
                destructor,
            } => {
                let storage = std::mem::replace(&mut regs[storage.0 as usize], Value::Moved);
                let payload = Self::uninit_storage_payload(storage, "destroy")?;
                self.drop_named(prog, payload, Some(element), destructor.as_deref())?;
                regs[dest.0 as usize] = Value::None;
            }
            MirInstr::GetField { dest, base, field } => {
                let base_ty = prog.mir.functions[function].1.reg_types.get(&base.0);
                let base = regs[base.0 as usize].clone();
                // A held struct's field is the struct's value.
                if prog.held_field(base_ty).is_some() {
                    regs[dest.0 as usize] = prog.unboxed(base, base_ty);
                    return Ok(());
                }
                // A ref-typed base register holds a handle; field access
                // addresses its referent (another read twin of the ref-field
                // second dereference).
                let base = if matches!(base, Value::Ref { .. }) {
                    self.read_reference(&base, frame_id, vars)?
                } else {
                    base
                };
                let referent_ty = base_ty.map(peel_references);
                regs[dest.0 as usize] = if prog.held_field(referent_ty).is_some() {
                    prog.unboxed(base, referent_ty)
                } else {
                    get_field(&base, field)?
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
                // ref-field store's second dereference). A pointer subscript
                // keeps its handle: a place pointer bound to a
                // placeholder-origin parameter IS the handle, and the pointer
                // intrinsic below dereferences it.
                let base_value = if matches!(base_value, Value::Ref { .. })
                    && *intrinsic != Some(MirIntrinsicSubscript::Pointer)
                {
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
                    let base_ty = prog.mir.functions[function].1.reg_types.get(&base.0);
                    let recv = match prog.boxed(base_value, base_ty.map(peel_references)) {
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
                        vec![index_place.as_deref().cloned()]
                    };
                    let result = self.method_call(
                        prog,
                        MethodInvocation {
                            receiver: recv,
                            receiver_ty: None,
                            method: "__getitem__",
                            resolved_name: Some(&call.target),
                            result_adapter: None,
                            arguments,
                            keyword_arguments: Vec::new(),
                            receiver_place: base_place.as_deref(),
                            argument_places: &argument_places,
                            keyword_argument_places: &[],
                        },
                        CallerFrame {
                            id: frame_id,
                            function: caller_function,
                            registers: regs,
                            variables: vars,
                        },
                    )?;
                    let target = prog.mir.functions[function].1.reg_types.get(&dest.0);
                    let result = self.heap_reference_result(result, target, frame_id, vars);
                    regs[dest.0 as usize] =
                        self.materialize_checked_result(prog, result, target)?;
                } else {
                    let intrinsic = intrinsic.ok_or_else(|| {
                        RuntimeError::TypeError(
                            "vm: call-less index lacks an intrinsic dispatch kind".to_string(),
                        )
                    })?;
                    // An `Indexer` struct normalizes through its
                    // `__mlir_index__`, found by runtime shape.
                    let index_operand = prog.boxed(
                        regs[index.0 as usize].clone(),
                        prog.mir.functions[function].1.reg_types.get(&index.0),
                    );
                    regs[dest.0 as usize] = match (intrinsic, base_value) {
                        // A pointer subscript is a shallow read of the
                        // pointee, as `LoadPlace` and `ReadRef` are; the
                        // checked `value.copy` makes any copy.
                        //
                        // `ptr[i]` through a place handle (a `Pointer(to=x)`
                        // bound to a placeholder-origin parameter) reads the
                        // pointee; the handle designates one value.
                        (MirIntrinsicSubscript::Pointer, handle @ Value::Ref { .. }) => {
                            let off = self.normalize_index(prog, &index_operand)?;
                            let handle = super::references::offset_place_handle(handle, off)?;
                            self.read_reference(&handle, frame_id, vars)?
                        }
                        // `ptr[i]` loads the pointee at `base + i` from the heap arena.
                        (MirIntrinsicSubscript::Pointer, Value::Pointer { allocation, offset }) => {
                            let off = self.normalize_index(prog, &index_operand)?;
                            self.heap_read(allocation, offset, off)?
                        }
                        (MirIntrinsicSubscript::ComptimeList, Value::ComptimeList(items))
                            if self.ctfe_fuel.is_some() =>
                        {
                            let idx = self.normalize_index(prog, &index_operand)?;
                            let index = crate::runtime::bounds_check(
                                idx,
                                items.len(),
                                "compile-time list index",
                            )?;
                            items[index].clone()
                        }
                        (
                            MirIntrinsicSubscript::TupleStorage
                            | MirIntrinsicSubscript::VariadicStorage,
                            value,
                        ) => {
                            let idx = self.normalize_index(prog, &index_operand)?;
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
                let subscript_value = |argument: &MirSubscriptArg| match argument {
                    MirSubscriptArg::Index(register) => regs[register.0 as usize].clone(),
                };
                let arguments = args.iter().map(subscript_value).collect();
                let call = call.as_ref().ok_or_else(|| {
                    RuntimeError::TypeError(
                        "vm: nominal multi-index lacks a checked call contract".to_string(),
                    )
                })?;
                let keyword_arguments = kwargs
                    .iter()
                    .map(|(name, argument)| (name.clone(), subscript_value(argument)))
                    .collect();
                let result = self.method_call(
                    prog,
                    MethodInvocation {
                        receiver: regs[object.0 as usize].clone(),
                        receiver_ty: prog.mir.functions[function].1.reg_types.get(&object.0),
                        method: "__getitem__",
                        resolved_name: Some(&call.target),
                        result_adapter: None,
                        arguments,
                        keyword_arguments,
                        receiver_place: object_place.as_deref(),
                        argument_places: arg_places,
                        keyword_argument_places: kwarg_places,
                    },
                    CallerFrame {
                        id: frame_id,
                        function: caller_function,
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
                let arguments = args
                    .iter()
                    .map(|argument| match argument {
                        MirSubscriptArg::Index(register) => regs[register.0 as usize].clone(),
                    })
                    .collect::<Vec<_>>();
                let keyword_arguments =
                    vec![(value_keyword.clone(), regs[value.0 as usize].clone())];
                let argument_places = arg_places.clone();
                let keyword_argument_places = vec![value_place.as_deref().cloned()];
                let _ = self.method_call(
                    prog,
                    MethodInvocation {
                        receiver: regs[receiver.0 as usize].clone(),
                        receiver_ty: prog.mir.functions[function].1.reg_types.get(&receiver.0),
                        method: "__setitem__",
                        resolved_name: Some(&call.target),
                        result_adapter: None,
                        arguments,
                        keyword_arguments,
                        receiver_place: receiver_place.as_deref(),
                        argument_places: &argument_places,
                        keyword_argument_places: &keyword_argument_places,
                    },
                    CallerFrame {
                        id: frame_id,
                        function: caller_function,
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
            MirInstr::MakeArray { dest, elems } => {
                regs[dest.0 as usize] =
                    Value::Tuple(elems.iter().map(|r| regs[r.0 as usize].clone()).collect());
            }
            MirInstr::ArrayRepeat { dest, value } => {
                let size = match prog.mir.functions[function]
                    .1
                    .reg_types
                    .get(&dest.0)
                    .map(Ty::kind)
                {
                    Some(TyKind::PopArray { size, .. }) => size
                        .known()
                        .and_then(|size| usize::try_from(size).ok())
                        .ok_or_else(|| {
                            parametric_instruction(&format!("the array size `{size}`"))
                        })?,
                    _ => {
                        return Err(RuntimeError::TypeError(
                            "array.repeat builds no `!pop.array`".to_string(),
                        ));
                    }
                };
                regs[dest.0 as usize] = Value::Tuple(vec![regs[value.0 as usize].clone(); size]);
            }
            MirInstr::ArrayGet { dest, array, index } => {
                let element = array_index(index)
                    .and_then(|index| match &regs[array.0 as usize] {
                        Value::Tuple(elements) => elements.get(index).cloned(),
                        _ => None,
                    })
                    .ok_or_else(|| {
                        RuntimeError::TypeError(format!("array.get of element {index:?}"))
                    })?;
                regs[dest.0 as usize] = element;
            }
            MirInstr::ArrayReplace {
                dest,
                array,
                value,
                index,
            } => {
                let mut replaced = regs[array.0 as usize].clone();
                let slot = array_index(index)
                    .and_then(|index| match &mut replaced {
                        Value::Tuple(elements) => elements.get_mut(index),
                        _ => None,
                    })
                    .ok_or_else(|| {
                        RuntimeError::TypeError(format!("array.replace of element {index:?}"))
                    })?;
                *slot = regs[value.0 as usize].clone();
                regs[dest.0 as usize] = replaced;
            }
            MirInstr::MakeStruct { dest, fields } => {
                let name = prog
                    .declared_struct(prog.mir.functions[function].1.reg_types.get(&dest.0))
                    .ok_or_else(|| {
                        RuntimeError::TypeError(
                            "struct.create builds no declared struct".to_string(),
                        )
                    })?;
                let values = fields.iter().map(|r| regs[r.0 as usize].clone()).collect();
                regs[dest.0 as usize] = calls::struct_value(&prog.structs[name], name, values)?;
            }
            MirInstr::MakeVariant {
                dest,
                alternatives,
                index,
                value,
                constructor,
            } => {
                let index = &known_variant_index(index)?;
                let selected = alternatives.get(*index).ok_or_else(|| {
                    RuntimeError::TypeError("Variant construction has an invalid tag".to_string())
                })?;
                let payload = self.move_payload(
                    prog,
                    regs[value.0 as usize].clone(),
                    selected,
                    constructor.as_deref(),
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
                let index = &known_variant_index(index)?;
                if *index >= alternatives.len() {
                    return Err(RuntimeError::TypeError(
                        "Variant.isa has an invalid checked tag".to_string(),
                    ));
                }
                regs[dest.0 as usize] = Value::Bool(active == index);
            }
            MirInstr::VariantTag { dest, variant } => {
                let Value::Variant { index, .. } = &regs[variant.0 as usize] else {
                    return Err(RuntimeError::TypeError(format!(
                        "an enum's discriminant read from {}",
                        crate::runtime::type_name(&regs[variant.0 as usize])
                    )));
                };
                regs[dest.0 as usize] = Value::Int(*index as i64);
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
                let index = &known_variant_index(index)?;
                let expected = alternatives.get(*index).ok_or_else(|| {
                    RuntimeError::TypeError(
                        "typed Variant projection has an invalid checked tag".to_string(),
                    )
                })?;
                if active != index {
                    let found = alternatives
                        .get(*active)
                        .map_or_else(|| "<invalid>".to_string(), ToString::to_string);
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
                let index = &known_variant_index(index)?;
                let expected = alternatives.get(*index).ok_or_else(|| {
                    RuntimeError::TypeError("Variant.take has an invalid checked tag".to_string())
                })?;
                if *checked && active != index {
                    let found = alternatives
                        .get(*active)
                        .map_or_else(|| "<invalid>".to_string(), ToString::to_string);
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
                destructors,
            } => {
                let index = &known_variant_index(index)?;
                let old = self.load_place_through_reference(place, regs, vars, frame_id)?;
                let Value::Variant { alternatives, .. } = &old else {
                    return Err(RuntimeError::TypeError(format!(
                        "Variant.set applied to {}",
                        crate::runtime::type_name(&old)
                    )));
                };
                let selected = alternatives.get(*index).cloned().ok_or_else(|| {
                    RuntimeError::TypeError("Variant.set has an invalid checked tag".to_string())
                })?;
                let factory = prog.boxed(
                    regs[factory.0 as usize].clone(),
                    prog.mir.functions[function].1.reg_types.get(&factory.0),
                );
                let produced = self.invoke_callable_value(
                    prog,
                    &factory,
                    Vec::new(),
                    (frame_id, function, vars),
                )?;
                let payload = crate::runtime::coerce_checked(produced, &selected);
                let replacement = Value::Variant {
                    alternatives: alternatives.clone(),
                    index: *index,
                    value: Box::new(payload),
                };
                self.store_at_call_place(frame_id, place, replacement, regs, vars)?;
                self.drop_variant_payload(prog, old, destructors)?;
                regs[dest.0 as usize] = Value::None;
            }
            MirInstr::VariantDeinitWith {
                dest,
                variant,
                handler,
                index: expected,
            } => {
                let expected = &known_variant_index(expected)?;
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
                let handler = prog.boxed(
                    regs[handler.0 as usize].clone(),
                    prog.mir.functions[function].1.reg_types.get(&handler.0),
                );
                self.invoke_callable_value(
                    prog,
                    &handler,
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
                constructor,
                destructors,
            } => {
                let index = &known_variant_index(index)?;
                let old = self.load_place_through_reference(place, regs, vars, frame_id)?;
                let Value::Variant { alternatives, .. } = &old else {
                    return Err(RuntimeError::TypeError(format!(
                        "Variant.set applied to {}",
                        crate::runtime::type_name(&old)
                    )));
                };
                let selected = alternatives.get(*index).ok_or_else(|| {
                    RuntimeError::TypeError("Variant.set has an invalid checked tag".to_string())
                })?;
                let payload = self.move_payload(
                    prog,
                    regs[value.0 as usize].clone(),
                    selected,
                    constructor.as_deref(),
                )?;
                let replacement = Value::Variant {
                    alternatives: alternatives.clone(),
                    index: *index,
                    value: Box::new(payload),
                };
                self.store_at_call_place(frame_id, place, replacement, regs, vars)?;
                self.drop_variant_payload(prog, old, destructors)?;
                regs[dest.0 as usize] = Value::None;
            }
            MirInstr::VariantReplace {
                dest,
                place,
                input_index,
                output_index,
                value,
                checked,
                constructor,
            } => {
                let input_index = &known_variant_index(input_index)?;
                let output_index = &known_variant_index(output_index)?;
                let old = self.load_place_through_reference(place, regs, vars, frame_id)?;
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
                        .map_or_else(|| "<invalid>".to_string(), ToString::to_string);
                    return Err(RuntimeError::TypeError(format!(
                        "Variant holds '{found}', not '{output}'"
                    )));
                }
                let payload = self.move_payload(
                    prog,
                    regs[value.0 as usize].clone(),
                    &input,
                    constructor.as_deref(),
                )?;
                let replacement = Value::Variant {
                    alternatives,
                    index: *input_index,
                    value: Box::new(payload),
                };
                self.store_at_call_place(frame_id, place, replacement, regs, vars)?;
                regs[dest.0 as usize] = *old_payload;
            }
            MirInstr::MakeSimd {
                dest,
                dtype,
                width,
                elems,
            } => {
                let reg_types = &prog.mir.functions[function].1.reg_types;
                let mut vals: Vec<Value> = elems
                    .iter()
                    .map(|r| prog.boxed(regs[r.0 as usize].clone(), reg_types.get(&r.0)))
                    .collect();
                // An `Intable` element of a `SIMD[dt, _](x)` construction
                // (R1544) arrives as its concrete struct; normalize it
                // through `__int__` before lane build.
                for val in &mut vals {
                    if let Value::Struct { name, .. } = val {
                        let index = prog.struct_member(name, "__int__", 0).ok_or_else(|| {
                            RuntimeError::Unsupported(format!(
                                "vm: struct '{name}' has no method '__int__'"
                            ))
                        })?;
                        let receiver = std::mem::replace(val, Value::None);
                        *val = self.call_function(prog, index, vec![receiver])?;
                    }
                }
                let (dtype, width) = concrete_simd_slots(&(dtype.clone(), width.clone()))?;
                regs[dest.0 as usize] = simd_from_values(dtype, width, &vals)?;
            }
            MirInstr::SimdCast {
                dest,
                value,
                dtype,
                width,
            } => {
                let (dtype, _) = concrete_simd_slots(&(dtype.clone(), width.clone()))?;
                if self.ctfe_fuel.is_some() {
                    crate::runtime::ctfe_float_to_int(dtype, &regs[value.0 as usize])?;
                }
                regs[dest.0 as usize] = crate::runtime::simd_cast(dtype, &regs[value.0 as usize])?;
            }
            MirInstr::SimdBitcast {
                dest,
                value,
                dtype,
                width,
            } => {
                let (dtype, width) = concrete_simd_slots(&(dtype.clone(), width.clone()))?;
                regs[dest.0 as usize] =
                    crate::runtime::simd_bitcast(dtype, width, &regs[value.0 as usize])?;
            }
            MirInstr::SimdShuffle {
                dest,
                value,
                other,
                mask,
            } => {
                let source = &regs[value.0 as usize];
                let Some(mask) = mask.known() else {
                    return Err(parametric_instruction(&format!(
                        "the symbolic lane mask `{mask}`"
                    )));
                };
                let gathered = crate::runtime::simd_shuffle(
                    source,
                    other.map(|other| &regs[other.0 as usize]),
                    mask,
                )?;
                regs[dest.0 as usize] = gathered;
            }
            MirInstr::Select {
                dest,
                condition,
                on_true,
                on_false,
            } => {
                regs[dest.0 as usize] = crate::runtime::select(
                    &regs[condition.0 as usize],
                    &regs[on_true.0 as usize],
                    &regs[on_false.0 as usize],
                )?;
            }
            MirInstr::SimdSelect {
                dest,
                mask,
                on_true,
                on_false,
            } => {
                regs[dest.0 as usize] = crate::runtime::simd_select(
                    &regs[mask.0 as usize],
                    &regs[on_true.0 as usize],
                    &regs[on_false.0 as usize],
                )?;
            }
            MirInstr::SimdExtract { dest, value, index } => {
                let index = value_as_index(&regs[index.0 as usize])?;
                regs[dest.0 as usize] =
                    crate::runtime::read_value_lane(&regs[value.0 as usize], index)
                        .ok_or_else(|| simd_operand_error("simd.extract"))??;
            }
            MirInstr::SimdInsert {
                dest,
                value,
                element,
                index,
            } => {
                let index = value_as_index(&regs[index.0 as usize])?;
                let mut vector = regs[value.0 as usize].clone();
                crate::runtime::set_value_lane(&mut vector, index, &regs[element.0 as usize])
                    .ok_or_else(|| simd_operand_error("simd.insert"))??;
                regs[dest.0 as usize] = vector;
            }
            MirInstr::SimdReduce { dest, value, op } => {
                regs[dest.0 as usize] =
                    crate::runtime::simd_reduce_lanes(*op, &regs[value.0 as usize])?;
            }
            MirInstr::Store { place, src } => {
                let mut v = regs[src.0 as usize].clone();
                // A place projected from a pointer slot writes within its
                // pointee, as the load of one reads within it.
                if let Some((Value::Pointer { allocation, offset }, suffix)) =
                    super::references::pointer_element(&vars[place.root as usize], place, regs)?
                {
                    let (region, slot) = self.heap_index(allocation, offset, 0)?;
                    self.write_heap_projection(region, slot, &suffix, v)?;
                    return Ok(());
                }
                // A handle at the root (a `mut`/`ref self` receiver) and one
                // reached below it (`p.src[].v`, through a `ref`-typed or
                // single-pointee pointer field) both designate storage this
                // frame may not own, so the write goes through the reference
                // walk. A final dynamic index is `store_at_place`'s own — a
                // heap element, or a nominal `__setitem__` receiver — and
                // keeps that path.
                let handle = if matches!(place.steps().next_back(), Some(Proj::Index(_))) {
                    self.extend_reference(&vars[place.root as usize], place, regs)?
                } else {
                    self.place_handle(frame_id, place, regs, vars)?
                };
                let slot = super::references::pointer_element_slot(place);
                if let Some(handle) = handle {
                    if slot {
                        self.canonicalize_value_references(frame_id, vars, &mut v);
                        self.write_reference(&handle, frame_id, vars, v)?;
                    } else {
                        self.store_through_handle(frame_id, place, handle, v, vars)?;
                    }
                } else if !slot && matches!(place.ty.as_ref().map(Ty::kind), Some(TyKind::Ref(_))) {
                    let reference = load_place(vars, regs, place)?;
                    self.canonicalize_value_references(frame_id, vars, &mut v);
                    self.write_reference(&reference, frame_id, vars, v)?;
                } else {
                    self.store_at_place(frame_id, place, v, regs, vars)?;
                }
            }
            MirInstr::StoreRef { place, reference } => {
                let handle = regs[reference.0 as usize].clone();
                if !matches!(handle, Value::Ref { .. }) {
                    return Err(RuntimeError::TypeError(
                        "vm: reference storage requires a reference handle".to_string(),
                    ));
                }
                self.store_at_place(frame_id, place, handle, regs, vars)?;
            }
            MirInstr::LoadPlace { dest, place } => {
                // The read half of `c[i] += e` on a user struct goes through
                // `c.__getitem__(i)`; any other place reads its slot / SIMD lane.
                // A pointer-typed variable holding a place handle is the
                // handle itself, as `UseVar` reads it: the whole slot is
                // the pointer, not its pointee.
                let pointer_slot = place.is_whole_root()
                    && place.through.is_none()
                    && matches!(
                        (prog.mir.functions[function].1.var_tys.get(&place.root)).map(Ty::kind),
                        Some(TyKind::Pointer { .. })
                    );
                regs[dest.0 as usize] = if pointer_slot {
                    vars[place.root as usize].clone()
                } else if let Some((element, suffix)) =
                    super::references::pointer_element(&vars[place.root as usize], place, regs)?
                {
                    // A place projected from a pointer slot addresses its
                    // pointee: the element the leading index selects, then
                    // the rest of the projection within it.
                    let referent = self.read_reference(&element, frame_id, vars)?;
                    self.read_reference_projection(frame_id, vars, &referent, &suffix)?
                } else if place.through == Some(place.root)
                    && let pointer @ Value::Pointer { .. } = &vars[place.root as usize]
                {
                    // A reference binding of a static-origin call result holds
                    // the element's heap pointer (`heap_reference_result`).
                    let referent = self.read_reference(pointer, frame_id, vars)?;
                    let projection = super::references::place_projection_segments(place, regs)?;
                    self.read_reference_projection(frame_id, vars, &referent, &projection)?
                } else if let Some(reference) =
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
                    if matches!(value, Value::Ref { .. })
                        && matches!(place.ty.as_ref().map(Ty::kind), Some(TyKind::Ref(_)))
                    {
                        self.read_reference(&value, frame_id, vars)?
                    } else {
                        value
                    }
                } else if let Some(v) = self.load_index_dunder(prog, place, regs, vars, frame_id)? {
                    v
                } else {
                    // A place reaching a stored handle below its root
                    // (`self.value.items` through a `ref` field,
                    // `self.src[]` on `Pointer[List[T], o]` storage) cannot
                    // walk raw storage — route it through the reference walk,
                    // which chases stored handles mid-projection.
                    if super::references::place_crosses_reference(place) {
                        let composed = Self::reference_to_place_parts(frame_id, regs, vars, place)?;
                        let value = self.read_reference(&composed, frame_id, vars)?;
                        if matches!(value, Value::Ref { .. })
                            && matches!(place.ty.as_ref().map(Ty::kind), Some(TyKind::Ref(_)))
                        {
                            self.read_reference(&value, frame_id, vars)?
                        } else {
                            value
                        }
                    } else {
                        let value = load_place(vars, regs, place)?;
                        if matches!(place.ty.as_ref().map(Ty::kind), Some(TyKind::Ref(_))) {
                            self.read_reference(&value, frame_id, vars)?
                        } else {
                            value
                        }
                    }
                };
            }
            MirInstr::MovePlace { dest, place } => {
                // A partial move `p.a^`: transfer the field's value out, leaving a
                // `Moved` tombstone so any stray use fails loudly. The ownership
                // analysis has proven the moved field is not read again, and
                // `mir::verify` that no destroy of the whole follows unless the
                // field is stored again.
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
            // Iterator protocol over compiler-private storage: only VM-CTFE's
            // explicit ComptimeList, an owned `VariadicList`, and the
            // heterogeneous runtime-pack carrier iterate without a nominal
            // `__iter__`, through HasNext/Next below. In place when
            // `dest == source`; a borrowed source binds its slot to a
            // reference, which `dest` reads through.
            MirInstr::GetIter { source, dest } => {
                let mut current = vars[*source as usize].clone();
                if matches!(current, Value::Ref { .. }) {
                    current = self.read_reference(&current, frame_id, vars)?;
                }
                vars[*dest as usize] = current;
            }
            MirInstr::HasNext { dest, iter } => {
                // Only compiler-private runtime-pack storage (a method-free
                // tuple), an owned `VariadicList`, which `for var v in xs^`
                // consumes, and a CTFE compile-time list iterate without a
                // nominal `__next__`; public Tuple values are nominal structs
                // and cannot reach this fallback.
                if let Some((_, length)) = variadic_list_span(&vars[*iter as usize]) {
                    regs[dest.0 as usize] = Value::Bool(length > 0);
                    return Ok(());
                }
                let value = &mut vars[*iter as usize];
                let items = match value {
                    Value::ComptimeList(items) if self.ctfe_fuel.is_some() => Some(items),
                    Value::Tuple(items) => Some(items),
                    _ => None,
                };
                let Some(items) = items else {
                    return Err(RuntimeError::Unsupported(format!(
                        "vm: native iterator fallback reached {}; runtime collections must use nominal __iter__",
                        crate::runtime::type_name(&vars[*iter as usize])
                    )));
                };
                regs[dest.0 as usize] = Value::Bool(!items.is_empty());
            }
            MirInstr::Next { dest, iter } => {
                // An owned `VariadicList` hands on its first element and
                // narrows its span past it, so its destructor reaches only
                // the elements the loop has not taken.
                if let Some((elements, length)) = variadic_list_span(&vars[*iter as usize]) {
                    let element = self.read_reference(&elements, frame_id, vars)?;
                    regs[dest.0 as usize] = self.read_reference(&element, frame_id, vars)?;
                    let rest = (
                        super::references::offset_place_handle(elements, 1)?,
                        Value::Int(length - 1),
                    );
                    set_variadic_list_span(&mut vars[*iter as usize], rest);
                    return Ok(());
                }
                let value = &mut vars[*iter as usize];
                let items = match value {
                    Value::ComptimeList(items) if self.ctfe_fuel.is_some() => Some(items),
                    Value::Tuple(items) => Some(items),
                    _ => None,
                };
                let Some(items) = items else {
                    return Err(RuntimeError::Unsupported(format!(
                        "vm: native iterator fallback reached {}; runtime collections must use nominal __next__",
                        crate::runtime::type_name(&vars[*iter as usize])
                    )));
                };
                regs[dest.0 as usize] = items.remove(0);
            }
            MirInstr::DeclareVar { .. } => {}
            // ASAP destruction: drop the value at the variable's last use,
            // running the destructor the drop names.
            MirInstr::DropVar { var, destructor } => {
                let v = std::mem::replace(&mut vars[*var as usize], Value::None);
                self.drop_named(
                    prog,
                    v,
                    prog.mir.functions[function].1.var_tys.get(var),
                    destructor.as_deref(),
                )?;
            }
            // The end of a value whose parts drop elaboration has already
            // destroyed, upstream's exit `mark_destroyed`: the slot holds
            // nothing left to destroy.
            MirInstr::ConsumeVar { var } => {
                let value = std::mem::replace(&mut vars[*var as usize], Value::Moved);
                let value = match prog.mir.functions[function].1.var_tys.get(var) {
                    Some(ty) => {
                        dispatch::erased_instance_value(prog, prog.boxed(value, Some(ty)), ty)
                    }
                    None => value,
                };
                // A synthesized destructor's consume of its receiver is no
                // event of the source program.
                if let Value::Struct { name, .. } = &value
                    && !prog.synthesized(&prog.mir.functions[function].0)
                {
                    self.record_lifecycle(format!("consume {name}"));
                }
            }
            MirInstr::DropPlace { place, destructor } => {
                let value = self.take_place(frame_id, place, regs, vars)?;
                self.drop_named(prog, value, place.ty.as_ref(), destructor.as_deref())?;
            }
            // An owned register's death, its value's fields dropped by
            // their types when it names no destructor.
            MirInstr::Drop { reg, destructor } => {
                let value = std::mem::replace(&mut regs[reg.0 as usize], Value::Moved);
                self.drop_named(
                    prog,
                    value,
                    prog.mir.functions[function].1.reg_types.get(&reg.0),
                    destructor.as_deref(),
                )?;
            }
            MirInstr::Unsupported(what) => {
                return Err(RuntimeError::Unsupported(format!(
                    "vm backend does not support {what} yet"
                )));
            }
        }
        Ok(())
    }

    /// The error a raise naming a `try` stores into its slot from register
    /// `src` of `function`: the error [`Self::raise_register`] makes, which
    /// the `try` catches.
    fn stage_error(
        &mut self,
        prog: &Prog,
        function: usize,
        registers: &[Value],
        src: Reg,
    ) -> Result<Value, RuntimeError> {
        let error = match self.raise_register(prog, function, registers, src) {
            RuntimeError::Raised(error) => error,
            raised => return Err(raised),
        };
        if let Value::Error(message) = &error {
            self.record_lifecycle(format!("catch {message}"));
        }
        Ok(error)
    }

    /// The error a `raise` of register `src` of `function` propagates as:
    /// a raise edge's error as it was raised, any other value as
    /// [`Self::raise`] makes it.
    pub(super) fn raise_register(
        &mut self,
        prog: &Prog,
        function: usize,
        registers: &[Value],
        src: Reg,
    ) -> RuntimeError {
        let value = &registers[src.0 as usize];
        if prog.edge_errors[function].contains(&src.0) {
            RuntimeError::Raised(value.clone())
        } else {
            self.raise(value)
        }
    }

    /// The error a `raise` of `value` propagates as: `Raised`, which the
    /// nearest enclosing `try` intercepts, or which otherwise unwinds the
    /// frame.
    pub(super) fn raise(&mut self, value: &Value) -> RuntimeError {
        let error = match value {
            Value::Str(message) => Value::Error(message.clone()),
            // A raised nominal String reads its buffer back through the
            // struct-to-literal bridge.
            Value::Struct { name, .. } if mojito_symbol::symbol::is_stdlib_string_struct(name) => {
                match self.string_struct_literal(value) {
                    Ok(Value::Str(message)) => Value::Error(message),
                    Ok(other) => other,
                    Err(error) => return error,
                }
            }
            other => other.clone(),
        };
        if let Value::Error(message) = &error {
            self.record_lifecycle(format!("raise {message}"));
        }
        RuntimeError::Raised(error)
    }
}

impl VmBackend {
    /// Read the value at `place` the way `LoadPlace` does: a reference-alias
    /// root (a `mut`/`ref self` receiver, as in the self-hosted `Variant`'s
    /// `self._storage`) reaches its referent through the handle; a plain
    /// root reads raw storage.
    #[allow(
        clippy::needless_pass_by_ref_mut,
        reason = "reborrowed as `&mut` by `read_reference`"
    )]
    fn load_place_through_reference(
        &mut self,
        place: &MirPlace,
        regs: &[Value],
        vars: &mut [Value],
        frame_id: FrameId,
    ) -> Result<Value, RuntimeError> {
        if let Some(reference) = self.extend_reference(&vars[place.root as usize], place, regs)? {
            return self.read_reference(&reference, frame_id, vars);
        }
        load_place(vars, regs, place)
    }
}

/// Upstream's stack allocation: a slot's storage exists from its allocation.
/// The value tree builds a struct or tuple slot's unwritten skeleton
/// ([`VmBackend::uninitialized_value`]) where an instruction first uses the
/// slot as storage: stores into it, reads or moves out of it, addresses it,
/// or passes it by address. A drop, consumption, or mark is no such use,
/// so it builds no storage. A place reached through a reference addresses
/// storage some other frame owns.
/// The error of a lane instruction whose vector operand is no SIMD value.
fn simd_operand_error(mnemonic: &str) -> RuntimeError {
    RuntimeError::TypeError(format!("vm: {mnemonic} of a non-SIMD value"))
}

fn give_storage(prog: &Prog, function: usize, instruction: &MirInstr, vars: &mut [Value]) {
    let mut give = |root: VarId| {
        let slot = &mut vars[root as usize];
        if matches!(slot, Value::None | Value::Moved)
            && let Some(skeleton) = (prog.mir.functions[function].1.var_tys.get(&root))
                .and_then(|ty| VmBackend::uninitialized_value(prog, ty))
        {
            *slot = skeleton;
        }
    };
    match instruction {
        MirInstr::UseVar { var, .. } => give(*var),
        MirInstr::DropPlace { .. }
        | MirInstr::MarkDestroyed { .. }
        | MirInstr::MarkInitialized { .. }
        | MirInstr::EstablishLoans { .. } => {}
        _ => instruction_places(instruction)
            .into_iter()
            .filter(|place| place.through.is_none())
            .for_each(|place| give(place.root)),
    }
}

/// The known lane dtype and width of a SIMD instruction. Concrete MIR holds
/// known slots; a symbolic one is a verifier finding before the VM runs, so
/// meeting it here is the invariant boundary.
fn concrete_simd_slots(
    (dtype, width): &(
        mojito_types::types::SimdDtype,
        mojito_types::types::SimdWidth,
    ),
) -> Result<(mojito_ast::ast::Dtype, usize), RuntimeError> {
    match (
        dtype.known(),
        width.known().and_then(|width| usize::try_from(width).ok()),
    ) {
        (Some(dtype), Some(width)) => Ok((dtype, width)),
        _ => Err(RuntimeError::Unsupported(format!(
            "a SIMD instruction over the symbolic slots `SIMD[{dtype}, {width}]` reached the VM"
        ))),
    }
}

/// The span of the `VariadicList` an owned loop over a `var` collector
/// consumes, which the builtin `HasNext`/`Next` step: the pointer to its
/// first element pointer and its length.
fn variadic_list_span(value: &Value) -> Option<(Value, i64)> {
    match variadic_list_span_fields(value)? {
        [(_, elements), (_, Value::Int(length))] => Some((elements.clone(), *length)),
        _ => None,
    }
}

/// Narrow the span of the `VariadicList` `value` to `(elements, length)`.
fn set_variadic_list_span(value: &mut Value, (elements, length): (Value, Value)) {
    if let Value::Struct { fields, .. } = value
        && let [(_, Value::Struct { fields: span, .. })] = fields.as_mut_slice()
        && let [(_, data), (_, size)] = span.as_mut_slice()
    {
        *data = elements;
        *size = length;
    }
}

/// The fields of a `VariadicList`'s span.
fn variadic_list_span_fields(value: &Value) -> Option<&[(String, Value)]> {
    match value {
        Value::Struct { name, fields }
            if mojito_types::types::struct_template(name)
                == mojito_types::types::VARIADIC_LIST_TYPE_NAME =>
        {
            match fields.as_slice() {
                [(_, Value::Struct { fields: span, .. })] => Some(span),
                _ => None,
            }
        }
        _ => None,
    }
}

/// The element a closed `array.get` or `array.replace` index names.
fn array_index(index: &Const) -> Option<usize> {
    index
        .index_attribute()
        .and_then(|index| usize::try_from(index).ok())
}
