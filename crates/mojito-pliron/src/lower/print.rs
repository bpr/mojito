//! Printing and hashing: `print`, `write_to` bridging, hash leaves,
//! scalar formatting, and string comparison operators.

use super::*;

impl<'a> FnLowering<'a> {
    /// `print(args…)`: format each argument through the runtime `mjrt_fmt_*`
    /// family (string-literal, Bool, and None text comes from the constant
    /// pool), joined by single spaces with a trailing newline — composing the
    /// same bytes as the VM's `format_value` join (`backend/vm.rs`). The
    /// destination register is `None`-typed and erased.
    pub(super) fn lower_print(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        args: &[Reg],
    ) -> Result<(), PlironError> {
        for (i, arg) in args.iter().enumerate() {
            if i > 0 {
                self.write_literal_bytes(ctx, b" ", dest);
            }
            self.print_value(ctx, *arg, dest)?;
        }
        self.write_literal_bytes(ctx, b"\n", dest);
        self.erased.insert(dest.0);
        Ok(())
    }

    /// Display one nominal struct by calling its unique compiled `write_to`
    /// instance over a fresh builtin-string accumulator, then write the
    /// accumulated bytes to stdout and free them.
    pub(super) fn print_struct_via_write_to(
        &mut self,
        ctx: &mut Context,
        arg: Reg,
        name: &str,
        dest: Reg,
    ) -> Result<(), PlironError> {
        let prefix = format!("{name}.write_to");
        // The instance's per-instantiation clone (`write_to$y3:Int`) wins
        // over the template's erased `write_to` instantiated for the same
        // receiver.
        let mut candidates: Vec<_> = self
            .signatures
            .iter()
            .filter(|(fname, _)| fname.starts_with(&prefix))
            .collect();
        if candidates.len() > 1 {
            candidates.retain(|(fname, _)| fname.as_str() != prefix);
        }
        let [(_, signature)] = candidates.as_slice() else {
            return Err(self.unsupported_reg(
                if candidates.is_empty() {
                    format!("display of `{name}` without a compiled `write_to`")
                } else {
                    format!("display of `{name}` with ambiguous `write_to` instances")
                },
                dest,
            ));
        };
        if signature.outcome.is_some() {
            return Err(self.unsupported_reg(format!("raising `{prefix}`"), dest));
        }
        let callee: Identifier = signature
            .mangled
            .as_str()
            .try_into()
            .expect("mangled names are identifier-safe");
        let func_ty = signature.func_ty;
        let writer = self.entry_alloca(ctx, 16, 8);
        self.mem_zero(ctx, writer, 16);
        let recv_ptr = self.reg_ptr(ctx, arg)?;
        let call = CallOp::new(
            ctx,
            CallOpCallable::Direct(callee),
            func_ty,
            vec![recv_ptr, writer],
        );
        self.append(ctx, call.get_operation(), Some(dest));
        let (data, len) = self.string_parts(ctx, writer, dest);
        self.write_stdout(ctx, data, len, dest);
        self.emit_free(ctx, data);
        Ok(())
    }

    /// Append one nominal struct's display text into an existing
    /// builtin-string writer by calling its unique compiled `write_to`
    /// instance with that writer — the VM's `format_value` recursion when a
    /// `Writer.write` argument is itself a struct.
    pub(super) fn append_struct_via_write_to(
        &mut self,
        ctx: &mut Context,
        arg: Reg,
        name: &str,
        writer: Value,
        dest: Reg,
    ) -> Result<(), PlironError> {
        let prefix = format!("{name}.write_to");
        // The instance's per-instantiation clone (`write_to$y3:Int`) wins
        // over the template's erased `write_to` instantiated for the same
        // receiver.
        let mut candidates: Vec<_> = self
            .signatures
            .iter()
            .filter(|(fname, _)| fname.starts_with(&prefix))
            .collect();
        if candidates.len() > 1 {
            candidates.retain(|(fname, _)| fname.as_str() != prefix);
        }
        let [(_, signature)] = candidates.as_slice() else {
            return Err(self.unsupported_reg(
                if candidates.is_empty() {
                    format!("display of `{name}` without a compiled `write_to`")
                } else {
                    format!("display of `{name}` with ambiguous `write_to` instances")
                },
                dest,
            ));
        };
        if signature.outcome.is_some() {
            return Err(self.unsupported_reg(format!("raising `{prefix}`"), dest));
        }
        let callee: Identifier = signature
            .mangled
            .as_str()
            .try_into()
            .expect("mangled names are identifier-safe");
        let func_ty = signature.func_ty;
        let recv_ptr = self.reg_ptr(ctx, arg)?;
        let call = CallOp::new(
            ctx,
            CallOpCallable::Direct(callee),
            func_ty,
            vec![recv_ptr, writer],
        );
        self.append(ctx, call.get_operation(), Some(dest));
        Ok(())
    }

