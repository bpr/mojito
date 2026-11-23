//! The `exec_instr` instruction dispatcher and the error a `raise`
//! propagates as.
//! Extracted from `backend/vm.rs`; see `docs/symbol-map.md`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_mir::mir::make_ref_forwards_stored_handle;
use mojito_mir::mir::verify::instruction_places;
use mojito_types::types::TyKind;
use mojito_types::types::storage_simd;

impl VmBackend {
    /// The value a `place.load` of `place` reads in the frame `frame_id` of
    /// `function`.
    pub(super) fn load_place_value(
        &mut self,
        prog: &Prog,
        function: usize,
        frame_id: FrameId,
        regs: &[Value],
        vars: &mut [Value],
        place: &MirPlace,
    ) -> Result<Value, RuntimeError> {
        // The read half of `c[i] += e` on a user struct goes through
        // `c.__getitem__(i)`; any other place reads its slot / SIMD lane.
        let holds_reference = prog.holds_reference(function, place);
        let value = if let Some((element, suffix)) =
            super::references::pointer_element(&vars[place.root as usize], place, regs)?
        {
            // A place projected from a pointer slot addresses its
            // pointee: the element the leading index selects, then
            // the rest of the projection within it.
            let referent = self.read_reference(&element, frame_id, vars)?;
            self.read_reference_projection(frame_id, vars, &referent, &suffix)?
        } else if let Some(handle) = self.heap_handle_place(
            (&prog.mir.functions[function].1, &prog.mir.declarations),
            place,
            regs,
            vars,
        )? {
            // A reference binding of a static-origin call result holds
            // the element's heap pointer (`heap_reference_result`).
            self.read_reference(&handle, frame_id, vars)?
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
            if matches!(value, Value::Ref { .. }) && holds_reference {
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
            if prog.crosses_reference(place) {
                let composed = Self::reference_to_place_parts(frame_id, regs, vars, place)?;
                let value = self.read_reference(&composed, frame_id, vars)?;
                if matches!(value, Value::Ref { .. }) && holds_reference {
                    self.read_reference(&value, frame_id, vars)?
                } else {
                    value
                }
            } else {
                let value = load_place(vars, regs, place)?;
                if holds_reference {
                    self.read_reference(&value, frame_id, vars)?
                } else {
                    value
                }
            }
        };
        Ok(value)
    }

    /// The value a call passes for its operand `register`, as upstream's
    /// `lit.call` passes the operand itself: a by-address operand (one the
    /// call passes as a reference to its parameter's value,
    /// [`Prog::addressed`]) is the reference the call formed, which the
    /// callee binds as its parameter.
    /// A heap element `ref.make` forwards as its pointer passes as the
    /// interned pointer's element handle, which a callee writes through as
    /// it writes any reference.
    pub(super) fn call_operand(
        &mut self,
        prog: &Prog,
        function: usize,
        (_, regs, _): (FrameId, &[Value], &[Value]),
        register: Reg,
    ) -> Value {
        let value = &regs[register.0 as usize];
        match value {
            Value::Pointer { allocation, offset } if prog.addressed(function, register) => {
                Value::Ref {
                    frame: POINTER_ROOT_FRAME,
                    slot: self.pointer_root(*allocation, *offset),
                    projection: vec![RefProjection::Index(0)],
                }
            }
            value => value.clone(),
        }
    }

    /// A method call's receiver operand `register`: the value its dispatch
    /// reads, with that value's type, and the reference a by-address
    /// receiver binds as its `self` (`None` for a receiver passed by
    /// value): a `ref.make`, or a borrow of a slot that holds a reference.
    pub(super) fn receiver_operand<'p>(
        &mut self,
        prog: &'p Prog,
        function: usize,
        (frame_id, regs, vars): (FrameId, &[Value], &[Value]),
        register: Reg,
    ) -> Result<(Value, Option<&'p Ty>, Option<Value>), RuntimeError> {
        let ty = prog.mir.functions[function].1.reg_types.get(&register.0);
        let value = regs[register.0 as usize].clone();
        let addressed = ty
            .and_then(mojito_mir::mir::addressed_receiver)
            .filter(|_| prog.addressed(function, register));
        Ok(match addressed {
            Some(reference) if matches!(value, Value::Ref { .. } | Value::Pointer { .. }) => {
                let referent = self.read_reference(&value, frame_id, vars)?;
                let reference_operand =
                    self.call_operand(prog, function, (frame_id, regs, vars), register);
                (referent, Some(&reference.referent), Some(reference_operand))
            }
            // A borrow of a slot holding a reference (a by-address
            // parameter or a `ref` binding, read at its referent's type)
            // is that reference, as upstream's `!lit.ref` value is.
            _ if matches!(value, Value::Ref { .. }) => {
                let referent = self.read_reference(&value, frame_id, vars)?;
                (referent, ty, Some(value))
            }
            _ => (value, ty, None),
        })
    }

