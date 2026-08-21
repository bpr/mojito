//! Backend-private MIR monomorphization.
//!
//! This pass consumes only verified, drop-elaborated MIR and returns an owned
//! entry-rooted concrete graph. It never mutates the canonical MIR artifact.

use std::collections::{HashMap, VecDeque};

use crate::call::{ArgSlot, CallVariadics, match_call_slots};
use crate::ct::{CtExpr, CtValue};
use crate::mir::{
    MirBlock, MirDeclarations, MirFunction, MirFunctionDeclaration, MirInstr, MirPlace, MirProgram,
    MirStructDeclaration, Reg,
};
use crate::symbol::{CallableCandidate, InstanceArg};
use crate::types::{DependentType, ParamDecl, Ty, TyArg};

/// A fully concrete backend-private program and the concrete identity of every
/// requested public entry.
pub struct SpecializedProgram {
    pub program: MirProgram,
    pub entries: HashMap<String, String>,
}

/// A source-template-oriented specialization failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonoError {
    pub function: Option<String>,
    pub construct: String,
}

/// Specialize the graph reachable from `entries` without modifying `program`.
pub fn specialize(
    program: &MirProgram,
    entries: &[String],
) -> Result<SpecializedProgram, MonoError> {
    Specializer::new(program).run(entries)
}

#[derive(Clone, PartialEq, Eq)]
struct InstanceKey {
    template: String,
    arguments: Vec<InstanceArg>,
}

#[derive(Clone, Default)]
struct Bindings {
    types: HashMap<String, Ty>,
    values: HashMap<String, CtValue>,
}

struct Specializer<'a> {
    source: &'a MirProgram,
    functions: HashMap<&'a str, &'a MirFunction>,
    declarations: HashMap<&'a str, &'a MirFunctionDeclaration>,
    structs: HashMap<&'a str, &'a MirStructDeclaration>,
    queue: VecDeque<(InstanceKey, Bindings)>,
    instances: Vec<(InstanceKey, String)>,
    output_functions: Vec<(String, MirFunction)>,
    output_function_decls: Vec<MirFunctionDeclaration>,
    output_structs: Vec<MirStructDeclaration>,
}

