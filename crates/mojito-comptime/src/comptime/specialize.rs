//! Monomorphization and specialization generation: `monomorphize` and `def`
//! specialization synthesis.
//! Extracted from `comptime.rs`; see `docs/symbol-map.md`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_ast::ast::Method;

impl Elab<'_> {
    /// Specialize every comptime-dependent generic template against the value
    /// arguments at its call sites, replacing each template with its concrete
    /// specializations (which have their `comptime if`/`for` resolved).
    pub(super) fn monomorphize(&self, program: Vec<Stmt>) -> Result<Elaborated, ComptimeError> {
        let consts = self.top_consts.borrow().clone();
        let mut mono = Mono::default();
        let mut program = program;
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
        // Rewrite call sites in every non-template statement, seeding the
        // worklist. A bound-generic template's body is live code whether the
        // template is retained or dropped, so it is scanned like any other
        // statement (its own symbolic-argument calls soft-retain their
        // callees); a value-pack template is replaced wholesale below.
        for stmt in &mut program {
            if let StmtKind::Def { name, .. } | StmtKind::Struct { name, .. } = &stmt.kind
                && self.specializable.contains_key(name)
                && !self.bound_generics.contains(name)
            {
                continue;
            }
            mono.in_bundled =
                mojito_checker::checker::is_bundled_module_source(stmt.module.as_deref());
            self.mono_stmt(stmt, &consts, &mut mono)?;
        }
        mono.in_bundled = false;
        self.drain_specialization_jobs(&mut mono, &consts)?;
        // Rebuild the program, replacing each template with its specializations at
        // the template's original position. Specializations are emitted in reverse
        // generation order so a callee is defined before its caller (the checker
        // binds names sequentially, without forward references).
        let mut out = Vec::with_capacity(program.len());
        for stmt in program {
            let template_name = match &stmt.kind {
                StmtKind::Def { name, .. } | StmtKind::Struct { name, .. }
                    if self.specializable.contains_key(name) =>
                {
                    name.clone()
                }
                _ => {
                    out.push(stmt);
                    continue;
                }
            };
            let generated = mono.generated.remove(&template_name);
            // A value-pack template either specialized or is a dead generic,
            // dropped either way. A bound-generic template always survives,
            // so its body keeps the abstract pre-check whether or not it was
            // instantiated: what a parametric body demands of its own
            // parameters is a fact about the template, not about the
            // arguments some call happened to supply. A retained template
            // precedes its specializations: a clone may still reference the
            // template abstractly (an inferred recursive call), and the
            // checker binds top-level names sequentially.
            if self.bound_generics.contains(&template_name) {
                out.push(stmt);
            }
            if let Some(mut specs) = generated {
                specs.reverse();
                out.extend(specs);
            }
        }
        Ok(Elaborated {
            program: out,
            def_traces: Vec::new(),
            generated: super::GeneratedDeclarations::default(),
            ctfe_template_stats: mojito_checked::templates::TemplateStats::default(),
        })
    }

    /// Whether the template of the `def` `name` serves every closed call of
    /// it, so no call mints a clone: the `def` is a plain trait-bound one
    /// whose compile-time parameters are all served binders. Whatever its
    /// body holds, the elaborator below MIR instantiates the template's MIR
    /// for each call, and a call's transfer summary names its loans by the
    /// stored type.
    pub(super) fn template_serves_def(&self, name: &str, template: &Stmt) -> bool {
        let StmtKind::Def { type_params, .. } = &template.kind else {
            return false;
        };
        self.bound_generics.contains(name) && template_serves_binders(type_params, name)
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
            let template = self.specializable[job.orig.as_str()];
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
        let mut value_pack_values: HashMap<String, Vec<CtValue>> = HashMap::new();
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
                ParamDecl::Value { name, variadic, .. } => {
                    if *variadic && let CtValue::Tuple(elements) = v {
                        value_pack_values.insert(binding.clone(), elements.clone());
                    }
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
        let mut elaborated = self.block(body, &mut env, true)?;
        spell_value_pack_reads(&mut elaborated, &value_pack_values);
        let mut final_body = materialize_block(
            elaborated,
            &subs,
            &self.struct_names,
            &self.applied_constants(),
            &|_| None,
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
            &self.pack_defs,
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
pub(super) fn intrinsic_statement(callee: &str, message: String, method: &Method) -> Stmt {
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
