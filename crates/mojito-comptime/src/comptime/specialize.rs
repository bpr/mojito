//! Monomorphization and specialization generation: `monomorphize`, tuple-spec
//! ordering, and `def`/`struct` specialization synthesis.
//! Extracted from `comptime.rs`; see `docs/symbol-map.md`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_ast::ast::Method;

impl Elab<'_> {
    /// Specialize every comptime-dependent generic template against the value
    /// arguments at its call sites, replacing each template with its concrete
    /// specializations (which have their `comptime if`/`for` resolved).
    pub(super) fn monomorphize(
        &self,
        program: Vec<Stmt>,
        tstring_requests: &[TStringSpecializationRequest],
        def_requests: &[DefSpecializationRequest],
        def_selections: &[DefSpecializationRequest],
    ) -> Result<Elaborated, ComptimeError> {
        if self.specializable.is_empty()
            && tstring_requests.is_empty()
            && self.instance_requests.is_empty()
        {
            return Ok(Elaborated {
                program,
                instances: Vec::new(),
                stub_reaching_structs: HashSet::new(),
                stub_reaching_methods: Vec::new(),
                unserved_template_uses: Vec::new(),
                def_traces: Vec::new(),
                method_traces: Vec::new(),
                generated: super::GeneratedDeclarations::default(),
                ctfe_template_stats: mojito_checked::templates::TemplateStats::default(),
                clones: mojito_checked::census::CloneCensus::default(),
            });
        }
        let consts = self.top_consts.borrow().clone();
        let mut mono = Mono::default();
        let mut program = program;
        self.stamp_per_call_clone_bodies(&mut program);
        self.per_call_stubs.get_or_init(|| per_call_stubs(&program));
        let mut module_bindings = HashMap::new();
        for statement in &program {
            if let StmtKind::Def { name, .. } | StmtKind::Struct { name, .. } = &statement.kind {
                module_bindings.insert(name.clone(), self.specializable.contains_key(name));
            }
        }
        mono.runtime_pack_scopes.push(
            module_bindings
                .keys()
                .map(|name| (name.clone(), None))
                .collect(),
        );
        mono.value_scopes.push(module_bindings);
        // Checker-discovered t-string occurrences: each records the element
        // types `mono_expr` rewrites the `t"…"` node by, into a construction
        // of the `TString` its template serves.
        for request in tstring_requests {
            mono.tstring_call_targets.insert(
                request.occurrence().clone().without_syntax(),
                request.elements().to_vec(),
            );
        }
        self.seed_def_call_targets(def_requests, &mut mono);
        self.seed_family_selections(def_selections, &mut mono);
        // Rewrite call sites in every non-template statement, seeding the
        // worklist. A bound-generic template's body is live code whether the
        // template is retained or dropped, so it is scanned like any other
        // statement (its own symbolic-argument calls soft-retain their
        // callees); comptime-class templates are replaced wholesale below.
        // Checker-discovered instances of ordinary generic structs seed the
        // instance worklist; the walk below and every generated clone can add
        // more (a closed application in an annotation or constructor call).
        let mut instance_templates: Vec<&String> = self.instance_requests.keys().collect();
        instance_templates.sort();
        for template in instance_templates {
            if !self.instance_template(template) {
                continue;
            }
            let Some(info) = self.structs.get(template.as_str()) else {
                continue;
            };
            // An instance whose argument carries a loan binds its origin
            // slots to the clone's own binders, a bundled template's as a
            // user template's; a clone of a certified template derives from
            // it rather than being checked again.
            for arguments in &self.instance_requests[template] {
                let mut origin_binders = CloneOriginBinders::default();
                let Some((values, _)) = self.method_request_values(
                    template,
                    info.source_params,
                    arguments,
                    &mut origin_binders,
                ) else {
                    continue;
                };
                if mono.instances_done.insert(mangle(template, &values)?) {
                    mono.instance_jobs.push_back((template.clone(), values));
                }
            }
        }
        for stmt in &mut program {
            if let StmtKind::Def { name, .. } | StmtKind::Struct { name, .. } = &stmt.kind
                && self.specializable.contains_key(name)
                && !self.bound_generics.contains(name)
                && !self.shares_a_family_name(stmt)
            {
                continue;
            }
            mono.in_bundled =
                mojito_checker::checker::is_bundled_module_source(stmt.module.as_deref());
            mono.abstract_owner = match &stmt.kind {
                StmtKind::Def { name, .. } if self.bound_generics.contains(name) => {
                    Some(name.clone())
                }
                _ => None,
            };
            self.mono_stmt(stmt, &consts, &mut mono)?;
        }
        mono.in_bundled = false;
        mono.abstract_owner = None;
        // Which erased method bodies can reach a compile-time-keyed stub is
        // settled once every template has been walked; the drain below needs
        // it to tell an instance that must mint such a method's clone from
        // one that keeps the erased path harmlessly.
        let stub_reaching: HashSet<String> = self
            .stub_reaching_bodies(&mono.abstract_uses, &mono.method_edges)
            .iter()
            .map(|body| (*body).to_string())
            .collect();
        // Drain the worklists: specializations, then the per-instantiation
        // method clones (whose bodies may request further specializations
        // and instances), until both are empty.
        loop {
            self.drain_specialization_jobs(&mut mono, &consts)?;
            let Some((template, values)) = mono.instance_jobs.pop_front() else {
                break;
            };
            self.mint_instance_clones(
                &mut program,
                &template,
                &values,
                &consts,
                &stub_reaching,
                &mut mono,
            )?;
        }
        // Rebuild the program, replacing each template with its specializations at
        // the template's original position. Specializations are emitted in reverse
        // generation order so a callee is defined before its caller (the checker
        // binds names sequentially, without forward references).
        let mut out = Vec::with_capacity(program.len());
        // A mixed family declares one name twice, once per specialization
        // class. Its clones are all filed under the name, so they are emitted
        // at its *last* template declaration: every retained sibling then
        // still precedes them, as a single template's stub does.
        let mut last_template = HashMap::new();
        for (index, stmt) in program.iter().enumerate() {
            if let StmtKind::Def { name, .. } | StmtKind::Struct { name, .. } = &stmt.kind
                && self.specializable.contains_key(name)
                && !self.shares_a_family_name(stmt)
            {
                last_template.insert(name.clone(), index);
            }
        }
        for (index, stmt) in program.into_iter().enumerate() {
            let template_name = match &stmt.kind {
                // A plain overload sharing a template family's name
                // is not a template: it survives the rebuild unchanged, and
                // the family's clones are emitted at its keyed sibling.
                StmtKind::Def { name, .. } | StmtKind::Struct { name, .. }
                    if self.specializable.contains_key(name)
                        && !self.shares_a_family_name(&stmt) =>
                {
                    name.clone()
                }
                _ => {
                    out.push(stmt);
                    continue;
                }
            };
            let generated = (last_template.get(&template_name) == Some(&index))
                .then(|| mono.generated.remove(&template_name))
                .flatten();
            // A comptime-class template either specialized or is a dead
            // generic, dropped either way. A bound-generic template always
            // survives, so its body keeps the abstract pre-check whether or
            // not it was instantiated: what a parametric body demands of its
            // own parameters is a fact about the template, not about the
            // arguments some call happened to supply. A retained template
            // precedes its specializations: a clone may still reference the
            // template abstractly (an inferred recursive call), and the
            // checker binds top-level names sequentially.
            //
            // Which stub a retained declaration stands as is its own class's
            // to say, not its name's: a mixed family holds two classes at one
            // name, and retention is recorded per name, so a sibling of the
            // deferred call's class would otherwise be stubbed under it.
            let retained = mono.retained.contains(&template_name);
            if self.bound_generics.contains(&template_name) {
                out.push(stmt);
            } else if retained
                && self.pack_generics.contains(&template_name)
                && pack_keyed_declaration(&stmt)
            {
                // A type-pack template with a deferred call survives as a
                // signature-only stub: the discovery check types the call
                // against it and records the instantiation the next round
                // mints; its body only specializes concretely.
                out.push(template_stub(&stmt, "unspecialized type-pack function"));
            } else if retained
                && self.comptime_generics.contains(&template_name)
                && comptime_keyed_declaration(&stmt, &self.scalar_reads)
            {
                // A compile-time-keyed template with a deferred call stands
                // in the same way: until the checker's request is served, or
                // for good under a call from an abstract generic body.
                out.push(template_stub(
                    &stmt,
                    "unspecialized compile-time-keyed function",
                ));
            }
            if let Some(mut specs) = generated {
                specs.reverse();
                out.extend(specs);
            }
        }
        let unserved_template_uses = self.unserved_template_uses(
            &mono.abstract_uses,
            &mono.method_edges,
            &mono.unclonable_methods,
        );
        Ok(Elaborated {
            program: out,
            instances: mono.minted_instances,
            stub_reaching_structs: stub_reaching
                .iter()
                .filter_map(|body| body.split_once('.'))
                .map(|(owner, _)| owner.to_string())
                .collect(),
            // Computed again after the drains: a generated clone's own
            // bodies are walked there.
            stub_reaching_methods: {
                let mut methods: Vec<(String, String)> = self
                    .stub_reaching_bodies(&mono.abstract_uses, &mono.method_edges)
                    .iter()
                    .filter(|body| owner_method(body).is_some())
                    .filter_map(|body| body.split_once('.'))
                    .map(|(owner, method)| (owner.to_string(), method.to_string()))
                    .collect();
                methods.sort();
                methods
            },
            unserved_template_uses,
            def_traces: Vec::new(),
            method_traces: Vec::new(),
            generated: super::GeneratedDeclarations::default(),
            ctfe_template_stats: mojito_checked::templates::TemplateStats::default(),
            clones: mojito_checked::census::CloneCensus::default(),
        })
    }

    /// Record the clone each checker-discovered inferred bound-generic
    /// application selects, for `mono_expr` to consult at that occurrence.
    ///
    /// Seeding only records the target; the Job queues lazily at the consult,
    /// so a drifted request produces no dead clone and a never-matched
    /// request leaves its template correctly retained. The compiler already
    /// resolved occurrence conflicts, so a duplicate here keeps the first
    /// target (defensive).
    fn seed_def_call_targets(&self, def_requests: &[DefSpecializationRequest], mono: &mut Mono) {
        for request in def_requests {
            let callee = request.callee();
            // A scalar-range request names a range-family struct, a generator
            // the template serves, and its dtype: the call becomes that
            // struct's construction at the dtype (`_ZeroStartingRange[dt](…)`).
            if self.struct_names.contains(callee)
                && let [TyArg::Val(value @ CtValue::Dtype(_))] = request.arguments()
            {
                mono.struct_call_targets
                    .entry(request.occurrence().clone().without_syntax())
                    .or_insert_with(|| (callee.to_string(), vec![value.clone()]));
                continue;
            }
            if !self.bound_generics.contains(callee)
                && !self.pack_generics.contains(callee)
                && !self.comptime_generics.contains(callee)
            {
                continue;
            }
            // An overloaded template name is a family: the request's
            // parameter names say which declaration the checker selected, and
            // a request that names none of them (or two of them) is skipped so
            // the call stays abstract.
            let (decl, template) = if self.overload_family(callee) {
                match self.family_declaration(callee, request) {
                    Some((index, declaration)) => (Some(index), declaration),
                    None => continue,
                }
            } else {
                match self.specializable.get(callee) {
                    Some(template) => (None, *template),
                    None => continue,
                }
            };
            let Some(vals) = self.def_request_values(template, request.arguments()) else {
                continue;
            };
            mono.def_call_targets
                .entry(request.occurrence().clone())
                .or_insert_with(|| DefCallTarget {
                    template: callee.to_string(),
                    decl,
                    vals,
                });
        }
    }

    /// Record the declaration each checker-recorded unclosed call of an
    /// overload family selected, for `mono_expr` to serve the call by when
    /// the template serves that declaration. A selection no declaration
    /// answers leaves the call abstract.
    fn seed_family_selections(&self, selections: &[DefSpecializationRequest], mono: &mut Mono) {
        for request in selections {
            let callee = request.callee();
            if !self.overload_family(callee) {
                continue;
            }
            let Some((index, _)) = self.family_declaration(callee, request) else {
                continue;
            };
            mono.family_selections
                .entry(request.occurrence().clone().without_syntax())
                .or_insert_with(|| (callee.to_string(), index));
        }
    }

    /// Mint one closed instance's method clones onto its template, and
    /// record the stub-reaching methods it could not serve.
    ///
    /// An instance whose clones cannot be minted at all keeps the erased path
    /// for every method; one that withholds a method as unavailable is not
    /// failing to serve it, because no call can reach that body either.
    fn mint_instance_clones(
        &self,
        program: &mut [Stmt],
        template: &str,
        values: &[CtValue],
        consts: &HashMap<String, CtValue>,
        stub_reaching: &HashSet<String>,
        mono: &mut Mono,
    ) -> Result<(), ComptimeError> {
        let Some(statement) = program.iter_mut().find(|statement| {
            matches!(&statement.kind, StmtKind::Struct { name, .. } if name == template)
        }) else {
            return Ok(());
        };
        let module = statement.module.clone();
        let StmtKind::Struct { methods, .. } = &mut statement.kind else {
            return Ok(());
        };
        let owed = owed_instance_clones(methods, template, values, stub_reaching);
        let mut keyed = keyed_methods(methods, template, stub_reaching);
        keyed.extend(
            self.keyed_methods
                .iter()
                .filter(|(owner, _)| owner == template)
                .map(|(_, method)| method.clone()),
        );
        let constructors = methods
            .iter()
            .filter(|method| {
                method.self_ty.is_none()
                    && mojito_symbol::symbol::lifecycle_method_name(method) == "__init__"
            })
            .count();
        let stubbed = methods
            .iter()
            .filter(|method| method.self_ty.is_none() && is_unspecialized_method_stub(&method.body))
            .map(|method| method.name.clone())
            .collect();
        let InstanceClones {
            clones,
            mut field_types,
            withheld,
            stubbed,
        } = self.generate_instance_clones(template, values, &keyed, &stubbed)?;
        mono.minted_instances.push(StructInstanceRequest::new(
            template.to_string(),
            values
                .iter()
                .map(|value| match value {
                    CtValue::Type(ty) => TyArg::Ty((**ty).clone()),
                    other => TyArg::Val(other.clone()),
                })
                .collect(),
        ));
        for ty in &mut field_types {
            self.mono_type(ty, consts, mono)?;
        }
        let mut kept =
            self.walk_instance_clones(clones, template, module.as_deref(), &stubbed, consts, mono);
        // A constructor family clones as a unit or not at all. The checker
        // names the member it selected by matching the substituted signature,
        // so a family that lost one member would let that constructor's call
        // reach a sibling's clone. A body that fails to walk is dropped here,
        // after `generate_instance_clones` has returned, so the count is taken
        // on what survived.
        let constructor_clone = mangle("__init__", values)?;
        if kept
            .iter()
            .filter(|clone| clone.name == constructor_clone)
            .count()
            != constructors
        {
            kept.retain(|clone| clone.name != constructor_clone);
        }
        for (method, clone) in owed {
            if !withheld.contains(&method) && !kept.iter().any(|minted| minted.name == clone) {
                mono.unclonable_methods
                    .push(super::method_owner(template, &method));
            }
        }
        methods.extend(kept);
        Ok(())
    }

    /// Tag the body of every per-call clone minted on a non-generic struct
    /// during elaboration.
    ///
    /// Those clones reuse their template's spans, and are tagged here rather
    /// than where they are minted because `Elab::block` re-stamps the whole
    /// statement with its module immediately afterwards.
    fn stamp_per_call_clone_bodies(&self, program: &mut [Stmt]) {
        let per_call_clones = self.per_call_clones.borrow();
        for statement in program {
            let module = statement.module.clone();
            let StmtKind::Struct { name, methods, .. } = &mut statement.kind else {
                continue;
            };
            for method in methods.iter_mut() {
                if per_call_clones.contains(&(name.clone(), method.name.clone())) {
                    let tag = super::clone_source_tag(module.as_deref(), name, &method.name);
                    mojito_ast::ast::stamp_source(&mut method.body, &tag);
                }
            }
        }
    }

    /// Walk each minted clone of one instance, dropping any whose own
    /// applications do not resolve: that call keeps the erased template
    /// rather than failing the program. A clone of a template whose body is
    /// the trap stub (`stubbed`, clone name to template method) has no
    /// erased body to fall back to, so it keeps its signature and reports
    /// the failure where a reachable call instantiates it.
    ///
    /// Every clone is stamped with its own source tag first, so the walk's
    /// span-keyed lookups find the checker's records for this instantiation
    /// rather than the template's. A clone that kept its own type parameters
    /// is still an erased body — every concrete call reaches a per-call clone
    /// of it — so it owns the references it leaves abstract.
    fn walk_instance_clones(
        &self,
        clones: Vec<Method>,
        template: &str,
        module: Option<&str>,
        stubbed: &HashMap<String, String>,
        consts: &HashMap<String, CtValue>,
        mono: &mut Mono,
    ) -> Vec<Method> {
        let mut kept = Vec::with_capacity(clones.len());
        for mut clone in clones {
            let tag = super::clone_source_tag(module, template, &clone.name);
            mojito_ast::ast::stamp_source(&mut clone.body, &tag);
            mono.abstract_owner =
                (!clone.type_params.is_empty()).then(|| super::method_owner(template, &clone.name));
            let walked = self.mono_method(&mut clone, consts, mono);
            mono.abstract_owner = None;
            match (walked, stubbed.get(&clone.name)) {
                (Ok(()), _) => kept.push(clone),
                (Err(error), Some(method)) => {
                    clone.body = vec![instantiation_failure_stub(template, method, &clone, &error)];
                    mojito_ast::ast::stamp_source(&mut clone.body, &tag);
                    kept.push(clone);
                }
                (Err(_), None) => {}
            }
        }
        kept
    }

    /// The bodies that can run a compile-time-keyed stub: the templates
    /// themselves, plus every abstract body that reaches one — a
    /// bound-generic `def` referencing such a template, or a struct method
    /// (`Struct.method`) whose erased body does, transitively through both
    /// kinds of reference and through the by-name method edges.
    fn stub_reaching_bodies<'a>(
        &'a self,
        uses: &'a [AbstractUse],
        edges: &'a [(String, String)],
    ) -> HashSet<&'a str> {
        let per_call_stubs = self.per_call_stubs.get().into_iter().flatten();
        let mut stubbed: HashSet<&str> = self
            .comptime_generics
            .iter()
            .map(String::as_str)
            .chain(per_call_stubs.map(String::as_str))
            .collect();
        loop {
            let reached: Vec<&str> = uses
                .iter()
                .filter(|reference| stubbed.contains(reference.callee.as_str()))
                .filter_map(|reference| reference.owner.as_deref())
                .chain(edges.iter().filter_map(|(owner, method)| {
                    stubbed
                        .iter()
                        .any(|body| owner_method(body) == Some(method.as_str()))
                        .then_some(owner.as_str())
                }))
                .filter(|owner| !stubbed.contains(owner))
                .collect();
            if reached.is_empty() {
                break;
            }
            stubbed.extend(reached);
        }
        stubbed
    }

    /// Whether the template of the `def` `name` serves every closed call of
    /// it, so no call mints a clone: the `def` is a plain trait-bound one
    /// whose compile-time parameters are all served binders, and its body
    /// neither holds a construct only an instance lowers (by its checked
    /// types as the driver read them) nor reaches a compile-time-keyed stub.
    /// A nested `def` or lambda is no such construct: its body is a generator
    /// over this one's binders, instantiated per instance below MIR. The
    /// elaborator instantiates the template's MIR for each call, and a call's
    /// transfer summary names its loans by the stored type. The verdict is
    /// the first one made for the name, so every call of it agrees.
    pub(super) fn template_serves_def(&self, name: &str, template: &Stmt, mono: &Mono) -> bool {
        if let Some(served) = self.template_served_defs.borrow().get(name) {
            return *served;
        }
        let StmtKind::Def { type_params, .. } = &template.kind else {
            return false;
        };
        let served = self.bound_generics.contains(name)
            && template_serves_binders(type_params, name)
            && !self
                .keyed_methods
                .contains(&(name.to_string(), String::new()))
            && !self
                .stub_reaching_bodies(&mono.abstract_uses, &mono.method_edges)
                .contains(name);
        self.template_served_defs
            .borrow_mut()
            .insert(name.to_string(), served);
        served
    }

    /// Whether the template of the non-generic struct `owner`'s method
    /// serves every call of it, so no call mints a per-call clone: its
    /// elaborated body is no trap stub, and is not keyed by what its checked
    /// body holds or reaches
    /// (the driver's `keyed_methods`, a compile-time-keyed stub among them).
    /// The elaborator instantiates its MIR per call, the method's own
    /// binders, a type pack among them, bound from the call.
    pub(super) fn template_serves_method(&self, owner: &str, method: &Method) -> bool {
        !is_unspecialized_method_stub(&method.body)
            && !self
                .keyed_methods
                .contains(&(owner.to_string(), method.name.clone()))
    }

    /// The abstract references that can run a compile-time-keyed stub.
    ///
    /// A reference made inside a stub-reaching body is dropped: that body
    /// runs only through a reference that is kept, or on one of the erased
    /// paths `stub_reaching_instance_uses` rejects.
    fn unserved_template_uses(
        &self,
        uses: &[AbstractUse],
        edges: &[(String, String)],
        unclonable: &[String],
    ) -> Vec<UnservedTemplateUse> {
        let stubbed = self.stub_reaching_bodies(uses, edges);
        // A method no instance could clone runs erased, and so does every
        // body it reaches: their references are unserved like a reference
        // from ordinary code.
        let mut erased: HashSet<&str> = stubbed
            .iter()
            .copied()
            .filter(|body| unclonable.iter().any(|method| method == body))
            .collect();
        loop {
            let reached: Vec<&str> = erased
                .iter()
                .flat_map(|body| body_callees(body, &stubbed, uses, edges))
                .filter(|body| !erased.contains(body))
                .collect();
            if reached.is_empty() {
                break;
            }
            erased.extend(reached);
        }
        let mut unserved: Vec<UnservedTemplateUse> = uses
            .iter()
            .filter(|reference| {
                stubbed.contains(reference.callee.as_str())
                    && reference
                        .owner
                        .as_deref()
                        .is_none_or(|owner| !stubbed.contains(owner) || erased.contains(owner))
            })
            .map(|reference| UnservedTemplateUse {
                callee: reference.callee.clone(),
                site: reference.site.clone(),
                function_value: reference.function_value,
            })
            .collect();
        unserved.sort_by(|left, right| {
            (&left.site.source, left.site.span, &left.callee).cmp(&(
                &right.site.source,
                right.site.span,
                &right.callee,
            ))
        });
        unserved.dedup();
        unserved
    }

    /// Generate each requested specialization, scanning its body for further
    /// (e.g. recursive) instantiations, until the specialization worklist is
    /// empty.
    fn drain_specialization_jobs(
        &self,
        mono: &mut Mono,
        consts: &HashMap<String, CtValue>,
    ) -> Result<(), ComptimeError> {
        while let Some(job) = mono.queue.pop_front() {
            self.burn().map_err(|_| {
                ComptimeError::NotComptime(format!(
                    "specialization quota exceeded while instantiating '{}' requested at {}; possible unbounded generic recursion",
                    mangle(&job.orig, &job.vals).unwrap_or_else(|_| job.orig.clone()),
                    job.site
                ))
            })?;
            let template = self.selected_declaration(&job.orig, job.decl);
            let mut spec =
                self.generate_def_spec(template, &job.orig, job.output_name.clone(), &job.vals)?;
            // A specialization of a bundled template is walked as bundled
            // code: the instances it reaches keep the erased path.
            mono.in_bundled =
                mojito_checker::checker::is_bundled_module_source(template.module.as_deref());
            // A specialization is walked whole — signature and body — for
            // further template uses (nested instantiations, recursive packs):
            // a def clone's expanded `-> Variant[Int, String]` requests that
            // concrete struct exactly as its body's calls do.
            self.mono_stmt(&mut spec, consts, mono)?;
            mono.in_bundled = false;
            // Scan while the parameter still carries its `$pack[T0, ...]`
            // identity: a whole-pack specialization may forward the collector
            // through another generic call. Select the regular Tuple ABI only
            // after all such calls have been rewritten.
            if job.whole_pack_abi {
                select_top_level_whole_pack_abi(&mut spec)?;
            }
            mono.generated.entry(job.orig).or_default().push(spec);
        }
        Ok(())
    }

    /// Declaration-based specialization core shared by top-level and lexical
    /// nested templates. `display_name` remains source-facing for diagnostics;
    /// `output_name` is the canonical, scope-qualified symbol selected by the
    /// caller.
    pub(super) fn generate_def_spec(
        &self,
        template: &Stmt,
        display_name: &str,
        output_name: String,
        vals: &[CtValue],
    ) -> Result<Stmt, ComptimeError> {
        let StmtKind::Def {
            decorators,
            type_params,
            params,
            positional_only,
            keyword_only,
            raises,
            raises_type,
            ret,
            body,
            ..
        } = &template.kind
        else {
            return Err(ComptimeError::NotComptime(format!(
                "specialization registry entry '{display_name}' is not a function"
            )));
        };
        let evaluated_count = type_params
            .iter()
            .filter(|parameter| !retained_specialization_param(parameter, type_params))
            .count();
        if evaluated_count != vals.len() {
            return Err(ComptimeError::Arity(format!(
                "'{display_name}' expects {} compile-time argument(s), got {}",
                evaluated_count,
                vals.len()
            )));
        }
        // Bind every parameter for comptime resolution; fold value parameters into
        // runtime literals (except where a regular parameter shadows the name); keep
        // type parameters on the specialized signature.
        let mut env = self.top_consts.borrow().clone();
        let mut subs = self.top_consts.borrow().clone();
        for p in params {
            subs.remove(&p.name);
        }
        // The declaration's own compile-time parameters shadow same-named
        // module constants everywhere in the clone. The body is materialized
        // as a bare statement list (no `Def` wrapper to shadow through), so a
        // kept-symbolic type parameter must not be replaced by an unrelated
        // outer constant; evaluated value parameters re-enter `subs` with
        // their concrete arguments below.
        for tp in type_params {
            subs.remove(tp.name.trim_start_matches('*'));
        }
        let mut kept_type_params = Vec::new();
        let mut type_substitutions: HashMap<String, Type> = HashMap::new();
        let mut specialized_params = params.clone();
        let mut type_pack_expansions: HashMap<String, Vec<Type>> = HashMap::new();
        let mut type_pack_values: HashMap<String, Vec<CtValue>> = HashMap::new();
        let mut values = vals.iter();
        for tp in type_params {
            let Some(decl) = classify_ct_param(tp, type_params, &output_name) else {
                // Origin/OriginSet binders and explicit callable-value
                // parameters remain symbolic. Their arguments are retained at
                // each rewritten call and therefore never enter `CtValue`.
                kept_type_params.push(tp.clone());
                continue;
            };
            let v = values
                .next()
                .expect("evaluated parameter count checked above");
            let binding = decl.name().trim_start_matches('*').to_string();
            if decl.name().starts_with('*') {
                env.insert(
                    super::elab::pack_binding_marker(&binding),
                    CtValue::Bool(true),
                );
            }
            env.insert(binding.clone(), v.clone());
            match &decl {
                ParamDecl::Value { name, .. } => {
                    subs.insert(name.trim_start_matches('*').to_string(), v.clone());
                }
                ParamDecl::Type { variadic: true, .. } => {
                    let CtValue::Tuple(types) = v else {
                        return Err(ComptimeError::NotComptime(
                            "a type pack specialization requires a tuple of types".to_string(),
                        ));
                    };
                    let source_types = types
                        .iter()
                        .map(|value| match value {
                            CtValue::Type(ty) => self.pack_element_source_type(ty),
                            _ => None,
                        })
                        .collect::<Option<Vec<_>>>()
                        .ok_or_else(|| {
                            ComptimeError::NotComptime(
                                "type pack contains a non-type value".to_string(),
                            )
                        })?;
                    type_pack_expansions.insert(binding.clone(), source_types.clone());
                    type_pack_values.insert(binding.clone(), types.clone());
                    for parameter in &mut specialized_params {
                        if matches!(&parameter.ty, Type::Named(name, _) if name.trim_start_matches('*') == decl.name().trim_start_matches('*'))
                        {
                            parameter.ty = Type::Named(
                                "$pack".to_string(),
                                source_types.iter().cloned().map(ParamArg::Type).collect(),
                            );
                        }
                    }
                }
                ParamDecl::Type { .. } => match spec_type_param_substitution(&decl, v) {
                    // A concrete type argument is baked into the clone rather
                    // than kept on the residual signature, so the clone checks
                    // concretely. `resolve_spec_args_for` makes the matching
                    // decision for the rewritten call's arguments.
                    Some(concrete) => {
                        type_substitutions.insert(binding.clone(), concrete);
                    }
                    None => kept_type_params.push(tp.clone()),
                },
            }
        }
        let leftover = values.next();
        debug_assert!(leftover.is_none());
        // A variadic type-pack specialization also exposes its sequence of
        // element types through the runtime `*args` parameter during compile-time
        // elaboration. This makes `len(args)` and `args[i]` evaluable while a
        // `comptime for` body is being unrolled.
        for pack_param in params {
            let Type::Named(pack_name, _) = &pack_param.ty else {
                continue;
            };
            let Some(types) = type_pack_values.get(pack_name.trim_start_matches('*')) else {
                continue;
            };
            env.insert(pack_param.name.clone(), CtValue::Tuple(types.clone()));
        }
        let constraint_env = env.clone();
        // Elaborate the body with the parameters bound, so its comptime constructs
        // select/unroll against the concrete arguments.
        let elaborated = self.block(body, &mut env, true)?;
        let mut final_body = materialize_block(
            elaborated,
            &subs,
            &self.struct_names,
            &self.applied_constants(),
        );
        fold_pack_uses(&mut final_body, &type_pack_values);
        for parameter in &mut specialized_params {
            if let Some(default) = &mut parameter.default {
                *default = materialize_expression(default, &subs);
            }
        }
        // Retained origin mutability and callable defaults may depend on an
        // earlier scalar value parameter that has just been baked out of the
        // signature. Keep their source declarations self-contained.
        for parameter in &mut kept_type_params {
            if let Some(mutability) = &mut parameter.origin_mutability {
                *mutability = materialize_expression(mutability, &subs);
            }
            if let Some(default) = &mut parameter.default {
                *default = materialize_expression(default, &subs);
            }
        }
        let mut specialized_decorators = decorators.clone();
        for decorator in &mut specialized_decorators {
            for argument in &mut decorator.args {
                *argument = materialize_expression(argument, &subs);
            }
            for argument in &mut decorator.kwargs {
                argument.value = materialize_expression(&argument.value, &subs);
            }
        }
        let mut specialized_where = match &template.kind {
            StmtKind::Def { where_clauses, .. } => where_clauses
                .iter()
                .map(|predicate| materialize_expression(predicate, &subs))
                .collect(),
            _ => Vec::new(),
        };
        // Once every compile-time binder has been baked into a clone, its
        // trailing predicates are specialization preconditions rather than
        // residual declaration constraints. Prove each now (retaining an
        // optional per-clause diagnostic message), then erase them so the
        // concrete clone does not pretend to have a parameter to which the
        // constraints can attach.
        let has_residual_constraint_binder = kept_type_params.iter().any(|parameter| {
            !matches!(parameter.bounds.as_slice(), [only] if only == "Origin" || only == "OriginSet")
                && !parameter.is_origin_mutability_binder(type_params)
        });
        if !has_residual_constraint_binder
            && let StmtKind::Def { where_clauses, .. } = &template.kind
        {
            for predicate in where_clauses {
                self.validate_specialized_where(predicate, &constraint_env, display_name)?;
            }
            specialized_where = Vec::new();
        }
        expand_pack_spreads_in_function_body(
            &mut final_body,
            &specialized_params,
            &type_pack_expansions,
            &self.served_packs,
        );
        let mut specialized_ret = ret.clone();
        if let Some(ret) = &mut specialized_ret {
            expand_type_packs(ret, &type_pack_expansions);
        }
        for parameter in &mut specialized_params {
            expand_type_packs(&mut parameter.ty, &type_pack_expansions);
        }
        let mut specialized_raises_type = raises_type.clone();
        // A scalar value parameter may appear inside a **type** position (a
        // SIMD width, `-> SIMD[DType.int32, w]`); bake it into the signature
        // exactly like the body/default/where expressions, so the clone's
        // types resolve concretely — `simd_width` then validates the bound
        // width during this checked elaboration.
        let value_subs: Subs = &|name| subs.get(name).cloned();
        if let Some(ret) = &mut specialized_ret {
            rewrite_type(ret, value_subs);
        }
        for parameter in &mut specialized_params {
            rewrite_type(&mut parameter.ty, value_subs);
        }
        if let Some(error) = &mut specialized_raises_type {
            rewrite_type(error, value_subs);
        }
        // Bake each dropped type parameter's concrete type into every remaining
        // type position: the residual signature no longer declares the binding
        // and the rewritten calls no longer supply it.
        if !type_substitutions.is_empty() {
            for parameter in &mut specialized_params {
                substitute_type_bindings_in_type(&mut parameter.ty, &type_substitutions);
                if let Some(default) = &mut parameter.default {
                    substitute_type_bindings_in_expr(default, &type_substitutions);
                }
            }
            // A retained binder's dependent callable bound (`F: def(T) -> T`)
            // or value type may reference a just-baked sibling type parameter;
            // rewrite them so the residual signature stays self-contained.
            for parameter in &mut kept_type_params {
                if let Some(bound) = &mut parameter.callable_bound {
                    substitute_type_bindings_in_type(bound, &type_substitutions);
                }
                if let Some(value_type) = &mut parameter.value_type {
                    substitute_type_bindings_in_type(value_type, &type_substitutions);
                }
            }
            if let Some(ret) = &mut specialized_ret {
                substitute_type_bindings_in_type(ret, &type_substitutions);
            }
            if let Some(error) = &mut specialized_raises_type {
                substitute_type_bindings_in_type(error, &type_substitutions);
            }
            for predicate in &mut specialized_where {
                substitute_type_bindings_in_expr(predicate, &type_substitutions);
            }
            substitute_type_bindings_in_block(&mut final_body, &type_substitutions);
        }
        let residual_names: Vec<String> = kept_type_params
            .iter()
            .map(|parameter| parameter.name.clone())
            .collect();
        // A loan-carrying type argument's origin slots spell the clone's own
        // binders (`Span[Int, __clone_origin0]`), which it declares first.
        kept_type_params.splice(
            0..0,
            self.clone_origin_binder_params(vals, &self.unspelled_clone_binders(template, vals)),
        );
        let mut specialization = mk(
            StmtKind::Def {
                name: output_name.clone(),
                decorators: specialized_decorators,
                type_params: kept_type_params,
                params: specialized_params,
                positional_only: *positional_only,
                keyword_only: *keyword_only,
                captures: match &template.kind {
                    StmtKind::Def { captures, .. } => captures.clone(),
                    _ => None,
                },
                raises: *raises,
                raises_type: specialized_raises_type,
                ret: specialized_ret,
                where_clauses: specialized_where,
                body: final_body,
            },
            template.span,
        );
        // Declaration facts are keyed by source identity plus span. Cloned
        // specializations share the template span, so give each concrete
        // function its own synthetic source before checking/HIR lowering.
        let tag = match &template.module {
            Some(module) => format!("{module}${output_name}"),
            None => output_name.clone(),
        };
        mojito_ast::ast::stamp_source(std::slice::from_mut(&mut specialization), &tag);
        let mut type_bindings: Vec<_> = type_substitutions.into_iter().collect();
        type_bindings.sort_by(|left, right| left.0.cmp(&right.0));
        let mut pack_bindings: Vec<_> = type_pack_expansions.into_iter().collect();
        pack_bindings.sort_by(|left, right| left.0.cmp(&right.0));
        self.generated.borrow_mut().defs.push(output_name.clone());
        self.def_traces.borrow_mut().push(super::DefInstanceTrace {
            clone_module: tag,
            clone_name: output_name,
            template_module: template.module.clone(),
            template_name: match &template.kind {
                StmtKind::Def { name, .. } => name.clone(),
                _ => display_name.to_string(),
            },
            template_span: template.span,
            type_bindings,
            value_bindings: type_params
                .iter()
                .filter_map(|parameter| {
                    let name = parameter.name.trim_start_matches('*');
                    subs.get(name)
                        .map(|value| (name.to_string(), value.clone()))
                })
                .collect(),
            pack_bindings,
            residual: residual_names,
        });
        Ok(specialization)
    }

    fn validate_specialized_where(
        &self,
        predicate: &Expr,
        environment: &HashMap<String, CtValue>,
        display_name: &str,
    ) -> Result<(), ComptimeError> {
        let (condition, message) = match &predicate.kind {
            ExprKind::TupleLit(elements) => {
                let [condition, message] = elements.as_slice() else {
                    return Err(ComptimeError::NotComptime(
                        "a diagnostic where clause must be `(condition, \"message\")`".to_string(),
                    ));
                };
                let ExprKind::Str(message) = &message.kind else {
                    return Err(ComptimeError::NotComptime(
                        "a where-clause diagnostic message must be a string literal".to_string(),
                    ));
                };
                (condition, Some(message.as_str()))
            }
            _ => (predicate, None),
        };
        if self
            .eval(condition, environment)?
            .as_bool("specialized where clause")?
        {
            return Ok(());
        }
        Err(ComptimeError::Constraint(message.map_or_else(
            || {
                format!(
                    "'{display_name}': {}",
                    super::unparse::violated_constraint_message(condition)
                )
            },
            str::to_string,
        )))
    }

    /// Fold the pack-valued `conforms_to(Ts.values, Trait)` atoms used by
    /// conditional conformances and method availability. Boolean structure is
    /// simplified while unrelated method-generic propositions are retained.
    /// The specialization values and type bindings a checker-recorded
    /// method instantiation selects, aligned with the checker's
    /// `specialized_method_values`: one value per baked parameter in
    /// declaration order (type arguments as `CtValue::Type`, value
    /// arguments as themselves); callable-bounded and retained
    /// callable-value parameters stay symbolic; Origin binders have no
    /// checker slot. `None` skips the request. A type argument mentioning an
    /// origin-slotted struct spells its slots as fresh `origin_binders`
    /// (see `Elab::clone_binding`).
    pub(super) fn method_request_values(
        &self,
        owner: &str,
        type_params: &[TypeParam],
        arguments: &[TyArg],
        origin_binders: &mut CloneOriginBinders,
    ) -> Option<(Vec<CtValue>, Vec<MethodBinding>)> {
        let mut values = Vec::new();
        let mut bindings = Vec::new();
        // The checker's origin tail has no elaborator slot: origins erase from
        // every clone.
        let mut cursor = arguments
            .iter()
            .filter(|argument| !matches!(argument, TyArg::Origin(_)));
        for parameter in type_params {
            if parameter.is_erased_origin_param(type_params) {
                continue;
            }
            let argument = cursor.next()?;
            if !method_parameter_is_baked(parameter, type_params) {
                continue;
            }
            let name = parameter.name.trim_start_matches('*').to_string();
            let decl = classify_ct_param(parameter, type_params, owner)?;
            match (&decl, argument) {
                (
                    ParamDecl::Type {
                        variadic: false,
                        bounds,
                        ..
                    },
                    TyArg::Ty(ty),
                ) => {
                    if bounds
                        .iter()
                        .any(|bound| self.conformance.require(ty, bound).is_err())
                    {
                        return None;
                    }
                    let (bound, source) = self.clone_binding(ty, origin_binders)?;
                    values.push(CtValue::Type(Box::new(ty.clone())));
                    bindings.push(MethodBinding {
                        name,
                        value: CtValue::Type(Box::new(bound)),
                        source: Some(source),
                    });
                }
                (
                    ParamDecl::Value {
                        variadic: false,
                        ty,
                        ..
                    },
                    TyArg::Val(value),
                ) => {
                    if matches!(
                        value,
                        CtValue::Expr(_) | CtValue::Deferred(_) | CtValue::Marker(_)
                    ) || !ct_value_has_type(value, ty)
                    {
                        return None;
                    }
                    values.push(value.clone());
                    bindings.push(MethodBinding {
                        name,
                        value: value.clone(),
                        source: None,
                    });
                }
                // A method-level type pack inferred from the call's overflow
                // arguments: every element must be a type meeting the bound;
                // the clone expands `*args: *Ts` to the element list.
                (
                    ParamDecl::Type {
                        variadic: true,
                        bounds,
                        ..
                    },
                    TyArg::Val(value @ CtValue::Tuple(elements)),
                ) => {
                    for element in elements {
                        let CtValue::Type(ty) = element else {
                            return None;
                        };
                        // A generated specialization (`TypeNames[Int]`) is not
                        // in the elaboration-start oracle; the checker proved
                        // its bound at the requesting call.
                        let known = match ty.as_ref() {
                            Ty::Struct(name, _) => self.struct_names.contains(name),
                            _ => true,
                        };
                        if known
                            && bounds
                                .iter()
                                .any(|bound| self.conformance.require(ty, bound).is_err())
                        {
                            return None;
                        }
                    }
                    values.push(value.clone());
                    bindings.push(MethodBinding {
                        name,
                        value: value.clone(),
                        source: None,
                    });
                }
                _ => return None,
            }
        }
        if cursor.next().is_some() {
            return None;
        }
        // Nothing baked (only callable-bounded parameters) would mangle to
        // the template's own name: no clone to mint.
        if values.is_empty() {
            return None;
        }
        Some((values, bindings))
    }

    /// Per-instantiation method clones of an ordinary generic struct: for
    /// each checker-discovered closed application (`Optional[Int]`), every
    /// `keyed` method whose `where` clause holds for the instance is cloned
    /// from the original template with the struct's parameters baked
    /// (`kind$y3:Int`) and an explicit receiver type, so the checker binds
    /// `self`/`Self` to the instance. Every other method's template serves
    /// the instance, so it mints no clone. A user template's
    /// lifecycle methods clone like any other, a constructor family as one
    /// overload set under the shared clone name; a bundled template's stay
    /// erased, carrying the value-parameter reification that path relies on. Only a template whose
    /// parameters are all plain type or value parameters specializes here
    /// (a value argument baked as itself, `f$i2;` on `S[2]`); packs,
    /// retained origin binders, and callable-bounded parameters keep the
    /// erased path. A method in `stubbed`, whose template body is the trap
    /// stub, has no erased path: a clone of one that fails to elaborate is
    /// minted anyway, its body reporting the failure.
    #[allow(
        clippy::unnecessary_wraps,
        reason = "TODO: drop the Result once callers stop using ?"
    )]
    fn generate_instance_clones(
        &self,
        name: &str,
        values: &[CtValue],
        keyed: &HashSet<String>,
        stubbed: &HashSet<String>,
    ) -> Result<InstanceClones, ComptimeError> {
        let Some(template) = self.program.iter().find(|statement| {
            matches!(&statement.kind, StmtKind::Struct { name: template, .. } if template == name)
        }) else {
            return Ok(InstanceClones::default());
        };
        let StmtKind::Struct {
            type_params,
            fields,
            methods,
            conformance_conditions,
            ..
        } = &template.kind
        else {
            return Ok(InstanceClones::default());
        };
        // The compile-time string keeps the literal runtime representation and
        // materializes `String` at `var` bindings, so a clone body would not
        // type against its own `Self.T`: such an instance keeps the erased
        // path (`symbol::specialized_method_values` names it no clone either).
        if values.iter().any(|value| {
            matches!(value, CtValue::Type(ty)
                if mojito_types::types::contains_string_literal(ty))
        }) {
            return Ok(InstanceClones::default());
        }
        // Bind each parameter; an instance whose argument violates a declared
        // bound, or does not round-trip to source syntax, keeps the erased path.
        let mut bindings = Vec::new();
        let mut receiver_arguments = Vec::new();
        let mut origin_binders = CloneOriginBinders::default();
        for (parameter, value) in type_params.iter().zip(values) {
            let CtValue::Type(ty) = value else {
                // A value argument is baked as itself, as a generic `def`'s
                // clone bakes it (`f$y2:2` on `S[2]`).
                let Some(ParamDecl::Value {
                    variadic: false,
                    ty,
                    ..
                }) = classify_ct_param(parameter, type_params, name)
                else {
                    return Ok(InstanceClones::default());
                };
                let Some(spelled) = (!matches!(
                    value,
                    CtValue::Expr(_) | CtValue::Deferred(_) | CtValue::Marker(_)
                ) && ct_value_has_type(value, &ty))
                .then(|| value.materialize(template.span))
                .flatten() else {
                    return Ok(InstanceClones::default());
                };
                receiver_arguments.push(ParamArg::Value(spelled));
                bindings.push(MethodBinding {
                    name: parameter.name.clone(),
                    value: value.clone(),
                    source: None,
                });
                continue;
            };
            if parameter
                .bounds
                .iter()
                .any(|bound| self.conformance.require(ty, bound).is_err())
            {
                return Ok(InstanceClones::default());
            }
            let Some((bound, source)) = self.clone_binding(ty, &mut origin_binders) else {
                return Ok(InstanceClones::default());
            };
            receiver_arguments.push(ParamArg::Type(source.clone()));
            bindings.push(MethodBinding {
                name: parameter.name.clone(),
                value: CtValue::Type(Box::new(bound)),
                source: Some(source),
            });
        }
        if bindings.len() != type_params.len() {
            return Ok(InstanceClones::default());
        }
        // The instance's storage types, with the parameters baked: closed
        // applications there (`List[DictEntry[String, Int]]`) are instances
        // the clones reach through `self`, minted in this elaboration too.
        let type_bindings: HashMap<String, Type> = bindings
            .iter()
            .filter_map(|binding| Some((binding.name.clone(), binding.source.clone()?)))
            .collect();
        let field_types = fields
            .iter()
            .map(|field| {
                let mut ty = field.ty.clone();
                substitute_type_bindings_in_type(&mut ty, &type_bindings);
                ty
            })
            .collect();
        let receiver = Type::Named(name.to_string(), receiver_arguments);
        let consts = self.top_consts.borrow().clone();
        let mut env = consts.clone();
        for binding in &bindings {
            env.insert(binding.name.clone(), binding.value.clone());
        }
        // A conditional conformance that is false for this instance
        // (`Iterator where conforms_to(T, Movable)` on a pinned element type)
        // withholds the trait's requirements: their bodies rely on the
        // condition, and Mojo instantiates them only through the conformance.
        let mut unavailable = HashSet::new();
        for (trait_name, predicate) in conformance_conditions {
            if !matches!(self.eval(predicate, &env), Ok(CtValue::Bool(true))) {
                unavailable.extend(self.trait_requirement_names(trait_name));
            }
        }
        // Checker-discovered instantiations of this instance's generic
        // methods (`b.kind[Bool]()` on `Box[Int]`) and generic constructors
        // (`Box[Int](s)`) mint per-call clones with the instance's values
        // baked before the call's (`kind$y3:Int$y4:Bool`); the request owner
        // is the instance key.
        let instance_key = mangle(name, values)?;
        let per_call_requests = self
            .method_requests
            .get(&instance_key)
            .map_or(&[][..], Vec::as_slice);
        let bundled = mojito_checker::checker::is_bundled_module_source(template.module.as_deref());
        // The methods this instance withholds rather than fails to clone: an
        // unavailable method cannot be called on it at all, so its erased body
        // never runs.
        let mut withheld: HashSet<String> = HashSet::new();
        let mut clones = Vec::new();
        let mut stubbed_clones = HashMap::new();
        for method in methods {
            if unavailable.contains(&method.name) {
                withheld.insert(method.name.clone());
                continue;
            }
            // A bundled template keeps its lifecycle methods on the erased
            // path: its constructors carry the value-parameter reification
            // the erased path relies on.
            let lifecycle = mojito_symbol::symbol::lifecycle_method_name(method);
            if bundled && matches!(lifecycle, "__init__" | "__copyinit__" | "__moveinit__") {
                continue;
            }
            // A method the template serves is instantiated per call by the
            // elaborator from its MIR, its own binders bound with the
            // struct's.
            let served = !keyed.contains(&method.name);
            let stub = stubbed.contains(&method.name);
            let per_call = if served {
                Vec::new()
            } else {
                self.per_call_method_clones(
                    name,
                    method,
                    stub,
                    per_call_requests,
                    &PerCallBase {
                        values,
                        bindings: &bindings,
                        receiver: Some(&receiver),
                        owner: Some(PerCallOwner {
                            name,
                            module: template.module.as_deref(),
                            template: name,
                        }),
                        origin_binders: Some(&origin_binders),
                        constructors: !bundled,
                    },
                    &consts,
                )
            };
            if stub {
                stubbed_clones.extend(
                    per_call
                        .iter()
                        .map(|clone| (clone.name.clone(), method.name.clone())),
                );
            }
            clones.extend(per_call);
            // A synthesized trait-default body (Copyable's `copy`, Hashable's
            // `__hash__`; no source provenance) has no instance-specific
            // behavior: the template's serves every instance.
            if method
                .body
                .iter()
                .all(|statement| statement.module.is_none())
            {
                continue;
            }
            // The template serves a method that holds no compile-time
            // construct, whatever the instance's arguments carry.
            if !keyed.contains(&method.name) {
                continue;
            }
            // Same-name overloads all clone: they share the mangled name and
            // stay an overload set on the clone side, constructors included —
            // the checker names the member it selected. A lifecycle method
            // clones under the name it is registered and dispatched by, so
            // `__init__(out self, *, copy: Self)` clones as
            // `__copyinit__$y3:Int`.
            let clone_name = mangle(lifecycle, values)?;
            // A `where` clause that is false (or cannot be evaluated) for this
            // instance leaves the method uncloned: the call reports the
            // template's availability failure as today.
            let available = method
                .where_clauses
                .iter()
                .all(|predicate| matches!(self.eval(predicate, &env), Ok(CtValue::Bool(true))));
            if !available {
                withheld.insert(method.name.clone());
                continue;
            }
            let specialized = self.specialize_method_clone(
                method,
                clone_name.clone(),
                &bindings,
                &consts,
                &consts,
            );
            if stub {
                stubbed_clones.insert(clone_name.clone(), method.name.clone());
            }
            let mut clone = match specialized {
                Ok(clone) => clone,
                Err(error) if stub => {
                    let Some(mut clone) = self
                        .failed_method_clone(name, method, clone_name, &bindings, &consts, &error)
                    else {
                        continue;
                    };
                    clone.where_clauses.clear();
                    clone.self_ty = Some(receiver.clone());
                    clones.push(clone);
                    continue;
                }
                Err(_) => continue,
            };
            clone.where_clauses.clear();
            clone.self_ty = Some(receiver.clone());
            // Its own lane still unbound, a vector construction lowers only
            // in the per-call clones: this one stands as the stub.
            if super::synth::constructs_at_own_lane(&clone) {
                clone.body = vec![unspecialized_method_stub(name, &clone)];
                clones.push(clone);
                continue;
            }
            let clone_body = clone.body.first().map(|first| first.span);
            if let (Some(first), Some(clone_body)) = (method.body.first(), clone_body) {
                self.method_traces
                    .borrow_mut()
                    .push(super::MethodInstanceTrace {
                        owner: name.to_string(),
                        template_owner: name.to_string(),
                        owner_module: template.module.clone(),
                        clone_module: super::clone_source_tag(
                            template.module.as_deref(),
                            name,
                            &clone.name,
                        ),
                        clone_name: clone.name.clone(),
                        template_name: method.name.clone(),
                        body: first.span,
                        clone_body,
                        type_bindings: bindings
                            .iter()
                            .filter_map(|binding| {
                                Some((binding.name.clone(), binding.source.clone()?))
                            })
                            .collect(),
                        value_bindings: Vec::new(),
                        pack_bindings: Vec::new(),
                        first_copy_template: false,
                    });
            }
            clones.push(clone);
        }
        // Overloads that differ only through the struct's parameters
        // (`pick(item: Self.T)` and `pick(count: Int)` on `Box[Int]`) collapse
        // to one shape on the instance; such a family keeps the erased path
        // rather than registering a redeclaration.
        let shape = |method: &Method| {
            (
                method.name.clone(),
                method.has_self,
                method.self_convention,
                // Overload identity counts the keyword names and the
                // positional/keyword boundaries too (`SignatureKey`), so two
                // keyword-only constructors (`capacity:` and
                // `unsafe_uninit_length:`) are distinct shapes here as well.
                method.positional_only,
                method.keyword_only,
                method
                    .params
                    .iter()
                    .enumerate()
                    .map(|(index, parameter)| {
                        let keyword = method
                            .keyword_only
                            .is_some_and(|first| index >= first)
                            .then(|| parameter.name.clone());
                        (
                            parameter.kind,
                            keyword,
                            mojito_symbol::symbol::TypeKey::from_ast(&parameter.ty),
                        )
                    })
                    .collect::<Vec<_>>(),
            )
        };
        let mut collapsed = HashSet::new();
        for (index, clone) in clones.iter().enumerate() {
            let this = shape(clone);
            if clones[..index].iter().any(|earlier| shape(earlier) == this) {
                collapsed.insert(clone.name.clone());
            }
        }
        clones.retain(|clone| !collapsed.contains(&clone.name));
        for clone in &mut clones {
            clone
                .type_params
                .splice(0..0, origin_binders.params().iter().cloned());
        }
        stubbed_clones.retain(|clone, _| clones.iter().any(|minted| minted.name == *clone));
        Ok(InstanceClones {
            clones,
            field_types,
            withheld,
            stubbed: stubbed_clones,
        })
    }

    /// Per-call clones of one generic method (`kind[U]`) for every
    /// checker-discovered instantiation in `requests` that selects it: the
    /// owner's values (`base_values`, an instance's baked arguments; empty
    /// for a non-generic struct) precede the call's in the clone name
    /// (`kind$y3:Int$y4:Bool`), `base_bindings` bind them for elaboration,
    /// and `receiver` is the instance clone's explicit receiver type. A
    /// method with no baked parameter, a request whose arguments do not
    /// align, a `where` clause false for the instantiation, or a body that
    /// fails to elaborate mints nothing: the call keeps the erased path.
    /// `template` is the struct whose declaration `method` is, which owns
    /// the method's binders. A `stubbed` method, whose template body is the
    /// trap stub, has no erased path: its clone that fails to elaborate is
    /// minted anyway, its body reporting the failure.
    pub(super) fn per_call_method_clones(
        &self,
        template: &str,
        method: &Method,
        stubbed: bool,
        requests: &[MethodSpecializationRequest],
        base: &PerCallBase<'_>,
        consts: &HashMap<String, CtValue>,
    ) -> Vec<Method> {
        let PerCallBase {
            values: base_values,
            bindings: base_bindings,
            receiver,
            owner,
            origin_binders: base_binders,
            constructors,
        } = *base;
        let specializable = method
            .type_params
            .iter()
            .any(|parameter| method_parameter_is_baked(parameter, &method.type_params));
        if !specializable || (method.name == "__init__" && !constructors) {
            return Vec::new();
        }
        let mut clones = Vec::new();
        let mut minted = HashSet::new();
        let binder_owner = self.method_binder_owners.owner(template, method);
        for request in requests
            .iter()
            .filter(|request| request.selects(method, template, &self.method_binder_owners))
        {
            // The call's own loan-carrying arguments bind their origin slots
            // to binders numbered after the instance's: both land on the
            // clone, which declares the instance's first.
            let mut binders = base_binders.cloned().unwrap_or_default();
            let Some((call_values, call_bindings)) = self.method_request_values(
                &binder_owner,
                &method.type_params,
                request.arguments(),
                &mut binders,
            ) else {
                continue;
            };
            let mut values = base_values.to_vec();
            values.extend(call_values);
            let Ok(clone_name) = mangle(&method.name, &values) else {
                continue;
            };
            if !minted.insert(clone_name.clone()) {
                continue;
            }
            let mut bindings = base_bindings.to_vec();
            bindings.extend(call_bindings);
            let mut env = consts.clone();
            for binding in &bindings {
                env.insert(binding.name.clone(), binding.value.clone());
            }
            let available = method
                .where_clauses
                .iter()
                .all(|predicate| matches!(self.eval(predicate, &env), Ok(CtValue::Bool(true))));
            if !available {
                continue;
            }
            let (mut clone, failed) = match self.specialize_method_clone(
                method,
                clone_name.clone(),
                &bindings,
                consts,
                consts,
            ) {
                Ok(clone) => (clone, false),
                Err(error) if stubbed => {
                    let Some(clone) = self.failed_method_clone(
                        template, method, clone_name, &bindings, consts, &error,
                    ) else {
                        continue;
                    };
                    (clone, true)
                }
                Err(_) => continue,
            };
            clone.where_clauses.clear();
            clone.self_ty = receiver.cloned();
            let first_own = base_binders.map_or(0, |base| base.params().len());
            clone
                .type_params
                .splice(0..0, binders.params()[first_own..].iter().cloned());
            if let (Some(owner), false) = (owner, failed) {
                self.trace_per_call_clone(owner, method, &clone, &bindings, base_bindings.len());
            }
            clones.push(clone);
        }
        clones
    }

    /// Record how a per-call clone came from its template: the struct's
    /// type bindings (an instance's, first) and the method's own, the
    /// method's folded values, and its expanded type packs.
    fn trace_per_call_clone(
        &self,
        owner: PerCallOwner<'_>,
        method: &Method,
        clone: &Method,
        bindings: &[MethodBinding],
        struct_bindings: usize,
    ) {
        let (Some(first), Some(clone_first)) = (method.body.first(), clone.body.first()) else {
            return;
        };
        let own = &bindings[struct_bindings..];
        let pack = |binding: &MethodBinding| {
            method.type_params.iter().any(|parameter| {
                parameter.name.strip_prefix('*') == Some(binding.name.as_str())
                    && parameter.value_type.is_none()
            })
        };
        let pack_bindings = own
            .iter()
            .filter(|binding| pack(binding))
            .filter_map(|binding| {
                let CtValue::Tuple(elements) = &binding.value else {
                    return None;
                };
                let sources = elements
                    .iter()
                    .map(|element| match element {
                        CtValue::Type(ty) => self.pack_element_source_type(ty),
                        _ => None,
                    })
                    .collect::<Option<Vec<_>>>()?;
                Some((binding.name.clone(), sources))
            })
            .collect();
        self.method_traces
            .borrow_mut()
            .push(super::MethodInstanceTrace {
                owner: owner.name.to_string(),
                template_owner: owner.template.to_string(),
                owner_module: owner.module.map(str::to_string),
                clone_module: super::clone_source_tag(owner.module, owner.name, &clone.name),
                clone_name: clone.name.clone(),
                template_name: method.name.clone(),
                body: first.span,
                clone_body: clone_first.span,
                type_bindings: bindings
                    .iter()
                    .filter_map(|binding| Some((binding.name.clone(), binding.source.clone()?)))
                    .collect(),
                value_bindings: own
                    .iter()
                    .filter(|binding| binding.source.is_none() && !pack(binding))
                    .map(|binding| (binding.name.clone(), binding.value.clone()))
                    .collect(),
                pack_bindings,
                first_copy_template: false,
            });
    }

    /// The method names a trait requires, including those of the traits it
    /// refines; a trait the program does not declare requires nothing.
    fn trait_requirement_names(&self, trait_name: &str) -> HashSet<String> {
        let mut names = HashSet::new();
        let mut pending = vec![trait_name.to_string()];
        let mut seen = HashSet::new();
        while let Some(current) = pending.pop() {
            if !seen.insert(current.clone()) {
                continue;
            }
            let declaration = self
                .program
                .iter()
                .find_map(|statement| match &statement.kind {
                    StmtKind::Trait {
                        name,
                        refines,
                        methods,
                        ..
                    } if *name == current
                        || name
                            .rsplit_once('$')
                            .is_some_and(|(_, unqualified)| unqualified == current) =>
                    {
                        Some((refines, methods))
                    }
                    _ => None,
                });
            let Some((refines, methods)) = declaration else {
                continue;
            };
            names.extend(methods.iter().map(|method| method.name.clone()));
            pending.extend(refines.iter().cloned());
        }
        names
    }

    /// The clone of `method`, of `owner`'s stubbed template, whose
    /// elaboration under `bindings` failed with `error`: the instance's
    /// signature over a body reporting the failure.
    fn failed_method_clone(
        &self,
        owner: &str,
        method: &Method,
        clone_name: String,
        bindings: &[MethodBinding],
        consts: &HashMap<String, CtValue>,
        error: &ComptimeError,
    ) -> Option<Method> {
        let mut marked = method.clone();
        marked.body = vec![instantiation_failure_stub(
            owner,
            &method.name,
            method,
            error,
        )];
        self.specialize_method_clone(&marked, clone_name, bindings, consts, consts)
            .ok()
    }

    /// Clone `method` with `bindings` baked: the bound parameters leave the
    /// declaration, their names are substituted in every type position, and
    /// the body elaborates with them bound so its comptime constructs fold.
    fn specialize_method_clone(
        &self,
        method: &Method,
        clone_name: String,
        bindings: &[MethodBinding],
        env: &HashMap<String, CtValue>,
        subs: &HashMap<String, CtValue>,
    ) -> Result<Method, ComptimeError> {
        let mut clone = method.clone();
        clone.name = clone_name;
        let baked: HashSet<&str> = bindings
            .iter()
            .map(|binding| binding.name.as_str())
            .collect();
        let type_bindings: HashMap<String, Type> = bindings
            .iter()
            .filter_map(|binding| Some((binding.name.clone(), binding.source.clone()?)))
            .collect();
        clone
            .type_params
            .retain(|parameter| !baked.contains(parameter.name.trim_start_matches('*')));
        for parameter in &mut clone.type_params {
            if let Some(bound) = &mut parameter.callable_bound {
                substitute_type_bindings_in_type(bound, &type_bindings);
            }
            if let Some(value_type) = &mut parameter.value_type {
                substitute_type_bindings_in_type(value_type, &type_bindings);
            }
        }
        // A callable-bounded type parameter whose contract is now concrete
        // (`F: def() -> Int`) leaves the declaration too: the parameter it
        // types is spelled with the contract itself, so the clone is fully
        // concrete and its overload symbol carries the contract.
        let mut type_bindings = type_bindings;
        let siblings = clone.type_params.clone();
        clone.type_params.retain(|parameter| {
            match &parameter.callable_bound {
                Some(bound) if !retained_specialization_param(parameter, &siblings) => {
                    // A bound names a contract, not an environment class:
                    // the parameter accepts any callable of that shape, so
                    // the folded spelling is the capturing-agnostic
                    // `def(...) capturing[_] -> R`.
                    let mut contract = bound.clone();
                    if let Type::Func {
                        thin: false,
                        capturing: capturing @ None,
                        ..
                    } = &mut contract
                    {
                        *capturing = Some(vec![Expr::new(
                            ExprKind::Identifier("_".to_string()),
                            mojito_common::token::DUMMY_SPAN,
                        )]);
                    }
                    type_bindings
                        .insert(parameter.name.trim_start_matches('*').to_string(), contract);
                    false
                }
                _ => true,
            }
        });
        // A baked value parameter may sit in a **type** position of the
        // method's own signature (`a: Scalar[dt]`, `-> SIMD[DType.int32, w]`),
        // where the clone no longer declares the binder: spell its value
        // there, as a def specialization does.
        let binding_values: HashMap<&str, &CtValue> = bindings
            .iter()
            .map(|binding| (binding.name.as_str(), &binding.value))
            .collect();
        let value_subs: Subs = &|name| binding_values.get(name).map(|value| (*value).clone());
        for parameter in &mut clone.type_params {
            if let Some(value_type) = &mut parameter.value_type {
                rewrite_type(value_type, value_subs);
            }
            if let Some(bound) = &mut parameter.callable_bound {
                rewrite_type(bound, value_subs);
            }
        }
        for parameter in &mut clone.params {
            rewrite_type(&mut parameter.ty, value_subs);
            substitute_type_bindings_in_type(&mut parameter.ty, &type_bindings);
            if let Some(default) = &mut parameter.default {
                rewrite_expr(default, value_subs);
                substitute_type_bindings_in_expr(default, &type_bindings);
            }
        }
        if let Some(ret) = &mut clone.ret {
            rewrite_type(ret, value_subs);
            substitute_type_bindings_in_type(ret, &type_bindings);
        }
        if let Some(error) = &mut clone.raises_type {
            rewrite_type(error, value_subs);
            substitute_type_bindings_in_type(error, &type_bindings);
        }
        for predicate in &mut clone.where_clauses {
            substitute_type_bindings_in_expr(predicate, &type_bindings);
        }
        // A type pack bound to a closed tuple of types expands as a def
        // specialization's does: `*args: *Ts` becomes the `$pack[...]`
        // element list, pack spellings in the signature expand, and the
        // body elaborates with `Ts` and `args` bound so `Ts.length`,
        // `Ts[i]`, and `len(args)` fold.
        let mut type_pack_expansions: HashMap<String, Vec<Type>> = HashMap::new();
        let mut clone_env = env.clone();
        let mut clone_subs = subs.clone();
        for binding in bindings {
            clone_env.insert(binding.name.clone(), binding.value.clone());
            clone_subs.insert(binding.name.clone(), binding.value.clone());
            let CtValue::Tuple(elements) = &binding.value else {
                continue;
            };
            if !method
                .type_params
                .iter()
                .any(|parameter| parameter.name.trim_start_matches('*') == binding.name)
            {
                continue;
            }
            let source_types = elements
                .iter()
                .map(|element| match element {
                    CtValue::Type(ty) => self.pack_element_source_type(ty),
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| {
                    ComptimeError::NotComptime(format!(
                        "type pack '{}' contains a type that cannot be spelled in a clone",
                        binding.name
                    ))
                })?;
            for parameter in &mut clone.params {
                if matches!(&parameter.ty, Type::Named(name, _) if name.trim_start_matches('*') == binding.name)
                {
                    parameter.ty = Type::Named(
                        "$pack".to_string(),
                        source_types.iter().cloned().map(ParamArg::Type).collect(),
                    );
                    clone_env.insert(parameter.name.clone(), binding.value.clone());
                }
            }
            type_pack_expansions.insert(binding.name.clone(), source_types);
        }
        if !type_pack_expansions.is_empty() {
            if let Some(ret) = &mut clone.ret {
                expand_type_packs(ret, &type_pack_expansions);
            }
            for parameter in &mut clone.params {
                expand_type_packs(&mut parameter.ty, &type_pack_expansions);
            }
        }
        let elaborated = self.block(&clone.body, &mut clone_env, true)?;
        clone.body = materialize_block(
            elaborated,
            &clone_subs,
            &self.struct_names,
            &self.applied_constants(),
        );
        if !type_pack_expansions.is_empty() {
            expand_pack_spreads_in_function_body(
                &mut clone.body,
                &clone.params,
                &type_pack_expansions,
                &self.served_packs,
            );
        }
        substitute_type_bindings_in_block(&mut clone.body, &type_bindings);
        Ok(clone)
    }
}

