//! The specialization driver: the worklist, instance naming and
//! materialization, block rewriting, and struct discovery.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
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
            closure_captures: HashMap::new(),
            enclosing: Bindings::default(),
            folded_slots: HashSet::new(),
            speculative: HashSet::new(),
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
            let name = self.instance_name(&key).to_string();
            if !self.speculative.contains(&name) {
                self.materialize(&key, &bindings)?;
                continue;
            }
            // A discovery-only constructor instance that cannot materialize
            // is not part of the program: forget it (a later call site would
            // re-enqueue and report the failure in its own context).
            let functions = self.output_functions.len();
            let decls = self.output_function_decls.len();
            if self.materialize(&key.clone(), &bindings).is_err() {
                self.output_functions.truncate(functions);
                self.output_function_decls.truncate(decls);
                self.instances.retain(|(known, _)| known != &key);
            }
            self.speculative.remove(&name);
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
        let program = MirProgram {
            functions: self.output_functions,
            declarations: MirDeclarations {
                structs: self.output_structs,
                functions: self.output_function_decls,
            },
            invariant_errors: self.source.invariant_errors.clone(),
        };
        let program = ConcreteMir::verified(program).map_err(|findings| MonoError {
            function: None,
            construct: format!(
                "specialized MIR that does not verify: {}",
                findings.join("; ")
            ),
        })?;
        let parametric_bodies = parametric_bodies(self.source);
        let emitted: HashSet<&str> = program
            .functions
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();
        let instance_templates: Vec<&str> = self
            .instances
            .iter()
            .filter(|(key, name)| {
                emitted.contains(name.as_str()) && parametric_bodies.contains(key.template.as_str())
            })
            .map(|(key, _)| key.template.as_str())
            .collect();
        let parametric = ParametricInstances {
            bodies: parametric_bodies.len(),
            reached: instance_templates.iter().collect::<HashSet<_>>().len(),
            instances: instance_templates.len(),
        };
        Ok(SpecializedProgram {
            program,
            entries: entry_map,
            parametric,
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
        self.enqueue_with(template, bindings, arguments, false)
    }

    /// `enqueue` for struct discovery's eager `__init__` walk: a fresh
    /// instance is speculative (see `Specializer::speculative`); an instance
    /// some call site already demanded stays firm, and a later call-site
    /// `enqueue` of a speculative instance makes it firm.
    fn enqueue_speculative(
        &mut self,
        template: &str,
        bindings: Bindings,
        arguments: Vec<InstanceArg>,
    ) -> Result<String, MonoError> {
        self.enqueue_with(template, bindings, arguments, true)
    }

    fn enqueue_with(
        &mut self,
        template: &str,
        bindings: Bindings,
        arguments: Vec<InstanceArg>,
        speculative: bool,
    ) -> Result<String, MonoError> {
        let owner = bindings.self_instance.as_ref().and_then(|(_, ty)| {
            if let Ty::Struct(name, _) = ty {
                Some(name.clone())
            } else {
                None
            }
        });

        // Origins erase from the runtime ABI and from the instance symbol, so
        // two arguments differing only in an origin name the same native
        // instance; keying on them would mint a second instance under the same
        // symbol and collide with the first.
        let key = InstanceKey {
            template: template.to_string(),
            arguments: arguments
                .into_iter()
                .map(|argument| match argument {
                    InstanceArg::Ty(ty) => {
                        InstanceArg::Ty(mojito_types::types::erase_origin_arguments(&ty))
                    }
                    value @ InstanceArg::Value(_) => value,
                })
                .collect(),
            owner,
        };
        if let Some((_, name)) = self.instances.iter().find(|(known, _)| known == &key) {
            let name = name.clone();
            if !speculative {
                self.speculative.remove(&name);
            }
            return Ok(name);
        }
        // A generic struct's method takes its concrete owner's spelling
        // (`List$mono$TInt.grow`), so lowering's name-composed lifecycle and
        // overload lookups against the instance struct name keep working.
        let name =
            if let Some(name) = lifecycle_clone_instance_symbol(template, key.owner.as_deref()) {
                // A per-instantiation lifecycle clone is the instance's lifecycle
                // body: it takes the plain symbol lowering composes by name
                // (`Box$mono$TInt.__deinit__`), whichever site enqueues it first.
                // A variadic initializer's clone is keyed by each call's pack
                // length and only ever reached through its call sites.
                if key.arguments.is_empty() {
                    name
                } else {
                    mojito_symbol::symbol::instance_symbol(&name, &key.arguments)
                }
            } else if let Some(owner) = &key.owner {
                let base = mojito_symbol::symbol::retarget_method_symbol(template, owner)
                    .ok_or_else(|| {
                        self.error(
                            Some(template),
                            format!("owner-bound instance `{template}` is not a method symbol"),
                        )
                    })?;
                if key.arguments.is_empty() {
                    base
                } else {
                    mojito_symbol::symbol::instance_symbol(&base, &key.arguments)
                }
            } else if key.arguments.is_empty() {
                template.to_string()
            } else {
                mojito_symbol::symbol::instance_symbol(template, &key.arguments)
            };
        // Two clones of one method whose owner instance and arguments agree
        // differ only in pointer provenance (a clone origin binder with and
        // without an interior projection), which erases natively: the second
        // is the instance the first already names.
        if self.instances.iter().any(|(known, n)| {
            n == &name
                && known.owner.is_some()
                && known.owner == key.owner
                && known.arguments == key.arguments
                && self
                    .functions
                    .get(known.template.as_str())
                    .zip(self.functions.get(template))
                    .is_some_and(|(known, template)| functions_equivalent(known, template))
        }) {
            if !speculative {
                self.speculative.remove(&name);
            }
            return Ok(name);
        }
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
        if speculative {
            self.speculative.insert(name.clone());
        }
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

    /// The callable parameters this call binds to a closure that captures.
    /// Their environments survive no name, so each becomes a runtime
    /// parameter of the instance and an argument here.
    pub(super) fn capturing_callable_arguments(
        &self,
        target: &str,
        param_args: &[mojito_mir::mir::MirParamArg],
    ) -> Vec<(ParamRef, Reg)> {
        let Some(declaration) = self.declarations.get(target) else {
            return Vec::new();
        };
        matched_parameter_arguments(&declaration.param_decls, param_args)
            .into_iter()
            .filter(|(decl, _, _)| {
                matches!(decl, ParamDecl::Value { ty, .. }
                    if matches!(peel_refs(ty), Ty::Func { .. } | Ty::GenericFunc { .. }))
            })
            .filter_map(|(decl, reg, _)| {
                let (_, captures_are_empty) = self.callable_targets.get(&reg.0)?;
                (!captures_are_empty).then(|| (decl.binder(), reg))
            })
            .collect()
    }

    pub(super) fn materialize(
        &mut self,
        key: &InstanceKey,
        bindings: &Bindings,
    ) -> Result<(), MonoError> {
        let name = self.instance_name(key).to_string();
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
        let scope = self
            .declarations
            .get(key.template.as_str())
            .map_or(&[][..], |declaration| &declaration.param_decls);
        substitute_function(&mut function, bindings, scope).map_err(|mut e| {
            e.function.get_or_insert_with(|| key.template.clone());
            e
        })?;
        if !bindings.folded_captures.is_empty() {
            let constants = bindings
                .folded_captures
                .iter()
                .zip(&function.param_types)
                .map(|((name, value), ty)| {
                    value_parameter_constant(value, Some(ty)).ok_or_else(|| {
                        self.error(
                            Some(&key.template),
                            format!("captured value parameter `{name}` has no native constant"),
                        )
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            fold_leading_captures(&mut function, &constants);
        }
        let folded_values = self.folded_parameter_values(&key.template, &function, bindings);
        if !folded_values.is_empty() {
            self.fold_parameter_closures(&mut function.blocks, &folded_values)?;
        }
        self.folded_slots = folded_values.into_keys().collect();
        // A callable parameter the call site could not fold into this body
        // becomes its last runtime parameter, carrying the closure — and its
        // environment — that the body then calls indirectly.
        let promoted: Vec<(String, Ty)> = bindings
            .runtime_callables
            .iter()
            .filter_map(|parameter| {
                promote_to_runtime_parameter(&mut function, &parameter.name)
                    .map(|ty| (parameter.name.to_string(), ty))
            })
            .collect();
        self.constant_values = function_constant_values(&function);
        self.callable_targets = function_callable_targets(&function);
        self.closure_captures = function_closure_captures(&function);
        self.enclosing.clone_from(bindings);
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
        ensure_concrete_function(&key.template, &name, &function)?;

        if let Some(declaration) = self.declarations.get(key.template.as_str()).copied() {
            let mut declaration = declaration.clone();
            substitute_declaration(&mut declaration, bindings)?;
            declaration.lowered_name.clone_from(&name);
            declaration.param_decls.clear();
            fold_leading_capture_parameters(&mut declaration, bindings.folded_captures.len());
            for (parameter, ty) in promoted {
                declare_runtime_parameter(&mut declaration, &parameter, ty);
            }
            self.instantiate_constructed_defaults(&key.template, &mut declaration)?;
            self.enqueue_keyword_collector(&key.template, &declaration)?;
            self.output_function_decls.push(declaration);
        }
        self.discover_structs(&key.template, &function)?;
        self.output_functions.push((name, function));
        Ok(())
    }

    /// A parameter defaulting to a lowered default function
    /// (`s: String = String("a")`) enqueues that function. One defaulting to
    /// a recorded constructor over a generic
    /// struct instance (`dir: Optional[String] = None`, `x: Optional[Int] = 5`):
    /// the omitted-argument path runs the constructor's instance for the
    /// concrete parameter type, so enqueue it and respell the default's target
    /// to that instance. The instance is keyed on the parameter's type, so the
    /// wrapped literal plays no part in selecting it.
    fn instantiate_constructed_defaults(
        &mut self,
        owner: &str,
        declaration: &mut MirFunctionDeclaration,
    ) -> Result<(), MonoError> {
        for (index, default) in declaration.defaults.iter_mut().enumerate() {
            // A lowered default function names no binder, so it has the one
            // instance, which only this default reaches.
            if let Some(CheckedConst::Evaluate { function }) = default {
                if self.functions.contains_key(function.as_str()) {
                    *function = self.enqueue(function, self.base_bindings(), Vec::new())?;
                }
                continue;
            }
            let Some(CheckedConst::Construct { target, arg }) = default else {
                continue;
            };
            if matches!(**arg, CheckedConst::Construct { .. }) {
                continue;
            }
            let Some(resolved) = self.constructed_default_target(target) else {
                continue;
            };
            *target = resolved;
            let Some(param_ty @ Ty::Struct(struct_name, arguments)) =
                declaration.param_types.get(index)
            else {
                continue;
            };
            if is_symbolic(param_ty) {
                continue;
            }
            // A non-generic constructor needs no instance, but this default may
            // be the only thing that reaches it, so it is enqueued like any
            // other concrete callee.
            if arguments.is_empty()
                || !self
                    .generic_templates
                    .contains(nominal_template(struct_name))
            {
                let Some(body) = self.functions.get(target.as_str()).copied() else {
                    continue;
                };
                if function_types(body).any(is_symbolic) {
                    continue;
                }
                *target = self.enqueue(target, self.base_bindings(), Vec::new())?;
                continue;
            }
            let (bindings, arguments, _) =
                self.infer_receiver_call(owner, target, param_ty, None)?;
            let instance = self.enqueue(target, bindings, arguments)?;
            *target = instance;
        }
        Ok(())
    }

    /// A `**kwargs: element` parameter binds a `StringDict[element]` the
    /// caller builds without a MIR call: the VM runs the instance's empty
    /// constructor and one `__setitem__` per collected keyword, so enqueue
    /// both for a callee that only ever reads the collector.
    fn enqueue_keyword_collector(
        &mut self,
        owner: &str,
        declaration: &MirFunctionDeclaration,
    ) -> Result<(), MonoError> {
        let Some(element) = declaration.kw_variadic.as_ref() else {
            return Ok(());
        };
        if is_symbolic(element) {
            return Ok(());
        }
        let collector = Ty::Struct(
            mojito_symbol::symbol::instance_symbol(
                "StringDict",
                &[InstanceArg::Ty(element.clone())],
            ),
            vec![mojito_types::types::TyArg::Ty(element.clone())],
        );
        self.enqueue_nominal_method_instance(owner, &collector, "__init__", 0, &[])?;
        self.enqueue_nominal_method_instance(owner, &collector, "__setitem__", 2, &[])
    }

    /// The compiled constructor a default's conversion target names. The
    /// checker records the overload symbol, or — for a hand-written
    /// `@implicit` constructor — the bare struct name, which resolves through
    /// the shared callable-symbol policy over the single wrapped literal, as
    /// the VM's `constructor_name` resolves it.
    fn constructed_default_target(&self, target: &str) -> Option<String> {
        if self.functions.contains_key(target) {
            return Some(target.to_string());
        }
        let init = format!("{target}.__init__");
        let resolved = mojito_symbol::symbol::resolve_callable_symbol(
            self.functions.iter().map(|(name, function)| {
                mojito_symbol::symbol::CallableCandidate {
                    name,
                    n_params: function.n_params,
                }
            }),
            &init,
            1,
        );
        self.functions
            .contains_key(resolved.as_str())
            .then_some(resolved)
    }

    #[allow(
        clippy::cognitive_complexity,
        clippy::too_many_lines,
        reason = "TODO: split this pass"
    )]
    pub(super) fn rewrite_blocks(
        &mut self,
        owner: &str,
        function: &mut MirFunction,
        blocks: &mut [MirBlock],
    ) -> Result<(), MonoError> {
        for block in blocks {
            // Argument loads a devirtualized capturing call needs, spliced in
            // ahead of the call once the walk over this block is done.
            let mut preludes: Vec<(usize, Vec<MirInstr>)> = Vec::new();
            for (index, instruction) in block.instrs.iter_mut().enumerate() {
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
                        let resolved = self.instance_dunder_target(function, args[0], method);
                        *instruction =
                            dunder_method_call(*dest, args[0], method, resolved, Vec::new());
                    }
                }
                // A prefix `-`/`~` on a nominal operand is the VM's dunder
                // dispatch (`apply_prefix` → `__neg__`/`__invert__`); `not`
                // arrives already converted through `Bool(x)` by MIR.
                if let MirInstr::UnOp { op, dest, a } = instruction
                    && matches!(
                        op,
                        mojito_ast::ast::PrefixOp::Neg | mojito_ast::ast::PrefixOp::Invert
                    )
                    && matches!(
                        function.reg_types.get(&a.0).map(peel_refs),
                        Some(Ty::Struct(..))
                    )
                {
                    let resolved = self.instance_dunder_target(function, *a, op.dunder());
                    *instruction = dunder_method_call(*dest, *a, op.dunder(), resolved, Vec::new());
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
                    && !matches!(
                        op,
                        mojito_ast::ast::InfixOp::In | mojito_ast::ast::InfixOp::NotIn
                    )
                    && matches!(
                        function.reg_types.get(&a.0).map(peel_refs),
                        Some(Ty::Struct(..))
                    )
                {
                    let resolved = resolved
                        .take()
                        .or_else(|| self.instance_dunder_target(function, *a, method));
                    *instruction = dunder_method_call(*dest, *a, method, resolved, vec![*b]);
                }
                // `**` on Int/UInt calls the bundled `_pow_int` body rather
                // than an emitted helper, so the operator is a use of that
                // function even though the instruction stays a `BinOp`.
                if let MirInstr::BinOp { op, a, b, .. } = instruction
                    && *op == mojito_ast::ast::InfixOp::Pow
                    && mojito_symbol::symbol::calls_pow_int(
                        function.reg_types.get(&a.0).map(peel_refs),
                        function.reg_types.get(&b.0).map(peel_refs),
                    )
                {
                    self.enqueue(
                        mojito_symbol::symbol::POW_INT_SYMBOL,
                        self.base_bindings(),
                        Vec::new(),
                    )?;
                }
                // The expansions that display a scalar — `print`,
                // `String(...)`, `repr`, `os.abort`, and a `Writer.write` —
                // call the bundled digit bodies without naming them; the
                // backend's reachable set follows the same rule.
                if matches!(instruction, MirInstr::Call { func, .. }
                    if matches!(func.0.as_str(), "print" | "String" | "repr" | "_mojito_abort"))
                    || matches!(instruction,
                        MirInstr::MethodCall { method, .. } if method == "write")
                {
                    for symbol in [
                        mojito_symbol::symbol::INT_DIGITS_SYMBOL,
                        mojito_symbol::symbol::UINT_DIGITS_SYMBOL,
                    ] {
                        if self.functions.contains_key(symbol) {
                            self.enqueue(symbol, self.base_bindings(), Vec::new())?;
                        }
                    }
                }
                match instruction {
                    MirInstr::Call {
                        dest,
                        func,
                        args,
                        kwargs,
                        arg_places,
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
                            if func.0 == "repr" {
                                for arg in args.clone() {
                                    self.enqueue_repr_instance(owner, function, arg)?;
                                }
                            }
                            if self.structs.contains_key(func.0.as_str())
                                && !mojito_symbol::symbol::is_stdlib_string_struct(&func.0)
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
                                        func.0.clone_from(concrete);
                                    }
                                    // The copied value carries the instance's
                                    // parameters; an explicitly spelled
                                    // type argument (`Dict[Int, Int,
                                    // AHasher](copy: self)` in an instance
                                    // clone) reified no runtime data here.
                                    param_arg_regs.clear();
                                    continue;
                                }
                                let init_base = format!("{}.__init__", func.0);
                                let init = mojito_symbol::symbol::resolve_callable_symbol(
                                    self.functions.iter().map(|(name, f)| CallableCandidate {
                                        name,
                                        n_params: f.n_params,
                                    }),
                                    &init_base,
                                    args.len() + kwargs.len(),
                                );
                                let init = if self.functions.contains_key(init.as_str()) {
                                    init
                                } else {
                                    self.runtime_pack_constructor(&init_base).unwrap_or(init)
                                };
                                // A closed instance constructs through its own
                                // clone, as struct discovery enqueues it.
                                let init = self
                                    .instance_dunder_target(function, *dest, "__init__")
                                    .unwrap_or(init);
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
                                    func.0.clone_from(concrete);
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
                        let (target, mut bindings, arguments) = self.infer_call(
                            owner,
                            function,
                            &func.0,
                            receiver,
                            *dest,
                            args,
                            kwargs,
                            param_arg_regs,
                        )?;
                        // A callable parameter bound to a closure with
                        // captures keeps its environment: the instance takes
                        // it as a trailing runtime parameter, so pass the
                        // closure here as an ordinary argument.
                        let capturing = self.capturing_callable_arguments(&target, param_arg_regs);
                        bindings.runtime_callables =
                            capturing.iter().map(|(binder, _)| binder.clone()).collect();
                        func.0 = self.enqueue(&target, bindings, arguments)?;
                        for param_arg in param_arg_regs.iter_mut() {
                            param_arg.value = None;
                        }
                        for (_, closure) in capturing {
                            args.push(closure);
                            arg_places.push(None);
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
                        param_decls,
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
                        // An erased `hasher._update_with_simd(x)` (a generic
                        // `__hash__[H: Hasher]` body now concrete) targets the
                        // clone for the argument's own vector type: the
                        // template's body is a stub.
                        let simd_clone = (resolved.is_none()
                            && method == "_update_with_simd"
                            && args.len() == 1
                            && kwargs.is_empty())
                        .then(|| function.reg_types.get(&args[0].0))
                        .flatten()
                        .filter(|leaf| {
                            mojito_types::types::simd_shape(leaf).is_some()
                                || matches!(leaf, Ty::Bool)
                        })
                        .map(mojito_symbol::symbol::simd_update_clone_name);
                        let method: &str = simd_clone.as_deref().unwrap_or(method);
                        // Source methods are declared under the template name;
                        // an instance-named receiver (`List$mono$TInt`) still
                        // resolves against `List.*` and gets its instance
                        // identity from `infer_call`'s receiver binding.
                        let target = mojito_symbol::symbol::resolve_method_symbol(
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
                                let write_string = mojito_symbol::symbol::resolve_callable_symbol(
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
                                    // Each argument is formatted through its
                                    // own `write_to` first.
                                    for arg in args.clone() {
                                        self.enqueue_display_instance(owner, function, arg)?;
                                    }
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
                        // The instance's declaration keeps no compile-time
                        // parameters, so the call to it keeps none either.
                        param_decls.clear();
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
                            // The lifted body takes its environment as leading
                            // reference parameters: the direct call passes the
                            // captured places themselves.
                            if !captures_are_empty {
                                let captured = self.capture_arguments(owner, function, &target)?;
                                arg_places.resize(args.len(), None);
                                args.splice(0..0, captured.iter().map(|(reg, _)| *reg));
                                arg_places.splice(
                                    0..0,
                                    captured.iter().map(|(_, place)| Some(place.clone())),
                                );
                                let loads = captured
                                    .into_iter()
                                    .map(|(dest, place)| MirInstr::LoadPlace { dest, place })
                                    .collect();
                                preludes.push((index, loads));
                            }
                            // A closure over folded value parameters already
                            // names its lifted body's instance.
                            let concrete = if self.functions.contains_key(target.as_str()) {
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
                                self.enqueue(&target, bindings, arguments)?
                            } else {
                                target
                            };
                            *instruction = MirInstr::Call {
                                dest: *dest,
                                func: mojito_mir::mir::FuncRef(concrete),
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
                        // A callable contract's dispatch symbol spells its
                        // parameter types, so a contract over the template's
                        // binders names the instance's concrete ones.
                        if let Some(target) = resolved.as_mut()
                            && target.starts_with("__trait_dispatch.")
                            && let Some(contract) =
                                mojito_symbol::symbol::callable_contract_target(receiver)
                        {
                            *target = contract;
                        }
                        let Ty::Struct(receiver_name, _) = peel_refs(receiver) else {
                            continue;
                        };
                        let target = mojito_symbol::symbol::resolve_method_symbol(
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
                    // A generic nested `def` is no body of its own: each
                    // call site binding its parameters enqueues an instance.
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
                        if function_types(body).any(is_symbolic)
                            || self
                                .declarations
                                .get(target.as_str())
                                .is_some_and(|declaration| !declaration.param_decls.is_empty())
                        {
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
                            && matches!(&place.proj[0], mojito_mir::mir::Proj::Field(_)) =>
                    {
                        let mojito_mir::mir::Proj::Field(field) = &place.proj[0] else {
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
                        index,
                        call: Some(call),
                        ..
                    } => {
                        let (dest, base, index) = (*dest, *base, *index);
                        self.rewrite_subscript_call(
                            owner,
                            function,
                            base,
                            &[index],
                            Some(dest),
                            call,
                        )?;
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
                        self.rewrite_subscript_call(
                            owner,
                            function,
                            object,
                            &[],
                            Some(dest),
                            call,
                        )?;
                    }
                    MirInstr::MultiSet { receiver, call, .. } => {
                        let receiver = *receiver;
                        self.rewrite_subscript_call(owner, function, receiver, &[], None, call)?;
                    }
                    // An untyped iterator slot passes through: it belongs to
                    // a compiler-private pack loop the backend rejects at its
                    // own boundary.
                    MirInstr::TryNext { iter, call, .. } => {
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
                    // Variant equality lowers to a tag switch that calls each
                    // nominal alternative's `__eq__`; enqueue those instances.
                    MirInstr::BinOp {
                        op: mojito_ast::ast::InfixOp::Eq | mojito_ast::ast::InfixOp::Ne,
                        a,
                        ..
                    } => {
                        if let Some(Ty::Variant(alternatives)) =
                            function.reg_types.get(&a.0).map(peel_refs).cloned()
                        {
                            for alternative in &alternatives {
                                if matches!(alternative, Ty::Struct(..)) {
                                    self.enqueue_nominal_method_instance(
                                        owner,
                                        alternative,
                                        "__eq__",
                                        1,
                                        &[],
                                    )?;
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            for (index, prelude) in preludes.into_iter().rev() {
                block.instrs.splice(index..index, prelude);
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
                        mut_self_methods: HashSet::default(),
                        fieldwise_init: false,
                        param_decls: Vec::new(),
                        explicit_destroy_message: None,
                        explicit_destructors: HashMap::default(),
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
                        mojito_symbol::symbol::retarget_method_symbol(entry, &declaration.name)
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
            // The nominal String's `__copyinit__` stays too: native lowering
            // bridges it and never reaches the body, but the VM runs it.
            for method in ["__init__", "__copyinit__", "__moveinit__", "__deinit__"] {
                // A closed instance whose clone of this lifecycle method
                // exists runs that body, under the template-shaped symbol
                // lowering composes for the instance. The clone was minted
                // over the checker's spelling of the arguments, so a nested
                // instance argument (`List$mono$TInt`) names its template.
                let clone = mojito_symbol::symbol::instance_method_clone_name(
                    method,
                    &template.param_decls,
                    &template_spelled_arguments(&arguments),
                )
                .map(|clone| format!("{template_name}.{clone}"))
                .filter(|symbol| self.functions.contains_key(symbol.as_str()));
                if let Some(clone) = clone {
                    // A variadic initializer's clone needs a call-site arity;
                    // those sites enqueue it.
                    if !self
                        .declarations
                        .get(clone.as_str())
                        .is_some_and(|decl| arity_keyed_variadic(decl))
                    {
                        self.enqueue(&clone, bindings.clone(), Vec::new())?;
                    }
                    continue;
                }
                let base = format!("{template_name}.{method}");
                let candidates = self
                    .functions
                    .iter()
                    .filter(|(candidate, _)| {
                        **candidate == base
                            || mojito_symbol::symbol::is_overload_of(candidate, &base)
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
                    if arity_keyed_variadic(function_decl) {
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
                    // Constructors are only ever reached through call sites;
                    // the eager walk over-approximates a conditional
                    // overload (`where conforms_to(Self.T, Defaultable)`
                    // on an instance whose element is not), so it is
                    // speculative. The copy/move/deinit lifecycle stays
                    // firm: lowering composes those names itself.
                    if method == "__init__" {
                        self.enqueue_speculative(&candidate, bindings.clone(), method_arguments)?;
                    } else {
                        self.enqueue(&candidate, bindings.clone(), method_arguments)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// The unique `__init__` overload of `init_base`'s struct whose
    /// runtime-pack collector binds any element count (current Tuple's
    /// `__init__(out self, var *args: *Ts)` beside its nullary constructor),
    /// which arity-keyed selection cannot see — the VM's `constructor_name`
    /// fallback.
    fn runtime_pack_constructor(&self, init_base: &str) -> Option<String> {
        let mut packs = self.functions.keys().filter(|name| {
            mojito_symbol::symbol::is_overload_of(name, init_base)
                && self.declarations.get(*name).is_some_and(|declaration| {
                    matches!(
                        declaration.variadic,
                        Some(mojito_types::types::Ty::RuntimePack(_))
                    )
                })
        });
        let first = (*packs.next()?).to_string();
        packs.next().is_none().then_some(first)
    }

    /// The captured places a direct call to the lifted body `target` passes
    /// ahead of its source arguments, each paired with a fresh register the
    /// call loads it into. Only by-reference environments are the caller's
    /// storage; a copied or moved capture is the closure's own snapshot,
    /// which a direct call cannot recover.
    /// The value each of `function`'s slots holds when it names one of
    /// `owner`'s value parameters (or a capture its instance folded) bound
    /// to a compile-time constant here, keyed by slot.
    fn folded_parameter_values(
        &self,
        owner: &str,
        function: &MirFunction,
        bindings: &Bindings,
    ) -> HashMap<u32, CtValue> {
        let scope = self
            .declarations
            .get(owner)
            .map_or(&[][..], |declaration| &declaration.param_decls);
        let locals = bound_parameter_locals(scope, bindings);
        (0u32..)
            .zip(&function.var_names)
            .filter_map(|(slot, name)| {
                let value = bindings
                    .callables
                    .get(name)
                    .map(|callable| CtValue::Str(callable.clone()))
                    .or_else(|| locals.get(name.as_str()).map(|value| (*value).clone()))
                    .or_else(|| {
                        bindings
                            .folded_captures
                            .iter()
                            .find(|(folded, _)| folded == name)
                            .map(|(_, value)| value.clone())
                    })?;
                Some((slot, value))
            })
            .collect()
    }

    /// Point each closure whose captures are all snapshots of folded value
    /// parameters at its lifted body's instance over those values, which
    /// takes no environment: the closure is the `thin` function the checker
    /// typed, with no frame-local record to outlive.
    fn fold_parameter_closures(
        &mut self,
        blocks: &mut [MirBlock],
        values: &HashMap<u32, CtValue>,
    ) -> Result<(), MonoError> {
        for instruction in blocks.iter_mut().flat_map(|block| &mut block.instrs) {
            match instruction {
                MirInstr::MakeClosure {
                    function: target,
                    captures,
                    ..
                } if !captures.is_empty()
                    && self
                        .declarations
                        .get(target.as_str())
                        .is_some_and(|declaration| declaration.param_decls.is_empty())
                    && self
                        .functions
                        .get(target.as_str())
                        .is_some_and(|body| !function_types(body).any(is_symbolic)) =>
                {
                    let Some(folded) = captures
                        .iter()
                        .map(|capture| {
                            (capture.mode == MirCaptureMode::Copy && capture.place.proj.is_empty())
                                .then_some(capture.place.root)
                                .and_then(|root| values.get(&root))
                                .cloned()
                        })
                        .collect::<Option<Vec<_>>>()
                    else {
                        continue;
                    };
                    let arguments = folded.iter().cloned().map(InstanceArg::Value).collect();
                    let bindings = Bindings {
                        folded_captures: self.functions[target.as_str()]
                            .var_names
                            .iter()
                            .cloned()
                            .zip(folded)
                            .collect(),
                        ..self.base_bindings()
                    };
                    *target = self.enqueue(target, bindings, arguments)?;
                    captures.clear();
                }
                MirInstr::Try {
                    body,
                    handler,
                    orelse,
                    finalbody,
                    ..
                } => {
                    self.fold_parameter_closures(body, values)?;
                    for blocks in handler
                        .iter_mut()
                        .map(|(_, blocks)| blocks)
                        .chain(orelse)
                        .chain(finalbody)
                    {
                        self.fold_parameter_closures(blocks, values)?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn capture_arguments(
        &self,
        owner: &str,
        function: &mut MirFunction,
        target: &str,
    ) -> Result<Vec<(Reg, MirPlace)>, MonoError> {
        let captures = self
            .closure_captures
            .get(target)
            .map_or(&[][..], Vec::as_slice);
        if captures.is_empty()
            || captures.iter().any(|capture| match capture.mode {
                MirCaptureMode::Reference => false,
                MirCaptureMode::Copy => !self.folded_slots.contains(&capture.place.root),
                MirCaptureMode::Move => true,
            })
        {
            return Err(self.error(
                Some(owner),
                format!("generic retained callable `{target}` captures by value"),
            ));
        }
        captures
            .iter()
            .map(|capture| {
                let ty = capture.place.ty.clone().ok_or_else(|| {
                    self.error(
                        Some(owner),
                        format!("capture of generic retained callable `{target}` is untyped"),
                    )
                })?;
                let reg = Reg(function.n_regs);
                function.n_regs += 1;
                function.reg_types.insert(reg.0, ty);
                Ok((reg, capture.place.clone()))
            })
            .collect()
    }

    #[allow(
        clippy::unused_self,
        reason = "TODO: make an associated function or use the receiver"
    )]
    pub(super) fn error(&self, function: Option<&str>, construct: impl Into<String>) -> MonoError {
        MonoError {
            function: function.map(str::to_string),
            construct: construct.into(),
        }
    }
}

/// Whether `declaration` collects a variadic parameter whose element count a
/// call site keys (`var *values: T`), rather than a runtime pack or a tuple.
const fn arity_keyed_variadic(declaration: &MirFunctionDeclaration) -> bool {
    matches!(&declaration.variadic, Some(element)
        if !matches!(element, Ty::RuntimePack(_) | Ty::Tuple(_)))
}

/// `arguments` with every monomorphized struct (`List$mono$TInt[Int]`)
/// respelled as its template application (`List[Int]`), the spelling the
/// checker mangled its per-instantiation clones over.
fn template_spelled_arguments(arguments: &[TyArg]) -> Vec<TyArg> {
    struct TemplateSpelling;
    impl mojito_types::types::TyRewrite for TemplateSpelling {
        fn whole(&mut self, ty: &Ty) -> Option<Ty> {
            let Ty::Struct(name, arguments) = ty else {
                return None;
            };
            let template = nominal_template(name);
            if template == name {
                return None;
            }
            let arguments = mojito_types::types::rewrite_tyargs(arguments, self).ok()?;
            Some(Ty::Struct(template.to_string(), arguments))
        }

        fn expr(
            &mut self,
            expr: &mojito_types::param_expr::ParamExpr,
        ) -> Result<mojito_types::param_expr::ParamExpr, mojito_types::param_expr::ParamError>
        {
            Ok(expr.clone())
        }
    }
    mojito_types::types::map_tyargs(arguments, |ty| {
        mojito_types::types::rewrite_ty(ty, &mut TemplateSpelling).unwrap_or_else(|_| ty.clone())
    })
}

/// The instance symbol a per-instantiation lifecycle clone is emitted under
/// (`Box.__deinit__$y3:Int` bound to owner `Box$mono$TInt` becomes
/// `Box$mono$TInt.__deinit__`), or `None` for any other method.
///
/// Lowering composes a struct's lifecycle symbols by name, so the clone must
/// answer to the plain one; its template is never instantiated for that
/// instance beside it.
fn lifecycle_clone_instance_symbol(template: &str, owner: Option<&str>) -> Option<String> {
    let owner = owner?;
    let (_, method) = mojito_symbol::symbol::split_method_symbol(template)?;
    let base = mojito_symbol::symbol::instance_clone_base(method);
    if !matches!(
        base,
        "__init__" | "__copyinit__" | "__moveinit__" | "__deinit__"
    ) {
        return None;
    }
    // Only a per-instantiation clone renames. A signature-qualified overload
    // of the template itself (`Optional.__init__$ov$None`) keeps its own
    // symbol: its siblings answer to the same base name.
    let baked = method.strip_prefix(base)?;
    (!baked.is_empty() && !baked.contains(mojito_symbol::symbol::OV_SEP))
        .then(|| format!("{owner}.{base}"))
}
