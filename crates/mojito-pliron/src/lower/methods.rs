//! Method-call lowering plus `repr`, `abort`, and writer builtins.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

impl FnLowering<'_> {
    /// A resolved method call: the receiver and aggregate arguments pass by
    /// pointer; a `mut self` (or `deinit self`) receiver's final state copies
    /// back to the caller's receiver place afterwards — the VM's
    /// `store_at_call_place` write-back.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn lower_method_call(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        recv: Reg,
        method: &str,
        resolved: Option<&str>,
        args: &[Reg],
        kwargs: &[(String, Reg)],
        arg_places: &[Option<MirPlace>],
        kwarg_places: &[Option<MirPlace>],
        recv_place: Option<&MirPlace>,
    ) -> Result<(), PlironError> {
        // A scalar/literal Hashable leaf: normalize its bits and contribute
        // them to the caller-owned hasher through the hasher's compiled
        // `_update_with_simd` (the VM's non-struct `__hash__` intrinsic).
        if resolved.is_none()
            && method == "__hash__"
            && args.len() == 1
            && kwargs.is_empty()
            && let Some(receiver) = self.func.reg_types.get(&recv.0).cloned()
            && !matches!(receiver, Ty::Struct(..) | Ty::Ref(..))
        {
            return self.lower_hash_leaf(
                ctx,
                dest,
                recv,
                &receiver,
                args[0],
                arg_places.first().and_then(Option::as_ref),
            );
        }
        // Pointer receivers dispatch to runtime intrinsics, never to compiled
        // stdlib bodies.
        if matches!(self.func.reg_types.get(&recv.0), Some(Ty::Pointer { .. })) {
            return self.lower_pointer_method(ctx, dest, recv, method, args);
        }
        if let Some(Ty::Simd { dtype, width }) = self.func.reg_types.get(&recv.0).cloned() {
            return self.lower_simd_method(ctx, dest, recv, dtype, width as usize, method, args);
        }
        // Slice descriptors are checker-virtual: `indices` is the VM's
        // intrinsic normalization and `__eq__`/`__ne__` compare the raw
        // bounds; no other method exists on them.
        if self
            .func
            .reg_types
            .get(&recv.0)
            .and_then(slice_struct_name)
            .is_some()
        {
            if method == "indices" {
                return self.lower_slice_indices(ctx, dest, recv, args);
            }
            if matches!(method, "__eq__" | "__ne__") {
                return self.lower_slice_equality(ctx, dest, recv, args, method == "__ne__");
            }
            return Err(self.unsupported_reg(format!("slice descriptor method `{method}`"), dest));
        }
        // The builtin-string writer receiver (`write_to`'s `Value::Str`
        // accumulator) appends each argument's display text in place.
        if resolved.is_none()
            && method == "write"
            && matches!(self.func.reg_types.get(&recv.0), Some(Ty::StringLiteral))
        {
            return self.lower_str_writer_write(ctx, dest, recv, args, recv_place);
        }
        // Trait-dispatched `Copyable.copy` on a scalar receiver — a generic
        // body's `value.copy()` monomorphized to a builtin, whose
        // `__trait_dispatch.` target the specializer clears on non-struct
        // receivers — is the value read itself, as the VM's non-struct `copy`
        // intrinsic. Struct receivers arrive resolved to their own `copy`.
        if resolved.is_none()
            && method == "copy"
            && args.is_empty()
            && kwargs.is_empty()
            && let Some(scalar) = self.func.reg_types.get(&recv.0).and_then(scalar_copy_ty)
        {
            let value = self.reg_value(ctx, recv, scalar)?;
            self.reg_values.insert(dest.0, value);
            return Ok(());
        }
        // The same dispatch on a receiver register typed as a `ref` to a
        // scalar (a reference result retained in a hidden `$call_ref` slot,
        // `span[i].copy()`): the receiver `LoadPlace` read the referent
        // through the slot, so the register carries the scalar itself.
        if resolved.is_none()
            && method == "copy"
            && args.is_empty()
            && kwargs.is_empty()
            && let Some(Ty::Ref(reference)) = self.func.reg_types.get(&recv.0)
            && let Some(scalar) = scalar_copy_ty(&reference.referent)
        {
            let value = self.reg_value(ctx, recv, scalar)?;
            self.reg_values.insert(dest.0, value);
            return Ok(());
        }
        // The struct-to-literal bridge (the VM's `string_struct_literal`):
        // the declared stub body must never execute, and the bridged bytes
        // would need an owner the literal value model cannot record — the
        // VM's arena never reclaims, while a native copy stored into a
        // drop-inert literal-typed field leaks with no releasing owner.
        // Reject until a literal-ownership design lands.
        if method == "_as_string_literal"
            && matches!(self.func.reg_types.get(&recv.0), Some(Ty::Struct(name, _))
                if mojito_symbol::symbol::is_stdlib_string_struct(name))
        {
            return Err(self.unsupported_reg("String struct-to-literal bridge".into(), dest));
        }
        // The VM-synthesized `Writer.write` dispatch: format each argument
        // and feed it through the receiver's compiled `write_string`.
        if resolved.is_none()
            && method == "write"
            && let Some(Ty::Struct(writer, _)) = self.func.reg_types.get(&recv.0).cloned()
            && self
                .signatures
                .contains_key(&format!("{writer}.write_string"))
        {
            return self.lower_writer_write(ctx, dest, &writer, args, recv_place);
        }
        // Unresolved scalar-receiver dunders are the VM's non-struct
        // intrinsic dispatch (`builtin_round_dir`/`builtin_ceildiv`); a
        // struct receiver with its own method arrives resolved instead.
        if resolved.is_none()
            && let Some(recv_ty) = self.func.reg_types.get(&recv.0).cloned()
            && matches!(recv_ty, Ty::Int | Ty::UInt | Ty::Float64)
        {
            match (method, args.len()) {
                ("__floor__" | "__ceil__" | "__trunc__", 0) => {
                    return self.lower_round_dir(ctx, dest, recv, &recv_ty, method);
                }
                ("__ceildiv__", 1) => {
                    return self.lower_ceildiv(ctx, dest, recv, args[0], &recv_ty);
                }
                _ => {}
            }
        }
        let Some(resolved) = resolved else {
            let receiver = self.func.reg_types.get(&recv.0).map_or_else(
                || "an untyped register".to_string(),
                std::string::ToString::to_string,
            );
            return Err(self.unsupported_reg(
                format!("unresolved method call `{method}` on a receiver of type {receiver}"),
                dest,
            ));
        };
        let Some(signature) = self.signatures.get(resolved) else {
            return Err(
                self.unsupported_reg(format!("method call to uncompiled `{resolved}`"), dest)
            );
        };
        let params = signature.params.clone();
        let owned = signature.owned_params.clone();
        let by_reference = signature.ref_params.clone();
        let deinit_receiver = signature.deinit_receiver;
        if params.is_empty() {
            return Err(self.unsupported_reg(
                format!("method `{resolved}` without a receiver parameter"),
                dest,
            ));
        }
        let recv_owned = owned.first().copied().unwrap_or(false);
        // A `mut`/`ref` receiver with a known place passes the caller's
        // storage address directly (write-through) — copy-in/copy-out would
        // point an escaping interior pointer at the copy. A `read`/`deinit`
        // receiver (or a placeless temporary) keeps the VM's clone-on-read
        // copy.
        let receiver_alias = recv_place.is_some() && matches!(params[0], LowerTy::Aggregate { .. });
        let recv_value = if receiver_alias {
            let place = recv_place.expect("aliased receivers have a place").clone();
            self.aliased_receiver_address(ctx, &place, dest)?
        } else {
            self.arg_value(ctx, recv, &params[0], recv_owned || deinit_receiver, dest)?
        };
        let rest = &params[1..];
        let rest_owned = if owned.len() > 1 { &owned[1..] } else { &[] };
        let rest_by_reference = if by_reference.len() > 1 {
            &by_reference[1..]
        } else {
            &[]
        };
        let mut lowered = vec![recv_value];
        if kwargs.is_empty() && args.len() == rest.len() && !self.variadic_callee(resolved) {
            for (i, (arg, expected)) in args.iter().zip(rest).enumerate() {
                // A zero-sized argument (the `NoneType` operand of
                // `Optional.__is__`) has no physical operand: the compiled
                // signature erased its slot.
                if matches!(expected, LowerTy::ZeroSized) {
                    continue;
                }
                let owned = rest_owned.get(i).copied().unwrap_or(false);
                let value = if rest_by_reference.get(i).copied().unwrap_or(false) {
                    // A `mut`/`ref` argument passes the address of the
                    // caller's designated storage (write-through).
                    let Some(place) = arg_places.get(i).and_then(Option::as_ref) else {
                        return Err(self.unsupported_reg(
                            format!("`mut`/`ref` argument of `{resolved}` without a place"),
                            dest,
                        ));
                    };
                    let place = place.clone();
                    self.place_address(ctx, &place, dest)?.0
                } else {
                    self.place_backed_arg_value(
                        ctx,
                        *arg,
                        expected,
                        owned,
                        arg_places.get(i).and_then(Option::as_ref),
                        dest,
                    )?
                };
                lowered.push(value);
            }
        } else {
            lowered.extend(self.bind_call_slots(
                ctx,
                dest,
                resolved,
                rest,
                rest_owned,
                rest_by_reference,
                args,
                kwargs,
                arg_places,
                kwarg_places,
            )?);
        }
        // A named destructor owns its aliased receiver from the call on (the
        // callee consumes the residual fields at their last use), so the
        // caller's presence flags clear before the call: the raise edge must
        // not destroy that storage again.
        if deinit_receiver && receiver_alias {
            let place = recv_place.expect("aliased deinit receiver has a place");
            if place.proj.is_empty() {
                self.set_drop_flag(ctx, place.root, false);
            } else if let Some(flag) = self.leaf_position(place).and_then(|position| {
                self.leaf_flags
                    .get(&place.root)
                    .and_then(|leaves| leaves.get(&position))
                    .copied()
            }) {
                let absent = self.bool_constant(ctx, false);
                let store = StoreOp::new(ctx, absent, flag);
                self.append(ctx, store.get_operation(), None);
            } else {
                self.partially_moved.insert(place.root);
            }
        }
        self.emit_bound_call(ctx, dest, resolved, lowered)?;
        // `mut self` (the struct's mut_self_methods set — keyed by either the
        // overload-qualified or the source method name) and named destructors
        // write the receiver back; a missing place means a discarded
        // temporary receiver.
        let write_back = !receiver_alias
            && match self.func.reg_types.get(&recv.0) {
                Some(Ty::Struct(struct_name, _)) => {
                    let is_mut = self
                        .struct_decls
                        .get(struct_name.as_str())
                        .is_some_and(|d| {
                            d.mut_self_methods.contains(resolved)
                                || d.mut_self_methods.contains(method)
                        });
                    is_mut || deinit_receiver
                }
                _ => false,
            };
        if write_back && let Some(place) = recv_place {
            let LowerTy::Aggregate { layout, .. } = &params[0] else {
                return Err(
                    self.unsupported_reg("mutating method on a scalar receiver".into(), dest)
                );
            };
            let size = layout.size;
            let recv_ptr = self.reg_ptr(ctx, recv)?;
            let (address, _) = self.place_address(ctx, place, dest)?;
            self.mem_copy(ctx, address, recv_ptr, size, dest);
        }
        Ok(())
    }

    /// `repr(value)`: an owned runtime `StringLiteral` matching the VM's scalar
    /// vocabulary or a nominal value's compiled `write_repr_to` — the nominal
    /// String included, whose single-quoted escaping loop is stdlib Mojo.
    pub(super) fn lower_repr_builtin(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        arg: Reg,
    ) -> Result<(), PlironError> {
        if self
            .func
            .reg_types
            .get(&arg.0)
            .and_then(slice_struct_name)
            .is_some()
        {
            let (data, len) = self.slice_string(ctx, arg, dest, true)?;
            self.str_runtime.insert(
                dest.0,
                RuntimeStr {
                    data,
                    len,
                    owned: true,
                },
            );
            return self.mark_owned_temp(dest, Ty::StringLiteral);
        }
        if matches!(self.func.reg_types.get(&arg.0), Some(Ty::None)) {
            let global = self.shared.intern_string(ctx, b"None");
            let source = self.global_address(ctx, &global, dest);
            let len = self.uint_constant(ctx, 4);
            let data = self.emit_alloc(ctx, len, 1, dest);
            self.mem_copy_dynamic(ctx, data, source, len, dest);
            self.str_runtime.insert(
                dest.0,
                RuntimeStr {
                    data,
                    len,
                    owned: true,
                },
            );
            return self.mark_owned_temp(dest, Ty::StringLiteral);
        }
        if let Some(literal_scalar) = match self.func.reg_types.get(&arg.0) {
            Some(Ty::IntLiteral) => Some(ScalarTy::Int),
            Some(Ty::FloatLiteral) => Some(ScalarTy::Float64),
            _ => None,
        } {
            let value = self.reg_value(ctx, arg, literal_scalar)?;
            let (source, len) = self.format_scalar(ctx, literal_scalar, value, dest)?;
            let label = match literal_scalar {
                ScalarTy::Int => Some("Int("),
                ScalarTy::Float64 => Some("Float64("),
                _ => None,
            };
            let (source, len) = if let Some(label) = label {
                self.wrap_repr_scalar(ctx, source, len, label, dest)
            } else {
                (source, len)
            };
            let data = self.emit_alloc(ctx, len, 1, dest);
            self.mem_copy_dynamic(ctx, data, source, len, dest);
            self.str_runtime.insert(
                dest.0,
                RuntimeStr {
                    data,
                    len,
                    owned: true,
                },
            );
            return self.mark_owned_temp(dest, Ty::StringLiteral);
        }
        if !matches!(
            self.func.reg_types.get(&arg.0),
            Some(Ty::Struct(..) | Ty::Ref(..))
        ) && let Some(scalar) = self.concrete_scalar_ty(arg)?
        {
            let value = self.reg_value(ctx, arg, scalar)?;
            let (source, len) = self.format_scalar(ctx, scalar, value, dest)?;
            let label = match scalar {
                ScalarTy::Int => Some("Int("),
                ScalarTy::UInt => Some("UInt("),
                ScalarTy::Float64 | ScalarTy::Sized(Dtype::Float32) => Some("Float64("),
                _ => None,
            };
            let (source, len) = if let Some(label) = label {
                self.wrap_repr_scalar(ctx, source, len, label, dest)
            } else {
                (source, len)
            };
            let data = self.emit_alloc(ctx, len, 1, dest);
            self.mem_copy_dynamic(ctx, data, source, len, dest);
            self.str_runtime.insert(
                dest.0,
                RuntimeStr {
                    data,
                    len,
                    owned: true,
                },
            );
            return self.mark_owned_temp(dest, Ty::StringLiteral);
        }
        if let Some(Ty::Ref(reference)) = self.func.reg_types.get(&arg.0).cloned()
            && let LowerTy::Scalar(scalar) = lower_ty(
                self.name,
                &reference.referent,
                &self.layout,
                self.reg_span(arg),
            )?
        {
            let pointer = self.reg_value(ctx, arg, ScalarTy::Ptr)?;
            let handle = scalar.handle(ctx);
            let value = LoadOp::new(ctx, pointer, handle);
            self.append(ctx, value.get_operation(), Some(dest));
            let (source, len) = self.format_scalar(ctx, scalar, value.get_result(ctx), dest)?;
            let data = self.emit_alloc(ctx, len, 1, dest);
            self.mem_copy_dynamic(ctx, data, source, len, dest);
            self.str_runtime.insert(
                dest.0,
                RuntimeStr {
                    data,
                    len,
                    owned: true,
                },
            );
            return self.mark_owned_temp(dest, Ty::StringLiteral);
        }
        if let Some(Ty::Struct(name, _)) = self.func.reg_types.get(&arg.0).cloned() {
            let prefix = format!("{name}.write_repr_to");
            let mut candidates: Vec<_> = self
                .signatures
                .iter()
                .filter(|(candidate, _)| candidate.starts_with(&prefix))
                .collect();
            if candidates.len() > 1 {
                candidates.retain(|(candidate, _)| candidate.as_str() != prefix);
            }
            if let [(_, signature)] = candidates.as_slice() {
                let writer = self.entry_alloca(ctx, 16, 8);
                self.mem_zero(ctx, writer, 16);
                let receiver = self.reg_ptr(ctx, arg)?;
                let callee: Identifier = signature
                    .mangled
                    .as_str()
                    .try_into()
                    .expect("mangled names are identifier-safe");
                let call = CallOp::new(
                    ctx,
                    CallOpCallable::Direct(callee),
                    signature.func_ty,
                    vec![receiver, writer],
                );
                self.append(ctx, call.get_operation(), Some(dest));
                let (data, len) = self.string_parts(ctx, writer, dest);
                self.str_runtime.insert(
                    dest.0,
                    RuntimeStr {
                        data,
                        len,
                        owned: true,
                    },
                );
                return self.mark_owned_temp(dest, Ty::StringLiteral);
            }
            return Err(self.unsupported_reg(
                format!("repr of `{name}` without compiled write_repr_to"),
                dest,
            ));
        }
        Err(self.unsupported_reg("repr over a non-String value".into(), dest))
    }

    fn wrap_repr_scalar(
        &mut self,
        ctx: &mut Context,
        data: Value,
        len: Value,
        label: &str,
        dest: Reg,
    ) -> (Value, Value) {
        let extra = self.uint_constant(ctx, (label.len() + 1) as u64);
        let total = AddOp::new_with_overflow_flag(ctx, len, extra, no_overflow_flags());
        self.append(ctx, total.get_operation(), Some(dest));
        let output = self.emit_alloc(ctx, total.get_result(ctx), 1, dest);
        let prefix = self.shared.intern_string(ctx, label.as_bytes());
        let prefix = self.global_address(ctx, &prefix, dest);
        let prefix_len = self.uint_constant(ctx, label.len() as u64);
        self.mem_copy_dynamic(ctx, output, prefix, prefix_len, dest);
        let i8_ty: TypeHandle = IntegerType::get(ctx, 8, Signedness::Signless).into();
        let body = GetElementPtrOp::new(
            ctx,
            output,
            vec![GepIndex::Constant(label.len() as u32)],
            i8_ty,
        );
        self.append(ctx, body.get_operation(), Some(dest));
        self.mem_copy_dynamic(ctx, body.get_result(ctx), data, len, dest);
        let end = AddOp::new_with_overflow_flag(ctx, len, prefix_len, no_overflow_flags());
        self.append(ctx, end.get_operation(), Some(dest));
        let tail = GetElementPtrOp::new(
            ctx,
            output,
            vec![GepIndex::Value(end.get_result(ctx))],
            i8_ty,
        );
        self.append(ctx, tail.get_operation(), Some(dest));
        let close = self.shared.intern_string(ctx, b")");
        let close = self.global_address(ctx, &close, dest);
        let one = self.uint_constant(ctx, 1);
        self.mem_copy_dynamic(ctx, tail.get_result(ctx), close, one, dest);
        (output, total.get_result(ctx))
    }

    /// `_mojito_abort(message)` — the `std.os.abort` crossing: report the
    /// dynamic message with the distinct uncatchable abort category.
    pub(super) fn lower_abort_builtin(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        message: Reg,
    ) -> Result<(), PlironError> {
        let (data, len) = self.writer_argument_text(ctx, message, dest)?;
        let abort_ty = self.shared.ensure_rt(ctx, "mjrt_abort");
        let call = CallOp::new(
            ctx,
            CallOpCallable::Direct("mjrt_abort".try_into().expect("valid identifier")),
            abort_ty,
            vec![data, len],
        );
        self.append(ctx, call.get_operation(), Some(dest));
        let unreachable = UnreachableOp::new(ctx);
        self.append(ctx, unreachable.get_operation(), None);
        // Dead continuation for the rest of the MIR block; the unreachable
        // pruning pass removes it.
        let region = self.region.expect("lowering is inside a function region");
        let dead = BasicBlock::new(ctx, None, vec![]);
        dead.insert_at_back(region, ctx);
        self.current = Some(dead);
        self.erased.insert(dest.0);
        Ok(())
    }

    fn slice_string(
        &mut self,
        ctx: &mut Context,
        arg: Reg,
        dest: Reg,
        repr: bool,
    ) -> Result<(Value, Value), PlironError> {
        let output = self.entry_alloca(ctx, 16, 8);
        self.mem_zero(ctx, output, 16);
        let descriptor = self.reg_ptr(ctx, arg)?;
        let i64_ty: TypeHandle = IntegerType::get(ctx, 64, Signedness::Signless).into();
        let flags_address = self.offset_address(ctx, descriptor, 24);
        let flags = LoadOp::new(ctx, flags_address, i64_ty);
        self.append(ctx, flags.get_operation(), Some(dest));
        let labels: [&[u8]; 3] = if repr {
            [b"Slice(start=", b", end=", b", step="]
        } else {
            [b"Slice(", b", ", b", "]
        };
        let none = self.shared.intern_string(ctx, b"None");
        for (index, ((offset, bit), label)) in [(0u64, 1i64), (8, 2), (16, 4)]
            .into_iter()
            .zip(labels)
            .enumerate()
        {
            let label_global = self.shared.intern_string(ctx, label);
            let label_data = self.global_address(ctx, &label_global, dest);
            let label_len = self.uint_constant(ctx, label.len() as u64);
            self.append_string_pair(ctx, output, label_data, label_len, dest);
            let address = self.offset_address(ctx, descriptor, offset);
            let word = LoadOp::new(ctx, address, i64_ty);
            self.append(ctx, word.get_operation(), Some(dest));
            let mask = self.int_constant(ctx, bit);
            let masked = AndOp::new(ctx, flags.get_result(ctx), mask);
            self.append(ctx, masked.get_operation(), Some(dest));
            let zero = self.int_constant(ctx, 0);
            let present = ICmpOp::new(ctx, ICmpPredicateAttr::NE, masked.get_result(ctx), zero);
            self.append(ctx, present.get_operation(), Some(dest));
            let (int_data, int_len) =
                self.format_scalar(ctx, ScalarTy::Int, word.get_result(ctx), dest)?;
            let none_data = self.global_address(ctx, &none, dest);
            let none_len = self.uint_constant(ctx, 4);
            let data = SelectOp::new(ctx, present.get_result(ctx), int_data, none_data);
            self.append(ctx, data.get_operation(), Some(dest));
            let len = SelectOp::new(ctx, present.get_result(ctx), int_len, none_len);
            self.append(ctx, len.get_operation(), Some(dest));
            self.append_string_pair(ctx, output, data.get_result(ctx), len.get_result(ctx), dest);
            let _ = index;
        }
        let close = self.shared.intern_string(ctx, b")");
        let close = self.global_address(ctx, &close, dest);
        let one = self.uint_constant(ctx, 1);
        self.append_string_pair(ctx, output, close, one, dest);
        Ok(self.string_parts(ctx, output, dest))
    }

    /// The VM-synthesized `Writer.write` dispatch: each argument's display
    /// text feeds one `write_string` call on the aliased `mut self` receiver.
    /// The payload `String` borrows the text bytes (`cap == len`); the callee
    /// reads it and never takes ownership.
    pub(super) fn lower_writer_write(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        writer: &str,
        args: &[Reg],
        recv_place: Option<&MirPlace>,
    ) -> Result<(), PlironError> {
        let write_string = format!("{writer}.write_string");
        let signature = &self.signatures[&write_string];
        if signature.outcome.is_some() {
            return Err(self.unsupported_reg(format!("raising `{write_string}`"), dest));
        }
        let payload_ty = self
            .declarations
            .get(&write_string)
            .and_then(|decl| decl.param_types.first());
        let nominal_payload = matches!(payload_ty, Some(Ty::Struct(payload, args))
            if args.is_empty() && mojito_symbol::symbol::is_stdlib_string_struct(payload));
        let literal_payload = matches!(payload_ty, Some(Ty::StringLiteral));
        if !nominal_payload && !literal_payload {
            return Err(self.unsupported_reg(
                format!("`{write_string}` without a nominal String payload"),
                dest,
            ));
        }
        let callee: Identifier = signature
            .mangled
            .as_str()
            .try_into()
            .expect("mangled names are identifier-safe");
        let func_ty = signature.func_ty;
        let Some(place) = recv_place else {
            return Err(self.unsupported_reg("`Writer.write` needs a mutable place".into(), dest));
        };
        let place = place.clone();
        let writer_address = self.place_address(ctx, &place, dest)?.0;
        for arg in args {
            let mut temporary = None;
            let (data, len) = if self
                .func
                .reg_types
                .get(&arg.0)
                .and_then(slice_struct_name)
                .is_some()
            {
                let parts = self.slice_string(ctx, *arg, dest, false)?;
                temporary = Some(parts.0);
                parts
            } else if let Some(Ty::Struct(name, _)) = self.func.reg_types.get(&arg.0).cloned()
                && !mojito_symbol::symbol::is_stdlib_string_struct(&name)
            {
                if mojito_symbol::symbol::is_stdlib_string_span_struct(&name) {
                    let span = self.reg_ptr(ctx, *arg)?;
                    self.string_parts(ctx, span, dest)
                } else if name.contains("$Named$mono$") || name.ends_with("$Named") {
                    let named_ty = self.func.reg_types[&arg.0].clone();
                    let base = self.reg_ptr(ctx, *arg)?;
                    let (name_offset, _) = self.field_offset(&named_ty, "_name", dest)?;
                    let name_address = self.offset_address(ctx, base, name_offset);
                    let (name_data, name_len) = self.string_parts(ctx, name_address, dest);
                    let descriptor = self.entry_alloca(ctx, 16, 8);
                    self.mem_zero(ctx, descriptor, 16);
                    self.append_string_pair(ctx, descriptor, name_data, name_len, dest);
                    let equals = self.shared.intern_string(ctx, b"=");
                    let equals = self.global_address(ctx, &equals, dest);
                    let one = self.uint_constant(ctx, 1);
                    self.append_string_pair(ctx, descriptor, equals, one, dest);
                    let (value_offset, value_ty) = self.field_offset(&named_ty, "_value", dest)?;
                    let Ty::Pointer { element, .. } = value_ty else {
                        return Err(self.unsupported_reg("Named value pointer".into(), dest));
                    };
                    let pointer_address = self.offset_address(ctx, base, value_offset);
                    let ptr_ty: TypeHandle = PointerType::get(ctx, 0).into();
                    let pointer = LoadOp::new(ctx, pointer_address, ptr_ty);
                    self.append(ctx, pointer.get_operation(), Some(dest));
                    let LowerTy::Scalar(scalar) =
                        lower_ty(self.name, &element, &self.layout, self.reg_span(dest))?
                    else {
                        return Err(self.unsupported_reg("aggregate Named value".into(), dest));
                    };
                    let handle = scalar.handle(ctx);
                    let value = LoadOp::new(ctx, pointer.get_result(ctx), handle);
                    self.append(ctx, value.get_operation(), Some(dest));
                    let (value_data, value_len) =
                        self.format_scalar(ctx, scalar, value.get_result(ctx), dest)?;
                    self.append_string_pair(ctx, descriptor, value_data, value_len, dest);
                    let parts = self.string_parts(ctx, descriptor, dest);
                    temporary = Some(parts.0);
                    parts
                } else {
                    let descriptor = self.entry_alloca(ctx, 16, 8);
                    self.mem_zero(ctx, descriptor, 16);
                    self.append_struct_via_write_to(ctx, *arg, &name, descriptor, dest)?;
                    let parts = self.string_parts(ctx, descriptor, dest);
                    temporary = Some(parts.0);
                    parts
                }
            } else {
                self.writer_argument_text(ctx, *arg, dest)?
            };
            let payload = self.entry_alloca(ctx, if nominal_payload { 24 } else { 16 }, 8);
            if nominal_payload {
                self.store_string_fields(ctx, payload, data, len, len, dest);
            } else {
                let store = StoreOp::new(ctx, data, payload);
                self.append(ctx, store.get_operation(), Some(dest));
                let len_address = self.gep_byte(ctx, payload, 8, dest);
                let store = StoreOp::new(ctx, len, len_address);
                self.append(ctx, store.get_operation(), Some(dest));
            }
            let call = CallOp::new(
                ctx,
                CallOpCallable::Direct(callee.clone()),
                func_ty,
                vec![writer_address, payload],
            );
            self.append(ctx, call.get_operation(), Some(dest));
            if let Some(allocation) = temporary {
                self.emit_free(ctx, allocation);
            }
        }
        self.erased.insert(dest.0);
        Ok(())
    }

    /// The display bytes of one `Writer.write` argument as a `(data, len)`
    /// pair — the VM's `format_value` over the supported argument shapes.
    pub(super) fn writer_argument_text(
        &mut self,
        ctx: &mut Context,
        arg: Reg,
        dest: Reg,
    ) -> Result<(Value, Value), PlironError> {
        if let Some(bytes) = self.str_consts.get(&arg.0).cloned() {
            let global = self.shared.intern_string(ctx, &bytes);
            let data = self.global_address(ctx, &global, dest);
            let len = self.uint_constant(ctx, bytes.len() as u64);
            return Ok((data, len));
        }
        if let Some(descriptor) = self.str_runtime.get(&arg.0).copied() {
            return Ok((descriptor.data, descriptor.len));
        }
        match self.func.reg_types.get(&arg.0) {
            Some(Ty::Error | Ty::StringLiteral) => {
                let ptr = self.reg_ptr(ctx, arg)?;
                return Ok(self.string_parts(ctx, ptr, dest));
            }
            Some(Ty::Struct(name, _)) if mojito_symbol::symbol::is_stdlib_string_struct(name) => {
                let ptr = self.reg_ptr(ctx, arg)?;
                return Ok(self.string_parts(ctx, ptr, dest));
            }
            Some(Ty::Struct(name, _))
                if mojito_symbol::symbol::is_stdlib_string_span_struct(name) =>
            {
                let ptr = self.reg_ptr(ctx, arg)?;
                return Ok(self.string_parts(ctx, ptr, dest));
            }
            Some(Ty::Ref(reference)) => {
                let referent = (*reference.referent).clone();
                if let LowerTy::Scalar(scalar) =
                    lower_ty(self.name, &referent, &self.layout, self.reg_span(arg))?
                {
                    let pointer = self.reg_value(ctx, arg, ScalarTy::Ptr)?;
                    let handle = scalar.handle(ctx);
                    let load = LoadOp::new(ctx, pointer, handle);
                    self.append(ctx, load.get_operation(), Some(dest));
                    return self.format_scalar(ctx, scalar, load.get_result(ctx), dest);
                }
            }
            _ => {}
        }
        let Some(ty) = self.concrete_scalar_ty(arg)? else {
            return Err(self.unsupported_reg("formatted write argument".into(), dest));
        };
        let value = self.reg_value(ctx, arg, ty)?;
        self.format_scalar(ctx, ty, value, dest)
    }

    /// The storage a `mut`/`ref` receiver aliases: the place's address,
    /// dereferenced once when the place designates a reference handle (a ref
    /// field like an iterator's `src`) — the VM reads through `Value::Ref`
    /// receivers before dispatch.
    pub(super) fn aliased_receiver_address(
        &mut self,
        ctx: &mut Context,
        place: &MirPlace,
        anchor: Reg,
    ) -> Result<Value, PlironError> {
        let (address, ty) = self.place_address(ctx, place, anchor)?;
        if matches!(ty, Ty::Ref(_)) {
            let handle = ScalarTy::Ptr.handle(ctx);
            let load = LoadOp::new(ctx, address, handle);
            self.append(ctx, load.get_operation(), Some(anchor));
            Ok(load.get_result(ctx))
        } else {
            Ok(address)
        }
    }
}
