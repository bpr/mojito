//! Direct call lowering: slot binding and contract ABI checks.

use super::*;

impl<'a> FnLowering<'a> {
    /// Lower a direct call: builtin scalar conversions intercept by name
    /// exactly like the VM; everything else binds against the compiled
    /// signature, resolving keywords and constant defaults through the shared
    /// call-slot matcher.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn lower_call(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        name: &str,
        args: &[Reg],
        kwargs: &[(String, Reg)],
        arg_places: &[Option<MirPlace>],
        kwarg_places: &[Option<MirPlace>],
    ) -> Result<(), PlironError> {
        if intercepted_call(name) {
            return self.lower_unsafe_alloc(ctx, dest, args, kwargs);
        }
        // Literal→String conversion arrives as the nominal literal/copy
        // constructor overload symbol, whose declared body is a
        // never-execute field-contract stub: route it to the native
        // constructor bridge exactly like the type-name call shape, ahead
        // of the compiled-signature dispatch that would run the stub.
        if crate::symbol::string_ctor_overload_struct(name).is_some() {
            return self.lower_string_ctor(ctx, dest, args, kwargs);
        }
        if !self.signatures.contains_key(name) {
            if matches!(name, "Int" | "UInt" | "Float64" | "Bool") {
                return self.lower_convert(ctx, dest, name, args, kwargs);
            }
            if name == "print" {
                if !kwargs.is_empty() {
                    return Err(
                        self.unsupported_reg("print call with keyword arguments".into(), dest)
                    );
                }
                return self.lower_print(ctx, dest, args);
            }
            if name == "String" {
                return self.lower_string_builtin(ctx, dest, args, kwargs);
            }
            if name == "Error" {
                return self.lower_error_builtin(ctx, dest, args, kwargs);
            }
            if self.struct_decls.contains_key(name) {
                return self.lower_constructor(
                    ctx,
                    dest,
                    name,
                    args,
                    kwargs,
                    arg_places,
                    kwarg_places,
                );
            }
            // The numeric/IO builtins the VM's `call_named` implements
            // directly. Nominal-receiver `len`/`abs`/`round` were rewritten
            // to `__len__`/`__abs__`/`__round__` method calls during
            // monomorphization; only the scalar/pack forms arrive here.
            match name {
                "len" => return self.lower_len_builtin(ctx, dest, args, kwargs),
                "abs" => return self.lower_abs_builtin(ctx, dest, args, kwargs),
                "min" | "max" => {
                    return self.lower_min_max_builtin(ctx, dest, name == "min", args, kwargs);
                }
                "round" => return self.lower_round_builtin(ctx, dest, args, kwargs),
                "divmod" => return self.lower_divmod_builtin(ctx, dest, args, kwargs),
                "repr" if args.len() == 1 && kwargs.is_empty() => {
                    return self.lower_repr_builtin(ctx, dest, args[0]);
                }
                "input" => return self.lower_input_builtin(ctx, dest, args, kwargs),
                "UnsafePointer.alloc" if kwargs.is_empty() && args.len() == 1 => {
                    return self.lower_alloc_core(ctx, dest, args[0], None);
                }
                "UnsafePointer.alloc_aligned" => {
                    let alignment = match (args, kwargs) {
                        ([_, alignment], []) => *alignment,
                        ([_], [(name, alignment)]) if name == "alignment" => *alignment,
                        _ => {
                            return Err(
                                self.unsupported_reg("allocation call contract".into(), dest)
                            );
                        }
                    };
                    return self.lower_alloc_core(ctx, dest, args[0], Some(alignment));
                }
                "UnsafePointer.unsafe_dangling" | "Pointer.unsafe_dangling"
                    if args.is_empty() && kwargs.is_empty() =>
                {
                    return self.lower_dangling_builtin(ctx, dest);
                }
                "_mojito_abort" if args.len() == 1 && kwargs.is_empty() => {
                    return self.lower_abort_builtin(ctx, dest, args[0]);
                }
                _ => {}
            }
            return Err(self.unsupported_reg(
                format!("call to unknown or builtin function `{name}`"),
                dest,
            ));
        }

        let params = self.signatures[name].params.clone();
        let owned = self.signatures[name].owned_params.clone();
        let by_reference = self.signatures[name].ref_params.clone();
        // A direct call to a compiled `__init__` (the checker's specialized
        // constructor symbols and their mono instances) binds its destination
        // as the `out self` receiver: allocate the result storage and bind
        // the remaining arguments past the receiver — the struct-name
        // constructor path's exact contract.
        if name.contains(".__init__")
            && !params.is_empty()
            && let Some(struct_ty @ Ty::Struct(..)) = self.func.reg_types.get(&dest.0).cloned()
        {
            let lowered = lower_ty(self.name, &struct_ty, &self.layout, self.reg_span(dest))?;
            let LowerTy::Aggregate { layout, .. } = lowered else {
                return Err(self.unsupported_reg(format!("constructor result `{struct_ty}`"), dest));
            };
            let storage = self.entry_alloca(ctx, layout.size, layout.align);
            let rest = &params[1..];
            let rest_owned = if owned.len() > 1 { &owned[1..] } else { &[] };
            let rest_by_reference = if by_reference.len() > 1 {
                &by_reference[1..]
            } else {
                &[]
            };
            let mut lowered = vec![storage];
            // A variadic callee always binds through the slot matcher: an
            // argument count that happens to equal the physical parameter
            // count (arity one against the single pack slot) must still
            // build pack storage, never pass the argument as the pack.
            if kwargs.is_empty()
                && args.len() == rest.len()
                && !rest_by_reference.iter().any(|&by_ref| by_ref)
                && !self.variadic_callee(name)
            {
                for (i, (arg, expected)) in args.iter().zip(rest).enumerate() {
                    let owned = rest_owned.get(i).copied().unwrap_or(false);
                    lowered.push(self.place_backed_arg_value(
                        ctx,
                        *arg,
                        expected,
                        owned,
                        arg_places.get(i).and_then(Option::as_ref),
                        dest,
                    )?);
                }
            } else {
                lowered.extend(self.bind_call_slots(
                    ctx,
                    dest,
                    name,
                    rest,
                    rest_owned,
                    rest_by_reference,
                    args,
                    kwargs,
                    arg_places,
                    kwarg_places,
                )?);
            }
            self.emit_bound_call(ctx, dest, name, lowered)?;
            // `__init__` returns nothing; the constructed value is the
            // storage its `out self` wrote through.
            self.erased.remove(&dest.0);
            self.reg_values.insert(dest.0, storage);
            return Ok(());
        }
        let lowered_args =
            if kwargs.is_empty() && args.len() == params.len() && !self.variadic_callee(name) {
                let mut lowered = Vec::with_capacity(args.len());
                for (i, (arg, expected)) in args.iter().zip(&params).enumerate() {
                    let owned = owned.get(i).copied().unwrap_or(false);
                    let value = if by_reference.get(i).copied().unwrap_or(false) {
                        // A `mut`/`ref` argument passes the address of the
                        // caller's designated storage (write-through).
                        let Some(place) = arg_places.get(i).and_then(Option::as_ref) else {
                            return Err(self.unsupported_reg(
                                format!("`mut`/`ref` argument of `{name}` without a place"),
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
                lowered
            } else {
                self.bind_call_slots(
                    ctx,
                    dest,
                    name,
                    &params,
                    &owned,
                    &by_reference,
                    args,
                    kwargs,
                    arg_places,
                    kwarg_places,
                )?
            };
        self.emit_bound_call(ctx, dest, name, lowered_args)
    }

    /// Resolve keyword arguments and constant defaults into the callee's
    /// positional parameter order via `call::match_call_slots` — the same
    /// structural binding the VM applies (`src/call.rs` owns the policy).
    /// `params`, `owned`, and `by_reference` are the expected slices of value
    /// parameters: a method or constructor caller passes its signature minus
    /// the receiver. A `mut`/`ref` slot passes the address of its checked
    /// place, taken from the source array the matched slot names —
    /// `arg_places[p]` for `Positional(p)`, `kwarg_places[k]` for
    /// `Keyword(k)` — never the parameter position.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn bind_call_slots(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        name: &str,
        params: &[LowerTy],
        owned: &[bool],
        by_reference: &[bool],
        args: &[Reg],
        kwargs: &[(String, Reg)],
        arg_places: &[Option<MirPlace>],
        kwarg_places: &[Option<MirPlace>],
    ) -> Result<Vec<Value>, PlironError> {
        let Some(decl) = self.declarations.get(name) else {
            return Err(self.unsupported_reg(
                format!("call to `{name}` without a recorded declaration"),
                dest,
            ));
        };
        let variadic = decl.variadic.clone().map(|ty| (ty, decl.variadic_index));
        let kw_variadic = decl
            .kw_variadic
            .clone()
            .map(|ty| (ty, decl.kw_variadic_index));
        let kw_names: Vec<&str> = kwargs.iter().map(|(n, _)| n.as_str()).collect();
        let matched = match_call_slots(
            &decl.param_names,
            &decl.required,
            decl.positional_only,
            decl.keyword_only,
            args.len(),
            &kw_names,
            CallVariadics {
                positional: variadic.is_some(),
                keyword: kw_variadic.is_some(),
            },
        )
        .map_err(|error| {
            self.unsupported_reg(format!("call binding for `{name}` failed: {error:?}"), dest)
        })?;
        let defaults = decl.defaults.clone();
        // The physical parameter list is the named parameters with the
        // collected pack inserted at `variadic_index` (the VM's `bind_args`
        // packs positional overflow into one tuple-shaped argument).
        let pack = match variadic {
            None => None,
            Some((pack_ty, index)) => {
                let Some(index) = index else {
                    return Err(self.unsupported_reg(
                        format!("variadic call to `{name}` without a recorded pack position"),
                        dest,
                    ));
                };
                let elements = match &pack_ty {
                    Ty::RuntimePack(elements) | Ty::Tuple(elements) => elements.clone(),
                    other => {
                        return Err(self.unsupported_reg(
                            format!("variadic call to `{name}` over unspecialized `{other}`"),
                            dest,
                        ));
                    }
                };
                if matched.positional_overflow.len() != elements.len() {
                    return Err(self.unsupported_reg(
                        format!(
                            "variadic call to `{name}`: {} overflow arguments against a \
                             {}-element pack",
                            matched.positional_overflow.len(),
                            elements.len()
                        ),
                        dest,
                    ));
                }
                let composed = self.struct_layout_of(&elements, dest)?;
                let storage = self.entry_alloca(
                    ctx,
                    composed.layout.size.max(1),
                    composed.layout.align.max(1),
                );
                for ((arg, element), offset) in matched
                    .positional_overflow
                    .iter()
                    .zip(&elements)
                    .zip(&composed.offsets)
                {
                    let address = if *offset == 0 {
                        storage
                    } else {
                        self.gep_byte(ctx, storage, *offset, dest)
                    };
                    // Overflow arguments relocate into the pack (the VM's
                    // `Tuple(*args^)` move); `store_to` transfers owned
                    // temporaries and forks borrowed heap-owners.
                    self.store_to(ctx, address, element, args[*arg])?;
                }
                Some((index, storage))
            }
        };
        let kw_pack = match kw_variadic {
            None => None,
            Some((_element, Some(index))) => {
                let Some(LowerTy::Aggregate { ty, layout }) = params.get(index) else {
                    return Err(self.unsupported_reg(
                        format!("keyword pack of `{name}` lacks aggregate storage"),
                        dest,
                    ));
                };
                let Ty::Struct(struct_name, _) = ty.as_ref() else {
                    return Err(self.unsupported_reg(
                        format!("keyword pack of `{name}` is not a StringDict"),
                        dest,
                    ));
                };
                let Some(struct_decl) = self.struct_decls.get(struct_name.as_str()) else {
                    return Err(self.unsupported_reg(
                        format!("keyword pack of `{name}` lacks a struct declaration"),
                        dest,
                    ));
                };
                let field_types = struct_decl
                    .fields
                    .iter()
                    .map(|(_, ty)| ty.clone())
                    .collect::<Vec<_>>();
                let composed = self.struct_layout_of(&field_types, dest)?;
                let Some(count_index) = struct_decl
                    .fields
                    .iter()
                    .position(|(field, _)| field == "count")
                else {
                    return Err(self.unsupported_reg(
                        format!("keyword pack of `{name}` lacks a count field"),
                        dest,
                    ));
                };
                let storage = self.entry_alloca(ctx, layout.size.max(1), layout.align.max(1));
                self.mem_zero(ctx, storage, layout.size);
                let count_address =
                    self.gep_byte(ctx, storage, composed.offsets[count_index], dest);
                let count = self.int_constant(ctx, matched.keyword_overflow.len() as i64);
                let store = StoreOp::new(ctx, count, count_address);
                self.append(ctx, store.get_operation(), Some(dest));
                Some((index, storage))
            }
            Some((_, None)) => {
                return Err(self.unsupported_reg(
                    format!("keyword-variadic call to `{name}` without a pack position"),
                    dest,
                ));
            }
        };
        let named =
            matched.slots.len() + usize::from(pack.is_some()) + usize::from(kw_pack.is_some());
        if named != params.len() {
            return Err(self.unsupported_reg(
                format!("call binding for `{name}` disagrees with its compiled arity"),
                dest,
            ));
        }
        let mut lowered = Vec::with_capacity(params.len());
        let mut slots = matched.slots.iter().enumerate();
        for (i, param) in params.iter().enumerate() {
            if let Some((pack_index, storage)) = pack
                && i == pack_index
            {
                lowered.push(storage);
                continue;
            }
            if let Some((pack_index, storage)) = kw_pack
                && i == pack_index
            {
                lowered.push(storage);
                continue;
            }
            let Some((slot_index, slot)) = slots.next() else {
                return Err(self.unsupported_reg(
                    format!("call binding for `{name}` disagrees with its compiled arity"),
                    dest,
                ));
            };
            let expected = param.clone();
            // A zero-sized marker parameter (`__list_literal__`) has no
            // physical operand; its slot is consumed and skipped.
            if matches!(expected, LowerTy::ZeroSized) {
                continue;
            }
            let owned = owned.get(i).copied().unwrap_or(false);
            let by_ref = by_reference.get(i).copied().unwrap_or(false);
            let place_address = |lowering: &mut Self,
                                 ctx: &mut Context,
                                 place: Option<&MirPlace>|
             -> Result<Value, PlironError> {
                let Some(place) = place.cloned() else {
                    return Err(lowering.unsupported_reg(
                        format!("`mut`/`ref` argument of `{name}` without a place"),
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
                ArgSlot::Positional(p) => self.place_backed_arg_value(
                    ctx,
                    args[*p],
                    &expected,
                    owned,
                    arg_places.get(*p).and_then(Option::as_ref),
                    dest,
                )?,
                ArgSlot::Keyword(k) => self.place_backed_arg_value(
                    ctx,
                    kwargs[*k].1,
                    &expected,
                    owned,
                    kwarg_places.get(*k).and_then(Option::as_ref),
                    dest,
                )?,
                ArgSlot::Default => {
                    if by_ref {
                        return Err(self.unsupported_reg(
                            format!("defaulted `mut`/`ref` parameter of `{name}`"),
                            dest,
                        ));
                    }
                    let Some(default) = defaults.get(slot_index).and_then(Option::as_ref) else {
                        return Err(self.unsupported_reg(
                            format!("non-constant default argument in call to `{name}`"),
                            dest,
                        ));
                    };
                    let LowerTy::Scalar(scalar) = expected else {
                        return Err(self.unsupported_reg(
                            format!("non-scalar default argument in call to `{name}`"),
                            dest,
                        ));
                    };
                    self.checked_const_value(ctx, default, scalar, dest)?
                }
            };
            lowered.push(value);
        }
        Ok(lowered)
    }

    /// Derive the physical indirect-call ABI from a checked `Ty::Func`
    /// contract, by the same classification rules `declare_function` applies
    /// to a compiled callee (a raising contract returns through a prepended
    /// outcome out-pointer, an aggregate return through prepended sret
    /// storage, never both). Contract shapes the thunk cannot bind reject
    /// contextually.
    pub(super) fn contract_abi(
        &mut self,
        ctx: &mut Context,
        contract: &Ty,
        dest: Reg,
    ) -> Result<ContractAbi, PlironError> {
        let Ty::Func {
            params,
            names,
            ret,
            required,
            variadic,
            kw_variadic,
            positional_only,
            keyword_only,
            raises,
            conventions,
            ref_params,
            ref_return,
            ..
        } = contract
        else {
            let construct = match contract {
                Ty::GenericFunc { .. } => "generic callable value invocation".to_string(),
                other => format!("indirect call through `{other}`"),
            };
            return Err(self.unsupported_reg(construct, dest));
        };
        if variadic.is_some() {
            return Err(self.unsupported_reg("variadic indirect-call contract".into(), dest));
        }
        if ref_return.is_some() {
            return Err(self.unsupported_reg("reference-returning indirect call".into(), dest));
        }
        let (result, returns_value, sret, outcome) = if *raises {
            let ok = lower_ty(self.name, ret, &self.layout, self.reg_span(dest))?;
            let composed = self.layout.outcome_layout(ret).map_err(|error| {
                self.unsupported_reg(
                    format!("raising indirect return of `{ret}` ({error})"),
                    dest,
                )
            })?;
            let outcome = OutcomeAbi {
                layout: composed.layout,
                ok_offset: composed.offsets[1],
                err_offset: composed.offsets[2],
                ok,
                ok_is_reference: false,
            };
            (VoidType::get(ctx).to_handle(), false, None, Some(outcome))
        } else {
            match lower_ty(self.name, ret, &self.layout, self.reg_span(dest))? {
                LowerTy::ZeroSized => (VoidType::get(ctx).to_handle(), false, None, None),
                LowerTy::Scalar(scalar) => (scalar.handle(ctx), true, None, None),
                LowerTy::Aggregate { layout, .. } => {
                    (VoidType::get(ctx).to_handle(), false, Some(layout), None)
                }
            }
        };
        let ptr_ty: TypeHandle = PointerType::get(ctx, 0).into();
        let mut param_handles: Vec<TypeHandle> = Vec::new();
        if sret.is_some() || outcome.is_some() {
            param_handles.push(ptr_ty);
        }
        // The environment pointer rides every indirect call (null for a
        // bare function value); the thunk unpacks it.
        param_handles.push(ptr_ty);
        let mut lowered_params = Vec::with_capacity(params.len());
        let mut owned_params = Vec::with_capacity(params.len());
        let mut by_reference = Vec::with_capacity(params.len());
        for (index, ty) in params.iter().enumerate() {
            let lowered = lower_ty(self.name, ty, &self.layout, self.reg_span(dest))?;
            let convention = conventions.get(index).copied().flatten();
            let by_ref = matches!(
                convention,
                Some(ArgConvention::Mut | ArgConvention::Ref | ArgConvention::Out)
            ) || ref_params.get(index).is_some_and(Option::is_some);
            let owned = matches!(convention, Some(ArgConvention::Var | ArgConvention::Deinit));
            match &lowered {
                _ if by_ref => param_handles.push(ptr_ty),
                LowerTy::Scalar(scalar) => param_handles.push(scalar.handle(ctx)),
                LowerTy::Aggregate { .. } => param_handles.push(ptr_ty),
                LowerTy::ZeroSized => {}
            }
            lowered_params.push(lowered);
            owned_params.push(owned);
            by_reference.push(by_ref);
        }
        let kw_pack_index = if let Some(element) = kw_variadic {
            let index = lowered_params.len();
            let arguments = vec![crate::symbol::InstanceArg::Ty((**element).clone())];
            let instance = crate::symbol::instance_symbol("StringDict", &arguments);
            let ty = Ty::Struct(instance, vec![crate::types::TyArg::Ty((**element).clone())]);
            let lowered = lower_ty(self.name, &ty, &self.layout, self.reg_span(dest))?;
            param_handles.push(ptr_ty);
            lowered_params.push(lowered);
            owned_params.push(true);
            by_reference.push(false);
            Some(index)
        } else {
            None
        };
        let func_ty = FuncType::get(ctx, result, param_handles, false);
        Ok(ContractAbi {
            func_ty,
            returns_value,
            params: lowered_params,
            sret,
            outcome,
            owned_params,
            ref_params: by_reference,
            names: names.clone(),
            required: required.clone(),
            positional_only: *positional_only,
            keyword_only: *keyword_only,
            kw_pack_index,
        })
    }

    /// Check that the physical shape an indirect caller derives from its
    /// contract agrees with the compiled target the thunk forwards to —
    /// out-pointer kind, parameter classification, and reference-ness must
    /// match slot for slot after the capture prefix, or the call would pass
    /// values where pointers are expected. Checked programs agree here; a
    /// disagreement is surfaced as a contextual rejection, never a silent
    /// miscompile.
    pub(super) fn check_contract_target(
        &self,
        abi: &ContractAbi,
        target: &FnSignature,
        captures: usize,
        name: &str,
        dest: Reg,
    ) -> Result<(), PlironError> {
        let disagree = |lowering: &Self| -> PlironError {
            lowering.unsupported_reg(
                format!("indirect-call contract disagreeing with compiled `{name}`"),
                dest,
            )
        };
        if abi.outcome.is_some() != target.outcome.is_some()
            || target.outcome.as_ref().is_some_and(|o| o.ok_is_reference)
            || abi.sret.is_some() != target.sret.is_some()
            || abi.params.len() + captures != target.params.len()
        {
            return Err(disagree(self));
        }
        for (index, param) in abi.params.iter().enumerate() {
            let target_index = captures + index;
            let target_param = &target.params[target_index];
            let contract_ref = abi.ref_params.get(index).copied().unwrap_or(false);
            let target_ref = target
                .ref_params
                .get(target_index)
                .copied()
                .unwrap_or(false);
            let agree = contract_ref == target_ref
                && match (param, target_param) {
                    (LowerTy::Scalar(a), LowerTy::Scalar(b)) => a == b,
                    (LowerTy::Aggregate { .. }, LowerTy::Aggregate { .. }) => true,
                    (LowerTy::ZeroSized, LowerTy::ZeroSized) => true,
                    _ => false,
                };
            if !agree {
                return Err(disagree(self));
            }
        }
        Ok(())
    }
}