/// What a per-call method clone inherits from where it is minted: an
/// instance's baked values and bindings (empty for a non-generic struct),
/// the instance clone's explicit receiver type, the struct whose method
/// list the clone joins under its own source tag, which a trace names, and
/// the origin binders the instance declares on every clone of it. A
/// clone minted into a struct specialized whole names the specialization
/// as its owner and the template struct as its template's. `constructors`
/// says a generic `__init__` mints here too (`__init__$y6:String`, or
/// `__init__$y3:Int$y6:String` on an instance); a struct specialized whole
/// mints those itself, under the name `__init__`, and a bundled template's
/// constructors stay erased.
#[derive(Clone, Copy, Default)]
pub(super) struct PerCallBase<'a> {
    pub(super) values: &'a [CtValue],
    pub(super) bindings: &'a [MethodBinding],
    pub(super) receiver: Option<&'a Type>,
    pub(super) owner: Option<PerCallOwner<'a>>,
    pub(super) origin_binders: Option<&'a CloneOriginBinders>,
    pub(super) constructors: bool,
}

/// The struct a traced per-call clone joins, its module, and the struct
/// whose method is the clone's template: the same struct, except for a
/// struct specialized whole (`AHasher$…`, whose template is `AHasher`).
#[derive(Clone, Copy)]
pub(super) struct PerCallOwner<'a> {
    pub(super) name: &'a str,
    pub(super) module: Option<&'a str>,
    pub(super) template: &'a str,
}

