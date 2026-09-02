//! The specialization driver: the worklist, instance naming and
//! materialization, block rewriting, and struct discovery.

use super::*;

impl<'a> Specializer<'a> {
    pub(super) fn new(source: &'a MirProgram) -> Self {
        Self {
            source,
            functions: source
                .functions
                .iter()
                .map(|(n, f)| (n.as_str(), f))
                .collect(),
            declarations: source
                .declarations
                .functions
                .iter()
                .map(|d| (d.lowered_name.as_str(), d))
                .collect(),
            structs: source
                .declarations
                .structs
                .iter()
                .map(|d| (d.name.as_str(), d))
                .collect(),
            generic_templates: Rc::new(
                source
                    .declarations
                    .structs
                    .iter()
                    .filter(|d| !d.param_decls.is_empty())
                    .map(|d| d.name.clone())
                    .collect(),
            ),
            queue: VecDeque::new(),
            instances: Vec::new(),
            output_functions: Vec::new(),
            output_function_decls: Vec::new(),
            output_structs: Vec::new(),
            constant_values: HashMap::new(),
            callable_targets: HashMap::new(),
        }
    }

    pub(super) fn run(mut self, entries: &[String]) -> Result<SpecializedProgram, MonoError> {
        let mut entry_map = HashMap::new();
        for entry in entries {
            let decl = self.declarations.get(entry.as_str()).copied();
            self.functions.get(entry.as_str()).copied().ok_or_else(|| {
                self.error(
                    None,
                    format!("entry function `{entry}` (not found in the MIR program)"),
                )
            })?;
            if decl.is_some_and(|decl| !decl.param_decls.is_empty()) {
                return Err(self.error(
                    Some(entry),
                    format!("generic entry `{entry}` has unresolved parameters"),
                ));
            }
            let name = self.enqueue(entry, self.base_bindings(), Vec::new())?;
            entry_map.insert(entry.clone(), name);
        }
        while let Some((key, bindings)) = self.queue.pop_front() {
            if self
                .output_functions
                .iter()
                .any(|(name, _)| name == self.instance_name(&key))
            {
                continue;
            }
            if self.output_functions.len() >= 4096 {
                return Err(self.error(
                    Some(&key.template),
                    "polymorphic recursion exceeded the 4096-instance budget",
                ));
            }
            self.materialize(key, bindings)?;
        }
        let function_order = self
            .source
            .functions
            .iter()
            .enumerate()
            .map(|(index, (name, _))| (name.as_str(), index))
            .collect::<HashMap<_, _>>();
        let instance_templates = self
            .instances
            .iter()
            .map(|(key, name)| (name.as_str(), key.template.as_str()))
            .collect::<HashMap<_, _>>();
        self.output_functions.sort_by_key(|(name, _)| {
            let template = instance_templates
                .get(name.as_str())
                .copied()
                .unwrap_or(name);
            function_order.get(template).copied().unwrap_or(usize::MAX)
        });
        self.output_function_decls.sort_by_key(|decl| {
            let template = instance_templates
                .get(decl.lowered_name.as_str())
                .copied()
                .unwrap_or(decl.lowered_name.as_str());
            function_order.get(template).copied().unwrap_or(usize::MAX)
        });
        Ok(SpecializedProgram {
            program: MirProgram {
                functions: self.output_functions,
                declarations: MirDeclarations {
                    structs: self.output_structs,
                    functions: self.output_function_decls,
                },
                invariant_errors: self.source.invariant_errors.clone(),
            },
            entries: entry_map,
        })
    }

    pub(super) fn base_bindings(&self) -> Bindings {
        Bindings {
            generic_templates: Rc::clone(&self.generic_templates),
            ..Bindings::default()
        }
    }

