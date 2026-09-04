//! `Convert` and variant (`MakeVariant`/`VariantIs`/`Get`/`Take`/`Set`/
//! `Replace`) lowering.

use super::*;

impl<'a> FnLowering<'a> {
    /// `Int(x)` / `UInt(x)` / `Float64(x)` / `Bool(x)` over a scalar operand,
    /// mirroring `runtime::builtin_convert`: float-to-integer saturates (NaN
    /// becomes 0), integer reinterpretations are bit-exact, and `Bool` is a
    /// non-zero test (`fcmp une` — `Bool(NaN)` is `True`).
    pub(super) fn lower_convert(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        name: &str,
        args: &[Reg],
        kwargs: &[(String, Reg)],
    ) -> Result<(), PlironError> {
        if args.len() != 1 || !kwargs.is_empty() {
            return Err(
                self.unsupported_reg(format!("conversion call contract for `{name}`"), dest)
            );
        }
        let arg = args[0];
        let target = match name {
            "Int" => ScalarTy::Int,
            "UInt" => ScalarTy::UInt,
            "Float64" => ScalarTy::Float64,
            _ => ScalarTy::Bool,
        };

        // Literal arguments fold at compile time (the VM's literal branches
        // of `builtin_convert`).
        if let Some(literal) = self.pending_literals.get(&arg.0).cloned() {
            let value = match (&literal, target) {
                (PendingLiteral::Int(_) | PendingLiteral::Float(_), ScalarTy::Bool) => {
                    let non_zero = match &literal {
                        PendingLiteral::Int(literal) => !literal.is_zero(),
                        PendingLiteral::Float(literal) => !literal.is_zero(),
                    };
                    self.bool_constant(ctx, non_zero)
                }
                (PendingLiteral::Float(literal), ScalarTy::Int | ScalarTy::UInt) => {
                    let truncated = PendingLiteral::Int(literal.trunc_to_int());
                    self.materialize_pending(ctx, &truncated, target, dest)?
                }
                _ => self.materialize_pending(ctx, &literal, target, dest)?,
            };
            self.reg_values.insert(dest.0, value);
            return Ok(());
        }

        let source = match self.concrete_scalar_ty(arg)? {
            Some(ty) => ty,
            // A runtime literal-typed value converts at its storage kind
            // (its constant was range-checked when it entered storage).
            None => match self.func.reg_types.get(&arg.0) {
                Some(Ty::FloatLiteral) => ScalarTy::Float64,
                Some(Ty::IntLiteral) => ScalarTy::Int,
                _ => return Err(self.unsupported_reg("untyped conversion operand".into(), dest)),
            },
        };
        let value = self.reg_value(ctx, arg, source)?;
        // A sized operand converts through its mathematical lane value (the
        // VM's `builtin_convert` width-1 arm): integers sign/zero-extend to
        // i64, a `Float32` converts through its f64 view. The normalized
        // kind then takes the ordinary scalar conversion arms.
        let (source, value) = match source {
            ScalarTy::Sized(Dtype::Float32) => {
                (ScalarTy::Float64, self.f32_to_f64(ctx, value, dest))
            }
            ScalarTy::Sized(dtype) => {
                let (_, signed) = mojito_vm::runtime::integer_dtype_bits(dtype)
                    .expect("Float32 is matched above");
                let wide = self.sized_to_i64(ctx, value, dtype, dest);
                (
                    if signed {
                        ScalarTy::Int
                    } else {
                        ScalarTy::UInt
                    },
                    wide,
                )
            }
            other => (other, value),
        };
        match (source, target) {
            // Same-representation moves are pure aliases.
            (ScalarTy::Int | ScalarTy::UInt, ScalarTy::Int | ScalarTy::UInt)
            | (ScalarTy::Float64, ScalarTy::Float64)
            | (ScalarTy::Bool, ScalarTy::Bool) => {
                self.reg_values.insert(dest.0, value);
                Ok(())
            }
            (ScalarTy::Float64, ScalarTy::Int | ScalarTy::UInt) => {
                let intrinsic = if target == ScalarTy::Int {
                    "llvm.fptosi.sat.i64.f64"
                } else {
                    "llvm.fptoui.sat.i64.f64"
                };
                let i64_ty: TypeHandle = IntegerType::get(ctx, 64, Signedness::Signless).into();
                let f64_ty: TypeHandle = FP64Type::get(ctx).into();
                let fn_ty = FuncType::get(ctx, i64_ty, vec![f64_ty], false);
                let call = CallIntrinsicOp::new(
                    ctx,
                    StringAttr::new(intrinsic.to_string()),
                    fn_ty,
                    vec![value],
                );
                self.define(ctx, dest, call.get_operation(), call.get_result(ctx))
            }
            (ScalarTy::Int, ScalarTy::Float64) => {
                let f64_ty: TypeHandle = FP64Type::get(ctx).into();
                let cast = SIToFPOp::new(ctx, value, f64_ty);
                self.define(ctx, dest, cast.get_operation(), cast.get_result(ctx))
            }
            (ScalarTy::UInt, ScalarTy::Float64) => {
                let f64_ty: TypeHandle = FP64Type::get(ctx).into();
                let cast = UIToFPOp::new_with_nneg(ctx, value, f64_ty, false);
                self.define(ctx, dest, cast.get_operation(), cast.get_result(ctx))
            }
            (ScalarTy::Bool, ScalarTy::Int | ScalarTy::UInt) => {
                let i64_ty: TypeHandle = IntegerType::get(ctx, 64, Signedness::Signless).into();
                let cast = ZExtOp::new_with_nneg(ctx, value, i64_ty, false);
                self.define(ctx, dest, cast.get_operation(), cast.get_result(ctx))
            }
            (ScalarTy::Bool, ScalarTy::Float64) => {
                let f64_ty: TypeHandle = FP64Type::get(ctx).into();
                let cast = UIToFPOp::new_with_nneg(ctx, value, f64_ty, false);
                self.define(ctx, dest, cast.get_operation(), cast.get_result(ctx))
            }
            (ScalarTy::Int | ScalarTy::UInt, ScalarTy::Bool) => {
                let zero = self.int_constant(ctx, 0);
                let cmp = ICmpOp::new(ctx, ICmpPredicateAttr::NE, value, zero);
                self.define(ctx, dest, cmp.get_operation(), cmp.get_result(ctx))
            }
            (ScalarTy::Float64, ScalarTy::Bool) => {
                let zero = self.float_constant(ctx, 0.0);
                let cmp = self.fcmp(ctx, FCmpPredicateAttr::UNE, value, zero);
                self.define(ctx, dest, cmp.get_operation(), cmp.get_result(ctx))
            }
            (ScalarTy::Ptr, _) | (_, ScalarTy::Ptr) => {
                Err(self
                    .unsupported_reg(format!("conversion `{name}` over a Pointer operand"), dest))
            }
            (ScalarTy::Sized(_), _) | (_, ScalarTy::Sized(_)) => {
                unreachable!("sized sources normalize above; conversion targets are builtins")
            }
        }
    }

