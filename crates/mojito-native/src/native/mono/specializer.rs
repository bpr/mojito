//! The specialization driver: the worklist, instance naming and
//! materialization, block rewriting, and struct discovery.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_mir::mir::{CleanupDrop, MirTerm};
use mojito_types::types::TyKind;
use std::collections::BTreeMap;

impl<'a> Specializer<'a> {
    /// The elaborator of `source`.
    pub(super) fn new(source: &'a MirProgram, target: Option<&'a NativeTarget>) -> Self {
        Self {
            source,
            target,
            functions: bodies::Bodies::new(source),
            declarations: Catalog::new(
                source
                    .declarations
                    .functions
                    .iter()
                    .map(|d| (d.lowered_name.as_str(), d)),
                source.declarations.imported.as_ref(),
            ),
            structs: Catalog::new(
                source
                    .declarations
                    .structs
                    .iter()
                    .map(|d| (d.name.as_str(), d)),
                source.declarations.imported.as_ref(),
            ),
            struct_catalog: Rc::new(StructCatalog::new(&source.declarations)),
            queue: VecDeque::new(),
            instances: Vec::new(),
            instance_index: HashMap::new(),
            instance_names: HashMap::new(),
            references: Vec::new(),
            reference_frames: Vec::new(),
            lifecycle_instances: HashMap::new(),
            states: Vec::new(),
            demand_stack: Vec::new(),
            evaluations: HashMap::new(),
            fragments: HashMap::new(),
            applications: Rc::new(Applications {
                declared: source
                    .declarations
                    .functions
                    .iter()
                    .map(|declaration| declaration.lowered_name.clone())
                    .collect(),
                imported: source.declarations.imported.clone(),
                ..Applications::default()
            }),
            fuel: mojito_vm::crossing::CTFE_FUEL,
            vm: VmBackend::new(),
            discovered_types: HashSet::new(),
            output_functions: Vec::new(),
            output_positions: HashMap::new(),
            output_function_decls: Vec::new(),
            output_structs: Vec::new(),
            output_struct_positions: HashMap::new(),
            pending_struct_members: HashMap::new(),
            table_keys: HashMap::new(),
            conformance_answers: RefCell::default(),
            layout_structs: Rc::default(),
            constant_values: HashMap::new(),
            callable_targets: HashMap::new(),
            closure_captures: HashMap::new(),
            enclosing: Bindings::default(),
            folded_slots: HashSet::new(),
            unreached_failures: HashMap::new(),
            region_generators: HashMap::new(),
        }
    }

    /// Elaborate against `package` too: a callee the program does not
    /// define is the package's.
    pub(super) fn importing(mut self, package: Option<&'a PackageBodies>) -> Self {
        if let Some(package) = package {
            self.functions = self.functions.importing(package);
        }
        self
    }

    /// The declarations a reflected field-name list's construction names.
    pub(super) const fn construction_decls(&self) -> ConstructionDecls<'_, 'a> {
        ConstructionDecls {
            functions: &self.declarations,
        }
    }