    pub(super) fn enqueue(
        &mut self,
        template: &str,
        bindings: Bindings,
        arguments: Vec<InstanceArg>,
    ) -> Result<String, MonoError> {
        let owner = bindings.self_instance.as_ref().and_then(|(_, ty)| {
            if let Ty::Struct(name, _) = ty {
                Some(name.clone())
            } else {
                None
            }
        });

        let key = InstanceKey {
            template: template.to_string(),
            arguments,
            owner,
        };
        if let Some((_, name)) = self.instances.iter().find(|(known, _)| known == &key) {
            return Ok(name.clone());
        }
        // A generic struct's method takes its concrete owner's spelling
        // (`List$mono$TInt.grow`), so lowering's name-composed lifecycle and
        // overload lookups against the instance struct name keep working.
        let name = if let Some(owner) = &key.owner {
            let base = crate::symbol::retarget_method_symbol(template, owner).ok_or_else(|| {
                self.error(
                    Some(template),
                    format!("owner-bound instance `{template}` is not a method symbol"),
                )
            })?;
            if key.arguments.is_empty() {
                base
            } else {
                crate::symbol::instance_symbol(&base, &key.arguments)
            }
        } else if key.arguments.is_empty() {
            template.to_string()
        } else {
            crate::symbol::instance_symbol(template, &key.arguments)
        };
        if (name != template && self.functions.contains_key(name.as_str()))
            || self.instances.iter().any(|(_, n)| n == &name)
        {
            return Err(self.error(
                Some(template),
                format!("concrete instance symbol `{name}` collides with an existing declaration"),
            ));
        }
        self.instances.push((key.clone(), name.clone()));
        self.queue.push_back((key, bindings));
        Ok(name)
    }

    pub(super) fn instance_name(&self, key: &InstanceKey) -> &str {
        self.instances
            .iter()
            .find(|(known, _)| known == key)
            .expect("queued instance has identity")
            .1
            .as_str()
    }

    pub(super) fn materialize(
        &mut self,
        key: InstanceKey,
        bindings: Bindings,
    ) -> Result<(), MonoError> {
        let name = self.instance_name(&key).to_string();
        let mut function = self
            .functions
            .get(key.template.as_str())
            .copied()
            .ok_or_else(|| {
                self.error(
                    Some(&key.template),
                    format!("callee `{}` has no MIR body", key.template),
                )
            })?
            .clone();
        substitute_function(&mut function, &bindings).map_err(|mut e| {
            e.function.get_or_insert_with(|| key.template.clone());
            e
        })?;
        self.constant_values = function_constant_values(&function);
        self.callable_targets = function_callable_targets(&function);
        self.constant_values.extend(
            self.callable_targets
                .iter()
                .map(|(reg, (target, _))| (*reg, CtValue::Str(target.clone()))),
        );
        // Take the blocks out so call rewriting can read the function's
        // substituted register-type table without aliasing its body.
        let mut blocks = std::mem::take(&mut function.blocks);
        // Iterator normalization first: block order does not put every
        // `GetIter` before the `HasNext`/`Next`/`TryNext` that reads its
        // destination (comprehension loops interleave), and the advance
        // rewrites need the iterator slot types this pass records.
        self.rewrite_iterator_inits(&key.template, &mut function, &mut blocks)?;
        self.rewrite_blocks(&key.template, &mut function, &mut blocks)?;
        function.blocks = blocks;
        repair_storage_result_types(&mut function);
        erase_specialized_generic_callable_storage(&mut function);
        ensure_concrete_function(&key.template, &function)?;

        if let Some(declaration) = self.declarations.get(key.template.as_str()).copied() {
            let mut declaration = declaration.clone();
            substitute_declaration(&mut declaration, &bindings)?;
            declaration.lowered_name = name.clone();
            declaration.param_decls.clear();
            self.output_function_decls.push(declaration);
        }
        self.discover_structs(&key.template, &function)?;
        self.output_functions.push((name, function));
        Ok(())
    }

