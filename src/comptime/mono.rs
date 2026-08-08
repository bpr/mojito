//! Monomorphizing AST rewrite (`mono_block`/`mono_stmt`/`mono_type`/`mono_expr`)
//! and struct-specialization argument resolution.
//! Extracted from `comptime.rs`; see `docs/symbol-map.md`.

use super::*;

impl<'a> Elab<'a> {
    pub(super) fn mono_block(
        &self,
        stmts: &mut [Stmt],
        consts: &HashMap<String, CtValue>,
        mono: &mut Mono,
    ) -> Result<(), ComptimeError> {
        mono.push_value_scope();
        let result = self.mono_block_contents(stmts, consts, mono);
        mono.pop_value_scope();
        result
    }

    pub(super) fn mono_function_body(
        &self,
        stmts: &mut [Stmt],
        parameters: &[FnParam],
        consts: &HashMap<String, CtValue>,
        mono: &mut Mono,
    ) -> Result<(), ComptimeError> {
        mono.push_function_scope();
        for parameter in parameters {
            mono.bind_parameter(parameter);
        }
        let result = self.mono_block_contents(stmts, consts, mono);
        mono.pop_function_scope();
        result
    }

    pub(super) fn mono_block_contents(
        &self,
        stmts: &mut [Stmt],
        consts: &HashMap<String, CtValue>,
        mono: &mut Mono,
    ) -> Result<(), ComptimeError> {
        for s in stmts {
            // Declarations bind before their body is visited, preserving
            // recursion while shadowing an outer top-level template.
            if let StmtKind::Def { name, .. }
            | StmtKind::Struct { name, .. }
            | StmtKind::Trait { name, .. } = &s.kind
            {
                mono.bind_value(name, false);
            }
            self.mono_stmt(s, consts, mono)?;
            match &s.kind {
                StmtKind::VarDecl { name, .. }
                | StmtKind::RefDecl { name, .. }
                | StmtKind::Comptime { name, .. } => mono.bind_value(name, false),
                StmtKind::Assign { name, .. } => mono.bind_named_value(name),
                StmtKind::Import { path, alias } => {
                    if let Some(name) = alias.as_ref().or_else(|| path.first()) {
                        mono.bind_value(name, false);
                    }
                }
                StmtKind::FromImport {
                    names: crate::ast::ImportNames::Names(names),
                    ..
                } => {
                    for import in names {
                        mono.bind_value(import.alias.as_deref().unwrap_or(&import.name), false);
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub(super) fn mono_stmt(
        &self,
        s: &mut Stmt,
        consts: &HashMap<String, CtValue>,
        mono: &mut Mono,
    ) -> Result<(), ComptimeError> {
        // Monomorphization substitutes one concrete parameter environment and
        // rewrites nested calls to their specialized symbols.
        match &mut s.kind {
            StmtKind::VarDecl { ty, value, .. } => {
                if let Some(ty) = ty {
                    self.mono_type(ty, consts, mono)?;
                }
                self.mono_expr(value, consts, mono)
            }
            StmtKind::RefDecl { value, .. }
            | StmtKind::Assign { value, .. }
            | StmtKind::Comptime { value, .. }
            | StmtKind::Raise(value)
            | StmtKind::Return(Some(value)) => self.mono_expr(value, consts, mono),
            StmtKind::Return(None)
            | StmtKind::Pass
            | StmtKind::Break
            | StmtKind::Continue
            | StmtKind::Import { .. }
            | StmtKind::FromImport { .. }
            | StmtKind::Trait { .. } => Ok(()),
            StmtKind::SetPlace { place, value } | StmtKind::AugAssign { place, value, .. } => {
                self.mono_expr(place, consts, mono)?;
                self.mono_expr(value, consts, mono)
            }
            StmtKind::Unpack { targets, value, .. } => {
                for t in targets.iter_mut() {
                    self.mono_expr(t, consts, mono)?;
                }
                self.mono_expr(value, consts, mono)
            }
            StmtKind::Expr(e) => self.mono_expr(e, consts, mono),
            StmtKind::If { branches, orelse } | StmtKind::ComptimeIf { branches, orelse } => {
                for (c, b) in branches.iter_mut() {
                    self.mono_expr(c, consts, mono)?;
                    self.mono_block(b, consts, mono)?;
                }
                if let Some(b) = orelse {
                    self.mono_block(b, consts, mono)?;
                }
                Ok(())
            }
            StmtKind::While { cond, body, .. } => {
                self.mono_expr(cond, consts, mono)?;
                self.mono_block(body, consts, mono)
            }
            StmtKind::For {
                var, iter, body, ..
            }
            | StmtKind::ComptimeFor { var, iter, body } => {
                self.mono_expr(iter, consts, mono)?;
                mono.push_value_scope();
                mono.bind_value(var, false);
                let result = self.mono_block_contents(body, consts, mono);
                mono.pop_value_scope();
                result
            }
            StmtKind::Try {
                body,
                except,
                orelse,
                finalbody,
            } => {
                self.mono_block(body, consts, mono)?;
                if let Some((name, b)) = except {
                    mono.push_value_scope();
                    if let Some(name) = name {
                        mono.bind_value(name, false);
                    }
                    let result = self.mono_block_contents(b, consts, mono);
                    mono.pop_value_scope();
                    result?;
                }
                if let Some(b) = orelse {
                    self.mono_block(b, consts, mono)?;
                }
                if let Some(b) = finalbody {
                    self.mono_block(b, consts, mono)?;
                }
                Ok(())
            }
            StmtKind::With { items, body } => {
                for WithItem { context, .. } in items.iter_mut() {
                    self.mono_expr(context, consts, mono)?;
                }
                mono.push_value_scope();
                for item in items {
                    if let Some(name) = &item.var {
                        mono.bind_value(name, false);
                    }
                }
                let result = self.mono_block_contents(body, consts, mono);
                mono.pop_value_scope();
                result
            }
            StmtKind::Def {
                params,
                raises_type,
                ret,
                body,
                ..
            } => {
                for parameter in params.iter_mut() {
                    self.mono_type(&mut parameter.ty, consts, mono)?;
                    if let Some(default) = &mut parameter.default {
                        self.mono_expr(default, consts, mono)?;
                    }
                }
                if let Some(error) = raises_type {
                    self.mono_type(error, consts, mono)?;
                }
                if let Some(ret) = ret {
                    self.mono_type(ret, consts, mono)?;
                }
                mono.push_function_scope();
                for parameter in params {
                    mono.bind_parameter(parameter);
                }
                let result = self.mono_block_contents(body, consts, mono);
                mono.pop_function_scope();
                result
            }
            StmtKind::Struct {
                type_params,
                fields,
                associated,
                methods,
                ..
            } => {
                let mut struct_consts = consts.clone();
                for (index, parameter) in type_params.iter().enumerate() {
                    if parameter.bounds.as_slice() != ["Origin"] {
                        continue;
                    }
                    let id = crate::origin::OriginParamId(index as u32);
                    let mutability = match parameter.origin_mutability.as_ref().map(|e| &e.kind) {
                        Some(ExprKind::Bool(true)) => crate::origin::Mutability::Mutable,
                        Some(ExprKind::Bool(false)) => crate::origin::Mutability::Immutable,
                        _ => crate::origin::Mutability::Param(id),
                    };
                    struct_consts
                        .insert(parameter.name.clone(), ct_origin_marker(index, mutability));
                }
                for field in fields.iter_mut() {
                    self.mono_type(&mut field.ty, &struct_consts, mono)?;
                }
                // Associated facts may themselves be type-valued.  A variadic
                // struct mentioned only here still needs a concrete request
                // before its template is removed (for example an Iterable's
                // associated iterator family).
                for member in associated.iter_mut() {
                    self.mono_expr(&mut member.value, &struct_consts, mono)?;
                }
                for m in methods.iter_mut() {
                    for parameter in m.params.iter_mut() {
                        self.mono_type(&mut parameter.ty, &struct_consts, mono)?;
                        if let Some(default) = &mut parameter.default {
                            self.mono_expr(default, &struct_consts, mono)?;
                        }
                    }
                    if let Some(error) = &mut m.raises_type {
                        self.mono_type(error, &struct_consts, mono)?;
                    }
                    if let Some(ret) = &mut m.ret {
                        self.mono_type(ret, &struct_consts, mono)?;
                    }
                    mono.push_function_scope();
                    if m.has_self {
                        mono.bind_value("self", false);
                    }
                    for parameter in &m.params {
                        mono.bind_parameter(parameter);
                    }
                    let result = self.mono_block_contents(&mut m.body, &struct_consts, mono);
                    mono.pop_function_scope();
                    result?;
                }
                Ok(())
            }
        }
    }

    /// Rewrite variadic-struct template names inside a type annotation to their
    /// specialized (mangled) names, enqueueing the needed instantiations.
    pub(super) fn mono_type(
        &self,
        ty: &mut Type,
        consts: &HashMap<String, CtValue>,
        mono: &mut Mono,
    ) -> Result<(), ComptimeError> {
        match ty {
            Type::Named(name, arguments) => {
                for argument in arguments.iter_mut() {
                    self.mono_param_arg(argument, consts, mono)?;
                }
                if self.specializable.contains_key(name.as_str())
                    && matches!(
                        self.specializable[name.as_str()].kind,
                        StmtKind::Struct { .. }
                    )
                {
                    let Some(vals) =
                        self.resolve_struct_spec_args_if_ready(name, arguments, consts)?
                    else {
                        // Public Tuple applications in an ordinary generic
                        // declaration remain symbolic until the checker has
                        // substituted the declaration's type parameters at a
                        // concrete use.  The discovery pass then requests the
                        // resulting closed nominal specialization.
                        return Ok(());
                    };
                    let mangled = mangle(name, &vals);
                    if mono.done.insert(mangled.clone()) {
                        mono.queue.push_back(Job {
                            orig: name.clone(),
                            vals,
                            site: "a type annotation".to_string(),
                            output_name: mangled.clone(),
                            whole_pack_abi: false,
                        });
                    }
                    *name = mangled;
                    arguments.clear();
                }
                Ok(())
            }
            Type::Assoc { base, .. } => self.mono_type(base, consts, mono),
            Type::IndexedProjection { base, index } => {
                self.mono_type(base, consts, mono)?;
                self.mono_expr(index, consts, mono)
            }
            Type::Func {
                type_params,
                params,
                ret,
                capturing,
                raises_type,
                ..
            } => {
                for parameter in type_params {
                    if let Some(value_type) = &mut parameter.value_type {
                        self.mono_type(value_type, consts, mono)?;
                    }
                    if let Some(callable) = &mut parameter.callable_bound {
                        self.mono_type(callable, consts, mono)?;
                    }
                    if let Some(mutability) = &mut parameter.origin_mutability {
                        self.mono_expr(mutability, consts, mono)?;
                    }
                    if let Some(default) = &mut parameter.default {
                        self.mono_expr(default, consts, mono)?;
                    }
                    for constraint in &mut parameter.constraints {
                        self.mono_expr(constraint, consts, mono)?;
                    }
                }
                for param in params {
                    self.mono_type(&mut param.ty, consts, mono)?;
                }
                self.mono_type(ret, consts, mono)?;
                for origin in capturing.iter_mut().flatten() {
                    self.mono_expr(origin, consts, mono)?;
                }
                if let Some(error) = raises_type {
                    self.mono_type(error, consts, mono)?;
                }
                Ok(())
            }
            Type::Ref { referent, .. } => self.mono_type(referent, consts, mono),
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

    pub(super) fn mono_param_arg(
        &self,
        argument: &mut ParamArg,
        consts: &HashMap<String, CtValue>,
        mono: &mut Mono,
    ) -> Result<(), ComptimeError> {
        match argument {
            ParamArg::Type(ty) => self.mono_type(ty, consts, mono),
            ParamArg::Named { value, .. } => self.mono_param_arg(value, consts, mono),
            ParamArg::Value(value) => self.mono_expr(value, consts, mono),
        }
    }

    pub(super) fn mono_expr(
        &self,
        e: &mut Expr,
        consts: &HashMap<String, CtValue>,
        mono: &mut Mono,
    ) -> Result<(), ComptimeError> {
        let source_span = e.source_span();
        let request_site = match &source_span.source {
            Some(source) => format!("{source}:{}..{}", source_span.span.0, source_span.span.1),
            None => format!("bytes {}..{}", source_span.span.0, source_span.span.1),
        };
        match &mut e.kind {
            ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Bool(_)
            | ExprKind::Str(_)
            | ExprKind::None => Ok(()),
            ExprKind::TString { parts, .. } => {
                for part in parts.iter_mut() {
                    if let TStringPart::Expr(value) = part {
                        self.mono_expr(value, consts, mono)?;
                    }
                }
                // Rewrite a checker-selected occurrence into the concrete
                // `TString` specialization's construction: literal segments
                // become string constants, and an interpolation whose
                // interleaved element type is the builtin string arrives
                // pre-formatted through a synthesized `String(...)`
                // conversion (identity for string-typed interpolations, the
                // creation-time snapshot for non-Copyable places).  An
                // occurrence without a target — the discovery round or a
                // retained abstract template body — deliberately survives
                // for the eager MIR fallback.
                let Some(target) = mono
                    .tstring_call_targets
                    .get(&source_span.clone().without_syntax())
                else {
                    return Ok(());
                };
                if parts.len() != target.elements.len() {
                    return Ok(());
                }
                let span = e.span;
                let source = e.source.clone();
                let ExprKind::TString { parts, .. } =
                    std::mem::replace(&mut e.kind, ExprKind::None)
                else {
                    unreachable!("the enclosing match arm established a t-string");
                };
                let mut args = Vec::with_capacity(parts.len());
                for (part, element) in parts.into_iter().zip(&target.elements) {
                    match part {
                        TStringPart::Literal(text) => {
                            let mut literal = Expr::new(ExprKind::Str(text), span);
                            literal.source = source.clone();
                            args.push(literal);
                        }
                        TStringPart::Expr(value) => {
                            if matches!(element, Ty::StringLiteral) {
                                let mut conversion = Expr::new(
                                    ExprKind::Call {
                                        name: "String".to_string(),
                                        param_args: Vec::new(),
                                        args: Vec::new(),
                                        kwargs: Vec::new(),
                                    },
                                    value.span,
                                );
                                conversion.source = value.source.clone();
                                let ExprKind::Call { args: inner, .. } = &mut conversion.kind
                                else {
                                    unreachable!("the conversion was just built as a call");
                                };
                                inner.push(*value);
                                args.push(conversion);
                            } else {
                                args.push(*value);
                            }
                        }
                    }
                }
                e.kind = ExprKind::Call {
                    name: target.symbol.clone(),
                    param_args: Vec::new(),
                    args,
                    kwargs: Vec::new(),
                };
                Ok(())
            }
            ExprKind::Identifier(name) => {
                // The template is dropped after monomorphization, so a bare
                // (argument-less) use of a variadic struct can never resolve.
                if mono.resolves_top_template(name) && self.struct_template(name) {
                    return Err(ComptimeError::NotComptime(format!(
                        "variadic struct '{name}' requires explicit compile-time type arguments, e.g. `{name}[Int, Bool](...)`"
                    )));
                }
                // A function-value use of a bound generic pins the abstract
                // template: there is no application to monomorphize against.
                if mono.resolves_top_template(name) && self.bound_generics.contains(name.as_str()) {
                    mono.retained.insert(name.clone());
                }
                Ok(())
            }
            ExprKind::TypeApply { name, args } => {
                if mono.resolves_top_template(name) && self.struct_template(name) {
                    let Some(vals) = self.resolve_struct_spec_args_if_ready(name, args, consts)?
                    else {
                        return Ok(());
                    };
                    let mangled = mangle(name, &vals);
                    if mono.done.insert(mangled.clone()) {
                        mono.queue.push_back(Job {
                            orig: name.clone(),
                            vals,
                            site: request_site,
                            output_name: mangled.clone(),
                            whole_pack_abi: false,
                        });
                    }
                    *name = mangled;
                    args.clear();
                }
                Ok(())
            }
            ExprKind::Prefix(_, inner) | ExprKind::Transfer(inner) | ExprKind::Spread(inner) => {
                self.mono_expr(inner, consts, mono)
            }
            ExprKind::Infix(_, l, r) => {
                self.mono_expr(l, consts, mono)?;
                self.mono_expr(r, consts, mono)
            }
            ExprKind::Compare { first, rest } => {
                self.mono_expr(first, consts, mono)?;
                for (_, r) in rest.iter_mut() {
                    self.mono_expr(r, consts, mono)?;
                }
                Ok(())
            }
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } => {
                for a in args.iter_mut() {
                    self.mono_expr(a, consts, mono)?;
                }
                for k in kwargs.iter_mut() {
                    self.mono_expr(&mut k.value, consts, mono)?;
                }
                // A bare public `Tuple(...)` has no source type arguments from
                // which pre-check elaboration could soundly choose `*Ts`.  Only
                // rewrite an occurrence the checker explicitly identified; an
                // unhinted occurrence deliberately survives for the discovery
                // check. Other variadic struct templates retain their existing
                // explicit-argument requirement.
                if name == "Tuple"
                    && param_args.is_empty()
                    && mono.resolves_top_template(name)
                    && self.struct_template(name)
                {
                    if let Some(target) = mono
                        .tuple_call_targets
                        .get(&source_span.clone().without_syntax())
                    {
                        *name = target.clone();
                    }
                    return Ok(());
                }
                if mono.resolves_top_template(name) && self.specializable.contains_key(name) {
                    let (vals, kept_type_args, whole_pack_abi) = if self.struct_template(name) {
                        // A struct specialization is fully concrete: every
                        // compile-time argument is baked into the mangled name.
                        let Some(values) =
                            self.resolve_struct_spec_args_if_ready(name, param_args, consts)?
                        else {
                            return Ok(());
                        };
                        (values, Vec::new(), false)
                    } else if self.bound_generics.contains(name.as_str()) {
                        // Soft resolution: only an explicit application whose
                        // arguments resolve concretely monomorphizes. A bound
                        // violation on a resolved argument is a real error;
                        // any other failure (inference, symbolic arguments)
                        // leaves the call on the template's abstract path and
                        // retains the template.
                        let template = self.specializable[name.as_str()];
                        match self.resolve_spec_args_for(
                            template,
                            name,
                            SpecRequest {
                                param_args,
                                call_args: args,
                                kwargs,
                                consts,
                                request_site: &request_site,
                                forwarded_pack_types: None,
                            },
                        ) {
                            Ok((values, kept)) => (values, kept, false),
                            Err(error @ ComptimeError::GenericBound(_)) => return Err(error),
                            // Source arguments could not resolve (an inferred
                            // call or symbolic arguments): consult the
                            // checker-discovered request for this occurrence
                            // before falling back to the abstract path.
                            Err(_) => {
                                match self.def_request_target(name, &source_span, param_args, mono)
                                {
                                    Some((values, kept)) => (values, kept, false),
                                    None => {
                                        mono.retained.insert(name.clone());
                                        return Ok(());
                                    }
                                }
                            }
                        }
                    } else {
                        let template = self.specializable[name.as_str()];
                        let whole_pack_abi = top_level_whole_pack_forwarding_call(template, args)?;
                        let forwarded =
                            top_level_forwarded_pack_types(template, name, args, kwargs, mono)?;
                        let (values, kept) = self.resolve_spec_args_for(
                            template,
                            name,
                            SpecRequest {
                                param_args,
                                call_args: args,
                                kwargs,
                                consts,
                                request_site: &request_site,
                                forwarded_pack_types: forwarded.as_deref(),
                            },
                        )?;
                        (values, kept, whole_pack_abi)
                    };
                    let original = name.clone();
                    let mut output_name = mangle(name, &vals);
                    if whole_pack_abi {
                        output_name.push_str("$whole_pack");
                    }
                    if mono.done.insert(output_name.clone()) {
                        mono.queue.push_back(Job {
                            orig: original,
                            vals,
                            site: request_site,
                            output_name: output_name.clone(),
                            whole_pack_abi,
                        });
                    }
                    *name = output_name;
                    if whole_pack_abi {
                        *args = unwrap_runtime_pack_arguments(std::mem::take(args));
                    }
                    // Value arguments are baked into the specialization; type
                    // arguments stay on the (still type-generic) specialized def.
                    *param_args = kept_type_args;
                }
                Ok(())
            }
            ExprKind::Member { object, .. } => self.mono_expr(object, consts, mono),
            ExprKind::MethodCall {
                object,
                args,
                kwargs,
                ..
            } => {
                self.mono_expr(object, consts, mono)?;
                for a in args.iter_mut() {
                    self.mono_expr(a, consts, mono)?;
                }
                for k in kwargs.iter_mut() {
                    self.mono_expr(&mut k.value, consts, mono)?;
                }
                Ok(())
            }
            ExprKind::Index { object, index } => {
                self.mono_expr(object, consts, mono)?;
                self.mono_expr(index, consts, mono)
            }
            ExprKind::Slice {
                object,
                lower,
                upper,
                step,
                ..
            } => {
                self.mono_expr(object, consts, mono)?;
                for b in [lower, upper, step].into_iter().flatten() {
                    self.mono_expr(b, consts, mono)?;
                }
                Ok(())
            }
            ExprKind::MultiIndex { object, args } => {
                self.mono_expr(object, consts, mono)?;
                for argument in args {
                    match argument {
                        crate::ast::SubscriptArg::Index(value)
                        | crate::ast::SubscriptArg::Keyword { value, .. } => {
                            self.mono_expr(value, consts, mono)?
                        }
                        crate::ast::SubscriptArg::Slice {
                            lower, upper, step, ..
                        } => {
                            for value in [lower, upper, step].into_iter().flatten() {
                                self.mono_expr(value, consts, mono)?;
                            }
                        }
                    }
                }
                Ok(())
            }
            ExprKind::ListLit(elems) | ExprKind::TupleLit(elems) => {
                for el in elems.iter_mut() {
                    self.mono_expr(el, consts, mono)?;
                }
                Ok(())
            }
            ExprKind::BraceLit(entries) => {
                for (key, value) in entries {
                    self.mono_expr(key, consts, mono)?;
                    if let Some(value) = value {
                        self.mono_expr(value, consts, mono)?;
                    }
                }
                Ok(())
            }
            ExprKind::Comprehension {
                key,
                value,
                clauses,
                ..
            } => {
                mono.push_value_scope();
                for clause in clauses {
                    match clause {
                        crate::ast::ComprehensionClause::For { var, iter, .. } => {
                            self.mono_expr(iter, consts, mono)?;
                            mono.bind_value(var, false);
                        }
                        crate::ast::ComprehensionClause::If(condition) => {
                            self.mono_expr(condition, consts, mono)?
                        }
                    }
                }
                if let Some(key) = key {
                    self.mono_expr(key, consts, mono)?;
                }
                let result = self.mono_expr(value, consts, mono);
                mono.pop_value_scope();
                result
            }
            ExprKind::Named { name, value } => {
                self.mono_expr(value, consts, mono)?;
                mono.bind_named_value(name);
                Ok(())
            }
            ExprKind::TypeValue(_) => Ok(()),
            ExprKind::Invoke {
                callee,
                param_args,
                args,
                kwargs,
            } => {
                self.mono_expr(callee, consts, mono)?;
                for argument in param_args {
                    self.mono_param_arg(argument, consts, mono)?;
                }
                for argument in args {
                    self.mono_expr(argument, consts, mono)?;
                }
                for argument in kwargs {
                    self.mono_expr(&mut argument.value, consts, mono)?;
                }
                Ok(())
            }
            ExprKind::Uninitialized => Ok(()),
            ExprKind::IfExpr {
                cond,
                then_branch,
                else_branch,
            } => {
                self.mono_expr(cond, consts, mono)?;
                self.mono_expr(then_branch, consts, mono)?;
                self.mono_expr(else_branch, consts, mono)
            }
        }
    }

    /// Whether `name` is a specializable variadic-struct template.
    pub(super) fn struct_template(&self, name: &str) -> bool {
        self.specializable
            .get(name)
            .is_some_and(|template| matches!(template.kind, StmtKind::Struct { .. }))
    }

    /// Resolve a variadic-struct instantiation's `[...]` arguments into the
    /// specialization key: every argument is a type, collected into the pack
    /// tuple. Instantiation requires explicit arguments (the elaborator does
    /// not infer types), and a template supports exactly one trailing pack.
    pub(super) fn resolve_struct_spec_args(
        &self,
        name: &str,
        param_args: &[ParamArg],
        consts: &HashMap<String, CtValue>,
    ) -> Result<Vec<CtValue>, ComptimeError> {
        let StmtKind::Struct { type_params, .. } = &self.specializable[name].kind else {
            return Err(ComptimeError::NotComptime(format!(
                "specialization registry entry '{name}' is not a struct"
            )));
        };
        let decls = classify_ct_params(type_params);
        let [ParamDecl::Type { variadic: true, .. }] = decls.as_slice() else {
            return Err(ComptimeError::NotComptime(format!(
                "variadic struct '{name}' supports exactly one type-parameter pack and no other compile-time parameters"
            )));
        };
        if param_args.is_empty() {
            return Err(ComptimeError::NotComptime(format!(
                "variadic struct '{name}' requires explicit compile-time type arguments, e.g. `{name}[Int, Bool](...)`"
            )));
        }
        let types = param_args
            .iter()
            .map(|argument| {
                self.param_arg_type(argument, consts)
                    .map(|ty| CtValue::Type(Box::new(ty)))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(vec![CtValue::Tuple(types)])
    }

    /// Resolve a variadic-struct application when it is ready for concrete
    /// monomorphization.  Public `Tuple[T, ...]` may appear in the signature or
    /// body of an ordinary generic declaration: pre-check elaboration has no
    /// binding for `T`, and manufacturing a `Tuple$T` implementation would be
    /// unsound.  Leave only that compiler-known public template canonical so
    /// the checker can retain the symbolic type and the later discovery pass can
    /// request its closed call-site instantiations.  User variadic structs keep
    /// their existing eager, explicit-specialization diagnostics.
    pub(super) fn resolve_struct_spec_args_if_ready(
        &self,
        name: &str,
        param_args: &[ParamArg],
        consts: &HashMap<String, CtValue>,
    ) -> Result<Option<Vec<CtValue>>, ComptimeError> {
        match self.resolve_struct_spec_args(name, param_args, consts) {
            Ok(values) => Ok(Some(values)),
            Err(_) if name == "Tuple" => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Resolve arguments for one concrete declaration. `forwarded_pack_types`
    /// supplies the element sequence when a specialized runtime pack is being
    /// forwarded into another heterogeneous collector; ordinary calls infer the
    /// sequence from their source expressions as before.
    /// The `vals` a checker-recorded instantiation selects for a
    /// bound-generic template, aligned with `resolve_spec_args_for`'s shape:
    /// one value per elaborator-classified parameter, in declaration order —
    /// so `mangle` and `mono.done` collide correctly with explicit
    /// applications. The checker's declaration-order `TyArg` list is a strict
    /// superset of that shape (it keeps callable-value parameters the
    /// elaborator retains symbolically, and omits Origin/OriginSet binders).
    /// `None` skips the request: a request can only upgrade a call from the
    /// abstract path, never introduce a new error.
    pub(super) fn def_request_values(
        &self,
        template: &Stmt,
        arguments: &[TyArg],
    ) -> Option<Vec<CtValue>> {
        let StmtKind::Def { type_params, .. } = &template.kind else {
            return None;
        };
        let mut vals = Vec::new();
        let mut cursor = arguments.iter();
        for parameter in type_params {
            // Origin/OriginSet binders have no checker declaration slot.
            if matches!(parameter.bounds.as_slice(), [only] if only == "Origin" || only == "OriginSet")
                || parameter.is_origin_mutability_binder(type_params)
            {
                continue;
            }
            let argument = cursor.next()?;
            if retained_specialization_param(parameter, type_params) {
                // A thin/capturing callable-value parameter keeps a checker
                // slot (a symbolic placeholder) but stays symbolic here.
                match argument {
                    TyArg::Val(CtValue::Param(_)) => continue,
                    _ => return None,
                }
            }
            let decl = classify_ct_param(parameter, type_params)?;
            let value = match (&decl, argument) {
                (
                    ParamDecl::Type {
                        variadic: false, ..
                    },
                    TyArg::Ty(ty),
                ) => CtValue::Type(Box::new(ty.clone())),
                (
                    ParamDecl::Value {
                        variadic: false,
                        ty,
                        ..
                    },
                    TyArg::Val(value),
                ) => {
                    if matches!(value, CtValue::Param(_)) || !ct_value_has_type(value, ty) {
                        return None;
                    }
                    value.clone()
                }
                // Packs never reach here (they classify as comptime-class
                // templates), and any other pairing is a drift signal.
                _ => return None,
            };
            // Drift guard between the checker's conformance and this oracle:
            // a dropped parameter's bounds are never re-validated later, so a
            // disagreement must keep the call abstract rather than bake an
            // unproven type into a clone.
            if let ParamDecl::Type { bounds, .. } = &decl
                && spec_type_param_substitution(&decl, &value).is_some()
            {
                let CtValue::Type(ty) = &value else {
                    return None;
                };
                if bounds
                    .iter()
                    .any(|bound| self.conformance.require(ty, bound).is_err())
                {
                    return None;
                }
            }
            vals.push(value);
        }
        if cursor.next().is_some() {
            return None;
        }
        Some(vals)
    }

    /// The checker-requested clone for an inferred bound-generic call whose
    /// source arguments could not resolve, plus the source arguments the
    /// rewritten call keeps. `None` leaves the call on the abstract path.
    fn def_request_target(
        &self,
        name: &str,
        source_span: &SourceSpan,
        param_args: &[ParamArg],
        mono: &Mono,
    ) -> Option<(Vec<CtValue>, Vec<ParamArg>)> {
        let target = mono
            .def_call_targets
            .get(&source_span.clone().without_syntax())?;
        if target.template != name {
            // A span collision with a different callee (duplicated source
            // provenance): stay abstract.
            return None;
        }
        let template = self.specializable.get(name)?;
        let kept = self.request_kept_param_args(template, name, param_args, &target.vals)?;
        Some((target.vals.clone(), kept))
    }

    /// The source arguments a request-rewritten call retains: arguments bound
    /// to symbolically retained parameters and to residual kept type
    /// parameters. Dropped parameters' arguments are baked into the clone; a
    /// kept parameter with no source argument contributes nothing (the
    /// checker re-infers it against the clone's residual signature, and the
    /// mangle already discriminates the identity).
    fn request_kept_param_args(
        &self,
        template: &Stmt,
        display_name: &str,
        param_args: &[ParamArg],
        vals: &[CtValue],
    ) -> Option<Vec<ParamArg>> {
        let StmtKind::Def { type_params, .. } = &template.kind else {
            return None;
        };
        let bound = bind_spec_param_args(type_params, param_args, display_name).ok()?;
        let mut kept = Vec::new();
        let mut values = vals.iter();
        for (parameter, arguments) in type_params.iter().zip(bound) {
            if retained_specialization_param(parameter, type_params) {
                kept.extend(arguments.into_iter().cloned());
                continue;
            }
            let decl = classify_ct_param(parameter, type_params)?;
            let value = values.next()?;
            if matches!(decl, ParamDecl::Type { .. })
                && spec_type_param_substitution(&decl, value).is_none()
            {
                kept.extend(arguments.into_iter().cloned());
            }
        }
        if values.next().is_some() {
            return None;
        }
        Some(kept)
    }

    pub(super) fn resolve_spec_args_for(
        &self,
        template: &Stmt,
        display_name: &str,
        request: SpecRequest<'_>,
    ) -> Result<(Vec<CtValue>, Vec<ParamArg>), ComptimeError> {
        let SpecRequest {
            param_args,
            call_args,
            kwargs,
            consts,
            request_site,
            forwarded_pack_types,
        } = request;
        let StmtKind::Def { type_params, .. } = &template.kind else {
            return Err(ComptimeError::NotComptime(format!(
                "specialization registry entry '{display_name}' is not a function"
            )));
        };

        let bound = bind_spec_param_args(type_params, param_args, display_name)?;

        let mut vals = Vec::new();
        let mut kept_type_args = Vec::new();
        let mut environment = consts.clone();
        for (parameter, arguments) in type_params.iter().zip(bound) {
            if retained_specialization_param(parameter, type_params) {
                if arguments.is_empty() && !parameter.infer_only && parameter.default.is_none() {
                    return Err(ComptimeError::Arity(format!(
                        "generic '{display_name}' requires compile-time parameter '{}'",
                        parameter.name.trim_start_matches('*')
                    )));
                }
                kept_type_args.extend(arguments.into_iter().cloned());
                continue;
            }

            let decl = classify_ct_param(parameter, type_params)
                .expect("non-retained source parameter must have a comptime classification");
            let binding = decl.name().trim_start_matches('*').to_string();
            if parameter.name.starts_with('*') {
                let value = match &decl {
                    ParamDecl::Value { name: pack, ty, .. } => {
                        let mut values = Vec::with_capacity(arguments.len());
                        for argument in arguments {
                            let value = self.resolve_ct_arg(&decl, argument, &environment)?;
                            if !ct_value_has_type(&value, ty) {
                                return Err(ComptimeError::NotComptime(format!(
                                    "value pack '{}' expects {ty}, got {value}",
                                    pack.trim_start_matches('*')
                                )));
                            }
                            values.push(value);
                        }
                        CtValue::Tuple(values)
                    }
                    ParamDecl::Type {
                        name: pack, bounds, ..
                    } => {
                        let types = if arguments.is_empty() {
                            match forwarded_pack_types {
                                Some(types) => types.to_vec(),
                                None => runtime_pack_call_arguments(
                                    template,
                                    display_name,
                                    call_args,
                                    kwargs,
                                )?
                                .into_iter()
                                .map(infer_pack_argument_type)
                                .collect::<Result<Vec<_>, _>>()?,
                            }
                        } else {
                            arguments
                                .into_iter()
                                .map(|argument| self.param_arg_type(argument, &environment))
                                .collect::<Result<Vec<_>, _>>()?
                        };
                        for (index, ty) in types.iter().enumerate() {
                            for trait_name in bounds {
                                if let Err(failure) = self.conformance.require(ty, trait_name) {
                                    return Err(ComptimeError::PackBound(Box::new(
                                        PackBoundError {
                                            function: display_name.to_string(),
                                            pack: pack.trim_start_matches('*').to_string(),
                                            index,
                                            ty: ty.to_string(),
                                            trait_name: trait_name.clone(),
                                            site: request_site.to_string(),
                                            reason: failure.reason,
                                        },
                                    )));
                                }
                            }
                        }
                        CtValue::Tuple(
                            types
                                .into_iter()
                                .map(|ty| CtValue::Type(Box::new(ty)))
                                .collect(),
                        )
                    }
                };
                environment.insert(binding, value.clone());
                vals.push(value);
                continue;
            }

            let value = if let Some(argument) = arguments.first() {
                self.resolve_ct_arg(&decl, argument, &environment)?
            } else {
                match &decl {
                    ParamDecl::Value {
                        default: Some(default),
                        ty,
                        ..
                    } => {
                        let evaluated = default.evaluate(&environment).ok_or_else(|| {
                            ComptimeError::NotComptime(format!(
                                "cannot evaluate default for parameter '{}'",
                                decl.name()
                            ))
                        })?;
                        materialize_ct_value(evaluated.clone(), ty).ok_or_else(|| {
                            ComptimeError::NotComptime(format!(
                                "default for parameter '{}' expects {ty}, got {evaluated}",
                                decl.name()
                            ))
                        })?
                    }
                    ParamDecl::Type {
                        default: Some(default),
                        ..
                    } => CtValue::Type(default.clone()),
                    _ => {
                        return Err(ComptimeError::Arity(format!(
                            "generic '{display_name}' requires compile-time parameter '{}'",
                            decl.name().trim_start_matches('*')
                        )));
                    }
                }
            };
            if let ParamDecl::Type { bounds, .. } = &decl {
                if spec_type_param_substitution(&decl, &value).is_some() {
                    // The argument is baked into the specialization
                    // (`generate_def_spec` makes the matching decision), so
                    // the checker never re-validates it against the residual
                    // signature; enforce the parameter's trait bounds here.
                    let CtValue::Type(ty) = &value else {
                        unreachable!("a dropped type parameter binds a type value");
                    };
                    for trait_name in bounds {
                        if let Err(failure) = self.conformance.require(ty, trait_name) {
                            return Err(ComptimeError::GenericBound(Box::new(GenericBoundError {
                                function: display_name.to_string(),
                                param: decl.name().to_string(),
                                ty: ty.to_string(),
                                trait_name: trait_name.clone(),
                                site: request_site.to_string(),
                                reason: failure.reason,
                            })));
                        }
                    }
                } else {
                    kept_type_args.extend(arguments.into_iter().cloned());
                }
            }
            environment.insert(binding, value.clone());
            vals.push(value);
        }
        Ok((vals, kept_type_args))
    }
}

/// Bind a call's source compile-time argument list to the template's
/// parameters before classifying anything away. In particular, an infer-only
/// Origin consumes no positional slot, and a pack consumes only the overflow
/// left after required suffix binders. This is the source-layout invariant
/// used again by `generate_def_spec`.
fn bind_spec_param_args<'t>(
    type_params: &[TypeParam],
    param_args: &'t [ParamArg],
    display_name: &str,
) -> Result<Vec<Vec<&'t ParamArg>>, ComptimeError> {
    let mut bound: Vec<Vec<&ParamArg>> = vec![Vec::new(); type_params.len()];
    let mut positional = Vec::new();
    for argument in param_args {
        if let ParamArg::Named { name, .. } = argument {
            let Some(index) = type_params
                .iter()
                .position(|parameter| parameter.name.trim_start_matches('*') == name)
            else {
                return Err(ComptimeError::Arity(format!(
                    "generic '{display_name}' has no compile-time parameter named '{name}'"
                )));
            };
            if !bound[index].is_empty() {
                return Err(ComptimeError::Arity(format!(
                    "generic '{display_name}' received compile-time parameter '{name}' more than once"
                )));
            }
            bound[index].push(argument);
        } else {
            positional.push(argument);
        }
    }

    let required_suffix = |start: usize, bound: &[Vec<&ParamArg>]| {
        type_params[start..]
            .iter()
            .zip(&bound[start..])
            .filter(|(parameter, arguments)| {
                arguments.is_empty()
                    && !parameter.infer_only
                    && !parameter.name.starts_with('*')
                    && parameter.default.is_none()
            })
            .count()
    };
    let mut next_positional = 0;
    for index in 0..type_params.len() {
        let parameter = &type_params[index];
        if !bound[index].is_empty() || parameter.infer_only {
            continue;
        }
        let remaining = positional.len() - next_positional;
        let suffix = required_suffix(index + 1, &bound);
        if parameter.name.starts_with('*') {
            let take = remaining.saturating_sub(suffix);
            bound[index].extend_from_slice(
                &positional[next_positional..next_positional.saturating_add(take)],
            );
            next_positional += take;
        } else if remaining > suffix {
            bound[index].push(positional[next_positional]);
            next_positional += 1;
        }
    }
    if next_positional != positional.len() {
        return Err(ComptimeError::Arity(format!(
            "generic '{display_name}' received {} unmatched compile-time argument(s)",
            positional.len() - next_positional
        )));
    }
    Ok(bound)
}