    /// Contribute one scalar/literal Hashable leaf to a caller-owned hasher —
    /// the VM's non-struct `__hash__` intrinsic. Scalars are normalized to
    /// their unsigned bit pattern zero-extended to `UInt64` (`-0.0` folds to
    /// `0.0`) and passed to the hasher's compiled `_update_with_simd`; a
    /// string literal materializes as a nominal `String` and dispatches to
    /// that struct's `__hash__` instance bound to the hasher.
    pub(super) fn lower_hash_leaf(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        recv: Reg,
        receiver: &Ty,
        hasher: Reg,
        hasher_place: Option<&MirPlace>,
    ) -> Result<(), PlironError> {
        let Some(Ty::Struct(hasher_name, _)) = self.func.reg_types.get(&hasher.0).cloned() else {
            return Err(self.unsupported_reg("`__hash__` without a nominal hasher".into(), dest));
        };
        let Some(place) = hasher_place else {
            return Err(self.unsupported_reg(
                "`__hash__` hasher argument without a mutable place".into(),
                dest,
            ));
        };
        let place = place.clone();
        let hasher_ptr = self.place_address(ctx, &place, dest)?.0;
        let unique_instance = |this: &Self, prefix: &str, by_hasher: bool| {
            this.unique_hash_instance(dest, &hasher_name, prefix, by_hasher)
        };
        let i64_ty: TypeHandle = IntegerType::get(ctx, 64, Signedness::Signless).into();
        let bits = match receiver {
            Ty::StringLiteral => {
                // Materialize the literal as an owned nominal String, hash it
                // through the struct's own `__hash__`, and release the copy.
                let target = unique_instance(self, "String.__hash__", true)?;
                let storage = self.entry_alloca(ctx, 24, 8);
                if let Some(bytes) = self.str_consts.get(&recv.0).cloned() {
                    let len = bytes.len() as u64;
                    let global = self.shared.intern_string(ctx, &bytes);
                    let len_value = self.uint_constant(ctx, len);
                    let data = self.emit_alloc(ctx, len_value, 1, dest);
                    if len > 0 {
                        let literal = self.global_address(ctx, &global, dest);
                        self.mem_copy(ctx, data, literal, len, dest);
                    }
                    self.store_string_fields(ctx, storage, data, len_value, len_value, dest);
                } else if let Some(descriptor) = self.str_runtime.get(&recv.0).copied() {
                    let data = self.emit_alloc(ctx, descriptor.len, 1, dest);
                    self.mem_copy_dynamic(ctx, data, descriptor.data, descriptor.len, dest);
                    self.store_string_fields(
                        ctx,
                        storage,
                        data,
                        descriptor.len,
                        descriptor.len,
                        dest,
                    );
                } else {
                    let ptr = self.reg_ptr(ctx, recv)?;
                    let (src_data, len) = self.string_parts(ctx, ptr, dest);
                    let data = self.emit_alloc(ctx, len, 1, dest);
                    self.mem_copy_dynamic(ctx, data, src_data, len, dest);
                    self.store_string_fields(ctx, storage, data, len, len, dest);
                }
                self.emit_bound_call(ctx, dest, &target, vec![storage, hasher_ptr])?;
                let (data, _) = self.string_parts(ctx, storage, dest);
                self.emit_free(ctx, data);
                return Ok(());
            }
            Ty::Int => self.reg_value(ctx, recv, ScalarTy::Int)?,
            Ty::UInt => self.reg_value(ctx, recv, ScalarTy::UInt)?,
            Ty::Bool => {
                let value = self.reg_value(ctx, recv, ScalarTy::Bool)?;
                let cast = ZExtOp::new_with_nneg(ctx, value, i64_ty, false);
                self.append(ctx, cast.get_operation(), Some(dest));
                cast.get_result(ctx)
            }
            Ty::Float64 => {
                let value = self.reg_value(ctx, recv, ScalarTy::Float64)?;
                self.folded_float_bits(ctx, value, ScalarTy::Float64, dest)
            }
            Ty::Simd { dtype, width: 1 } => match ScalarTy::of_dtype(*dtype) {
                ScalarTy::Int => self.reg_value(ctx, recv, ScalarTy::Int)?,
                ScalarTy::Float64 => {
                    let value = self.reg_value(ctx, recv, ScalarTy::Float64)?;
                    self.folded_float_bits(ctx, value, ScalarTy::Float64, dest)
                }
                ScalarTy::Bool => {
                    let value = self.reg_value(ctx, recv, ScalarTy::Bool)?;
                    let cast = ZExtOp::new_with_nneg(ctx, value, i64_ty, false);
                    self.append(ctx, cast.get_operation(), Some(dest));
                    cast.get_result(ctx)
                }
                ScalarTy::Sized(Dtype::Float32) => {
                    let value = self.reg_value(ctx, recv, ScalarTy::Sized(Dtype::Float32))?;
                    self.folded_float_bits(ctx, value, ScalarTy::Sized(Dtype::Float32), dest)
                }
                scalar @ ScalarTy::Sized(sized) => {
                    let value = self.reg_value(ctx, recv, scalar)?;
                    let (bits, _) =
                        mojito_vm::runtime::integer_dtype_bits(sized).expect("sized integer lane");
                    if bits == 64 {
                        value
                    } else {
                        let cast = ZExtOp::new_with_nneg(ctx, value, i64_ty, false);
                        self.append(ctx, cast.get_operation(), Some(dest));
                        cast.get_result(ctx)
                    }
                }
                ScalarTy::UInt | ScalarTy::Ptr => {
                    return Err(self.unsupported_reg(format!("hashing a `{receiver}` leaf"), dest));
                }
            },
            other => {
                return Err(self.unsupported_reg(format!("hashing a `{other}` leaf"), dest));
            }
        };
        let target = unique_instance(self, &format!("{hasher_name}._update_with_simd"), false)?;
        self.emit_bound_call(ctx, dest, &target, vec![hasher_ptr, bits])
    }