    pub(super) fn rewrite_blocks(
        &mut self,
        owner: &str,
        function: &mut MirFunction,
        blocks: &mut [MirBlock],
    ) -> Result<(), MonoError> {
        for block in blocks {
            for instruction in &mut block.instrs {
                if let MirInstr::Try {
                    body,
                    handler,
                    orelse,
                    finalbody,
                    ..
                } = instruction
                {
                    self.rewrite_blocks(owner, function, body)?;
                    if let Some((_, blocks)) = handler {
                        self.rewrite_blocks(owner, function, blocks)?;
                    }
                    if let Some(blocks) = orelse {
                        self.rewrite_blocks(owner, function, blocks)?;
                    }
                    if let Some(blocks) = finalbody {
                        self.rewrite_blocks(owner, function, blocks)?;
                    }
                    continue;
                }
                // `len`/`abs`/`round` on a nominal receiver are checker-typed
                // dunder dispatches the VM performs by name (`call_dunder`);
                // rewrite them into ordinary method calls so the shared
                // resolver and the `MethodCall` arm below monomorphize the
                // dunder instance like any other method. Non-struct operands
                // pass through for the backend's scalar interception.
                if let MirInstr::Call {
                    dest,
                    func,
                    args,
                    kwargs,
                    ..
                } = instruction
                {
                    let dunder = match func.0.as_str() {
                        "len" => Some("__len__"),
                        "abs" => Some("__abs__"),
                        "round" => Some("__round__"),
                        // Conversion builtins over a nominal receiver are the
                        // same VM dunder dispatch (`builtin_convert`'s struct
                        // arm).
                        "Int" => Some("__int__"),
                        "Float64" => Some("__float__"),
                        "Bool" => Some("__bool__"),
                        _ => None,
                    };
                    if let Some(method) = dunder
                        && !self.functions.contains_key(func.0.as_str())
                        && kwargs.is_empty()
                        && args.len() == 1
                        && matches!(
                            function.reg_types.get(&args[0].0).map(peel_refs),
                            Some(Ty::Struct(..))
                        )
                    {
                        *instruction = dunder_method_call(*dest, args[0], method, None, Vec::new());
                    }
                }
                // A binary operator on a nominal left operand is the same VM
                // dunder dispatch (`apply_binop` → `call_dunder`): rewrite to
                // the operator method so the shared resolver monomorphizes
                // the compiled instance (`String.__add__`, user `__eq__`, …).
                // `in`/`not in` dispatch on the right operand and stay
                // untouched (they keep their contextual rejection).
                if let MirInstr::BinOp {
                    op,
                    dest,
                    a,
                    b,
                    resolved,
                } = instruction
                    && let Some(method) = op.dunder()
                    && !matches!(op, crate::ast::InfixOp::In | crate::ast::InfixOp::NotIn)
                    && matches!(
                        function.reg_types.get(&a.0).map(peel_refs),
                        Some(Ty::Struct(..))
                    )
                {
                    *instruction = dunder_method_call(*dest, *a, method, resolved.take(), vec![*b]);
                }
                match instruction {
                    MirInstr::Call {
                        dest,
                        func,
                        args,
                        kwargs,
                        param_arg_regs,
                        ..
                    } => {
                        if !self.functions.contains_key(func.0.as_str()) {
                            // `print` of a nominal struct displays through
                            // `write_to` over the builtin-string writer (the
                            // VM's `format_value` dispatch); enqueue the
                            // instances the lowered expansion calls.
                            if matches!(func.0.as_str(), "print" | "String") {
                                for arg in args.clone() {
                                    self.enqueue_display_instance(owner, function, arg)?;
                                }
                            }
                            if self.structs.contains_key(func.0.as_str())
                                && !crate::symbol::is_stdlib_string_struct(&func.0)
                            {
                                // `Type(copy=value)` runs `__copyinit__` (the
                                // VM's `construct_via_copy`), which struct
                                // discovery enqueues per instance — never an
                                // `__init__` contract.
                                let copy_form =
                                    args.is_empty() && kwargs.len() == 1 && kwargs[0].0 == "copy";
                                if copy_form {
                                    if let Some(Ty::Struct(concrete, _)) =
                                        function.reg_types.get(&dest.0)
                                        && concrete != &func.0
                                        && nominal_template(concrete) == func.0.as_str()
                                    {
                                        func.0 = concrete.clone();
                                    }
                                    continue;
                                }
                                let init_base = format!("{}.__init__", func.0);
                                let init = crate::symbol::resolve_callable_symbol(
                                    self.functions.iter().map(|(name, f)| CallableCandidate {
                                        name,
                                        n_params: f.n_params,
                                    }),
                                    &init_base,
                                    args.len() + kwargs.len(),
                                );
                                if self.functions.contains_key(init.as_str()) {
                                    let (target, bindings, arguments) = self.infer_call(
                                        owner,
                                        function,
                                        &init,
                                        Some(*dest),
                                        *dest,
                                        args,
                                        kwargs,
                                        param_arg_regs,
                                    )?;
                                    if let Some((_, concrete)) = &bindings.self_instance {
                                        function.reg_types.insert(dest.0, concrete.clone());
                                    }
                                    self.enqueue(&target, bindings, arguments)?;
                                    // The instance identity now carries every
                                    // compile-time solution; the call-site
                                    // value registers are redundant (a body
                                    // that still needs one fails its own
                                    // contextual check).
                                    for param_arg in param_arg_regs.iter_mut() {
                                        param_arg.value = None;
                                    }
                                }
                                // A generic struct's output declaration is
                                // instance-named; respell the constructor
                                // call so lowering's struct lookup matches.
                                if let Some(Ty::Struct(concrete, _)) =
                                    function.reg_types.get(&dest.0)
                                    && concrete != &func.0
                                    && nominal_template(concrete) == func.0.as_str()
                                {
                                    func.0 = concrete.clone();
                                }
                                for param_arg in param_arg_regs.iter_mut() {
                                    param_arg.value = None;
                                }
                            }
                            continue;
                        }
                        // A direct constructor call's destination is its
                        // `out self`, not the declared `None` return — bind
                        // it as the receiver.
                        let receiver = (func.0.contains(".__init__")
                            && function.reg_types.contains_key(&dest.0))
                        .then_some(*dest);
                        let (target, bindings, arguments) = self.infer_call(
                            owner,
                            function,
                            &func.0,
                            receiver,
                            *dest,
                            args,
                            kwargs,
                            param_arg_regs,
                        )?;
                        func.0 = self.enqueue(&target, bindings, arguments)?;
                        for param_arg in param_arg_regs.iter_mut() {
                            param_arg.value = None;
                        }
                    }
                    MirInstr::MakeSimd { elems, .. } => {
                        for elem in elems.clone() {
                            self.enqueue_intable_instance(owner, function, elem)?;
                        }
                    }
                    MirInstr::MethodCall {
                        dest,
                        recv,
                        method,
                        resolved,
                        args,
                        kwargs,
                        param_arg_regs,
                        ..
                    } => {
                        let receiver = function.reg_types.get(&recv.0).ok_or_else(|| {
                            self.error(Some(owner), "method receiver lacks a MIR type")
                        })?;
                        // A scalar/literal Hashable leaf contributes to the
                        // hasher through the hasher's compiled
                        // `_update_with_simd` (a literal through the nominal
                        // String's `__hash__`); enqueue those instances for
                        // the lowered leaf dispatch.
                        if method == "__hash__"
                            && args.len() == 1
                            && kwargs.is_empty()
                            && !matches!(peel_refs(receiver), Ty::Struct(..))
                        {
                            let receiver = peel_refs(receiver).clone();
                            self.enqueue_hash_leaf_instances(owner, function, args[0], &receiver)?;
                        }
                        if resolved
                            .as_deref()
                            .is_some_and(|target| target.starts_with("__trait_dispatch."))
                            && !matches!(peel_refs(receiver), Ty::Struct(..))
                        {
                            *resolved = None;
                            continue;
                        }
                        // `write` on the builtin-string accumulator (the
                        // `Value::Str` writer inside a `write_to` expansion)
                        // formats nominal arguments through their own
                        // `write_to` conformance — enqueue those instances
                        // for the lowered recursion.
                        if method == "write" && matches!(peel_refs(receiver), Ty::StringLiteral) {
                            for arg in args.clone() {
                                self.enqueue_display_instance(owner, function, arg)?;
                            }
                            continue;
                        }
                        // A borrowed receiver dispatches on its referent, as
                        // the VM dereferences `Value::Ref` receivers.
                        let Ty::Struct(receiver_name, _) = peel_refs(receiver) else {
                            continue;
                        };
                        // Source methods are declared under the template name;
                        // an instance-named receiver (`List$mono$TInt`) still
                        // resolves against `List.*` and gets its instance
                        // identity from `infer_call`'s receiver binding.
                        let target = crate::symbol::resolve_method_symbol(
                            self.functions.iter().map(|(name, f)| CallableCandidate {
                                name,
                                n_params: f.n_params,
                            }),
                            nominal_template(receiver_name),
                            method,
                            resolved.as_deref(),
                            args.len() + kwargs.len(),
                        );
                        if !self.functions.contains_key(target.as_str()) {
                            // The VM-synthesized `Writer.write` dispatch calls
                            // the receiver's `write_string`; enqueue its
                            // instance for the lowered expansion.
                            if method == "write" {
                                let write_string = crate::symbol::resolve_callable_symbol(
                                    self.functions.iter().map(|(name, f)| CallableCandidate {
                                        name,
                                        n_params: f.n_params,
                                    }),
                                    &format!("{}.write_string", nominal_template(receiver_name)),
                                    1,
                                );
                                if self.functions.contains_key(write_string.as_str()) {
                                    let receiver_ty = peel_refs(receiver).clone();
                                    let (bindings, arguments, _) = self.infer_receiver_call(
                                        owner,
                                        &write_string,
                                        &receiver_ty,
                                        None,
                                    )?;
                                    self.enqueue(&write_string, bindings, arguments)?;
                                }
                            }
                            continue;
                        }
                        let (target, bindings, arguments) = self.infer_call(
                            owner,
                            function,
                            &target,
                            Some(*recv),
                            *dest,
                            args,
                            kwargs,
                            param_arg_regs,
                        )?;
                        let concrete = self.enqueue(&target, bindings, arguments)?;
                        *resolved = Some(concrete);
                        for param_arg in param_arg_regs.iter_mut() {
                            param_arg.value = None;
                        }
                    }
                    // An indirect call whose callee is a nominal callable
                    // struct devirtualizes into a direct `__call__` method
                    // call — the VM's `runtime_method_name` dispatch, made
                    // static — so the ordinary method lowering (mut-receiver
                    // write-back, outcome, sret) serves it. Func-typed
                    // callees keep the instruction and lower through their
                    // two-word `{invoke, env}` value.
                    MirInstr::CallIndirect {
                        dest,
                        callee,
                        raises,
                        args,
                        kwargs,
                        callee_place,
                        arg_places,
                        kwarg_places,
                        capture_accesses,
                        param_arg_regs,
                        resolved,
                        ..
                    } => {
                        let dependent_callable =
                            function.reg_types.get(&callee.0).is_some_and(|ty| {
                                matches!(
                                    peel_refs(ty),
                                    Ty::GenericFunc { .. }
                                        | Ty::Param {
                                            callable_bound: Some(_),
                                            ..
                                        }
                                )
                            });
                        if let Some((target, captures_are_empty)) =
                            self.callable_targets.get(&callee.0).cloned()
                            && (captures_are_empty || dependent_callable)
                        {
                            if !captures_are_empty {
                                return Err(self.error(
                                    Some(owner),
                                    format!("generic retained callable `{target}` has captures"),
                                ));
                            }
                            let (target, bindings, arguments) = self.infer_call(
                                owner,
                                function,
                                &target,
                                None,
                                *dest,
                                args,
                                kwargs,
                                param_arg_regs,
                            )?;
                            let concrete = self.enqueue(&target, bindings, arguments)?;
                            *instruction = MirInstr::Call {
                                dest: *dest,
                                func: crate::mir::FuncRef(concrete),
                                raises: raises.clone(),
                                args: std::mem::take(args),
                                kwargs: std::mem::take(kwargs),
                                arg_places: std::mem::take(arg_places),
                                kwarg_places: std::mem::take(kwarg_places),
                                capture_accesses: std::mem::take(capture_accesses),
                                param_arg_regs: Vec::new(),
                            };
                            continue;
                        }
                        let Some(receiver) = function.reg_types.get(&callee.0) else {
                            continue;
                        };
                        let Ty::Struct(receiver_name, _) = peel_refs(receiver) else {
                            continue;
                        };
                        let target = crate::symbol::resolve_method_symbol(
                            self.functions.iter().map(|(name, f)| CallableCandidate {
                                name,
                                n_params: f.n_params,
                            }),
                            nominal_template(receiver_name),
                            "__call__",
                            resolved.as_deref(),
                            args.len() + kwargs.len(),
                        );
                        if !self.functions.contains_key(target.as_str()) {
                            continue;
                        }
                        let (target, bindings, arguments) = self.infer_call(
                            owner,
                            function,
                            &target,
                            Some(*callee),
                            *dest,
                            args,
                            kwargs,
                            param_arg_regs,
                        )?;
                        let concrete = self.enqueue(&target, bindings, arguments)?;
                        *instruction = MirInstr::MethodCall {
                            dest: *dest,
                            recv: *callee,
                            method: "__call__".to_string(),
                            resolved: Some(concrete),
                            raises: raises.clone(),
                            reference_result: None,
                            result_adapter: None,
                            args: std::mem::take(args),
                            kwargs: std::mem::take(kwargs),
                            recv_place: callee_place.take(),
                            recv_writes: true,
                            arg_places: std::mem::take(arg_places),
                            kwarg_places: std::mem::take(kwarg_places),
                            capture_accesses: Vec::new(),
                            param_arg_regs: std::mem::take(param_arg_regs),
                            param_decls: Vec::new(),
                        };
                    }
                    // A retained callable names its lifted body on the
                    // instruction; enqueue it so the reachable graph carries
                    // the compiled target the thunk will call. Lifted bodies
                    // are monomorphic in the supported subset — one whose
                    // signature still spells generic parameters (a lambda
                    // inside an unspecialized generic) rejects contextually.
                    MirInstr::MakeClosure {
                        function: target, ..
                    }
                    | MirInstr::Const {
                        k: Const::Function(target),
                        ..
                    } => {
                        let Some(body) = self.functions.get(target.as_str()).copied() else {
                            continue;
                        };
                        if function_types(body).any(is_symbolic) {
                            continue;
                        }
                        *target = self.enqueue(target, self.base_bindings(), Vec::new())?;
                    }
                    // A value-parameter read (`Self.length`) resolves to
                    // the bound constant carried by the receiver instance's
                    // type arguments — the VM's `get_field` value-parameter
                    // fallback over reified `value_params`.
                    MirInstr::GetField { dest, base, field } => {
                        let Some(receiver) = function.reg_types.get(&base.0) else {
                            continue;
                        };
                        let Some(constant) = self.value_param_constant(receiver, field) else {
                            continue;
                        };
                        *instruction = MirInstr::Const {
                            dest: *dest,
                            k: constant,
                        };
                    }
                    MirInstr::LoadPlace { dest, place }
                        if place.proj.len() == 1
                            && matches!(&place.proj[0], crate::mir::Proj::Field(_)) =>
                    {
                        let crate::mir::Proj::Field(field) = &place.proj[0] else {
                            continue;
                        };
                        let Some(receiver) = function.var_tys.get(&place.root) else {
                            continue;
                        };
                        let Some(constant) = self.value_param_constant(receiver, field) else {
                            continue;
                        };
                        *instruction = MirInstr::Const {
                            dest: *dest,
                            k: constant,
                        };
                    }
                    // Checker-selected subscript invocations retarget to
                    // their concrete instances exactly like method calls;
                    // intrinsic storage subscripts carry no nominal call and
                    // pass through.
                    MirInstr::Index {
                        dest,
                        base,
                        call: Some(call),
                        ..
                    } => {
                        let (dest, base) = (*dest, *base);
                        self.rewrite_subscript_call(owner, function, base, Some(dest), call)?;
                    }
                    MirInstr::Slice {
                        dest,
                        object,
                        call: Some(call),
                        ..
                    }
                    | MirInstr::MultiIndex {
                        dest,
                        object,
                        call: Some(call),
                        ..
                    } => {
                        let (dest, object) = (*dest, *object);
                        self.rewrite_subscript_call(owner, function, object, Some(dest), call)?;
                    }
                    MirInstr::MultiSet { receiver, call, .. } => {
                        let receiver = *receiver;
                        self.rewrite_subscript_call(owner, function, receiver, None, call)?;
                    }
                    // An untyped iterator slot passes through: it belongs to
                    // a compiler-private pack loop the backend rejects at its
                    // own boundary.
                    MirInstr::HasNext {
                        iter,
                        method: Some(method),
                        ..
                    } => {
                        if let Some(receiver) = function.var_tys.get(iter).cloned() {
                            let (target, _) = self.resolve_iterator_step(
                                owner,
                                &receiver,
                                "__len__",
                                Some(method),
                                None,
                            )?;
                            *method = target;
                        }
                    }
                    MirInstr::Next {
                        iter,
                        call: Some(call),
                        ..
                    }
                    | MirInstr::TryNext { iter, call, .. } => {
                        if let Some(receiver) = function.var_tys.get(iter).cloned() {
                            let (target, _) = self.resolve_iterator_step(
                                owner,
                                &receiver,
                                "__next__",
                                Some(&call.target),
                                Some(&call.result_ty),
                            )?;
                            call.target = target;
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    pub(super) fn discover_structs(
        &mut self,
        owner: &str,
        function: &MirFunction,
    ) -> Result<(), MonoError> {
        let mut types = function_types(function).cloned().collect::<Vec<_>>();
        // Storage take/destroy intrinsics name their element type directly on
        // the instruction; seed it so the element's lifecycle methods
        // (notably `__deinit__` for the destroy forms) always join the walk
        // even when no register or variable carries the bare element type.
        push_instruction_types(&function.blocks, &mut types);
        while let Some(ty) = types.pop() {
            collect_nested_types(&ty, &mut types);
            let Ty::Struct(name, arguments) = ty else {
                continue;
            };
            // The checker-virtual slice descriptors have no source template;
            // give them the backend's raw layout (three i64 bounds plus a
            // presence bitmask — the VM's `Value::Slice` `Option<i64>` fields)
            // so descriptor-typed parameters and locals lay out.
            if matches!(name.as_str(), "Slice" | "ContiguousSlice" | "StridedSlice") {
                if !self.output_structs.iter().any(|decl| decl.name == name) {
                    self.output_structs.push(MirStructDeclaration {
                        name,
                        fields: vec![
                            ("start".to_string(), Ty::Int),
                            ("end".to_string(), Ty::Int),
                            ("step".to_string(), Ty::Int),
                            ("flags".to_string(), Ty::Int),
                        ],
                        mut_self_methods: Default::default(),
                        fieldwise_init: false,
                        param_decls: Vec::new(),
                        explicit_destroy_message: None,
                        explicit_destructors: Default::default(),
                    });
                }
                continue;
            }
            let template_name = name.split("$mono").next().unwrap_or(&name).to_string();
            let Some(template) = self.structs.get(template_name.as_str()).copied() else {
                continue;
            };
            if arguments.len() < template.param_decls.len() {
                continue;
            }
            let mut bindings = self.base_bindings();
            bind_ty_args(&template.param_decls, &arguments, &mut bindings).map_err(|e| {
                self.error(
                    Some(owner),
                    format!("monomorphizing struct `{template_name}`: {e}"),
                )
            })?;
            if name != template_name {
                bindings.self_instance = Some((
                    template_name.clone(),
                    Ty::Struct(name.clone(), arguments.clone()),
                ));
            }
            let mut declaration = template.clone();
            for (_, field) in &mut declaration.fields {
                *field = substitute_ty(field, &bindings)?;
            }
            declaration.name = name;
            declaration.param_decls.clear();
            // Overload-qualified `mut self` entries name the template
            // (for example, a signature-qualified `List.pop`); respell them
            // under the instance so
            // lowering's receiver write-back check matches the retargeted
            // method symbols. Bare method-name entries stay as they are.
            declaration.mut_self_methods = declaration
                .mut_self_methods
                .iter()
                .map(|entry| {
                    if entry.contains('.') {
                        crate::symbol::retarget_method_symbol(entry, &declaration.name)
                            .unwrap_or_else(|| entry.clone())
                    } else {
                        entry.clone()
                    }
                })
                .collect();
            if let Some(existing) = self
                .output_structs
                .iter()
                .find(|decl| decl.name == declaration.name)
            {
                // Output declarations dedupe by name, but a checker-concrete
                // generic application keeps its template name — two distinct
                // instantiations would silently share whichever declaration
                // was discovered first. Sharing is benign only when the field
                // substitutions are equivalent modulo pointer element types
                // (every pointer is one opaque target word and drops inertly
                // — the `_RawAlloc`/`List` shape); anything else rejects
                // contextually instead of laying out against the wrong
                // instance. Renaming concrete applications to instance
                // symbols is the Collections slice's canonicalization
                // prerequisite.
                if !fields_equivalent(&existing.fields, &declaration.fields) {
                    return Err(self.error(
                        Some(owner),
                        format!(
                            "struct instance `{}` has conflicting field \
                             substitutions (instance identity collision): {:?} versus {:?}",
                            declaration.name, existing.fields, declaration.fields
                        ),
                    ));
                }
                continue;
            }
            types.extend(declaration.fields.iter().map(|(_, ty)| ty.clone()));
            self.output_structs.push(declaration);
            for method in ["__init__", "__copyinit__", "__moveinit__", "__deinit__"] {
                if method == "__copyinit__"
                    && crate::symbol::is_stdlib_string_struct(&template_name)
                {
                    continue;
                }
                let base = format!("{template_name}.{method}");
                let candidates = self
                    .functions
                    .iter()
                    .filter(|(candidate, _)| {
                        **candidate == base || crate::symbol::is_overload_of(candidate, &base)
                    })
                    .map(|(candidate, _)| (*candidate).to_string())
                    .collect::<Vec<_>>();
                for candidate in candidates {
                    let Some(function_decl) = self.declarations.get(candidate.as_str()).copied()
                    else {
                        continue;
                    };
                    // An unspecialized variadic overload cannot materialize
                    // without a call-site arity; those sites enqueue it.
                    if matches!(&function_decl.variadic, Some(element)
                        if !matches!(element, Ty::RuntimePack(_) | Ty::Tuple(_)))
                    {
                        continue;
                    }
                    let Ok(mut method_arguments) =
                        ordered_arguments(&function_decl.param_decls, &bindings, &candidate)
                    else {
                        continue;
                    };
                    if bindings.self_instance.is_some() {
                        let covered =
                            owner_covered_prefix(&template.param_decls, &function_decl.param_decls);
                        method_arguments.drain(..covered);
                    }
                    self.enqueue(&candidate, bindings.clone(), method_arguments)?;
                }
            }
        }
        Ok(())
    }

    pub(super) fn error(&self, function: Option<&str>, construct: impl Into<String>) -> MonoError {
        MonoError {
            function: function.map(str::to_string),
            construct: construct.into(),
        }
    }
}
