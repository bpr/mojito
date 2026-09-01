//! Monomorphization and specialization generation: `monomorphize`, tuple-spec
//! ordering, and `def`/`struct` specialization synthesis.
//! Extracted from `comptime.rs`; see `docs/symbol-map.md`.

use super::*;
use crate::types::tuple_elements;

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

impl<'a> Elab<'a> {
    /// Specialize every comptime-dependent generic template against the value
    /// arguments at its call sites, replacing each template with its concrete
    /// specializations (which have their `comptime if`/`for` resolved).
    pub(super) fn monomorphize(
        &self,
        program: Vec<Stmt>,
        tuple_requests: &[TupleSpecializationRequest],
        tstring_requests: &[TStringSpecializationRequest],
        def_requests: &[DefSpecializationRequest],
    ) -> Result<Vec<Stmt>, ComptimeError> {
        if self.specializable.is_empty() && tuple_requests.is_empty() && tstring_requests.is_empty()
        {
            return Ok(program);
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
                    vals,
                    site: request
                        .occurrence()
                        .map(|span| match &span.source {
                            Some(source) => {
                                format!("{source}:{}..{}", span.span.0, span.span.1)
                            }
                            None => format!("bytes {}..{}", span.span.0, span.span.1),
                        })
                        .unwrap_or_else(|| "a checked Tuple type".to_string()),
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
            // A concrete TString owns every textual snapshot it stores.
            // StringLiteral remains a borrowed, drop-inert descriptor
            // everywhere else; specialize textual fields to nominal String
            // so the ordinary struct lifecycle owns their runtime buffers.
            let storage_elements = request
                .elements()
                .iter()
                .map(|element| {
                    if matches!(element, Ty::StringLiteral) {
                        Ty::Struct(crate::symbol::STDLIB_STRING_STRUCT.to_string(), Vec::new())
                    } else {
                        element.clone()
                    }
                })
                .collect::<Vec<_>>();
            let vals = tuple_specialization_values(&storage_elements);
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
        // Checker-discovered inferred bound-generic applications. Seeding only
        // records each occurrence's target; the Job queues lazily at the
        // consult hit in `mono_expr`, so a drifted request produces no dead
        // clone and a never-matched request leaves its template correctly
        // retained. The compiler already resolved occurrence conflicts, so a
        // duplicate here keeps the first target (defensive).
        for request in def_requests {
            let callee = request.callee();
            // A scalar-range request targets a DType-value-param struct
            // template rather than a bound-generic def: record the
            // constructor-rewrite target consumed by `mono_expr`'s
            // `range(...)` occurrence. The Job queues lazily at the rewrite,
            // like the def targets below.
            if self.struct_template(callee) {
                if let [TyArg::Val(value @ CtValue::Dtype(_))] = request.arguments() {
                    mono.range_call_targets
                        .entry(request.occurrence().clone().without_syntax())
                        .or_insert_with(|| (callee.to_string(), vec![value.clone()]));
                }
                continue;
            }
            if !self.bound_generics.contains(callee) {
                continue;
            }
            let Some(template) = self.specializable.get(callee) else {
                continue;
            };
            let Some(vals) = self.def_request_values(template, request.arguments()) else {
                continue;
            };
            mono.def_call_targets
                .entry(request.occurrence().clone())
                .or_insert_with(|| DefCallTarget {
                    template: callee.to_string(),
                    vals,
                });
        }
        // Rewrite call sites in every non-template statement, seeding the
        // worklist. A bound-generic template's body is live code whether the
        // template is retained or dropped, so it is scanned like any other
        // statement (its own symbolic-argument calls soft-retain their
        // callees); comptime-class templates are replaced wholesale below.
        for stmt in program.iter_mut() {
            if let StmtKind::Def { name, .. } | StmtKind::Struct { name, .. } = &stmt.kind
                && self.specializable.contains_key(name)
                && !self.bound_generics.contains(name)
            {
                continue;
            }
            self.mono_stmt(stmt, &consts, &mut mono)?;
        }
        // Drain the worklist, generating each requested specialization and scanning
        // its body for further (e.g. recursive) instantiations.
        while let Some(job) = mono.queue.pop_front() {
            self.burn().map_err(|_| {
                ComptimeError::NotComptime(format!(
                    "specialization quota exceeded while instantiating '{}' requested at {}; possible unbounded generic recursion",
                    mangle(&job.orig, &job.vals), job.site
                ))
            })?;
            let mut spec = match &self.specializable[&job.orig].kind {
                StmtKind::Struct { type_params, .. }
                    if !classify_ct_params(type_params)
                        .iter()
                        .any(|decl| matches!(decl, ParamDecl::Type { variadic: true, .. })) =>
                {
                    self.generate_value_struct_spec(&job.orig, &job.vals)?
                }
                StmtKind::Struct { .. } => self.generate_struct_spec(&job.orig, &job.vals)?,
                _ => self.generate_def_spec(
                    self.specializable[&job.orig],
                    &job.orig,
                    job.output_name.clone(),
                    &job.vals,
                )?,
            };
            // TString's public specialization identity describes its source
            // segments, while its concrete storage pack upgrades textual
            // elements to owning nominal String. Preserve the checker-picked
            // public symbol instead of remangling from that private ABI pack.
            if job.orig == "TString"
                && let StmtKind::Struct { name, .. } = &mut spec.kind
            {
                *name = job.output_name.clone();
            }
            match &mut spec.kind {
                StmtKind::Def { params, body, .. } => {
                    self.mono_function_body(body, params, &consts, &mut mono)?
                }
                // A struct specialization is fully concrete; walk its members for
                // further template uses (nested instantiations, recursive packs).
                StmtKind::Struct { .. } => self.mono_stmt(&mut spec, &consts, &mut mono)?,
                _ => {}
            }
            // Scan while the parameter still carries its `$pack[T0, ...]`
            // identity: a whole-pack specialization may forward the collector
            // through another generic call. Select the regular Tuple ABI only
            // after all such calls have been rewritten.
            if job.whole_pack_abi {
                select_top_level_whole_pack_abi(&mut spec)?;
            }
            mono.generated.entry(job.orig).or_default().push(spec);
        }
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
            let monomorphized = generated.is_some();
            // A comptime-class template either specialized or is a dead
            // generic, dropped either way. A bound-generic template survives
            // while any reference stays on its abstract path — and when it has
            // no references at all, keeping the uninstantiated body's
            // Mojo-style pre-check. A retained template precedes its
            // specializations: a clone may still reference the template
            // abstractly (an inferred recursive call), and the checker binds
            // top-level names sequentially.
            if self.bound_generics.contains(&template_name)
                && (mono.retained.contains(&template_name) || !monomorphized)
            {
                out.push(stmt);
            }
            if let Some(mut specs) = generated {
                specs.reverse();
                if template_name == "Tuple" {
                    specs = self.order_tuple_specializations(specs)?;
                }
                out.extend(specs);
            }
        }
        Ok(out)
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
            .filter(|parameter| classify_ct_param(parameter, type_params).is_some())
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
            let Some(decl) = classify_ct_param(tp, type_params) else {
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
                            CtValue::Type(ty) => source_type_from_ty(ty),
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
        debug_assert!(values.next().is_none());
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
        let mut final_body = materialize_block(elaborated, &subs);
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
            None => output_name,
        };
        crate::ast::stamp_source(std::slice::from_mut(&mut specialization), &tag);
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
            || format!("'{display_name}' where clause evaluated to False"),
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
    /// retain Origin/`mut` binders as TypeParams (the specialization stays
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
        } = &template.kind
        else {
            return Err(ComptimeError::NotComptime(format!(
                "specialization registry entry '{orig}' is not a struct"
            )));
        };
        let mut kept_type_params = Vec::new();
        let mut env = self.top_consts.borrow().clone();
        let mut subs = self.top_consts.borrow().clone();
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
            subs.insert(binding, value.clone());
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
        let mut specialized_where: Vec<Expr> = where_clauses
            .iter()
            .map(|predicate| materialize_expression(predicate, &subs))
            .collect();
        if kept_type_params.is_empty() {
            for predicate in where_clauses {
                self.validate_specialized_where(predicate, &env, orig)?;
            }
            specialized_where = Vec::new();
        }
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
        let mut specialized_methods = Vec::with_capacity(methods.len());
        for method in methods {
            let mut method = method.clone();
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
            method.body = materialize_block(elaborated, &method_subs);
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
            specialized_methods.push(method);
        }
        let mangled = mangle(orig, vals);
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
            },
            template.span,
        );
        // Same provenance discipline as the pack path: specializations reuse
        // template spans, so each subtree gets a unique source tag.
        let tag = match &template.module {
            Some(module) => format!("{module}${mangled}"),
            None => mangled,
        };
        crate::ast::stamp_source(std::slice::from_mut(&mut spec), &tag);
        spec.module = None;
        Ok(spec)
    }

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
        let decls = classify_ct_params(type_params);
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
                    crate::origin::OriginParamId(index as u32),
                    format!("__tuple_origin_{index}"),
                )
            })
            .collect::<HashMap<_, _>>();
        let retained_origin_parameters = (0..origin_count)
            .map(|index| {
                let id = crate::origin::OriginParamId(index as u32);
                let mutability = reference_origins
                    .get(&id)
                    .copied()
                    .unwrap_or(crate::origin::Mutability::Param(id));
                TypeParam {
                    name: origin_names[&id].clone(),
                    bounds: vec!["Origin".to_string()],
                    value_type: None,
                    callable_bound: None,
                    origin_mutability: match mutability {
                        crate::origin::Mutability::Immutable => {
                            Some(Expr::new(ExprKind::Bool(false), template.span))
                        }
                        crate::origin::Mutability::Mutable => {
                            Some(Expr::new(ExprKind::Bool(true), template.span))
                        }
                        crate::origin::Mutability::Param(_) => None,
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
            if matches!(&member.value.kind, ExprKind::Identifier(name) if name == &binding) {
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
        for method in methods {
            let mut method = method.clone();
            let dependent_index_accessor =
                matches!(method.name.as_str(), "__getitem__" | "__getitem_param__")
                    && !method.type_params.is_empty();
            let mut env = self.top_consts.borrow().clone();
            env.insert(binding.clone(), CtValue::Tuple(types.clone()));
            let mut subs = self.top_consts.borrow().clone();
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
            let mut method_unavailable = false;
            let mut residual_clauses = Vec::new();
            for condition in method.where_clauses.drain(..) {
                let folded = self.fold_pack_conformance_predicate(&condition, &binding, types)?;
                match &folded.kind {
                    ExprKind::Bool(false) => {
                        method_unavailable = true;
                        break;
                    }
                    ExprKind::Bool(true) => {}
                    _ => residual_clauses.push(folded),
                }
            }
            if method_unavailable {
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
                    overload.body = materialize_block(elaborated, &subs);
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
                let index_decls = classify_ct_params(&method.type_params);
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
                    unrolled.body = materialize_block(elaborated, &subs_k);
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
                    elaborated_methods.push(unrolled);
                    if let Some(value_accessor) = value_accessor {
                        elaborated_methods.push(value_accessor);
                    }
                }
                continue;
            }
            let elaborated = self.block(&method.body, &mut env, true).map_err(|error| {
                ComptimeError::NotComptime(format!(
                    "while specializing {orig}.{}: {error}",
                    method.name
                ))
            })?;
            method.body = materialize_block(elaborated, &subs);
            elaborated_methods.push(method);
        }
        if orig == "Tuple" {
            self.append_tuple_transform_methods(
                &mut elaborated_methods,
                &semantic_types,
                template.span,
            );
        }
        let mangled = mangle(orig, vals);
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
        crate::ast::stamp_source(std::slice::from_mut(&mut spec), &tag);
        if let StmtKind::Struct { methods, .. } = &mut spec.kind {
            for method in methods {
                if method.name.starts_with("__getitem__$")
                    || method.name.starts_with("__getitem_param__$")
                    || method.name.starts_with("__getitem_value__$")
                    || method.name.starts_with("__getitem_param_value__$")
                {
                    crate::ast::stamp_source(&mut method.body, &format!("{tag}.{}", method.name));
                }
            }
        }
        // The subtree is stamped; disarm `elaborate`'s uniform module re-stamp
        // (it would collapse the per-accessor tags back into one).
        spec.module = None;
        Ok(spec)
    }

    /// Emit closed-world, fully concrete Tuple transforms as ordinary methods.
    /// The discovery checker has already recorded every result Tuple type. No
    /// dependent pack transform survives into checking or MIR, and execution is
    /// normal constructor/method dispatch rather than a VM tuple intrinsic.
    pub(super) fn append_tuple_transform_methods(
        &self,
        methods: &mut Vec<crate::ast::Method>,
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
                        if field == "values"
                            && matches!(&object.kind, ExprKind::Identifier(name) if name == binding)
                );
                if !pack_matches {
                    return Ok(expression.clone());
                }
                let ExprKind::Identifier(trait_name) = &args[1].kind else {
                    return Err(ComptimeError::NotComptime(
                        "conforms_to on a type pack requires a trait name".to_string(),
                    ));
                };
                let trait_name = crate::ast::canonical_trait_name(trait_name);
                let satisfied = elements.iter().all(|element| match element {
                    CtValue::Type(ty) => self.conformance.require(ty, trait_name).is_ok(),
                    _ => false,
                });
                Ok(with_kind(ExprKind::Bool(satisfied)))
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