    /// The unique compiled instance whose symbol starts with `prefix` (a
    /// `String.` prefix matches the module-qualified nominal String owner);
    /// with `by_hasher` the instance's second parameter must be the hasher.
    pub(super) fn unique_hash_instance(
        &self,
        dest: Reg,
        hasher_name: &str,
        prefix: &str,
        by_hasher: bool,
    ) -> Result<String, PlironError> {
        let matches_prefix = |fname: &str| {
            if let Some(method) = prefix.strip_prefix("String.") {
                fname.rsplit_once(method).is_some_and(|(owner, rest)| {
                    owner.ends_with('.')
                        && owner[..owner.len() - 1]
                            .rsplit('$')
                            .next()
                            .is_some_and(mojito_symbol::symbol::is_stdlib_string_struct)
                        && (rest.is_empty() || rest.starts_with('$'))
                })
            } else {
                fname.starts_with(prefix)
            }
        };
        let mut candidates = self.signatures.iter().filter(|(fname, signature)| {
            matches_prefix(fname)
                && (!by_hasher
                    || matches!(signature.params.get(1), Some(LowerTy::Aggregate { ty, .. })
                    if matches!(&**ty, Ty::Struct(name, _) if *name == hasher_name)))
        });
        let Some((name, _)) = candidates.next() else {
            return Err(self.unsupported_reg(
                format!("hashing without a compiled `{prefix}` instance"),
                dest,
            ));
        };
        if candidates.next().is_some() {
            return Err(
                self.unsupported_reg(format!("hashing with ambiguous `{prefix}` instances"), dest)
            );
        }
        Ok(name.clone())
    }

    /// The IEEE bit pattern of a float leaf zero-extended to i64, with
    /// `-0.0` folded to `0.0` (the two compare equal, so they hash alike).
    pub(super) fn folded_float_bits(
        &mut self,
        ctx: &mut Context,
        value: Value,
        scalar: ScalarTy,
        dest: Reg,
    ) -> Value {
        let i64_ty: TypeHandle = IntegerType::get(ctx, 64, Signedness::Signless).into();
        let (zero, int_ty): (Value, TypeHandle) = match scalar {
            ScalarTy::Sized(Dtype::Float32) => {
                let f64_zero = self.float_constant(ctx, 0.0);
                let f32_ty: TypeHandle = FP32Type::get(ctx).into();
                let narrowed = FPTruncOp::new(ctx, f64_zero, f32_ty);
                self.append(ctx, narrowed.get_operation(), Some(dest));
                (
                    narrowed.get_result(ctx),
                    IntegerType::get(ctx, 32, Signedness::Signless).into(),
                )
            }
            _ => (self.float_constant(ctx, 0.0), i64_ty),
        };
        let is_zero = self.fcmp(ctx, FCmpPredicateAttr::OEQ, value, zero);
        self.append(ctx, is_zero.get_operation(), Some(dest));
        let folded = SelectOp::new(ctx, is_zero.get_result(ctx), zero, value);
        self.append(ctx, folded.get_operation(), Some(dest));
        let cast = BitcastOp::new(ctx, folded.get_result(ctx), int_ty);
        self.append(ctx, cast.get_operation(), Some(dest));
        let raw = cast.get_result(ctx);
        if matches!(scalar, ScalarTy::Sized(Dtype::Float32)) {
            let widened = ZExtOp::new_with_nneg(ctx, raw, i64_ty, false);
            self.append(ctx, widened.get_operation(), Some(dest));
            widened.get_result(ctx)
        } else {
            raw
        }
    }