    /// The value a builtin takes for the call operand `register`: a
    /// by-address operand read through its reference, as upstream loads an
    /// argument a callee takes by value, and a held struct as its box, but
    /// for a `Pointer`, whose builtins read the address its held
    /// `!kgen.pointer` field is.
    pub(super) fn value_operand(
        &self,
        prog: &Prog,
        function: usize,
        (frame_id, regs, vars): (FrameId, &[Value], &[Value]),
        register: Reg,
    ) -> Result<Value, RuntimeError> {
        let operand = |value: Value, ty: Option<&Ty>| {
            if ty.is_some_and(|ty| ty.kind().is_pointer_layout()) {
                prog.unboxed(value, ty)
            } else {
                prog.boxed(value, ty)
            }
        };
        let ty = prog.mir.functions[function].1.reg_types.get(&register.0);
        let value = regs[register.0 as usize].clone();
        let addressed = ty
            .and_then(mojito_mir::mir::addressed_receiver)
            .filter(|_| prog.addressed(function, register));
        Ok(match addressed {
            Some(reference) if matches!(value, Value::Ref { .. } | Value::Pointer { .. }) => {
                let referent = self.read_reference(&value, frame_id, vars)?;
                operand(referent, Some(&reference.referent))
            }
            _ => operand(value, ty),
        })
    }

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
                if let Some(handle) = self.heap_handle_place(
                    (&prog.mir.functions[function].1, &prog.mir.declarations),
                    place,
                    regs,
                    vars,
                )? {
                    // A `ref` field (a closure's `mut` capture) forwards the
                    // handle it stores when the destination is typed as
                    // that handle, as below.
                    let target = prog.mir.functions[function].1.reg_types.get(&dest.0);
                    regs[dest.0 as usize] = if target.is_some_and(|made| {
                        make_ref_forwards_stored_handle(
                            place,
                            made,
                            prog.holds_reference(function, place),
                        )
                    }) {
                        match self.read_reference(&handle, frame_id, vars)? {
                            stored @ (Value::Ref { .. } | Value::Pointer { .. }) => stored,
                            _ => handle,
                        }
                    } else {
                        handle
                    };
                    return Ok(());
                }
                let root = vars[place.root as usize].clone();
                // A place within a tracked pointer variable's pointee, the
                // variable holding a heap pointer (a constructor's
                // `Pointer[T, Self.origin]` parameter bound to
                // `xs.unsafe_ptr()`), is addressed by that pointer: forward
                // it, applying an element projection as pointer arithmetic
                // (a single-pointee deref designates the pointee the pointer
                // already addresses). A `ref`-typed root holds its handle
                // whatever the referent's representation, so only a root
                // that is itself a held struct is a value here. The whole
                // of a pointer slot, as for a place pointer, is the slot.
                if let Value::Pointer { allocation, offset } = root
                    && prog.held_field(place.root_ty.as_ref()).is_none()
                    && !prog.pointer_slot(function, place)
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
                            address_as(
                                &prog.structs,
                                Value::Ref {
                                    frame: POINTER_ROOT_FRAME,
                                    slot: self.pointer_root(element_allocation, element_offset),
                                    projection: std::iter::once(RefProjection::Index(0))
                                        .chain(suffix)
                                        .collect(),
                                },
                                prog.mir.functions[function].1.reg_types.get(&dest.0),
                            )
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
                // A place dereferencing the place pointer its root holds
                // (`p[]`, `elements[unsafe_offset=i]`) is the pointee's
                // storage; a reference root (a `mut`/`ref` binding) extends
                // to its referent.
                let (frame, slot, mut projection) = match (root, segments.first()) {
                    (
                        Value::PlacePointer {
                            frame,
                            slot,
                            projection,
                        },
                        Some(first),
                    ) if let Some(target) = super::references::place_pointer_target(
                        frame,
                        slot,
                        &projection,
                        first,
                    ) =>
                    {
                        segments.remove(0);
                        target?
                    }
                    (
                        Value::Ref {
                            frame,
                            slot,
                            projection,
                        },
                        _,
                    ) => (frame, slot, projection),
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
                    && target.is_some_and(|made| {
                        make_ref_forwards_stored_handle(
                            place,
                            made,
                            prog.holds_reference(function, place),
                        )
                    })
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
                let result = self.heap_reference_result(handle, target, frame_id, vars);
                regs[dest.0 as usize] = address_as(&prog.structs, result, target);
            }
            MirInstr::LoadIndirect { dest, pack } => {
                let Value::Tuple(references) = regs[pack.0 as usize].clone() else {
                    return Err(RuntimeError::TypeError(
                        "vm: kgen.struct.load_indirect over no struct of pointers".to_string(),
                    ));
                };
                let loaded = references
                    .iter()
                    .map(|handle| self.read_reference(handle, frame_id, vars))
                    .collect::<Result<Vec<_>, _>>()?;
                regs[dest.0 as usize] = Value::Tuple(loaded);
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
                ..
            } => {
                let environment = captures
                    .iter()
                    .map(|capture| {
                        Self::reference_to_place_parts(frame_id, regs, vars, &capture.place)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
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
                    // A string literal is the empty struct whose text its
                    // type carries.
                    Const::Str(_) => literal_register_value(prog, function, *dest)
                        .unwrap_or_else(|| const_value(k))?,
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
                let Some(element) = (prog.mir.functions[function].1.reg_types.get(&dest.0))
                    .and_then(Ty::pointer_element)
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
                let reference = self.store_to_comptime_memory(value);
                regs[dest.0 as usize] = address_as(
                    &prog.structs,
                    reference,
                    prog.mir.functions[function].1.reg_types.get(&dest.0),
                );
            }
            MirInstr::ConstructTypeParam { param, .. } => {
                return Err(parametric_instruction(&format!(
                    "a construction of the type parameter `{}`",
                    param.name
                )));
            }
            MirInstr::ConstructType { ty, .. } => {
                return Err(parametric_instruction(&format!(
                    "a construction of the type `{ty}`"
                )));
            }
            MirInstr::MaterializeLiteral {
                dest,
                value,
                target,
            } => {
                let vector = storage_simd(target).ok_or_else(|| {
                    RuntimeError::TypeError(format!(
                        "literal.materialize targets {target}, not a `!kgen.simd` storage"
                    ))
                })?;
                let literal = register_literal(prog, function, *value)?;
                regs[dest.0 as usize] = crate::runtime::materialize_literal(&literal, &vector)?;
            }
            // A literal operand's conversion reads the number its type
            // carries, as upstream's literal `__int__`/`__bool__` do.
            MirInstr::ScalarCast {
                dest,
                value,
                target,
            } if is_literal_register(prog, function, *value) => {
                let literal = register_literal(prog, function, *value)?;
                regs[dest.0 as usize] = match target {
                    ScalarConversion::Bool => Value::Bool(literal.is_nonzero()),
                    ScalarConversion::Int => {
                        crate::runtime::materialize_literal(&literal.truncated(), &Ty::int())?
                    }
                    ScalarConversion::UInt => crate::runtime::materialize_literal(
                        &literal.truncated(),
                        &mojito_types::types::canonical_simd_ty(mojito_ast::ast::Dtype::UInt, 1),
                    )?,
                    ScalarConversion::Float64 => {
                        crate::runtime::materialize_literal(&literal, &Ty::float64())?
                    }
                };
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
                regs[dest.0 as usize] = address_as(
                    &prog.structs,
                    regs[value.0 as usize].clone(),
                    prog.mir.functions[function].1.reg_types.get(&dest.0),
                );
            }
            // The heap and frames hold one typed value per element, so an
            // address retyped to a pointee held differently cannot be
            // reinterpreted through: the VM refuses it here, at the bitcast.
            MirInstr::PointerBitcast { dest, value } => {
                let reg_types = &prog.mir.functions[function].1.reg_types;
                crate::runtime::pointer_bitcast_preserves_pointee(
                    reg_types.get(&value.0),
                    reg_types.get(&dest.0),
                )?;
                regs[dest.0 as usize] = address_as(
                    &prog.structs,
                    regs[value.0 as usize].clone(),
                    reg_types.get(&dest.0),
                );
            }
            // An `index` and its `Int` storage are one value, as an `i1` and
            // a `!kgen.scalar<bool>` are, as LLVM's one `i1`, and an `ui8`
            // and a `!kgen.scalar<ui8>`.
            MirInstr::BuiltinCast { dest, value } => {
                regs[dest.0 as usize] = regs[value.0 as usize].clone();
            }
            MirInstr::IsRunInComptimeInterpreter { dest } => {
                regs[dest.0 as usize] = Value::Bool(self.ctfe_fuel.is_some());
            }
            // Only a compile-time application still holds one: the driver
            // resolved every run-time op to constants.
            MirInstr::SourceLoc { dest, .. } => {
                regs[dest.0 as usize] = self.thaw(
                    prog,
                    &mojito_mir::mir::unknown_source_location(),
                    prog.mir.functions[function].1.reg_types.get(&dest.0),
                )?;
            }
            MirInstr::Pop {
                dest,
                op,
                operands,
                fastmath: _,
            } if op.is_string_read() => {
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
            // A `!kgen.dtype` is its `Dtype`, and an `ui8` its `UInt8`
            // lane, so the conversions are the folds over the values.
            MirInstr::Pop {
                dest,
                op,
                operands,
                fastmath: _,
            } if op.is_dtype_code() => {
                let operand = operands
                    .first()
                    .map(|operand| regs[operand.0 as usize].clone())
                    .ok_or_else(|| {
                        RuntimeError::TypeError(format!("'pop.{}' takes 1 operand", op.mnemonic()))
                    })?;
                let operand = crate::crossing::vm_to_ct(operand)?;
                let folded = if *op == mojito_types::mlir::PopOp::DtypeToUi8 {
                    mojito_types::param_expr::fold::fold_dtype_to_ui8(&operand)
                } else {
                    mojito_types::param_expr::fold::fold_dtype_from_ui8(&operand)
                }
                .map_err(|error| RuntimeError::Unsupported(error.to_string()))?;
                regs[dest.0 as usize] = crate::crossing::ct_to_vm(&folded)?;
            }
            // A heap pointer's address is its allocation's; a place pointer
            // or reference takes its place's synthetic one, as
            // `Pointer.__int__` does. Either counts its element offset in
            // bytes of the operand's pointee.
            MirInstr::Pop {
                dest,
                op: mojito_types::mlir::PopOp::PointerToIndex,
                operands,
                fastmath: _,
            } => {
                let operand = operands.first();
                let stride = |vm: &Self| {
                    operand
                        .and_then(|operand| {
                            prog.mir.functions[function].1.reg_types.get(&operand.0)
                        })
                        .and_then(crate::runtime::address_pointee)
                        .ok_or_else(|| {
                            RuntimeError::TypeError(
                                "'pop.pointer_to_index' reads an operand of no pointer type"
                                    .to_string(),
                            )
                        })
                        .and_then(|pointee| vm.element_stride(prog, pointee))
                };
                let address = match operand.map(|operand| &regs[operand.0 as usize]) {
                    Some(Value::Pointer { allocation, offset }) => {
                        crate::runtime::pointer_address(*allocation, *offset, stride(self)?)
                    }
                    Some(
                        Value::PlacePointer {
                            frame,
                            slot,
                            projection,
                        }
                        | Value::Ref {
                            frame,
                            slot,
                            projection,
                        },
                    ) => {
                        let (frame, slot, projection) = (*frame, *slot, projection.clone());
                        let stride = stride(self)?;
                        self.place_address(frame, slot, &projection, stride)
                    }
                    other => {
                        return Err(RuntimeError::TypeError(format!(
                            "'pop.pointer_to_index' reads an address, not {}",
                            other.map_or_else(|| "nothing".to_string(), crate::runtime::type_name)
                        )));
                    }
                };
                regs[dest.0 as usize] = Value::Int(address);
            }
            MirInstr::Pop {
                dest,
                op,
                operands,
                fastmath: _,
            } => {
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
                    .map(|ty| prog.mir.declarations.translated_intrinsic_type(ty));
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
                    && prog.mir.functions[function].1.reads_through(*var)
                {
                    // A reference binding, or a borrowed parameter bound to
                    // its argument's address, reads through its stored
                    // handle; any other slot's handle is its value.
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
                if matches!(value, Value::Undef) && !mode.moves() {
                    return Err(undef_read());
                }
                regs[dest.0 as usize] = value;
            }
            MirInstr::DefVar { var, src, .. } if prog.error_slots[function].contains(var) => {
                vars[*var as usize] = self.stage_error(regs, *src)?;
            }
            MirInstr::DefVar { var, src, .. }
                if prog.mir.functions[function].1.error_slot == Some(*var) =>
            {
                vars[*var as usize] = match self.raise(&regs[src.0 as usize]) {
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
                    // ordinary bindings, a place pointer's included, retain
                    // their existing runtime shape.
                    let writes_through = matches!(&vars[slot], Value::Ref { .. });
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
            // An operator over numeric literals yields the literal its
            // result type carries, or the `Bool` its operands' types decide,
            // as upstream's literal dunders read their `value` parameters.
            MirInstr::UnOp { op, dest, a, .. } if is_literal_register(prog, function, *a) => {
                regs[dest.0 as usize] = match literal_register_value(prog, function, *dest) {
                    Some(result) => result?,
                    None if *op == mojito_ast::ast::PrefixOp::Not => {
                        Value::Bool(!register_literal(prog, function, *a)?.is_nonzero())
                    }
                    None => {
                        return Err(RuntimeError::TypeError(format!(
                            "a numeric literal's '{op:?}' yields no literal"
                        )));
                    }
                };
            }
            MirInstr::UnOp { op, dest, a, .. } => {
                let operand = operand_value(prog, function, regs, *a);
                regs[dest.0 as usize] = self.apply_prefix(prog, *op, operand)?;
            }
            MirInstr::BinOp { op, dest, a, b, .. }
                if is_literal_register(prog, function, *a)
                    && is_literal_register(prog, function, *b) =>
            {
                regs[dest.0 as usize] =
                    if let Some(result) = literal_register_value(prog, function, *dest) {
                        result?
                    } else {
                        let left = register_literal(prog, function, *a)?;
                        let right = register_literal(prog, function, *b)?;
                        Value::Bool(left.compare_by(*op, &right).ok_or_else(|| {
                            RuntimeError::TypeError(format!(
                                "a numeric literal's '{op:?}' yields no literal"
                            ))
                        })?)
                    };
            }
            MirInstr::BinOp {
                op,
                dest,
                a,
                b,
                resolved,
                ..
            } => {
                let l = operand_value(prog, function, regs, *a);
                let r = operand_value(prog, function, regs, *b);
                let value = self.apply_binop(prog, *op, l, r, resolved.as_deref())?;
                // A generic instance types a comparison over its parameter
                // `Bool`, as its bound declares; sized scalars yield a mask.
                let declared_bool = matches!(value, Value::Simd { .. })
                    && prog.mir.functions[function].1.reg_types.get(&dest.0) == Some(&Ty::bool());
                regs[dest.0 as usize] =
                    if let Some(literal) = literal_register_value(prog, function, *dest) {
                        literal?
                    } else if declared_bool {
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
                instantiated_args,
                ..
            } => {
                let reg_types = &prog.mir.functions[function].1.reg_types;
                let constructed = prog.declared_struct(reg_types.get(&dest.0));
                // A MIR function enters a frame binding its operands as the
                // call passes them, as the continuation path's does; this
                // instruction interpreter runs it synchronously (inside a
                // structured `try` region). A lifecycle constructor, an
                // initializer of a receiver the VM holds as a value, and a
                // builtin go through `call_named`.
                let frame_callee = prog
                    .lifecycle_constructor(&func.0, constructed)
                    .is_none()
                    .then(|| prog.index_of(&func.0))
                    .flatten()
                    .filter(|&index| {
                        !matches!(
                            prog.constructed(&func.0, &prog.mir.functions[index].1.param_types),
                            Some(Constructed::Value(_))
                        )
                    });
                let result = if let Some(index) = frame_callee {
                    let mut operand = |register: &Reg| {
                        self.call_operand(prog, function, (frame_id, regs, vars), *register)
                    };
                    let argv: Vec<Value> = args.iter().map(&mut operand).collect();
                    let kw: Vec<(String, Value)> = kwargs
                        .iter()
                        .map(|(name, register)| (name.clone(), operand(register)))
                        .collect();
                    let bound = match prog.sigs.get(&func.0) {
                        Some(signature) => {
                            self.bind_for_call(prog, &func.0, signature, &argv, &kw)?.0
                        }
                        None => argv,
                    };
                    check_reference_arguments(&prog.mir.functions[index].1, &func.0, &bound)?;
                    self.call_synchronously(
                        prog,
                        index,
                        bound,
                        CallerFrame {
                            id: frame_id,
                            function: caller_function,
                            registers: regs,
                            variables: vars,
                        },
                    )?
                    .0
                } else {
                    // A builtin (`abs`, `input`) takes values, dispatched by
                    // runtime shape: a by-address operand is read through,
                    // and a held struct operand passes as its box. A
                    // constructor binds its operands as a function's frame
                    // does, unboxing a held struct at entry.
                    let builtin = prog.index_of(&func.0).is_none();
                    let mut operand = |register: &Reg| {
                        if builtin {
                            self.value_operand(prog, function, (frame_id, regs, vars), *register)
                        } else {
                            Ok(prog.boxed(
                                self.call_operand(
                                    prog,
                                    function,
                                    (frame_id, regs, vars),
                                    *register,
                                ),
                                reg_types.get(&register.0),
                            ))
                        }
                    };
                    let argv = args
                        .iter()
                        .map(&mut operand)
                        .collect::<Result<Vec<_>, _>>()?;
                    let kw = kwargs
                        .iter()
                        .map(|(name, register)| Ok((name.clone(), operand(register)?)))
                        .collect::<Result<Vec<_>, RuntimeError>>()?;
                    let stack_base = self.push_caller_mirror(frame_id, regs, vars);
                    let arg_types: Vec<Option<mojito_types::types::Ty>> = args
                        .iter()
                        .map(|reg| reg_types.get(&reg.0).cloned())
                        .collect();
                    let outcome = self.call_named(
                        prog,
                        &func.0,
                        argv,
                        &kw,
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
                ..
            } => {
                let callable = regs[callee.0 as usize].clone();
                let (function, captures) = match &callable {
                    Value::Function(function) => (function.clone(), Vec::new()),
                    Value::Closure {
                        function, captures, ..
                    } => (function.clone(), captures.clone()),
                    value => {
                        return Err(RuntimeError::NotCallable(crate::runtime::type_name(value)));
                    }
                };
                let index = prog
                    .index_of(&function)
                    .ok_or_else(|| RuntimeError::NotCallable(function.clone()))?;
                // An initializer's `out self` enters as its receiver storage,
                // bound as a closure's is, and leaves as the call's result.
                let receiver = Self::initializer_receiver(prog, index);
                let initializes = receiver.is_some();
                let mut positional = receiver.into_iter().collect::<Vec<_>>();
                positional.extend(captures);
                let mut operand = |register: &Reg| {
                    self.call_operand(prog, caller_function, (frame_id, regs, vars), *register)
                };
                positional.extend(args.iter().map(&mut operand));
                let keywords: Vec<(String, Value)> = kwargs
                    .iter()
                    .map(|(name, register)| (name.clone(), operand(register)))
                    .collect();
                let bound = match prog.sigs.get(&function) {
                    Some(signature) => {
                        self.bind_for_closure_call(
                            prog,
                            &function,
                            signature,
                            &positional,
                            &keywords,
                        )?
                        .0
                    }
                    None => positional,
                };
                check_reference_arguments(&prog.mir.functions[index].1, &function, &bound)?;
                let (result, frame_vars, _) = self.call_synchronously(
                    prog,
                    index,
                    bound,
                    CallerFrame {
                        id: frame_id,
                        function: caller_function,
                        registers: regs,
                        variables: vars,
                    },
                )?;
                regs[dest.0 as usize] = if initializes {
                    frame_vars.into_iter().next().unwrap_or(Value::None)
                } else {
                    result
                };
            }
            MirInstr::MethodCall {
                dest,
                recv,
                method,
                resolved,
                result_adapter,
                args,
                kwargs,
                ..
            } => {
                let reg_types = &prog.mir.functions[function].1.reg_types;
                let (recv_val, receiver_ty, receiver_operand) =
                    self.receiver_operand(prog, function, (frame_id, regs, vars), *recv)?;
                // A place pointer is a reference handle here, and its
                // trait-dispatched `copy` is the handle, not its pointee's.
                let pointer_copy = method == "copy"
                    && resolved.is_none()
                    && args.is_empty()
                    && kwargs.is_empty()
                    && reg_types.get(&recv.0).is_some_and(Ty::is_pointer);
                if pointer_copy {
                    regs[dest.0 as usize] = recv_val;
                } else {
                    // A builtin method formats its operands by runtime
                    // shape, so a held struct operand passes as its box.
                    let mut operand = |register: &Reg| {
                        prog.boxed(
                            self.call_operand(prog, function, (frame_id, regs, vars), *register),
                            reg_types.get(&register.0),
                        )
                    };
                    let argv: Vec<Value> = args.iter().map(&mut operand).collect();
                    let kw: Vec<(String, Value)> = kwargs
                        .iter()
                        .map(|(name, reg)| (name.clone(), operand(reg)))
                        .collect();
                    let result = self.method_call(
                        prog,
                        MethodInvocation {
                            receiver: recv_val,
                            receiver_ty,
                            method,
                            resolved_name: resolved.as_deref(),
                            result_adapter: result_adapter.as_deref().map(|adapter| {
                                ResultAdapter {
                                    kind: adapter.kind,
                                    constructor: adapter.constructor.as_deref(),
                                    ty: reg_types.get(&dest.0),
                                }
                            }),
                            arguments: argv,
                            keyword_arguments: kw,
                            receiver_operand,
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
                    // A place pointer (`Pointer(to=x)`) or a reference
                    // designates one value whose element the checker proved
                    // trivially destructible: taking it reads the pointee.
                    handle @ (Value::PlacePointer { .. } | Value::Ref { .. }) if index == 0 => {
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
                    // A place pointer (`Pointer(to=x)`) or a reference
                    // designates one value: destroying it runs the pointee's
                    // destructor, which only a bundled module that then ends
                    // its storage (`lit.ownership.mark_destroyed`) names for
                    // a value that is not trivially destructible.
                    ref handle @ (Value::PlacePointer { .. } | Value::Ref { .. }) if index == 0 => {
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
                if matches!(base, Value::Undef) {
                    return Err(undef_read());
                }
                // A held struct's field is the struct's value.
                if prog.field_is_whole(base_ty) {
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
                regs[dest.0 as usize] = if prog.field_is_whole(referent_ty) {
                    prog.unboxed(base, referent_ty)
                } else {
                    get_field(&base, field)?
                };
            }
            MirInstr::Index {
                dest,
                base,
                index,
                call,
                intrinsic,
                ..
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
                    let base_ty = prog.mir.functions[function].1.reg_types.get(&base.0);
                    // A `SIMD` or `Pointer` receiver is the builtin value
                    // `method_call` dispatches to the selected declared
                    // member.
                    let recv = match prog.boxed(base_value, base_ty.map(peel_references)) {
                        recv @ Value::Struct { .. } => recv,
                        recv if base_ty.is_some_and(|ty| {
                            let kind = peel_references(ty).kind();
                            kind.is_simd() || kind.is_pointer_layout()
                        }) =>
                        {
                            recv
                        }
                        other => {
                            return Err(RuntimeError::TypeError(format!(
                                "vm: checked nominal subscript receiver is {}",
                                crate::runtime::type_name(&other)
                            )));
                        }
                    };
                    let idx = self.call_operand(prog, function, (frame_id, regs, vars), *index);
                    let arguments = if call.arguments.is_empty() {
                        Vec::new()
                    } else {
                        vec![idx]
                    };
                    let receiver_operand = match &regs[base.0 as usize] {
                        _ if prog.addressed(function, *base) => {
                            Some(self.call_operand(prog, function, (frame_id, regs, vars), *base))
                        }
                        reference @ Value::Ref { .. } => Some(reference.clone()),
                        _ => None,
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
                            receiver_operand,
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
                kwargs,
                call,
                ..
            } => {
                let mut subscript_value = |argument: &MirSubscriptArg| match argument {
                    MirSubscriptArg::Index(register) => {
                        self.call_operand(prog, function, (frame_id, regs, vars), *register)
                    }
                };
                let arguments: Vec<Value> = args.iter().map(&mut subscript_value).collect();
                let call = call.as_ref().ok_or_else(|| {
                    RuntimeError::TypeError(
                        "vm: nominal multi-index lacks a checked call contract".to_string(),
                    )
                })?;
                let keyword_arguments: Vec<(String, Value)> = kwargs
                    .iter()
                    .map(|(name, argument)| (name.clone(), subscript_value(argument)))
                    .collect();
                let (receiver, receiver_ty, receiver_operand) =
                    self.receiver_operand(prog, function, (frame_id, regs, vars), *object)?;
                let result = self.method_call(
                    prog,
                    MethodInvocation {
                        receiver,
                        receiver_ty,
                        method: "__getitem__",
                        resolved_name: Some(&call.target),
                        result_adapter: None,
                        arguments,
                        keyword_arguments,
                        receiver_operand,
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
                args,
                kwargs,
                value,
                value_keyword,
                call,
            } => {
                let mut operand = |register: Reg| {
                    self.call_operand(prog, function, (frame_id, regs, vars), register)
                };
                let arguments = args
                    .iter()
                    .map(|MirSubscriptArg::Index(register)| operand(*register))
                    .collect::<Vec<_>>();
                let mut keyword_arguments = kwargs
                    .iter()
                    .map(|(name, MirSubscriptArg::Index(register))| {
                        (name.clone(), operand(*register))
                    })
                    .collect::<Vec<_>>();
                keyword_arguments.push((value_keyword.clone(), operand(*value)));
                let (receiver, receiver_ty, receiver_operand) =
                    self.receiver_operand(prog, function, (frame_id, regs, vars), *receiver)?;
                let _ = self.method_call(
                    prog,
                    MethodInvocation {
                        receiver,
                        receiver_ty,
                        method: "__setitem__",
                        resolved_name: Some(&call.target),
                        result_adapter: None,
                        arguments,
                        keyword_arguments,
                        receiver_operand,
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
            MirInstr::MakeArray { dest, elems } | MirInstr::MakeRefPack { dest, elems } => {
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
            // The array's elements are its storage's indices: a place handle
            // selects one more, and a heap pointer to the array roots one.
            MirInstr::ArrayGep { dest, array, index } => {
                let element =
                    usize::try_from(crate::runtime::value_as_index(&regs[index.0 as usize])?)
                        .map_err(|_| {
                            RuntimeError::TypeError("array.gep of a negative element".to_string())
                        })?;
                let address = match regs[array.0 as usize].clone() {
                    Value::Ref {
                        frame,
                        slot,
                        mut projection,
                    }
                    | Value::PlacePointer {
                        frame,
                        slot,
                        mut projection,
                    } => {
                        projection.push(RefProjection::Index(element));
                        Value::Ref {
                            frame,
                            slot,
                            projection,
                        }
                    }
                    Value::Pointer { allocation, offset } => Value::Ref {
                        frame: POINTER_ROOT_FRAME,
                        slot: self.pointer_root(allocation, offset),
                        projection: vec![RefProjection::Index(0), RefProjection::Index(element)],
                    },
                    other => {
                        return Err(RuntimeError::TypeError(format!(
                            "array.gep through {}",
                            crate::runtime::type_name(&other)
                        )));
                    }
                };
                regs[dest.0 as usize] = address_as(
                    &prog.structs,
                    address,
                    prog.mir.functions[function].1.reg_types.get(&dest.0),
                );
            }
            MirInstr::MakeStruct { dest, fields } => {
                let dest_ty = prog.mir.functions[function].1.reg_types.get(&dest.0);
                let name = dest_ty
                    .and_then(|ty| super::simd_instance(&prog.structs, ty))
                    .map(|(name, _)| name)
                    .or_else(|| prog.declared_struct(dest_ty))
                    .or_else(|| prog.declared_literal_struct(dest_ty))
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
                self.store_at_call_place(
                    &prog.mir.declarations,
                    frame_id,
                    place,
                    replacement,
                    regs,
                    vars,
                )?;
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
                self.store_at_call_place(
                    &prog.mir.declarations,
                    frame_id,
                    place,
                    replacement,
                    regs,
                    vars,
                )?;
                regs[dest.0 as usize] = *old_payload;
            }
            MirInstr::MakeSimd {
                dest,
                dtype,
                width,
                elems,
            } => {
                let reg_types = &prog.mir.functions[function].1.reg_types;
                // A `Bool` element is its held `!kgen.scalar<bool>` lane, as
                // upstream's `SIMD.__init__(value: Bool)` reads it.
                let vals: Vec<Value> = elems
                    .iter()
                    .map(|r| match reg_types.get(&r.0) {
                        Some(ty) if ty.is_bool() => regs[r.0 as usize].clone(),
                        ty => prog.boxed(regs[r.0 as usize].clone(), ty),
                    })
                    .collect();
                let (dtype, width) = concrete_simd_slots(&(dtype.clone(), width.clone()))?;
                regs[dest.0 as usize] = simd_from_values(dtype, width, &vals)?;
            }
            MirInstr::SimdCast {
                dest,
                value,
                dtype,
                width,
                fastmath: _,
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
                if let Some(handle) = self.heap_handle_place(
                    (&prog.mir.functions[function].1, &prog.mir.declarations),
                    place,
                    regs,
                    vars,
                )? {
                    self.canonicalize_value_references(frame_id, vars, &mut v);
                    self.write_reference(&handle, frame_id, vars, v)?;
                    return Ok(());
                }
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
                    self.place_handle(&prog.mir.declarations, frame_id, place, regs, vars)?
                };
                let slot = super::references::pointer_element_slot(place);
                if let Some(handle) = handle {
                    if slot {
                        self.canonicalize_value_references(frame_id, vars, &mut v);
                        self.write_reference(&handle, frame_id, vars, v)?;
                    } else {
                        self.store_through_handle(
                            frame_id,
                            prog.holds_reference(function, place),
                            handle,
                            v,
                            vars,
                        )?;
                    }
                } else if !slot && prog.holds_reference(function, place) {
                    let reference = load_place(vars, regs, place)?;
                    self.canonicalize_value_references(frame_id, vars, &mut v);
                    self.write_reference(&reference, frame_id, vars, v)?;
                } else {
                    self.store_at_place(&prog.mir.declarations, frame_id, place, v, regs, vars)?;
                }
            }
            MirInstr::StoreRef { place, reference } => {
                // A heap pointer standing for a getter's reference into an
                // allocation (`heap_reference_result`) is stored as the
                // reference to its pointee.
                let handle = match regs[reference.0 as usize].clone() {
                    Value::Pointer { allocation, offset } => Value::Ref {
                        frame: POINTER_ROOT_FRAME,
                        slot: self.pointer_root(allocation, offset),
                        projection: vec![RefProjection::Index(0)],
                    },
                    handle => handle,
                };
                if !matches!(handle, Value::Ref { .. }) {
                    return Err(RuntimeError::TypeError(
                        "vm: reference storage requires a reference handle".to_string(),
                    ));
                }
                self.store_at_place(&prog.mir.declarations, frame_id, place, handle, regs, vars)?;
            }
            MirInstr::LoadPlace { dest, place } => {
                regs[dest.0 as usize] =
                    self.load_place_value(prog, function, frame_id, regs, vars, place)?;
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
                // and cannot reach this fallback. A held list is recognized
                // by its slot's type, as it is its `Span`.
                let held = held_variadic_list(prog, function, *iter);
                if let Some((_, length)) = variadic_list_span(&vars[*iter as usize], held) {
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
                let held = held_variadic_list(prog, function, *iter);
                if let Some((elements, length)) = variadic_list_span(&vars[*iter as usize], held) {
                    let element = self.read_reference(&elements, frame_id, vars)?;
                    regs[dest.0 as usize] = self.read_reference(&element, frame_id, vars)?;
                    let rest = (
                        super::references::offset_place_handle(elements, 1)?,
                        Value::Int(length - 1),
                    );
                    set_variadic_list_span(&mut vars[*iter as usize], rest, held);
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
                let value = self.take_place(&prog.mir.declarations, frame_id, place, regs, vars)?;
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
    /// `src`: the error [`Self::raise`] makes, which the `try` catches.
    fn stage_error(&mut self, registers: &[Value], src: Reg) -> Result<Value, RuntimeError> {
        let error = match self.raise(&registers[src.0 as usize]) {
            RuntimeError::Raised(error) => error,
            raised => return Err(raised),
        };
        self.record_catch(&error);
        Ok(error)
    }

    /// Record that a `try` caught `error` into its slot.
    pub(super) fn record_catch(&mut self, error: &Value) {
        if let Some(message) = self.error_text(error) {
            self.record_lifecycle(format!("catch {message}"));
        }
    }

    /// The error a `raise` of `value` propagates as: `Raised`, which the
    /// nearest enclosing `try` intercepts, or which otherwise unwinds the
    /// frame. The checker converted a raised string to the `Error` struct.
    pub(super) fn raise(&mut self, value: &Value) -> RuntimeError {
        if let Some(message) = self.error_text(value) {
            self.record_lifecycle(format!("raise {message}"));
        }
        RuntimeError::Raised(value.clone())
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

/// Whether the register `register` of `function` is a numeric literal.
fn is_literal_register(prog: &Prog, function: usize, register: Reg) -> bool {
    prog.mir.functions[function]
        .1
        .reg_types
        .get(&register.0)
        .is_some_and(Ty::is_numeric_literal)
}

/// The number the numeric literal register `register` of `function` holds,
/// which its closed type carries: the register itself is the empty literal
/// struct.
fn register_literal(
    prog: &Prog,
    function: usize,
    register: Reg,
) -> Result<crate::runtime::ExactNum, RuntimeError> {
    let ty = prog.mir.functions[function].1.reg_types.get(&register.0);
    ty.and_then(crate::runtime::literal_of)
        .ok_or_else(|| crate::runtime::literal_type_error(ty))
}

/// The empty literal value of the register `register` of `function` when
/// it is a numeric or string literal, whose number or text its type
/// carries; `None` for a register of any other type.
fn literal_register_value(
    prog: &Prog,
    function: usize,
    register: Reg,
) -> Option<Result<Value, RuntimeError>> {
    let ty = prog.mir.functions[function].1.reg_types.get(&register.0)?;
    let closed = if ty.is_numeric_literal() {
        crate::runtime::literal_of(ty).is_some()
    } else if ty.is_string_literal() {
        ty.kind().string_literal_text().is_some()
    } else {
        return None;
    };
    Some(
        closed
            .then(|| crate::runtime::literal_value(ty))
            .flatten()
            .ok_or_else(|| crate::runtime::literal_type_error(Some(ty))),
    )
}

/// The value an operator reads from the register `register` of
/// `function`: a string literal register is the empty struct whose text
/// its type carries, which the operator reads as that text.
fn operand_value(prog: &Prog, function: usize, regs: &[Value], register: Reg) -> Value {
    prog.mir.functions[function]
        .1
        .reg_types
        .get(&register.0)
        .and_then(|ty| ty.kind().string_literal_text())
        .map_or_else(
            || regs[register.0 as usize].clone(),
            |text| Value::Str(text.to_string()),
        )
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

/// Whether the slot `var` of `function` is a `VariadicList` by its static
/// type, which holds the list as its one `Span` field.
fn held_variadic_list(prog: &Prog, function: usize, var: u32) -> bool {
    prog.mir.functions[function]
        .1
        .var_tys
        .get(&var)
        .and_then(|ty| ty.kind().as_non_simd_struct())
        .is_some_and(|(name, _)| {
            mojito_types::types::struct_template(name)
                == mojito_types::types::VARIADIC_LIST_TYPE_NAME
        })
}

/// The span of the `VariadicList` an owned loop over a `var` collector
/// consumes, which the builtin `HasNext`/`Next` step: the pointer to its
/// first element pointer and its length. A `held` list is the `Span`
/// itself; any other is the boxed struct around it.
fn variadic_list_span(value: &Value, held: bool) -> Option<(Value, i64)> {
    if held
        && let Value::Struct { fields, .. } = value
        && let [(_, elements), (_, Value::Int(length))] = fields.as_slice()
    {
        return Some((elements.clone(), *length));
    }
    match variadic_list_span_fields(value)? {
        [(_, elements), (_, Value::Int(length))] => Some((elements.clone(), *length)),
        _ => None,
    }
}

/// Narrow the span of the `VariadicList` `value`, `held` as its `Span` or
/// boxed, to `(elements, length)`.
fn set_variadic_list_span(value: &mut Value, (elements, length): (Value, Value), held: bool) {
    if held
        && let Value::Struct { fields, .. } = value
        && let [(_, data), (_, size)] = fields.as_mut_slice()
    {
        *data = elements;
        *size = length;
        return;
    }
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