    pub(super) fn lower_make_variant(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        alternatives: &[Ty],
        index: usize,
        value: Reg,
    ) -> Result<(), PlironError> {
        let Some(selected) = alternatives.get(index) else {
            return Err(self.unsupported_reg("Variant construction tag".into(), dest));
        };
        let variant = self
            .layout
            .variant_layout(alternatives)
            .map_err(|error| self.unsupported_reg(format!("Variant layout ({error})"), dest))?;
        let storage = self.entry_alloca(ctx, variant.layout.size, variant.layout.align);
        let tag = self.tag_constant(ctx, index as u32);
        let store = StoreOp::new(ctx, tag, storage);
        self.append(ctx, store.get_operation(), Some(dest));
        let payload = self.offset_address(ctx, storage, variant.payload_offset);
        self.copy_reg_into(ctx, payload, selected, value, dest)?;
        self.reg_values.insert(dest.0, storage);
        if self.needs_drop(selected) {
            self.mark_owned_temp(dest, Ty::Variant(alternatives.to_vec()))?;
        }
        Ok(())
    }

    pub(super) fn copy_reg_into(
        &mut self,
        ctx: &mut Context,
        address: Value,
        ty: &Ty,
        src: Reg,
        anchor: Reg,
    ) -> Result<(), PlironError> {
        match lower_ty(self.name, ty, &self.layout, self.reg_span(anchor))? {
            LowerTy::Scalar(scalar) => {
                let value = if let Some(literal) = self.pending_literals.get(&src.0).cloned() {
                    self.materialize_pending(ctx, &literal, scalar, anchor)?
                } else if !self.func.reg_types.contains_key(&src.0) {
                    self.reg_values.get(&src.0).copied().ok_or_else(|| {
                        self.unsupported_reg(
                            format!("undefined produced register %r{}", src.0),
                            anchor,
                        )
                    })?
                } else {
                    let source = self.concrete_scalar_ty(src)?.unwrap_or(scalar);
                    let value = self.reg_value(ctx, src, source)?;
                    self.convert_lane(ctx, source, scalar, value, anchor)?
                };
                let store = StoreOp::new(ctx, value, address);
                self.append(ctx, store.get_operation(), Some(anchor));
                Ok(())
            }
            LowerTy::Aggregate { layout, .. } => {
                let source = self.reg_ptr(ctx, src)?;
                self.copy_aggregate(ctx, anchor, ty, layout, source)?;
                let copy = self.reg_ptr(ctx, anchor)?;
                self.mem_copy(ctx, address, copy, layout.size, anchor);
                self.owned_temps.remove(&anchor.0);
                Ok(())
            }
            LowerTy::ZeroSized => Ok(()),
        }
    }

