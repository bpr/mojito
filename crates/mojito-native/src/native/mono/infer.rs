//! Call-site inference: subscript/iterator rewrites and structural
//! binding inference for direct and receiver calls.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_checked::checked::CheckedCallArgumentSource;
use mojito_hir::hir::VarId;
use mojito_mir::mir::SlotParameter;
use mojito_types::types::TyKind;

/// The runtime operands one subscript instruction passes its accessor
/// past the receiver.
#[derive(Clone, Copy)]
pub(super) struct SubscriptOperands<'a> {
    /// The operands held in registers: an `index.get`'s index.
    pub(super) registers: &'a [Reg],
    /// How many positional operands the instruction passes, inline slice
    /// descriptors included.
    pub(super) positional: usize,
    /// The names of its keyword operands, in order.
    pub(super) keywords: &'a [&'a str],
}

/// The operand indexes a subscript instruction sheds for its accessor's
/// removed arguments, as upstream's rewritten call passes none for them.
pub(super) struct DroppedOperands {
    pub(super) positional: BTreeSet<usize>,
    pub(super) keyword: BTreeSet<usize>,
}

/// What a receiver call demands of its callee besides the receiver.
#[derive(Clone, Copy, Default)]
pub(super) struct ReceiverCall<'a> {
    /// The checked result type anchoring the callee's return, if any.
    pub(super) result: Option<&'a Ty>,
    /// Explicit compile-time value arguments, by binder.
    pub(super) explicit: &'a [(ParamRef, CtValue)],
    /// The checked parameter types past the receiver, by declaration
    /// position.
    pub(super) parameters: &'a [&'a Ty],
    /// Whether the demand is a call, which names the generator without the
    /// callee's unused arguments ([`Bindings::reduced`]); a protocol or a
    /// member table reaches the full one.
    pub(super) reduced: bool,
}