    /// The builtin-string writer's `write`: grow-and-append each argument's
    /// display text into the `mut`-aliased `{data, len}` descriptor — the
    /// VM's `Value::Str` writer.
    pub(super) fn lower_str_writer_write(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        recv: Reg,
        args: &[Reg],
        recv_place: Option<&MirPlace>,
    ) -> Result<(), PlironError> {
        let descriptor = match recv_place {
            Some(place) => {
                let place = place.clone();
                self.place_address(ctx, &place, dest)?.0
            }
            None => self.reg_ptr(ctx, recv)?,
        };
        let i64_handle: TypeHandle = IntegerType::get(ctx, 64, Signedness::Signless).into();
        let ptr_handle: TypeHandle = PointerType::get(ctx, 0).into();
        let i8_ty: TypeHandle = IntegerType::get(ctx, 8, Signedness::Signless).into();
        for arg in args {
            if let Some(Ty::Struct(name, _)) = self.func.reg_types.get(&arg.0).cloned()
                && !mojito_symbol::symbol::is_stdlib_string_struct(&name)
            {
                self.append_struct_via_write_to(ctx, *arg, &name, descriptor, dest)?;
                continue;
            }
            let (chunk, chunk_len) = self.writer_argument_text(ctx, *arg, dest)?;
            let data = LoadOp::new(ctx, descriptor, ptr_handle);
            self.append(ctx, data.get_operation(), Some(dest));
            let len_address = self.offset_address(ctx, descriptor, 8);
            let len = LoadOp::new(ctx, len_address, i64_handle);
            self.append(ctx, len.get_operation(), Some(dest));
            let total = AddOp::new_with_overflow_flag(
                ctx,
                len.get_result(ctx),
                chunk_len,
                no_overflow_flags(),
            );
            self.append(ctx, total.get_operation(), Some(dest));
            let merged = self.emit_alloc(ctx, total.get_result(ctx), 1, dest);
            self.mem_copy_dynamic(ctx, merged, data.get_result(ctx), len.get_result(ctx), dest);
            let tail = GetElementPtrOp::new(
                ctx,
                merged,
                vec![GepIndex::Value(len.get_result(ctx))],
                i8_ty,
            );
            self.append(ctx, tail.get_operation(), Some(dest));
            self.mem_copy_dynamic(ctx, tail.get_result(ctx), chunk, chunk_len, dest);
            self.emit_free(ctx, data.get_result(ctx));
            let store = StoreOp::new(ctx, merged, descriptor);
            self.append(ctx, store.get_operation(), Some(dest));
            let store = StoreOp::new(ctx, total.get_result(ctx), len_address);
            self.append(ctx, store.get_operation(), Some(dest));
        }
        self.erased.insert(dest.0);
        Ok(())
    }

    /// Write the UTF-8 bytes of a string-valued register to stdout when the
    /// register holds one of the supported string shapes — an interned
    /// constant, a runtime StringLiteral (descriptor or typed storage), or a
    /// nominal String. Returns whether the register was such a string.
    pub(super) fn try_write_string_bytes(
        &mut self,
        ctx: &mut Context,
        arg: Reg,
        dest: Reg,
    ) -> Result<bool, PlironError> {
        if let Some(bytes) = self.str_consts.get(&arg.0).cloned() {
            self.write_literal_bytes(ctx, &bytes, dest);
            return Ok(true);
        }
        if let Some(descriptor) = self.str_runtime.get(&arg.0).copied() {
            self.write_stdout(ctx, descriptor.data, descriptor.len, dest);
            return Ok(true);
        }
        // A nominal String's byte buffer (the VM's `write_to` bridge reads
        // the same bytes), or a runtime StringLiteral value's (typed
        // storage) descriptor bytes.
        let is_string = match self.func.reg_types.get(&arg.0) {
            Some(Ty::Struct(name, _)) => mojito_symbol::symbol::is_stdlib_string_struct(name),
            Some(Ty::StringLiteral) => true,
            _ => false,
        };
        if is_string {
            let ptr = self.reg_ptr(ctx, arg)?;
            let (data, size) = self.string_parts(ctx, ptr, dest);
            self.write_stdout(ctx, data, size, dest);
            return Ok(true);
        }
        Ok(false)
    }

