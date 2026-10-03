//! Monomorphization and specialization generation: `monomorphize`, tuple-spec
//! ordering, and `def`/`struct` specialization synthesis.
//! Extracted from `comptime.rs`; see `docs/symbol-map.md`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_ast::ast::Method;
use mojito_types::types::tuple_elements;

/// Read the concrete truth value from a folded declaration constraint while
/// preserving validation of the diagnostic tuple form.
fn folded_constraint_truth(expression: &Expr) -> Result<Option<bool>, ComptimeError> {
    match &expression.kind {
        ExprKind::Bool(value) => Ok(Some(*value)),
        ExprKind::TupleLit(elements) => {
            let [condition, message] = elements.as_slice() else {
                return Err(ComptimeError::NotComptime(
                    "a diagnostic where clause must be `(condition, \"message\")`".to_string(),
                ));
            };
            if !matches!(&message.kind, ExprKind::Str(_)) {
                return Err(ComptimeError::NotComptime(
                    "a where-clause diagnostic message must be a string literal".to_string(),
                ));
            }
            Ok(match &condition.kind {
                ExprKind::Bool(value) => Some(*value),
                _ => None,
            })
        }
        _ => Ok(None),
    }
}

impl Elab<'_> {
    /// Specialize every comptime-dependent generic template against the value
    /// arguments at its call sites, replacing each template with its concrete
    /// specializations (which have their `comptime if`/`for` resolved).
    pub(super) fn monomorphize(
        &self,
        program: Vec<Stmt>,
        tuple_requests: &[TupleSpecializationRequest],
        tstring_requests: &[TStringSpecializationRequest],
        def_requests: &[DefSpecializationRequest],
    ) -> Result<Elaborated, ComptimeError> {
        if self.specializable.is_empty()
            && tuple_requests.is_empty()
            && tstring_requests.is_empty()
            && self.instance_requests.is_empty()
        {
            return Ok(Elaborated {
                program,
                instances: Vec::new(),
                stub_reaching_structs: HashSet::new(),
                unserved_template_uses: Vec::new(),
                def_traces: Vec::new(),
                method_traces: Vec::new(),
                generated: super::GeneratedDeclarations::default(),
                ctfe_template_stats: mojito_checked::templates::TemplateStats::default(),
                clones: mojito_checked::census::CloneCensus::default(),
            });
        }
        if !tuple_requests.is_empty() && !self.struct_template("Tuple") {
            return Err(ComptimeError::NotComptime(
                "checked Tuple specialization requests require a public variadic `Tuple[*Ts]` template"
                    .to_string(),
            ));
        }
        if !tstring_requests.is_empty() && !self.struct_template("TString") {
            return Err(ComptimeError::NotComptime(
                "checked TString specialization requests require the prelude's variadic `TString[*Ts]` template"
                    .to_string(),
            ));
        }
        let consts = self.top_consts.borrow().clone();
        let mut mono = Mono::default();
        let mut program = program;
        self.stamp_per_call_clone_bodies(&mut program);
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
        // Vector-keyed specializations named by compile-time type expressions
        // (`comptime default_hasher = AHasher[...]`) mint like call-site
        // applications; the alias already spells the clone.
        for (mangled, (orig, vals)) in self.pending_struct_instances.borrow().iter() {
            if mono.done.insert(mangled.clone()) {
                mono.queue.push_back(Job {
                    orig: orig.clone(),
                    decl: None,
                    vals: vals.clone(),
                    site: "a compile-time type alias".to_string(),
                    output_name: mangled.clone(),
                    whole_pack_abi: false,
                });
            }
        }
        for request in tuple_requests {
            let vals = tuple_specialization_values(request.elements());
            let output_name = tuple_specialization_symbol(request.elements());
            if let Some(occurrence) = request.occurrence()
                && let Some(existing) = mono
                    .tuple_call_targets
                    .insert(occurrence.clone().without_syntax(), output_name.clone())
                && existing != output_name
            {
                return Err(ComptimeError::NotComptime(format!(
                    "one bare Tuple call was assigned incompatible specializations '{existing}' and '{output_name}'"
                )));
            }
            if mono.done.insert(output_name.clone()) {
                mono.queue.push_back(Job {
                    orig: "Tuple".to_string(),
                    decl: None,
                    vals,
                    site: request.occurrence().map_or_else(
                        || "a checked Tuple type".to_string(),
                        |span| match &span.source {
                            Some(source) => {
                                format!("{source}:{}..{}", span.span.0, span.span.1)
                            }
                            None => format!("bytes {}..{}", span.span.0, span.span.1),
                        },
                    ),
                    output_name,
                    whole_pack_abi: false,
                });
            }
        }
        // Checker-discovered t-string occurrences: each one materializes the
        // concrete `TString` specialization and records the occurrence target
        // consumed by `mono_expr`'s rewrite of the `t"…"` node into that
        // specialization's construction.
        for request in tstring_requests {
            // StringLiteral remains a borrowed, drop-inert descriptor
            // everywhere else; specialize textual fields to nominal String
            // so the ordinary struct lifecycle owns their runtime buffers.
            let vals = tuple_specialization_values(
                &mojito_symbol::symbol::tstring_storage_elements(request.elements()),
            );
            let output_name = tstring_specialization_symbol(request.elements());
            let target = TStringTarget {
                symbol: output_name.clone(),
                elements: request.elements().to_vec(),
            };
            if let Some(existing) = mono
                .tstring_call_targets
                .insert(request.occurrence().clone().without_syntax(), target)
                && existing.symbol != output_name
            {
                return Err(ComptimeError::NotComptime(format!(
                    "one t-string occurrence was assigned incompatible specializations '{}' and '{output_name}'",
                    existing.symbol
                )));
            }
            if mono.done.insert(output_name.clone()) {
                mono.queue.push_back(Job {
                    orig: "TString".to_string(),
                    decl: None,
                    vals,
                    site: match &request.occurrence().source {
                        Some(source) => {
                            format!(
                                "{source}:{}..{}",
                                request.occurrence().span.0,
                                request.occurrence().span.1
                            )
                        }
                        None => format!(
                            "bytes {}..{}",
                            request.occurrence().span.0,
                            request.occurrence().span.1
                        ),
                    },
                    output_name,
                    whole_pack_abi: false,
                });
            }
        }
        self.seed_def_call_targets(def_requests, &mut mono);
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
        // The nested pass runs after this walk and registers a nested `def`
        // whose body is named here, so that its instances reach the clone the
        // erased body could only leave on a stub. It is computed again after
        // the drains: a generated clone's own nested `def` is walked there,
        // and carries a source of its own.
        self.stub_reaching.replace(
            self.stub_reaching_bodies(&mono.abstract_uses, &mono.method_edges)
                .iter()
                .map(|body| (*body).to_string())
                .collect(),
        );
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
            // A variadic struct template applied over a retained generic
            // body's own parameters survives as a shell: its parameters,
            // conformances (taken unconditionally — the concrete
            // specialization re-verifies them), and method signatures, with
            // no fields, bodies, or lowering.
            if self.struct_template(&template_name) && mono.retained.contains(&template_name) {
                out.push(template_shell(&stmt));
            }
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
                && comptime_keyed_declaration(&stmt)
            {
                // A compile-time-keyed template with a deferred call stands
                // in the same way: until the checker's request is served, or
                // for good under a call from an abstract generic body.
                out.push(template_stub(
                    &stmt,
                    "unspecialized compile-time-keyed function",
                ));
            } else if retained
                && self.dtype_generics.contains(&template_name)
                && dtype_keyed_declaration(&stmt)
            {
                // A `DType`-keyed template with a deferred call stands in the
                // same way, until the checker reads the lane off the call's
                // argument and its request is served.
                out.push(template_stub(&stmt, "unspecialized DType-keyed function"));
            }
            if let Some(mut specs) = generated {
                specs.reverse();
                if template_name == "Tuple" {
                    specs = self.order_tuple_specializations(specs)?;
                }
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
            // A request on a struct template is a constructor rewrite rather
            // than a def clone: a scalar-range request names the DType-keyed
            // range-family template and its dtype, and a bare variadic-struct
            // construction names the template and the pack the checker
            // inferred, spelled element by element. The Job queues lazily at
            // the rewrite, like the def targets below.
            if self.struct_template(callee) {
                let vals = match request.arguments() {
                    [TyArg::Val(value @ CtValue::Dtype(_))] => Some(vec![value.clone()]),
                    [TyArg::Val(value @ CtValue::Tuple(_))]
                        if self.single_pack_template(callee) =>
                    {
                        Some(vec![value.clone()])
                    }
                    arguments if self.single_pack_template(callee) => arguments
                        .iter()
                        .map(|argument| match argument {
                            TyArg::Ty(ty) => Some(ty.clone()),
                            TyArg::Val(_) | TyArg::Origin(_) => None,
                        })
                        .collect::<Option<Vec<Ty>>>()
                        .map(|types| tuple_specialization_values(&types)),
                    _ => None,
                };
                if let Some(vals) = vals {
                    mono.struct_call_targets
                        .entry(request.occurrence().clone().without_syntax())
                        .or_insert_with(|| (callee.to_string(), vals));
                }
                continue;
            }
            if !self.bound_generics.contains(callee)
                && !self.pack_generics.contains(callee)
                && !self.comptime_generics.contains(callee)
                && !self.dtype_generics.contains(callee)
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
            // A call inside a nested instance is rewritten by the lexical
            // pass, which runs after this walk and cannot queue work of its
            // own, so its clone is queued here instead of at the consult. The
            // occurrence names a real call, so a clone minted for a request
            // whose occurrence has since drifted is dead code rather than a
            // wrong answer.
            if request
                .occurrence()
                .source
                .as_deref()
                .is_some_and(|source| source.contains(NESTED_MARKER_INFIX))
            {
                let Ok(output_name) = mangle(callee, &vals) else {
                    continue;
                };
                if mono.queue_specialization(&output_name, decl) {
                    mono.queue.push_back(Job {
                        orig: callee.to_string(),
                        decl,
                        vals: vals.clone(),
                        site: format!("a call inside a nested specialization of '{callee}'"),
                        output_name,
                        whole_pack_abi: false,
                    });
                }
            }
            mono.def_call_targets
                .entry(request.occurrence().clone())
                .or_insert_with(|| DefCallTarget {
                    template: callee.to_string(),
                    decl,
                    vals,
                });
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
        let InstanceClones {
            clones,
            mut field_types,
            withheld,
        } = self.generate_instance_clones(template, values, &keyed)?;
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
        let mut kept = self.walk_instance_clones(clones, template, module.as_deref(), consts, mono);
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
    /// rather than failing the program.
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
            if walked.is_ok() {
                kept.push(clone);
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
        let mut stubbed: HashSet<&str> = self
            .comptime_generics
            .iter()
            .map(String::as_str)
            // A `DType`-keyed template stands as a stub only where a call
            // actually deferred to the checker; the rest specialize outright
            // and are nobody's stub.
            .chain(
                self.dtype_generics
                    .iter()
                    .map(String::as_str)
                    .filter(|name| uses.iter().any(|reference| reference.callee == **name)),
            )
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
    /// whose compile-time parameters are all type parameters, and its body
    /// neither holds a construct only an instance lowers (by its syntax, or
    /// by its checked types as the driver read them) nor reaches a
    /// compile-time-keyed stub. The elaborator instantiates the template's
    /// MIR for each call, and a call's transfer summary names its loans by
    /// the stored type. A value parameter keeps the clone until the
    /// elaborator binds one from a call. The verdict is the first one made
    /// for the name, so every call of it agrees.
    pub(super) fn template_serves_def(&self, name: &str, template: &Stmt, mono: &Mono) -> bool {
        if let Some(served) = self.template_served_defs.borrow().get(name) {
            return *served;
        }
        let StmtKind::Def {
            body, type_params, ..
        } = &template.kind
        else {
            return false;
        };
        let served = self.bound_generics.contains(name)
            && classify_ct_params(type_params, name).iter().all(|decl| {
                matches!(
                    decl,
                    ParamDecl::Type {
                        variadic: false,
                        ..
                    }
                )
            })
            && !holds_instance_construct(body)
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
            let traced = self.method_traces.borrow().len();
            let mut spec = match &template.kind {
                StmtKind::Struct { type_params, .. }
                    if !classify_ct_params(type_params, &job.orig)
                        .iter()
                        .any(|decl| matches!(decl, ParamDecl::Type { variadic: true, .. })) =>
                {
                    self.generate_value_struct_spec(&job.orig, &job.vals)?
                }
                StmtKind::Struct { .. } => self.generate_struct_spec(&job.orig, &job.vals)?,
                _ => {
                    self.generate_def_spec(template, &job.orig, job.output_name.clone(), &job.vals)?
                }
            };
            // TString's public specialization identity describes its source
            // segments, while its concrete storage pack upgrades textual
            // elements to owning nominal String. Preserve the checker-picked
            // public symbol instead of remangling from that private ABI pack,
            // in the specialization and in the traces of its members.
            if job.orig == "TString"
                && let StmtKind::Struct { name, .. } = &mut spec.kind
            {
                for trace in self.method_traces.borrow_mut().iter_mut().skip(traced) {
                    if trace.owner == *name {
                        trace.owner.clone_from(&job.output_name);
                    }
                }
                name.clone_from(&job.output_name);
            }
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
            {
                let mut generated = self.generated.borrow_mut();
                if let StmtKind::Struct { name, .. } = &spec.kind {
                    generated.structs.push(name.clone());
                }
            }
            mono.generated.entry(job.orig).or_default().push(spec);
        }
        Ok(())
    }

    /// Order concrete Tuple declarations by the ordinary method-signature and
    /// constructor dependencies introduced for the transforms actually used by
    /// the checked program. The generic worklist's blanket reversal handles a
    /// newly discovered callee, but all checked Tuple result types are seeded up
    /// front, so that incidental queue order is not a dependency relation.
    pub(super) fn order_tuple_specializations(
        &self,
        specs: Vec<Stmt>,
    ) -> Result<Vec<Stmt>, ComptimeError> {
        fn visit(
            name: &str,
            dependencies: &HashMap<String, Vec<String>>,
            visiting: &mut HashSet<String>,
            emitted: &mut HashSet<String>,
            order: &mut Vec<String>,
        ) -> Result<(), ComptimeError> {
            if emitted.contains(name) {
                return Ok(());
            }
            if !visiting.insert(name.to_string()) {
                return Err(ComptimeError::NotComptime(format!(
                    "checked Tuple transforms create a cyclic declaration dependency involving '{name}'"
                )));
            }
            if let Some(required) = dependencies.get(name) {
                for dependency in required {
                    visit(dependency, dependencies, visiting, emitted, order)?;
                }
            }
            visiting.remove(name);
            emitted.insert(name.to_string());
            order.push(name.to_string());
            Ok(())
        }

        let baseline = specs
            .iter()
            .map(|statement| match &statement.kind {
                StmtKind::Struct { name, .. } => Ok(name.clone()),
                _ => Err(ComptimeError::NotComptime(
                    "Tuple specialization produced a non-struct declaration".to_string(),
                )),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let declared = baseline.iter().cloned().collect::<HashSet<_>>();
        let mut dependencies = HashMap::<String, Vec<String>>::new();
        let mut add_dependency = |receiver: &str, dependency: String| {
            if dependency != receiver && declared.contains(&dependency) {
                let entries = dependencies.entry(receiver.to_string()).or_default();
                if !entries.contains(&dependency) {
                    entries.push(dependency);
                }
            }
        };
        for (left, transforms) in &self.tuple_transforms {
            let receiver = tuple_specialization_symbol(left);
            for transform in transforms {
                match transform {
                    TupleTransformRequest::Reverse => {
                        // Generated Tuple identities are predeclared before any
                        // specialization members are checked.  A reverse method's
                        // result annotation and constructor can therefore name the
                        // reverse specialization before its full declaration.  Do
                        // not manufacture a hard ordering edge here: requesting
                        // reverse in both directions is a valid two-node cycle.
                    }
                    TupleTransformRequest::Concat(right) => {
                        add_dependency(&receiver, tuple_specialization_symbol(right));
                        let mut result = left.clone();
                        result.extend(right.iter().cloned());
                        add_dependency(&receiver, tuple_specialization_symbol(&result));
                    }
                }
            }
        }

        let mut order = Vec::with_capacity(baseline.len());
        let mut visiting = HashSet::new();
        let mut emitted = HashSet::new();
        for name in &baseline {
            visit(name, &dependencies, &mut visiting, &mut emitted, &mut order)?;
        }
        let mut by_name = specs
            .into_iter()
            .map(|statement| {
                let StmtKind::Struct { name, .. } = &statement.kind else {
                    unreachable!("validated Tuple specialization shape")
                };
                (name.clone(), statement)
            })
            .collect::<HashMap<_, _>>();
        Ok(order
            .into_iter()
            .map(|name| {
                by_name
                    .remove(&name)
                    .expect("topological Tuple name came from generated declarations")
            })
            .collect())
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
        let mut final_body = materialize_block(elaborated, &subs, &self.struct_names);
        fold_pack_element_constructions(&mut final_body, &type_pack_values);
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

    /// Generate one specialization of variadic-struct template `orig` for the
    /// compile-time arguments `vals`: bind the type pack in the comptime env so
    /// member bodies' `comptime if`/`for` resolve against the concrete element
    /// types, expand pack-typed member annotations (`Tuple[*Ts]`) to the concrete
    /// list, and emit a fully concrete (parameter-free) struct under the mangled
    /// name. Unlike a def specialization, nothing stays symbolic.
    /// Emit a concrete struct for a template whose compile-time parameters
    /// are scalar/`DType`/struct **values** (no type packs): fold each value
    /// into method bodies, defaults, and field/signature type positions,
    /// retain Origin/`mut` binders as `TypeParams` (the specialization stays
    /// origin-generic, like `_ListIter`), and name the result by the
    /// specialization mangle.
    pub(super) fn generate_value_struct_spec(
        &self,
        orig: &str,
        vals: &[CtValue],
    ) -> Result<Stmt, ComptimeError> {
        let template = self.specializable[orig];
        let StmtKind::Struct {
            name: _,
            decorators,
            type_params,
            conforms,
            callable_conformance,
            conformance_conditions,
            where_clauses,
            fields,
            associated,
            methods,
            fieldwise_init,
            template_shell: _,
        } = &template.kind
        else {
            return Err(ComptimeError::NotComptime(format!(
                "specialization registry entry '{orig}' is not a struct"
            )));
        };
        let mut kept_type_params = Vec::new();
        let mut env = self.top_consts.borrow().clone();
        let mut subs = self.top_consts.borrow().clone();
        let mut folded = Vec::new();
        let mut values = vals.iter();
        for parameter in type_params {
            if retained_specialization_param(parameter, type_params) {
                kept_type_params.push(parameter.clone());
                continue;
            }
            let value = values.next().ok_or_else(|| {
                ComptimeError::Arity(format!(
                    "'{orig}' expects a compile-time argument for parameter '{}'",
                    parameter.name
                ))
            })?;
            let binding = parameter.name.trim_start_matches('*').to_string();
            env.insert(binding.clone(), value.clone());
            subs.insert(binding.clone(), value.clone());
            folded.push((binding, value.clone()));
        }
        if values.next().is_some() {
            return Err(ComptimeError::Arity(format!(
                "'{orig}' received more compile-time arguments than parameters"
            )));
        }
        let value_subs: Subs = &|name| subs.get(name).cloned();
        let mut specialized_fields = fields.clone();
        for field in &mut specialized_fields {
            rewrite_type(&mut field.ty, value_subs);
        }
        let mut specialized_callable_conformance = callable_conformance.clone();
        if let Some(callable) = &mut specialized_callable_conformance {
            rewrite_type(callable, value_subs);
        }
        let specialized_conformance_conditions = conformance_conditions
            .iter()
            .map(|(trait_name, condition)| {
                (trait_name.clone(), materialize_expression(condition, &subs))
            })
            .collect();
        let specialized_where: Vec<Expr> = if kept_type_params.is_empty() {
            for predicate in where_clauses {
                self.validate_specialized_where(predicate, &env, orig)?;
            }
            Vec::new()
        } else {
            where_clauses
                .iter()
                .map(|predicate| materialize_expression(predicate, &subs))
                .collect()
        };
        let mut specialized_associated = associated.clone();
        for member in &mut specialized_associated {
            // An associated declaration's own parameters shadow names from the
            // enclosing struct. Materialize only the outer bindings which are
            // still visible in its annotation, availability, and value.
            let mut member_subs = subs.clone();
            for parameter in &member.params {
                member_subs.remove(parameter.name.trim_start_matches('*'));
            }
            let member_value_subs: Subs = &|name| member_subs.get(name).cloned();
            for parameter in &mut member.params {
                if let Some(value_type) = &mut parameter.value_type {
                    rewrite_type(value_type, member_value_subs);
                }
                if let Some(callable) = &mut parameter.callable_bound {
                    rewrite_type(callable, member_value_subs);
                }
                if let Some(mutability) = &mut parameter.origin_mutability {
                    *mutability = materialize_expression(mutability, &member_subs);
                }
                if let Some(default) = &mut parameter.default {
                    *default = materialize_expression(default, &member_subs);
                }
                for constraint in &mut parameter.constraints {
                    *constraint = materialize_expression(constraint, &member_subs);
                }
            }
            if let Some(ty) = &mut member.ty {
                rewrite_type(ty, member_value_subs);
            }
            for condition in &mut member.where_clauses {
                *condition = materialize_expression(condition, &member_subs);
            }
            member.value = materialize_expression(&member.value, &member_subs);
        }
        let mangled = mangle(orig, vals)?;
        let mut specialized_methods = Vec::with_capacity(methods.len());
        let mut simd_clones = Vec::new();
        let mut call_clones = Vec::new();
        let mut members = Vec::new();
        let leaf_traces = self.method_traces.borrow().len();
        let requests = self
            .method_requests
            .get(&mangled)
            .map_or(&[][..], Vec::as_slice);
        for method in methods {
            let template_body = traced_template_body(method);
            let source_body = method.body.clone();
            let mut method = method.clone();
            // A SIMD-keyed method (`_update_with_simd(mut self, value:
            // SIMD[_, _])`) checks only as per-leaf clones, minted here for
            // this specialization; its template body is the trap stub.
            if super::synth::is_simd_keyed_method(&method) {
                let requests = super::synth::hasher_leaf_requests(template, &self.hash_leaf_types);
                simd_clones.extend(self.per_call_method_clones(
                    orig,
                    &method,
                    &requests,
                    &PerCallBase {
                        owner: Some(PerCallOwner {
                            name: &mangled,
                            module: template.module.as_deref(),
                            template: orig,
                        }),
                        ..PerCallBase::default()
                    },
                    &env,
                ));
                // Every specialization holds the same stub, spanned at the
                // struct so its template stays apart from the leaves'.
                match method.body.first().map(|first| first.syntax_id) {
                    Some(parent) => {
                        method.body =
                            vec![shared_method_stub(orig, &method, template.span, parent)];
                        members.push(TracedMember {
                            first_copy: true,
                            ..TracedMember::whole(
                                specialized_methods.len(),
                                &method.name,
                                template.span,
                            )
                        });
                    }
                    None => method.body = vec![unspecialized_method_stub(orig, &method)],
                }
                specialized_methods.push(method);
                continue;
            }
            // A vector constructed at the method's own lane lowers only in
            // the per-call clones; the instance's template body is the stub.
            if super::synth::constructs_at_own_lane(&method) {
                method.body = vec![unspecialized_method_stub(orig, &method)];
            } else if let Some(body) = template_body {
                members.push(TracedMember::whole(
                    specialized_methods.len(),
                    &method.name,
                    body,
                ));
            }
            // A regular runtime parameter shadows a same-named compile-time
            // binding inside its own body.
            let mut method_env = env.clone();
            let mut method_subs = subs.clone();
            method_subs.remove("self");
            for parameter in &method.params {
                method_subs.remove(&parameter.name);
                method_env.remove(&parameter.name);
            }
            let elaborated = self
                .block(&method.body, &mut method_env.clone(), true)
                .map_err(|error| {
                    ComptimeError::NotComptime(format!(
                        "while specializing {orig}.{}: {error}",
                        method.name
                    ))
                })?;
            method.body = materialize_block(elaborated, &method_subs, &self.struct_names);
            let method_value_subs: Subs = &|name| method_subs.get(name).cloned();
            for parameter in &mut method.params {
                rewrite_type(&mut parameter.ty, method_value_subs);
                if let Some(default) = &mut parameter.default {
                    *default = materialize_expression(default, &method_subs);
                }
            }
            if let Some(ret) = &mut method.ret {
                rewrite_type(ret, method_value_subs);
            }
            if let Some(error) = &mut method.raises_type {
                rewrite_type(error, method_value_subs);
            }
            for condition in &mut method.where_clauses {
                *condition = materialize_expression(condition, &method_subs);
            }
            // A method with its own baked binder specializes per call from
            // its unelaborated body over the folded struct values; the
            // checker retargets the call to `mangle(method, call values)` on
            // this specialization, and the clone's trace takes the folded
            // values before its own (`restamp_leaf_traces`).
            if !requests.is_empty() {
                let source = Method {
                    body: source_body,
                    ..method.clone()
                };
                call_clones.extend(self.per_call_method_clones(
                    orig,
                    &source,
                    requests,
                    &PerCallBase {
                        owner: Some(PerCallOwner {
                            name: &mangled,
                            module: template.module.as_deref(),
                            template: orig,
                        }),
                        ..PerCallBase::default()
                    },
                    &env,
                ));
            }
            specialized_methods.push(method);
        }
        let call_clone_names: Vec<String> =
            call_clones.iter().map(|clone| clone.name.clone()).collect();
        specialized_methods.extend(simd_clones);
        specialized_methods.extend(call_clones);
        let mut spec = mk(
            StmtKind::Struct {
                name: mangled.clone(),
                decorators: decorators.clone(),
                type_params: kept_type_params,
                conforms: conforms.clone(),
                callable_conformance: specialized_callable_conformance,
                conformance_conditions: specialized_conformance_conditions,
                where_clauses: specialized_where,
                fields: specialized_fields,
                associated: specialized_associated,
                methods: specialized_methods,
                fieldwise_init: *fieldwise_init,
                template_shell: false,
            },
            template.span,
        );
        // Same provenance discipline as the pack path: specializations reuse
        // template spans, so each subtree gets a unique source tag.
        let tag = match &template.module {
            Some(module) => format!("{module}${mangled}"),
            None => mangled,
        };
        mojito_ast::ast::stamp_source(std::slice::from_mut(&mut spec), &tag);
        // A per-call clone shares its template member's spans, so it gets a
        // tag of its own for its span-keyed checked facts.
        if let StmtKind::Struct { methods, .. } = &mut spec.kind {
            for method in methods
                .iter_mut()
                .filter(|method| call_clone_names.contains(&method.name))
            {
                mojito_ast::ast::stamp_source(&mut method.body, &format!("{tag}.{}", method.name));
            }
        }
        spec.module = None;
        self.restamp_leaf_traces(&spec, leaf_traces, &folded);
        self.trace_struct_members(
            &spec,
            &members,
            &StructTemplate {
                name: orig,
                module: template.module.as_deref(),
                values: folded,
                packs: Vec::new(),
            },
        );
        Ok(spec)
    }

    #[allow(
        clippy::cognitive_complexity,
        clippy::too_many_lines,
        reason = "TODO: split this pass; TODO: split this pass"
    )]
    pub(super) fn generate_struct_spec(
        &self,
        orig: &str,
        vals: &[CtValue],
    ) -> Result<Stmt, ComptimeError> {
        let template = self.specializable[orig];
        let StmtKind::Struct {
            decorators,
            type_params,
            conforms,
            callable_conformance,
            conformance_conditions,
            where_clauses,
            fields,
            associated,
            methods,
            fieldwise_init,
            ..
        } = &template.kind
        else {
            return Err(ComptimeError::NotComptime(format!(
                "specialization registry entry '{orig}' is not a struct"
            )));
        };
        let decls = classify_ct_params(type_params, orig);
        let (
            [
                ParamDecl::Type {
                    name: pack,
                    variadic: true,
                    ..
                },
            ],
            [CtValue::Tuple(types)],
        ) = (decls.as_slice(), vals)
        else {
            return Err(ComptimeError::NotComptime(format!(
                "variadic struct '{orig}' supports exactly one type-parameter pack and no other compile-time parameters"
            )));
        };
        let binding = pack.trim_start_matches('*').to_string();
        let semantic_types = types
            .iter()
            .map(|value| match value {
                CtValue::Type(ty) => Some((**ty).clone()),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| {
                ComptimeError::NotComptime("type pack contains a non-type value".to_string())
            })?;
        let mut reference_origins = HashMap::new();
        for ty in &semantic_types {
            collect_reference_origin_parameters(ty, &mut reference_origins).ok_or_else(|| {
                ComptimeError::NotComptime(format!(
                    "Tuple element type '{ty}' has an origin that cannot be retained by a nominal specialization"
                ))
            })?;
        }
        // OriginParamId is declaration-order based. Preserve that identity even
        // when an earlier ordinary type/value parameter did not itself occur in
        // this pack by emitting semantic-only padding origins up to the highest
        // retained id.
        let origin_count = reference_origins
            .keys()
            .map(|id| id.0 as usize + 1)
            .max()
            .unwrap_or(0);
        let origin_names = (0..origin_count)
            .map(|index| {
                (
                    mojito_types::origin::OriginParamId(index as u32),
                    format!("__tuple_origin_{index}"),
                )
            })
            .collect::<HashMap<_, _>>();
        let retained_origin_parameters = (0..origin_count)
            .map(|index| {
                let id = mojito_types::origin::OriginParamId(index as u32);
                let mutability = reference_origins
                    .get(&id)
                    .copied()
                    .unwrap_or(mojito_types::origin::Mutability::Param(id));
                TypeParam {
                    name: origin_names[&id].clone(),
                    bounds: vec!["Origin".to_string()],
                    value_type: None,
                    callable_bound: None,
                    origin_mutability: match mutability {
                        mojito_types::origin::Mutability::Immutable => {
                            Some(Expr::new(ExprKind::Bool(false), template.span))
                        }
                        mojito_types::origin::Mutability::Mutable => {
                            Some(Expr::new(ExprKind::Bool(true), template.span))
                        }
                        mojito_types::origin::Mutability::Param(_) => None,
                    },
                    infer_only: true,
                    default: None,
                    constraints: Vec::new(),
                }
            })
            .collect::<Vec<_>>();
        let source_types = semantic_types
            .iter()
            .map(|ty| {
                source_type_from_ty_with_origins(ty, &origin_names, &self.materialized_callables)
                    .map(|source| self.insert_origin_placeholders(source))
            })
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| {
                ComptimeError::NotComptime(
                    "type pack contains a type which cannot be materialized in source".to_string(),
                )
            })?;
        let mut type_pack_expansions = HashMap::new();
        type_pack_expansions.insert(binding.clone(), source_types.clone());
        let mut specialized_associated = associated.clone();
        for member in &mut specialized_associated {
            if names_pack(&member.value, &binding) {
                member.value.kind = ExprKind::TupleLit(
                    source_types
                        .iter()
                        .cloned()
                        .map(|ty| Expr::new(ExprKind::TypeValue(ty), member.value.span))
                        .collect(),
                );
            }
            for condition in &mut member.where_clauses {
                *condition = self.fold_pack_conformance_predicate(condition, &binding, types)?;
            }
        }
        // Conditional conformances on the source pack become unconditional
        // facts (or disappear) on the concrete implementation struct. Leaving
        // `Ts.values` attached after erasing the pack declaration would make the
        // checker reconstruct a dependency that no longer exists.
        let mut specialized_conforms = Vec::with_capacity(conforms.len());
        for conformance in conforms {
            let Some((_, condition)) = conformance_conditions
                .iter()
                .find(|(candidate, _)| candidate == conformance)
            else {
                specialized_conforms.push(conformance.clone());
                continue;
            };
            let folded = self.fold_pack_conformance_predicate(condition, &binding, types)?;
            match folded_constraint_truth(&folded)? {
                Some(true) => specialized_conforms.push(conformance.clone()),
                Some(false) => {}
                None => {
                    return Err(ComptimeError::NotComptime(format!(
                        "variadic struct '{orig}': conditional conformance '{conformance}' did not become concrete after specializing '*{binding}'"
                    )));
                }
            }
        }
        let specialized_where = where_clauses
            .iter()
            .map(|predicate| self.fold_pack_conformance_predicate(predicate, &binding, types))
            .collect::<Result<Vec<_>, _>>()?;
        if !specialized_where.is_empty() {
            let mut environment = self.top_consts.borrow().clone();
            environment.insert(binding.clone(), CtValue::Tuple(types.clone()));
            for predicate in &specialized_where {
                self.validate_specialized_where(predicate, &environment, orig)?;
            }
        }
        // Elaborate each method body with the pack bound, so comptime constructs
        // select/unroll against the concrete element types.
        let mut elaborated_methods = Vec::with_capacity(methods.len());
        let mut members = Vec::new();
        for method in methods {
            let template_body = traced_template_body(method);
            let mut method = method.clone();
            // An Int-indexed accessor unrolls per element below, as does any
            // other method whose one Int binder selects its `Ts[i]` result
            // (`def item[i: Int](self) -> Self.Ts[i]`); a type-keyed one
            // (`__getitem_param__[T: AnyType]`) is an ordinary generic method
            // specialized per call like any other.
            let dependent_index_accessor = match classify_ct_params(
                &method.type_params,
                &self.method_binder_owners.owner(orig, &method),
            )
            .as_slice()
            {
                [ParamDecl::Value { name, ty, .. }] => {
                    matches!(method.name.as_str(), "__getitem__" | "__getitem_param__")
                        || (**ty == Ty::Int
                            && method.has_self
                            && method.params.is_empty()
                            && method
                                .ret
                                .as_ref()
                                .is_some_and(|ret| indexes_pack_by(ret, &binding, name)))
                }
                _ => false,
            };
            let mut env = self.top_consts.borrow().clone();
            env.insert(binding.clone(), CtValue::Tuple(types.clone()));
            let mut subs = self.top_consts.borrow().clone();
            // The bound pack folds its runtime-position `TypeList` uses
            // (`Ts.length`, `Ts.contains[X]()`) during materialization.
            subs.insert(binding.clone(), CtValue::Tuple(types.clone()));
            subs.remove("self");
            for parameter in &method.params {
                subs.remove(&parameter.name);
            }
            // The source pack declaration is erased from a concrete variadic
            // struct. Rewrite every pack-index annotation before checking:
            // concrete indices select their element immediately, while a
            // method/callable binder such as `index` becomes the structural
            // `Self.element_types[index]` projection retained by checked HIR.
            for parameter in &mut method.type_params {
                if let Some(value_type) = &mut parameter.value_type {
                    self.fold_pack_index_annotation(value_type, &binding, &source_types, &env)?;
                }
                if let Some(callable) = &mut parameter.callable_bound {
                    self.fold_pack_index_annotation(callable, &binding, &source_types, &env)?;
                }
            }
            for parameter in &mut method.params {
                self.fold_pack_index_annotation(&mut parameter.ty, &binding, &source_types, &env)?;
            }
            if let Some(error) = &mut method.raises_type {
                self.fold_pack_index_annotation(error, &binding, &source_types, &env)?;
            }
            // Keep `Ts[i]` intact until the dependent-index accessor is
            // unrolled below. At this point `i` is not bound yet; eagerly
            // rewriting it to `Self.element_types[i]` would require every
            // user-defined variadic struct to manufacture Tuple's private
            // `element_types` associated member. Each unrolled accessor has
            // an `env_k` in which `i` is concrete, so the original annotation
            // can be folded directly to the selected element type there.
            if !dependent_index_accessor && let Some(ret) = &mut method.ret {
                self.fold_pack_index_annotation(ret, &binding, &source_types, &env)?;
            }
            // Availability clauses over the struct pack are just as dependent
            // as its conditional conformances. Fold their pack atoms now. A
            // false concrete clause removes the unavailable method; a true one
            // is erased. Any residual method-generic proposition remains for
            // ordinary checker specialization.
            let mut unavailable: Option<Expr> = None;
            let mut residual_clauses = Vec::new();
            for condition in method.where_clauses.drain(..) {
                let folded = self.fold_pack_conformance_predicate(&condition, &binding, types)?;
                match &folded.kind {
                    ExprKind::Bool(false) => {
                        unavailable = Some(condition);
                        break;
                    }
                    ExprKind::Bool(true) => {}
                    _ => residual_clauses.push(folded),
                }
            }
            if let Some(condition) = unavailable {
                // An unavailable method stays declared as a trap stub behind
                // a diagnostic clause: a call reports the violated clause
                // (upstream's note, spelled from the source clause) rather
                // than a missing member, and the stub body never runs.
                method.where_clauses = vec![unavailable_method_clause(&condition)];
                method.body = vec![shared_method_stub(
                    orig,
                    &method,
                    condition.span,
                    condition.syntax_id,
                )];
                members.push(TracedMember {
                    first_copy: true,
                    ..TracedMember::whole(elaborated_methods.len(), &method.name, condition.span)
                });
                elaborated_methods.push(method);
                continue;
            }
            method.where_clauses = residual_clauses;
            // A pack-typed runtime parameter (`var *args: *Ts`) becomes the
            // concrete `$pack[T0, ...]`; its element sequence is exposed in the
            // comptime env so `len(args)`/`args[i]`/`comptime for` evaluate
            // while the body is elaborated (mirrors the def-pack path).
            for parameter in &mut method.params {
                if matches!(&parameter.ty, Type::Named(name, _) if name.trim_start_matches('*') == binding)
                {
                    parameter.ty = Type::Named(
                        "$pack".to_string(),
                        source_types.iter().cloned().map(ParamArg::Type).collect(),
                    );
                    env.insert(parameter.name.clone(), CtValue::Tuple(types.clone()));
                }
            }
            // Tuple membership is source-generic, but each comparison is legal
            // only for elements whose concrete type equals the searched type.
            // Once `*Ts` is known, emit one ordinary overload per distinct
            // element type and resolve the `is_same_type` branches now. This
            // leaves no dependent/generic reconstruction for the checker or VM.
            if orig == "Tuple" && method.name == "__contains__" && !source_types.is_empty() {
                let [type_parameter] = method.type_params.as_slice() else {
                    return Err(ComptimeError::NotComptime(
                        "Tuple.__contains__ must have exactly one type parameter".to_string(),
                    ));
                };
                let parameter_name = type_parameter.name.trim_start_matches('*').to_string();
                let mut distinct = Vec::<(Type, CtValue)>::new();
                for ((source_type, semantic_type), value) in source_types
                    .iter()
                    .cloned()
                    .zip(semantic_types.iter())
                    .zip(types.iter())
                {
                    // Specialization erases the method type parameter, so its
                    // declaration bounds must be discharged now. Emitting a
                    // List-valued overload for `T: Equatable`, for example,
                    // would type-check a comparison the source method was never
                    // available to perform.
                    if type_parameter
                        .bounds
                        .iter()
                        .any(|bound| self.conformance.require(semantic_type, bound).is_err())
                    {
                        continue;
                    }
                    if !distinct
                        .iter()
                        .any(|(existing, _)| existing == &source_type)
                    {
                        distinct.push((source_type, value.clone()));
                    }
                }
                for (source_type, value) in distinct {
                    let mut overload = method.clone();
                    overload.type_params.clear();
                    for parameter in &mut overload.params {
                        substitute_source_type_binding(
                            &mut parameter.ty,
                            &parameter_name,
                            &source_type,
                        );
                    }
                    if let Some(ret) = &mut overload.ret {
                        substitute_source_type_binding(ret, &parameter_name, &source_type);
                    }
                    let mut overload_env = env.clone();
                    overload_env.insert(parameter_name.clone(), value.clone());
                    let elaborated = self
                        .block(&overload.body, &mut overload_env, true)
                        .map_err(|error| {
                            ComptimeError::NotComptime(format!(
                                "while specializing {orig}.{}: {error}",
                                overload.name
                            ))
                        })?;
                    // The erased parameter shadows a same-named module
                    // constant; its type positions bake out below.
                    let mut overload_subs = subs.clone();
                    overload_subs.remove(&parameter_name);
                    overload.body =
                        materialize_block(elaborated, &overload_subs, &self.struct_names);
                    // The erased parameter's type positions in the body
                    // (`rebind[T](...)`) bake out like a per-call clone's.
                    substitute_type_bindings_in_block(
                        &mut overload.body,
                        &HashMap::from([(parameter_name.clone(), source_type.clone())]),
                    );
                    if let Some(body) = template_body {
                        members.push(TracedMember {
                            types: vec![(parameter_name.clone(), source_type)],
                            ..TracedMember::whole(elaborated_methods.len(), &overload.name, body)
                        });
                    }
                    elaborated_methods.push(overload);
                }
                continue;
            }
            // The dependent-index accessor `def __getitem__[i: Int](self) ->
            // Ts[i]` cannot survive as one checked method (its return type
            // depends on the compile-time index), so it unrolls into one
            // concrete accessor per element — `__getitem__$k` with `i`
            // substituted and the `Ts[i]` annotation folded to that element.
            if dependent_index_accessor {
                let accessor_name = method.name.clone();
                let index_decls = classify_ct_params(
                    &method.type_params,
                    &self.method_binder_owners.owner(orig, &method),
                );
                let (
                    [
                        ParamDecl::Value {
                            name: index_name,
                            ty: index_ty,
                            ..
                        },
                    ],
                    true,
                    true,
                ) = (
                    index_decls.as_slice(),
                    method.has_self,
                    method.params.is_empty(),
                )
                else {
                    return Err(ComptimeError::NotComptime(format!(
                        "variadic struct '{orig}': a compile-time-parameterized {accessor_name} must take exactly one Int index parameter and only self"
                    )));
                };
                if **index_ty != Ty::Int {
                    return Err(ComptimeError::NotComptime(format!(
                        "variadic struct '{orig}': the {accessor_name} index parameter must be Int, got {index_ty}"
                    )));
                }
                for k in 0..source_types.len() {
                    let mut unrolled = method.clone();
                    unrolled.name = format!("{accessor_name}${k}");
                    unrolled.type_params = Vec::new();
                    let mut env_k = env.clone();
                    env_k.insert(index_name.clone(), CtValue::Int(k as i64));
                    let mut subs_k = subs.clone();
                    subs_k.insert(index_name.clone(), CtValue::Int(k as i64));
                    let elaborated =
                        self.block(&unrolled.body, &mut env_k, true)
                            .map_err(|error| {
                                ComptimeError::NotComptime(format!(
                                    "while specializing {orig}.{}: {error}",
                                    unrolled.name
                                ))
                            })?;
                    unrolled.body = materialize_block(elaborated, &subs_k, &self.struct_names);
                    if let Some(ret) = &mut unrolled.ret {
                        self.fold_pack_index_annotation(ret, &binding, &source_types, &env_k)?;
                    }
                    // Indexing private storage whose element is itself a
                    // reference reads through that stored handle. Its public
                    // result therefore carries the element's original origin,
                    // not a newly nested `ref[origin_of(self)] ref[...] T`.
                    if matches!(semantic_types[k], Ty::Ref(_)) {
                        unrolled.ret = Some(source_types[k].clone());
                    }
                    // A reference-returning accessor needs a stable receiver
                    // place. Rvalue Tuple subscripts and destructuring instead
                    // use a value-returning twin when the selected element is
                    // implicitly copyable. Keeping this as an ordinary method
                    // preserves nominal dispatch without manufacturing an
                    // origin for a temporary expression.
                    let value_accessor = if matches!(accessor_name.as_str(), "__getitem__" | "__getitem_param__")
                            && matches!(&unrolled.ret, Some(Type::Ref { .. }))
                            // A callable may be reached through a checked
                            // reference to live Tuple storage, but copying it
                            // out of an rvalue aggregate would turn the
                            // compiler-generated accessor into an escaping
                            // callable return.
                            && !matches!(
                                semantic_types[k],
                                Ty::Func { .. } | Ty::GenericFunc { .. } | Ty::Overload(_)
                            )
                            && self.specialization_type_is_implicitly_copyable(&semantic_types[k])
                    {
                        let mut value_accessor = unrolled.clone();
                        let value_name = if accessor_name == "__getitem_param__" {
                            "__getitem_param_value__"
                        } else {
                            "__getitem_value__"
                        };
                        value_accessor.name = format!("{value_name}${k}");
                        value_accessor.self_convention = None;
                        value_accessor.ret = match value_accessor.ret.take() {
                            Some(Type::Ref { referent, .. }) => Some(*referent),
                            _ => {
                                unreachable!("value-accessor gate requires a reference return")
                            }
                        };
                        Some(value_accessor)
                    } else {
                        None
                    };
                    // The reference accessor is its template's instance at
                    // this index; the value twin has no template of its own.
                    if let Some(body) = template_body {
                        members.push(TracedMember {
                            values: vec![(index_name.clone(), CtValue::Int(k as i64))],
                            ..TracedMember::whole(elaborated_methods.len(), &accessor_name, body)
                        });
                    }
                    elaborated_methods.push(unrolled);
                    if let Some(value_accessor) = value_accessor {
                        if let Some(body) = template_body {
                            members.push(TracedMember {
                                values: vec![(index_name.clone(), CtValue::Int(k as i64))],
                                ..TracedMember::whole(
                                    elaborated_methods.len(),
                                    &accessor_name,
                                    body,
                                )
                            });
                        }
                        elaborated_methods.push(value_accessor);
                    }
                }
                continue;
            }
            // A method with its own compile-time parameters: every
            // checker-discovered instantiation on this specialization mints
            // a clone with those parameters baked (`find$y3:Int`); the
            // template survives for signature resolution and the erased
            // path. A template body that only elaborates once its own
            // parameters are bound (a `comptime if` on `T`) becomes a trap
            // stub — every concrete call retargets to a clone.
            let specializable_method = method
                .type_params
                .iter()
                .any(|parameter| method_parameter_is_baked(parameter, &method.type_params));
            if specializable_method {
                let owner = mangle(orig, vals)?;
                let mut minted = HashSet::new();
                let binder_owner = self.method_binder_owners.owner(orig, &method);
                for request in self
                    .method_requests
                    .get(&owner)
                    .map_or(&[][..], Vec::as_slice)
                    .iter()
                    .filter(|request| request.selects(&method, orig, &self.method_binder_owners))
                {
                    let mut origin_binders = CloneOriginBinders::default();
                    let Some((values, bindings)) = self.method_request_values(
                        &binder_owner,
                        &method.type_params,
                        request.arguments(),
                        &mut origin_binders,
                    ) else {
                        continue;
                    };
                    let instance = mangle(&method.name, &values)?;
                    if !minted.insert(instance.clone()) {
                        continue;
                    }
                    // Constructor semantics key on the exact name `__init__`,
                    // so a constructor clone is a same-name overload selected
                    // by its concrete argument types; every other clone is
                    // retargeted by its mangled name.
                    let clone_name = if method.name == "__init__" {
                        method.name.clone()
                    } else {
                        instance
                    };
                    let mut clone = self
                        .specialize_method_clone(&method, clone_name, &bindings, &env, &subs)
                        .map_err(|error| {
                            ComptimeError::NotComptime(format!(
                                "while specializing {orig}.{}: {error}",
                                method.name
                            ))
                        })?;
                    clone
                        .type_params
                        .splice(0..0, origin_binders.params().iter().cloned());
                    if method.name == "__init__" {
                        clone.provenance = mojito_ast::ast::MethodProvenance::PerCallConstructor;
                    }
                    elaborated_methods.push(clone);
                }
                match self.block(&method.body, &mut env, true) {
                    Ok(elaborated) => {
                        method.body = materialize_block(elaborated, &subs, &self.struct_names);
                        if let Some(body) = template_body {
                            members.push(TracedMember::whole(
                                elaborated_methods.len(),
                                &method.name,
                                body,
                            ));
                        }
                    }
                    Err(_) => method.body = vec![unspecialized_method_stub(orig, &method)],
                }
                elaborated_methods.push(method);
                continue;
            }
            let elaborated = self.block(&method.body, &mut env, true).map_err(|error| {
                ComptimeError::NotComptime(format!(
                    "while specializing {orig}.{}: {error}",
                    method.name
                ))
            })?;
            method.body = materialize_block(elaborated, &subs, &self.struct_names);
            if let Some(body) = template_body {
                members.push(TracedMember::whole(
                    elaborated_methods.len(),
                    &method.name,
                    body,
                ));
            }
            elaborated_methods.push(method);
        }
        if orig == "Tuple" {
            self.append_tuple_transform_methods(
                &mut elaborated_methods,
                &semantic_types,
                template.span,
            );
        }
        let mangled = mangle(orig, vals)?;
        let mut spec = mk(
            StmtKind::Struct {
                name: mangled.clone(),
                decorators: decorators.clone(),
                type_params: retained_origin_parameters,
                conforms: specialized_conforms,
                callable_conformance: callable_conformance.clone(),
                conformance_conditions: Vec::new(),
                // Every source pack binder has been discharged above. The
                // constraint is a specialization precondition, not a residual
                // declaration predicate on the concrete clone.
                where_clauses: Vec::new(),
                fields: fields.clone(),
                associated: specialized_associated,
                methods: elaborated_methods,
                fieldwise_init: *fieldwise_init,
                template_shell: false,
            },
            template.span,
        );
        expand_pack_spreads_in_stmt(&mut spec, &type_pack_expansions);
        // Every specialization reuses the template's spans (correct provenance),
        // so checked facts keyed by source location would collide across
        // specializations of one template. Stamp each subtree with a unique
        // source tag — the mangled name layered on the template's module — and
        // give each unrolled dependent accessor (a clone of one source method)
        // its own tag on top, so their checked facts stay separate too.
        let tag = match &template.module {
            Some(module) => format!("{module}${mangled}"),
            None => mangled,
        };
        mojito_ast::ast::stamp_source(std::slice::from_mut(&mut spec), &tag);
        if let StmtKind::Struct { methods, .. } = &mut spec.kind {
            // Each per-element membership overload shares its name and body
            // spans with the others: its own tag tells their facts and
            // traces apart.
            let mut overloads = 0;
            for method in methods.iter_mut() {
                if orig == "Tuple" && method.name == "__contains__" {
                    mojito_ast::ast::stamp_source(
                        &mut method.body,
                        &format!("{tag}.__contains__${overloads}"),
                    );
                    overloads += 1;
                }
            }
            for method in methods {
                if method.name.starts_with("__getitem__$")
                    || method.name.starts_with("__getitem_param__$")
                    || method.name.starts_with("__getitem_value__$")
                    || method.name.starts_with("__getitem_param_value__$")
                {
                    mojito_ast::ast::stamp_source(
                        &mut method.body,
                        &format!("{tag}.{}", method.name),
                    );
                }
            }
        }
        // The subtree is stamped; disarm `elaborate`'s uniform module re-stamp
        // (it would collapse the per-accessor tags back into one).
        spec.module = None;
        self.trace_struct_members(
            &spec,
            &members,
            &StructTemplate {
                name: orig,
                module: template.module.as_deref(),
                values: Vec::new(),
                packs: vec![(binding, source_types)],
            },
        );
        Ok(spec)
    }

    /// Emit closed-world, fully concrete Tuple transforms as ordinary methods.
    /// The discovery checker has already recorded every result Tuple type. No
    /// dependent pack transform survives into checking or MIR, and execution is
    /// normal constructor/method dispatch rather than a VM tuple intrinsic.
    pub(super) fn append_tuple_transform_methods(
        &self,
        methods: &mut Vec<mojito_ast::ast::Method>,
        left: &[Ty],
        span: Span,
    ) {
        let Some((_, transforms)) = self
            .tuple_transforms
            .iter()
            .find(|(elements, _)| elements == left)
        else {
            return;
        };
        for transform in transforms {
            match transform {
                TupleTransformRequest::Reverse => {
                    let reversed = left.iter().rev().cloned().collect::<Vec<_>>();
                    if !self
                        .tuple_universe
                        .iter()
                        .any(|elements| elements == &reversed)
                    {
                        continue;
                    }
                    let target = tuple_specialization_symbol(&reversed);
                    let arguments = (0..left.len())
                        .rev()
                        .map(|index| tuple_storage_element("self", index, true, span))
                        .collect();
                    methods.push(tuple_transform_method(
                        "reverse",
                        Some(ArgConvention::Deinit),
                        Vec::new(),
                        target,
                        arguments,
                        span,
                    ));
                }
                TupleTransformRequest::Concat(right) => {
                    let mut result = left.to_vec();
                    result.extend(right.iter().cloned());
                    if !self
                        .tuple_universe
                        .iter()
                        .any(|elements| elements == &result)
                    {
                        continue;
                    }
                    let right_symbol = tuple_specialization_symbol(right);
                    let target = tuple_specialization_symbol(&result);
                    let mut arguments = (0..left.len())
                        .map(|index| tuple_storage_element("self", index, true, span))
                        .collect::<Vec<_>>();
                    arguments.extend(
                        (0..right.len())
                            .map(|index| tuple_storage_element("other", index, true, span)),
                    );
                    methods.push(tuple_transform_method(
                        "concat",
                        Some(ArgConvention::Deinit),
                        vec![FnParam {
                            name: "other".to_string(),
                            ty: Type::Named(right_symbol, Vec::new()),
                            default: None,
                            kind: ParamKind::Regular,
                            convention: Some(ArgConvention::Deinit),
                            origin: None,
                        }],
                        target,
                        arguments,
                        span,
                    ));
                }
            }
        }
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
    /// parameters are all plain type parameters specializes here; value
    /// parameters, retained origin binders, and callable-bounded parameters
    /// keep the erased path.
    #[allow(
        clippy::unnecessary_wraps,
        reason = "TODO: drop the Result once callers stop using ?"
    )]
    fn generate_instance_clones(
        &self,
        name: &str,
        values: &[CtValue],
        keyed: &HashSet<String>,
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
        let mut origin_binders = CloneOriginBinders::default();
        for (parameter, value) in type_params.iter().zip(values) {
            let CtValue::Type(ty) = value else {
                return Ok(InstanceClones::default());
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
        let receiver = Type::Named(
            name.to_string(),
            bindings
                .iter()
                .filter_map(|binding| binding.source.clone().map(ParamArg::Type))
                .collect(),
        );
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
            clones.extend(self.per_call_method_clones(
                name,
                method,
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
            ));
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
            let Ok(mut clone) =
                self.specialize_method_clone(method, clone_name, &bindings, &consts, &consts)
            else {
                continue;
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
        Ok(InstanceClones {
            clones,
            field_types,
            withheld,
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
    /// the method's binders.
    pub(super) fn per_call_method_clones(
        &self,
        template: &str,
        method: &Method,
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
            let Ok(mut clone) =
                self.specialize_method_clone(method, clone_name, &bindings, consts, consts)
            else {
                continue;
            };
            clone.where_clauses.clear();
            clone.self_ty = receiver.cloned();
            let first_own = base_binders.map_or(0, |base| base.params().len());
            clone
                .type_params
                .splice(0..0, binders.params()[first_own..].iter().cloned());
            if let Some(owner) = owner {
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

    /// Complete the traces of the per-call leaves and clones minted into a
    /// struct specialized whole, from `from` on: each leaf's source tag is
    /// the specialization's, which stamping gave its body, and the struct's
    /// folded values precede the leaf's own bindings.
    fn restamp_leaf_traces(&self, spec: &Stmt, from: usize, folded: &[(String, CtValue)]) {
        let StmtKind::Struct { name, methods, .. } = &spec.kind else {
            return;
        };
        let mut traces = self.method_traces.borrow_mut();
        for trace in traces.iter_mut().skip(from) {
            let Some(module) = methods
                .iter()
                .find(|method| method.name == trace.clone_name)
                .and_then(|method| method.body.first()?.module.clone())
            else {
                continue;
            };
            debug_assert_eq!(trace.owner, *name);
            trace.clone_module = module;
            trace.value_bindings.splice(0..0, folded.iter().cloned());
        }
    }

    /// Record how each member of a struct specialized whole came from its
    /// template: `members` lists each traced member ([`TracedMember`]),
    /// a shared trap stub among them. Any other member the
    /// specializer stubbed or synthesized is not listed, nor is a per-call
    /// clone minted inside the specialization.
    fn trace_struct_members(
        &self,
        spec: &Stmt,
        members: &[TracedMember],
        template: &StructTemplate<'_>,
    ) {
        let StmtKind::Struct { name, methods, .. } = &spec.kind else {
            return;
        };
        let mut traces = self.method_traces.borrow_mut();
        for traced in members {
            let Some(member) = methods.get(traced.index) else {
                continue;
            };
            let Some((first, module)) = member
                .body
                .first()
                .and_then(|first| Some((first.span, first.module.clone()?)))
            else {
                continue;
            };
            traces.push(super::MethodInstanceTrace {
                owner: name.clone(),
                template_owner: template.name.to_string(),
                owner_module: template.module.map(str::to_string),
                clone_name: member.name.clone(),
                clone_module: module,
                template_name: traced.template_name.clone(),
                body: traced.body,
                clone_body: first,
                type_bindings: traced.types.clone(),
                value_bindings: if traced.first_copy {
                    Vec::new()
                } else {
                    template
                        .values
                        .iter()
                        .chain(&traced.values)
                        .cloned()
                        .collect()
                },
                pack_bindings: if traced.first_copy {
                    Vec::new()
                } else {
                    template.packs.clone()
                },
                first_copy_template: traced.first_copy,
            });
        }
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
        clone.body = materialize_block(elaborated, &clone_subs, &self.struct_names);
        if !type_pack_expansions.is_empty() {
            expand_pack_spreads_in_function_body(
                &mut clone.body,
                &clone.params,
                &type_pack_expansions,
            );
        }
        substitute_type_bindings_in_block(&mut clone.body, &type_bindings);
        Ok(clone)
    }

    pub(super) fn fold_pack_conformance_predicate(
        &self,
        expression: &Expr,
        binding: &str,
        elements: &[CtValue],
    ) -> Result<Expr, ComptimeError> {
        let with_kind = |kind| {
            let mut folded = expression.clone();
            folded.kind = kind;
            folded
        };
        match &expression.kind {
            ExprKind::TupleLit(diagnostic_elements) => Ok(with_kind(ExprKind::TupleLit(
                diagnostic_elements
                    .iter()
                    .map(|element| self.fold_pack_conformance_predicate(element, binding, elements))
                    .collect::<Result<Vec<_>, _>>()?,
            ))),
            ExprKind::Call {
                name, args, kwargs, ..
            } if name == "conforms_to" && kwargs.is_empty() && args.len() == 2 => {
                let pack_matches = matches!(
                    &args[0].kind,
                    ExprKind::Member { object, field }
                        if field == "values" && names_pack(object, binding)
                );
                if !pack_matches {
                    return Ok(expression.clone());
                }
                let Some(trait_names) = mojito_ast::ast::trait_conjunction_names(&args[1]) else {
                    return Err(ComptimeError::NotComptime(
                        "conforms_to on a type pack requires a trait name".to_string(),
                    ));
                };
                let satisfied = elements.iter().all(|element| match element {
                    CtValue::Type(ty) => trait_names.iter().all(|trait_name| {
                        self.conformance
                            .require(ty, mojito_ast::ast::canonical_trait_name(trait_name))
                            .is_ok()
                    }),
                    _ => false,
                });
                Ok(with_kind(ExprKind::Bool(satisfied)))
            }
            // Upstream's TypeList spelling on the pack itself:
            // `Ts.all_conforms_to[Trait]()`, `Ts.contains[T]()`,
            // `Ts.all[Pred]()` / `Ts.any[Pred]()`, with `Self.Ts` accepted.
            ExprKind::Invoke {
                callee,
                param_args,
                args,
                kwargs,
            } if args.is_empty() && kwargs.is_empty() => {
                let ExprKind::Member { object, field } = &callee.kind else {
                    return Ok(expression.clone());
                };
                if !names_pack(object, binding)
                    || !matches!(
                        field.as_str(),
                        "all_conforms_to" | "contains" | "all" | "any"
                    )
                {
                    return Ok(expression.clone());
                }
                let receiver = CtValue::Tuple(elements.to_vec());
                let value =
                    self.eval_typelist_method(&receiver, field, param_args, &HashMap::new())?;
                Ok(with_kind(ExprKind::Bool(
                    value.as_bool("pack conformance predicate")?,
                )))
            }
            ExprKind::Prefix(PrefixOp::Not, operand) => {
                let operand = self.fold_pack_conformance_predicate(operand, binding, elements)?;
                match operand.kind {
                    ExprKind::Bool(value) => Ok(with_kind(ExprKind::Bool(!value))),
                    _ => Ok(with_kind(ExprKind::Prefix(
                        PrefixOp::Not,
                        Box::new(operand),
                    ))),
                }
            }
            ExprKind::Infix(op @ (InfixOp::And | InfixOp::Or), left, right) => {
                let left = self.fold_pack_conformance_predicate(left, binding, elements)?;
                let right = self.fold_pack_conformance_predicate(right, binding, elements)?;
                match (op, &left.kind, &right.kind) {
                    (InfixOp::And, ExprKind::Bool(false), _)
                    | (InfixOp::And, _, ExprKind::Bool(false)) => {
                        Ok(with_kind(ExprKind::Bool(false)))
                    }
                    (InfixOp::And, ExprKind::Bool(true), _) => Ok(right),
                    (InfixOp::And, _, ExprKind::Bool(true)) => Ok(left),
                    (InfixOp::Or, ExprKind::Bool(true), _)
                    | (InfixOp::Or, _, ExprKind::Bool(true)) => Ok(with_kind(ExprKind::Bool(true))),
                    (InfixOp::Or, ExprKind::Bool(false), _) => Ok(right),
                    (InfixOp::Or, _, ExprKind::Bool(false)) => Ok(left),
                    _ => Ok(with_kind(ExprKind::Infix(
                        *op,
                        Box::new(left),
                        Box::new(right),
                    ))),
                }
            }
            _ => Ok(expression.clone()),
        }
    }

    /// Fold a dependent pack-element annotation `Ts[expr]` (with `expr`
    /// evaluable in `env`, e.g. the unrolled accessor's index) to the concrete
    /// element type it selects.
    pub(super) fn fold_pack_index_annotation(
        &self,
        ty: &mut Type,
        binding: &str,
        elements: &[Type],
        env: &HashMap<String, CtValue>,
    ) -> Result<(), ComptimeError> {
        match ty {
            Type::Named(name, arguments) => {
                if name.trim_start_matches('*') == binding
                    && let [ParamArg::Value(index)] = arguments.as_slice()
                {
                    if let Ok(index_value) = self.eval(index, env) {
                        let index_value = index_value.as_int("pack index")?;
                        let element = elements.get(index_value as usize).ok_or_else(|| {
                            ComptimeError::BadArithmetic(format!(
                                "pack index {index_value} out of range for '{binding}' of length {}",
                                elements.len()
                            ))
                        })?;
                        *ty = element.clone();
                    } else {
                        *ty = Type::IndexedProjection {
                            base: Box::new(Type::Assoc {
                                base: Box::new(Type::SelfType),
                                name: "element_types".to_string(),
                                args: Vec::new(),
                            }),
                            index: Box::new(materialize_expression(index, env)),
                        };
                    }
                    return Ok(());
                }
                for argument in arguments {
                    if let ParamArg::Type(inner) = argument {
                        self.fold_pack_index_annotation(inner, binding, elements, env)?;
                    }
                }
                Ok(())
            }
            Type::Assoc { base, .. } => {
                self.fold_pack_index_annotation(base, binding, elements, env)
            }
            Type::IndexedProjection { base, index } => {
                // `Self.Ts[i]` is upstream's qualified spelling of the bare
                // `Ts[i]` pack index; normalize it so one fold handles both.
                if let Type::SelfParam(name) = base.as_ref()
                    && name.trim_start_matches('*') == binding
                {
                    let mut folded =
                        Type::Named(name.clone(), vec![ParamArg::Value((**index).clone())]);
                    self.fold_pack_index_annotation(&mut folded, binding, elements, env)?;
                    *ty = folded;
                    return Ok(());
                }
                self.fold_pack_index_annotation(base, binding, elements, env)?;
                **index = materialize_expression(index, env);
                Ok(())
            }
            Type::Func {
                type_params,
                params,
                ret,
                raises_type,
                ..
            } => {
                // The contract's own binders (`def[index: Int](Ts[index])`)
                // shadow same-named outer bindings throughout its signature.
                let mut own = env.clone();
                for parameter in type_params.iter() {
                    own.remove(parameter.name.trim_start_matches('*'));
                }
                let env = &own;
                for parameter in type_params {
                    if let Some(value_type) = &mut parameter.value_type {
                        self.fold_pack_index_annotation(value_type, binding, elements, env)?;
                    }
                    if let Some(callable) = &mut parameter.callable_bound {
                        self.fold_pack_index_annotation(callable, binding, elements, env)?;
                    }
                }
                for param in params {
                    self.fold_pack_index_annotation(&mut param.ty, binding, elements, env)?;
                }
                self.fold_pack_index_annotation(ret, binding, elements, env)?;
                if let Some(error) = raises_type {
                    self.fold_pack_index_annotation(error, binding, elements, env)?;
                }
                Ok(())
            }
            Type::Ref { referent, .. } => {
                self.fold_pack_index_annotation(referent, binding, elements, env)
            }
            Type::Int
            | Type::UInt
            | Type::Bool
            | Type::StringLiteral
            | Type::Float64
            | Type::None
            | Type::SelfParam(_)
            | Type::SelfType
            | Type::MaterializedCallable(_) => Ok(()),
        }
    }

    /// Test implicit-copyability while generated Tuple specializations are
    /// still being ordered. Their declarations may not yet be registered in
    /// the conformance environment, so inspect nested Tuple elements instead
    /// of trusting an unresolved conditional conformance on the outer shell.
    fn specialization_type_is_implicitly_copyable(&self, ty: &Ty) -> bool {
        if let Some(elements) = tuple_elements(ty) {
            return elements
                .iter()
                .all(|element| self.specialization_type_is_implicitly_copyable(element));
        }
        self.conformance.require(ty, "ImplicitlyCopyable").is_ok()
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

/// The body of a generic template method that cannot elaborate until its
/// own parameters are bound: a runtime trap. Every concrete call retargets
/// to a per-call clone, so the stub only runs through the erased path.
/// The diagnostic `where (False, "…")` clause carried by a method whose
/// pack-dependent availability clause `condition` folded to `False` for
/// this specialization (see the unavailable-method branch of the struct
/// specializer): the message is upstream's note naming the source clause.
fn unavailable_method_clause(condition: &Expr) -> Expr {
    let span = condition.span;
    let message = super::unparse::violated_constraint_message(condition);
    Expr::new(
        ExprKind::TupleLit(vec![
            Expr::new(ExprKind::Bool(false), span),
            Expr::new(ExprKind::Str(message), span),
        ]),
        span,
    )
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
/// compile-time construct folds only per instance), one that holds a
/// construct only an instance lowers, and one whose erased body reaches a
/// compile-time-keyed stub. Every other method's template serves each
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
                    || holds_instance_construct(&method.body)
                    || stub_reaching.contains(&super::method_owner(template, &method.name)))
        })
        .map(|method| method.name.clone())
        .collect()
}

/// Whether a method body holds a construct only an instance can lower: a
/// nested `def` or a lambda, whose lifted body is cloned per instance.
fn holds_instance_construct(body: &[Stmt]) -> bool {
    struct Finder {
        found: bool,
    }

    impl mojito_ast::visit::Visitor for Finder {
        fn visit_stmt(&mut self, statement: &Stmt) {
            self.found |= matches!(&statement.kind, StmtKind::Def { .. });
        }

        fn visit_expr(&mut self, expr: &Expr) {
            self.found |= matches!(&expr.kind, ExprKind::Lambda { .. });
        }
    }

    let mut finder = Finder { found: false };
    mojito_ast::visit::walk_block(&mut finder, body);
    finder.found
}

/// Whether `body` is [`unspecialized_method_stub`]'s trap.
fn is_unspecialized_method_stub(body: &[Stmt]) -> bool {
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
    let span = method
        .body
        .first()
        .map_or(mojito_common::token::DUMMY_SPAN, |statement| statement.span);
    mk(
        StmtKind::Expr(Expr::new(
            ExprKind::Call {
                name: "_mojito_abort".to_string(),
                param_args: Vec::new(),
                args: vec![Expr::new(
                    ExprKind::Str(format!("{owner}.{}{METHOD_STUB_REASON}", method.name)),
                    span,
                )],
                kwargs: Vec::new(),
            },
            span,
        )),
        span,
    )
}

/// The shell of a variadic struct template (see
/// `StmtKind::Struct::template_shell`).
pub(super) fn template_shell(template: &Stmt) -> Stmt {
    let mut shell = template.clone();
    if let StmtKind::Struct {
        type_params,
        conformance_conditions,
        where_clauses,
        fields,
        associated,
        methods,
        template_shell,
        ..
    } = &mut shell.kind
    {
        conformance_conditions.clear();
        where_clauses.clear();
        // A variadic template's members resolve symbolically over its pack,
        // so its shell keeps them: the discovery check types a construction
        // of the shell — inferring the pack from the constructor it selects —
        // and the members of the instance that construction produces. A
        // struct-valued-parameter template has no symbolic member form.
        if !type_params
            .iter()
            .any(|parameter| parameter.name.starts_with('*'))
        {
            fields.clear();
            associated.clear();
        }
        for method in methods.iter_mut() {
            method.body.clear();
        }
        *template_shell = true;
    }
    shell
}

/// The trap stub every specialization of a template method shares:
/// `unspecialized_method_stub` spanned at `span`, each node identified by an
/// identity derived from `parent`. An unavailable member's stub is spanned
/// and derived at the availability clause that folded false; a SIMD-keyed
/// method's at its struct and its own first statement. Every specialization
/// thereby holds the same stub, identities and all.
fn shared_method_stub(
    owner: &str,
    method: &Method,
    span: Span,
    parent: mojito_common::token::SyntaxId,
) -> Stmt {
    use mojito_common::token::SyntaxId;
    let mut stub = unspecialized_method_stub(owner, method);
    stub.span = span;
    stub.syntax_id = SyntaxId::derived(parent, 0);
    if let StmtKind::Expr(call) = &mut stub.kind {
        call.span = span;
        call.syntax_id = SyntaxId::derived(stub.syntax_id, 0);
        if let ExprKind::Call { args, .. } = &mut call.kind {
            for (ordinal, argument) in (0..).zip(args.iter_mut()) {
                argument.span = span;
                argument.syntax_id = SyntaxId::derived(call.syntax_id, ordinal);
            }
        }
    }
    stub
}

/// The template of a struct specialized whole, and what its compile-time
/// parameters became: folded values, or a type pack's source element types.
struct StructTemplate<'a> {
    name: &'a str,
    module: Option<&'a str>,
    values: Vec<(String, CtValue)>,
    packs: Vec<(String, Vec<Type>)>,
}

/// A member of a struct specialized whole that instantiates one template
/// method: its position in the specialization's method list, the template
/// method's name and the first statement of its body, and what the member
/// baked beyond the struct's own parameters — an unrolled accessor's index
/// (`__getitem_param__$k`, and its value twin), or the element type a
/// per-element overload (`Tuple.__contains__`) erased.
///
/// A member whose body is a trap stub the specializer shaped itself
/// (`first_copy`: an unavailable member's, or a SIMD-keyed method's) shares
/// it across every specialization, its first checked copy being the
/// template; it bakes nothing, and `body` is that body's own first
/// statement.
struct TracedMember {
    index: usize,
    template_name: String,
    body: Span,
    values: Vec<(String, CtValue)>,
    types: Vec<(String, Type)>,
    first_copy: bool,
}

impl TracedMember {
    /// A member that bakes nothing of its own.
    fn whole(index: usize, template_name: &str, body: Span) -> Self {
        Self {
            index,
            template_name: template_name.to_string(),
            body,
            values: Vec::new(),
            types: Vec::new(),
            first_copy: false,
        }
    }
}

/// The first statement of a template method's body, which identifies it as a
/// template, when a specialization of the whole struct traces the member it
/// becomes: a synthesized body (no source provenance) and an empty one have
/// no template.
/// Whether a result annotation selects an element of the struct's pack
/// `binding` by the method binder `index` (`Ts[i]`, `Self.Ts[i]`, or either
/// under `ref`).
fn indexes_pack_by(ty: &Type, binding: &str, index: &str) -> bool {
    let by_index =
        |expression: &Expr| matches!(&expression.kind, ExprKind::Identifier(name) if name == index);
    match ty {
        Type::Named(name, arguments) => {
            (name.trim_start_matches('*') == binding
                && matches!(arguments.as_slice(), [ParamArg::Value(expression)] if by_index(expression)))
                || arguments.iter().any(|argument| {
                    matches!(argument, ParamArg::Type(inner) if indexes_pack_by(inner, binding, index))
                })
        }
        Type::IndexedProjection { base, index: expression } => {
            matches!(base.as_ref(), Type::SelfParam(name) if name.trim_start_matches('*') == binding)
                && by_index(expression)
        }
        Type::Ref { referent, .. } => indexes_pack_by(referent, binding, index),
        _ => false,
    }
}

fn traced_template_body(method: &Method) -> Option<Span> {
    method
        .body
        .first()
        .filter(|first| first.module.is_some())
        .map(|first| first.span)
}