impl Specializer<'_> {
    /// Retarget one checker-selected subscript invocation (the
    /// `__getitem__`/`__setitem__` family) to its concrete instance. The
    /// receiver binds the owner's parameters and the destination's checked
    /// type anchors the result, mirroring the nullary iterator-step
    /// inference; subscript actuals are `Int` indexes or slice descriptors
    /// and never carry generic solutions of their own. A subscript is a
    /// call of its accessor, so it names the generator without the
    /// accessor's unused arguments, as upstream's rewritten call does, and
    /// its contract loses the arguments that bound them: returns the
    /// operand indexes the instruction sheds, or `None` when it keeps every
    /// operand.
    pub(super) fn rewrite_subscript_call(
        &mut self,
        owner: &str,
        function: &MirFunction,
        receiver: Reg,
        operands: SubscriptOperands<'_>,
        dest: Option<Reg>,
        call: &mut mojito_mir::mir::MirSubscriptCall,
    ) -> Result<Option<DroppedOperands>, MonoError> {
        let Some(receiver_ty) = function.reg_types.get(&receiver.0) else {
            return Ok(None);
        };
        let receiver_ty = peel_refs(receiver_ty).clone();
        let TyKind::Struct(receiver_name, _) = receiver_ty.kind() else {
            return Ok(None);
        };
        let method = if dest.is_some() {
            "__getitem__"
        } else {
            "__setitem__"
        };
        let target = mojito_symbol::symbol::resolve_method_symbol(
            &self.functions,
            nominal_template(receiver_name),
            method,
            Some(&call.target),
            call.arguments.len(),
        );
        if !self.functions.contains_key(target.as_str()) {
            return Ok(None);
        }
        // The checker-selected result fact is the authoritative anchor even
        // for a reference result. `unify_result` peels the handle layer, so a
        // bare implicit-view receiver can recover its owner element type from
        // `ref Int` without confusing it with a reference-valued element.
        let result = Some(&call.result_ty);
        let callable_parameter =
            self.declarations[target.as_str()]
                .param_decls
                .iter()
                .any(|decl| {
                    matches!(
                        decl,
                        ParamDecl::Type {
                            callable_bound: Some(_),
                            ..
                        }
                    )
                });
        // An accessor keyed on a value parameter of its own
        // (`Tuple.__getitem_param__[index: Int]`) varies by it: the constant
        // index binds before the receiver is inferred, so the result type
        // over it closes, and joins the instance identity.
        let mut explicit = Vec::new();
        let removed = self.removed_params(&target, true);
        for (decl, param_arg) in call.param_decls.iter().zip(&call.param_arg_regs) {
            // The argument of a parameter the accessor removed is no longer
            // passed ([`mojito_mir::mir::with_removed_forwards_erased`]).
            let erased = param_arg.value.is_none()
                && param_arg.expr.is_none()
                && removed.contains(decl.id());
            if !matches!(decl, ParamDecl::Value { .. }) || erased {
                continue;
            }
            // A constant register is the value; an index built from the
            // caller's binders is its recorded expression under the caller
            // instance's bindings.
            let value = param_arg
                .value
                .and_then(|reg| {
                    self.constant_values
                        .get(&reg.0)
                        .cloned()
                        .or_else(|| const_reg_value(function, reg))
                })
                .or_else(|| {
                    param_arg
                        .expr
                        .as_ref()
                        .and_then(|expr| eval_ct(expr, &self.enclosing).ok())
                });
            let Some(value) = value else {
                return Ok(None);
            };
            explicit.push((decl.binder(), value));
        }
        let (target, mut bindings, mut arguments) = if callable_parameter {
            self.infer_call(
                owner,
                function,
                &target,
                Some(receiver),
                dest.unwrap_or(receiver),
                operands.registers,
                &[],
                &call.param_arg_regs,
                &[],
            )?
        } else {
            let parameters: Vec<&Ty> = call
                .arguments
                .iter()
                .map(|argument| &argument.parameter_ty)
                .collect();
            let (bindings, arguments, _) = self.infer_receiver_call(
                owner,
                &target,
                &receiver_ty,
                ReceiverCall {
                    result,
                    explicit: &explicit,
                    parameters: &parameters,
                    reduced: true,
                },
            )?;
            (target, bindings, arguments)
        };
        let declared = &self.declarations[target.as_str()].param_decls;
        for (binder, value) in explicit {
            if bindings.values.contains_key(&binder) {
                continue;
            }
            bindings.values.insert(binder.clone(), value.clone());
            if !declared.iter().any(|decl| decl.binder() == binder) {
                arguments.push(InstanceArg::Value(value));
            }
        }
        let reduced = bindings.reduced;
        call.target = self.enqueue(&target, bindings, arguments)?;
        call.param_arg_regs.clear();
        call.param_decls.clear();
        if !reduced {
            return Ok(None);
        }
        let (Some(declaration), Some(generator)) = (
            self.declarations.get(target.as_str()),
            self.functions.get(target.as_str()),
        ) else {
            return Ok(None);
        };
        let (positional, keyword) = mojito_mir::mir::removed_argument_operands(
            declaration,
            &generator.removed_args,
            operands.positional,
            operands.keywords,
        )
        .map_err(|e| {
            self.error(
                Some(owner),
                format!("binding subscript call to `{target}` during argument removal: {e:?}"),
            )
        })?;
        // The contract lists one argument per declared parameter past the
        // receiver, by position; the kept ones name their operands by the
        // indexes the instruction keeps.
        let removed: BTreeSet<usize> = generator
            .removed_args
            .iter()
            .filter_map(|&slot| match declaration.slot_parameter(slot as usize) {
                SlotParameter::Position(position) => Some(position),
                _ => None,
            })
            .collect();
        let mut position = 0;
        call.arguments.retain(|_| {
            let kept = !removed.contains(&position);
            position += 1;
            kept
        });
        for argument in &mut call.arguments {
            argument.source = match argument.source {
                CheckedCallArgumentSource::Positional(index) => {
                    CheckedCallArgumentSource::Positional(index - positional.range(..index).count())
                }
                CheckedCallArgumentSource::Keyword(index) => {
                    CheckedCallArgumentSource::Keyword(index - keyword.range(..index).count())
                }
                CheckedCallArgumentSource::Default => CheckedCallArgumentSource::Default,
            };
        }
        Ok(Some(DroppedOperands {
            positional,
            keyword,
        }))
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
        instantiated: &[TyArg],
    ) -> Result<(String, Bindings, Vec<InstanceArg>), MonoError> {
        let declaration = self.declarations.get(target).ok_or_else(|| {
            self.error(
                Some(owner),
                format!("callee `{target}` lacks declaration facts"),
            )
        })?;
        // A direct call names the generator with its template's unused
        // arguments removed; the caller drops their arguments
        // ([`Self::drop_removed_arguments`]).
        let mut bindings = Bindings {
            reduced: true,
            ..self.base_bindings()
        };
        bindings.reduced = self.reduces(target, &bindings);
        let receiver_pattern_for_instance = receiver.and_then(|_| {
            self.functions
                .get(target)
                .and_then(|function| function.param_types.first())
                .cloned()
        });
        let mut owner_covered = 0;
        // A constructor's declaration restates its struct's parameters
        // first, which a method's receiver binds.
        let mut own_start = 0;
        if let Some(receiver) = receiver {
            let actual_receiver = peel_refs(reg_ty(caller, receiver, owner)?);
            if let TyKind::Struct(receiver_name, arguments) = actual_receiver.kind()
                && let Some(struct_decl) = self.structs.get(nominal_template(receiver_name))
            {
                own_start =
                    owner_covered_prefix(&struct_decl.param_decls, &declaration.param_decls);
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
        // A construction's solved arguments restate its struct's, as its
        // declaration does; a method call's are the method's own.
        let instantiated_start = if receiver == Some(dest) { 0 } else { own_start };
        self.bind_instantiated_arguments(
            owner,
            target,
            &declaration.param_decls[instantiated_start..],
            instantiated,
            &mut bindings,
        )?;
        bind_explicit_value_arguments(
            &declaration.param_decls,
            self.removed_params(target, bindings.reduced),
            param_args,
            &self.constant_values,
            &mut bindings,
            target,
            &self.enclosing,
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
                // A callable-bounded method parameter is represented by its
                // structural callable contract in the runtime signature, so
                // ordinary `TyKind::Param` unification has no binder spelling to
                // recover. The declaration still carries that spelling;
                // bind the (unique) callable type parameter from the actual.
                let unresolved_callable_decls = declaration
                    .param_decls
                    .iter()
                    .filter(|decl| {
                        matches!(
                            decl,
                            ParamDecl::Type {
                                callable_bound: Some(_),
                                ..
                            }
                        )
                    })
                    .map(ParamDecl::binder)
                    .filter(|binder| !bindings.types.contains_key(binder))
                    .collect::<Vec<_>>();
                if let [parameter] = unresolved_callable_decls.as_slice() {
                    bindings.types.insert(parameter.clone(), actual.clone());
                }
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
                    peel_refs(&declaration.param_types[index]).kind(),
                    TyKind::GenericFunc(_)
                        | TyKind::Param {
                            callable_bound: Some(_),
                            ..
                        }
                ) && let Some((callable, captures_are_empty)) =
                    self.callable_targets.get(&actual_reg.0)
                    && *captures_are_empty
                {
                    bindings
                        .callables
                        .insert(declaration.param_names[index].clone(), callable.clone());
                    callable_arguments.push(InstanceArg::Value(CtValue::Str(callable.clone())));
                }
            }
        }
        // A type pack binds to the overflow's element types whole, one per
        // position, unless the call's recorded solution bound it. An
        // unspecialized homogeneous collector is a run-time value, as the
        // pin's `VariadicList`: its overflow binds only the element type, so
        // there is one instance for every element type, never one per arity.
        // A concrete `RuntimePack`/`Tuple` spelling means the checker
        // already specialized the pack.
        match &declaration.variadic {
            Some(element)
                if let Some(TyKind::Param { binder, .. }) =
                    (mojito_types::types::pack_spread(std::slice::from_ref(element)))
                        .map(Ty::kind) =>
            {
                if !bindings.types.contains_key(binder) {
                    let elements = slots
                        .positional_overflow
                        .iter()
                        .map(|index| {
                            reg_ty(caller, args[*index], owner).map(materialize_nested_literals)
                        })
                        .collect::<Result<Vec<Ty>, _>>()?;
                    bind_pack(binder, elements, &mut bindings);
                }
            }
            Some(element)
                if !matches!(element.kind(), TyKind::RuntimePack(_) | TyKind::Tuple(_)) =>
            {
                for index in &slots.positional_overflow {
                    let actual = reg_ty(caller, args[*index], owner)?;
                    unify(element, actual, &mut bindings).map_err(|e| {
                        self.error(Some(owner), format!("monomorphizing `{target}` pack: {e}"))
                    })?;
                }
            }
            _ => {}
        }
        // Each collected keyword unifies against the `**kwargs` element; a
        // forwarded collector (`**options^`) contributes its own element.
        if let Some(element) = declaration.kw_variadic.as_ref()
            && is_symbolic(element)
        {
            for index in &slots.keyword_overflow {
                let (name, reg) = &kwargs[*index];
                let actual = peel_refs(reg_ty(caller, *reg, owner)?);
                let actual = match actual.kind() {
                    TyKind::Struct(_, arguments) if name == "**" => match arguments.as_slice() {
                        [TyArg::Ty(collected)] => collected,
                        _ => continue,
                    },
                    _ => actual,
                };
                unify(element, actual, &mut bindings).map_err(|e| {
                    self.error(
                        Some(owner),
                        format!("monomorphizing `{target}` keywords: {e}"),
                    )
                })?;
            }
        }
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
            && let TyKind::Struct(template, arguments) = peel_refs(receiver_pattern).kind()
            && !arguments.is_empty()
        {
            let concrete = substitute_ty(peel_refs(receiver_pattern), &bindings)?;
            bindings.self_instance = Some((nominal_template(template).to_string(), concrete));
            if let Some(struct_decl) = self.structs.get(nominal_template(template)) {
                owner_covered =
                    owner_covered_prefix(&struct_decl.param_decls, &declaration.param_decls);
            }
        }
        if bindings.self_instance.is_none()
            && let Some(receiver) = receiver
            && let TyKind::Struct(receiver_name, _) =
                (peel_refs(reg_ty(caller, receiver, owner)?)).kind()
            && let Some(struct_decl) = self.structs.get(nominal_template(receiver_name))
            && !struct_decl.param_decls.is_empty()
        {
            bindings.self_instance = Some((
                nominal_template(receiver_name).to_string(),
                owner_instance_ty(
                    nominal_template(receiver_name),
                    &struct_decl.param_decls,
                    &bindings,
                )?,
            ));
            owner_covered =
                owner_covered_prefix(&struct_decl.param_decls, &declaration.param_decls);
        }
        // The owner-restating prefix (`__init__` prepends the struct's
        // `param_decls`) is already carried by the instance's `owner`
        // identity; keep only the method's own parameters.
        let mut arguments =
            self.own_arguments(&declaration.param_decls, &bindings, target, owner_covered)?;
        arguments.extend(callable_arguments);
        push_sugar_arguments(declaration, &bindings, &mut arguments);
        Ok((target.to_string(), bindings, arguments))
    }

    /// Bind the owner's parameters of a static call from its spelled
    /// receiver (`Pair[Int].count()`), and name the instance under that
    /// owner: no runtime argument need carry them.
    pub(super) fn bind_static_receiver(
        &self,
        owner: &str,
        target: &str,
        receiver: &Ty,
        bindings: &mut Bindings,
    ) -> Result<(), MonoError> {
        let TyKind::Struct(receiver_name, arguments) = receiver.kind() else {
            return Ok(());
        };
        let template = nominal_template(receiver_name);
        let Some(struct_decl) = self.structs.get(template) else {
            return Ok(());
        };
        if struct_decl.param_decls.is_empty() {
            return Ok(());
        }
        bind_ty_args(&struct_decl.param_decls, arguments, bindings).map_err(|e| {
            self.error(
                Some(owner),
                format!("monomorphizing receiver for `{target}`: {e}"),
            )
        })?;
        if bindings.self_instance.is_none() {
            // An erased argument (`S[_REMOVED_ARG]`, spelled where the body
            // removed the parameter it named) binds nothing and names the
            // erased instance.
            let owner_arguments = struct_decl
                .param_decls
                .iter()
                .enumerate()
                .map(|(position, decl)| match arguments.get(position) {
                    Some(TyArg::Val(CtValue::RemovedArg)) => {
                        Ok(InstanceArg::Value(CtValue::RemovedArg))
                    }
                    _ => bound_argument(decl, bindings, template),
                })
                .collect::<Result<Vec<_>, _>>()?;
            let ty_arguments = owner_arguments
                .iter()
                .map(|argument| match argument {
                    InstanceArg::Ty(ty) => TyArg::Ty(ty.clone()),
                    InstanceArg::Value(value) => TyArg::Val(value.clone()),
                })
                .collect();
            let instance = mojito_symbol::symbol::instance_symbol(template, &owner_arguments);
            bindings.self_instance = Some((
                template.to_string(),
                TyKind::Struct(instance, ty_arguments).into(),
            ));
        }
        Ok(())
    }

    /// Walk `blocks` (recursing into every region) folding every `GetIter`
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
                    MirInstr::GetIter {
                        source,
                        dest,
                        mode: _,
                        prepare,
                    } => {
                        let (source, dest) = (*source, *dest);
                        self.rewrite_get_iter(owner, function, source, dest, prepare)?;
                    }
                    instruction => {
                        for region in mojito_mir::mir::instruction_regions_mut(instruction) {
                            self.rewrite_iterator_inits(owner, function, region)?;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Fold a `GetIter` normalization chain: retarget every `prepare` step to
    /// its concrete instance, statically unroll dynamic `__trait_dispatch.`
    /// normalization (the VM repeats that step at runtime until the value has a
    /// `__next__`; the receiver is concrete here). A split iterator slot keeps
    /// the type MIR declared it with, the checked `__iter__` result; a source
    /// normalized in place takes the chain's final return type.
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
        if matches!(current.kind(), TyKind::RuntimePack(_) | TyKind::Tuple(_)) {
            function.var_tys.insert(dest, current);
            return Ok(());
        }
        // A borrowed named source binds the slot to a reference; follow it to
        // the underlying iterable type, as the VM does for name resolution.
        if let TyKind::Ref(reference) = current.kind() {
            current = reference.referent.clone();
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
        if source == dest || !function.var_tys.contains_key(&dest) {
            function.var_tys.insert(dest, current);
        }
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
        let TyKind::Struct(receiver_name, _) = receiver.kind() else {
            return Err(self.error(
                Some(owner),
                format!("iterator `{method}` operation applied to non-struct type `{receiver}`"),
            ));
        };
        let target = self.iterator_step_template(receiver_name, method, selected);
        if !self.functions.contains_key(target.as_str()) {
            return Err(self.error(
                Some(owner),
                format!("iterator method `{target}` is missing from the MIR program"),
            ));
        }
        let (bindings, arguments, result) = self.infer_receiver_call(
            owner,
            &target,
            receiver,
            ReceiverCall {
                result,
                ..ReceiverCall::default()
            },
        )?;
        let concrete = self.enqueue(&target, bindings, arguments)?;
        Ok((concrete, result))
    }

    /// Whether the nullary iterator-protocol operation `method` of
    /// `receiver` returns a reference.
    pub(super) fn iterator_step_returns_reference(
        &self,
        receiver: &Ty,
        method: &str,
        selected: Option<&str>,
    ) -> bool {
        let TyKind::Struct(receiver_name, _) = receiver.kind() else {
            return false;
        };
        let target = self.iterator_step_template(receiver_name, method, selected);
        self.declarations
            .get(target.as_str())
            .is_some_and(|declaration| declaration.returns_reference)
    }

    /// The template method a nullary iterator-protocol operation on a
    /// receiver named `receiver_name` selects.
    fn iterator_step_template(
        &self,
        receiver_name: &str,
        method: &str,
        selected: Option<&str>,
    ) -> String {
        mojito_symbol::symbol::resolve_method_symbol(
            &self.functions,
            nominal_template(receiver_name),
            method,
            selected,
            0,
        )
    }

    /// The receiver-typed sibling of [`Self::infer_call`] for method calls
    /// carried by iterator and subscript instructions, which name their
    /// receiver as a variable slot or carry no argument registers. A
    /// subscript's checked parameters (its index and stored value, past the
    /// receiver) bind the method's own parameters, those of a reduced
    /// demand's removed arguments excepted.
    pub(super) fn infer_receiver_call(
        &self,
        owner: &str,
        target: &str,
        receiver: &Ty,
        demand: ReceiverCall<'_>,
    ) -> Result<(Bindings, Vec<InstanceArg>, Ty), MonoError> {
        let declaration = self.declarations.get(target).ok_or_else(|| {
            self.error(
                Some(owner),
                format!("callee `{target}` lacks declaration facts"),
            )
        })?;
        let mut bindings = self.base_bindings();
        bindings.reduced = demand.reduced;
        bindings.reduced = self.reduces(target, &bindings);
        for (binder, value) in demand.explicit {
            if declaration
                .param_decls
                .iter()
                .any(|decl| decl.binder() == *binder)
            {
                bindings.values.insert(binder.clone(), value.clone());
            }
        }
        let mut owner_covered = 0;
        if let TyKind::Struct(receiver_name, arguments) = receiver.kind()
            && let Some(struct_decl) = self.structs.get(nominal_template(receiver_name))
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
        let removed: &[VarId] = match self.functions.get(target) {
            Some(function) if bindings.reduced => &function.removed_args,
            _ => &[],
        };
        let kept = |slot: usize| !removed.contains(&(slot as VarId));
        let patterns: Vec<&Ty> = self
            .functions
            .get(target)
            .map(|function| {
                function
                    .param_types
                    .iter()
                    .enumerate()
                    .skip(1)
                    .filter(|(slot, _)| kept(*slot))
                    .map(|(_, ty)| ty)
                    .collect()
            })
            .unwrap_or_default();
        let parameters: Vec<&Ty> = demand
            .parameters
            .iter()
            .enumerate()
            .filter(|(position, _)| kept(position + 1))
            .map(|(_, ty)| *ty)
            .collect();
        for (pattern, parameter) in parameter_patterns(&patterns, &parameters) {
            unify(pattern, parameter, &mut bindings)
                .map_err(|e| self.error(Some(owner), format!("monomorphizing `{target}`: {e}")))?;
        }
        if let Some(result) = demand.result {
            unify_result(&declaration.ret_ty, result, &mut bindings).map_err(|e| {
                self.error(
                    Some(owner),
                    format!("monomorphizing `{target}` return: {e}"),
                )
            })?;
        }
        apply_defaults(&declaration.param_decls, &mut bindings)?;
        if bindings.self_instance.is_none()
            && let TyKind::Struct(template, arguments) = peel_refs(receiver_pattern).kind()
            && !arguments.is_empty()
        {
            let concrete = substitute_ty(peel_refs(receiver_pattern), &bindings)?;
            bindings.self_instance = Some((nominal_template(template).to_string(), concrete));
            if let Some(struct_decl) = self.structs.get(nominal_template(template)) {
                owner_covered =
                    owner_covered_prefix(&struct_decl.param_decls, &declaration.param_decls);
            }
        }
        if bindings.self_instance.is_none()
            && let TyKind::Struct(receiver_name, _) = receiver.kind()
            && let Some(struct_decl) = self.structs.get(nominal_template(receiver_name))
            && !struct_decl.param_decls.is_empty()
        {
            bindings.self_instance = Some((
                nominal_template(receiver_name).to_string(),
                owner_instance_ty(
                    nominal_template(receiver_name),
                    &struct_decl.param_decls,
                    &bindings,
                )?,
            ));
            owner_covered =
                owner_covered_prefix(&struct_decl.param_decls, &declaration.param_decls);
        }
        let arguments =
            self.own_arguments(&declaration.param_decls, &bindings, target, owner_covered)?;
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
        let TyKind::Struct(name, _) = receiver.kind() else {
            return false;
        };
        let target = mojito_symbol::symbol::resolve_method_symbol(
            &self.functions,
            nominal_template(name),
            "__next__",
            None,
            0,
        );
        self.functions.contains_key(target.as_str())
    }

    /// The type arguments the checker solved for the call bind the
    /// callee's own binders first: a parameter no runtime parameter or
    /// result spells has no other source. One still symbolic here belongs
    /// to a parametric caller, which unification leaves as it stands.
    fn bind_instantiated_arguments(
        &self,
        owner: &str,
        target: &str,
        decls: &[ParamDecl],
        instantiated: &[TyArg],
        bindings: &mut Bindings,
    ) -> Result<(), MonoError> {
        for (decl, argument) in decls.iter().zip(instantiated) {
            match (decl, argument) {
                // A pack the caller forwarded whole (`show(*args)`) was
                // recorded as the caller's own pack, which substitution has
                // bound to its elements.
                (ParamDecl::Type { variadic: true, .. }, TyArg::Ty(pack))
                    if let TyKind::RuntimePack(elements) = pack.kind()
                        && !elements.iter().any(is_symbolic) =>
                {
                    bind_pack(&decl.binder(), elements.clone(), bindings);
                }
                (ParamDecl::Type { .. }, TyArg::Ty(ty)) if !is_symbolic(ty) => {
                    bind_type(&decl.binder(), ty, bindings).map_err(|e| {
                        self.error(Some(owner), format!("monomorphizing `{target}`: {e}"))
                    })?;
                }
                // A value the checker solved from an argument's type (`dt`
                // from a `Scalar[dt]` parameter) binds here, as does an
                // expression the caller's bindings close (an applied local
                // constant); one still over the caller's binders is
                // unification's.
                (
                    ParamDecl::Value {
                        variadic: false, ..
                    },
                    TyArg::Val(value),
                ) if !matches!(
                    value,
                    CtValue::Deferred(_) | CtValue::Marker(_) | CtValue::RemovedArg
                ) =>
                {
                    let closed = match value {
                        CtValue::Expr(expr) => match eval_ct(expr, &self.enclosing) {
                            Ok(value) => value,
                            Err(_) => continue,
                        },
                        value => value.clone(),
                    };
                    bind_value(&decl.binder(), &closed, bindings).map_err(|e| {
                        self.error(Some(owner), format!("monomorphizing `{target}`: {e}"))
                    })?;
                }
                // A value pack's solution is the list of values the checker
                // recorded, each closed under the caller's bindings.
                (ParamDecl::Value { variadic: true, .. }, TyArg::Val(CtValue::Tuple(elements))) => {
                    let elements = elements
                        .iter()
                        .map(|element| match element {
                            CtValue::Expr(expr) => eval_ct(expr, &self.enclosing).ok(),
                            CtValue::Deferred(_) | CtValue::Marker(_) => None,
                            value => Some(value.clone()),
                        })
                        .collect::<Option<Vec<CtValue>>>();
                    if let Some(elements) = elements {
                        bindings
                            .values
                            .insert(decl.binder(), CtValue::Tuple(elements));
                    }
                }
                // A type pack's solution is the element list the checker
                // recorded, bound whole.
                (ParamDecl::Type { variadic: true, .. }, TyArg::Val(CtValue::Tuple(elements))) => {
                    // The checker spells each element as the template
                    // application; the instance names its own symbol.
                    let elements = elements
                        .iter()
                        .map(|element| match element {
                            CtValue::Type(ty) if !is_symbolic(ty) => {
                                substitute_ty(ty, bindings).ok()
                            }
                            _ => None,
                        })
                        .collect::<Option<Vec<Ty>>>();
                    if let Some(elements) = elements {
                        bind_pack(&decl.binder(), elements, bindings);
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
}

/// Each of a method's parameter `patterns` beside the checked parameter it
/// binds. A homogeneous variadic (`*indices: Int`) binds every parameter
/// between the ones before it and the ones after it (a keyword-only
/// `value`), each at its element type.
fn parameter_patterns<'a>(patterns: &[&'a Ty], parameters: &[&'a Ty]) -> Vec<(&'a Ty, &'a Ty)> {
    let Some((variadic, element)) =
        patterns
            .iter()
            .enumerate()
            .find_map(|(position, pattern)| match pattern.kind() {
                TyKind::VariadicPack(element) => Some((position, element)),
                _ => None,
            })
    else {
        return patterns
            .iter()
            .copied()
            .zip(parameters.iter().copied())
            .collect();
    };
    let trailing = patterns.len() - variadic - 1;
    let Some(collected) = parameters.len().checked_sub(variadic + trailing) else {
        return Vec::new();
    };
    let (leading, rest) = parameters.split_at(variadic);
    let (middle, after) = rest.split_at(collected);
    patterns[..variadic]
        .iter()
        .copied()
        .zip(leading.iter().copied())
        .chain(middle.iter().map(|parameter| (element, *parameter)))
        .chain(
            patterns[variadic + 1..]
                .iter()
                .copied()
                .zip(after.iter().copied()),
        )
        .collect()
}