    pub(super) fn variant_parts(
        &mut self,
        ctx: &mut Context,
        variant: Reg,
        anchor: Reg,
    ) -> Result<(Value, Vec<Ty>, mojito_native::native::layout::VariantLayout), PlironError> {
        let Some(Ty::Variant(alternatives)) = self.func.reg_types.get(&variant.0).cloned() else {
            return Err(self.unsupported_reg("Variant operand type".into(), anchor));
        };
        let layout = self
            .layout
            .variant_layout(&alternatives)
            .map_err(|error| self.unsupported_reg(format!("Variant layout ({error})"), anchor))?;
        let ptr = self.reg_ptr(ctx, variant)?;
        Ok((ptr, alternatives, layout))
    }

    pub(super) fn lower_variant_is(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        variant: Reg,
        index: usize,
    ) -> Result<(), PlironError> {
        let (ptr, alternatives, _) = self.variant_parts(ctx, variant, dest)?;
        if index >= alternatives.len() {
            return Err(self.unsupported_reg("Variant.isa checked tag".into(), dest));
        }
        let handle: TypeHandle = IntegerType::get(ctx, 32, Signedness::Signless).into();
        let tag = LoadOp::new(ctx, ptr, handle);
        self.append(ctx, tag.get_operation(), Some(dest));
        let expected = self.tag_constant(ctx, index as u32);
        let equal = ICmpOp::new(ctx, ICmpPredicateAttr::EQ, tag.get_result(ctx), expected);
        self.define(ctx, dest, equal.get_operation(), equal.get_result(ctx))
    }

    pub(super) fn lower_variant_get(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        variant: Reg,
        index: usize,
        checked: bool,
    ) -> Result<(), PlironError> {
        let (ptr, alternatives, layout) = self.variant_parts(ctx, variant, dest)?;
        let Some(selected) = alternatives.get(index) else {
            return Err(self.unsupported_reg("Variant projection checked tag".into(), dest));
        };
        if checked {
            self.emit_variant_tag_guard(ctx, ptr, index, dest)?;
        }
        let payload = self.offset_address(ctx, ptr, layout.payload_offset);
        self.load_from(ctx, payload, selected, dest)?;
        if self.needs_drop(selected) {
            self.mark_owned_temp(dest, selected.clone())?;
        }
        Ok(())
    }

