//! Closure lowering: `MakeClosure` and indirect calls through contract
//! slots.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::TyKind;

impl FnLowering<'_> {
    /// Build the two-word `{ invoke, env }` value of a retained callable:
    /// intern the target's `invoke` thunk and store its address next to the
    /// environment pointer. Bare function values and empty-capture closures
    /// carry a null environment.
    pub(super) fn lower_make_closure(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        function: &str,
        captures: &[MirClosureCapture],
    ) -> Result<(), PlironError> {
        let Some(contract) = self.func.reg_types.get(&dest.0).cloned() else {
            return Err(self.unsupported_reg("untyped closure result".into(), dest));
        };
        let abi = self.contract_abi(ctx, &contract, dest)?;
        let signatures = self.signatures;
        let Some(target) = signatures.get(function) else {
            return Err(self.unsupported_reg(format!("closure over uncompiled `{function}`"), dest));
        };
        self.check_contract_target(&abi, target, captures.len(), function, dest)?;
        // The environment record: one slot per capture, each storing the
        // captured place's address.
        let env = if captures.is_empty() {
            let ptr_ty: TypeHandle = PointerType::get(ctx, 0).into();
            let null = ZeroOp::new(ctx, ptr_ty);
            self.append(ctx, null.get_operation(), Some(dest));
            null.get_result(ctx)
        } else {
            let record = self.entry_alloca(ctx, 8 * captures.len() as u64, 8);
            for (index, capture) in captures.iter().enumerate() {
                let slot = if index == 0 {
                    record
                } else {
                    self.gep_byte(ctx, record, 8 * index as u64, dest)
                };
                let (source, _) = self.place_address(ctx, &capture.place, dest)?;
                let store = StoreOp::new(ctx, source, slot);
                self.append(ctx, store.get_operation(), Some(dest));
            }
            record
        };
        let thunk = self.shared.ensure_thunk(ctx, target, captures.len());
        let storage = self.entry_alloca(ctx, 16, 8);
        let invoke = AddressOfOp::new(ctx, thunk, 0);
        self.append(ctx, invoke.get_operation(), Some(dest));
        let store_invoke = StoreOp::new(ctx, invoke.get_result(ctx), storage);
        self.append(ctx, store_invoke.get_operation(), Some(dest));
        let env_address = self.gep_byte(ctx, storage, 8, dest);
        let store_env = StoreOp::new(ctx, env, env_address);
        self.append(ctx, store_env.get_operation(), Some(dest));
        self.reg_values.insert(dest.0, storage);
        Ok(())
    }

    /// Call through a retained callable value: bind the arguments against
    /// the callee register's checked contract, load `{ invoke, env }`, and
    /// call `invoke` indirectly with the environment pointer prepended
    /// (after the outcome/sret out-pointer, when one exists).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn lower_call_indirect(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        callee: Reg,
        args: &[Reg],
        kwargs: &[(String, Reg)],
        arg_places: &[Option<MirPlace>],
        kwarg_places: &[Option<MirPlace>],
        instantiated_contract: Option<&Ty>,
    ) -> Result<(), PlironError> {
        let contract = instantiated_contract
            .cloned()
            .or_else(|| self.func.reg_types.get(&callee.0).cloned());
        let Some(mut contract) = contract else {
            return Err(self.unsupported_reg("untyped indirect callee".into(), dest));
        };
        while let TyKind::Ref(reference) = contract.kind() {
            contract = reference.referent.clone();
        }
        // Monomorphization devirtualizes a call through a nominal callable
        // into a direct `__call__` method call.
        if let Some((name, _)) = contract.kind().as_non_simd_struct() {
            return Err(self.unsupported_reg(
                format!("indirect call through nominal callable `{name}`"),
                dest,
            ));
        }
        let abi = self.contract_abi(ctx, &contract, dest)?;
        let bound =
            self.bind_contract_slots(ctx, dest, &abi, args, kwargs, arg_places, kwarg_places)?;
        let base = self.reg_ptr(ctx, callee)?;
        let ptr_ty: TypeHandle = PointerType::get(ctx, 0).into();
        let invoke = LoadOp::new(ctx, base, ptr_ty);
        self.append(ctx, invoke.get_operation(), Some(dest));
        let env_address = self.gep_byte(ctx, base, 8, dest);
        let env = LoadOp::new(ctx, env_address, ptr_ty);
        self.append(ctx, env.get_operation(), Some(dest));
        let mut operands = Vec::with_capacity(bound.len() + 1);
        operands.push(env.get_result(ctx));
        operands.extend(bound);
        self.emit_call_shaped(
            ctx,
            dest,
            CallOpCallable::Indirect(invoke.get_result(ctx)),
            abi.func_ty,
            abi.returns_value,
            abi.sret,
            abi.outcome.clone(),
            operands,
        )
    }

    /// Resolve an indirect call's arguments into the contract's positional
    /// parameter order via `call::match_call_slots` — the same structural
    /// binding as `bind_call_slots`, off the `TyKind::Func` contract instead of
    /// a compiled declaration. Defaults reject: the VM binds an omitted
    /// argument from the runtime callee's declaration, which the native
    /// caller cannot see behind the thunk.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn bind_contract_slots(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        abi: &ContractAbi,
        args: &[Reg],
        kwargs: &[(String, Reg)],
        arg_places: &[Option<MirPlace>],
        kwarg_places: &[Option<MirPlace>],
    ) -> Result<Vec<Value>, PlironError> {
        let kw_names: Vec<&str> = kwargs.iter().map(|(name, _)| name.as_str()).collect();
        let matched = match_call_slots(
            &abi.names,
            &abi.required,
            abi.positional_only,
            abi.keyword_only,
            args.len(),
            &kw_names,
            CallVariadics {
                positional: false,
                keyword: abi.kw_pack_index.is_some(),
            },
        )
        .map_err(|error| {
            self.unsupported_reg(format!("indirect-call binding failed: {error:?}"), dest)
        })?;
        if matched.slots.len() + usize::from(abi.kw_pack_index.is_some()) != abi.params.len() {
            return Err(self.unsupported_reg(
                "indirect-call binding disagrees with the contract arity".into(),
                dest,
            ));
        }
        let mut lowered = Vec::with_capacity(abi.params.len());
        let mut slots = matched.slots.iter();
        for (index, expected) in abi.params.iter().enumerate() {
            if abi.kw_pack_index == Some(index) {
                let Some(passed) =
                    super::calls::keyword_collector_operand(&matched.keyword_overflow, kwargs)
                else {
                    return Err(self.unsupported_reg(
                        "an indirect call passes no keyword collector".into(),
                        dest,
                    ));
                };
                let owned = abi.owned_params.get(index).copied().unwrap_or(true);
                lowered.push(self.arg_value(ctx, kwargs[passed].1, expected, owned, dest)?);
                continue;
            }
            let slot = slots.next().expect("matched named indirect slot");
            if matches!(expected, LowerTy::ZeroSized) {
                continue;
            }
            let expected = expected.clone();
            let owned = abi.owned_params.get(index).copied().unwrap_or(false);
            let by_ref = abi.ref_params.get(index).copied().unwrap_or(false);
            let place_address = |lowering: &mut Self,
                                 ctx: &mut Context,
                                 place: Option<&MirPlace>|
             -> Result<Value, PlironError> {
                let Some(place) = place.cloned() else {
                    return Err(lowering.unsupported_reg(
                        "`mut`/`ref` indirect argument without a place".into(),
                        dest,
                    ));
                };
                Ok(lowering.place_address(ctx, &place, dest)?.0)
            };
            let value = match slot {
                ArgSlot::Positional(p) if by_ref => {
                    place_address(self, ctx, arg_places.get(*p).and_then(Option::as_ref))?
                }
                ArgSlot::Keyword(k) if by_ref => {
                    place_address(self, ctx, kwarg_places.get(*k).and_then(Option::as_ref))?
                }
                ArgSlot::Positional(p) => self.arg_value(ctx, args[*p], &expected, owned, dest)?,
                ArgSlot::Keyword(k) => self.arg_value(ctx, kwargs[*k].1, &expected, owned, dest)?,
                ArgSlot::Default => {
                    return Err(self.unsupported_reg(
                        "defaulted argument at an indirect call site".into(),
                        dest,
                    ));
                }
            };
            lowered.push(value);
        }
        Ok(lowered)
    }

    /// Materialize a constant default at the parameter's scalar type, exactly
    /// as the VM's default binding materializes the literal.
    pub(super) fn checked_const_value(
        &mut self,
        ctx: &mut Context,
        value: &CheckedConst,
        expected: ScalarTy,
        dest: Reg,
    ) -> Result<Value, PlironError> {
        match (value, expected) {
            (CheckedConst::Int(literal), _) => {
                let literal = PendingLiteral::Int(literal.clone());
                self.materialize_pending(ctx, &literal, expected, dest)
            }
            (CheckedConst::Float(literal), _) => {
                let literal = PendingLiteral::Float(literal.clone());
                self.materialize_pending(ctx, &literal, expected, dest)
            }
            (CheckedConst::Bool(value), ScalarTy::Bool) => Ok(self.bool_constant(ctx, *value)),
            (CheckedConst::Bool(_), other) => Err(self.unsupported_reg(
                format!("Bool default argument for a `{}` parameter", other.name()),
                dest,
            )),
            (CheckedConst::Dtype(dtype), ScalarTy::Dtype) => Ok(self.dtype_constant(ctx, *dtype)),
            (CheckedConst::Dtype(_), other) => Err(self.unsupported_reg(
                format!("DType default argument for a `{}` parameter", other.name()),
                dest,
            )),
            (CheckedConst::String(_) | CheckedConst::None, _) => {
                Err(self.unsupported_reg("non-scalar default argument".into(), dest))
            }
            // A converting-constructor default (e.g. `Optional[T] = None`) runs
            // the constructor to build a heap-backed aggregate — the VM oracle
            // supports it, native default-fill does not yet.
            (CheckedConst::Construct { .. }, _) => Err(self.unsupported_reg(
                "converting-constructor default argument is not yet lowered natively".into(),
                dest,
            )),
            (CheckedConst::Evaluate { .. }, _) => Err(self.unsupported_reg(
                "evaluated default argument where only a constant default is lowered".into(),
                dest,
            )),
        }
    }
}
