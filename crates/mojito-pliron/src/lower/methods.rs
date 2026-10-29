//! Method-call lowering plus `repr`, `abort`, and writer builtins.

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
        match lower_ty(self.name, &result, &self.layout, self.reg_span(dest))? {
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
        // A `Bool`/`DType`/string-literal Hashable leaf: contribute it to
        // the caller-owned hasher through the hasher's compiled
        // `_update_with_simd` (the VM's non-struct `__hash__` intrinsic).
        if resolved.is_none()
            && method == "__hash__"
            && args.len() == 1
            && kwargs.is_empty()
            && let Some(receiver) = self.func.reg_types.get(&recv.0).cloned()
            && !matches!(receiver.kind(), TyKind::Struct(..) | TyKind::Ref(..))
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
        if matches!(
            self.func.reg_types.get(&recv.0).map(Ty::kind),
            Some(TyKind::Pointer { .. })
        ) {
            return self.lower_pointer_method(ctx, dest, recv, method, args);
        }
        // `Boolable`, `Intable`, `Floatable`, and `Indexer` on a `Bool` or a
        // numeric literal: its conversion, as the VM dispatches it; a
        // `SIMD`'s are its declared members.
        if resolved.is_none()
            && args.is_empty()
            && kwargs.is_empty()
            && self.func.reg_types.get(&recv.0).is_some_and(|ty| {
                matches!(
                    ty.kind(),
                    TyKind::Bool | TyKind::IntLiteral | TyKind::FloatLiteral
                )
            })
            && let Some(target) = match method {
                "__bool__" => Some(mojito_types::ct::ScalarConversion::Bool),
                "__float__" => Some(mojito_types::ct::ScalarConversion::Float64),
                "__int__" | "__mlir_index__" => Some(mojito_types::ct::ScalarConversion::Int),
                _ => None,
            }
        {
            return self.lower_convert(ctx, dest, target, recv);
        }
        if matches!(
            self.func.reg_types.get(&recv.0).map(Ty::kind),
            Some(TyKind::Dtype)
        ) && args.is_empty()
        {
            return self.lower_dtype_predicate(ctx, dest, recv, method);
        }
        // `StringLiteral`'s byte primitives read the descriptor: the interned
        // constant's address and length, or a runtime literal's `(data, len)`.
        if resolved.is_none()
            && args.is_empty()
            && matches!(method, "byte_length" | "ptr" | "unsafe_ptr")
            && matches!(
                self.func.reg_types.get(&recv.0).map(Ty::kind),
                Some(TyKind::StringLiteral(_))
            )
        {
            let (data, len) = self.writer_argument_text(ctx, recv, dest)?;
            let value = if method == "byte_length" { len } else { data };
            self.reg_values.insert(dest.0, value);
            return Ok(());
        }
        // The builtin-string writer receiver (`write_to`'s `Value::Str`
        // accumulator) appends each argument's display text in place; its
        // `write_string` appends the view's bytes the same way.
        if resolved.is_none()
            && matches!(
                method,
                "write" | "write_string" | mojito_symbol::symbol::WRITE_FORMATTED
            )
            && matches!(
                self.func.reg_types.get(&recv.0).map(Ty::kind),
                Some(TyKind::StringLiteral(_))
            )
        {
            return self.lower_str_writer_write(ctx, dest, recv, args, recv_place);
        }
        // The struct-to-literal bridge (the VM's `string_struct_literal`):
        // the declared stub body must never execute, and the bridged bytes
        // would need an owner the literal value model cannot record — the
        // VM's arena never reclaims, while a native copy stored into a
        // drop-inert literal-typed field leaks with no releasing owner.
        // Reject until a literal-ownership design lands.
        if method == "_as_string_literal"
            && matches!(self.func.reg_types.get(&recv.0).map(Ty::kind), Some(TyKind::Struct(name, _))
                if mojito_symbol::symbol::is_stdlib_string_struct(name))
        {
            return Err(self.unsupported_reg("String struct-to-literal bridge".into(), dest));
        }
        // The VM-synthesized `Writer.write` dispatch: format each argument
        // and feed it through the receiver's compiled `write_string`.
        if resolved.is_none()
            && method == mojito_symbol::symbol::WRITE_FORMATTED
            && let Some(TyKind::Struct(writer, _)) = (self.func.reg_types.get(&recv.0).cloned())
                .as_ref()
                .map(Ty::kind)
            && let Some(write_string) = self.struct_member(writer, "write_string", 1)
        {
            return self.lower_writer_write(ctx, dest, write_string, args, recv_place);
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
        // Unresolved scalar-receiver dunders are the VM's non-struct
        // intrinsic dispatch (`builtin_round_dir`, `builtin_ceildiv`, and
        // the `abs`/`round`/`divmod` builtins), and an integer literal's
        // rounding dunders; a struct receiver with its own method arrives
        // resolved instead.
        if resolved.is_none()
            && let Some(recv_ty) = self.func.reg_types.get(&recv.0).cloned()
            && matches!(
                recv_ty.kind(),
                TyKind::INT | TyKind::UINT | TyKind::FLOAT64 | TyKind::IntLiteral
            )
        {
            match (method, args.len()) {
                ("__floor__" | "__ceil__" | "__trunc__", 0) => {
                    return self.lower_round_dir(ctx, dest, recv, &recv_ty, method);
                }
                _ if recv_ty == TyKind::IntLiteral => {}
                ("__ceildiv__", 1) => {
                    return self.lower_ceildiv(ctx, dest, recv, args[0], &recv_ty);
                }
                ("__abs__", 0) => return self.lower_abs_builtin(ctx, dest, &[recv], &[]),
                ("__round__", 0) => return self.lower_round_builtin(ctx, dest, &[recv], &[]),
                ("__divmod__", 1) => {
                    return self.lower_divmod_builtin(ctx, dest, &[recv, args[0]], &[]);
                }
                _ => {}
            }
        }
        // `Equatable` and `Comparable` on a scalar: its operator, as the VM
        // dispatches it.
        if resolved.is_none()
            && args.len() == 1
            && kwargs.is_empty()
            && matches!(
                self.func.reg_types.get(&recv.0).map(Ty::kind),
                Some(TyKind::INT | TyKind::UINT | TyKind::FLOAT64 | TyKind::Bool)
            )
            && let Some(op) = [
                InfixOp::Eq,
                InfixOp::Ne,
                InfixOp::Lt,
                InfixOp::Le,
                InfixOp::Gt,
                InfixOp::Ge,
            ]
            .into_iter()
            .find(|op| op.dunder() == Some(method))
        {
            return self.lower_binop(ctx, op, dest, recv, args[0], None);
        }
        // The compiler-known `SIMD` methods, after the scalar dunders the
        // width-one host scalars answer above. A member `std.simd` declares
        // arrives resolved to its compiled instance, and is called below.
        if resolved.is_none_or(|resolved| !self.signatures.contains_key(resolved))
            && let Some((dtype, width)) = self.func.reg_types.get(&recv.0).and_then(simd_dims)
        {
            return self.lower_simd_method(ctx, dest, recv, dtype, width as usize, method, args);
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
        self.emit_bound_call(ctx, dest, resolved, lowered)?;
        // `mut self` (the struct's mut_self_methods set — keyed by either the
        // overload-qualified or the source method name) and named destructors
        // write the receiver back; a missing place means a discarded
        // temporary receiver.
        let write_back = !receiver_alias
            && !self.aliased_load_regs.contains(&recv.0)
            && match self.func.reg_types.get(&recv.0).map(Ty::kind) {
                Some(TyKind::Struct(struct_name, _)) => {
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
        // A string literal's repr is its `as_string_slice()`'s: the owned
        // `String` of its bytes through the nominal member (R1660).
        if self
            .func
            .reg_types
            .get(&arg.0)
            .is_some_and(Ty::is_string_literal)
        {
            let string = mojito_symbol::symbol::STDLIB_STRING_STRUCT;
            let Some(symbol) = self.display_member(string, "write_repr_to", dest)? else {
                return Err(self.unsupported_reg(
                    "repr of a literal without a compiled `String.write_repr_to`".into(),
                    dest,
                ));
            };
            let storage = self.owned_string_of_literal(ctx, arg, dest)?;
            let writer = self.entry_alloca(ctx, 16, 8);
            self.mem_zero(ctx, writer, 16);
            let signature = &self.signatures[symbol];
            let callee: Identifier = signature
                .mangled
                .as_str()
                .try_into()
                .expect("mangled names are identifier-safe");
            let call = CallOp::new(
                ctx,
                CallOpCallable::Direct(callee),
                signature.func_ty,
                vec![storage, writer],
            );
            self.append(ctx, call.get_operation(), Some(dest));
            let (copy, _) = self.string_parts(ctx, storage, dest);
            self.emit_free(ctx, copy);
            let (data, len) = self.string_parts(ctx, writer, dest);
            self.str_runtime.insert(
                dest.0,
                RuntimeStr {
                    data,
                    len,
                    owned: true,
                },
            );
            return Ok(());
        }
        if matches!(
            self.func.reg_types.get(&arg.0).map(Ty::kind),
            Some(TyKind::None)
        ) {
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
            return Ok(());
        }
        if let Some(literal_scalar) = match self.func.reg_types.get(&arg.0).map(Ty::kind) {
            Some(TyKind::IntLiteral) => Some(ScalarTy::Int),
            Some(TyKind::FloatLiteral) => Some(ScalarTy::Float64),
            _ => None,
        } {
            let value = self.reg_value(ctx, arg, literal_scalar)?;
            let (source, len) = self.format_scalar(ctx, literal_scalar, value, dest)?;
            let label = match literal_scalar {
                ScalarTy::Int => Some("Int("),
                ScalarTy::Float64 => Some("Float64("),
                _ => None,
            };
            let (data, len) = match label {
                Some(label) => self.wrap_repr_scalar(ctx, source, len, label, dest),
                None => self.owned_copy(ctx, source, len, dest),
            };
            self.str_runtime.insert(
                dest.0,
                RuntimeStr {
                    data,
                    len,
                    owned: true,
                },
            );
            return Ok(());
        }
        if !matches!(
            self.func.reg_types.get(&arg.0).map(Ty::kind),
            Some(TyKind::Struct(..) | TyKind::Ref(..))
        ) && let Some(scalar) = self.concrete_scalar_ty(arg)?
        {
            let value = self.reg_value(ctx, arg, scalar)?;
            let (source, len) = if scalar == ScalarTy::Dtype {
                self.dtype_text(ctx, value, true, dest)
            } else {
                self.format_scalar(ctx, scalar, value, dest)?
            };
            let label = match scalar {
                ScalarTy::Int => Some("Int"),
                ScalarTy::UInt => Some("UInt"),
                ScalarTy::Float64 => Some("Float64"),
                ScalarTy::Sized(dtype) => dtype.repr_alias(),
                _ => None,
            };
            let (data, len) = match label {
                Some(label) => self.wrap_repr_scalar(ctx, source, len, &format!("{label}("), dest),
                None => self.owned_copy(ctx, source, len, dest),
            };
            self.str_runtime.insert(
                dest.0,
                RuntimeStr {
                    data,
                    len,
                    owned: true,
                },
            );
            return Ok(());
        }
        if let Some(TyKind::Ref(reference)) = (self.func.reg_types.get(&arg.0).cloned())
            .as_ref()
            .map(Ty::kind)
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
            return Ok(());
        }
        if let Some(TyKind::Struct(name, _)) = (self.func.reg_types.get(&arg.0).cloned())
            .as_ref()
            .map(Ty::kind)
        {
            if let Some(symbol) = self.display_member(name, "write_repr_to", dest)? {
                let signature = &self.signatures[symbol];
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
                return Ok(());
            }
            return Err(self.unsupported_reg(
                format!("repr of `{name}` without compiled write_repr_to"),
                dest,
            ));
        }
        Err(self.unsupported_reg("repr over a non-String value".into(), dest))
    }

    /// `DType`'s `Bool` queries over the code byte, spelled as upstream's
    /// mask tests: bit 64 marks a float, bit 128 a sized integer, bit 1 a
    /// signed type, and `DType.int` and `DType.uint` are the unsized
    /// integers.
    fn lower_dtype_predicate(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        recv: Reg,
        method: &str,
    ) -> Result<(), PlironError> {
        if !mojito_ast::ast::DTYPE_PREDICATES.contains(&method) {
            return Err(self.unsupported_reg(format!("DType method `{method}`"), dest));
        }
        let code = self.reg_value(ctx, recv, ScalarTy::Dtype)?;
        let has_bit = |this: &mut Self, ctx: &mut Context, mask: u8| {
            let mask = this.sized_int_constant(ctx, Dtype::UInt8, u64::from(mask));
            let and = AndOp::new(ctx, code, mask);
            this.append(ctx, and.get_operation(), Some(dest));
            let zero = this.sized_int_constant(ctx, Dtype::UInt8, 0);
            let test = ICmpOp::new(ctx, ICmpPredicateAttr::NE, and.get_result(ctx), zero);
            this.append(ctx, test.get_operation(), Some(dest));
            test.get_result(ctx)
        };
        let float = has_bit(self, ctx, mojito_ast::ast::DTYPE_FLOAT_MASK);
        let sized_integer = has_bit(self, ctx, mojito_ast::ast::DTYPE_INTEGER_MASK);
        let signed = has_bit(self, ctx, mojito_ast::ast::DTYPE_SIGNED_MASK);
        let index = self.dtype_constant(ctx, Dtype::Int);
        let is_index = ICmpOp::new(ctx, ICmpPredicateAttr::EQ, code, index);
        self.append(ctx, is_index.get_operation(), Some(dest));
        let unsigned_index = self.dtype_constant(ctx, Dtype::UInt);
        let is_unsigned_index = ICmpOp::new(ctx, ICmpPredicateAttr::EQ, code, unsigned_index);
        self.append(ctx, is_unsigned_index.get_operation(), Some(dest));
        let is_unsigned_index = is_unsigned_index.get_result(ctx);
        let unsized_integer = OrOp::new(ctx, is_index.get_result(ctx), is_unsigned_index);
        self.append(ctx, unsized_integer.get_operation(), Some(dest));
        let integral = OrOp::new(ctx, unsized_integer.get_result(ctx), sized_integer);
        self.append(ctx, integral.get_operation(), Some(dest));
        let integral = integral.get_result(ctx);
        let result = match method {
            "is_floating_point" => float,
            "is_integral" => integral,
            "is_signed" => {
                let signed_integral = AndOp::new(ctx, integral, signed);
                self.append(ctx, signed_integral.get_operation(), Some(dest));
                let or = OrOp::new(ctx, float, signed_integral.get_result(ctx));
                self.append(ctx, or.get_operation(), Some(dest));
                or.get_result(ctx)
            }
            "is_unsigned" => {
                let unsigned = self.bool_constant(ctx, true);
                let unsigned = XorOp::new(ctx, signed, unsigned);
                self.append(ctx, unsigned.get_operation(), Some(dest));
                let and = AndOp::new(ctx, sized_integer, unsigned.get_result(ctx));
                self.append(ctx, and.get_operation(), Some(dest));
                let or = OrOp::new(ctx, is_unsigned_index, and.get_result(ctx));
                self.append(ctx, or.get_operation(), Some(dest));
                or.get_result(ctx)
            }
            "is_numeric" => {
                let or = OrOp::new(ctx, integral, float);
                self.append(ctx, or.get_operation(), Some(dest));
                or.get_result(ctx)
            }
            "is_half_float" => {
                let half = self.dtype_constant(ctx, Dtype::Float16);
                let is_half = ICmpOp::new(ctx, ICmpPredicateAttr::EQ, code, half);
                self.append(ctx, is_half.get_operation(), Some(dest));
                is_half.get_result(ctx)
            }
            // `is_float8`: no Mojito dtype is an eight-bit float.
            _ => self.bool_constant(ctx, false),
        };
        self.reg_values.insert(dest.0, result);
        Ok(())
    }

    /// A fresh allocation holding the `len` bytes at `source`.
    fn owned_copy(
        &mut self,
        ctx: &mut Context,
        source: Value,
        len: Value,
        dest: Reg,
    ) -> (Value, Value) {
        let data = self.emit_alloc(ctx, len, 1, dest);
        self.mem_copy_dynamic(ctx, data, source, len, dest);
        (data, len)
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

    /// The VM-synthesized `Writer.write` dispatch: each argument's display
    /// text feeds one `write_string` call on the aliased `mut self` receiver.
    /// The payload `String` borrows the text bytes (`cap == len`); the callee
    /// reads it and never takes ownership.
    pub(super) fn lower_writer_write(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        write_string: &str,
        args: &[Reg],
        recv_place: Option<&MirPlace>,
    ) -> Result<(), PlironError> {
        let signature = &self.signatures[write_string];
        if signature.outcome.is_some() {
            return Err(self.unsupported_reg(format!("raising `{write_string}`"), dest));
        }
        let payload_ty = self
            .declarations
            .get(write_string)
            .and_then(|decl| decl.param_types.first());
        if !matches!(payload_ty.map(Ty::kind), Some(TyKind::Struct(payload, _))
            if mojito_symbol::symbol::is_stdlib_string_span_struct(payload))
        {
            return Err(self.unsupported_reg(
                format!("`{write_string}` without a StringSpan payload"),
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
            let (data, len) = if let Some(TyKind::Struct(name, _)) =
                (self.func.reg_types.get(&arg.0).cloned())
                    .as_ref()
                    .map(Ty::kind)
                && !mojito_symbol::symbol::is_stdlib_string_struct(name)
            {
                if mojito_symbol::symbol::is_stdlib_string_span_struct(name) {
                    let span = self.reg_ptr(ctx, *arg)?;
                    self.string_parts(ctx, span, dest)
                } else {
                    let descriptor = self.entry_alloca(ctx, 16, 8);
                    self.mem_zero(ctx, descriptor, 16);
                    self.append_struct_via_write_to(ctx, *arg, name, descriptor, dest)?;
                    let parts = self.string_parts(ctx, descriptor, dest);
                    temporary = Some(parts.0);
                    parts
                }
            } else {
                self.writer_argument_text(ctx, *arg, dest)?
            };
            let payload = self.entry_alloca(ctx, 16, 8);
            self.store_string_span_fields(ctx, payload, data, len, dest);
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
        match self.func.reg_types.get(&arg.0).map(Ty::kind) {
            Some(TyKind::Error | TyKind::StringLiteral(_)) => {
                let ptr = self.reg_ptr(ctx, arg)?;
                return Ok(self.string_parts(ctx, ptr, dest));
            }
            Some(TyKind::Struct(name, _))
                if mojito_symbol::symbol::is_stdlib_string_struct(name) =>
            {
                let ptr = self.reg_ptr(ctx, arg)?;
                return Ok(self.string_parts(ctx, ptr, dest));
            }
            Some(TyKind::Struct(name, _))
                if mojito_symbol::symbol::is_stdlib_string_span_struct(name) =>
            {
                let ptr = self.reg_ptr(ctx, arg)?;
                return Ok(self.string_parts(ctx, ptr, dest));
            }
            Some(TyKind::Ref(reference)) => {
                let referent = reference.referent.clone();
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