/// One baked compile-time parameter of a per-call method clone: its name,
/// the compile-time value bound in the clone's elaboration environment, and
/// (for a type parameter) the source type substituted into the signature.
#[derive(Clone)]
pub(super) struct MethodBinding {
    name: String,
    value: CtValue,
    source: Option<Type>,
}

/// Whether a method compile-time parameter is baked into per-call clones: a
/// plain type or value parameter. Callable-bounded parameters
/// (`F: def() -> T`) and retained callable-value parameters stay on the
/// clone's signature, and Origin binders have no argument at all.
pub(super) fn method_parameter_is_baked(parameter: &TypeParam, siblings: &[TypeParam]) -> bool {
    parameter.callable_bound.is_none() && !retained_specialization_param(parameter, siblings)
}

/// The clone each stub-reaching method of `template` owes one instance: its
/// source name and the clone name that instance must mint.
///
/// A method missing from what the instance actually mints keeps the erased
/// path there, and that path can reach a compile-time-keyed stub.
fn owed_instance_clones(
    methods: &[Method],
    template: &str,
    values: &[CtValue],
    stub_reaching: &HashSet<String>,
) -> Vec<(String, String)> {
    methods
        .iter()
        .filter(|method| {
            method.self_ty.is_none()
                && stub_reaching.contains(&super::method_owner(template, &method.name))
        })
        .filter_map(|method| {
            let lifecycle = mojito_symbol::symbol::lifecycle_method_name(method);
            Some((method.name.clone(), mangle(lifecycle, values).ok()?))
        })
        .collect()
}

