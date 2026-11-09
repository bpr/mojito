//! Method-call lowering plus `abort` and writer builtins.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::TyKind;

impl FnLowering<'_> {
    /// The `CopyIteratorReference` adapter over a method result: the callee
    /// returned a pointer to storage it borrowed out, while the contract the
    /// caller checked against yields the element by value, so the result is
    /// an owned copy of the referent (the VM's `clone_value`).
    pub(super) fn copy_reference_result(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        constructor: Option<&str>,
    ) -> Result<(), PlironError> {
        let Some(result) = self.func.reg_types.get(&dest.0).cloned() else {
            return Err(self.unsupported_reg("untyped reference result".into(), dest));
        };
        let place = self.reg_ptr(ctx, dest)?;
        match lower_ty(
            self.name,
            &result,
            &self.layout,
            self.mir_declarations,
            self.reg_span(dest),
        )? {
            LowerTy::Scalar(scalar) => {
                let handle = scalar.handle(ctx);
                let load = LoadOp::new(ctx, place, handle);
                self.define(ctx, dest, load.get_operation(), load.get_result(ctx))
            }
            LowerTy::Aggregate { ty, layout } => {
                self.copy_named_aggregate(ctx, dest, &ty, layout, place, constructor)
            }
            LowerTy::ZeroSized => {
                self.erased.insert(dest.0);
                Ok(())
            }
        }
    }

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
        // Pointer receivers dispatch to runtime intrinsics, never to compiled
        // stdlib bodies.
        if matches!(
            self.func.reg_types.get(&recv.0).map(Ty::kind),
            Some(TyKind::Pointer { .. })
        ) {
            return self.lower_pointer_method(ctx, dest, recv, method, args);
        }
        // The struct-to-literal bridge (the VM's `string_struct_literal`):
        // the declared stub body must never execute, and the bridged bytes
        // would need an owner the literal value model cannot record — the
        // VM's arena never reclaims, while a native copy stored into a
        // drop-inert literal-typed field leaks with no releasing owner.
        // Reject until a literal-ownership design lands.
        if method == "_as_string_literal"
            && self
                .func
                .reg_types
                .get(&recv.0)
                .and_then(|ty| ty.kind().as_non_simd_struct())
                .is_some_and(|(name, _)| mojito_symbol::symbol::is_stdlib_string_struct(name))
        {
            return Err(self.unsupported_reg("String struct-to-literal bridge".into(), dest));
        }
        // `Sized` spelled on a specialized pack's storage: it has no nominal
        // struct to resolve the method onto, so the static count answers it
        // exactly as the `len` builtin does.
        if resolved.is_none()
            && method == "__len__"
            && args.is_empty()
            && kwargs.is_empty()
            && matches!(
                self.func.reg_types.get(&recv.0).map(Ty::kind),
                Some(TyKind::Tuple(_) | TyKind::RuntimePack(_))
            )
        {
            return self.lower_len_builtin(ctx, dest, &[recv], &[]);
        }
        // An unresolved scalar-receiver rounding dunder is the VM's
        // non-struct intrinsic dispatch (`builtin_round_dir`), an integer
        // literal's included; a struct receiver with its own method arrives
        // resolved instead.
        if resolved.is_none()
            && matches!(method, "__floor__" | "__ceil__" | "__trunc__")
            && args.is_empty()
            && let Some(recv_ty) = self.func.reg_types.get(&recv.0).cloned()
            && (recv_ty.is_int() || recv_ty.is_uint() || recv_ty.is_float64())
        {
            return self.lower_round_dir(ctx, dest, recv, &recv_ty, method);
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
        // A receiver with a known place passes the caller's storage address
        // directly (write-through) — copy-in/copy-out would point an escaping
        // interior pointer at the copy — as does a `mut` receiver the callee
        // takes by reference (a scalar's `SIMD.__setitem__`). A placeless
        // receiver register is either an aliased load (its address) or an
        // owned register.
        let receiver_alias = recv_place.is_some()
            && (matches!(params[0], LowerTy::Aggregate { .. })
                || by_reference.first().copied().unwrap_or(false));
        // A zero-sized receiver (a numeric literal) has no physical
        // operand: the compiled signature erased its slot.
        let recv_value = if receiver_alias {
            let place = recv_place.expect("aliased receivers have a place").clone();
            Some(self.aliased_receiver_address(ctx, &place, dest)?)
        } else if matches!(params[0], LowerTy::ZeroSized) {
            None
        } else {
            Some(self.arg_value(ctx, recv, &params[0], recv_owned || deinit_receiver, dest)?)
        };
        let rest = &params[1..];
        let rest_owned = if owned.len() > 1 { &owned[1..] } else { &[] };
        let rest_by_reference = if by_reference.len() > 1 {
            &by_reference[1..]
        } else {
            &[]
        };
        let mut lowered: Vec<Value> = recv_value.into_iter().collect();
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
        self.emit_bound_call(ctx, dest, resolved, lowered)?;
        // `mut self` (the struct's mut_self_methods set — keyed by either the
        // overload-qualified or the source method name) and named destructors
        // write the receiver back; a missing place means a discarded
        // temporary receiver.
        let write_back = !receiver_alias
            && !self.aliased_load_regs.contains(&recv.0)
            && match self.func.reg_types.get(&recv.0).map(Ty::kind) {
                Some(struct_ty) if let Some((struct_name, _)) = struct_ty.as_non_simd_struct() => {
                    let is_mut = self
                        .struct_decls
                        .get(struct_name.as_str())
                        .is_some_and(|d| {
                            d.mut_self_methods.contains(resolved)
                                || d.mut_self_methods.contains(method)
                        });
                    is_mut || (deinit_receiver && matches!(params[0], LowerTy::Aggregate { .. }))
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

    /// `_mojito_abort(message)` — the `std.os.abort` crossing: report the
    /// dynamic message with the distinct uncatchable abort category.
    pub(super) fn lower_abort_builtin(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        message: Reg,
    ) -> Result<(), PlironError> {
        let (data, len) = self.writer_argument_text(ctx, message, false, dest)?;
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

    /// The display bytes of one `Writer.write` argument as a `(data, len)`
    /// pair, or with `repr` its representation bytes — the VM's host text
    /// (`format_value`) over the supported argument shapes.
    pub(super) fn writer_argument_text(
        &mut self,
        ctx: &mut Context,
        arg: Reg,
        repr: bool,
        dest: Reg,
    ) -> Result<(Value, Value), PlironError> {
        let quoted = self.is_string_constant(arg)
            || self.str_runtime.contains_key(&arg.0)
            || self
                .func
                .reg_types
                .get(&arg.0)
                .is_some_and(|ty| !ty.kind().is_error() && ty.is_non_simd_struct());
        if repr && quoted {
            return Err(self.unsupported_reg(
                "representation of a string through a formatted write".into(),
                dest,
            ));
        }
        if let Some(bytes) = self.string_constant(arg) {
            let global = self.shared.intern_string(ctx, &bytes);
            let data = self.global_address(ctx, &global, dest);
            let len = self.uint_constant(ctx, bytes.len() as u64);
            return Ok((data, len));
        }
        if let Some(descriptor) = self.str_runtime.get(&arg.0).copied() {
            return Ok((descriptor.data, descriptor.len));
        }
        match self.func.reg_types.get(&arg.0).map(Ty::kind) {
            Some(kind) if kind.is_error() => {
                let ptr = self.reg_ptr(ctx, arg)?;
                return Ok(self.string_parts(ctx, ptr, dest));
            }
            Some(struct_ty)
                if let Some((name, _)) = struct_ty.as_non_simd_struct()
                    && mojito_symbol::symbol::is_stdlib_string_struct(name) =>
            {
                let ptr = self.reg_ptr(ctx, arg)?;
                return Ok(self.string_parts(ctx, ptr, dest));
            }
            Some(struct_ty)
                if let Some((name, _)) = struct_ty.as_non_simd_struct()
                    && mojito_symbol::symbol::is_stdlib_string_span_struct(name) =>
            {
                let ptr = self.reg_ptr(ctx, arg)?;
                return Ok(self.string_parts(ctx, ptr, dest));
            }
            Some(TyKind::Ref(reference)) => {
                let referent = reference.referent.clone();
                if let LowerTy::Scalar(scalar) = lower_ty(
                    self.name,
                    &referent,
                    &self.layout,
                    self.mir_declarations,
                    self.reg_span(arg),
                )? {
                    let pointer = self.reg_value(ctx, arg, ScalarTy::Ptr)?;
                    let handle = scalar.handle(ctx);
                    let load = LoadOp::new(ctx, pointer, handle);
                    self.append(ctx, load.get_operation(), Some(dest));
                    return self.format_scalar(ctx, scalar, load.get_result(ctx), repr, dest);
                }
            }
            _ => {}
        }
        let Some(ty) = self.concrete_scalar_ty(arg)? else {
            return Err(self.unsupported_reg("formatted write argument".into(), dest));
        };
        let value = self.reg_value(ctx, arg, ty)?;
        self.format_scalar(ctx, ty, value, repr, dest)
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
        if matches!(ty.kind(), TyKind::Ref(_)) {
            let handle = ScalarTy::Ptr.handle(ctx);
            let load = LoadOp::new(ctx, address, handle);
            self.append(ctx, load.get_operation(), Some(anchor));
            Ok(load.get_result(ctx))
        } else {
            Ok(address)
        }
    }
}