impl<'a> Specializer<'a> {
    fn new(source: &'a MirProgram) -> Self {
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
            queue: VecDeque::new(),
            instances: Vec::new(),
            output_functions: Vec::new(),
            output_function_decls: Vec::new(),
            output_structs: Vec::new(),
        }
    }

    fn run(mut self, entries: &[String]) -> Result<SpecializedProgram, MonoError> {
        let mut entry_map = HashMap::new();
        for entry in entries {
            let decl = self.declarations.get(entry.as_str()).copied();
            let function = self.functions.get(entry.as_str()).copied().ok_or_else(|| {
                self.error(
                    None,
                    format!("entry function `{entry}` (not found in the MIR program)"),
                )
            })?;
            if decl.is_some_and(|decl| !decl.param_decls.is_empty())
                || function_types(function).any(is_symbolic)
            {
                return Err(self.error(
                    Some(entry),
                    format!("generic entry `{entry}` has unresolved parameters"),
                ));
            }
            let name = self.enqueue(entry, Bindings::default(), Vec::new())?;
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

    fn enqueue(
        &mut self,
        template: &str,
        bindings: Bindings,
        arguments: Vec<InstanceArg>,
    ) -> Result<String, MonoError> {
        let key = InstanceKey {
            template: template.to_string(),
            arguments,
        };
        if let Some((_, name)) = self.instances.iter().find(|(known, _)| known == &key) {
            return Ok(name.clone());
        }
        let name = if key.arguments.is_empty() {
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

    fn instance_name(&self, key: &InstanceKey) -> &str {
        self.instances
            .iter()
            .find(|(known, _)| known == key)
            .expect("queued instance has identity")
            .1
            .as_str()
    }

    fn materialize(&mut self, key: InstanceKey, bindings: Bindings) -> Result<(), MonoError> {
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

    fn rewrite_blocks(
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
                match instruction {
                    MirInstr::Call {
                        dest,
                        func,
                        args,
                        kwargs,
                        ..
                    } => {
                        if !self.functions.contains_key(func.0.as_str()) {
                            if self.structs.contains_key(func.0.as_str())
                                && !crate::symbol::is_stdlib_string_struct(&func.0)
                            {
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
                                    )?;
                                    self.enqueue(&target, bindings, arguments)?;
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
                        let (target, bindings, arguments) = self
                            .infer_call(owner, function, &func.0, receiver, *dest, args, kwargs)?;
                        func.0 = self.enqueue(&target, bindings, arguments)?;
                    }
                    MirInstr::MethodCall {
                        dest,
                        recv,
                        method,
                        resolved,
                        args,
                        kwargs,
                        ..
                    } => {
                        let receiver = function.reg_types.get(&recv.0).ok_or_else(|| {
                            self.error(Some(owner), "method receiver lacks a MIR type")
                        })?;
                        let Ty::Struct(receiver_name, _) = receiver else {
                            continue;
                        };
                        let target = crate::symbol::resolve_method_symbol(
                            self.functions.iter().map(|(name, f)| CallableCandidate {
                                name,
                                n_params: f.n_params,
                            }),
                            receiver_name,
                            method,
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
                            Some(*recv),
                            *dest,
                            args,
                            kwargs,
                        )?;
                        let concrete = self.enqueue(&target, bindings, arguments)?;
                        *resolved = Some(concrete);
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

    #[allow(clippy::too_many_arguments)]
    fn infer_call(
        &self,
        owner: &str,
        caller: &MirFunction,
        target: &str,
        receiver: Option<Reg>,
        dest: Reg,
        args: &[Reg],
        kwargs: &[(String, Reg)],
    ) -> Result<(String, Bindings, Vec<InstanceArg>), MonoError> {
        let declaration = self.declarations.get(target).copied().ok_or_else(|| {
            self.error(
                Some(owner),
                format!("callee `{target}` lacks declaration facts"),
            )
        })?;
        let mut bindings = Bindings::default();
        if let Some(receiver) = receiver {
            let actual_receiver = reg_ty(caller, receiver, owner)?;
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
        for (index, slot) in slots.slots.iter().enumerate() {
            let actual = match slot {
                ArgSlot::Positional(i) => Some(reg_ty(caller, args[*i], owner)?),
                ArgSlot::Keyword(i) => Some(reg_ty(caller, kwargs[*i].1, owner)?),
                ArgSlot::Default => None,
            };
            if let Some(actual) = actual {
                unify(&declaration.param_types[index], actual, &mut bindings).map_err(|e| {
                    self.error(Some(owner), format!("monomorphizing `{target}`: {e}"))
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
        apply_defaults(&declaration.param_decls, &mut bindings)?;
        let arguments = ordered_arguments(&declaration.param_decls, &bindings, target)?;
        Ok((target.to_string(), bindings, arguments))
    }

    /// Walk `blocks` (recursing into `try` regions) folding every `GetIter`
    /// before the main call rewrite reads iterator slot types.
    fn rewrite_iterator_inits(
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
    fn rewrite_get_iter(
        &mut self,
        owner: &str,
        function: &mut MirFunction,
        source: crate::hir::VarId,
        dest: crate::hir::VarId,
        prepare: &mut Vec<String>,
    ) -> Result<(), MonoError> {
        let Some(mut current) = function.var_tys.get(&source).cloned() else {
            // An untyped source belongs to a compiler-private pack loop the
            // backend rejects at its own boundary.
            return Ok(());
        };
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
    fn resolve_iterator_step(
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
        let target = crate::symbol::resolve_method_symbol(
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
    fn infer_receiver_call(
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
        let mut bindings = Bindings::default();
        if let Ty::Struct(receiver_name, arguments) = receiver
            && let Some(struct_decl) = self.structs.get(nominal_template(receiver_name)).copied()
        {
            bind_ty_args(&struct_decl.param_decls, arguments, &mut bindings).map_err(|e| {
                self.error(
                    Some(owner),
                    format!("monomorphizing receiver for `{target}`: {e}"),
                )
            })?;
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
        let arguments = ordered_arguments(&declaration.param_decls, &bindings, target)?;
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
    fn has_iterator_next(&self, receiver: &Ty) -> bool {
        let Ty::Struct(name, _) = receiver else {
            return false;
        };
        let target = crate::symbol::resolve_method_symbol(
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

    fn discover_structs(&mut self, owner: &str, function: &MirFunction) -> Result<(), MonoError> {
        let mut types = function_types(function).cloned().collect::<Vec<_>>();
        while let Some(ty) = types.pop() {
            collect_nested_types(&ty, &mut types);
            let Ty::Struct(name, arguments) = ty else {
                continue;
            };
            if self.output_structs.iter().any(|decl| decl.name == name) {
                continue;
            }
            let template_name = name.split("$mono").next().unwrap_or(&name).to_string();
            let Some(template) = self.structs.get(template_name.as_str()).copied() else {
                continue;
            };
            if arguments.len() < template.param_decls.len() {
                continue;
            }
            let mut bindings = Bindings::default();
            bind_ty_args(&template.param_decls, &arguments, &mut bindings).map_err(|e| {
                self.error(
                    Some(owner),
                    format!("monomorphizing struct `{template_name}`: {e}"),
                )
            })?;
            let mut declaration = template.clone();
            for (_, field) in &mut declaration.fields {
                *field = substitute_ty(field, &bindings)?;
            }
            declaration.name = name;
            declaration.param_decls.clear();
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
                    let Ok(method_arguments) =
                        ordered_arguments(&function_decl.param_decls, &bindings, &candidate)
                    else {
                        continue;
                    };
                    self.enqueue(&candidate, bindings.clone(), method_arguments)?;
                }
            }
        }
        Ok(())
    }

    fn error(&self, function: Option<&str>, construct: impl Into<String>) -> MonoError {
        MonoError {
            function: function.map(str::to_string),
            construct: construct.into(),
        }
    }
}

fn reg_ty<'a>(function: &'a MirFunction, reg: Reg, owner: &str) -> Result<&'a Ty, MonoError> {
    function.reg_types.get(&reg.0).ok_or_else(|| MonoError {
        function: Some(owner.to_string()),
        construct: format!("register r{} lacks a concrete type", reg.0),
    })
}

fn ordered_arguments(
    decls: &[ParamDecl],
    bindings: &Bindings,
    target: &str,
) -> Result<Vec<InstanceArg>, MonoError> {
    decls
        .iter()
        .map(|decl| {
            match decl {
                ParamDecl::Type { name, .. } => {
                    bindings.types.get(name).cloned().map(InstanceArg::Ty)
                }
                ParamDecl::Value { name, .. } => {
                    bindings.values.get(name).cloned().map(InstanceArg::Value)
                }
            }
            .ok_or_else(|| MonoError {
                function: Some(target.to_string()),
                construct: format!(
                    "monomorphization cannot resolve parameter `{}`",
                    decl.name()
                ),
            })
        })
        .collect()
}

fn apply_defaults(decls: &[ParamDecl], bindings: &mut Bindings) -> Result<(), MonoError> {
    for decl in decls {
        match decl {
            ParamDecl::Type {
                name,
                default: Some(default),
                ..
            } if !bindings.types.contains_key(name) => {
                bindings
                    .types
                    .insert(name.clone(), substitute_ty(default, bindings)?);
            }
            ParamDecl::Value {
                name,
                default: Some(default),
                ..
            } if !bindings.values.contains_key(name) => {
                bindings
                    .values
                    .insert(name.clone(), eval_ct(default, bindings)?);
            }
            _ => {}
        }
    }
    Ok(())
}

fn bind_ty_args(
    decls: &[ParamDecl],
    args: &[TyArg],
    bindings: &mut Bindings,
) -> Result<(), String> {
    for (decl, arg) in decls.iter().zip(args) {
        match (decl, arg) {
            (ParamDecl::Type { name, .. }, TyArg::Ty(ty)) => bind_type(name, ty, bindings)?,
            (ParamDecl::Value { name, .. }, TyArg::Val(value)) => {
                bind_value(name, value, bindings)?
            }
            (_, TyArg::Origin(_)) => {}
            _ => {
                return Err(format!(
                    "argument for `{}` has the wrong parameter kind",
                    decl.name()
                ));
            }
        }
    }
    Ok(())
}

fn unify(pattern: &Ty, actual: &Ty, bindings: &mut Bindings) -> Result<(), String> {
    match pattern {
        Ty::Param { name, .. } => bind_type(name, actual, bindings),
        // A literal-typed register materializes into whatever concrete
        // storage the checker admitted (`MaterializeLiteral` converts the
        // value at the boundary); the pattern constrains nothing here.
        _ if matches!(
            actual,
            Ty::IntLiteral | Ty::FloatLiteral | Ty::StringLiteral
        ) && pattern != actual =>
        {
            Ok(())
        }
        Ty::Struct(pn, pa) => match actual {
            Ty::Struct(an, _) if nominal_template(pn) == nominal_template(an) && pa.is_empty() => {
                Ok(())
            }
            Ty::Struct(an, aa)
                if nominal_template(pn) == nominal_template(an) && pa.len() == aa.len() =>
            {
                pa.iter()
                    .zip(aa)
                    .try_for_each(|(p, a)| unify_arg(p, a, bindings))
            }
            _ => Err(format!("expected `{pattern}`, found `{actual}`")),
        },
        Ty::Tuple(p) | Ty::RuntimePack(p) | Ty::Variant(p) => match actual {
            Ty::Tuple(a) | Ty::RuntimePack(a) | Ty::Variant(a) if p.len() == a.len() => {
                p.iter().zip(a).try_for_each(|(p, a)| unify(p, a, bindings))
            }
            _ => Err(format!("expected `{pattern}`, found `{actual}`")),
        },
        Ty::ComptimeList(p) | Ty::VariadicPack(p) => match actual {
            Ty::ComptimeList(a) | Ty::VariadicPack(a) => unify(p, a, bindings),
            _ => Err(format!("expected `{pattern}`, found `{actual}`")),
        },
        Ty::Pointer { element: p, .. } => match actual {
            Ty::Pointer { element: a, .. } => unify(p, a, bindings),
            _ => Err(format!("expected `{pattern}`, found `{actual}`")),
        },
        Ty::Ref(p) => match actual {
            Ty::Ref(a) if p.mutability == a.mutability => unify(&p.referent, &a.referent, bindings),
            _ => Err(format!("expected `{pattern}`, found `{actual}`")),
        },
        _ if pattern == actual => Ok(()),
        _ => Err(format!("expected `{pattern}`, found `{actual}`")),
    }
}

/// Unify a callee's declared result against the caller's checked result type,
/// stripping `ref` layers on both sides first: a reference-returning call
/// spells its declared referent and the checked handle with differing layers.
fn unify_result(pattern: &Ty, actual: &Ty, bindings: &mut Bindings) -> Result<(), String> {
    let mut pattern = pattern;
    while let Ty::Ref(reference) = pattern {
        pattern = &reference.referent;
    }
    let mut actual = actual;
    while let Ty::Ref(reference) = actual {
        actual = &reference.referent;
    }
    unify(pattern, actual, bindings)
}

fn unify_arg(pattern: &TyArg, actual: &TyArg, bindings: &mut Bindings) -> Result<(), String> {
    match (pattern, actual) {
        (TyArg::Ty(p), TyArg::Ty(a)) => unify(p, a, bindings),
        (TyArg::Val(CtValue::Param(name)), TyArg::Val(value)) => bind_value(name, value, bindings),
        (TyArg::Val(p), TyArg::Val(a)) if p == a => Ok(()),
        (TyArg::Origin(_), TyArg::Origin(_)) => Ok(()),
        _ => Err("generic application arguments disagree".to_string()),
    }
}

fn bind_type(name: &str, ty: &Ty, bindings: &mut Bindings) -> Result<(), String> {
    if is_symbolic(ty) {
        return Err(format!("solution for `{name}` is not concrete: `{ty}`"));
    }
    match bindings.types.get(name) {
        Some(old) if old != ty => Err(format!(
            "conflicting solutions for `{name}`: `{old}` and `{ty}`"
        )),
        Some(_) => Ok(()),
        None => {
            bindings.types.insert(name.to_string(), ty.clone());
            Ok(())
        }
    }
}

fn bind_value(name: &str, value: &CtValue, bindings: &mut Bindings) -> Result<(), String> {
    if matches!(value, CtValue::Param(_)) {
        return Err(format!("solution for `{name}` is not constant"));
    }
    match bindings.values.get(name) {
        Some(old) if old != value => Err(format!(
            "conflicting solutions for `{name}`: `{old}` and `{value}`"
        )),
        Some(_) => Ok(()),
        None => {
            bindings.values.insert(name.to_string(), value.clone());
            Ok(())
        }
    }
}

fn substitute_function(function: &mut MirFunction, bindings: &Bindings) -> Result<(), MonoError> {
    for ty in &mut function.param_types {
        *ty = substitute_ty(ty, bindings)?;
    }
    for ty in function.var_tys.values_mut() {
        *ty = substitute_ty(ty, bindings)?;
    }
    for ty in function.reg_types.values_mut() {
        *ty = substitute_ty(ty, bindings)?;
    }
    if let Some(ty) = &mut function.ret_ty {
        *ty = substitute_ty(ty, bindings)?;
    }
    if let Some(ty) = &mut function.error_ty {
        *ty = substitute_ty(ty, bindings)?;
    }
    substitute_blocks_metadata(&mut function.blocks, bindings)
}

fn substitute_declaration(
    decl: &mut MirFunctionDeclaration,
    bindings: &Bindings,
) -> Result<(), MonoError> {
    for ty in &mut decl.param_types {
        *ty = substitute_ty(ty, bindings)?;
    }
    if let Some(ty) = &mut decl.variadic {
        *ty = substitute_ty(ty, bindings)?;
    }
    if let Some(ty) = &mut decl.kw_variadic {
        *ty = substitute_ty(ty, bindings)?;
    }
    decl.ret_ty = substitute_ty(&decl.ret_ty, bindings)?;
    if let Some(ty) = &mut decl.error_ty {
        *ty = substitute_ty(ty, bindings)?;
    }
    Ok(())
}

fn substitute_blocks_metadata(
    blocks: &mut [MirBlock],
    bindings: &Bindings,
) -> Result<(), MonoError> {
    for block in blocks {
        for instruction in &mut block.instrs {
            substitute_instruction(instruction, bindings)?;
        }
    }
    Ok(())
}

fn substitute_instruction(
    instruction: &mut MirInstr,
    bindings: &Bindings,
) -> Result<(), MonoError> {
    use MirInstr::*;
    match instruction {
        EstablishLoans { loans, .. } => {
            for loan in loans {
                substitute_place(&mut loan.place, bindings)?;
            }
        }
        MakeRef { place, .. }
        | MovePlace { place, .. }
        | Store { place, .. }
        | StoreRef { place, .. }
        | LoadPlace { place, .. }
        | ConsumePlace { place, .. } => substitute_place(place, bindings)?,
        MakeClosure { captures, .. } => {
            for capture in captures {
                substitute_place(&mut capture.place, bindings)?;
            }
        }
        MaterializeLiteral { target, .. }
        | PointerStorageTake {
            element: target, ..
        }
        | PointerStorageDestroy {
            element: target, ..
        }
        | UninitStorageTake {
            element: target, ..
        }
        | UninitStorageDestroy {
            element: target, ..
        } => *target = substitute_ty(target, bindings)?,
        Next {
            call: Some(call), ..
        } => substitute_iterator_call(call, bindings)?,
        TryNext {
            call, exhaustion, ..
        } => {
            substitute_iterator_call(call, bindings)?;
            *exhaustion = substitute_ty(exhaustion, bindings)?;
        }
        DefVar {
            binding_ty: Some(ty),
            ..
        } => *ty = substitute_ty(ty, bindings)?,
        Call {
            raises,
            arg_places,
            kwarg_places,
            ..
        } => {
            sub_opt_ty(raises, bindings)?;
            sub_places(arg_places, bindings)?;
            sub_places(kwarg_places, bindings)?;
        }
        CallIndirect {
            raises,
            callee_place,
            arg_places,
            kwarg_places,
            instantiated_contract,
            instantiated_args,
            ..
        } => {
            sub_opt_ty(raises, bindings)?;
            sub_place_opt(callee_place, bindings)?;
            sub_places(arg_places, bindings)?;
            sub_places(kwarg_places, bindings)?;
            sub_opt_ty(instantiated_contract, bindings)?;
            for arg in instantiated_args {
                *arg = substitute_arg(arg, bindings)?;
            }
        }
        MethodCall {
            raises,
            reference_result,
            recv_place,
            arg_places,
            kwarg_places,
            ..
        } => {
            sub_opt_ty(raises, bindings)?;
            sub_ref_opt(reference_result, bindings)?;
            sub_place_opt(recv_place, bindings)?;
            sub_places(arg_places, bindings)?;
            sub_places(kwarg_places, bindings)?;
        }
        Index {
            base_place,
            index_place,
            call,
            ..
        } => {
            sub_place_opt(base_place, bindings)?;
            sub_place_opt(index_place, bindings)?;
            if let Some(call) = call {
                substitute_subscript_call(call, bindings)?;
            }
        }
        Slice {
            object_place,
            arg_places,
            call,
            ..
        }
        | MultiIndex {
            object_place,
            arg_places,
            call,
            ..
        } => {
            sub_place_opt(object_place, bindings)?;
            sub_places(arg_places, bindings)?;
            if let Some(call) = call {
                substitute_subscript_call(call, bindings)?;
            }
        }
        MultiSet {
            receiver_place,
            arg_places,
            value_place,
            call,
            ..
        } => {
            sub_place_opt(receiver_place, bindings)?;
            sub_places(arg_places, bindings)?;
            sub_place_opt(value_place, bindings)?;
            substitute_subscript_call(call, bindings)?;
        }
        MakeTuple {
            element_types: Some(types),
            ..
        }
        | MakeVariant {
            alternatives: types,
            ..
        } => {
            for ty in types {
                *ty = substitute_ty(ty, bindings)?;
            }
        }
        VariantSet { place, .. }
        | VariantSetInitWith { place, .. }
        | VariantReplace { place, .. } => substitute_place(place, bindings)?,
        Try {
            body,
            handler,
            orelse,
            finalbody,
            ..
        } => {
            substitute_blocks_metadata(body, bindings)?;
            if let Some((_, b)) = handler {
                substitute_blocks_metadata(b, bindings)?;
            }
            if let Some(b) = orelse {
                substitute_blocks_metadata(b, bindings)?;
            }
            if let Some(b) = finalbody {
                substitute_blocks_metadata(b, bindings)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn substitute_subscript_call(
    call: &mut crate::mir::MirSubscriptCall,
    bindings: &Bindings,
) -> Result<(), MonoError> {
    sub_opt_ty(&mut call.raises, bindings)?;
    call.result_ty = substitute_ty(&call.result_ty, bindings)?;
    sub_ref_opt(&mut call.reference_result, bindings)
}

fn substitute_place(place: &mut MirPlace, bindings: &Bindings) -> Result<(), MonoError> {
    sub_opt_ty(&mut place.root_ty, bindings)?;
    for ty in &mut place.projection_tys {
        *ty = substitute_ty(ty, bindings)?;
    }
    sub_opt_ty(&mut place.ty, bindings)
}
fn sub_places(places: &mut [Option<MirPlace>], bindings: &Bindings) -> Result<(), MonoError> {
    for place in places {
        sub_place_opt(place, bindings)?;
    }
    Ok(())
}
fn sub_place_opt(place: &mut Option<MirPlace>, bindings: &Bindings) -> Result<(), MonoError> {
    if let Some(place) = place {
        substitute_place(place, bindings)?;
    }
    Ok(())
}
fn sub_opt_ty(ty: &mut Option<Ty>, bindings: &Bindings) -> Result<(), MonoError> {
    if let Some(ty) = ty {
        *ty = substitute_ty(ty, bindings)?;
    }
    Ok(())
}
fn sub_ref_opt(
    ty: &mut Option<crate::origin::RefTy>,
    bindings: &Bindings,
) -> Result<(), MonoError> {
    if let Some(ty) = ty {
        *ty.referent = substitute_ty(&ty.referent, bindings)?;
    }
    Ok(())
}

fn substitute_iterator_call(
    call: &mut crate::checked::CheckedIteratorCall,
    bindings: &Bindings,
) -> Result<(), MonoError> {
    call.result_ty = substitute_ty(&call.result_ty, bindings)?;
    sub_opt_ty(&mut call.raises, bindings)?;
    sub_ref_opt(&mut call.reference_result, bindings)?;
    Ok(())
}

fn substitute_ty(ty: &Ty, bindings: &Bindings) -> Result<Ty, MonoError> {
    let unsupported = |what: String| MonoError {
        function: None,
        construct: what,
    };
    Ok(match ty {
        Ty::Param { name, .. } => bindings
            .types
            .get(name)
            .cloned()
            .ok_or_else(|| unsupported(format!("unresolved type parameter `{name}`")))?,
        Ty::Struct(name, args) => {
            let was_symbolic = args.iter().any(arg_has_symbolic);
            let args = args
                .iter()
                .map(|arg| substitute_arg(arg, bindings))
                .collect::<Result<Vec<_>, _>>()?;
            let concrete_name =
                if !was_symbolic || args.iter().any(arg_has_symbolic) || args.is_empty() {
                    name.clone()
                } else {
                    crate::symbol::instance_symbol(
                        name,
                        &args
                            .iter()
                            .filter_map(|arg| match arg {
                                TyArg::Ty(ty) => Some(InstanceArg::Ty(ty.clone())),
                                TyArg::Val(value) => Some(InstanceArg::Value(value.clone())),
                                TyArg::Origin(_) => None,
                            })
                            .collect::<Vec<_>>(),
                    )
                };
            Ty::Struct(concrete_name, args)
        }
        Ty::Tuple(v) => Ty::Tuple(sub_types(v, bindings)?),
        Ty::RuntimePack(v) => Ty::RuntimePack(sub_types(v, bindings)?),
        Ty::Variant(v) => Ty::Variant(sub_types(v, bindings)?),
        Ty::Overload(v) => Ty::Overload(sub_types(v, bindings)?),
        Ty::ComptimeList(v) => Ty::ComptimeList(Box::new(substitute_ty(v, bindings)?)),
        Ty::VariadicPack(v) => Ty::VariadicPack(Box::new(substitute_ty(v, bindings)?)),
        Ty::Pointer { element, origin } => Ty::Pointer {
            element: Box::new(substitute_ty(element, bindings)?),
            origin: origin.clone(),
        },
        Ty::Ref(value) => {
            let mut value = value.clone();
            value.referent = Box::new(substitute_ty(&value.referent, bindings)?);
            Ty::Ref(value)
        }
        Ty::Dependent(DependentType::Indexed { elements, index }) => {
            let value = eval_ct(index, bindings)?;
            let index = match value {
                CtValue::Int(v) => usize::try_from(v).ok(),
                CtValue::UInt(v) => usize::try_from(v).ok(),
                _ => None,
            }
            .ok_or_else(|| {
                unsupported("dependent type index is not a non-negative integer".to_string())
            })?;
            substitute_ty(
                elements.get(index).ok_or_else(|| {
                    unsupported(format!("dependent type index {index} is out of range"))
                })?,
                bindings,
            )?
        }
        Ty::Assoc { .. } => {
            return Err(unsupported(format!(
                "associated type `{ty}` has no concrete MIR declaration fact"
            )));
        }
        Ty::SelfType | Ty::Infer | Ty::GenericFunc { .. } => {
            return Err(unsupported(format!("unresolved type `{ty}`")));
        }
        Ty::Func {
            environment,
            params,
            names,
            ret,
            required,
            variadic,
            kw_variadic,
            positional_only,
            keyword_only,
            raises,
            error,
            conventions,
            ref_params,
            ref_return,
            transfers,
        } => Ty::Func {
            environment: environment.clone(),
            params: sub_types(params, bindings)?,
            names: names.clone(),
            ret: Box::new(substitute_ty(ret, bindings)?),
            required: required.clone(),
            variadic: variadic
                .as_ref()
                .map(|t| substitute_ty(t, bindings).map(Box::new))
                .transpose()?,
            kw_variadic: kw_variadic
                .as_ref()
                .map(|t| substitute_ty(t, bindings).map(Box::new))
                .transpose()?,
            positional_only: *positional_only,
            keyword_only: *keyword_only,
            raises: *raises,
            error: error
                .as_ref()
                .map(|t| substitute_ty(t, bindings).map(Box::new))
                .transpose()?,
            conventions: conventions.clone(),
            ref_params: ref_params.clone(),
            ref_return: ref_return.clone(),
            transfers: transfers.clone(),
        },
        other => other.clone(),
    })
}

fn substitute_arg(arg: &TyArg, bindings: &Bindings) -> Result<TyArg, MonoError> {
    Ok(match arg {
        TyArg::Ty(ty) => TyArg::Ty(substitute_ty(ty, bindings)?),
        TyArg::Val(CtValue::Param(name)) => TyArg::Val(
            bindings
                .values
                .get(name)
                .cloned()
                .ok_or_else(|| MonoError {
                    function: None,
                    construct: format!("unresolved value parameter `{name}`"),
                })?,
        ),
        TyArg::Val(value) => TyArg::Val(value.clone()),
        TyArg::Origin(origin) => TyArg::Origin(origin.clone()),
    })
}
fn sub_types(types: &[Ty], bindings: &Bindings) -> Result<Vec<Ty>, MonoError> {
    types.iter().map(|ty| substitute_ty(ty, bindings)).collect()
}

fn eval_ct(expr: &CtExpr, bindings: &Bindings) -> Result<CtValue, MonoError> {
    use CtExpr::*;
    let int = |value: CtValue| match value {
        CtValue::Int(v) => Ok(v),
        _ => Err(MonoError {
            function: None,
            construct: "dependent expression requires an Int value".to_string(),
        }),
    };
    Ok(match expr {
        Value(CtValue::Param(name)) | Param(name) => bindings
            .values
            .get(name)
            .cloned()
            .ok_or_else(|| MonoError {
                function: None,
                construct: format!("unresolved value parameter `{name}`"),
            })?,
        Value(value) => value.clone(),
        Neg(v) => CtValue::Int(int(eval_ct(v, bindings)?)?.wrapping_neg()),
        Add(a, b) => {
            CtValue::Int(int(eval_ct(a, bindings)?)?.wrapping_add(int(eval_ct(b, bindings)?)?))
        }
        Sub(a, b) => {
            CtValue::Int(int(eval_ct(a, bindings)?)?.wrapping_sub(int(eval_ct(b, bindings)?)?))
        }
        Mul(a, b) => {
            CtValue::Int(int(eval_ct(a, bindings)?)?.wrapping_mul(int(eval_ct(b, bindings)?)?))
        }
        FloorDiv(a, b) => {
            CtValue::Int(int(eval_ct(a, bindings)?)?.div_euclid(int(eval_ct(b, bindings)?)?))
        }
        Mod(a, b) => {
            CtValue::Int(int(eval_ct(a, bindings)?)?.rem_euclid(int(eval_ct(b, bindings)?)?))
        }
        Pow(a, b) => CtValue::Int(int(eval_ct(a, bindings)?)?.wrapping_pow(
            u32::try_from(int(eval_ct(b, bindings)?)?).map_err(|_| MonoError {
                function: None,
                construct: "dependent exponent is out of range".to_string(),
            })?,
        )),
    })
}

fn is_symbolic(ty: &Ty) -> bool {
    match ty {
        Ty::Infer
        | Ty::Param { .. }
        | Ty::Assoc { .. }
        | Ty::Dependent(_)
        | Ty::SelfType
        | Ty::GenericFunc { .. } => true,
        Ty::Struct(_, args) => args.iter().any(arg_has_symbolic),
        Ty::Tuple(v) | Ty::RuntimePack(v) | Ty::Variant(v) | Ty::Overload(v) => {
            v.iter().any(is_symbolic)
        }
        Ty::ComptimeList(v) | Ty::VariadicPack(v) | Ty::Pointer { element: v, .. } => {
            is_symbolic(v)
        }
        Ty::Ref(v) => is_symbolic(&v.referent),
        Ty::Func {
            params,
            ret,
            variadic,
            kw_variadic,
            error,
            ..
        } => {
            params.iter().any(is_symbolic)
                || is_symbolic(ret)
                || variadic.as_deref().is_some_and(is_symbolic)
                || kw_variadic.as_deref().is_some_and(is_symbolic)
                || error.as_deref().is_some_and(is_symbolic)
        }
        _ => false,
    }
}
fn arg_has_symbolic(arg: &TyArg) -> bool {
    match arg {
        TyArg::Ty(ty) => is_symbolic(ty),
        TyArg::Val(CtValue::Param(_)) => true,
        TyArg::Val(_) | TyArg::Origin(_) => false,
    }
}
fn function_types(function: &MirFunction) -> impl Iterator<Item = &Ty> {
    function
        .param_types
        .iter()
        .chain(function.ret_ty.iter())
        .chain(function.error_ty.iter())
        .chain(function.var_tys.values())
        .chain(function.reg_types.values())
}
fn ensure_concrete_function(name: &str, function: &MirFunction) -> Result<(), MonoError> {
    if let Some(ty) = function_types(function).find(|ty| is_symbolic(ty)) {
        Err(MonoError {
            function: Some(name.to_string()),
            construct: format!("symbolic type `{ty}` remains after monomorphization"),
        })
    } else {
        Ok(())
    }
}
fn collect_nested_types(ty: &Ty, output: &mut Vec<Ty>) {
    match ty {
        Ty::Struct(_, args) => output.extend(args.iter().filter_map(|a| {
            if let TyArg::Ty(t) = a {
                Some(t.clone())
            } else {
                None
            }
        })),
        Ty::Tuple(v) | Ty::RuntimePack(v) | Ty::Variant(v) | Ty::Overload(v) => {
            output.extend(v.iter().cloned())
        }
        Ty::ComptimeList(v) | Ty::VariadicPack(v) | Ty::Pointer { element: v, .. } => {
            output.push((**v).clone())
        }
        Ty::Ref(v) => output.push((*v.referent).clone()),
        _ => {}
    }
}
fn nominal_template(name: &str) -> &str {
    name.split("$mono").next().unwrap_or(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn specialized_main(source: &str) -> SpecializedProgram {
        let compiler = crate::Compiler::default().with_snippet_module_scope();
        let compiled = compiler
            .compile_source(source, std::path::Path::new("mono_test.mojo"))
            .expect("compile iterator program");
        specialize(compiled.elaborated_mir(), &["main".to_string()])
            .expect("specialize iterator program")
    }

    fn instructions(blocks: &[MirBlock]) -> Vec<&MirInstr> {
        let mut result = Vec::new();
        for block in blocks {
            for instruction in &block.instrs {
                if let MirInstr::Try {
                    body,
                    handler,
                    orelse,
                    finalbody,
                    ..
                } = instruction
                {
                    result.extend(instructions(body));
                    if let Some((_, blocks)) = handler {
                        result.extend(instructions(blocks));
                    }
                    if let Some(blocks) = orelse {
                        result.extend(instructions(blocks));
                    }
                    if let Some(blocks) = finalbody {
                        result.extend(instructions(blocks));
                    }
                } else {
                    result.push(instruction);
                }
            }
        }
        result
    }

    fn function<'a>(program: &'a SpecializedProgram, name: &str) -> &'a MirFunction {
        &program
            .program
            .functions
            .iter()
            .find(|(known, _)| known == name)
            .unwrap_or_else(|| panic!("specialized program lacks `{name}`"))
            .1
    }

    #[test]
    fn bounded_user_iterator_types_the_split_slot_and_retargets_its_operations() {
        let source = "@fieldwise_init\n\
                      struct RangeIter:\n\
                      \x20   var cur: Int\n\
                      \x20   var stop: Int\n\
                      \n\
                      \x20   def __len__(self) -> Int:\n\
                      \x20       return self.stop - self.cur\n\
                      \n\
                      \x20   def __next__(mut self) -> Int:\n\
                      \x20       var v: Int = self.cur\n\
                      \x20       self.cur = self.cur + 1\n\
                      \x20       return v\n\
                      \n\
                      @fieldwise_init\n\
                      struct Countdown:\n\
                      \x20   var n: Int\n\
                      \n\
                      \x20   def __iter__(self) -> RangeIter:\n\
                      \x20       return RangeIter(0, self.n)\n\
                      \n\
                      def main():\n\
                      \x20   var total: Int = 0\n\
                      \x20   for x in Countdown(5):\n\
                      \x20       total = total + x\n\
                      \x20   print(total)\n";
        let specialized = specialized_main(source);
        let main = function(&specialized, "main");
        let instrs = instructions(&main.blocks);
        let (dest, prepare) = instrs
            .iter()
            .find_map(|instruction| match instruction {
                MirInstr::GetIter { dest, prepare, .. } => Some((*dest, prepare)),
                _ => None,
            })
            .expect("main normalizes its iterable");
        assert!(
            matches!(main.var_tys.get(&dest), Some(Ty::Struct(name, _)) if name == "RangeIter"),
            "the split iterator slot must be typed by the prepare chain: {:?}",
            main.var_tys.get(&dest)
        );
        for step in prepare {
            assert!(
                specialized
                    .program
                    .functions
                    .iter()
                    .any(|(name, _)| name == step),
                "prepare step `{step}` must name a specialized function"
            );
        }
        let method = instrs
            .iter()
            .find_map(|instruction| match instruction {
                MirInstr::HasNext {
                    method: Some(method),
                    ..
                } => Some(method),
                _ => None,
            })
            .expect("bounded iteration reads a length method");
        assert!(
            specialized
                .program
                .functions
                .iter()
                .any(|(name, _)| name == method),
            "`{method}` must name a specialized function"
        );
        let target = instrs
            .iter()
            .find_map(|instruction| match instruction {
                MirInstr::Next {
                    call: Some(call), ..
                } => Some(&call.target),
                _ => None,
            })
            .expect("bounded iteration advances through `__next__`");
        assert!(
            specialized
                .program
                .functions
                .iter()
                .any(|(name, _)| name == target),
            "`{target}` must name a specialized function"
        );
    }

    #[test]
    fn raising_range_iteration_types_the_slot_and_reaches_its_operations() {
        let specialized =
            specialized_main("def main():\n    for x in range(3):\n        print(x)\n");
        let main = function(&specialized, "main");
        let instrs = instructions(&main.blocks);
        let dest = instrs
            .iter()
            .find_map(|instruction| match instruction {
                MirInstr::GetIter { dest, .. } => Some(*dest),
                _ => None,
            })
            .expect("range iteration normalizes its iterable");
        assert!(
            matches!(main.var_tys.get(&dest), Some(Ty::Struct(..))),
            "the range iterator slot must be struct-typed: {:?}",
            main.var_tys.get(&dest)
        );
        let call = instrs
            .iter()
            .find_map(|instruction| match instruction {
                MirInstr::TryNext { call, .. } => Some(call),
                _ => None,
            })
            .expect("range iteration advances through a raising `__next__`");
        assert!(
            specialized
                .program
                .functions
                .iter()
                .any(|(name, _)| name == &call.target),
            "`{}` must name a specialized function",
            call.target
        );
    }

    #[test]
    fn generic_dispatch_iteration_unrolls_to_a_typed_concrete_chain() {
        let source = include_str!("../../assets/ok/generic_borrowed_dispatch_overloaded_iter.mojo");
        let specialized = specialized_main(source);
        let first_count = specialized
            .program
            .functions
            .iter()
            .find(|(name, _)| name.starts_with("first_count"))
            .expect("the generic loop body was specialized");
        let instrs = instructions(&first_count.1.blocks);
        let (dest, prepare) = instrs
            .iter()
            .find_map(|instruction| match instruction {
                MirInstr::GetIter { dest, prepare, .. } => Some((*dest, prepare)),
                _ => None,
            })
            .expect("the generic loop normalizes its iterable");
        assert!(
            !prepare
                .iter()
                .any(|step| step.starts_with("__trait_dispatch.")),
            "dispatch steps must resolve statically post-mono: {prepare:?}"
        );
        assert!(
            matches!(
                first_count.1.var_tys.get(&dest),
                Some(Ty::Struct(name, _)) if name.starts_with("CountIter")
            ),
            "the dispatched iterator slot must be concretely typed: {:?}",
            first_count.1.var_tys.get(&dest)
        );
    }

    #[test]
    fn structural_inference_rejects_conflicting_solutions() {
        let parameter = Ty::Param {
            name: "T".into(),
            bounds: vec![],
            callable_bound: None,
        };
        let mut bindings = Bindings::default();
        unify(&parameter, &Ty::Int, &mut bindings).unwrap();
        assert!(
            unify(&parameter, &Ty::Bool, &mut bindings)
                .unwrap_err()
                .contains("conflicting")
        );
    }

    #[test]
    fn substitution_resolves_nested_type_and_value_arguments() {
        let mut bindings = Bindings::default();
        bindings.types.insert("T".into(), Ty::UInt);
        bindings.values.insert("n".into(), CtValue::Int(4));
        let ty = Ty::Struct(
            "Buffer".into(),
            vec![
                TyArg::Ty(Ty::Param {
                    name: "T".into(),
                    bounds: vec![],
                    callable_bound: None,
                }),
                TyArg::Val(CtValue::Param("n".into())),
            ],
        );
        let Ty::Struct(name, args) = substitute_ty(&ty, &bindings).unwrap() else {
            panic!()
        };
        assert!(name.contains("$mono$"));
        assert_eq!(args, vec![TyArg::Ty(Ty::UInt), TyArg::Val(CtValue::Int(4))]);
    }
}