/// The methods of `template` whose bodies run only with the struct's
/// parameters bound, by name: one whose template body is the trap stub (a
/// compile-time construct folds only per instance), and one whose erased
/// body reaches a compile-time-keyed stub. Every other method's template serves each
/// instance, so it mints no clone. An overload family is keyed
/// whole.
fn keyed_methods(
    methods: &[Method],
    template: &str,
    stub_reaching: &HashSet<String>,
) -> HashSet<String> {
    methods
        .iter()
        .filter(|method| {
            method.self_ty.is_none()
                && (is_unspecialized_method_stub(&method.body)
                    || stub_reaching.contains(&super::method_owner(template, &method.name)))
        })
        .map(|method| method.name.clone())
        .collect()
}

/// The [`super::method_owner`] keys of the struct methods in `program`
/// whose template body is the trap stub although they declare compile-time
/// parameters of their own: only a per-call clone serves a call of one.
fn per_call_stubs(program: &[Stmt]) -> HashSet<String> {
    program
        .iter()
        .filter_map(|statement| match &statement.kind {
            StmtKind::Struct { name, methods, .. } => Some((name, methods)),
            _ => None,
        })
        .flat_map(|(name, methods)| {
            methods
                .iter()
                .filter(|method| {
                    method.self_ty.is_none()
                        && is_unspecialized_method_stub(&method.body)
                        && method.type_params.iter().any(|parameter| {
                            method_parameter_is_baked(parameter, &method.type_params)
                        })
                })
                .map(|method| super::method_owner(name, &method.name))
        })
        .collect()
}

