//! Call-site inference: subscript/iterator rewrites and structural
//! binding inference for direct and receiver calls.

use super::*;

impl<'a> Specializer<'a> {
    /// Retarget one checker-selected subscript invocation (the
    /// `__getitem__`/`__setitem__` family) to its concrete instance. The
    /// receiver binds the owner's parameters and the destination's checked
    /// type anchors the result, mirroring the nullary iterator-step
    /// inference; subscript actuals are `Int` indexes or slice descriptors
    /// and never carry generic solutions of their own.
    pub(super) fn rewrite_subscript_call(
        &mut self,
        owner: &str,
        function: &MirFunction,
        receiver: Reg,
        dest: Option<Reg>,
        call: &mut mojito_mir::mir::MirSubscriptCall,
    ) -> Result<(), MonoError> {
        let Some(receiver_ty) = function.reg_types.get(&receiver.0) else {
            return Ok(());
        };
        let receiver_ty = peel_refs(receiver_ty).clone();
        let Ty::Struct(receiver_name, _) = &receiver_ty else {
            return Ok(());
        };
        let method = if dest.is_some() {
            "__getitem__"
        } else {
            "__setitem__"
        };
        let target = mojito_symbol::symbol::resolve_method_symbol(
            self.functions.iter().map(|(name, f)| CallableCandidate {
                name,
                n_params: f.n_params,
            }),
            nominal_template(receiver_name),
            method,
            Some(&call.target),
            call.arguments.len(),
        );
        if !self.functions.contains_key(target.as_str()) {
            return Ok(());
        }
        // The checker-selected result fact is the authoritative anchor even
        // for a reference result. `unify_result` peels the handle layer, so a
        // bare implicit-view receiver can recover its owner element type from
        // `ref Int` without confusing it with a reference-valued element.
        let result = Some(&call.result_ty);
        let (mut bindings, mut arguments, _) =
            self.infer_receiver_call(owner, &target, &receiver_ty, result)?;
        // A comptime-specialized accessor (`Tuple$tN.__getitem__[i: Int]`)
        // varies by its value parameter: the constant index joins the
        // instance identity — sharing on the receiver alone would collapse
        // same-element-type indexes onto one body — and binds for the
        // instance body's value-parameter reads.
        for (decl, param_arg) in call.param_decls.iter().zip(&call.param_arg_regs) {
            let ParamDecl::Value { name, .. } = decl else {
                continue;
            };
            if bindings.values.contains_key(name.as_str()) {
                continue;
            }
            let value = param_arg
                .value
                .and_then(|reg| const_reg_value(function, reg));
            let Some(value) = value else {
                return Ok(());
            };
            bindings.values.insert(name.clone(), value.clone());
            arguments.push(InstanceArg::Value(value));
        }
        call.target = self.enqueue(&target, bindings, arguments)?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn infer_call(
        &self,
        owner: &str,
        caller: &MirFunction,
        target: &str,
        receiver: Option<Reg>,
        dest: Reg,
        args: &[Reg],
        kwargs: &[(String, Reg)],
        param_args: &[mojito_mir::mir::MirParamArg],
    ) -> Result<(String, Bindings, Vec<InstanceArg>), MonoError> {
        let declaration = self.declarations.get(target).copied().ok_or_else(|| {
            self.error(
                Some(owner),
                format!("callee `{target}` lacks declaration facts"),
            )
        })?;
        let mut bindings = self.base_bindings();
        let receiver_pattern_for_instance = receiver.and_then(|_| {
            self.functions
                .get(target)
                .and_then(|function| function.param_types.first())
                .cloned()
        });
        let mut owner_covered = 0;
        if let Some(receiver) = receiver {
            let actual_receiver = peel_refs(reg_ty(caller, receiver, owner)?);
            if let Ty::Struct(receiver_name, arguments) = actual_receiver
                && let Some(struct_decl) =
                    self.structs.get(nominal_template(receiver_name)).copied()
            {
                bind_ty_args(&struct_decl.param_decls, arguments, &mut bindings).map_err(|e| {
                    self.error(
                        Some(owner),
                        format!("monomorphizing receiver for `{target}`: {e}"),
                    )
                })?;
                // An instance-named receiver carries the owner's concrete
                // identity: record it so the method instance is named under
                // the owner and its body's bare `self` spelling resolves.
                if nominal_template(receiver_name) != receiver_name {
                    bindings.self_instance = Some((
                        nominal_template(receiver_name).to_string(),
                        actual_receiver.clone(),
                    ));
                    owner_covered =
                        owner_covered_prefix(&struct_decl.param_decls, &declaration.param_decls);
                }
            }
            let receiver_pattern = self
                .functions
                .get(target)
                .and_then(|function| function.param_types.first())
                .ok_or_else(|| {
                    self.error(
                        Some(owner),
                        format!("method `{target}` lacks a receiver type"),
                    )
                })?;
            unify(receiver_pattern, actual_receiver, &mut bindings)
                .map_err(|e| self.error(Some(owner), format!("monomorphizing `{target}`: {e}")))?;
        }
        bind_explicit_value_arguments(
            &declaration.param_decls,
            param_args,
            &self.constant_values,
            &mut bindings,
            target,
        )?;
        apply_defaults(&declaration.param_decls, &mut bindings)?;
        let names = &declaration.param_names;
        let required = &declaration.required;
        let slots = match_call_slots(
            names,
            required,
            declaration.positional_only,
            declaration.keyword_only,
            args.len(),
            &kwargs
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            CallVariadics {
                positional: declaration.variadic.is_some(),
                keyword: declaration.kw_variadic.is_some(),
            },
        )
        .map_err(|e| {
            self.error(
                Some(owner),
                format!("binding call to `{target}` during monomorphization: {e:?}"),
            )
        })?;
        let mut callable_arguments = Vec::new();
        for (index, slot) in slots.slots.iter().enumerate() {
            let actual_reg = match slot {
                ArgSlot::Positional(i) => Some(args[*i]),
                ArgSlot::Keyword(i) => Some(kwargs[*i].1),
                ArgSlot::Default => None,
            };
            if let Some(actual_reg) = actual_reg {
                let actual = reg_ty(caller, actual_reg, owner)?;
                // Explicit value parameters may resolve a dependent pattern;
                // an ordinary unresolved type parameter must remain available
                // for structural inference from this runtime argument.
                let pattern = substitute_ty(&declaration.param_types[index], &bindings)
                    .unwrap_or_else(|_| declaration.param_types[index].clone());
                if is_symbolic(&pattern) {
                    unify(&pattern, actual, &mut bindings).map_err(|e| {
                        self.error(Some(owner), format!("monomorphizing `{target}`: {e}"))
                    })?;
                }
                // Ordinary `Func` parameters carry their closure environment
                // at runtime. Only retained generic-callable parameters are
                // compile-time inputs to instance selection; treating every
                // statically traceable closure as such would discard captures.
                if matches!(
                    peel_refs(&declaration.param_types[index]),
                    Ty::GenericFunc { .. }
                        | Ty::Param {
                            callable_bound: Some(_),
                            ..
                        }
                ) && let Some((callable, captures_are_empty)) =
                    self.callable_targets.get(&actual_reg.0)
                {
                    if !captures_are_empty {
                        return Err(self.error(
                            Some(owner),
                            format!("generic retained callable `{callable}` has captures"),
                        ));
                    }
                    bindings.values.insert(
                        declaration.param_names[index].clone(),
                        CtValue::Str(callable.clone()),
                    );
                    bindings
                        .callables
                        .insert(declaration.param_names[index].clone(), callable.clone());
                    callable_arguments.push(InstanceArg::Value(CtValue::Str(callable.clone())));
                }
            }
        }
        // An unspecialized variadic callee instantiates at its call-site
        // arity: each overflow positional unifies against the pack element
        // and the arity joins the instance identity. Checker-specialized
        // packs (`Tuple$tN`'s concrete `RuntimePack`) keep their identity.
        let variadic_arity = match &declaration.variadic {
            // The declaration records the pack ELEMENT type; a concrete
            // `RuntimePack`/`Tuple` spelling means the checker already
            // specialized the pack (`Tuple$tN`).
            Some(element) if !matches!(element, Ty::RuntimePack(_) | Ty::Tuple(_)) => {
                for index in &slots.positional_overflow {
                    let actual = reg_ty(caller, args[*index], owner)?;
                    unify(element, actual, &mut bindings).map_err(|e| {
                        self.error(Some(owner), format!("monomorphizing `{target}` pack: {e}"))
                    })?;
                }
                bindings.variadic_arity = Some(slots.positional_overflow.len());
                Some(slots.positional_overflow.len())
            }
            _ => None,
        };
        if receiver != Some(dest)
            && let Some(actual) = caller.reg_types.get(&dest.0)
        {
            unify_result(&declaration.ret_ty, actual, &mut bindings).map_err(|e| {
                self.error(
                    Some(owner),
                    format!("monomorphizing `{target}` return: {e}"),
                )
            })?;
        }
        if bindings.self_instance.is_none()
            && let Some(receiver_pattern) = receiver_pattern_for_instance.as_ref()
            && let Ty::Struct(template, arguments) = peel_refs(receiver_pattern)
            && !arguments.is_empty()
        {
            let concrete = substitute_ty(peel_refs(receiver_pattern), &bindings)?;
            bindings.self_instance = Some((nominal_template(template).to_string(), concrete));
            if let Some(struct_decl) = self.structs.get(nominal_template(template)).copied() {
                owner_covered =
                    owner_covered_prefix(&struct_decl.param_decls, &declaration.param_decls);
            }
        }
        if bindings.self_instance.is_none()
            && let Some(receiver) = receiver
            && let Ty::Struct(receiver_name, _) = peel_refs(reg_ty(caller, receiver, owner)?)
            && let Some(struct_decl) = self.structs.get(nominal_template(receiver_name)).copied()
            && !struct_decl.param_decls.is_empty()
        {
            let owner_arguments = ordered_arguments(
                &struct_decl.param_decls,
                &bindings,
                nominal_template(receiver_name),
            )?;
            let ty_arguments = owner_arguments
                .iter()
                .map(|argument| match argument {
                    InstanceArg::Ty(ty) => TyArg::Ty(ty.clone()),
                    InstanceArg::Value(value) => TyArg::Val(value.clone()),
                })
                .collect::<Vec<_>>();
            let owner = mojito_symbol::symbol::instance_symbol(
                nominal_template(receiver_name),
                &owner_arguments,
            );
            bindings.self_instance = Some((
                nominal_template(receiver_name).to_string(),
                Ty::Struct(owner, ty_arguments),
            ));
            owner_covered =
                owner_covered_prefix(&struct_decl.param_decls, &declaration.param_decls);
        }
        let mut arguments = ordered_arguments(&declaration.param_decls, &bindings, target)?;
        // The owner-restating prefix (`__init__` prepends the struct's
        // `param_decls`) is already carried by the instance's `owner`
        // identity; keep only the method's own parameters.
        arguments.drain(..owner_covered);
        if let Some(arity) = variadic_arity {
            arguments.push(InstanceArg::Value(CtValue::Int(arity as i64)));
        }
        arguments.extend(callable_arguments);
        push_sugar_arguments(declaration, &bindings, &mut arguments);
        Ok((target.to_string(), bindings, arguments))
    }

    /// Walk `blocks` (recursing into `try` regions) folding every `GetIter`
    /// before the main call rewrite reads iterator slot types.
    pub(super) fn rewrite_iterator_inits(
        &mut self,
        owner: &str,
        function: &mut MirFunction,
        blocks: &mut [MirBlock],
    ) -> Result<(), MonoError> {
        for block in blocks {
            for instruction in &mut block.instrs {
                match instruction {
                    MirInstr::Try {
                        body,
                        handler,
                        orelse,
                        finalbody,
                        ..
                    } => {
                        self.rewrite_iterator_inits(owner, function, body)?;
                        if let Some((_, blocks)) = handler {
                            self.rewrite_iterator_inits(owner, function, blocks)?;
                        }
                        if let Some(blocks) = orelse {
                            self.rewrite_iterator_inits(owner, function, blocks)?;
                        }
                        if let Some(blocks) = finalbody {
                            self.rewrite_iterator_inits(owner, function, blocks)?;
                        }
                    }
                    MirInstr::GetIter {
                        source,
                        dest,
                        mode: _,
                        prepare,
                    } => {
                        let (source, dest) = (*source, *dest);
                        self.rewrite_get_iter(owner, function, source, dest, prepare)?;
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    /// Fold a `GetIter` normalization chain: retarget every `prepare` step to
    /// its concrete instance, statically unroll dynamic `__trait_dispatch.`
    /// normalization (the VM repeats that step at runtime until the value has a
    /// `__next__`; the receiver is concrete here), and record the chain's final
    /// return type as the iterator variable's type — HIR leaves the split
    /// `$iterobj` slot untyped.
    pub(super) fn rewrite_get_iter(
        &mut self,
        owner: &str,
        function: &mut MirFunction,
        source: mojito_hir::hir::VarId,
        dest: mojito_hir::hir::VarId,
        prepare: &mut Vec<String>,
    ) -> Result<(), MonoError> {
        let Some(mut current) = function.var_tys.get(&source).cloned() else {
            // An untyped source belongs to a compiler-private pack loop the
            // backend rejects at its own boundary.
            return Ok(());
        };
        // A pack-typed source is the compiler-private pack fallback (the
        // VM's `remove(0)` loop): no nominal protocol resolves. The split
        // slot keeps the pack layout; lowering tracks the advance position
        // in a backend-side shadow slot.
        if matches!(&current, Ty::RuntimePack(_) | Ty::Tuple(_)) {
            function.var_tys.insert(dest, current);
            return Ok(());
        }
        // A borrowed named source binds the slot to a reference; follow it to
        // the underlying iterable type, as the VM does for name resolution.
        if let Ty::Ref(reference) = &current {
            current = (*reference.referent).clone();
        }
        let dispatch = prepare
            .iter()
            .find(|symbol| symbol.starts_with("__trait_dispatch."))
            .cloned();
        for selected in prepare.iter_mut() {
            let (target, result) =
                self.resolve_iterator_step(owner, &current, "__iter__", Some(selected), None)?;
            *selected = target;
            current = result;
        }
        if let Some(selected) = dispatch {
            let mut budget = 8u32;
            while !self.has_iterator_next(&current) {
                if budget == 0 {
                    return Err(self.error(
                        Some(owner),
                        "iterator normalization did not converge within the dispatch budget",
                    ));
                }
                budget -= 1;
                let (target, result) =
                    self.resolve_iterator_step(owner, &current, "__iter__", Some(&selected), None)?;
                prepare.push(target);
                current = result;
            }
        }
        function.var_tys.insert(dest, current);
        Ok(())
    }

    /// Resolve one nullary iterator-protocol operation against a concrete
    /// receiver type, enqueue the target instance, and return its concrete
    /// name plus its substituted result type.
    pub(super) fn resolve_iterator_step(
        &mut self,
        owner: &str,
        receiver: &Ty,
        method: &str,
        selected: Option<&str>,
        result: Option<&Ty>,
    ) -> Result<(String, Ty), MonoError> {
        let Ty::Struct(receiver_name, _) = receiver else {
            return Err(self.error(
                Some(owner),
                format!("iterator `{method}` operation applied to non-struct type `{receiver}`"),
            ));
        };
        let target = mojito_symbol::symbol::resolve_method_symbol(
            self.functions.iter().map(|(name, f)| CallableCandidate {
                name,
                n_params: f.n_params,
            }),
            nominal_template(receiver_name),
            method,
            selected,
            0,
        );
        if !self.functions.contains_key(target.as_str()) {
            return Err(self.error(
                Some(owner),
                format!("iterator method `{target}` is missing from the MIR program"),
            ));
        }
        let (bindings, arguments, result) =
            self.infer_receiver_call(owner, &target, receiver, result)?;
        let concrete = self.enqueue(&target, bindings, arguments)?;
        Ok((concrete, result))
    }

    /// The receiver-typed sibling of [`Self::infer_call`] for nullary method
    /// calls carried by iterator instructions, which name their receiver as a
    /// variable slot rather than a register.
    pub(super) fn infer_receiver_call(
        &self,
        owner: &str,
        target: &str,
        receiver: &Ty,
        result: Option<&Ty>,
    ) -> Result<(Bindings, Vec<InstanceArg>, Ty), MonoError> {
        let declaration = self.declarations.get(target).copied().ok_or_else(|| {
            self.error(
                Some(owner),
                format!("callee `{target}` lacks declaration facts"),
            )
        })?;
        let mut bindings = self.base_bindings();
        let mut owner_covered = 0;
        if let Ty::Struct(receiver_name, arguments) = receiver
            && let Some(struct_decl) = self.structs.get(nominal_template(receiver_name)).copied()
        {
            bind_ty_args(&struct_decl.param_decls, arguments, &mut bindings).map_err(|e| {
                self.error(
                    Some(owner),
                    format!("monomorphizing receiver for `{target}`: {e}"),
                )
            })?;
            if nominal_template(receiver_name) != receiver_name {
                bindings.self_instance = Some((
                    nominal_template(receiver_name).to_string(),
                    receiver.clone(),
                ));
                owner_covered =
                    owner_covered_prefix(&struct_decl.param_decls, &declaration.param_decls);
            }
        }
        let receiver_pattern = self
            .functions
            .get(target)
            .and_then(|function| function.param_types.first())
            .ok_or_else(|| {
                self.error(
                    Some(owner),
                    format!("method `{target}` lacks a receiver type"),
                )
            })?;
        unify(receiver_pattern, receiver, &mut bindings)
            .map_err(|e| self.error(Some(owner), format!("monomorphizing `{target}`: {e}")))?;
        if let Some(result) = result {
            unify_result(&declaration.ret_ty, result, &mut bindings).map_err(|e| {
                self.error(
                    Some(owner),
                    format!("monomorphizing `{target}` return: {e}"),
                )
            })?;
        }
        apply_defaults(&declaration.param_decls, &mut bindings)?;
        if bindings.self_instance.is_none()
            && let Ty::Struct(template, arguments) = peel_refs(receiver_pattern)
            && !arguments.is_empty()
        {
            let concrete = substitute_ty(peel_refs(receiver_pattern), &bindings)?;
            bindings.self_instance = Some((nominal_template(template).to_string(), concrete));
            if let Some(struct_decl) = self.structs.get(nominal_template(template)).copied() {
                owner_covered =
                    owner_covered_prefix(&struct_decl.param_decls, &declaration.param_decls);
            }
        }
        if bindings.self_instance.is_none()
            && let Ty::Struct(receiver_name, _) = receiver
            && let Some(struct_decl) = self.structs.get(nominal_template(receiver_name)).copied()
            && !struct_decl.param_decls.is_empty()
        {
            let owner_arguments = ordered_arguments(
                &struct_decl.param_decls,
                &bindings,
                nominal_template(receiver_name),
            )?;
            let ty_arguments = owner_arguments
                .iter()
                .map(|argument| match argument {
                    InstanceArg::Ty(ty) => TyArg::Ty(ty.clone()),
                    InstanceArg::Value(value) => TyArg::Val(value.clone()),
                })
                .collect::<Vec<_>>();
            let owner = mojito_symbol::symbol::instance_symbol(
                nominal_template(receiver_name),
                &owner_arguments,
            );
            bindings.self_instance = Some((
                nominal_template(receiver_name).to_string(),
                Ty::Struct(owner, ty_arguments),
            ));
            owner_covered =
                owner_covered_prefix(&struct_decl.param_decls, &declaration.param_decls);
        }
        let mut arguments = ordered_arguments(&declaration.param_decls, &bindings, target)?;
        arguments.drain(..owner_covered);
        let result = substitute_ty(&declaration.ret_ty, &bindings).map_err(|e| {
            self.error(
                Some(owner),
                format!("monomorphizing `{target}` result: {}", e.construct),
            )
        })?;
        Ok((bindings, arguments, result))
    }

    /// Whether the concrete receiver type resolves a nullary `__next__` — the
    /// VM's runtime convergence test for dynamic iterator normalization.
    pub(super) fn has_iterator_next(&self, receiver: &Ty) -> bool {
        let Ty::Struct(name, _) = receiver else {
            return false;
        };
        let target = mojito_symbol::symbol::resolve_method_symbol(
            self.functions.iter().map(|(name, f)| CallableCandidate {
                name,
                n_params: f.n_params,
            }),
            nominal_template(name),
            "__next__",
            None,
            0,
        );
        self.functions.contains_key(target.as_str())
    }
}