    pub(super) fn run(mut self, entries: &[String]) -> Result<SpecializedProgram, MonoError> {
        let demanded = self.demand(entries);
        // A package declaration the elaboration reached that does not decode
        // was a miss to every lookup that reached it, so its failure outranks
        // whatever that miss made the elaboration conclude.
        let failures = self
            .source
            .declarations
            .imported
            .as_ref()
            .map(mojito_mir::mir::link::ImportedDeclarations::failures)
            .unwrap_or_default();
        if !failures.is_empty() {
            return Err(MonoError {
                kind: MonoErrorKind::Decode,
                function: None,
                construct: failures.join("; "),
            });
        }
        let entry_map = demanded?;
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
            self.functions.position(template).unwrap_or(usize::MAX)
        });
        self.output_function_decls.sort_by_key(|decl| {
            let template = instance_templates
                .get(decl.lowered_name.as_str())
                .copied()
                .unwrap_or(decl.lowered_name.as_str());
            self.functions.position(template).unwrap_or(usize::MAX)
        });
        let program = MirProgram {
            functions: self.output_functions,
            declarations: MirDeclarations {
                structs: self.output_structs,
                functions: self.output_function_decls,
                traits: self.source.declarations.traits.clone(),
                imported: None,
            },
            invariant_errors: self.source.invariant_errors.clone(),
        };
        // Each instance keeps its runtime structured control flow, as
        // upstream's elaborator emits `hlcf`; the VM runs it, and Pliron
        // lowers it to LLVM blocks.
        let program = ConcreteMir::verified(program).map_err(|findings| MonoError {
            kind: MonoErrorKind::Unsupported,
            function: None,
            construct: format!(
                "specialized MIR that does not verify: {}",
                findings.join("; ")
            ),
        })?;
        let counts = elaboration_counts(&self.instances, &self.functions, &program);
        Ok(SpecializedProgram {
            program,
            entries: entry_map,
            counts,
        })
    }

    /// Demand every entry of `entries` and elaborate all it reaches,
    /// answering each entry's instance name.
    fn demand(&mut self, entries: &[String]) -> Result<HashMap<String, String>, MonoError> {
        let mut entry_map = HashMap::new();
        for entry in entries {
            let decl = self.declarations.get(entry.as_str());
            self.functions.get(entry.as_str()).ok_or_else(|| {
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
        self.drain()?;
        Ok(entry_map)
    }

    pub(super) fn base_bindings(&self) -> Bindings {
        Bindings {
            structs: Rc::clone(&self.struct_catalog),
            applications: Some(Rc::clone(&self.applications)),
            ..Bindings::default()
        }
    }

    /// The own parameters of `template` its body never reads, which the
    /// pre-elaboration pipeline removed: no instance's identity holds them.
    /// A template that also removed an argument removed them from the
    /// generator a direct call names, so they are those of a `reduced`
    /// demand alone; any other keeps every parameter.
    pub(super) fn removed_params(
        &self,
        template: &str,
        reduced: bool,
    ) -> &'a [mojito_types::param_expr::ParamId] {
        self.functions
            .get(template)
            .filter(|function| reduced || function.removed_args.is_empty())
            .map_or(&[], |function| function.removed_params.as_slice())
    }

    /// Whether a demand of `template` that `bindings` carry takes the
    /// generator with its unused arguments removed ([`InstanceKey::reduced`]).
    pub(super) fn reduces(&self, template: &str, bindings: &Bindings) -> bool {
        bindings.reduced
            && self
                .functions
                .get(template)
                .is_some_and(|function| !function.removed_args.is_empty())
    }

    /// Whether `target`'s reduced generator removed its method receiver
    /// ([`MirFunction::removed_args`]), so a call of it passes none.
    pub(super) fn removes_receiver(&self, target: &str) -> bool {
        self.declarations
            .get(target)
            .is_some_and(|declaration| declaration.has_receiver)
            && self
                .functions
                .get(target)
                .is_some_and(|function| function.removed_args.contains(&0))
    }

    /// Drop from a direct call of `target`'s reduced generator the
    /// arguments that bind the parameters it removed, as upstream rewrites
    /// each call of a generator it removed an argument from: the call binds
    /// against `target`'s full declaration, and a positional or keyword
    /// argument leaves with its place, while an omitted one, whose default
    /// is never evaluated, leaves nothing. A method call's arguments follow
    /// its receiver, so a removed slot past the receiver binds the
    /// declaration's position one lower. It gives the operand registers
    /// the call no longer passes.
    pub(super) fn drop_removed_arguments(
        &self,
        owner: &str,
        target: &str,
        args: &mut Vec<Reg>,
        arg_places: &mut Vec<Option<MirPlace>>,
        kwargs: &mut Vec<(String, Reg)>,
        kwarg_places: &mut Vec<Option<MirPlace>>,
    ) -> Result<Vec<Reg>, MonoError> {
        let (Some(declaration), Some(function)) =
            (self.declarations.get(target), self.functions.get(target))
        else {
            return Ok(Vec::new());
        };
        let keywords: Vec<&str> = kwargs.iter().map(|(name, _)| name.as_str()).collect();
        let (positional, keyword) = mojito_mir::mir::removed_argument_operands(
            declaration,
            &function.removed_args,
            args.len(),
            &keywords,
        )
        .map_err(|e| {
            self.error(
                Some(owner),
                format!("binding call to `{target}` during argument removal: {e:?}"),
            )
        })?;
        let mut dropped = Vec::with_capacity(positional.len() + keyword.len());
        for &index in positional.iter().rev() {
            dropped.push(args.remove(index));
            if index < arg_places.len() {
                arg_places.remove(index);
            }
        }
        for &index in keyword.iter().rev() {
            dropped.push(kwargs.remove(index).1);
            if index < kwarg_places.len() {
                kwarg_places.remove(index);
            }
        }
        Ok(dropped)
    }

    /// [`Self::removed_params`] of `template` as binders, each with the
    /// spelling its declaration gives it: its own, an enclosing body's, or
    /// its struct's.
    fn removed_binders(&self, template: &str, reduced: bool) -> Vec<ParamRef> {
        let removed = self.removed_params(template, reduced);
        if removed.is_empty() {
            return Vec::new();
        }
        let scope = self.binder_scope(template);
        removed
            .iter()
            .map(|id| {
                scope
                    .iter()
                    .chain(
                        self.structs
                            .get(&id.owner)
                            .into_iter()
                            .flat_map(|owner| &owner.param_decls),
                    )
                    .find(|decl| decl.id() == id)
                    .map_or_else(
                        || ParamRef {
                            id: id.clone(),
                            name: id.owner.clone(),
                        },
                        ParamDecl::binder,
                    )
            })
            .collect()
    }

    /// Erase the parameters of the struct an owner-less method instance
    /// removed from the `self` its bare struct spelling stands for: the
    /// instance takes every struct instance agreeing on the kept arguments
    /// (`Box[_REMOVED_ARG]`), as upstream's generator takes its `self`.
    fn erase_removed_owner(&self, bindings: &mut Bindings) -> Result<(), MonoError> {
        let Some((owner, _)) = &bindings.self_instance else {
            return Ok(());
        };
        let Some(decls) = self
            .structs
            .get(owner.as_str())
            .map(|owner| &owner.param_decls)
        else {
            return Ok(());
        };
        if !decls
            .iter()
            .any(|decl| bindings.removed.contains(&decl.binder()))
        {
            return Ok(());
        }
        let spelled: Ty = TyKind::Struct(
            owner.clone(),
            decls.iter().map(ParamDecl::own_argument).collect(),
        )
        .into();
        let erased = substitute_ty(&spelled, bindings)?;
        bindings.self_instance = Some((owner.clone(), erased));
        Ok(())
    }

    /// The instance arguments of `target`'s parameters `decls` under
    /// `bindings`: [`bound_argument`] of each past the `owner_covered` decls that
    /// restate the owner's parameters, without those the body removed.
    pub(super) fn own_arguments(
        &self,
        decls: &[ParamDecl],
        bindings: &Bindings,
        target: &str,
        owner_covered: usize,
    ) -> Result<Vec<InstanceArg>, MonoError> {
        let removed = self.removed_params(target, bindings.reduced);
        decls
            .iter()
            .skip(owner_covered)
            .filter(|decl| !removed.contains(decl.id()))
            .map(|decl| bound_argument(decl, bindings, target))
            .collect()
    }

    /// Demand the instance of `template` under `bindings`. A member whose
    /// `where` clause these bindings disprove is not part of its owner's
    /// instance, so demanding it is an error.
    pub(super) fn enqueue(
        &mut self,
        template: &str,
        mut bindings: Bindings,
        arguments: Vec<InstanceArg>,
    ) -> Result<String, MonoError> {
        bindings.erase_origins();
        bindings.reduced = self.reduces(template, &bindings);
        let member = self.table_owner(template, &bindings);
        if let Some(declaration) = self.declarations.get(template)
            && let Availability::Disproven(message) = self.availability(declaration, &bindings)
        {
            let instance = bindings
                .self_instance
                .as_ref()
                .map_or_else(|| "this instance".to_string(), |(_, ty)| format!("`{ty}`"));
            return Err(self.error(
                Some(template),
                format!(
                    "member `{template}`, which is unavailable in {instance}{}",
                    message.map_or_else(String::new, |message| format!(": {message}"))
                ),
            ));
        }
        let mut arguments = arguments;
        let owner = match bindings.self_instance.as_ref() {
            // A method that removed one of its struct's parameters is a
            // generator over the ones it kept, as upstream's method
            // generator lists them before its own: it names no owner
            // instance, and its kept owner arguments lead its own: a
            // lifecycle or dispatched member too, which each struct
            // instance's member table names, and a constructor, which each
            // construction names.
            Some((owner_template, _))
                if self
                    .removed_params(template, bindings.reduced)
                    .iter()
                    .any(|id| *id.owner == **owner_template) =>
            {
                let removed = self.removed_params(template, bindings.reduced);
                let decls = self
                    .structs
                    .get(owner_template.as_str())
                    .map_or(&[][..], |owner| owner.param_decls.as_slice());
                arguments = decls
                    .iter()
                    .filter(|decl| !removed.contains(decl.id()))
                    .map(|decl| bound_argument(decl, &bindings, template))
                    .chain(arguments.into_iter().map(Ok))
                    .collect::<Result<_, _>>()?;
                None
            }
            Some((_, instance)) if let TyKind::Struct(name, _) = instance.kind() => {
                Some(name.clone())
            }
            _ => None,
        };

        // Origins erase from the runtime ABI and from the instance symbol, so
        // two arguments differing only in an origin name the same native
        // instance; keying on them would mint a second instance under the same
        // symbol and collide with the first.
        let key = InstanceKey {
            template: template.to_string(),
            arguments: arguments
                .into_iter()
                .map(|argument| match argument {
                    InstanceArg::Ty(ty) => InstanceArg::Ty(origin_erased(&ty)),
                    value @ InstanceArg::Value(_) => value,
                })
                .collect(),
            owner,
            reduced: bindings.reduced,
        };
        if let Some(&index) = self.instance_index.get(&key) {
            let name = self.instances[index].1.clone();
            self.record_struct_member(member.as_ref(), &name);
            self.note_reference(&name);
            return Ok(name);
        }
        // The one elaboration bound: counted where an instance is demanded,
        // so a wide and a deep explosion both stop here.
        if self.instances.len() >= INSTANCE_BUDGET {
            return Err(self.error(
                Some(template),
                format!("instantiation past the {INSTANCE_BUDGET}-instance budget"),
            ));
        }
        // A generic struct's method takes its concrete owner's spelling
        // (`List$mono$TInt.grow`), so lowering's name-composed constructor
        // and overload lookups against the instance struct name keep working.
        // A template whose removed parameters left no argument is still an
        // instance (`f$mono`), never the template itself.
        let spelled = !key.arguments.is_empty()
            || self
                .removed_params(template, key.reduced)
                .iter()
                .any(|id| key.owner.is_none() || !self.structs.contains_key(&id.owner));
        // The generator a direct call names with the unused arguments
        // removed is a clone of its own, as upstream's `_REMOVED_ARG`.
        let reduced_template;
        let template_spelling = if key.reduced {
            reduced_template = mojito_symbol::symbol::removed_argument_symbol(template);
            reduced_template.as_str()
        } else {
            template
        };
        let name = if let Some(owner) = &key.owner {
            let base = mojito_symbol::symbol::retarget_method_symbol(template_spelling, owner)
                .ok_or_else(|| {
                    self.error(
                        Some(template),
                        format!("owner-bound instance `{template}` is not a method symbol"),
                    )
                })?;
            if spelled {
                mojito_symbol::symbol::instance_symbol(&base, &key.arguments)
            } else {
                base
            }
        } else if spelled {
            mojito_symbol::symbol::instance_symbol(template_spelling, &key.arguments)
        } else {
            template_spelling.to_string()
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
            self.record_struct_member(member.as_ref(), &name);
            self.note_reference(&name);
            return Ok(name);
        }
        if (name != template && self.functions.contains_key(name.as_str()))
            || self.instance_names.contains_key(&name)
        {
            return Err(self.error(
                Some(template),
                format!("concrete instance symbol `{name}` collides with an existing declaration"),
            ));
        }
        self.instance_index
            .insert(key.clone(), self.instances.len());
        self.instance_names
            .insert(name.clone(), self.instances.len());
        // A destructor, a copy, and a move constructor are references of
        // each body that drops, copies, or moves its struct
        // (`enqueue_lifecycle_members`), not members of every value's
        // closure.
        if let Some((owner, method)) = mojito_symbol::symbol::split_method_symbol(&name)
            && ["__init__", "__del__"]
                .iter()
                .any(|lifecycle| method.starts_with(lifecycle))
        {
            self.lifecycle_instances
                .entry(owner.to_string())
                .or_default()
                .push(self.instances.len());
        }
        self.instances.push((key.clone(), name.clone()));
        self.states.push(InstanceState::Pending);
        self.references.push(Vec::new());
        self.queue.push_back((key, bindings));
        self.record_struct_member(member.as_ref(), &name);
        self.note_reference(&name);
        Ok(name)
    }

    /// The concrete struct whose member table records an instance of
    /// `template` under `bindings`, with the key it records it under: its
    /// owner instance, else its non-generic owner, when `template` is a
    /// member a backend reaches from its struct's type
    /// ([`Self::table_key`]). `None` for any other function.
    fn table_owner(&mut self, template: &str, bindings: &Bindings) -> Option<(String, String)> {
        let key = self.table_key(template)?;
        let name = match bindings.self_instance.as_ref().map(|(_, ty)| ty.kind()) {
            Some(TyKind::Struct(name, _)) => name.clone(),
            _ => mojito_symbol::symbol::split_method_symbol(template)?
                .0
                .to_string(),
        };
        Some((name, key))
    }

    /// The key a struct's member table records an instance of the method
    /// `template` under, when a backend reaches it from the struct's type:
    /// a lifecycle member under its bare name (`__deinit__`); a dispatched
    /// member (`mir::table_member`), another special method, or a
    /// requirement of a trait the struct declares
    /// (`MirDeclarations::reached_from_type`) under the template's method
    /// spelling, overload qualifier included (`__eq__$ov$Self`), as
    /// upstream's struct nests these members and its conformances' witness
    /// tables name them. `None` for any other function, and for a
    /// constructor, which only the keyword collector's construction, which
    /// no call names, records ([`constructed_member`]).
    fn table_key(&mut self, template: &str) -> Option<String> {
        if let Some(key) = self.table_keys.get(template) {
            return key.clone();
        }
        let key =
            mojito_symbol::symbol::split_method_symbol(template).and_then(|(owner, method)| {
                let base = method.split('$').next().unwrap_or(method);
                if mojito_mir::mir::CONSTRUCTED_MEMBERS.contains(&base) {
                    return None;
                }
                if mojito_mir::mir::LIFECYCLE_MEMBERS.contains(&base) {
                    return Some(base.to_string());
                }
                (mojito_mir::mir::table_member(method).is_some()
                    || self.source.declarations.reached_from_type(owner, method))
                .then(|| method.to_string())
            });
        self.table_keys.insert(template.to_string(), key.clone());
        key
    }

    /// Record `instance` under the key `member` names in its struct's
    /// output declaration's member table, or in the one it gets when
    /// declared, as upstream's struct nests the member instances it owns.
    fn record_struct_member(&mut self, member: Option<&(String, String)>, instance: &str) {
        let Some((name, key)) = member else {
            return;
        };
        if let Some(&position) = self.output_struct_positions.get(name) {
            self.output_structs[position].record_member(key, instance);
            return;
        }
        let pending = self.pending_struct_members.entry(name.clone()).or_default();
        if !pending
            .iter()
            .any(|(known, symbol)| known == key && symbol == instance)
        {
            pending.push((key.clone(), instance.to_string()));
        }
    }

    /// Materialize every pending instance on the queue, the instances their
    /// bodies enqueue included.
    fn drain(&mut self) -> Result<(), MonoError> {
        while let Some((key, bindings)) = self.queue.pop_front() {
            if self.states[self.instance_index[&key]] != InstanceState::Pending {
                continue;
            }
            self.materialize(&key, &bindings)?;
        }
        Ok(())
    }

    /// Record `name` as a reference of the instance being materialized.
    fn note_reference(&mut self, name: &str) {
        if let Some(frame) = self.reference_frames.last_mut() {
            frame.push(name.to_string());
        }
    }

    /// Materialize the reference closure of the instance at `root` — the
    /// instances its body demanded, the destructors its drops run among
    /// them, the other lifecycle members of every struct its types name,
    /// theirs, and so on — and return the positions of its
    /// completed members. A pending instance off the closure stays queued
    /// for the outer drain, so a failure there is not this demand's. A path
    /// into an instance being materialized is a cycle in the parameter
    /// domain.
    fn materialize_closure(&mut self, root: usize) -> Result<Vec<usize>, MonoError> {
        // Each member beside whether a call reaches it, rather than only a
        // value of its struct.
        let mut pending = vec![(root, true)];
        let mut seen = HashSet::new();
        let mut structs = HashSet::new();
        let mut closure = Vec::new();
        while let Some((index, called)) = pending.pop() {
            match self.states[index] {
                // A lifecycle member being materialized is the struct whose
                // layout this evaluation answers; no call reaches it.
                InstanceState::Active if !called => continue,
                _ if !seen.insert(index) => continue,
                InstanceState::Pending => {
                    let key = self.instances[index].0.clone();
                    let bindings = self
                        .queue
                        .iter()
                        .find(|(queued, _)| *queued == key)
                        .map(|(_, bindings)| bindings.clone())
                        .expect("a pending instance is queued");
                    self.materialize(&key, &bindings)?;
                }
                InstanceState::Failed => {
                    return Err(self.error(
                        None,
                        format!(
                            "compile-time application reaches `{}`, which failed to elaborate",
                            self.instances[index].1
                        ),
                    ));
                }
                // The fragment would call an instance whose body is still
                // being written: the evaluation requires itself.
                InstanceState::Active => {
                    let path = self
                        .demand_stack
                        .iter()
                        .map(String::as_str)
                        .chain(std::iter::once(self.instances[index].1.as_str()))
                        .collect::<Vec<_>>()
                        .join(" -> ");
                    return Err(self.error(
                        None,
                        format!(
                            "function instantiation in parameter domain that recursively \
                             requires itself: {path}"
                        ),
                    ));
                }
                InstanceState::Completed => {}
            }
            if self.states[index] == InstanceState::Completed {
                closure.push(index);
                let lifecycle = self.lifecycle_references(index, &mut structs);
                pending.extend(lifecycle.into_iter().map(|member| (member, false)));
            }
            pending.extend(
                self.references[index]
                    .iter()
                    .filter_map(|name| self.instance_names.get(name).map(|&member| (member, true))),
            );
        }
        Ok(closure)
    }

    /// The lifecycle members of the structs the completed instance at
    /// `index` names in its types, fields of fields included, that no
    /// earlier member named (`structs`).
    fn lifecycle_references(&self, index: usize, structs: &mut HashSet<String>) -> Vec<usize> {
        let Some(&position) = self.output_positions.get(&self.instances[index].1) else {
            return Vec::new();
        };
        let function = &self.output_functions[position].1;
        let mut types: Vec<Ty> = function_types(function).cloned().collect();
        push_instruction_types(&function.blocks, &mut types);
        let mut references = Vec::new();
        while let Some(ty) = types.pop() {
            collect_nested_types(&ty, &mut types);
            let TyKind::Struct(name, _) = ty.kind() else {
                continue;
            };
            if !structs.insert(name.clone()) {
                continue;
            }
            if let Some(&declared) = self.output_struct_positions.get(name) {
                types.extend(
                    self.output_structs[declared]
                        .fields
                        .iter()
                        .map(|(_, ty)| ty.clone()),
                );
            }
            references.extend(self.lifecycle_instances.get(name).into_iter().flatten());
        }
        references
    }

    pub(super) fn instance_name(&self, key: &InstanceKey) -> &str {
        let index = self.instance_index[key];
        self.instances[index].1.as_str()
    }

    /// Bind the callable parameters this call binds to a closure that
    /// captures, but those the callee removed, answering the trailing arguments the call passes for them
    /// and the loads those arguments need first. An environment survives no
    /// name: a generic closure's instance folds the lifted body and takes
    /// the captured places as reference parameters, and any other closure
    /// is passed whole, a runtime parameter its instance calls indirectly.
    pub(super) fn bind_capturing_callables(
        &self,
        owner: &str,
        function: &mut MirFunction,
        target: &str,
        param_args: &[mojito_mir::mir::MirParamArg],
        bindings: &mut Bindings,
    ) -> Result<CapturingArguments, MonoError> {
        bindings.runtime_callables.clear();
        bindings.callable_environments.clear();
        let mut trailing = CapturingArguments::default();
        let Some(declaration) = self.declarations.get(target) else {
            return Ok(trailing);
        };
        let removed = self.removed_params(target, bindings.reduced);
        for (decl, reg, _) in
            mojito_mir::mir::matched_parameter_arguments(&declaration.param_decls, param_args)
        {
            let ParamDecl::Value { ty, .. } = decl else {
                continue;
            };
            if removed.contains(decl.id()) {
                continue;
            }
            let generic = match peel_refs(ty).kind() {
                TyKind::GenericFunc(_) => true,
                TyKind::Func(_) => false,
                _ => continue,
            };
            let Some((callee, false)) = self.callable_targets.get(&reg.0) else {
                continue;
            };
            if !generic {
                bindings.runtime_callables.push(decl.binder());
                trailing.closures.push(reg);
                continue;
            }
            let captured = self.capture_arguments(owner, function, callee)?;
            let environment = captured
                .iter()
                .map(|(reg, _)| function.reg_types[&reg.0].clone())
                .collect();
            bindings
                .callable_environments
                .push((decl.binder(), callee.clone(), environment));
            trailing.environment.extend(captured);
        }
        Ok(trailing)
    }

    /// The values a call through a callable contract gives each of the
    /// contract's compile-time parameters, in order: a supplied argument, or
    /// the contract's default, which overrides the implementation's own. A
    /// parameter neither fills stays deferred, for the implementation's
    /// default.
    pub(super) fn contract_values(
        &self,
        target: &str,
        contract: &[ParamDecl],
        param_args: &[mojito_mir::mir::MirParamArg],
    ) -> Result<Vec<TyArg>, MonoError> {
        if contract.is_empty() {
            return Ok(Vec::new());
        }
        let mut bindings = self.base_bindings();
        bind_explicit_value_arguments(
            contract,
            &[],
            param_args,
            &self.constant_values,
            &mut bindings,
            target,
            &self.enclosing,
        )?;
        apply_defaults(contract, &mut bindings)?;
        // A default is evaluated as a literal; it binds at the parameter's
        // declared type, as the VM materializes it.
        Ok(contract
            .iter()
            .map(|decl| {
                let value = bindings.values.get(&decl.binder()).cloned();
                match (decl, value) {
                    (ParamDecl::Value { ty, .. }, Some(value)) => {
                        TyArg::Val(value.clone().materialize_as(ty).unwrap_or(value))
                    }
                    (_, Some(value)) => TyArg::Val(value),
                    (_, None) => TyArg::Val(CtValue::Deferred(decl.binder())),
                }
            })
            .collect())
    }

    pub(super) fn materialize(
        &mut self,
        key: &InstanceKey,
        bindings: &Bindings,
    ) -> Result<(), MonoError> {
        let index = self.instance_index[key];
        self.states[index] = InstanceState::Active;
        self.reference_frames.push(Vec::new());
        let result = self.materialize_body(key, bindings);
        self.references[index] = self.reference_frames.pop().unwrap_or_default();
        self.states[index] = if result.is_ok() {
            InstanceState::Completed
        } else {
            InstanceState::Failed
        };
        result
    }

    fn materialize_body(
        &mut self,
        key: &InstanceKey,
        bindings: &Bindings,
    ) -> Result<(), MonoError> {
        let name = self.instance_name(key).to_string();
        // A package record the body's materialization reached that does
        // not decode outranks the symptom its miss caused, as the program's
        // own view's does in `run`.
        let reached = self.functions.reached(&key.template);
        if !reached.is_empty() {
            return Err(MonoError {
                kind: MonoErrorKind::Decode,
                function: Some(key.template.clone()),
                construct: reached.join("; "),
            });
        }
        let findings = self.functions.findings(&key.template);
        let template = self.functions.get(key.template.as_str()).ok_or_else(|| {
            let reason = if findings.is_empty() {
                String::new()
            } else {
                format!(": {}", findings.join("; "))
            };
            self.error(
                Some(&key.template),
                format!("callee `{}` has no MIR body{reason}", key.template),
            )
        })?;
        if !findings.is_empty() {
            return Err(MonoError {
                kind: MonoErrorKind::Verify,
                function: Some(key.template.clone()),
                construct: findings.join("; "),
            });
        }
        // Each direct call of a generator that removed an argument passes
        // it no operand, as upstream's `RemoveUnusedParams` rewrote the call
        // to name the clone without it, so the instance computes none, nor
        // the default of one the call leaves out. The callees whose removals
        // the body was read through answer first, then every other callee
        // ([`Self::drop_removed_arguments`]).
        let arguments = mojito_mir::mir::with_removed_argument_forwards_erased(
            template,
            &self.source.declarations,
            &|callee| {
                template
                    .callee_removed_args
                    .iter()
                    .find(|(name, _)| name == callee)
                    .map(|(_, slots)| slots.as_slice())
                    .or_else(|| {
                        self.functions
                            .get(callee)
                            .map(|function| function.removed_args.as_slice())
                    })
            },
        );
        let template = arguments.as_ref().map_or(template, |(view, _)| view);
        let undefined_operands = arguments.as_ref().map(|(_, operands)| operands.clone());
        // The generator a direct call names has its unused arguments
        // removed, from its body and its declaration alike.
        let reduced = key
            .reduced
            .then(|| self.declarations.get(key.template.as_str()))
            .flatten()
            .map(|declaration| {
                mojito_mir::mir::without_removed_arguments(
                    template,
                    declaration,
                    &template.removed_args,
                )
            });
        let template = reduced.as_ref().map_or(template, |(view, _)| view);
        // Each call of a generator that removed a parameter passes it no
        // argument, as upstream's `RemoveUnusedParams` rewrote the call, so
        // the instance evaluates none.
        let forwarded = mojito_mir::mir::with_removed_forwards_erased(
            &key.template,
            template,
            &self.source.declarations,
            &|callee| Some(self.removed_params(callee, true)),
        );
        let template = forwarded.as_ref().unwrap_or(template);
        let scope = &self.binder_scope(&key.template);
        let mut removed = bindings.removed.clone();
        removed.extend(
            self.removed_binders(&key.template, key.reduced)
                .into_iter()
                .filter(|binder| {
                    key.owner.is_none() || !self.structs.contains_key(&binder.id.owner)
                }),
        );
        let mut bindings = Bindings {
            layout: self.layout_oracle(),
            applications: Some(Rc::clone(&self.applications)),
            removed,
            ..bindings.clone()
        };
        if key.owner.is_none() {
            self.erase_removed_owner(&mut bindings)?;
        }
        let bindings = &bindings;
        // A type or a parameter argument may carry the application of a
        // lowered function (`SIMD[dt, L[0]]`), which only running it
        // answers: each one a substitution meets is demanded, and the
        // template is substituted again with its value.
        let mut function = loop {
            self.applications.pending.borrow_mut().clear();
            self.unreached_failures.clear();
            let substituted = self.substituted(&key.template, template, scope, bindings);
            if !self.answer_pending(&key.template, bindings)? {
                break substituted?;
            }
        };
        let unreached_failures = std::mem::take(&mut self.unreached_failures);
        function.removed_params.clear();
        function.callee_removed_params.clear();
        function.callee_removed_args.clear();
        function.removed_args.clear();
        expand_pack_spreads(&key.template, &mut function)?;
        self.select_comptime_branches(&key.template, &mut function, bindings)?;
        failure::discharge_comptime_asserts(&mut function).map_err(|construct| MonoError {
            kind: MonoErrorKind::Instantiation,
            ..self.error(Some(&key.template), construct)
        })?;
        self.discharge_instantiation_failures(&key.template, &function)?;
        self.discharge_rebinds(&key.template, &mut function)?;
        self.answer_param_constants(
            &key.template,
            &mut function.blocks,
            bindings,
            &mut function.n_regs,
            &mut function.reg_types,
            true,
        )
        .and_then(|()| failure::reached_failure(&function.blocks, unreached_failures))
        .map_err(|mut error| {
            error.function.get_or_insert_with(|| key.template.clone());
            error
        })?;
        self.close_lane_masks(&key.template, &mut function)?;
        self.folded_slots = self
            .folded_parameter_values(&key.template, &function, bindings)
            .into_keys()
            .collect();
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
        // A generic closure's environment follows them: the body folds the
        // lifted body's name, and a call through the parameter passes these
        // reference parameters as the environment of its specialization.
        let environments: Vec<(String, Vec<mojito_hir::hir::VarId>)> = bindings
            .callable_environments
            .iter()
            .map(|(parameter, target, types)| {
                let slots = append_environment_parameters(&mut function, &parameter.name, types);
                (target.clone(), slots)
            })
            .collect();
        self.enclosing.clone_from(bindings);
        self.instantiate_nested_bodies(&key.template, &mut function.blocks)?;
        self.constant_values = function_constant_values(&function);
        self.callable_targets = callable_targets(&function);
        self.closure_captures = function_closure_captures(&function);
        for (target, slots) in &environments {
            let captures = slots
                .iter()
                .map(|&slot| MirClosureCapture {
                    place: MirPlace {
                        through: Some(slot),
                        ..MirPlace::root(slot, function.var_tys.get(&slot).cloned())
                    },
                    mode: MirCaptureMode::Reference,
                    constructor: None,
                    destructor: None,
                })
                .collect();
            self.closure_captures.insert(target.clone(), captures);
            for (callee, captures_are_empty) in self.callable_targets.values_mut() {
                if callee == target {
                    *captures_are_empty = false;
                }
            }
        }
        self.constant_values.extend(
            self.callable_targets
                .iter()
                .map(|(reg, (target, _))| (*reg, CtValue::Str(target.clone()))),
        );
        // Take the blocks out so call rewriting can read the function's
        // substituted register-type table without aliasing its body.
        let mut blocks = std::mem::take(&mut function.blocks);
        // Iterator normalization first: the advance rewrites of each
        // `HasNext`/`Next`/`TryNext` need the iterator slot types this pass
        // records.
        self.rewrite_iterator_inits(&key.template, &mut function, &mut blocks)?;
        let arguments = parameter_argument_regs(&mut blocks);
        self.rewrite_blocks(&key.template, &mut function, &mut blocks)?;
        function.blocks = blocks;
        erase_dropped_argument_reads(&mut function, &arguments);
        if let Some(operands) = undefined_operands {
            shed_dropped_operand_types(&mut function, &operands);
        }
        retire_forwarded_packs(&mut function);
        erase_specialized_generic_callable_storage(&mut function);

        if let Some(declaration) = reduced
            .map(|(_, declaration)| declaration)
            .or_else(|| self.declarations.get(key.template.as_str()).cloned())
        {
            let mut declaration = declaration;
            substitute_declaration(&mut declaration, bindings)?;
            erase_generic_callable_parameters(&mut declaration.param_types);
            declaration.lowered_name.clone_from(&name);
            declaration.param_decls.clear();
            declaration.availability.clear();
            declaration.enclosing = None;
            for (parameter, ty) in promoted {
                declare_runtime_parameter(&mut declaration, &parameter, ty);
            }
            for (target, slots) in environments {
                let body = &self.declarations[target.as_str()];
                for (position, slot) in slots.into_iter().enumerate() {
                    declare_environment_parameter(
                        &mut declaration,
                        function.var_names[slot as usize].clone(),
                        function.param_types[slot as usize].clone(),
                        body,
                        position,
                    );
                }
            }
            self.instantiate_constructed_defaults(&key.template, &mut declaration)?;
            self.enqueue_keyword_collector(&key.template, &declaration)?;
            self.output_function_decls.push(declaration);
        }
        self.discover_structs(&key.template, &function)?;
        self.name_lifecycle_calls(&key.template, &mut function)?;
        // An instance's destructor names only a member available in it.
        mojito_mir::mir::lower_whole_drops(&mut function, &|_| true);
        mojito_mir::mir::lower_copies_and_moves(&key.template, &mut function, &|instance| {
            self.template_declaration(instance)
        });
        self.answer_layout_queries(&key.template, &mut function)?;
        ensure_concrete_function(&key.template, &name, &function)?;
        self.output_positions
            .insert(name.clone(), self.output_functions.len());
        self.output_functions.push((name, function));
        Ok(())
    }

    /// A copy of `template` with its `comptime for` loops unrolled and
    /// `bindings` substituted through it, beside the slots its parameter
    /// reads reified.
    fn substituted(
        &mut self,
        name: &str,
        template: &MirFunction,
        scope: &[ParamDecl],
        bindings: &Bindings,
    ) -> Result<MirFunction, MonoError> {
        let mut function = template.clone();
        default_construct_parameters(
            &mut function.blocks,
            &mut function.n_regs,
            &mut function.reg_types,
            bindings,
        )?;
        self.unroll_comptime_loops(name, &mut function, scope, bindings)?;
        self.erase_valueless_removed_arguments(&mut function.blocks, bindings);
        substitute_function(&mut function, bindings, scope).map_err(|mut e| {
            e.function.get_or_insert_with(|| name.to_string());
            e
        })?;
        Ok(function)
    }

    /// Erase (`_REMOVED_ARG`) each argument in `blocks` and their regions
    /// that binds a parameter its callee removed and has no value under
    /// `bindings` (`10 // n` at `n == 0`), in a call's `instantiated_args`
    /// or its static receiver: upstream's `RemoveUnusedParams` rewrote the
    /// call without it, so nothing evaluates it. One with a value stays,
    /// which binds the callee's removed parameter for inference.
    fn erase_valueless_removed_arguments(&self, blocks: &mut [MirBlock], bindings: &Bindings) {
        for instruction in blocks.iter_mut().flat_map(|block| &mut block.instrs) {
            for region in mojito_mir::mir::instruction_regions_mut(instruction) {
                self.erase_valueless_removed_arguments(region, bindings);
            }
            let (target, decls, instantiated_args, receiver) = match instruction {
                MirInstr::Call {
                    func,
                    instantiated_args,
                    receiver,
                    ..
                } => {
                    let Some(declaration) = self.declarations.get(func.0.as_str()) else {
                        continue;
                    };
                    (
                        func.0.as_str(),
                        declaration.param_decls.as_slice(),
                        instantiated_args,
                        receiver.as_mut(),
                    )
                }
                MirInstr::MethodCall {
                    resolved: Some(target),
                    param_decls,
                    instantiated_args,
                    ..
                } => (
                    target.as_str(),
                    param_decls.as_slice(),
                    instantiated_args,
                    None,
                ),
                _ => continue,
            };
            let removed = self.removed_params(target, true);
            if removed.is_empty() {
                continue;
            }
            let erase = |decls: &[ParamDecl], arguments: &mut [TyArg]| {
                for (decl, argument) in decls.iter().zip(arguments) {
                    if removed.contains(decl.id()) && substitute_arg(argument, bindings).is_err() {
                        *argument = TyArg::Val(CtValue::RemovedArg);
                    }
                }
            };
            if instantiated_args.len() == decls.len() {
                erase(decls, instantiated_args);
            }
            if let Some(receiver) = receiver
                && let TyKind::Struct(name, arguments) = receiver.kind()
                && let Some(owner) = self.structs.get(name.as_str())
                && substitute_ty(receiver, bindings).is_err()
            {
                let mut arguments = arguments.clone();
                erase(&owner.param_decls, &mut arguments);
                *receiver = TyKind::Struct(name.clone(), arguments).into();
            }
        }
    }

    /// Demand every application the last substitution met with no value
    /// ([`Applications::answer`]) and record its value. Whether one was
    /// answered, so that substituting again gets further.
    pub(super) fn answer_pending(
        &mut self,
        template: &str,
        bindings: &Bindings,
    ) -> Result<bool, MonoError> {
        let pending = std::mem::take(&mut *self.applications.pending.borrow_mut());
        let mut answered = false;
        for (function, args) in pending {
            let value = self.demand_application(template, &function, &args, bindings)?;
            self.applications
                .answered
                .borrow_mut()
                .insert((function, args), value);
            answered = true;
        }
        Ok(answered)
    }

    /// The condition with every compile-time application in it evaluated:
    /// an application under the instance's bindings is demanded and run on
    /// the VM, a layout query is answered under the target, whether the
    /// application is the operand itself (`f(n) == True`, `size_of[T]()`)
    /// or sits under the operand's arithmetic (`L[0] + 1`).
    fn resolve_applications(
        &mut self,
        template: &str,
        cond: &GenericConstraint,
        bindings: &Bindings,
    ) -> Result<GenericConstraint, MonoError> {
        let mut resolved: Vec<(ParamExpr, CtValue)> = Vec::new();
        for expr in constraint_expressions(cond) {
            if let Some(value) = self.resolve_application(template, &expr, bindings)? {
                resolved.push((expr, value));
            }
        }
        if resolved.is_empty() {
            return Ok(cond.clone());
        }
        Ok(cond.map(&Clone::clone, &|operand| match operand {
            ConstraintOperand::Expr(expr) | ConstraintOperand::Value(CtValue::Expr(expr)) => {
                resolved
                    .iter()
                    .find(|(application, _)| application == expr)
                    .map_or_else(
                        || operand.clone(),
                        |(_, value)| ConstraintOperand::Value(value.clone()),
                    )
            }
            other => other.clone(),
        }))
    }

    /// The value of a condition operand or a loop sequence that is a
    /// compile-time application not yet evaluated, or a conformance
    /// proposition the instance decides; `None` for any other expression.
    pub(super) fn resolve_application(
        &mut self,
        template: &str,
        expr: &ParamExpr,
        bindings: &Bindings,
    ) -> Result<Option<CtValue>, MonoError> {
        // A conformance proposition over a dependent element the instance
        // closes (`conforms_to(types[i], Writable)`).
        if let ParamKind::Conforms {
            subject,
            trait_name,
        } = expr.kind()
        {
            let Ok(CtValue::Type(ty)) = eval_ct(subject, bindings) else {
                return Ok(None);
            };
            return Ok(self
                .type_conforms(&ty, trait_name, &mut HashSet::new())
                .map(CtValue::Bool));
        }
        let ParamKind::Apply { function, args } = expr.kind() else {
            let answered = self.applied(template, expr, bindings)?;
            return Ok((answered != *expr)
                .then(|| eval_ct(&answered, bindings).ok())
                .flatten());
        };
        // A raw list's length is an attribute fold (upstream's
        // `#kgen.param_list.size`), never a function: the list's own
        // applications are demanded, then the instance answers it.
        if function == LIST_LENGTH_FUNCTION {
            let answered = self.applied(template, expr, bindings)?;
            return eval_ct(&answered, bindings).map(Some);
        }
        if function == SIZE_OF_FUNCTION {
            let [subject] = args.as_slice() else {
                return Err(self.error(
                    Some(template),
                    format!("layout query `{expr}` applies to {} arguments", args.len()),
                ));
            };
            let ty = match subject.kind() {
                ParamKind::TypeShape(ty) => substitute_ty(ty, bindings),
                ParamKind::Constant(CtValue::Type(ty)) => Ok(ty.clone()),
                _ => {
                    return Err(self.error(
                        Some(template),
                        format!("layout query `{expr}` applies to a value"),
                    ));
                }
            }
            .map_err(|mut error| {
                error.function.get_or_insert_with(|| template.to_string());
                error
            })?;
            return self.demand_layout(template, &ty).map(Some);
        }
        self.demand_application(template, function, args, bindings)
            .map(Some)
    }

    /// `expr` with every compile-time application of a lowered function in
    /// it evaluated under `bindings`, each demanded and run on the VM
    /// ([`Self::demand_application`]): a scalar read off a local display
    /// binding (`L[0]`, `len(L)`) is such an application, alone or under
    /// arithmetic.
    pub(super) fn applied(
        &mut self,
        template: &str,
        expr: &ParamExpr,
        bindings: &Bindings,
    ) -> Result<ParamExpr, MonoError> {
        let mut pending = false;
        expr.visit(&mut |node| {
            pending |= matches!(node.kind(), ParamKind::Apply {
                function,
                ..
            } if self.declarations.contains_key(function.as_str()));
        });
        if !pending {
            return Ok(expr.clone());
        }
        // The instance's values close what sits between two applications
        // (`h(L[i])` at the index), so that it folds once the inner one is
        // answered.
        let context = ParamContext::detached();
        let closed = context.replace(expr, &ct_bindings(bindings));
        let expr = closed.as_ref().unwrap_or(expr);
        let mut failure = None;
        let answered = context.answer_applications(expr, &mut |function, args| {
            if failure.is_some() || !self.declarations.contains_key(function) {
                return Ok(None);
            }
            Ok(self
                .demand_application(template, function, args, bindings)
                .map_err(|error| failure = Some(error))
                .ok())
        });
        match failure {
            Some(error) => Err(error),
            // A fold the answer reaches that has no value (an index past
            // its list) fails the instantiation, as `eval_ct` classifies it.
            None => answered.map_err(|error| MonoError {
                kind: if matches!(
                    error,
                    mojito_types::param_expr::ParamError::Reflect(_)
                        | mojito_types::param_expr::ParamError::Constraint(_)
                        | mojito_types::param_expr::ParamError::Arithmetic(_)
                ) {
                    MonoErrorKind::Instantiation
                } else {
                    MonoErrorKind::Unsupported
                },
                function: Some(template.to_string()),
                construct: error.to_string(),
            }),
        }
    }

    /// Evaluate the application of `function` to `args` under `bindings` by
    /// demanding its instance: the instance's reference closure is
    /// materialized now and verified as the fragment the VM runs, and the
    /// frozen result is cached by the instance's name. The
    /// arguments past the function's compile-time parameters are the values
    /// of its runtime ones (`h(n)`, a `def` the check applied by name), which
    /// the call passes and the cache keys beside the name. A binder the
    /// function removed is bound but not keyed, as a call's is. A demand on an
    /// instance being materialized is a cycle in the parameter domain.
    fn demand_application(
        &mut self,
        template: &str,
        function: &str,
        args: &[ParamExpr],
        bindings: &Bindings,
    ) -> Result<CtValue, MonoError> {
        let declaration = self.declarations.get(function).ok_or_else(|| {
            self.error(
                Some(template),
                format!("compile-time application of `{function}`, which has no MIR declaration"),
            )
        })?;
        let result_ty = declaration.ret_ty.clone();
        // A static method of a generic struct is applied to the instance it
        // is called on first (`G[n].f()`).
        let owner = mojito_symbol::symbol::split_method_symbol(function)
            .and_then(|(owner, _)| self.structs.get(owner))
            .filter(|owner| !owner.param_decls.is_empty());
        let (instance, args) = match (owner, args.split_first()) {
            (Some(owner), Some((instance, args))) => (Some((owner, instance)), args),
            _ => (None, args),
        };
        let parameters = declaration.param_decls.len() + declaration.param_types.len();
        if parameters != args.len() {
            return Err(self.error(
                Some(template),
                format!(
                    "compile-time application of `{function}` binds {} of its {parameters} \
                     parameters",
                    args.len(),
                ),
            ));
        }
        let (args, called) = args.split_at(declaration.param_decls.len());
        let unresolved = |what: &str, arg: &ParamExpr| {
            self.error(
                Some(template),
                format!("compile-time application `{function}` reads {what} `{arg}`"),
            )
        };
        // A function the check applied by its own name takes the values of
        // its arguments as a call does.
        let called = called
            .iter()
            .map(|arg| {
                match arg.kind() {
                    ParamKind::DeclRef(reference) => bindings.values.get(reference).cloned(),
                    _ => eval_ct(arg, bindings).ok(),
                }
                .ok_or_else(|| unresolved("an unresolved value", arg))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut instance_bindings = self.base_bindings();
        if let Some((owner, instance)) = instance {
            let ty = match instance.kind() {
                ParamKind::TypeShape(ty) => substitute_ty(ty, bindings).ok(),
                ParamKind::Constant(CtValue::Type(ty)) => substitute_ty(ty, bindings).ok(),
                _ => None,
            }
            .ok_or_else(|| unresolved("an unresolved instance", instance))?;
            let TyKind::Struct(name, arguments) = ty.kind() else {
                return Err(unresolved("an instance that is no struct", instance));
            };
            bind_ty_args(&owner.param_decls, arguments, &mut instance_bindings)
                .map_err(|error| self.error(Some(template), error))?;
            instance_bindings.self_instance = Some((nominal_template(name).to_string(), ty));
        }
        let removed = self.removed_params(function, false);
        let mut arguments = Vec::new();
        for (decl, arg) in declaration.param_decls.iter().zip(args) {
            let keyed = !removed.contains(decl.id());
            let binder = decl.binder();
            let unresolved = |what: &str| unresolved(what, arg);
            match decl {
                ParamDecl::Type { variadic, .. } => {
                    let ty = match arg.kind() {
                        ParamKind::DeclRef(reference) => bindings.types.get(reference).cloned(),
                        // A closed type names its generic structs by their
                        // instances, as a shape does once substituted.
                        ParamKind::TypeShape(ty) | ParamKind::Constant(CtValue::Type(ty)) => {
                            substitute_ty(ty, bindings).ok()
                        }
                        // A pack bound to its element list by a caller the
                        // thunk's body was inlined into is applied to that
                        // list, as its runtime pack.
                        ParamKind::Constant(CtValue::Tuple(elements)) if *variadic => elements
                            .iter()
                            .map(|element| match element {
                                CtValue::Type(ty) => substitute_ty(ty, bindings).ok(),
                                _ => None,
                            })
                            .collect::<Option<Vec<_>>>()
                            .map(|elements| TyKind::RuntimePack(elements).into()),
                        _ => None,
                    }
                    .ok_or_else(|| unresolved("an unresolved type"))?;
                    if keyed {
                        arguments.push(InstanceArg::Ty(ty.clone()));
                    }
                    // A type pack binds its element tuple too, which the
                    // thunk's reads over the pack (`len(Ts)`) fold under.
                    match ty.kind() {
                        TyKind::RuntimePack(elements) => {
                            unify::bind_pack(&binder, elements.clone(), &mut instance_bindings);
                        }
                        _ => {
                            instance_bindings.types.insert(binder, ty);
                        }
                    }
                }
                ParamDecl::Value { .. } => {
                    let value = match arg.kind() {
                        ParamKind::DeclRef(reference) => bindings.values.get(reference).cloned(),
                        _ => eval_ct(arg, bindings).ok(),
                    }
                    .ok_or_else(|| unresolved("an unresolved value"))?;
                    if keyed {
                        arguments.push(InstanceArg::Value(value.clone()));
                    }
                    instance_bindings.values.insert(binder, value);
                }
            }
        }
        let name = self.enqueue(function, instance_bindings, arguments)?;
        let evaluation = if called.is_empty() {
            name.clone()
        } else {
            let called: Vec<String> = called.iter().map(ToString::to_string).collect();
            format!("{name}({})", called.join(", "))
        };
        if let Some(value) = self.evaluations.get(&evaluation) {
            return Ok(value.clone());
        }
        // The fragment an instance's applications run is built and verified
        // once: its closure is complete when it is first demanded.
        let fragment = if let Some(fragment) = self.fragments.get(&name) {
            Rc::clone(fragment)
        } else {
            let fragment = Rc::new(ConcreteProgram::new(
                &self.verified_fragment(template, &name)?,
            ));
            self.fragments.insert(name.clone(), Rc::clone(&fragment));
            fragment
        };
        self.fuel = self.fuel.checked_sub(1).ok_or_else(|| {
            self.error(
                Some(template),
                "compile-time execution exceeded the VM CTFE fuel quota".to_string(),
            )
        })?;
        let (value, remaining) = self
            .vm
            .call_concrete(&fragment, &name, &called, self.fuel)
            .map_err(|error| {
                self.error(
                    Some(template),
                    format!("VM CTFE failed for '{name}': {error}"),
                )
            })?;
        self.fuel = remaining;
        let value = self.vm.freeze(value).map_err(|error| {
            self.error(
                Some(template),
                format!("VM CTFE failed for '{name}': {error}"),
            )
        })?;
        // The result is the function's declared one: a literal it returns
        // (`return y` of `comptime y: Int = 5`) is materialized at that type.
        let value = value.clone().materialize_as(&result_ty).unwrap_or(value);
        self.evaluations.insert(evaluation, value.clone());
        Ok(value)
    }

    /// The verified concrete fragment the instance `name` runs in: its
    /// reference closure, materialized now, and nothing else.
    fn verified_fragment(&mut self, template: &str, name: &str) -> Result<ConcreteMir, MonoError> {
        let index = self.instance_names[name];
        let closure = match self.states[index] {
            InstanceState::Active => {
                let path = self
                    .demand_stack
                    .iter()
                    .map(String::as_str)
                    .chain(std::iter::once(name))
                    .collect::<Vec<_>>()
                    .join(" -> ");
                return Err(self.error(
                    Some(template),
                    format!(
                        "function instantiation in parameter domain that recursively requires \
                         itself: {path}"
                    ),
                ));
            }
            InstanceState::Failed => {
                return Err(self.error(
                    Some(template),
                    format!("compile-time application `{name}` failed to elaborate"),
                ));
            }
            // The fragment the VM runs holds every instance the thunk can
            // reach, and only those: the demand blocks on its own closure.
            InstanceState::Pending | InstanceState::Completed => {
                let frame = self.take_frame();
                self.demand_stack.push(name.to_string());
                let closure = self.materialize_closure(index);
                self.demand_stack.pop();
                self.restore_frame(frame);
                closure.map_err(|mut error| {
                    error.function.get_or_insert_with(|| template.to_string());
                    error
                })?
            }
        };
        let names: HashSet<&str> = closure
            .iter()
            .map(|&member| self.instances[member].1.as_str())
            .collect();
        let fragment = ConcreteMir::verified(MirProgram {
            functions: closure
                .iter()
                .filter_map(|&member| self.output_positions.get(&self.instances[member].1))
                .map(|&position| self.output_functions[position].clone())
                .collect(),
            declarations: MirDeclarations {
                // A member table names only the instances the fragment
                // holds.
                structs: self
                    .output_structs
                    .iter()
                    .map(|declaration| {
                        let mut declaration = declaration.clone();
                        declaration
                            .members
                            .retain(|entry| names.contains(entry.symbol.as_str()));
                        declaration
                    })
                    .collect(),
                functions: self
                    .output_function_decls
                    .iter()
                    .filter(|declaration| names.contains(declaration.lowered_name.as_str()))
                    .cloned()
                    .collect(),
                traits: self.source.declarations.traits.clone(),
                imported: None,
            },
            invariant_errors: Vec::new(),
        })
        .map_err(|findings| {
            self.error(
                Some(template),
                format!(
                    "compile-time application `{name}` over MIR that does not verify: {}",
                    findings.join("; ")
                ),
            )
        })?;
        if let Some(effectful) = effectful_callee(&fragment, name) {
            return Err(self.error(
                Some(template),
                format!(
                    "'{name}' is not safe for VM-backed compile-time execution: it reaches \
                     `{effectful}`"
                ),
            ));
        }
        Ok(fragment)
    }

    /// The size of the concrete `ty` under the elaboration's target: the
    /// layout application `size_of[ty]()` a type or a condition carries.
    fn demand_layout(&self, template: &str, ty: &Ty) -> Result<CtValue, MonoError> {
        let Some(target) = self.target else {
            return Err(self.error(
                Some(template),
                "layout query: no native target for this host".to_string(),
            ));
        };
        self.struct_catalog.fill_layout(&self.layout_structs, ty);
        let structs = self.layout_structs.borrow();
        let layout = mojito_native_core::layout::LayoutCx {
            target,
            structs: &structs,
        };
        layout
            .layout_of(ty)
            .map(|size| CtValue::Int(size.size as i64))
            .map_err(|error| self.error(Some(template), format!("size_of of `{ty}`: {error}")))
    }

    /// What answers a layout application in the instance being
    /// materialized: the target and every struct declaration known so far —
    /// the instances discovered before it and the source's non-generic
    /// structs. `None` on a host with no native target.
    fn layout_oracle(&self) -> Option<Rc<LayoutOracle>> {
        let target = *self.target?;
        Some(Rc::new(LayoutOracle {
            target,
            structs: Rc::clone(&self.layout_structs),
            catalog: Rc::clone(&self.struct_catalog),
        }))
    }

    /// Add `declaration` to the output structs and, unless a non-generic
    /// source struct already answers its name, to the layout field index.
    fn declare_output_struct(&mut self, mut declaration: MirStructDeclaration) {
        for (member, instance) in self
            .pending_struct_members
            .remove(&declaration.name)
            .into_iter()
            .flatten()
        {
            declaration.record_member(&member, &instance);
        }
        let shadowed = self
            .structs
            .get(declaration.name.as_str())
            .is_some_and(|source| source.param_decls.is_empty());
        if !shadowed {
            self.layout_structs.borrow_mut().insert(
                declaration.name.clone(),
                declaration
                    .fields
                    .iter()
                    .map(|(_, ty)| ty.clone())
                    .collect(),
            );
        }
        self.output_struct_positions
            .insert(declaration.name.clone(), self.output_structs.len());
        self.output_structs.push(declaration);
    }

    fn take_frame(&mut self) -> FunctionFrame {
        FunctionFrame {
            constant_values: std::mem::take(&mut self.constant_values),
            callable_targets: std::mem::take(&mut self.callable_targets),
            closure_captures: std::mem::take(&mut self.closure_captures),
            enclosing: std::mem::take(&mut self.enclosing),
            folded_slots: std::mem::take(&mut self.folded_slots),
        }
    }

    fn restore_frame(&mut self, frame: FunctionFrame) {
        self.constant_values = frame.constant_values;
        self.callable_targets = frame.callable_targets;
        self.closure_captures = frame.closure_captures;
        self.enclosing = frame.enclosing;
        self.folded_slots = frame.folded_slots;
    }

    /// Whether the `comptime if` condition `cond` holds under the bindings.
    /// `and` and `or` decide left to right and stop once the left side
    /// decides them, as upstream's `ParamOperatorAttr` folds them, so a
    /// right side's compile-time applications run only when it is reached:
    /// a payload read guarded by its tag test never runs over the other
    /// variant. Each other leaf runs its applications first.
    pub(super) fn decide_comptime_condition(
        &mut self,
        template: &str,
        cond: &GenericConstraint,
        bindings: &Bindings,
    ) -> Result<bool, MonoError> {
        match cond {
            GenericConstraint::WithMessage(inner, _) => {
                return self.decide_comptime_condition(template, inner, bindings);
            }
            GenericConstraint::Not(inner) => {
                return Ok(!self.decide_comptime_condition(template, inner, bindings)?);
            }
            GenericConstraint::And(left, right) | GenericConstraint::Or(left, right) => {
                let conjunction = matches!(cond, GenericConstraint::And(..));
                if self.decide_comptime_condition(template, left, bindings)? != conjunction {
                    return Ok(!conjunction);
                }
                return self.decide_comptime_condition(template, right, bindings);
            }
            _ => {}
        }
        let cond = self.resolve_applications(template, cond, bindings)?;
        self.constraint_holds(&cond, bindings, &mut HashSet::new())
            .ok_or_else(|| {
                self.error(
                    Some(template),
                    format!("comptime if condition `{cond:?}` the instance does not decide"),
                )
            })
    }

    /// Fold every parameter constant of the instance, and every bracket
    /// argument's expression, to the value its
    /// expression denotes under the bindings: a pack's length by
    /// replacement, a pack's membership, conformance, or predicate by the
    /// oracle that decides a `comptime if`, and a compile-time application
    /// by demanding it. One the bindings leave open stays for
    /// the verifier's concrete mode to name; one the bindings close with no
    /// answer (a reflection query of a field the struct lacks) fails the
    /// instantiation. A display constant is the frozen `Array` its lifted
    /// application built; a reflected field-name list read at an `Array` is
    /// constructed at the read's register type instead
    /// ([`array_display_construction`]), minting
    /// its parts' registers in `n_regs` and `reg_types`. A failure in blocks
    /// the instance has not yet `reached` (an unrolled iteration, before
    /// its branches are decided) is held in `unreached_failures` and
    /// reported only if the instance reaches the read.
    pub(super) fn answer_param_constants(
        &mut self,
        template: &str,
        blocks: &mut [MirBlock],
        bindings: &Bindings,
        n_regs: &mut u32,
        reg_types: &mut HashMap<u32, Ty>,
        reached: bool,
    ) -> Result<(), MonoError> {
        for block in blocks {
            let mut instrs = Vec::with_capacity(block.instrs.len());
            for mut instruction in std::mem::take(&mut block.instrs) {
                let regions = mojito_mir::mir::instruction_regions_mut(&mut instruction);
                if !regions.is_empty() {
                    for region in regions {
                        self.answer_param_constants(
                            template, region, bindings, n_regs, reg_types, reached,
                        )?;
                    }
                    instrs.push(instruction);
                    continue;
                }
                // A bracket argument read off a local `comptime` binding
                // (`g[e]()` after `comptime e = L[1]`) is the application
                // the callee's instance is keyed by.
                for argument in mojito_mir::mir::instruction_param_args_mut(&mut instruction) {
                    if let Some(expr) = &argument.expr {
                        argument.expr = Some(self.applied(template, expr, bindings)?);
                    }
                }
                // A parameter list's static address names the list the
                // instance binds.
                if let MirInstr::ParamListAddress { dest, values } = &mut instruction
                    && let Const::Param(list) = values
                {
                    let list = self.applied(template, list, bindings)?;
                    match symbolic::eval_reached_ct(&list, bindings) {
                        Ok(closed @ CtValue::Tuple(_)) => *values = Const::Value(closed),
                        Ok(other) => {
                            return Err(self.error(
                                Some(template),
                                format!(
                                    "r{}: the parameter list `{list}` closes to `{other}`, not \
                                     a list",
                                    dest.0
                                ),
                            ));
                        }
                        Err(error) if error.kind == MonoErrorKind::Instantiation => {
                            return Err(error);
                        }
                        Err(_) => {}
                    }
                }
                let MirInstr::Const {
                    dest,
                    k: Const::Param(value),
                } = &instruction
                else {
                    instrs.push(instruction);
                    continue;
                };
                let dest = *dest;
                let ty = reg_types.get(&dest.0).cloned();
                let constant = match self
                    .applied(template, value, bindings)
                    .and_then(|value| self.param_constant(&value, bindings, ty.as_ref()))
                {
                    // An unrolled iteration's arm may be one the instance
                    // never takes: its failure waits for the branches.
                    // It keeps its value closed over the iteration's
                    // binders, which the reached pass meets again.
                    Err(error) if !reached && error.kind == MonoErrorKind::Instantiation => {
                        self.unreached_failures.insert(dest.0, error);
                        ParamContext::detached()
                            .replace(value, &symbolic::ct_bindings(bindings))
                            .ok()
                            .map(Const::Param)
                    }
                    constant => constant?,
                };
                match constant {
                    // A reflected field-name list read at an `Array` is
                    // built by its list-literal initializer; any other value
                    // is the constant typed by `dest`'s register.
                    Some(k)
                        if let Some(ty) = &ty
                            && let Some(owned) = constructed_parameter_constant(&k, ty) =>
                    {
                        instrs.extend(array_display_construction(
                            dest,
                            &owned,
                            ty,
                            n_regs,
                            reg_types,
                            self.construction_decls(),
                        )?);
                    }
                    Some(k) => instrs.push(MirInstr::Const { dest, k }),
                    None => instrs.push(instruction),
                }
            }
            block.instrs = instrs;
        }
        Ok(())
    }

    fn param_constant(
        &self,
        value: &ParamExpr,
        bindings: &Bindings,
        ty: Option<&Ty>,
    ) -> Result<Option<Const>, MonoError> {
        let proposition = match value.kind() {
            ParamKind::PackQuery {
                pack,
                query: PackQuery::Conforms(trait_name),
            } => Some(GenericConstraint::ConformsPack {
                param: pack.clone(),
                trait_name: trait_name.clone(),
            }),
            ParamKind::PackQuery {
                pack,
                query: PackQuery::Predicate { predicate, all },
            } => Some(GenericConstraint::PackPredicate {
                param: pack.clone(),
                predicate: predicate.clone(),
                all: *all,
            }),
            ParamKind::PackQuery {
                pack,
                query: PackQuery::Contains(element),
            } => {
                let element = match element.kind() {
                    ParamKind::TypeShape(ty) => ty.clone(),
                    ParamKind::Constant(CtValue::Type(ty)) => ty.clone(),
                    _ => return Ok(None),
                };
                Some(GenericConstraint::PackContains {
                    param: pack.clone(),
                    element: ConstraintOperand::Type(element),
                })
            }
            _ => None,
        };
        match proposition {
            Some(proposition) => Ok(self
                .constraint_holds(&proposition, bindings, &mut HashSet::new())
                .map(Const::Bool)),
            None => match symbolic::eval_reached_ct(value, bindings) {
                // A value pack spread whole into a bracket (`f[*vs]()`) is
                // its bound list, which the callee's instance keys.
                Ok(list @ CtValue::Tuple(_))
                    if matches!(value.meta(), mojito_types::param_expr::MetaTy::ParamList(_)) =>
                {
                    Ok(Some(Const::Value(list)))
                }
                Ok(value) => Ok(Const::of_parameter_value(&value, ty)),
                Err(error) if error.kind == MonoErrorKind::Instantiation => Err(error),
                Err(_) => Ok(None),
            },
        }
    }

    /// Answer every layout query of the instance from its now-concrete type
    /// under the elaboration's target: the struct instances the type names
    /// were just discovered, so their fields lay out.
    fn answer_layout_queries(
        &self,
        template: &str,
        function: &mut MirFunction,
    ) -> Result<(), MonoError> {
        fn queries(blocks: &mut [MirBlock]) -> Vec<&mut MirInstr> {
            let mut found = Vec::new();
            for block in blocks {
                for instruction in &mut block.instrs {
                    if matches!(instruction, MirInstr::SizeOf { .. }) {
                        found.push(instruction);
                    } else {
                        for region in mojito_mir::mir::instruction_regions_mut(instruction) {
                            found.extend(queries(region));
                        }
                    }
                }
            }
            found
        }
        let queries = queries(&mut function.blocks);
        if queries.is_empty() {
            return Ok(());
        }
        let Some(target) = self.target else {
            return Err(self.error(
                Some(template),
                "layout query: no native target for this host".to_string(),
            ));
        };
        for query in &queries {
            if let MirInstr::SizeOf { ty, .. } = query {
                self.struct_catalog.fill_layout(&self.layout_structs, ty);
            }
        }
        let structs = self.layout_structs.borrow();
        let layout = mojito_native_core::layout::LayoutCx {
            target,
            structs: &structs,
        };
        for query in queries {
            let MirInstr::SizeOf { dest, ty } = query else {
                continue;
            };
            let size = layout.layout_of(ty).map_err(|error| {
                self.error(Some(template), format!("size_of of `{ty}`: {error}"))
            })?;
            *query = MirInstr::Const {
                dest: *dest,
                k: Const::Int(size.size as i64),
            };
        }
        Ok(())
    }

    /// A parameter defaulting to a recorded constructor over a generic struct
    /// instance (`dir: Optional[String] = None`, `x: Optional[Int] = 5`):
    /// the omitted-argument path runs the constructor's instance for the
    /// concrete parameter type, so enqueue it and respell the default's target
    /// to that instance. The instance is keyed on the parameter's type, so the
    /// wrapped literal plays no part in selecting it. A lowered default
    /// function (`s: String = String("a")`) is the call's to run: every call
    /// leaving the slot out names its instance, so the declaration enqueues
    /// none.
    fn instantiate_constructed_defaults(
        &mut self,
        owner: &str,
        declaration: &mut MirFunctionDeclaration,
    ) -> Result<(), MonoError> {
        for (index, default) in declaration.defaults.iter_mut().enumerate() {
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
            let Some(param_ty) = declaration.param_types.get(index) else {
                continue;
            };
            let TyKind::Struct(struct_name, arguments) = param_ty.kind() else {
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
                    .struct_catalog
                    .is_generic(nominal_template(struct_name))
            {
                let Some(body) = self.functions.get(target.as_str()) else {
                    continue;
                };
                if function_types(body).any(is_symbolic) {
                    continue;
                }
                *target = self.enqueue(target, self.base_bindings(), Vec::new())?;
                continue;
            }
            let (bindings, arguments, _) =
                self.infer_receiver_call(owner, target, param_ty, ReceiverCall::default())?;
            // An instance that erased a struct parameter its constructor
            // reads names no construction for the default: each call leaving
            // the slot out builds the struct its own application spells
            // (`mir::fill_omitted_arguments`), as upstream's callee names no
            // default.
            let removed = self.removed_params(target, false);
            if bindings
                .removed
                .iter()
                .any(|binder| !removed.contains(&binder.id))
            {
                *default = None;
                continue;
            }
            let instance = self.enqueue(target, bindings, arguments)?;
            *target = instance;
        }
        Ok(())
    }

    /// The binders a body of `template` may name: those of each declaration
    /// it is nested in, outermost first, then its own.
    pub(super) fn binder_scope(&self, template: &str) -> Vec<ParamDecl> {
        mojito_mir::mir::binder_scope(template, |name| self.declarations.get(name))
            .into_iter()
            .cloned()
            .collect()
    }

    /// Point each closure and function value naming a body nested in
    /// `owner` at that body's full instance under the instance of `owner`
    /// being specialized: the nested body is a generator over `owner`'s
    /// binders, not a function of its own. A call of it by name is a
    /// direct call, which names its reduced generator as any direct call
    /// does.
    fn instantiate_nested_bodies(
        &mut self,
        owner: &str,
        blocks: &mut [MirBlock],
    ) -> Result<(), MonoError> {
        for instruction in blocks.iter_mut().flat_map(|block| &mut block.instrs) {
            match instruction {
                MirInstr::MakeClosure {
                    function: target, ..
                }
                | MirInstr::Const {
                    k: Const::Function(target),
                    ..
                } if self
                    .declarations
                    .get(target.as_str())
                    .is_some_and(|declaration| {
                        declaration.enclosing.as_deref() == Some(owner)
                            && declaration.param_decls.is_empty()
                    }) =>
                {
                    let (bindings, arguments) =
                        self.nested_bindings(target, &self.enclosing, false)?;
                    *target = self.enqueue(target, bindings, arguments)?;
                }
                instruction => {
                    for region in mojito_mir::mir::instruction_regions_mut(instruction) {
                        self.instantiate_nested_bodies(owner, region)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// The bindings and instance arguments of the nested body `target` under
    /// `bindings`, those of the instance (or the unrolled region copy) it is
    /// declared in: keyed by the solution of each binder of the scope it is
    /// declared in, its enclosing declarations' and its regions', but those
    /// the body removed, from its `reduced` generator or its full one.
    pub(super) fn nested_bindings(
        &self,
        target: &str,
        bindings: &Bindings,
        reduced: bool,
    ) -> Result<(Bindings, Vec<InstanceArg>), MonoError> {
        let bindings = Bindings {
            reduced,
            ..bindings.clone()
        };
        let mut scope = self.binder_scope(target);
        let own = self
            .declarations
            .get(target)
            .map_or(0, |declaration| declaration.param_decls.len());
        scope.truncate(scope.len().saturating_sub(own));
        let removed = self.removed_params(target, reduced);
        let arguments = scope
            .iter()
            .filter(|decl| !removed.contains(decl.id()))
            .map(|decl| {
                let binder = decl.binder();
                let argument = match decl {
                    ParamDecl::Type { .. } => {
                        bindings.types.get(&binder).cloned().map(InstanceArg::Ty)
                    }
                    ParamDecl::Value { .. } => bindings
                        .values
                        .get(&binder)
                        .cloned()
                        .map(InstanceArg::Value),
                };
                argument.ok_or_else(|| {
                    self.error(
                        Some(target),
                        format!("nested body reading unresolved parameter `{}`", binder.name),
                    )
                })
            })
            .collect::<Result<_, _>>()?;
        Ok((bindings, arguments))
    }

    /// A `**kwargs: element` parameter binds a `StringDict[element]` the
    /// caller builds without a MIR call: the backends run the instance's
    /// empty constructor and one `__setitem__` per collected keyword, so
    /// enqueue both for a callee that only ever reads the collector, and
    /// record both in the collector's member table.
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
        let collector = Ty::new(TyKind::Struct(
            mojito_symbol::symbol::instance_symbol(
                "StringDict",
                &[InstanceArg::Ty(element.clone())],
            ),
            vec![mojito_types::types::TyArg::Ty(element.clone())],
        ));
        let template = mojito_symbol::symbol::resolve_method_symbol(
            &self.functions,
            "StringDict",
            "__init__",
            None,
            0,
        );
        if let Some(instance) =
            self.nominal_method_instance(owner, &collector, "__init__", 0, &[], &[])?
            && let TyKind::Struct(name, _) = collector.kind()
        {
            self.record_struct_member(constructed_member(name, &template).as_ref(), &instance);
        }
        self.enqueue_nominal_method_instance(owner, &collector, "__setitem__", 2, &[])
    }

    /// The compiled constructor a default's conversion target names. The
    /// checker records the selected constructor's symbol; a bare struct name,
    /// which only hand-written MIR spells, resolves through the shared
    /// callable-symbol policy over the single wrapped literal.
    fn constructed_default_target(&self, target: &str) -> Option<String> {
        if self.functions.contains_key(target) {
            return Some(target.to_string());
        }
        let init = format!("{target}.__init__");
        let resolved = mojito_symbol::symbol::resolve_callable_symbol(&self.functions, &init, 1);
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
            // What converts an operator's result to the `Bool` its template
            // declared, spliced in after the operator the same way.
            let mut conversions: Vec<(usize, MirInstr)> = Vec::new();
            for (index, instruction) in block.instrs.iter_mut().enumerate() {
                let regions = mojito_mir::mir::instruction_regions_mut(instruction);
                if !regions.is_empty() {
                    for region in regions {
                        self.rewrite_blocks(owner, function, region)?;
                    }
                    continue;
                }
                // `abs`/`round` on a nominal receiver are checker-typed
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
                    raises,
                    ..
                } = instruction
                {
                    let dunder = match func.0.as_str() {
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
                            (function.reg_types.get(&args[0].0).map(peel_refs)).map(Ty::kind),
                            Some(TyKind::Struct(..))
                        )
                    {
                        *instruction = dunder_method_call(
                            *dest,
                            args[0],
                            method,
                            None,
                            raises.take(),
                            Vec::new(),
                        );
                    }
                }
                // A prefix `-`/`~` on a nominal operand is the VM's dunder
                // dispatch (`apply_prefix` → `__neg__`/`__invert__`); `not`
                // arrives already converted through `Bool(x)` by MIR.
                if let MirInstr::UnOp {
                    op,
                    dest,
                    a,
                    raises,
                } = instruction
                    && matches!(
                        op,
                        mojito_ast::ast::PrefixOp::Neg | mojito_ast::ast::PrefixOp::Invert
                    )
                    && matches!(
                        (function.reg_types.get(&a.0).map(peel_refs)).map(Ty::kind),
                        Some(TyKind::Struct(..))
                    )
                {
                    let raises = raises.take();
                    *instruction =
                        dunder_method_call(*dest, *a, op.dunder(), None, raises, Vec::new());
                }
                // A comparison through a bound names its requirement; the
                // builtin an instance binds answers it itself.
                if let MirInstr::BinOp { a, resolved, .. } = instruction
                    && resolved
                        .as_deref()
                        .is_some_and(mojito_symbol::symbol::is_trait_dispatch_symbol)
                    && !matches!(
                        (function.reg_types.get(&a.0).map(peel_refs)).map(Ty::kind),
                        Some(TyKind::Struct(..))
                    )
                {
                    *resolved = None;
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
                    raises,
                } = instruction
                    && let Some(method) = op.dunder()
                    && !matches!(
                        op,
                        mojito_ast::ast::InfixOp::In | mojito_ast::ast::InfixOp::NotIn
                    )
                    && matches!(
                        (function.reg_types.get(&a.0).map(peel_refs)).map(Ty::kind),
                        Some(TyKind::Struct(..))
                    )
                {
                    // A type that declares `__eq__` and no `__ne__` takes
                    // `Equatable`'s default `!=`, through a bound as well:
                    // dispatch `__eq__` and negate its result.
                    if *op == mojito_ast::ast::InfixOp::Ne
                        && resolved
                            .as_deref()
                            .is_none_or(mojito_symbol::symbol::is_trait_dispatch_symbol)
                        && self.negates_equality(function, *a)
                    {
                        let equal = Reg(function.n_regs);
                        function.n_regs += 1;
                        function.reg_types.insert(equal.0, Ty::bool());
                        conversions.push((
                            index,
                            MirInstr::UnOp {
                                op: mojito_ast::ast::PrefixOp::Not,
                                dest: *dest,
                                a: equal,
                                raises: None,
                            },
                        ));
                        *instruction =
                            dunder_method_call(equal, *a, "__eq__", None, raises.take(), vec![*b]);
                    } else {
                        let (resolved, raises) = (resolved.take(), raises.take());
                        *instruction =
                            dunder_method_call(*dest, *a, method, resolved, raises, vec![*b]);
                    }
                }
                // A comparison a template typed `Bool` through its bound
                // yields a mask over sized scalars: convert it, as the
                // checker does where the operands are spelled.
                if let MirInstr::BinOp { op, dest, a, .. } = instruction
                    && matches!(
                        op,
                        mojito_ast::ast::InfixOp::Eq
                            | mojito_ast::ast::InfixOp::Ne
                            | mojito_ast::ast::InfixOp::Lt
                            | mojito_ast::ast::InfixOp::Le
                            | mojito_ast::ast::InfixOp::Gt
                            | mojito_ast::ast::InfixOp::Ge
                    )
                    && function.reg_types.get(&dest.0) == Some(&Ty::bool())
                    && let Some(TyKind::Simd { width, .. }) =
                        (function.reg_types.get(&a.0).map(peel_refs).cloned())
                            .as_ref()
                            .map(Ty::kind)
                {
                    let mask = Reg(function.n_regs);
                    function.n_regs += 1;
                    function.reg_types.insert(
                        mask.0,
                        TyKind::Simd {
                            dtype: mojito_types::types::SimdDtype::Known(
                                mojito_ast::ast::Dtype::Bool,
                            ),
                            width: width.clone(),
                        }
                        .into(),
                    );
                    conversions.push((
                        index,
                        MirInstr::Call {
                            dest: std::mem::replace(dest, mask),
                            func: mojito_mir::mir::FuncRef::named("Bool"),
                            raises: None,
                            args: vec![mask],
                            kwargs: Vec::new(),
                            arg_places: vec![None],
                            kwarg_places: Vec::new(),
                            capture_accesses: Vec::new(),
                            param_arg_regs: Vec::new(),
                            receiver: None,
                            instantiated_args: Vec::new(),
                            spread: None,
                        },
                    ));
                }
                // `**` on Int/UInt and the expansions that display a scalar
                // call bundled bodies without naming them, so the
                // instruction is a use of each.
                for symbol in mojito_mir::mir::implicit_callees(instruction, &function.reg_types) {
                    if self.functions.contains_key(symbol) {
                        self.enqueue(symbol, self.base_bindings(), Vec::new())?;
                    }
                }
                match instruction {
                    MirInstr::Call {
                        dest,
                        func,
                        args,
                        kwargs,
                        arg_places,
                        kwarg_places,
                        param_arg_regs,
                        receiver: static_receiver,
                        instantiated_args,
                        ..
                    } => {
                        // A body nested in an unrolled region is named as
                        // its region copy's generator, whose bindings its
                        // instance inherits.
                        let region = self.region_generators.get(func.0.as_str()).cloned();
                        if let Some((template, _)) = &region {
                            func.0.clone_from(template);
                        }
                        if !self.functions.contains_key(func.0.as_str()) {
                            // `print` of a nominal struct displays through
                            // `write_to` over the builtin-string writer (the
                            // VM's `format_value` dispatch); enqueue the
                            // instances the lowered expansion calls, its
                            // `sep` and `end` keywords' too.
                            if matches!(func.0.as_str(), "print" | "String") {
                                let shown = kwargs
                                    .iter()
                                    .filter(|(keyword, _)| {
                                        matches!(keyword.as_str(), "sep" | "end")
                                    })
                                    .map(|(_, arg)| *arg);
                                for arg in args.clone().into_iter().chain(shown) {
                                    self.enqueue_display_instance(owner, function, arg)?;
                                }
                            }
                            if func.0 == "repr" {
                                for arg in args.clone() {
                                    self.enqueue_repr_instance(owner, function, arg)?;
                                }
                            }
                            // A call naming an instance (`Box$mono$TInt`,
                            // a type parameter's substitution) constructs
                            // through its template's members; an instance
                            // already minted (a body nested in an unrolled
                            // region of a struct instance's method,
                            // `Box$mono$V1.get$f`) is called as it is.
                            let template = nominal_template(&func.0).to_string();
                            if func.0 != "String"
                                && self.structs.contains_key(template.as_str())
                                && !self.instance_names.contains_key(func.0.as_str())
                            {
                                // `Type(copy=value)` runs `__copyinit__` (the
                                // VM's `construct_via_copy`), which struct
                                // discovery enqueues per instance — never an
                                // `__init__` contract.
                                let copy_form =
                                    args.is_empty() && kwargs.len() == 1 && kwargs[0].0 == "copy";
                                if copy_form {
                                    if let Some(TyKind::Struct(concrete, _)) =
                                        function.reg_types.get(&dest.0).map(Ty::kind)
                                        && concrete != &func.0
                                        && nominal_template(concrete) == template
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
                                let init_base = format!("{template}.__init__");
                                let init = mojito_symbol::symbol::resolve_callable_symbol(
                                    &self.functions,
                                    &init_base,
                                    args.len() + kwargs.len(),
                                );
                                let init = if self.functions.contains_key(init.as_str()) {
                                    init
                                } else {
                                    self.runtime_pack_constructor(&init_base).unwrap_or(init)
                                };
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
                                        instantiated_args,
                                    )?;
                                    if let Some((_, concrete)) = &bindings.self_instance {
                                        function.reg_types.insert(dest.0, concrete.clone());
                                    }
                                    // The call names the constructor instance
                                    // it reaches, as upstream's names the
                                    // constructor's symbol.
                                    let reduced = bindings.reduced;
                                    func.0 = self.enqueue(&target, bindings, arguments)?;
                                    if reduced {
                                        self.drop_removed_arguments(
                                            owner,
                                            &target,
                                            args,
                                            arg_places,
                                            kwargs,
                                            kwarg_places,
                                        )?;
                                    }
                                    param_arg_regs.clear();
                                    instantiated_args.clear();
                                    continue;
                                }
                                // A generic struct's output declaration is
                                // instance-named; respell the constructor
                                // call so lowering's struct lookup matches.
                                if let Some(TyKind::Struct(concrete, _)) =
                                    function.reg_types.get(&dest.0).map(Ty::kind)
                                    && concrete != &func.0
                                    && nominal_template(concrete) == template
                                {
                                    func.0.clone_from(concrete);
                                }
                                // The instance identity carries every
                                // compile-time solution, so the call keeps
                                // no slot for one (a body that still needs
                                // one fails its own contextual check).
                                param_arg_regs.clear();
                                instantiated_args.clear();
                            }
                            continue;
                        }
                        // A direct constructor call's destination is its
                        // `out self`, not the declared `None` return — bind
                        // it as the receiver.
                        let receiver = (mojito_symbol::symbol::is_initializer_symbol(&func.0)
                            && function.reg_types.contains_key(&dest.0))
                        .then_some(*dest);
                        let (target, mut bindings, mut arguments) = self.infer_call(
                            owner,
                            function,
                            &func.0,
                            receiver,
                            *dest,
                            args,
                            kwargs,
                            param_arg_regs,
                            instantiated_args,
                        )?;
                        // A nested `def` is a generator over its enclosing
                        // declaration's binders beside its own: its direct
                        // call binds them from this instance's, or from its
                        // region copy's.
                        if self
                            .declarations
                            .get(target.as_str())
                            .is_some_and(|declaration| declaration.enclosing.is_some())
                        {
                            let (inherited, inherited_arguments) = self.nested_bindings(
                                &target,
                                region
                                    .as_ref()
                                    .map_or(&self.enclosing, |(_, bindings)| bindings),
                                bindings.reduced,
                            )?;
                            inherit_bindings(&mut bindings, inherited);
                            arguments.splice(0..0, inherited_arguments);
                        }
                        // The instance identity carries the spelled receiver
                        // of a static call, so the resolved call keeps none.
                        if let Some(spelled) = static_receiver.take() {
                            self.bind_static_receiver(owner, &target, &spelled, &mut bindings)?;
                        }
                        // A callable parameter bound to a closure with
                        // captures keeps its environment: the instance takes
                        // it as trailing runtime parameters, passed here.
                        let capturing = self.bind_capturing_callables(
                            owner,
                            function,
                            &target,
                            param_arg_regs,
                            &mut bindings,
                        )?;
                        let reduced = bindings.reduced;
                        func.0 = self.enqueue(&target, bindings, arguments)?;
                        if reduced {
                            self.drop_removed_arguments(
                                owner,
                                &target,
                                args,
                                arg_places,
                                kwargs,
                                kwarg_places,
                            )?;
                        }
                        param_arg_regs.clear();
                        instantiated_args.clear();
                        pass_capturing_arguments(capturing, index, args, arg_places, &mut preludes);
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
                        raises,
                        result_adapter,
                        args,
                        kwargs,
                        arg_places,
                        kwarg_places,
                        capture_accesses,
                        param_arg_regs,
                        param_decls,
                        instantiated_args,
                        ..
                    } => {
                        let receiver = function.reg_types.get(&recv.0).ok_or_else(|| {
                            self.error(Some(owner), "method receiver lacks a MIR type")
                        })?;
                        // A scalar/literal Hashable leaf contributes to the
                        // hasher through the hasher's `_update_with_simd`
                        // instance at the leaf's vector type (a literal
                        // through the nominal String's `__hash__`); enqueue
                        // those instances for the lowered leaf dispatch.
                        if method == "__hash__"
                            && args.len() == 1
                            && kwargs.is_empty()
                            && !matches!(peel_refs(receiver).kind(), TyKind::Struct(..))
                        {
                            let receiver = peel_refs(receiver).clone();
                            self.enqueue_hash_leaf_instances(owner, function, args[0], &receiver)?;
                        }
                        if resolved
                            .as_deref()
                            .is_some_and(|target| target.starts_with("__trait_dispatch."))
                            && !matches!(peel_refs(receiver).kind(), TyKind::Struct(..))
                        {
                            // The builtin answers the call itself and takes
                            // none of the requirement's compile-time arguments.
                            *resolved = None;
                            param_decls.clear();
                            instantiated_args.clear();
                            param_arg_regs.clear();
                            if !matches!(peel_refs(receiver).kind(), TyKind::StringLiteral) {
                                continue;
                            }
                        }
                        // `write` on the builtin-string accumulator (the
                        // `Value::Str` writer inside a `write_to` expansion)
                        // formats nominal arguments through their own
                        // `write_to` conformance — enqueue those instances
                        // for the lowered recursion.
                        if matches!(
                            method.as_str(),
                            "write" | mojito_symbol::symbol::WRITE_FORMATTED
                        ) && matches!(peel_refs(receiver).kind(), TyKind::StringLiteral)
                        {
                            for arg in args.clone() {
                                self.enqueue_display_instance(owner, function, arg)?;
                            }
                            continue;
                        }
                        // A borrowed receiver dispatches on its referent, as
                        // the VM dereferences `Value::Ref` receivers.
                        let TyKind::Struct(receiver_name, _) = peel_refs(receiver).kind() else {
                            continue;
                        };
                        // Source methods are declared under the template name;
                        // an instance-named receiver (`List$mono$TInt`) still
                        // resolves against `List.*` and gets its instance
                        // identity from `infer_call`'s receiver binding.
                        let mut target = mojito_symbol::symbol::resolve_method_symbol(
                            &self.functions,
                            nominal_template(receiver_name),
                            method,
                            resolved.as_deref(),
                            args.len() + kwargs.len(),
                        );
                        if !self.functions.contains_key(target.as_str())
                            && let Some(dispatch) = resolved
                                .as_deref()
                                .filter(|symbol| symbol.starts_with("__trait_dispatch."))
                            && let Some(overload) = self.dispatched_overload_target(
                                nominal_template(receiver_name),
                                method,
                                dispatch,
                            )
                        {
                            target = overload;
                        }
                        if !self.functions.contains_key(target.as_str()) {
                            // The VM-synthesized `Writer.write` dispatch calls
                            // the receiver's `write_string`; enqueue its
                            // instance for the lowered expansion.
                            if method == mojito_symbol::symbol::WRITE_FORMATTED {
                                let write_string = mojito_symbol::symbol::resolve_callable_symbol(
                                    &self.functions,
                                    &format!("{}.write_string", nominal_template(receiver_name)),
                                    1,
                                );
                                if self.functions.contains_key(write_string.as_str()) {
                                    let receiver_ty = peel_refs(receiver).clone();
                                    let (bindings, arguments, _) = self.infer_receiver_call(
                                        owner,
                                        &write_string,
                                        &receiver_ty,
                                        ReceiverCall::default(),
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
                        // The adapter copies the element out of a reference
                        // the method returns: one that returns the element
                        // itself needs none.
                        if !self
                            .declarations
                            .get(target.as_str())
                            .is_some_and(|declaration| declaration.returns_reference)
                        {
                            *result_adapter = None;
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
                            instantiated_args,
                        )?;
                        let reduced = bindings.reduced;
                        let concrete = self.enqueue(&target, bindings, arguments)?;
                        if reduced {
                            self.drop_removed_arguments(
                                owner,
                                &target,
                                args,
                                arg_places,
                                kwargs,
                                kwarg_places,
                            )?;
                        }
                        // A generator without its receiver takes none: the
                        // call names it as upstream's rewritten call names
                        // `k3(q::Holder)_REMOVED_ARG()`, with no receiver.
                        if reduced && self.removes_receiver(&target) {
                            *instruction = MirInstr::Call {
                                dest: *dest,
                                func: mojito_mir::mir::FuncRef::named(&concrete),
                                raises: raises.take(),
                                args: std::mem::take(args),
                                kwargs: std::mem::take(kwargs),
                                arg_places: std::mem::take(arg_places),
                                kwarg_places: std::mem::take(kwarg_places),
                                capture_accesses: std::mem::take(capture_accesses),
                                param_arg_regs: Vec::new(),
                                receiver: None,
                                instantiated_args: Vec::new(),
                                spread: None,
                            };
                            continue;
                        }
                        *resolved = Some(concrete);
                        param_arg_regs.clear();
                        instantiated_args.clear();
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
                        param_decls: contract,
                        resolved,
                        instantiated_args,
                        ..
                    } => {
                        let dependent_callable =
                            function.reg_types.get(&callee.0).is_some_and(|ty| {
                                matches!(
                                    peel_refs(ty).kind(),
                                    TyKind::GenericFunc(_)
                                        | TyKind::Param {
                                            callable_bound: Some(_),
                                            ..
                                        }
                                )
                            });
                        if let Some((callable, captures_are_empty)) =
                            self.callable_targets.get(&callee.0).cloned()
                            && (captures_are_empty || dependent_callable)
                        {
                            let target = callable.clone();
                            // The lifted body takes its environment as leading
                            // reference parameters: the direct call passes the
                            // captured places themselves.
                            if !captures_are_empty {
                                let captured =
                                    self.capture_arguments(owner, function, &callable)?;
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
                            // A generic body declared in an unrolled region
                            // is named as its region copy's generator.
                            let (target, region) = match self.region_generators.get(&target) {
                                Some((template, bindings)) => {
                                    (template.clone(), Some(bindings.clone()))
                                }
                                None => (target, None),
                            };
                            // A closure over folded value parameters already
                            // names its lifted body's instance.
                            let concrete = if self.functions.contains_key(target.as_str()) {
                                // The call names the contract's parameters:
                                // its supplied and defaulted values bind the
                                // implementation's binders by position.
                                // A spelled application of a generic nested
                                // `def` carries its solved arguments.
                                let contract_values = if instantiated_args.is_empty() {
                                    self.contract_values(&target, contract, param_arg_regs)?
                                } else {
                                    instantiated_args.clone()
                                };
                                let (target, mut bindings, mut arguments) = self.infer_call(
                                    owner,
                                    function,
                                    &target,
                                    None,
                                    *dest,
                                    args,
                                    kwargs,
                                    param_arg_regs,
                                    &contract_values,
                                )?;
                                // A generic `def` nested in this body binds
                                // this instance's binders beside its own.
                                if self.declarations.get(target.as_str()).is_some_and(
                                    |declaration| declaration.enclosing.as_deref() == Some(owner),
                                ) {
                                    let (inherited, inherited_arguments) = self.nested_bindings(
                                        &target,
                                        region.as_ref().unwrap_or(&self.enclosing),
                                        bindings.reduced,
                                    )?;
                                    inherit_bindings(&mut bindings, inherited);
                                    arguments.splice(0..0, inherited_arguments);
                                }
                                // As at a direct call, a callable parameter
                                // bound to a capturing closure becomes the
                                // instance's trailing runtime parameters.
                                let capturing = self.bind_capturing_callables(
                                    owner,
                                    function,
                                    &target,
                                    param_arg_regs,
                                    &mut bindings,
                                )?;
                                let reduced = bindings.reduced;
                                let concrete = self.enqueue(&target, bindings, arguments)?;
                                if reduced {
                                    self.drop_removed_arguments(
                                        owner,
                                        &target,
                                        args,
                                        arg_places,
                                        kwargs,
                                        kwarg_places,
                                    )?;
                                }
                                pass_capturing_arguments(
                                    capturing,
                                    index,
                                    args,
                                    arg_places,
                                    &mut preludes,
                                );
                                concrete
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
                                receiver: None,
                                instantiated_args: Vec::new(),
                                spread: None,
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
                        let TyKind::Struct(receiver_name, _) = peel_refs(receiver).kind() else {
                            continue;
                        };
                        let target = mojito_symbol::symbol::resolve_method_symbol(
                            &self.functions,
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
                            &[],
                        )?;
                        // A `__call__` returning a reference hands the call
                        // the reference its destination register holds.
                        let reference_result = self
                            .declarations
                            .get(target.as_str())
                            .filter(|declaration| declaration.returns_reference)
                            .and_then(|_| match function.reg_types.get(&dest.0).map(Ty::kind) {
                                Some(TyKind::Ref(reference)) => Some(reference.clone()),
                                _ => None,
                            });
                        let concrete = self.enqueue(&target, bindings, arguments)?;
                        *instruction = MirInstr::MethodCall {
                            dest: *dest,
                            recv: *callee,
                            method: "__call__".to_string(),
                            resolved: Some(concrete),
                            raises: raises.clone(),
                            reference_result: reference_result.map(Box::new),
                            result_adapter: None,
                            args: std::mem::take(args),
                            kwargs: std::mem::take(kwargs),
                            recv_place: callee_place.take(),
                            recv_writes: true,
                            arg_places: std::mem::take(arg_places),
                            kwarg_places: std::mem::take(kwarg_places),
                            capture_accesses: Vec::new(),
                            param_arg_regs: Vec::new(),
                            param_decls: Vec::new(),
                            instantiated_args: Vec::new(),
                            spread: None,
                        };
                    }
                    // A retained callable names its lifted body on the
                    // instruction; enqueue it so the reachable graph carries
                    // the compiled target the thunk will call. A body nested
                    // in this one already names its instance
                    // ([`Self::instantiate_nested_bodies`]). A generic nested
                    // `def` is no body of its own: each call site binding its
                    // parameters enqueues an instance.
                    MirInstr::MakeClosure {
                        function: target, ..
                    }
                    | MirInstr::Const {
                        k: Const::Function(target),
                        ..
                    } => {
                        let Some(body) = self.functions.get(target.as_str()) else {
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
                    MirInstr::GetField {
                        dest,
                        base,
                        field,
                        constructors,
                    } => {
                        let Some(receiver) = function.reg_types.get(&base.0) else {
                            continue;
                        };
                        // A slice descriptor's bound read builds its
                        // `Optional` through that struct's constructors,
                        // which it names.
                        if matches!(peel_refs(receiver).kind(), TyKind::Struct(name, _)
                            if matches!(name.as_str(), "Slice" | "ContiguousSlice" | "StridedSlice"))
                            && matches!(field.as_str(), "start" | "end" | "step")
                        {
                            if let Some(bound) = function.reg_types.get(&dest.0).cloned() {
                                *constructors =
                                    self.enqueue_slice_bound_constructors(owner, &bound)?;
                            }
                            continue;
                        }
                        let Some(constant) = self.value_param_constant(
                            receiver,
                            field,
                            function.reg_types.get(&dest.0),
                        ) else {
                            continue;
                        };
                        *instruction = MirInstr::Const {
                            dest: *dest,
                            k: constant,
                        };
                    }
                    // A projection below a struct-typed parameter value
                    // (`Self.e.rows`) folds through the bound value's fields.
                    MirInstr::LoadPlace { dest, place }
                        if !place.is_whole_root()
                            && place.steps().all(|projection| {
                                matches!(projection, mojito_mir::mir::Proj::Field(_))
                            }) =>
                    {
                        let mut steps = place.steps();
                        let Some(mojito_mir::mir::Proj::Field(field)) = steps.next() else {
                            continue;
                        };
                        let Some(receiver) = function.var_tys.get(&place.root) else {
                            continue;
                        };
                        let read = function.reg_types.get(&dest.0);
                        let Some(constant) = steps
                            .try_fold(
                                self.value_param_constant(receiver, field, read),
                                |constant, projection| {
                                    let (
                                        mojito_mir::mir::Proj::Field(field),
                                        Some(Const::Value(CtValue::Struct { fields, .. })),
                                    ) = (projection, constant)
                                    else {
                                        return None;
                                    };
                                    Some(fields.iter().find(|(name, _)| name == field).and_then(
                                        |(_, value)| Const::of_parameter_value(value, read),
                                    ))
                                },
                            )
                            .flatten()
                        else {
                            continue;
                        };
                        let dest = *dest;
                        *instruction = MirInstr::Const { dest, k: constant };
                    }
                    // Checker-selected subscript invocations retarget to
                    // their concrete instances exactly like method calls;
                    // intrinsic storage subscripts carry no nominal call and
                    // pass through.
                    // An accessor's removed argument leaves the subscript
                    // as upstream's rewritten call passes none for it: an
                    // `index.get` or slice whose one operand goes becomes a
                    // multi-subscript with none.
                    MirInstr::Index {
                        dest,
                        base,
                        index,
                        base_place,
                        call: call @ Some(_),
                        ..
                    } => {
                        let (dest, base, index) = (*dest, *base, *index);
                        let Some(contract) = call.as_deref_mut() else {
                            continue;
                        };
                        let dropped = self.rewrite_subscript_call(
                            owner,
                            function,
                            base,
                            SubscriptOperands {
                                registers: &[index],
                                positional: 1,
                                keywords: &[],
                            },
                            Some(dest),
                            contract,
                        )?;
                        if dropped.is_some_and(|dropped| dropped.positional.contains(&0)) {
                            let (call, object_place) = (call.take(), base_place.take());
                            *instruction = MirInstr::MultiIndex {
                                dest,
                                object: base,
                                args: Vec::new(),
                                object_place,
                                arg_places: Vec::new(),
                                kwargs: Vec::new(),
                                kwarg_places: Vec::new(),
                                call,
                            };
                        }
                    }
                    MirInstr::Slice {
                        dest,
                        object,
                        object_place,
                        call: call @ Some(_),
                        ..
                    } => {
                        let (dest, object) = (*dest, *object);
                        let Some(contract) = call.as_deref_mut() else {
                            continue;
                        };
                        let dropped = self.rewrite_subscript_call(
                            owner,
                            function,
                            object,
                            SubscriptOperands {
                                registers: &[],
                                positional: 1,
                                keywords: &[],
                            },
                            Some(dest),
                            contract,
                        )?;
                        if dropped.is_some_and(|dropped| dropped.positional.contains(&0)) {
                            let (call, object_place) = (call.take(), object_place.take());
                            *instruction = MirInstr::MultiIndex {
                                dest,
                                object,
                                args: Vec::new(),
                                object_place,
                                arg_places: Vec::new(),
                                kwargs: Vec::new(),
                                kwarg_places: Vec::new(),
                                call,
                            };
                        }
                    }
                    MirInstr::MultiIndex {
                        dest,
                        object,
                        args,
                        arg_places,
                        kwargs,
                        kwarg_places,
                        call: Some(call),
                        ..
                    } => {
                        let (dest, object) = (*dest, *object);
                        let keywords: Vec<&str> =
                            kwargs.iter().map(|(name, _)| name.as_str()).collect();
                        let dropped = self.rewrite_subscript_call(
                            owner,
                            function,
                            object,
                            SubscriptOperands {
                                registers: &[],
                                positional: args.len(),
                                keywords: &keywords,
                            },
                            Some(dest),
                            call,
                        )?;
                        if let Some(dropped) = dropped {
                            drop_subscript_operands(args, arg_places, &dropped.positional);
                            drop_subscript_operands(kwargs, kwarg_places, &dropped.keyword);
                        }
                    }
                    MirInstr::MultiSet {
                        receiver,
                        args,
                        call,
                        ..
                    } => {
                        let receiver = *receiver;
                        self.rewrite_subscript_call(
                            owner,
                            function,
                            receiver,
                            SubscriptOperands {
                                registers: &[],
                                positional: args.len(),
                                keywords: &[],
                            },
                            None,
                            call,
                        )?;
                    }
                    // An untyped iterator slot passes through: it belongs to
                    // a compiler-private pack loop the backend rejects at its
                    // own boundary.
                    MirInstr::TryNext {
                        iter,
                        call,
                        adapter_constructor,
                        ..
                    } => {
                        if let Some(receiver) = function.var_tys.get(iter).cloned() {
                            // The adapter copies the element out of a
                            // reference the step returns: a step that
                            // returns the element itself needs none.
                            if !self.iterator_step_returns_reference(
                                &receiver,
                                "__next__",
                                Some(&call.target),
                            ) {
                                call.result_adapter = None;
                                *adapter_constructor = None;
                            }
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
                        if let Some(TyKind::Variant(alternatives)) =
                            function.reg_types.get(&a.0).map(peel_refs).map(Ty::kind)
                        {
                            for alternative in alternatives {
                                if matches!(alternative.kind(), TyKind::Struct(..)) {
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
            block.instrs.iter_mut().for_each(drop_parameter_slots);
            // A conversion follows instruction `index` and a prelude precedes
            // it, so at one position the conversion comes first.
            let mut insertions: Vec<(usize, bool, Vec<MirInstr>)> = conversions
                .into_iter()
                .map(|(index, conversion)| (index + 1, false, vec![conversion]))
                .chain(
                    preludes
                        .into_iter()
                        .map(|(index, prelude)| (index, true, prelude)),
                )
                .collect();
            insertions.sort_by_key(|(position, prelude, _)| (*position, *prelude));
            for (position, _, instructions) in insertions.into_iter().rev() {
                block.instrs.splice(position..position, instructions);
            }
        }
        Ok(())
    }

    pub(super) fn discover_structs(
        &mut self,
        owner: &str,
        function: &MirFunction,
    ) -> Result<(), MonoError> {
        let mut seeds = function_types(function).cloned().collect::<Vec<_>>();
        // Storage take/destroy intrinsics name their element type directly on
        // the instruction; seed it so the element's struct lays out and its
        // lifecycle members join the walk even when no register or variable
        // carries the bare element type.
        push_instruction_types(&function.blocks, &mut seeds);
        // Each type beside whether a value of it is held: a struct's
        // argument and a pointee are not laid out by the type naming them.
        let mut types = seeds.into_iter().map(|ty| (ty, true)).collect::<Vec<_>>();
        while let Some((ty, held)) = types.pop() {
            if matches!(ty.kind(), TyKind::Struct(..)) && !self.discovered_types.insert(ty.clone())
            {
                continue;
            }
            let mut nested = Vec::new();
            collect_nested_types(&ty, &mut nested);
            let opaque = matches!(ty.kind(), TyKind::Struct(..) | TyKind::Pointer { .. });
            types.extend(nested.into_iter().map(|nested| (nested, held && !opaque)));
            let TyKind::Struct(name, _) = ty.kind() else {
                continue;
            };
            // The checker-virtual slice descriptors have no source template;
            // give them the backend's raw layout (three i64 bounds plus a
            // presence bitmask — the VM's `Value::Slice` `Option<i64>` fields)
            // so descriptor-typed parameters and locals lay out.
            if matches!(name.as_str(), "Slice" | "ContiguousSlice" | "StridedSlice") {
                if !self.output_struct_positions.contains_key(name) {
                    self.declare_output_struct(MirStructDeclaration {
                        name: name.clone(),
                        fields: vec![
                            ("start".to_string(), Ty::int()),
                            ("end".to_string(), Ty::int()),
                            ("step".to_string(), Ty::int()),
                            ("flags".to_string(), Ty::int()),
                        ],
                        mut_self_methods: HashSet::default(),
                        fieldwise_init: false,
                        param_decls: Vec::new(),
                        explicit_destructors: HashMap::new(),
                        explicit_destroy: None,
                        conformances: Vec::new(),
                        declared: Vec::new(),
                        associated_types: Vec::new(),
                        parameterized_associated_types: Vec::new(),
                        associated_values: Vec::new(),
                        parameterized_associated_values: Vec::new(),
                        members: Vec::new(),
                        witness_tables: Vec::new(),
                    });
                }
                continue;
            }
            let template_name = name.split("$mono").next().unwrap_or(name).to_string();
            let Some(template) = self.structs.get(template_name.as_str()) else {
                continue;
            };
            // A static method being run at compile time may be what a field
            // type of its own struct applies (`SIMD[dt, Self.width()]`): the
            // struct is declared by the body that uses it, once the method
            // has run. A struct whose field types are closed lays out now,
            // for the method to construct it. A method that removed its
            // struct's parameters is named off the template.
            if template.fields.iter().any(|(_, ty)| is_symbolic(ty))
                && self.demand_stack.iter().any(|demanded| {
                    mojito_symbol::symbol::split_method_symbol(demanded)
                        .is_some_and(|(instance, _)| instance == name || instance == template_name)
                })
            {
                self.discovered_types.remove(&ty);
                continue;
            }
            let Some(bindings) = self.struct_bindings(owner, &ty)? else {
                continue;
            };
            // A field type may apply a function to the struct's parameters
            // (`SIMD[dt, h(Self.n)]`): the application is demanded as a
            // body's is, and the fields are substituted again.
            let fields = loop {
                self.applications.pending.borrow_mut().clear();
                let fields = template
                    .fields
                    .iter()
                    .map(|(field, ty)| Ok((field.clone(), substitute_field(ty, &bindings)?)))
                    .collect::<Result<Vec<_>, MonoError>>();
                if !self.answer_pending(owner, &bindings)? {
                    break self.erased_layout(owner, template, &bindings, fields);
                }
            };
            // An erased instance no value holds needs no layout, as upstream
            // lays out no pointee; one a value reaches later lays out then.
            let fields = match fields {
                Err(_) if !held => {
                    self.discovered_types.remove(&ty);
                    continue;
                }
                fields => fields?,
            };
            let mut declaration = template.clone();
            declaration.fields = fields;
            declaration.name.clone_from(name);
            declaration.param_decls.clear();
            declaration.associated_types.clear();
            declaration.parameterized_associated_types.clear();
            declaration.associated_values.clear();
            declaration.parameterized_associated_values.clear();
            declaration.members.clear();
            declaration.witness_tables.clear();
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
                .output_struct_positions
                .get(&declaration.name)
                .map(|&position| &self.output_structs[position])
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
            types.extend(declaration.fields.iter().map(|(_, ty)| (ty.clone(), true)));
            declaration.conformances =
                self.instance_conformances(&template.conformances, &bindings);
            declaration.declared = self.instance_conformances(&template.declared, &bindings);
            self.declare_output_struct(declaration);
        }
        self.enqueue_lifecycle_members(owner, function)
    }

    /// The substituted `fields` of a struct instance under `bindings`. An
    /// erased instance (`Box[_REMOVED_ARG]`) lays out only where no field
    /// reads an erased argument; one that does fails explicitly, since the
    /// instance taking it removed a parameter its layout needs.
    fn erased_layout(
        &self,
        owner: &str,
        template: &MirStructDeclaration,
        bindings: &Bindings,
        fields: Result<Vec<(String, Ty)>, MonoError>,
    ) -> Result<Vec<(String, Ty)>, MonoError> {
        if bindings.removed.is_empty() {
            return fields;
        }
        if fields
            .as_ref()
            .is_ok_and(|fields| !fields.iter().any(|(_, ty)| is_symbolic(ty)))
        {
            return fields;
        }
        let read = bindings.removed.iter().find(|binder| {
            template
                .fields
                .iter()
                .any(|(_, ty)| mojito_types::types::names_binder(ty, binder))
        });
        match read {
            Some(binder) => Err(self.error(
                Some(owner),
                format!(
                    "`{}` lays out parameter `{}`, which `{owner}` removed",
                    template.name, binder.name
                ),
            )),
            None => fields,
        }
    }

    /// Enqueue the destructor of every struct `function` tears down
    /// ([`destroyed_structs`]), and the copy and move constructors of every
    /// struct it copies and moves ([`relocated_structs`]), without naming
    /// them, as upstream's destructor and copy/move insertion leaves a call
    /// to each at its site: the member is a reference of the body, as a
    /// call is. A body that drops, copies, or moves a value of a symbolic
    /// type demands that member of every struct it names. A copy or move
    /// of an erased instance (`Box[_REMOVED_ARG]`), or of one over an
    /// erased type, runs its member only when that member removed every
    /// erased parameter, as the generator that copies or moves it read the
    /// member's kept parameters alone (`mir::StructReads::member_reads`);
    /// a member keeping one is demanded by no generator that erased it.
    fn enqueue_lifecycle_members(
        &mut self,
        owner: &str,
        function: &MirFunction,
    ) -> Result<(), MonoError> {
        let structs = |name: &str| {
            self.output_struct_positions
                .get(name)
                .map(|&position| &self.output_structs[position])
        };
        let fields = |name: &str| structs(name).map(|declaration| declaration.fields.as_slice());
        let destroyed = destroyed_structs(function, &fields);
        let relocated = relocated_structs(function, &structs);
        let named = || {
            let mut types = function_types(function).cloned().collect::<Vec<_>>();
            push_instruction_types(&function.blocks, &mut types);
            let mut structs = Vec::new();
            while let Some(ty) = types.pop() {
                collect_nested_types(&ty, &mut types);
                if matches!(ty.kind(), TyKind::Struct(..)) {
                    structs.push(ty);
                }
            }
            structs
        };
        let demands = [
            ("__deinit__", destroyed.structs, destroyed.symbolic),
            ("__copyinit__", relocated.copied, relocated.symbolic),
            ("__moveinit__", relocated.moved, relocated.symbolic),
        ];
        for (method, mut structs, symbolic) in demands {
            if symbolic {
                structs.extend(named());
            }
            let mut seen = HashSet::new();
            for ty in structs {
                if !seen.insert(ty.clone()) {
                    continue;
                }
                let TyKind::Struct(name, _) = ty.kind() else {
                    continue;
                };
                let template_name = name.split("$mono").next().unwrap_or(name).to_string();
                let Some(bindings) = self.struct_bindings(owner, &ty)? else {
                    continue;
                };
                if method != "__deinit__"
                    && self.keeps_an_erased_parameter(&template_name, &ty, &bindings, method)
                {
                    continue;
                }
                self.enqueue_struct_members(&template_name, &bindings, method)?;
            }
        }
        Ok(())
    }

    /// The declaration of the template the instance `instance` is of, or of
    /// `instance` itself where it is a template's own name.
    fn template_declaration(&self, instance: &str) -> Option<&'a MirFunctionDeclaration> {
        let template = self
            .instance_names
            .get(instance)
            .map(|&index| self.instances[index].0.template.as_str())
            .or_else(|| mojito_symbol::symbol::specialization_template(instance))
            .unwrap_or(instance);
        self.declarations.get(template)
    }

    /// Name, at each drop of `function`, the `__deinit__` instance of the
    /// dropped value's type, and at each copy or move the `__copyinit__` or
    /// `__moveinit__` instance of the copied or moved value's type, as
    /// upstream's elaborator specializes the call its destructor and
    /// copy/move insertion left at each site: the instance [`Self::enqueue`]
    /// makes, which the struct's concrete member table records. A site
    /// whose type was symbolic in the template gets its instance's member
    /// here too.
    fn name_lifecycle_calls(
        &mut self,
        owner: &str,
        function: &mut MirFunction,
    ) -> Result<(), MonoError> {
        let mut named: HashMap<(Ty, &'static str), Option<String>> = HashMap::new();
        for_each_lifecycle_site_mut(
            &mut function.blocks,
            &function.var_tys,
            &function.reg_types,
            &mut |ty, member, symbol| {
                let key = (ty.clone(), member);
                if !named.contains_key(&key) {
                    let instance = self.lifecycle_instance(owner, ty, member)?;
                    named.insert(key.clone(), instance);
                }
                symbol.clone_from(&named[&key]);
                Ok(())
            },
        )
    }

    /// The one `member` instance of the struct instance `ty`, `None` when it
    /// has none, or when it is a copy or move constructor keeping a
    /// parameter `ty` erased, which no generator copying or moving it
    /// demands ([`Self::enqueue_lifecycle_members`]).
    fn lifecycle_instance(
        &mut self,
        owner: &str,
        ty: &Ty,
        member: &str,
    ) -> Result<Option<String>, MonoError> {
        let TyKind::Struct(name, _) = ty.kind() else {
            return Ok(None);
        };
        let Some(bindings) = self.struct_bindings(owner, ty)? else {
            return Ok(None);
        };
        let template = nominal_template(name);
        if member != "__deinit__" && self.keeps_an_erased_parameter(template, ty, &bindings, member)
        {
            return Ok(None);
        }
        let instances = self.enqueue_struct_overloads(template, &bindings, member, &|_, _| true)?;
        Ok(match instances.as_slice() {
            [(_, instance)] => Some(instance.clone()),
            _ => None,
        })
    }

    /// Whether an overload of `{template_name}.{method}` keeps a parameter
    /// of its struct that the instance `ty`, bound by `bindings`, erased or
    /// binds to a type erasing an argument.
    fn keeps_an_erased_parameter(
        &self,
        template_name: &str,
        ty: &Ty,
        bindings: &Bindings,
        method: &str,
    ) -> bool {
        let Some(template) = self.structs.get(template_name) else {
            return false;
        };
        let arguments = match ty.kind() {
            TyKind::Struct(_, arguments) => arguments.as_slice(),
            _ => &[],
        };
        let erased: Vec<&mojito_types::param_expr::ParamId> = template
            .param_decls
            .iter()
            .enumerate()
            .filter(|(position, decl)| {
                bindings
                    .removed
                    .iter()
                    .any(|binder| binder.id == *decl.id())
                    || matches!(arguments.get(*position), Some(TyArg::Ty(argument))
                        if mojito_types::types::erases_an_argument(argument))
            })
            .map(|(_, decl)| decl.id())
            .collect();
        !erased.is_empty()
            && self
                .functions
                .family(&format!("{template_name}.{method}"))
                .iter()
                .any(|candidate| {
                    let removed = self.removed_params(candidate, false);
                    erased.iter().any(|id| !removed.contains(id))
                })
    }

    /// The bindings of the struct instance `ty`, `None` when it names no
    /// struct template or applies too few of its parameters.
    fn struct_bindings(&self, owner: &str, ty: &Ty) -> Result<Option<Bindings>, MonoError> {
        let TyKind::Struct(name, arguments) = ty.kind() else {
            return Ok(None);
        };
        let template_name = name.split("$mono").next().unwrap_or(name);
        let Some(template) = self.structs.get(template_name) else {
            return Ok(None);
        };
        // An instance applied element by element binds its lone pack to
        // every argument, of which the empty tuple has none.
        let elementwise = name != template_name
            && matches!(
                template.param_decls.as_slice(),
                [ParamDecl::Type { variadic: true, .. }]
            );
        if arguments.len() < template.param_decls.len() && !elementwise {
            return Ok(None);
        }
        let mut bindings = self.base_bindings();
        bind_ty_args(&template.param_decls, arguments, &mut bindings).map_err(|e| {
            self.error(
                Some(owner),
                format!("monomorphizing struct `{template_name}`: {e}"),
            )
        })?;
        if name != template_name {
            bindings.self_instance = Some((
                template_name.to_string(),
                TyKind::Struct(name.clone(), arguments.clone()).into(),
            ));
        }
        Ok(Some(bindings))
    }

    /// Enqueue the members `{template_name}.{method}` the struct instance
    /// `bindings` binds has: each overload its arguments determine and its
    /// `where` clause does not disprove.
    fn enqueue_struct_members(
        &mut self,
        template_name: &str,
        bindings: &Bindings,
        method: &str,
    ) -> Result<(), MonoError> {
        self.enqueue_struct_overloads(template_name, bindings, method, &|_, _| true)
            .map(drop)
    }

    /// Enqueue the constructors a slice descriptor's bound read builds its
    /// `Optional` result `bound` through, the VM's `slice_bound_optional`:
    /// the nullary one and the positional one-argument one, which the read
    /// names. Returns their instances.
    fn enqueue_slice_bound_constructors(
        &mut self,
        owner: &str,
        bound: &Ty,
    ) -> Result<Vec<String>, MonoError> {
        let TyKind::Struct(name, _) = bound.kind() else {
            return Ok(Vec::new());
        };
        let Some(bindings) = self.struct_bindings(owner, bound)? else {
            return Ok(Vec::new());
        };
        let constructors = self.enqueue_struct_overloads(
            nominal_template(name),
            &bindings,
            "__init__",
            &|candidate, declaration| {
                declaration.param_names.len() <= 1
                    && !candidate.contains("$kw$")
                    && !mojito_symbol::symbol::is_none_overload(candidate)
            },
        )?;
        Ok(constructors
            .into_iter()
            .map(|(_, instance)| instance)
            .collect())
    }

    /// Enqueue the overloads `{template_name}.{method}` of the struct
    /// instance `bindings` binds that `select` accepts, as
    /// [`Self::enqueue_struct_members`] does, and return each overload
    /// with the instance it names.
    fn enqueue_struct_overloads(
        &mut self,
        template_name: &str,
        bindings: &Bindings,
        method: &str,
        select: &dyn Fn(&str, &MirFunctionDeclaration) -> bool,
    ) -> Result<Vec<(String, String)>, MonoError> {
        let mut enqueued = Vec::new();
        let Some(template) = self.structs.get(template_name) else {
            return Ok(enqueued);
        };
        let base = format!("{template_name}.{method}");
        for candidate in self.functions.family(&base) {
            let Some(function_decl) = self.declarations.get(candidate.as_str()) else {
                continue;
            };
            if !select(&candidate, function_decl) {
                continue;
            }
            // An unspecialized variadic overload cannot materialize
            // without a call-site arity; those sites enqueue it.
            if arity_keyed_variadic(function_decl) {
                continue;
            }
            let covered = if bindings.self_instance.is_some() {
                owner_covered_prefix(&template.param_decls, &function_decl.param_decls)
            } else {
                0
            };
            let Ok(method_arguments) =
                self.own_arguments(&function_decl.param_decls, bindings, &candidate, covered)
            else {
                continue;
            };
            // A member whose `where` clause the instance disproves
            // is not part of it. A constructor whose clause stays
            // undecided waits for the call site that reaches it;
            // lowering composes the copy/move/deinit names itself,
            // so those join the instance unless disproven.
            match self.availability(function_decl, bindings) {
                Availability::Proven => {}
                Availability::Undecided if method != "__init__" => {}
                Availability::Disproven(_) | Availability::Undecided => continue,
            }
            let instance = self.enqueue(&candidate, bindings.clone(), method_arguments)?;
            enqueued.push((candidate, instance));
        }
        Ok(enqueued)
    }

    /// Whether a `!=` on `receiver` is served by `Equatable`'s default: its
    /// struct declares an `__eq__` and no `__ne__`.
    fn negates_equality(&self, function: &MirFunction, receiver: Reg) -> bool {
        let Some(TyKind::Struct(name, _)) =
            (function.reg_types.get(&receiver.0).map(peel_refs)).map(Ty::kind)
        else {
            return false;
        };
        let declares = |method: &str| {
            let base = format!("{}.{method}", nominal_template(name));
            self.functions.prefixed(base.clone()).any(|candidate| {
                candidate
                    .name
                    .strip_prefix(base.as_str())
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with('$'))
            })
        };
        declares("__eq__") && !declares("__ne__")
    }

    /// The unique `__init__` overload of `init_base`'s struct whose
    /// runtime-pack collector binds any element count (current Tuple's
    /// `__init__(out self, var *args: *Ts)` beside its nullary constructor),
    /// which arity-keyed selection cannot see.
    fn runtime_pack_constructor(&self, init_base: &str) -> Option<String> {
        let mut packs = self.functions.overloads(init_base).filter(|candidate| {
            self.declarations
                .get(candidate.name)
                .is_some_and(|declaration| {
                    declaration.variadic.as_ref().is_some_and(|element| {
                        matches!(element.kind(), mojito_types::types::TyKind::RuntimePack(_))
                            || mojito_types::types::pack_spread(std::slice::from_ref(element))
                                .is_some()
                    })
                })
        });
        let first = packs.next()?.name.to_string();
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
        let scope = self.binder_scope(owner);
        let locals = bound_parameter_locals(&scope, bindings);
        (0u32..)
            .zip(&function.var_names)
            .filter_map(|(slot, name)| {
                let value = bindings
                    .callables
                    .get(name)
                    .map(|callable| CtValue::Str(callable.clone()))
                    .or_else(|| locals.get(name.as_str()).map(|value| (*value).clone()))?;
                Some((slot, value))
            })
            .collect()
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
            kind: MonoErrorKind::Unsupported,
            function: function.map(str::to_string),
            construct: construct.into(),
        }
    }
}

/// Whether `declaration` collects a variadic parameter whose element count a
/// call site keys (`var *values: T`), rather than a runtime pack or a tuple.
fn arity_keyed_variadic(declaration: &MirFunctionDeclaration) -> bool {
    matches!(&declaration.variadic, Some(element)
        if !matches!(element.kind(), TyKind::RuntimePack(_) | TyKind::Tuple(_)))
}

/// Append a call's trailing arguments for its capturing callable parameters,
/// queueing the loads they need ahead of instruction `index`.
fn pass_capturing_arguments(
    capturing: CapturingArguments,
    index: usize,
    args: &mut Vec<Reg>,
    arg_places: &mut Vec<Option<MirPlace>>,
    preludes: &mut Vec<(usize, Vec<MirInstr>)>,
) {
    arg_places.resize(args.len(), None);
    for closure in capturing.closures {
        args.push(closure);
        arg_places.push(None);
    }
    if capturing.environment.is_empty() {
        return;
    }
    let mut loads = Vec::with_capacity(capturing.environment.len());
    for (dest, place) in capturing.environment {
        args.push(dest);
        arg_places.push(Some(place.clone()));
        loads.push(MirInstr::LoadPlace { dest, place });
    }
    preludes.push((index, loads));
}

/// Remove from a multi-subscript's `operands`, and their `places`, the
/// `dropped` indexes its accessor's removed arguments bound.
fn drop_subscript_operands<T>(
    operands: &mut Vec<T>,
    places: &mut Vec<Option<MirPlace>>,
    dropped: &BTreeSet<usize>,
) {
    for &index in dropped.iter().rev() {
        if index < operands.len() {
            operands.remove(index);
        }
        if index < places.len() {
            places.remove(index);
        }
    }
}

/// Drops the compile-time argument slots `instruction` still carries: a
/// concrete call has no parameter list, its compile-time arguments being
/// attributes its instantiated arguments carry. A slot that still holds a
/// run-time register stays, with every other slot of its call, for the
/// concrete verifier to report.
fn drop_parameter_slots(instruction: &mut MirInstr) {
    let slots = match instruction {
        MirInstr::Call { param_arg_regs, .. }
        | MirInstr::MethodCall { param_arg_regs, .. }
        | MirInstr::CallIndirect { param_arg_regs, .. } => param_arg_regs,
        MirInstr::Index {
            call: Some(call), ..
        }
        | MirInstr::Slice {
            call: Some(call), ..
        }
        | MirInstr::MultiIndex {
            call: Some(call), ..
        }
        | MirInstr::MultiSet { call, .. } => &mut call.param_arg_regs,
        _ => return,
    };
    if slots.iter().all(|slot| slot.value.is_none()) {
        slots.clear();
    }
}

/// Every register a compile-time argument slot in `blocks` holds, their
/// regions included.
fn parameter_argument_regs(blocks: &mut [MirBlock]) -> HashSet<u32> {
    let mut regs = HashSet::new();
    for instruction in blocks.iter_mut().flat_map(|block| &mut block.instrs) {
        regs.extend(
            mojito_mir::mir::instruction_param_args_mut(instruction)
                .into_iter()
                .filter_map(|argument| argument.value.map(|reg| reg.0)),
        );
        for region in mojito_mir::mir::instruction_regions_mut(instruction) {
            regs.extend(parameter_argument_regs(region));
        }
    }
    regs
}

/// Erase the constant each of `arguments` was read into once no
/// instruction reads it: the call it fed takes its compile-time argument
/// as an attribute, as upstream's call carries a parameter, so the instance
/// holds no register for it.
fn erase_dropped_argument_reads(function: &mut MirFunction, arguments: &HashSet<u32>) {
    if arguments.is_empty() {
        return;
    }
    let mut reads = vec![0; function.n_regs as usize];
    mojito_mir::mir::count_register_reads(&mut function.blocks, &mut reads);
    let dead: HashSet<u32> = arguments
        .iter()
        .copied()
        .filter(|reg| reads.get(*reg as usize) == Some(&0))
        .collect();
    let mut erased = HashSet::new();
    erase_constants(&mut function.blocks, &dead, &mut erased);
    for reg in erased {
        function.reg_types.remove(&reg);
        function.spans.0.remove(&reg);
    }
}

/// Drop the types of `operands`, the registers a call passed for a
/// removed argument whose definitions the instance erased
/// ([`mojito_mir::mir::with_removed_argument_forwards_erased`]), once no
/// instruction reads them: the call dropped them, as upstream's rewritten
/// call passes none, so the instance holds no register for one.
fn shed_dropped_operand_types(function: &mut MirFunction, operands: &[Reg]) {
    let mut reads = vec![0; function.n_regs as usize];
    mojito_mir::mir::count_register_reads(&mut function.blocks, &mut reads);
    for reg in operands {
        if reads.get(reg.0 as usize) == Some(&0) {
            function.reg_types.remove(&reg.0);
            function.spans.0.remove(&reg.0);
        }
    }
}

/// Remove each `Const` in `blocks`, their regions included, that defines
/// one of `dead`, recording its register in `erased`.
fn erase_constants(blocks: &mut [MirBlock], dead: &HashSet<u32>, erased: &mut HashSet<u32>) {
    for block in blocks {
        block.instrs.retain(|instruction| match instruction {
            MirInstr::Const { dest, .. } if dead.contains(&dest.0) => {
                erased.insert(dest.0);
                false
            }
            _ => true,
        });
        for instruction in &mut block.instrs {
            for region in mojito_mir::mir::instruction_regions_mut(instruction) {
                erase_constants(region, dead, erased);
            }
        }
    }
}

/// The first effectful builtin (`print`, `input`) `entry` reaches in
/// `program`, through every edge by which one function names another.
fn effectful_callee(program: &MirProgram, entry: &str) -> Option<String> {
    let mut pending = vec![entry.to_string()];
    let mut seen = HashSet::new();
    while let Some(name) = pending.pop() {
        if !seen.insert(name.clone()) {
            continue;
        }
        let Some((_, function)) = program.functions.iter().find(|(known, _)| *known == name) else {
            continue;
        };
        let mut referenced = Vec::new();
        for block in &function.blocks {
            collect_referenced_functions(&block.instrs, &mut referenced);
        }
        for callee in referenced {
            if matches!(callee.as_str(), "print" | "input") {
                return Some(callee);
            }
            pending.push(callee);
        }
    }
    None
}

/// The functions `instructions` name: a call's callee, a method call's or a
/// subscript's resolved target, a callable struct's `__call__`, a closure's
/// lifted body, and those of the regions they hold.
fn collect_referenced_functions(instructions: &[MirInstr], referenced: &mut Vec<String>) {
    for instruction in instructions {
        match instruction {
            MirInstr::Call { func, .. } => referenced.push(func.0.clone()),
            MirInstr::MethodCall { resolved, .. } | MirInstr::CallIndirect { resolved, .. } => {
                referenced.extend(resolved.iter().cloned());
            }
            MirInstr::MakeClosure { function, .. } => referenced.push(function.clone()),
            MirInstr::Index { call, .. }
            | MirInstr::Slice { call, .. }
            | MirInstr::MultiIndex { call, .. } => {
                referenced.extend(call.iter().map(|call| call.target.clone()));
            }
            MirInstr::MultiSet { call, .. } => referenced.push(call.target.clone()),
            instruction => {
                for block in mojito_mir::mir::instruction_regions(instruction)
                    .into_iter()
                    .flatten()
                {
                    collect_referenced_functions(&block.instrs, referenced);
                }
            }
        }
    }
}

/// Every expression operand of a constraint, in reading order.
fn constraint_expressions(constraint: &GenericConstraint) -> Vec<ParamExpr> {
    use GenericConstraint::{
        And, Bool, Conforms, ConformsPack, Eq, Ge, Gt, Le, Lt, Ne, Not, Or, PackContains,
        PackPredicate, Trivial, WithMessage,
    };
    let of_operand = |operand: &ConstraintOperand| match operand {
        ConstraintOperand::Expr(expr) | ConstraintOperand::Value(CtValue::Expr(expr)) => {
            vec![expr.clone()]
        }
        _ => Vec::new(),
    };
    match constraint {
        Bool(_) | Conforms { .. } | ConformsPack { .. } | PackPredicate { .. } => Vec::new(),
        WithMessage(inner, _) | Not(inner) => constraint_expressions(inner),
        And(left, right) | Or(left, right) => {
            let mut expressions = constraint_expressions(left);
            expressions.extend(constraint_expressions(right));
            expressions
        }
        PackContains { element, .. } => of_operand(element),
        Trivial(_, operand) => of_operand(operand),
        Eq(left, right)
        | Ne(left, right)
        | Lt(left, right)
        | Le(left, right)
        | Gt(left, right)
        | Ge(left, right) => {
            let mut expressions = of_operand(left);
            expressions.extend(of_operand(right));
            expressions
        }
    }
}

/// Bind an enclosing instance's solutions in a nested body's call-site
/// bindings, beside the body's own binders.
fn inherit_bindings(bindings: &mut Bindings, inherited: Bindings) {
    for (binder, ty) in inherited.types {
        bindings.types.entry(binder).or_insert(ty);
    }
    for (binder, value) in inherited.values {
        bindings.values.entry(binder).or_insert(value);
    }
    for (name, callee) in inherited.callables {
        bindings.callables.entry(name).or_insert(callee);
    }
    if bindings.self_instance.is_none() {
        bindings.self_instance = inherited.self_instance;
    }
}

/// The pass statistics of a finished elaboration: every instance minted under
/// a name other than its template's is a generator instance.
fn elaboration_counts(
    instances: &[(InstanceKey, String)],
    templates: &bodies::Bodies<'_>,
    program: &MirProgram,
) -> ElaborationCounts {
    let output: HashSet<&str> = program
        .functions
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    let mut generators: BTreeMap<&str, GeneratorCount> = BTreeMap::new();
    for (key, name) in instances {
        if *name == key.template || !output.contains(name.as_str()) {
            continue;
        }
        let row = generators.entry(key.template.as_str()).or_insert_with(|| {
            let template = templates.get(&key.template);
            GeneratorCount {
                template: key.template.clone(),
                template_instructions: template.map_or(0, MirFunction::instruction_count),
                template_blocks: template.map_or(0, MirFunction::block_count),
                ..GeneratorCount::default()
            }
        });
        row.instances += 1;
        row.instance_names.push(name.clone());
    }
    let mut counts = ElaborationCounts {
        generators: generators.into_values().collect(),
        concrete_instructions: 0,
        concrete_blocks: 0,
    };
    counts.recount(program);
    counts
}

/// The member table key a constructor `template` of the struct `name` is
/// recorded under: its template's method spelling (`__init__$ov$`).
fn constructed_member(name: &str, template: &str) -> Option<(String, String)> {
    let (_, method) = mojito_symbol::symbol::split_method_symbol(template)?;
    Some((name.to_string(), method.to_string()))
}

/// What [`for_each_lifecycle_site_mut`] calls on each site: its value's
/// type, the lifecycle member it runs, and its symbol operand.
type LifecycleNamer<'n> =
    dyn FnMut(&Ty, &'static str, &mut Option<String>) -> Result<(), MonoError> + 'n;

/// Name, through `name`, the destructor each alternative of the variant at
/// `place` runs when a store replaces it: one `destructors` entry per
/// alternative.
fn name_variant_destructors(
    place: &MirPlace,
    destructors: &mut Vec<Option<String>>,
    var_tys: &HashMap<mojito_hir::hir::VarId, Ty>,
    name: &mut LifecycleNamer<'_>,
) -> Result<(), MonoError> {
    let alternatives =
        mojito_mir::mir::variant_place_alternatives(place, var_tys).unwrap_or_default();
    destructors.resize(alternatives.len(), None);
    alternatives
        .iter()
        .zip(destructors.iter_mut())
        .try_for_each(|(alternative, destructor)| name(alternative, "__deinit__", destructor))
}

/// Call `name` on each `drop.var`, `drop.place`, cleanup-list drop,
/// storage destroy, variant store (per alternative), owned closure capture
/// (its constructor and its destructor), copying or moving `var.use`, and
/// `value.copy` of `blocks`, regions included, with the value's type, the
/// member the site runs, and its `destructor` or `constructor`.
fn for_each_lifecycle_site_mut(
    blocks: &mut [MirBlock],
    var_tys: &HashMap<mojito_hir::hir::VarId, Ty>,
    reg_types: &HashMap<u32, Ty>,
    name: &mut LifecycleNamer<'_>,
) -> Result<(), MonoError> {
    let name_cleanup = |cleanup: &mut [CleanupDrop], name: &mut LifecycleNamer<'_>| {
        cleanup.iter_mut().try_for_each(|drop| {
            var_tys
                .get(&drop.var)
                .map_or(Ok(()), |ty| name(ty, "__deinit__", &mut drop.destructor))
        })
    };
    for block in blocks.iter_mut() {
        if let MirTerm::ReturnWithCleanup { cleanup, .. } = &mut block.term {
            name_cleanup(cleanup, name)?;
        }
        for instruction in &mut block.instrs {
            let site = match instruction {
                MirInstr::DropVar { var, destructor } => {
                    var_tys.get(var).map(|ty| (ty, "__deinit__", destructor))
                }
                MirInstr::DropPlace { place, destructor } => {
                    place.ty.as_ref().map(|ty| (ty, "__deinit__", destructor))
                }
                MirInstr::UseVar {
                    var,
                    mode,
                    constructor,
                    ..
                } => var_tys
                    .get(var)
                    .zip(mojito_mir::mir::use_constructor_member(*mode))
                    .map(|(ty, member)| (ty, member, constructor)),
                MirInstr::CopyValue {
                    dest, constructor, ..
                } => reg_types
                    .get(&dest.0)
                    .map(|ty| (ty, "__copyinit__", constructor)),
                MirInstr::MakeClosure { captures, .. } => {
                    for capture in captures {
                        let Some(ty) = capture.place.storage_ty(var_tys) else {
                            continue;
                        };
                        if let Some(member) = capture.constructor_member() {
                            name(ty, member, &mut capture.constructor)?;
                        }
                        if let Some(member) = capture.destructor_member() {
                            name(ty, member, &mut capture.destructor)?;
                        }
                    }
                    None
                }
                MirInstr::PointerStorageDestroy {
                    element,
                    destructor,
                    ..
                }
                | MirInstr::UninitStorageDestroy {
                    element,
                    destructor,
                    ..
                } => Some((&*element, "__deinit__", destructor)),
                MirInstr::Try { cleanup, .. } => {
                    name_cleanup(cleanup, name)?;
                    None
                }
                MirInstr::MakeVariant {
                    alternatives,
                    index,
                    constructor,
                    ..
                } => mojito_mir::mir::variant_payload_ty(alternatives, index)
                    .map(|ty| (ty, "__copyinit__", constructor)),
                MirInstr::VariantSet {
                    place,
                    index,
                    constructor,
                    destructors,
                    ..
                } => {
                    if let Some(ty) =
                        mojito_mir::mir::variant_place_payload_ty(place, index, var_tys)
                    {
                        name(ty, "__copyinit__", constructor)?;
                    }
                    name_variant_destructors(place, destructors, var_tys, name)?;
                    None
                }
                MirInstr::VariantSetInitWith {
                    place, destructors, ..
                } => {
                    name_variant_destructors(place, destructors, var_tys, name)?;
                    None
                }
                MirInstr::VariantReplace {
                    place,
                    input_index: index,
                    constructor,
                    ..
                } => mojito_mir::mir::variant_place_payload_ty(place, index, var_tys)
                    .map(|ty| (ty, "__copyinit__", constructor)),
                MirInstr::MethodCall {
                    dest,
                    result_adapter: Some(adapter),
                    ..
                } => reg_types
                    .get(&dest.0)
                    .map(|ty| (ty, "__copyinit__", &mut adapter.constructor)),
                MirInstr::TryNext {
                    dest,
                    call,
                    adapter_constructor,
                    ..
                } if call.result_adapter.is_some() => reg_types
                    .get(&dest.0)
                    .map(|ty| (ty, "__copyinit__", adapter_constructor)),
                _ => None,
            };
            if let Some((ty, member, symbol)) = site {
                name(ty, member, symbol)?;
            }
            for region in mojito_mir::mir::instruction_regions_mut(instruction) {
                for_each_lifecycle_site_mut(region, var_tys, reg_types, name)?;
            }
        }
    }
    Ok(())
}
