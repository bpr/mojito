//! Variable-slot lowering: loans, `UseVar`/`DefVar`, copies, and raw
//! allocation.

use super::*;

impl<'a> FnLowering<'a> {
    /// Materialize the runtime handle of a static `ref` binding. MIR normally
    /// erases loan bookkeeping at execution, but a binding whose only
    /// definition is `EstablishLoans` still needs its verified target address
    /// in native storage. Reference-result variables already arrive through
    /// `DefVar` and retain that more precise (possibly interior) handle.
    pub(super) fn lower_establish_loans(
        &mut self,
        ctx: &mut Context,
        reference: u32,
        loans: &[crate::mir::MirLoan],
        marker: Reg,
    ) -> Result<(), PlironError> {
        if !self.initialized_vars.contains(&reference)
            && matches!(self.func.var_tys.get(&reference), Some(Ty::Ref(_)))
        {
            let [loan] = loans else {
                return Err(self.unsupported_reg(
                    "reference binding with non-singleton runtime target".into(),
                    marker,
                ));
            };
            let place = loan.place.clone();
            let address = self.place_address(ctx, &place, marker)?.0;
            let store = StoreOp::new(ctx, address, self.var_slots[reference as usize]);
            self.append(ctx, store.get_operation(), Some(marker));
            self.initialized_vars.insert(reference);
        }
        self.erased.insert(marker.0);
        Ok(())
    }

    /// `UseVar`: scalars load from their slot; aggregate copies run the VM's
    /// `clone_value` semantics and aggregate moves transfer the bytes (the VM
    /// tombstones the source, and ownership analysis rejects later uses
    /// statically, so no runtime tombstone is needed).
    pub(super) fn lower_use_var(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        var: u32,
        mode: UseMode,
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
                    self.copy_aggregate(ctx, dest, &ty, layout, src)
                } else {
                    // The nominal String's `__moveinit__` is an identity
                    // field move — the byte copy below is exactly it.
                    let stdlib_string = matches!(ty.as_ref(), Ty::Struct(name, _)
                        if crate::symbol::is_stdlib_string_struct(name));
                    if !stdlib_string && self.has_lifecycle_method(&ty, "__moveinit__") {
                        // A `^` transfer with a compiled `__moveinit__` always
                        // runs that constructor. Ownership checking has
                        // already established the consuming source contract;
                        // the constructor, not a backend pointer heuristic,
                        // defines the move semantics.
                        if let Ty::Struct(name, _) = ty.as_ref()
                            && self
                                .signatures
                                .contains_key(&format!("{name}.__moveinit__"))
                        {
                            let name = name.clone();
                            return self.move_via_moveinit(ctx, dest, var, &name, layout);
                        }
                        return Err(self.unsupported_reg(
                            format!("move of `{ty}` with a user `__moveinit__`"),
                            dest,
                        ));
                    }
                    let storage = self.entry_alloca(ctx, layout.size, layout.align);
                    self.mem_copy(ctx, storage, src, layout.size, dest);
                    self.reg_values.insert(dest.0, storage);
                    // The move vacates the slot (the VM tombstones it); a
                    // later cleanup-edge drop must find it empty. The moved
                    // value is an owned temporary until consumed.
                    self.set_drop_flag(ctx, var, false);
                    if self.owns_heap(&ty) || self.stdlib_deinit_temp(&ty) || self.needs_drop(&ty) {
                        self.mark_owned_temp(dest, (*ty).clone())?;
                    }
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
        self.initialized_vars.insert(var);
        match self.var_lower_ty(var)? {
            LowerTy::Scalar(expected) => {
                let value = self.literal_slot_value(ctx, var, src, expected)?;
                let store = StoreOp::new(ctx, value, self.var_slots[var as usize]);
                self.append(ctx, store.get_operation(), None);
                Ok(())
            }
            LowerTy::Aggregate { layout, .. } => {
                let ptr = self.reg_ptr(ctx, src)?;
                let slot = self.var_slots[var as usize];
                self.mem_copy(ctx, slot, ptr, layout.size, src);
                // The variable owns the value now; the temporary transfers.
                self.owned_temps.remove(&src.0);
                if let Some(condition) = self.conditional_values.get(&src.0).copied() {
                    self.set_drop_flag_value(ctx, var, condition);
                } else {
                    self.set_drop_flag(ctx, var, true);
                }
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
        if let Some(ty @ (Ty::IntLiteral | Ty::FloatLiteral)) = self.func.var_tys.get(&var).cloned()
            && let Some(literal) = self.pending_literals.get(&src.0).cloned()
        {
            let constant = self.exact_literal_storage(ctx, &literal, &ty, src)?;
            self.reg_values.insert(src.0, constant);
            return Ok(constant);
        }
        self.reg_value(ctx, src, expected)
    }

    /// `CopyValue` — materialize an owned copy of a register: scalars and
    /// compile-time literals alias (their SSA values are already owned);
    /// aggregates run the VM's `clone_value` copy.
    pub(super) fn lower_copy_value(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        value: Reg,
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
                self.copy_aggregate(ctx, dest, &ty, layout, src)
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
        let Some(Ty::Pointer { element, .. }) = self.func.reg_types.get(&dest.0).cloned() else {
            return Err(
                self.unsupported_reg("allocation without a concrete pointer result".into(), dest)
            );
        };
        let element_layout = self.layout.layout_of(&element).map_err(|error| {
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