    /// Emit the display bytes of one `print` argument.
    pub(super) fn print_value(
        &mut self,
        ctx: &mut Context,
        arg: Reg,
        dest: Reg,
    ) -> Result<(), PlironError> {
        if self.try_write_string_bytes(ctx, arg, dest)? {
            return Ok(());
        }
        // An error value prints its bare message (the VM's `format_value`
        // over `Value::Error`).
        if matches!(self.func.reg_types.get(&arg.0), Some(Ty::Error)) {
            let ptr = self.reg_ptr(ctx, arg)?;
            let (data, size) = self.string_parts(ctx, ptr, dest);
            self.write_stdout(ctx, data, size, dest);
            return Ok(());
        }
        // A `None`-typed argument prints its constant text without reading
        // the (erased) register.
        if matches!(self.func.reg_types.get(&arg.0), Some(Ty::None)) {
            self.write_literal_bytes(ctx, b"None", dest);
            return Ok(());
        }
        // A slice descriptor writes as `Slice(start, end, step)` with `None`
        // for an absent bound (upstream's `Slice.write_to`; the contiguous and
        // strided kinds delegate to it).
        if self
            .func
            .reg_types
            .get(&arg.0)
            .and_then(slice_struct_name)
            .is_some()
        {
            return self.print_slice_descriptor(ctx, arg, dest);
        }
        // A nominal struct displays through its `write_to` conformance over
        // the builtin-string accumulator — the VM's `format_value` dispatch.
        if let Some(Ty::Struct(name, _)) = self.func.reg_types.get(&arg.0).cloned()
            && !mojito_symbol::symbol::is_stdlib_string_struct(&name)
        {
            return self.print_struct_via_write_to(ctx, arg, &name, dest);
        }
        if let Some(Ty::Simd { dtype, width }) = self.func.reg_types.get(&arg.0).cloned()
            && width > 1
        {
            return self.print_simd(ctx, arg, dtype, width as usize, dest);
        }
        if let Some(Ty::Ref(reference)) = self.func.reg_types.get(&arg.0).cloned() {
            let referent = *reference.referent;
            if let LowerTy::Scalar(scalar) =
                lower_ty(self.name, &referent, &self.layout, self.reg_span(arg))?
            {
                let pointer = self.reg_value(ctx, arg, ScalarTy::Ptr)?;
                let handle = scalar.handle(ctx);
                let load = LoadOp::new(ctx, pointer, handle);
                self.append(ctx, load.get_operation(), Some(dest));
                return self.print_scalar(ctx, scalar, load.get_result(ctx), dest);
            }
        }
        let ty = match self.concrete_scalar_ty(arg)? {
            Some(ty) => ty,
            // A bare literal argument materializes at the VM's default kind.
            // A runtime FloatLiteral value rejects: the VM displays its
            // exact rational (`1/10`), which f64 storage cannot reproduce.
            None => match self.func.reg_types.get(&arg.0) {
                Some(Ty::FloatLiteral) => {
                    if !self.pending_literals.contains_key(&arg.0) {
                        return Err(self.unsupported_reg(
                            "display of a runtime FloatLiteral value".into(),
                            dest,
                        ));
                    }
                    ScalarTy::Float64
                }
                _ => ScalarTy::Int,
            },
        };
        let value = self.reg_value(ctx, arg, ty)?;
        self.print_scalar(ctx, ty, value, dest)
    }

    pub(super) fn print_simd(
        &mut self,
        ctx: &mut Context,
        arg: Reg,
        dtype: Dtype,
        width: usize,
        dest: Reg,
    ) -> Result<(), PlironError> {
        let ptr = self.reg_ptr(ctx, arg)?;
        let lane_ty = ScalarTy::of_dtype(dtype);
        let lane_handle = lane_ty.handle(ctx);
        let lane_layout = self
            .layout
            .layout_of(&Ty::Simd { dtype, width: 1 })
            .expect("SIMD lane layout");
        self.write_literal_bytes(ctx, b"[", dest);
        for lane in 0..width {
            if lane > 0 {
                self.write_literal_bytes(ctx, b", ", dest);
            }
            let address = self.offset_address(ctx, ptr, lane_layout.size * lane as u64);
            let load = LoadOp::new(ctx, address, lane_handle);
            self.append(ctx, load.get_operation(), Some(dest));
            self.print_scalar(ctx, lane_ty, load.get_result(ctx), dest)?;
        }
        self.write_literal_bytes(ctx, b"]", dest);
        Ok(())
    }

    /// Emit the display bytes of one scalar value: `mjrt_fmt_*` into the
    /// scratch buffer for the numeric kinds (the runtime formats exactly the
    /// VM's display text), pooled `True`/`False` selection for Bool.
    pub(super) fn print_scalar(
        &mut self,
        ctx: &mut Context,
        ty: ScalarTy,
        value: Value,
        dest: Reg,
    ) -> Result<(), PlironError> {
        let (data, len) = self.format_scalar(ctx, ty, value, dest)?;
        self.write_stdout(ctx, data, len, dest);
        Ok(())
    }

