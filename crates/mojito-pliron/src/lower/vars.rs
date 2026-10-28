//! Variable-slot lowering: `UseVar`/`DefVar`, copies, and raw allocation.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::TyKind;

impl FnLowering<'_> {
    /// `UseVar`: scalars load from their slot; an aggregate copy or move
    /// runs the `constructor` it names, and one naming none copies with the
    /// VM's `clone_value` semantics or transfers the bytes (the VM
    /// tombstones the source, and ownership analysis rejects later uses
    /// statically, so no runtime tombstone is needed). A forwarded move hands
    /// the bytes to an owned parameter without the move initializer.
    pub(super) fn lower_use_var(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        var: u32,
        mode: UseMode,
        constructor: Option<&str>,
    ) -> Result<(), PlironError> {
        if matches!(mode, UseMode::BorrowShared | UseMode::BorrowMut) {
            // A borrow is the address of the variable's storage (the VM's
            // `Value::Ref` handle); ownership already verified the
            // discipline.
            self.reg_values.insert(dest.0, self.var_slots[var as usize]);
            return Ok(());
        }
        match self.var_lower_ty(var)? {
            LowerTy::Scalar(scalar) => {
                let handle = scalar.handle(ctx);
                let load = LoadOp::new(ctx, self.var_slots[var as usize], handle);
                self.define(ctx, dest, load.get_operation(), load.get_result(ctx))
            }
            LowerTy::Aggregate { ty, layout } => {
                let src = self.var_slots[var as usize];
                if matches!(mode, UseMode::Copy) {
                    self.copy_named_aggregate(ctx, dest, &ty, layout, src, constructor)
                } else {
                    // The nominal String's `__moveinit__` is an identity
                    // field move — the byte copy below is exactly it.
                    let stdlib_string = matches!(&ty.kind(), TyKind::Struct(name, _)
                        if mojito_symbol::symbol::is_stdlib_string_struct(name));
                    if let Some(moveinit) = constructor.filter(|_| !stdlib_string) {
                        // A `^` transfer naming a compiled `__moveinit__`
                        // always runs that constructor. Ownership checking
                        // has already established the consuming source
                        // contract; the constructor, not a backend pointer
                        // heuristic, defines the move semantics.
                        if self.signatures.contains_key(moveinit) {
                            return self.move_via_moveinit(ctx, dest, var, moveinit, layout);
                        }
                        return Err(self.unsupported_reg(
                            format!("move of `{ty}` with a user `__moveinit__`"),
                            dest,
                        ));
                    }
                    let storage = self.value_storage(ctx, &ty, layout);
                    self.copy_value(ctx, storage, src, &ty, layout, dest);
                    self.reg_values.insert(dest.0, storage);
                    Ok(())
                }
            }
            LowerTy::ZeroSized => {
                self.erased.insert(dest.0);
                Ok(())
            }
        }
    }

    /// `DefVar`: the VM clones the register into the variable slot — for
    /// aggregates a byte copy of the register's storage.
    pub(super) fn lower_def_var(
        &mut self,
        ctx: &mut Context,
        var: u32,
        src: Reg,
    ) -> Result<(), PlironError> {
        if self.error_slots.contains(&var) {
            return self.stage_error(ctx, var, src);
        }
        if self.func.error_slot == Some(var) {
            return self.stage_function_error(ctx, src);
        }
        if matches!(
            self.func.var_tys.get(&var).map(Ty::kind),
            Some(TyKind::Ref(_))
        ) && let Some(place) = self.made_ref_places.get(&src.0)
        {
            self.reference_places.insert(var, place.clone());
        }
        match self.var_lower_ty(var)? {
            LowerTy::Scalar(expected) => {
                let value = self.literal_slot_value(ctx, var, src, expected)?;
                let store = StoreOp::new(ctx, value, self.var_slots[var as usize]);
                self.append(ctx, store.get_operation(), None);
                Ok(())
            }
            LowerTy::Aggregate { ty, layout } => {
                let ptr = self.reg_ptr(ctx, src)?;
                let slot = self.var_slots[var as usize];
                self.copy_value(ctx, slot, ptr, &ty, layout, src);
                // The variable owns the value now.
                Ok(())
            }
            LowerTy::ZeroSized => Ok(()),
        }
    }

    /// The SSA value of `src` for a store into variable `var`'s scalar slot:
    /// a pending literal entering `IntLiteral`/`FloatLiteral`-typed storage
    /// converts exactly (rejecting what the storage cannot hold) instead of
    /// wrapping at the consumer's kind.
    pub(super) fn literal_slot_value(
        &mut self,
        ctx: &mut Context,
        var: u32,
        src: Reg,
        expected: ScalarTy,
    ) -> Result<Value, PlironError> {
        if let Some(ty) = self.func.var_tys.get(&var).cloned()
            && matches!(ty.kind(), TyKind::IntLiteral | TyKind::FloatLiteral)
            && let Some(literal) = self.pending_literals.get(&src.0).cloned()
        {
            let constant = self.exact_literal_storage(ctx, &literal, &ty, src)?;
            self.cache_lazy(src, constant);
            return Ok(constant);
        }
        self.reg_value(ctx, src, expected)
    }

    /// `CopyValue` — materialize an owned copy of a register: scalars and
    /// compile-time literals alias (their SSA values are already owned);
    /// an aggregate runs the `constructor` the copy names, else the VM's
    /// `clone_value` copy.
    pub(super) fn lower_copy_value(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        value: Reg,
        constructor: Option<&str>,
    ) -> Result<(), PlironError> {
        if self.pointer_slot_refs.contains(&value.0) {
            self.pointer_slot_refs.insert(dest.0);
        }
        if let Some(literal) = self.pending_literals.get(&value.0).cloned() {
            self.pending_literals.insert(dest.0, literal);
            return Ok(());
        }
        if let Some(bytes) = self.str_consts.get(&value.0).cloned() {
            self.str_consts.insert(dest.0, bytes);
            return Ok(());
        }
        let Some(ty) = self.func.reg_types.get(&value.0).cloned() else {
            return Err(self.unsupported_reg(format!("untyped copy source %r{}", value.0), dest));
        };
        match lower_ty(self.name, &ty, &self.layout, self.reg_span(dest))? {
            LowerTy::Scalar(scalar) => {
                let copied = self.reg_value(ctx, value, scalar)?;
                self.reg_values.insert(dest.0, copied);
                Ok(())
            }
            LowerTy::Aggregate { ty, layout } => {
                let src = self.reg_ptr(ctx, value)?;
                self.copy_named_aggregate(ctx, dest, &ty, layout, src, constructor)
            }
            LowerTy::ZeroSized => {
                self.erased.insert(dest.0);
                Ok(())
            }
        }
    }

    /// `select` — one `select` over two scalar cases, or over the
    /// addresses of two aggregate cases, the chosen one then copied as a
    /// trivially register-passable value is (`runtime::select`).
    pub(super) fn lower_select(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        condition: Reg,
        yes: Reg,
        no: Reg,
    ) -> Result<(), PlironError> {
        let Some(ty) = self.func.reg_types.get(&dest.0).cloned() else {
            return Err(self.unsupported_reg("untyped select result".into(), dest));
        };
        let condition = self.reg_value(ctx, condition, ScalarTy::Bool)?;
        match lower_ty(self.name, &ty, &self.layout, self.reg_span(dest))? {
            LowerTy::Scalar(scalar) => {
                let yes = self.reg_value(ctx, yes, scalar)?;
                let no = self.reg_value(ctx, no, scalar)?;
                let select = SelectOp::new(ctx, condition, yes, no);
                self.define(ctx, dest, select.get_operation(), select.get_result(ctx))
            }
            LowerTy::Aggregate { ty, layout } => {
                let yes = self.reg_ptr(ctx, yes)?;
                let no = self.reg_ptr(ctx, no)?;
                let select = SelectOp::new(ctx, condition, yes, no);
                self.append(ctx, select.get_operation(), Some(dest));
                self.copy_aggregate(ctx, dest, &ty, layout, select.get_result(ctx))
            }
            LowerTy::ZeroSized => {
                self.erased.insert(dest.0);
                Ok(())
            }
        }
    }

    /// The intercepted `std.memory` allocation entry point,
    /// `unsafe_alloc[T](count, *, alignment = 0)`: `mjrt_alloc(count *
    /// sizeof(T), align)` with the element type taken from the call site's
    /// concrete `Pointer[T]` destination. An excessive count traps with the
    /// allocation-failure category (a recorded divergence — the VM raises a
    /// `TypeError` for a negative count).
    pub(super) fn lower_unsafe_alloc(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        args: &[Reg],
        kwargs: &[(String, Reg)],
    ) -> Result<(), PlironError> {
        if args.len() != 1 {
            return Err(self.unsupported_reg("allocation call contract".into(), dest));
        }
        let alignment = match kwargs {
            [] => None,
            [(name, reg)] if name == "alignment" => Some(*reg),
            _ => {
                return Err(self.unsupported_reg("allocation call contract".into(), dest));
            }
        };
        self.lower_alloc_core(ctx, dest, args[0], alignment)
    }

    /// The shared allocation core behind `unsafe_alloc` and the
    /// `UnsafePointer.alloc`/`alloc_aligned` builtins: `mjrt_alloc` of
    /// `count * sizeof(element)` bytes at the element's natural alignment
    /// (or the requested one; `0` selects natural).
    pub(super) fn lower_alloc_core(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        count: Reg,
        alignment: Option<Reg>,
    ) -> Result<(), PlironError> {
        let register_ty = self.func.reg_types.get(&dest.0).cloned();
        let Some(TyKind::Pointer { element, .. }) = register_ty.as_ref().map(Ty::kind) else {
            return Err(
                self.unsupported_reg("allocation without a concrete pointer result".into(), dest)
            );
        };
        let element_layout = self.layout.layout_of(element).map_err(|error| {
            self.unsupported_reg(format!("allocation element layout ({error})"), dest)
        })?;
        let count = self.reg_value(ctx, count, ScalarTy::Int)?;
        // Guard the byte-size multiplication: any count above the safe bound
        // (negative counts arrive as huge unsigned values) traps.
        let element_size = element_layout.size.max(1);
        let limit = self.uint_constant(ctx, u64::MAX / element_size);
        let excessive = ICmpOp::new(ctx, ICmpPredicateAttr::UGT, count, limit);
        self.append(ctx, excessive.get_operation(), Some(dest));
        self.emit_trap_guard(
            ctx,
            excessive.get_result(ctx),
            TrapCategory::AllocFailure,
            dest,
        )?;
        let size_const = self.uint_constant(ctx, element_layout.size);
        let bytes = MulOp::new_with_overflow_flag(ctx, count, size_const, no_overflow_flags());
        self.append(ctx, bytes.get_operation(), Some(dest));
        let natural_align = self.uint_constant(ctx, element_layout.align);
        let align = match alignment {
            None => natural_align,
            Some(reg) => {
                // `alignment = 0` selects the element's natural alignment.
                let requested = self.reg_value(ctx, reg, ScalarTy::Int)?;
                let zero = self.int_constant(ctx, 0);
                let is_zero = ICmpOp::new(ctx, ICmpPredicateAttr::EQ, requested, zero);
                self.append(ctx, is_zero.get_operation(), Some(dest));
                let select = SelectOp::new(ctx, is_zero.get_result(ctx), natural_align, requested);
                self.append(ctx, select.get_operation(), Some(dest));
                select.get_result(ctx)
            }
        };
        let alloc_ty = self.shared.ensure_rt(ctx, "mjrt_alloc");
        let call = CallOp::new(
            ctx,
            CallOpCallable::Direct("mjrt_alloc".try_into().expect("valid identifier")),
            alloc_ty,
            vec![bytes.get_result(ctx), align],
        );
        self.define(ctx, dest, call.get_operation(), call.get_result(ctx))
    }
}