    pub(super) fn lower_variant_take(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        variant: Reg,
        index: usize,
        checked: bool,
    ) -> Result<(), PlironError> {
        let (ptr, alternatives, layout) = self.variant_parts(ctx, variant, dest)?;
        let Some(selected) = alternatives.get(index).cloned() else {
            return Err(self.unsupported_reg("Variant.take checked tag".into(), dest));
        };
        if checked {
            self.emit_variant_tag_guard(ctx, ptr, index, dest)?;
        }
        let payload = self.offset_address(ctx, ptr, layout.payload_offset);
        match lower_ty(self.name, &selected, &self.layout, self.reg_span(dest))? {
            LowerTy::Scalar(scalar) => {
                let handle = scalar.handle(ctx);
                let load = LoadOp::new(ctx, payload, handle);
                self.define(ctx, dest, load.get_operation(), load.get_result(ctx))?;
            }
            LowerTy::Aggregate { layout, .. } => {
                let storage = self.entry_alloca(ctx, layout.size, layout.align);
                self.mem_copy(ctx, storage, payload, layout.size, dest);
                self.reg_values.insert(dest.0, storage);
                if self.needs_drop(&selected) {
                    self.mark_owned_temp(dest, selected)?;
                }
            }
            LowerTy::ZeroSized => {
                self.erased.insert(dest.0);
            }
        }
        // MIR moved the receiver place before `VariantTake`; ownership of the
        // payload, not a clone, is now in `dest`.
        self.owned_temps.remove(&variant.0);
        Ok(())
    }

    pub(super) fn emit_variant_tag_guard(
        &mut self,
        ctx: &mut Context,
        ptr: Value,
        index: usize,
        anchor: Reg,
    ) -> Result<(), PlironError> {
        let handle: TypeHandle = IntegerType::get(ctx, 32, Signedness::Signless).into();
        let tag = LoadOp::new(ctx, ptr, handle);
        self.append(ctx, tag.get_operation(), Some(anchor));
        let expected = self.tag_constant(ctx, index as u32);
        let mismatch = ICmpOp::new(ctx, ICmpPredicateAttr::NE, tag.get_result(ctx), expected);
        self.append(ctx, mismatch.get_operation(), Some(anchor));
        self.emit_trap_guard(
            ctx,
            mismatch.get_result(ctx),
            TrapCategory::UnhandledError,
            anchor,
        )
    }