/// Whether `body` is [`unspecialized_method_stub`]'s trap.
pub(super) fn is_unspecialized_method_stub(body: &[Stmt]) -> bool {
    let [statement] = body else {
        return false;
    };
    let StmtKind::Expr(call) = &statement.kind else {
        return false;
    };
    matches!(&call.kind, ExprKind::Call { name, args, .. }
        if name == "_mojito_abort"
            && matches!(args.as_slice(), [message]
                if matches!(&message.kind, ExprKind::Str(text)
                    if text.ends_with(METHOD_STUB_REASON))))
}

const METHOD_STUB_REASON: &str = ": unspecialized type-keyed method";

/// A type-pack or compile-time-keyed `def` template reduced to its
/// signature: the body traps with `reason`, so the retained declaration
/// checks (an abort diverges past any return type) without specializing
/// `args[i]` over an unknown pack or selecting a `comptime if` arm over an
/// unbound parameter.
pub(super) fn template_stub(template: &Stmt, reason: &str) -> Stmt {
    let mut stub = template.clone();
    if let StmtKind::Def { name, body, .. } = &mut stub.kind {
        let span = body
            .first()
            .map_or(mojito_common::token::DUMMY_SPAN, |statement| statement.span);
        *body = vec![mk(
            StmtKind::Expr(Expr::new(
                ExprKind::Call {
                    name: "_mojito_abort".to_string(),
                    param_args: Vec::new(),
                    args: vec![Expr::new(ExprKind::Str(format!("{name}: {reason}")), span)],
                    kwargs: Vec::new(),
                },
                span,
            )),
            span,
        )];
    }
    stub
}