    /// The display bytes of one scalar value as a `(data, len)` pair. The
    /// numeric kinds live in the shared scratch buffer, valid until the next
    /// formatting call.
    pub(super) fn format_scalar(
        &mut self,
        ctx: &mut Context,
        ty: ScalarTy,
        value: Value,
        dest: Reg,
    ) -> Result<(Value, Value), PlironError> {
        let (symbol, value) = match ty {
            ScalarTy::Int => ("mjrt_fmt_i64", value),
            ScalarTy::UInt => ("mjrt_fmt_u64", value),
            ScalarTy::Float64 => ("mjrt_fmt_f64", value),
            // A `Float32` displays as its f64 view (the VM formats the lane's
            // stored f64 with the same shortest-round-trip rules).
            ScalarTy::Sized(Dtype::Float32) => ("mjrt_fmt_f64", self.f32_to_f64(ctx, value, dest)),
            // Sized integers display their mathematical value.
            ScalarTy::Sized(dtype) => {
                let (_, signed) = mojito_vm::runtime::integer_dtype_bits(dtype)
                    .expect("Float32 is matched above");
                let wide = self.sized_to_i64(ctx, value, dtype, dest);
                (
                    if signed {
                        "mjrt_fmt_i64"
                    } else {
                        "mjrt_fmt_u64"
                    },
                    wide,
                )
            }
            ScalarTy::Ptr => {
                return Err(self.unsupported_reg("display of a Pointer".into(), dest));
            }
            ScalarTy::Bool => {
                let true_global = self.shared.intern_string(ctx, b"True");
                let false_global = self.shared.intern_string(ctx, b"False");
                let true_ptr = self.global_address(ctx, &true_global, dest);
                let false_ptr = self.global_address(ctx, &false_global, dest);
                let data = SelectOp::new(ctx, value, true_ptr, false_ptr);
                self.append(ctx, data.get_operation(), Some(dest));
                let true_len = self.uint_constant(ctx, 4);
                let false_len = self.uint_constant(ctx, 5);
                let len = SelectOp::new(ctx, value, true_len, false_len);
                self.append(ctx, len.get_operation(), Some(dest));
                return Ok((data.get_result(ctx), len.get_result(ctx)));
            }
        };
        let scratch = self.scratch_buffer(ctx);
        let fmt_ty = self.shared.ensure_rt(ctx, symbol);
        let identifier: Identifier = symbol
            .try_into()
            .expect("runtime symbols are identifier-safe");
        let call = CallOp::new(
            ctx,
            CallOpCallable::Direct(identifier),
            fmt_ty,
            vec![value, scratch],
        );
        self.append(ctx, call.get_operation(), Some(dest));
        Ok((scratch, call.get_result(ctx)))
    }

    /// `Slice(start, end, step)`: each raw bound word prints as an Int when
    /// its presence bit is set and as `None` otherwise (selected without
    /// branching: the Int text is formatted into the scratch buffer either
    /// way, then the `(data, len)` pair is chosen).
    fn print_slice_descriptor(
        &mut self,
        ctx: &mut Context,
        arg: Reg,
        dest: Reg,
    ) -> Result<(), PlironError> {
        let descriptor = self.reg_ptr(ctx, arg)?;
        let i64_handle: TypeHandle = IntegerType::get(ctx, 64, Signedness::Signless).into();
        let none_global = self.shared.intern_string(ctx, b"None");
        self.write_literal_bytes(ctx, b"Slice(", dest);
        for (index, (offset, bit)) in [(0u64, 1i64), (8, 2), (16, 4)].into_iter().enumerate() {
            if index > 0 {
                self.write_literal_bytes(ctx, b", ", dest);
            }
            let address = self.offset_address(ctx, descriptor, offset);
            let word = LoadOp::new(ctx, address, i64_handle);
            self.append(ctx, word.get_operation(), Some(dest));
            let flags_address = self.offset_address(ctx, descriptor, 24);
            let flags = LoadOp::new(ctx, flags_address, i64_handle);
            self.append(ctx, flags.get_operation(), Some(dest));
            let mask = self.int_constant(ctx, bit);
            let masked = AndOp::new(ctx, flags.get_result(ctx), mask);
            self.append(ctx, masked.get_operation(), Some(dest));
            let zero = self.int_constant(ctx, 0);
            let is_set = ICmpOp::new(ctx, ICmpPredicateAttr::NE, masked.get_result(ctx), zero);
            self.append(ctx, is_set.get_operation(), Some(dest));
            let (int_data, int_len) =
                self.format_scalar(ctx, ScalarTy::Int, word.get_result(ctx), dest)?;
            let none_data = self.global_address(ctx, &none_global, dest);
            let none_len = self.uint_constant(ctx, 4);
            let data = SelectOp::new(ctx, is_set.get_result(ctx), int_data, none_data);
            self.append(ctx, data.get_operation(), Some(dest));
            let len = SelectOp::new(ctx, is_set.get_result(ctx), int_len, none_len);
            self.append(ctx, len.get_operation(), Some(dest));
            self.write_stdout(ctx, data.get_result(ctx), len.get_result(ctx), dest);
        }
        self.write_literal_bytes(ctx, b")", dest);
        Ok(())
    }

    /// Intern `bytes` and write them to stdout.
    pub(super) fn write_literal_bytes(&mut self, ctx: &mut Context, bytes: &[u8], dest: Reg) {
        let global = self.shared.intern_string(ctx, bytes);
        self.write_global(ctx, &global, bytes.len() as u64, dest);
    }

    /// Write `len` bytes of a constant-pool global to stdout.
    pub(super) fn write_global(
        &mut self,
        ctx: &mut Context,
        global: &Identifier,
        len: u64,
        dest: Reg,
    ) {
        let data = self.global_address(ctx, global, dest);
        let len = self.uint_constant(ctx, len);
        self.write_stdout(ctx, data, len, dest);
    }