    pub(super) fn lower_variant_set(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        place: &MirPlace,
        index: usize,
        value: Reg,
    ) -> Result<(), PlironError> {
        let (address, ty) = self.place_address(ctx, place, dest)?;
        let Ty::Variant(alternatives) = ty else {
            return Err(self.unsupported_reg("Variant.set place type".into(), dest));
        };
        let Some(selected) = alternatives.get(index) else {
            return Err(self.unsupported_reg("Variant.set checked tag".into(), dest));
        };
        let layout = self
            .layout
            .variant_layout(&alternatives)
            .map_err(|error| self.unsupported_reg(format!("Variant layout ({error})"), dest))?;
        self.emit_drop_variant_payload(ctx, address, &alternatives, layout.payload_offset)?;
        let tag = self.tag_constant(ctx, index as u32);
        let store = StoreOp::new(ctx, tag, address);
        self.append(ctx, store.get_operation(), Some(dest));
        let payload = self.offset_address(ctx, address, layout.payload_offset);
        self.copy_reg_into(ctx, payload, selected, value, dest)?;
        self.erased.insert(dest.0);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn lower_variant_replace(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        place: &MirPlace,
        input_index: usize,
        output_index: usize,
        value: Reg,
        checked: bool,
    ) -> Result<(), PlironError> {
        let (address, ty) = self.place_address(ctx, place, dest)?;
        let Ty::Variant(alternatives) = ty else {
            return Err(self.unsupported_reg("Variant.replace place type".into(), dest));
        };
        let Some(input) = alternatives.get(input_index).cloned() else {
            return Err(self.unsupported_reg("Variant.replace input tag".into(), dest));
        };
        let Some(output) = alternatives.get(output_index).cloned() else {
            return Err(self.unsupported_reg("Variant.replace output tag".into(), dest));
        };
        if checked {
            self.emit_variant_tag_guard(ctx, address, output_index, dest)?;
        }
        let layout = self
            .layout
            .variant_layout(&alternatives)
            .map_err(|error| self.unsupported_reg(format!("Variant layout ({error})"), dest))?;
        let payload = self.offset_address(ctx, address, layout.payload_offset);
        match lower_ty(self.name, &output, &self.layout, self.reg_span(dest))? {
            LowerTy::Scalar(scalar) => {
                let handle = scalar.handle(ctx);
                let load = LoadOp::new(ctx, payload, handle);
                self.define(ctx, dest, load.get_operation(), load.get_result(ctx))?;
            }
            LowerTy::Aggregate { layout, .. } => {
                let storage = self.entry_alloca(ctx, layout.size, layout.align);
                self.mem_copy(ctx, storage, payload, layout.size, dest);
                self.reg_values.insert(dest.0, storage);
                if self.needs_drop(&output) {
                    self.mark_owned_temp(dest, output)?;
                }
            }
            LowerTy::ZeroSized => {
                self.erased.insert(dest.0);
            }
        }
        let tag = self.tag_constant(ctx, input_index as u32);
        let store = StoreOp::new(ctx, tag, address);
        self.append(ctx, store.get_operation(), Some(dest));
        self.copy_reg_into(ctx, payload, &input, value, Reg(u32::MAX - 3))
    }

    pub(super) fn lower_variant_set_init_with(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        place: &MirPlace,
        index: usize,
        factory: Reg,
    ) -> Result<(), PlironError> {
        let produced = Reg(u32::MAX - 1);
        self.lower_call_indirect(ctx, produced, factory, &[], &[], &[], &[], None)?;
        self.lower_variant_set(ctx, dest, place, index, produced)
    }

    pub(super) fn lower_variant_deinit_with(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        variant: Reg,
        handler: Reg,
        index: usize,
    ) -> Result<(), PlironError> {
        let (ptr, alternatives, layout) = self.variant_parts(ctx, variant, dest)?;
        let Some(selected) = alternatives.get(index).cloned() else {
            return Err(self.unsupported_reg("Variant.deinit_with checked tag".into(), dest));
        };
        self.emit_variant_tag_guard(ctx, ptr, index, dest)?;
        let payload_address = self.offset_address(ctx, ptr, layout.payload_offset);
        let payload = Reg(u32::MAX - 2);
        match lower_ty(self.name, &selected, &self.layout, self.reg_span(dest))? {
            LowerTy::Scalar(scalar) => {
                let handle = scalar.handle(ctx);
                let load = LoadOp::new(ctx, payload_address, handle);
                self.append(ctx, load.get_operation(), Some(dest));
                self.reg_values.insert(payload.0, load.get_result(ctx));
            }
            LowerTy::Aggregate { .. } => {
                self.reg_values.insert(payload.0, payload_address);
            }
            LowerTy::ZeroSized => {
                self.erased.insert(payload.0);
            }
        }
        self.lower_call_indirect(ctx, dest, handler, &[payload], &[], &[None], &[], None)?;
        self.owned_temps.remove(&variant.0);
        self.reg_values.remove(&payload.0);
        self.erased.remove(&payload.0);
        self.erased.insert(dest.0);
        Ok(())
    }

    /// `Variant == Variant` / `!=`: equal when the tags match and the active
    /// alternative's payloads are equal — scalars by compare, nominal
    /// alternatives through their compiled `__eq__` instance. The result is
    /// merged through an `i1` block argument of the continuation.
    pub(super) fn lower_variant_equality(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        a: Reg,
        b: Reg,
        negate: bool,
    ) -> Result<(), PlironError> {
        let (ptr_a, alternatives, layout) = self.variant_parts(ctx, a, dest)?;
        let ptr_b = self.reg_ptr(ctx, b)?;
        let tag_handle: TypeHandle = IntegerType::get(ctx, 32, Signedness::Signless).into();
        let bool_handle = ScalarTy::Bool.handle(ctx);
        let tag_a = LoadOp::new(ctx, ptr_a, tag_handle);
        self.append(ctx, tag_a.get_operation(), Some(dest));
        let tag_b = LoadOp::new(ctx, ptr_b, tag_handle);
        self.append(ctx, tag_b.get_operation(), Some(dest));
        let falsehood = self.bool_constant(ctx, false);
        let same_tag = ICmpOp::new(
            ctx,
            ICmpPredicateAttr::EQ,
            tag_a.get_result(ctx),
            tag_b.get_result(ctx),
        );
        self.append(ctx, same_tag.get_operation(), Some(dest));
        // Every operand-derived value is computed before the first branch:
        // ops appended after a terminator would form a stray tail block.
        let payload_a = self.offset_address(ctx, ptr_a, layout.payload_offset);
        let payload_b = self.offset_address(ctx, ptr_b, layout.payload_offset);
        let region = self.region.expect("Variant equality is inside a function");
        let continuation = BasicBlock::new(ctx, None, vec![bool_handle]);
        continuation.insert_at_back(region, ctx);
        let same_block = BasicBlock::new(ctx, None, vec![]);
        same_block.insert_at_back(region, ctx);
        let branch = CondBrOp::new(
            ctx,
            same_tag.get_result(ctx),
            same_block,
            vec![],
            continuation,
            vec![falsehood],
        );
        self.append(ctx, branch.get_operation(), Some(dest));
        let mut next = same_block;
        for (index, alternative) in alternatives.iter().enumerate() {
            self.current = Some(next);
            let alt_block = BasicBlock::new(ctx, None, vec![]);
            alt_block.insert_at_back(region, ctx);
            let rest = BasicBlock::new(ctx, None, vec![]);
            rest.insert_at_back(region, ctx);
            let expected = self.tag_constant(ctx, index as u32);
            let matches = ICmpOp::new(ctx, ICmpPredicateAttr::EQ, tag_a.get_result(ctx), expected);
            self.append(ctx, matches.get_operation(), Some(dest));
            let branch = CondBrOp::new(
                ctx,
                matches.get_result(ctx),
                alt_block,
                vec![],
                rest,
                vec![],
            );
            self.append(ctx, branch.get_operation(), Some(dest));
            self.current = Some(alt_block);
            let equal = match alternative {
                Ty::Struct(name, _) => {
                    let prefix = if mojito_symbol::symbol::is_stdlib_string_struct(name) {
                        "String.__eq__".to_string()
                    } else {
                        format!("{name}.__eq__")
                    };
                    let target = self.unique_hash_instance(dest, "", &prefix, false)?;
                    let scratch = Reg(u32::MAX - 4);
                    self.emit_bound_call(ctx, scratch, &target, vec![payload_a, payload_b])?;
                    let Some(value) = self.reg_values.remove(&scratch.0) else {
                        return Err(self
                            .unsupported_reg(format!("`{target}` did not produce a Bool"), dest));
                    };
                    value
                }
                scalar => {
                    let LowerTy::Scalar(kind) =
                        lower_ty(self.name, scalar, &self.layout, self.reg_span(dest))?
                    else {
                        return Err(self.unsupported_reg(
                            format!("comparing a `{scalar}` Variant alternative"),
                            dest,
                        ));
                    };
                    let handle = kind.handle(ctx);
                    let left = LoadOp::new(ctx, payload_a, handle);
                    self.append(ctx, left.get_operation(), Some(dest));
                    let right = LoadOp::new(ctx, payload_b, handle);
                    self.append(ctx, right.get_operation(), Some(dest));
                    if matches!(kind, ScalarTy::Float64 | ScalarTy::Sized(Dtype::Float32)) {
                        let compare = FCmpOp::new(
                            ctx,
                            FCmpPredicateAttr::OEQ,
                            left.get_result(ctx),
                            right.get_result(ctx),
                        );
                        self.append(ctx, compare.get_operation(), Some(dest));
                        compare.get_result(ctx)
                    } else {
                        let compare = ICmpOp::new(
                            ctx,
                            ICmpPredicateAttr::EQ,
                            left.get_result(ctx),
                            right.get_result(ctx),
                        );
                        self.append(ctx, compare.get_operation(), Some(dest));
                        compare.get_result(ctx)
                    }
                }
            };
            let jump = BrOp::new(ctx, continuation, vec![equal]);
            self.append(ctx, jump.get_operation(), Some(dest));
            next = rest;
        }
        // No alternative matched the (checked) tag: unreachable, merges false.
        self.current = Some(next);
        let jump = BrOp::new(ctx, continuation, vec![falsehood]);
        self.append(ctx, jump.get_operation(), Some(dest));
        self.current = Some(continuation);
        let merged = continuation.deref(ctx).get_argument(0);
        if negate {
            let truth = self.bool_constant(ctx, true);
            let flipped = XorOp::new(ctx, merged, truth);
            self.define(ctx, dest, flipped.get_operation(), flipped.get_result(ctx))
        } else {
            self.reg_values.insert(dest.0, merged);
            Ok(())
        }
    }

    pub(super) fn emit_drop_variant_payload(
        &mut self,
        ctx: &mut Context,
        ptr: Value,
        alternatives: &[Ty],
        payload_offset: u64,
    ) -> Result<(), PlironError> {
        if !alternatives.iter().any(|ty| self.needs_drop(ty)) {
            return Ok(());
        }
        let tag_handle: TypeHandle = IntegerType::get(ctx, 32, Signedness::Signless).into();
        let tag = LoadOp::new(ctx, ptr, tag_handle);
        self.append(ctx, tag.get_operation(), None);
        let payload = self.offset_address(ctx, ptr, payload_offset);
        let region = self.region.expect("Variant drop is inside a function");
        let continuation = BasicBlock::new(ctx, None, vec![]);
        continuation.insert_at_back(region, ctx);
        let mut next = self.current.expect("Variant drop has a current block");
        for (index, alternative) in alternatives.iter().enumerate() {
            if !self.needs_drop(alternative) {
                continue;
            }
            self.current = Some(next);
            let drop_block = BasicBlock::new(ctx, None, vec![]);
            drop_block.insert_at_back(region, ctx);
            let rest = BasicBlock::new(ctx, None, vec![]);
            rest.insert_at_back(region, ctx);
            let expected = self.tag_constant(ctx, index as u32);
            let matches = ICmpOp::new(ctx, ICmpPredicateAttr::EQ, tag.get_result(ctx), expected);
            self.append(ctx, matches.get_operation(), None);
            let branch = CondBrOp::new(
                ctx,
                matches.get_result(ctx),
                drop_block,
                vec![],
                rest,
                vec![],
            );
            self.append(ctx, branch.get_operation(), None);
            self.current = Some(drop_block);
            self.emit_drop_value(ctx, payload, alternative, false)?;
            let jump = BrOp::new(ctx, continuation, vec![]);
            self.append(ctx, jump.get_operation(), None);
            next = rest;
        }
        self.current = Some(next);
        let jump = BrOp::new(ctx, continuation, vec![]);
        self.append(ctx, jump.get_operation(), None);
        self.current = Some(continuation);
        Ok(())
    }
}