pub(super) fn unspecialized_method_stub(owner: &str, method: &Method) -> Stmt {
    intrinsic_statement(
        "_mojito_abort",
        format!("{owner}.{}{METHOD_STUB_REASON}", method.name),
        method,
    )
}

/// The body of a method clone of `owner`'s template method `name` whose
/// elaboration failed with `error`: the elaborator below MIR reports the
/// failure if a reachable call instantiates the clone.
pub(super) fn instantiation_failure_stub(
    owner: &str,
    name: &str,
    method: &Method,
    error: &ComptimeError,
) -> Stmt {
    intrinsic_statement(
        "_mojito_instantiation_failed",
        format!("{owner}.{name}: {error}"),
        method,
    )
}

/// A statement calling the compiler-private intrinsic `callee` with the
/// literal `message`, at `method`'s first statement.
fn intrinsic_statement(callee: &str, message: String, method: &Method) -> Stmt {
    let span = method
        .body
        .first()
        .map_or(mojito_common::token::DUMMY_SPAN, |statement| statement.span);
    mk(
        StmtKind::Expr(Expr::new(
            ExprKind::Call {
                name: callee.to_string(),
                param_args: Vec::new(),
                args: vec![Expr::new(ExprKind::Str(message), span)],
                kwargs: Vec::new(),
            },
            span,
        )),
        span,
    )
}