    /// `mjrt_write_stdout(data, len)` — writes exactly the given bytes or
    /// traps (category 4).
    pub(super) fn write_stdout(&mut self, ctx: &mut Context, data: Value, len: Value, dest: Reg) {
        let write_ty = self.shared.ensure_rt(ctx, "mjrt_write_stdout");
        let call = CallOp::new(
            ctx,
            CallOpCallable::Direct("mjrt_write_stdout".try_into().expect("valid identifier")),
            write_ty,
            vec![data, len],
        );
        self.append(ctx, call.get_operation(), Some(dest));
    }

    /// The address of a module global in the current block.
    pub(super) fn global_address(
        &mut self,
        ctx: &mut Context,
        global: &Identifier,
        dest: Reg,
    ) -> Value {
        let address = AddressOfOp::new(ctx, global.clone(), 0);
        self.append(ctx, address.get_operation(), Some(dest));
        address.get_result(ctx)
    }

    /// The function's 32-byte formatting buffer (`mjrt_fmt_i64`/`u64` need
    /// at least 20 bytes, `mjrt_fmt_f64` at least 32), created once at the
    /// top of the entry block so loops reuse one slot.
    pub(super) fn scratch_buffer(&mut self, ctx: &mut Context) -> Value {
        if let Some(scratch) = self.scratch {
            return scratch;
        }
        let value = self.entry_alloca(ctx, 32, 8);
        self.scratch = Some(value);
        value
    }

    /// Compile-time folding of the string-literal operators the VM evaluates
    /// on `Value::Str`: `+` concatenates into a new interned literal, `==` and
    /// `!=` fold to Bool constants. Both operands must be compile-time
    /// literals — no runtime StringLiteral representation exists.
    /// The `(data, len)` byte pair of a string-shaped operand: an interned
    /// constant, a runtime string pair, or StringLiteral/String descriptor
    /// storage.
    pub(super) fn string_operand_parts(
        &mut self,
        ctx: &mut Context,
        reg: Reg,
        dest: Reg,
    ) -> Result<(Value, Value), PlironError> {
        if let Some(bytes) = self.str_consts.get(&reg.0).cloned() {
            let global = self.shared.intern_string(ctx, &bytes);
            let data = self.global_address(ctx, &global, dest);
            let len = self.uint_constant(ctx, bytes.len() as u64);
            return Ok((data, len));
        }
        if let Some(descriptor) = self.str_runtime.get(&reg.0).copied() {
            return Ok((descriptor.data, descriptor.len));
        }
        match self.func.reg_types.get(&reg.0) {
            Some(Ty::StringLiteral | Ty::Error) => {
                let ptr = self.reg_ptr(ctx, reg)?;
                Ok(self.string_parts(ctx, ptr, dest))
            }
            Some(Ty::Struct(name, _)) if mojito_symbol::symbol::is_stdlib_string_struct(name) => {
                let ptr = self.reg_ptr(ctx, reg)?;
                Ok(self.string_parts(ctx, ptr, dest))
            }
            _ => Err(self.unsupported_reg("string operand".into(), dest)),
        }
    }

    /// Runtime string equality: equal lengths and equal bytes, via an inline
    /// byte-compare loop over slot-backed state (mem2reg promotes it).
    pub(super) fn lower_str_runtime_eq(
        &mut self,
        ctx: &mut Context,
        op: InfixOp,
        dest: Reg,
        a: Reg,
        b: Reg,
    ) -> Result<(), PlironError> {
        let (a_data, a_len) = self.string_operand_parts(ctx, a, dest)?;
        let (b_data, b_len) = self.string_operand_parts(ctx, b, dest)?;
        let i1_handle: TypeHandle = IntegerType::get(ctx, 1, Signedness::Signless).into();
        let i64_handle: TypeHandle = IntegerType::get(ctx, 64, Signedness::Signless).into();
        let i8_handle: TypeHandle = IntegerType::get(ctx, 8, Signedness::Signless).into();
        let result_slot = self.entry_typed_alloca(ctx, i1_handle);
        let index_slot = self.entry_typed_alloca(ctx, i64_handle);
        let len_eq = ICmpOp::new(ctx, ICmpPredicateAttr::EQ, a_len, b_len);
        self.append(ctx, len_eq.get_operation(), Some(dest));
        let store = StoreOp::new(ctx, len_eq.get_result(ctx), result_slot);
        self.append(ctx, store.get_operation(), Some(dest));
        let zero = self.int_constant(ctx, 0);
        let store = StoreOp::new(ctx, zero, index_slot);
        self.append(ctx, store.get_operation(), Some(dest));
        let region = self.region.expect("lowering is inside a function");
        let head = BasicBlock::new(ctx, None, vec![]);
        head.insert_at_back(region, ctx);
        let body = BasicBlock::new(ctx, None, vec![]);
        body.insert_at_back(region, ctx);
        let done = BasicBlock::new(ctx, None, vec![]);
        done.insert_at_back(region, ctx);
        let enter = BrOp::new(ctx, head, vec![]);
        self.append(ctx, enter.get_operation(), Some(dest));
        // head: continue while `index < len` and no mismatch was found.
        self.current = Some(head);
        let index = LoadOp::new(ctx, index_slot, i64_handle);
        self.append(ctx, index.get_operation(), Some(dest));
        let result = LoadOp::new(ctx, result_slot, i1_handle);
        self.append(ctx, result.get_operation(), Some(dest));
        let in_range = ICmpOp::new(ctx, ICmpPredicateAttr::ULT, index.get_result(ctx), a_len);
        self.append(ctx, in_range.get_operation(), Some(dest));
        let live = AndOp::new(ctx, in_range.get_result(ctx), result.get_result(ctx));
        self.append(ctx, live.get_operation(), Some(dest));
        let branch = CondBrOp::new(ctx, live.get_result(ctx), body, vec![], done, vec![]);
        self.append(ctx, branch.get_operation(), Some(dest));
        // body: compare one byte, fold into the result, advance.
        self.current = Some(body);
        let index = LoadOp::new(ctx, index_slot, i64_handle);
        self.append(ctx, index.get_operation(), Some(dest));
        let a_byte_ptr = GetElementPtrOp::new(
            ctx,
            a_data,
            vec![GepIndex::Value(index.get_result(ctx))],
            i8_handle,
        );
        self.append(ctx, a_byte_ptr.get_operation(), Some(dest));
        let a_byte = LoadOp::new(ctx, a_byte_ptr.get_result(ctx), i8_handle);
        self.append(ctx, a_byte.get_operation(), Some(dest));
        let b_byte_ptr = GetElementPtrOp::new(
            ctx,
            b_data,
            vec![GepIndex::Value(index.get_result(ctx))],
            i8_handle,
        );
        self.append(ctx, b_byte_ptr.get_operation(), Some(dest));
        let b_byte = LoadOp::new(ctx, b_byte_ptr.get_result(ctx), i8_handle);
        self.append(ctx, b_byte.get_operation(), Some(dest));
        let byte_eq = ICmpOp::new(
            ctx,
            ICmpPredicateAttr::EQ,
            a_byte.get_result(ctx),
            b_byte.get_result(ctx),
        );
        self.append(ctx, byte_eq.get_operation(), Some(dest));
        let store = StoreOp::new(ctx, byte_eq.get_result(ctx), result_slot);
        self.append(ctx, store.get_operation(), Some(dest));
        let one = self.int_constant(ctx, 1);
        let next =
            AddOp::new_with_overflow_flag(ctx, index.get_result(ctx), one, no_overflow_flags());
        self.append(ctx, next.get_operation(), Some(dest));
        let store = StoreOp::new(ctx, next.get_result(ctx), index_slot);
        self.append(ctx, store.get_operation(), Some(dest));
        let advance = BrOp::new(ctx, head, vec![]);
        self.append(ctx, advance.get_operation(), Some(dest));
        // done: the folded verdict, negated for `!=`.
        self.current = Some(done);
        let result = LoadOp::new(ctx, result_slot, i1_handle);
        self.append(ctx, result.get_operation(), Some(dest));
        let mut value = result.get_result(ctx);
        if matches!(op, InfixOp::Ne) {
            let truth = self.bool_constant(ctx, true);
            let flipped = XorOp::new(ctx, value, truth);
            self.append(ctx, flipped.get_operation(), Some(dest));
            value = flipped.get_result(ctx);
        }
        self.reg_values.insert(dest.0, value);
        Ok(())
    }

    pub(super) fn lower_str_literal_binop(
        &mut self,
        ctx: &mut Context,
        op: InfixOp,
        dest: Reg,
        a: Reg,
        b: Reg,
    ) -> Result<(), PlironError> {
        let (Some(lhs), Some(rhs)) = (
            self.str_consts.get(&a.0).cloned(),
            self.str_consts.get(&b.0).cloned(),
        ) else {
            return Err(self.unsupported_reg("runtime StringLiteral operand".into(), dest));
        };
        match op {
            InfixOp::Add => {
                let mut bytes = lhs;
                bytes.extend_from_slice(&rhs);
                self.str_consts.insert(dest.0, bytes);
                Ok(())
            }
            InfixOp::Eq | InfixOp::Ne => {
                let equal = lhs == rhs;
                let value = if matches!(op, InfixOp::Eq) {
                    equal
                } else {
                    !equal
                };
                let constant = self.bool_constant(ctx, value);
                self.reg_values.insert(dest.0, constant);
                Ok(())
            }
            other => Err(self.unsupported_reg(
                format!("operator `{other:?}` on StringLiteral operands"),
                dest,
            )),
        }
    }
}
