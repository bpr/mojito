//! Monomorphizing AST rewrite (`mono_block`/`mono_stmt`/`mono_type`/`mono_expr`)
//! and struct-specialization argument resolution.
//! Extracted from `comptime.rs`; see `docs/symbol-map.md`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

impl Elab<'_> {
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

    pub(super) fn mono_block_contents(
        &self,
        stmts: &mut [Stmt],
        consts: &HashMap<String, CtValue>,
        mono: &mut Mono,
    ) -> Result<(), ComptimeError> {
        // A local `comptime NAME = DType.<name>` (elaboration folded any
        // compile-time dtype expression to that spelling) keys the value
        // arguments of the statements after it (`width[NAME]()`).
        let mut local_consts: Option<HashMap<String, CtValue>> = None;
        for s in stmts {
            // Declarations bind before their body is visited, preserving
            // recursion while shadowing an outer top-level template.
            if let StmtKind::Def { name, .. }
            | StmtKind::Struct { name, .. }
            | StmtKind::Trait { name, .. } = &s.kind
            {
                mono.bind_value(name, false);
            }
            self.mono_stmt(s, local_consts.as_ref().unwrap_or(consts), mono)?;
            if let StmtKind::Comptime {
                name,
                type_params,
                value,
                ..
            } = &s.kind
                && type_params.is_empty()
                && let ExprKind::Member { object, field } = &value.kind
                && matches!(&object.kind, ExprKind::Identifier(namespace) if namespace == "DType")
                && let Some(dtype) = mojito_ast::ast::Dtype::from_name(field)
            {
                local_consts
                    .get_or_insert_with(|| consts.clone())
                    .insert(name.clone(), CtValue::Dtype(dtype));
            }
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
                    names: mojito_ast::ast::ImportNames::Names(names),
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
            | StmtKind::Raise(value)
            | StmtKind::Return(Some(value)) => self.mono_expr(value, consts, mono),
            StmtKind::Comptime {
                type_params,
                ty,
                where_clauses,
                value,
                ..
            } => {
                let inner_consts = consts_without_type_params(consts, type_params);
                for parameter in type_params {
                    self.mono_type_parameter(parameter, &inner_consts, mono)?;
                }
                if let Some(ty) = ty {
                    self.mono_type(ty, &inner_consts, mono)?;
                }
                for condition in where_clauses {
                    self.mono_expr(condition, &inner_consts, mono)?;
                }
                self.mono_expr(value, &inner_consts, mono)
            }
            StmtKind::Return(None)
            | StmtKind::Pass
            | StmtKind::Break
            | StmtKind::Continue
            | StmtKind::Import { .. }
            | StmtKind::FromImport { .. } => Ok(()),
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
            } => {
                self.mono_expr(iter, consts, mono)?;
                mono.push_value_scope();
                mono.bind_value(var, false);
                let result = self.mono_block_contents(body, consts, mono);
                mono.pop_value_scope();
                result
            }
            // A `comptime for` the walk reaches is kept for the elaborator to
            // unroll, so its index is a parameter of the body: an
            // application over it stays symbolic, as one over the enclosing
            // declaration's own parameters does.
            StmtKind::ComptimeFor { var, iter, body } => {
                self.mono_expr(iter, consts, mono)?;
                mono.push_value_scope();
                mono.bind_value(var, false);
                let symbolic_base = mono.symbolic_type_params.len();
                mono.symbolic_type_params.push(var.clone());
                let result = self.mono_block_contents(body, consts, mono);
                mono.symbolic_type_params.truncate(symbolic_base);
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
            StmtKind::Scope(body) => self.mono_block(body, consts, mono),
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
                type_params,
                params,
                raises_type,
                ret,
                where_clauses,
                body,
                ..
            } => {
                let inner_consts = consts_without_type_params(consts, type_params);
                let symbolic_base = mono.push_symbolic_type_params(type_params);
                let result = (|| {
                    for parameter in type_params {
                        self.mono_type_parameter(parameter, &inner_consts, mono)?;
                    }
                    for parameter in params.iter_mut() {
                        self.mono_fn_parameter(parameter, &inner_consts, mono)?;
                    }
                    if let Some(error) = raises_type {
                        self.mono_type(error, &inner_consts, mono)?;
                    }
                    if let Some(ret) = ret {
                        self.mono_type(ret, &inner_consts, mono)?;
                    }
                    for condition in where_clauses {
                        self.mono_expr(condition, &inner_consts, mono)?;
                    }
                    mono.push_function_scope();
                    for parameter in params {
                        mono.bind_parameter(parameter);
                    }
                    let result = self.mono_block_contents(body, &inner_consts, mono);
                    mono.pop_function_scope();
                    result
                })();
                mono.symbolic_type_params.truncate(symbolic_base);
                result
            }
            StmtKind::Struct {
                type_params,
                callable_conformance,
                conformance_conditions,
                where_clauses,
                fields,
                associated,
                methods,
                ..
            } => {
                let symbolic_base = mono.push_symbolic_type_params(type_params);
                let result = (|| {
                    let mut struct_consts = consts.clone();
                    for (index, parameter) in type_params.iter().enumerate() {
                        if parameter.bounds.as_slice() != ["Origin"] {
                            continue;
                        }
                        let id = mojito_types::origin::OriginParamId(index as u32);
                        let mutability = match parameter.origin_mutability.as_ref().map(|e| &e.kind)
                        {
                            Some(ExprKind::Bool(true)) => mojito_types::origin::Mutability::Mutable,
                            Some(ExprKind::Bool(false)) => {
                                mojito_types::origin::Mutability::Immutable
                            }
                            _ => mojito_types::origin::Mutability::Param(id),
                        };
                        struct_consts
                            .insert(parameter.name.clone(), ct_origin_marker(id, mutability));
                    }
                    for parameter in type_params.iter_mut() {
                        self.mono_type_parameter(parameter, &struct_consts, mono)?;
                    }
                    if let Some(callable) = callable_conformance {
                        self.mono_type(callable, &struct_consts, mono)?;
                    }
                    for (_, condition) in conformance_conditions {
                        self.mono_expr(condition, &struct_consts, mono)?;
                    }
                    for condition in where_clauses {
                        self.mono_expr(condition, &struct_consts, mono)?;
                    }
                    for field in fields.iter_mut() {
                        self.mono_type(&mut field.ty, &struct_consts, mono)?;
                    }
                    // Associated facts may themselves be type-valued.  A variadic
                    // struct mentioned only here still needs a concrete request
                    // before its template is removed (for example an Iterable's
                    // associated iterator family).
                    for member in associated.iter_mut() {
                        let member_consts =
                            consts_without_type_params(&struct_consts, &member.params);
                        for parameter in &mut member.params {
                            self.mono_type_parameter(parameter, &member_consts, mono)?;
                        }
                        if let Some(ty) = &mut member.ty {
                            self.mono_type(ty, &member_consts, mono)?;
                        }
                        for condition in &mut member.where_clauses {
                            self.mono_expr(condition, &member_consts, mono)?;
                        }
                        self.mono_expr(&mut member.value, &member_consts, mono)?;
                    }
                    for m in methods.iter_mut() {
                        self.mono_method(m, &struct_consts, mono)?;
                    }
                    Ok(())
                })();
                mono.symbolic_type_params.truncate(symbolic_base);
                result
            }
            StmtKind::Trait {
                methods,
                comptime_members,
                ..
            } => {
                for method in methods {
                    let inner_consts = consts_without_type_params(consts, &method.type_params);
                    for parameter in &mut method.type_params {
                        self.mono_type_parameter(parameter, &inner_consts, mono)?;
                    }
                    if let Some(origins) = &mut method.self_origin {
                        for origin in origins {
                            self.mono_expr(origin, &inner_consts, mono)?;
                        }
                    }
                    for parameter in &mut method.params {
                        self.mono_fn_parameter(parameter, &inner_consts, mono)?;
                    }
                    if let Some(error) = &mut method.raises_type {
                        self.mono_type(error, &inner_consts, mono)?;
                    }
                    if let Some(ret) = &mut method.ret {
                        self.mono_type(ret, &inner_consts, mono)?;
                    }
                    for condition in &mut method.where_clauses {
                        self.mono_expr(condition, &inner_consts, mono)?;
                    }
                    if let Some(body) = &mut method.default_body {
                        self.mono_block(body, &inner_consts, mono)?;
                    }
                }
                for member in comptime_members {
                    let inner_consts = consts_without_type_params(consts, &member.params);
                    for parameter in &mut member.params {
                        self.mono_type_parameter(parameter, &inner_consts, mono)?;
                    }
                    self.mono_type(&mut member.ty, &inner_consts, mono)?;
                    for condition in &mut member.where_clauses {
                        self.mono_expr(condition, &inner_consts, mono)?;
                    }
                }
                Ok(())
            }
        }
    }

    fn mono_type_parameter(
        &self,
        parameter: &mut TypeParam,
        consts: &HashMap<String, CtValue>,
        mono: &mut Mono,
    ) -> Result<(), ComptimeError> {
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
        Ok(())
    }

    fn mono_fn_parameter(
        &self,
        parameter: &mut FnParam,
        consts: &HashMap<String, CtValue>,
        mono: &mut Mono,
    ) -> Result<(), ComptimeError> {
        self.mono_type(&mut parameter.ty, consts, mono)?;
        if let Some(origins) = &mut parameter.origin {
            for origin in origins {
                self.mono_expr(origin, consts, mono)?;
            }
        }
        if let Some(default) = &mut parameter.default {
            self.mono_expr(default, consts, mono)?;
        }
        Ok(())
    }

    /// Rewrite one struct method's signature and body (`mono_stmt`'s struct
    /// arm), also used for per-instantiation method clones appended after
    /// the main walk.
    pub(super) fn mono_method(
        &self,
        m: &mut mojito_ast::ast::Method,
        struct_consts: &HashMap<String, CtValue>,
        mono: &mut Mono,
    ) -> Result<(), ComptimeError> {
        let method_consts = consts_without_type_params(struct_consts, &m.type_params);
        let symbolic_base = mono.push_symbolic_type_params(&m.type_params);
        let result = self.mono_method_members(m, &method_consts, mono);
        mono.symbolic_type_params.truncate(symbolic_base);
        result
    }

    fn mono_method_members(
        &self,
        m: &mut mojito_ast::ast::Method,
        method_consts: &HashMap<String, CtValue>,
        mono: &mut Mono,
    ) -> Result<(), ComptimeError> {
        for parameter in &mut m.type_params {
            self.mono_type_parameter(parameter, method_consts, mono)?;
        }
        if let Some(origins) = &mut m.self_origin {
            for origin in origins {
                self.mono_expr(origin, method_consts, mono)?;
            }
        }
        for parameter in &mut m.params {
            self.mono_fn_parameter(parameter, method_consts, mono)?;
        }
        if let Some(error) = &mut m.raises_type {
            self.mono_type(error, method_consts, mono)?;
        }
        if let Some(ret) = &mut m.ret {
            self.mono_type(ret, method_consts, mono)?;
        }
        for condition in &mut m.where_clauses {
            self.mono_expr(condition, method_consts, mono)?;
        }
        mono.push_function_scope();
        if m.has_self {
            mono.bind_value("self", false);
        }
        for parameter in &m.params {
            mono.bind_parameter(parameter);
        }
        let result = self.mono_block_contents(&mut m.body, method_consts, mono);
        mono.pop_function_scope();
        result
    }

    /// Whether `name` is an ordinary generic struct whose closed applications
    /// get per-instantiation method clones: every parameter is a plain type
    /// parameter (no pack, value, or callable-bounded parameter and no
    /// retained origin binder), and the struct is not a specialization
    /// template of its own.
    /// Freeze each computed argument of a struct-typed value parameter of
    /// the generator `name`, a struct or a uniquely named `def`
    /// (`Tagged[Extent.square(4)]`), into the fieldwise
    /// construction of its compile-time value (`Tagged[Extent(4, 4)]`), the
    /// form the checker reads as a frozen struct value. An argument over a
    /// binder, which has no value yet, is left as written.
    pub(super) fn freeze_struct_value_arguments(
        &self,
        name: &str,
        arguments: &mut [ParamArg],
        consts: &HashMap<String, CtValue>,
    ) {
        let computed = |argument: &ParamArg| match argument {
            ParamArg::Value(expression) => matches!(
                expression.kind,
                ExprKind::Call { .. } | ExprKind::MethodCall { .. }
            ),
            ParamArg::Named { value, .. } => matches!(&**value, ParamArg::Value(expression)
                if matches!(expression.kind, ExprKind::Call { .. } | ExprKind::MethodCall { .. })),
            ParamArg::Type(_) => false,
        };
        if !arguments.iter().any(computed) {
            return;
        }
        let type_params = if let Some(info) = self.structs.get(name) {
            info.source_params
        } else {
            let mut defs = self
                .program
                .iter()
                .filter_map(|statement| match &statement.kind {
                    StmtKind::Def {
                        name: declared,
                        type_params,
                        ..
                    } if declared == name => Some(type_params.as_slice()),
                    _ => None,
                });
            match (defs.next(), defs.next()) {
                (Some(type_params), None) => type_params,
                _ => return,
            }
        };
        let struct_valued = |parameter: &TypeParam| matches!(parameter.bounds.as_slice(), [only] if self.structs.contains_key(only));
        if !type_params.iter().any(struct_valued) {
            return;
        }
        let explicit: Vec<&TypeParam> = type_params
            .iter()
            .filter(|parameter| {
                !parameter.infer_only
                    && !matches!(parameter.bounds.as_slice(),
                        [only] if only == "Origin" || only == "OriginSet")
            })
            .collect();
        for (index, argument) in arguments.iter_mut().enumerate() {
            let (parameter, expression) = match argument {
                ParamArg::Value(expression) => (explicit.get(index).copied(), expression),
                ParamArg::Named { name, value } => match &mut **value {
                    ParamArg::Value(expression) => (
                        explicit
                            .iter()
                            .copied()
                            .find(|parameter| parameter.name == *name),
                        expression,
                    ),
                    _ => continue,
                },
                ParamArg::Type(_) => continue,
            };
            if !parameter.is_some_and(struct_valued) {
                continue;
            }
            if let Ok(value @ CtValue::Struct { .. }) = self.eval(expression, consts)
                && let Some(frozen) = value.materialize(expression.span)
            {
                *expression = frozen;
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
                self.freeze_struct_value_arguments(name, arguments, consts);
                Ok(())
            }
            Type::Assoc { base, args, .. } => {
                self.mono_type(base, consts, mono)?;
                for argument in args {
                    self.mono_param_arg(argument, consts, mono)?;
                }
                Ok(())
            }
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
            | Type::ClosedStringLiteral
            | Type::Float64
            | Type::None
            | Type::SelfParam(_)
            | Type::SelfType => Ok(()),
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

    #[allow(clippy::too_many_lines, reason = "TODO: split this pass")]
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
            | ExprKind::None
            | ExprKind::EmptySubscript => Ok(()),
            ExprKind::TString { parts, .. } => {
                for part in parts.iter_mut() {
                    if let TStringPart::Expr(value) = part {
                        self.mono_expr(value, consts, mono)?;
                    }
                }
                Ok(())
            }
            ExprKind::Identifier(name) => {
                // A function-value use of a bound generic pins the abstract
                // template: there is no application to monomorphize against.
                if mono.resolves_top_template(name) && self.bound_generics.contains(name.as_str()) {
                    mono.retain_abstract(name);
                }
                Ok(())
            }
            ExprKind::TypeApply { name, args } => {
                self.freeze_struct_value_arguments(name, args, consts);
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
                // A plain generic struct's or function's compile-time
                // arguments may name specializable instances
                // (`Dict[Variant[Int, String], Int]()`); rewrite those to
                // their concrete names. Template applications resolve their
                // own arguments below.
                if !self.specializable.contains_key(name.as_str())
                    && !self.bound_generics.contains(name.as_str())
                {
                    for argument in param_args.iter_mut() {
                        self.mono_param_arg(argument, consts, mono)?;
                    }
                }
                self.freeze_struct_value_arguments(name, param_args, consts);
                if mono.resolves_top_template(name) && self.specializable.contains_key(name) {
                    let template = self.specializable[name.as_str()];
                    let (vals, kept_type_args, whole_pack_abi) =
                        if self.bound_generics.contains(name.as_str()) {
                            // A call the template serves is left as written,
                            // explicit application included: the elaborator
                            // instantiates the template's MIR.
                            if self.template_serves_def(name, template) {
                                mono.retain_abstract(name);
                                return Ok(());
                            }
                            // Soft resolution: only an explicit application whose
                            // arguments resolve concretely monomorphizes. A bound
                            // violation on a resolved argument is a real error;
                            // any other failure (inference, symbolic arguments)
                            // leaves the call on the template's abstract path and
                            // retains the template.
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
                                // call or symbolic arguments): the call stays on
                                // the template's abstract path.
                                Err(_) => {
                                    mono.retain_abstract(name);
                                    return Ok(());
                                }
                            }
                        } else {
                            let whole_pack_abi =
                                top_level_whole_pack_forwarding_call(template, args)?;
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
                    let mut output_name = mangle(name, &vals)?;
                    if whole_pack_abi {
                        output_name.push_str("$whole_pack");
                    }
                    if mono.queue_specialization(&output_name) {
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
                        mojito_ast::ast::SubscriptArg::Index(value)
                        | mojito_ast::ast::SubscriptArg::Keyword { value, .. } => {
                            self.mono_expr(value, consts, mono)?;
                        }
                        mojito_ast::ast::SubscriptArg::Slice {
                            lower, upper, step, ..
                        }
                        | mojito_ast::ast::SubscriptArg::KeywordSlice {
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
                        mojito_ast::ast::ComprehensionClause::For { var, iter, .. } => {
                            self.mono_expr(iter, consts, mono)?;
                            mono.bind_value(var, false);
                        }
                        mojito_ast::ast::ComprehensionClause::If(condition) => {
                            self.mono_expr(condition, consts, mono)?;
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
            // A lambda's hidden definition monomorphizes like the equivalent
            // nested `def` statement (signature plus body in its own scope).
            ExprKind::Lambda { def } => self.mono_stmt(def, consts, mono),
        }
    }

    /// The clone binders `vals` bind for the origin slots of a type argument
    /// whose parameter no runtime parameter of `template` spells
    /// (`unsafe_alloc[Span[Int, o]](count)`), in binder order: such a clone
    /// declares them explicit, since no argument could infer them.
    pub(super) fn unspelled_clone_binders(&self, template: &Stmt, vals: &[CtValue]) -> Vec<u32> {
        let StmtKind::Def {
            name,
            type_params,
            params,
            ..
        } = &template.kind
        else {
            return Vec::new();
        };
        let mut found = Vec::new();
        let mut values = vals.iter();
        for parameter in type_params {
            if retained_specialization_param(parameter, type_params) {
                continue;
            }
            if classify_ct_param(parameter, type_params, name).is_none() {
                continue;
            }
            let Some(value) = values.next() else {
                break;
            };
            if let CtValue::Type(ty) = value
                && !parameter_spelled(parameter, params)
            {
                self.collect_clone_binders(ty, &mut found);
            }
        }
        let mut indices: Vec<u32> = found.into_iter().map(|(index, _)| index).collect();
        indices.sort_unstable();
        indices.dedup();
        indices
    }

    #[allow(
        clippy::needless_pass_by_value,
        reason = "borrow bundle: destructuring through a reference re-borrows its fields"
    )]
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
        let StmtKind::Def {
            name,
            type_params,
            params,
            ..
        } = &template.kind
        else {
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

            let decl = classify_ct_param(parameter, type_params, name)
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
                        // A pack no collector gathers (`other: Tuple[*Ts]`)
                        // is inferred through a parameter's type, which is
                        // the checker's to solve.
                        let collected = params.iter().any(|parameter| {
                            parameter.kind == ParamKind::Variadic
                                && matches!(&parameter.ty, Type::Named(spread, _) if spread == pack)
                        });
                        if arguments.is_empty() && forwarded_pack_types.is_none() && !collected {
                            return Err(ComptimeError::NotComptime(format!(
                                "type pack '{}' of '{display_name}' is bound by no collector",
                                pack.trim_start_matches('*')
                            )));
                        }
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
                match (&decl, &parameter.default) {
                    (ParamDecl::Value { ty, .. }, Some(default)) => {
                        let evaluated = self.eval(default, &environment).map_err(|_| {
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
                    (
                        ParamDecl::Type {
                            default: Some(default),
                            ..
                        },
                        _,
                    ) => CtValue::Type(default.clone()),
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

fn consts_without_type_params(
    consts: &HashMap<String, CtValue>,
    parameters: &[TypeParam],
) -> HashMap<String, CtValue> {
    let mut inner = consts.clone();
    for parameter in parameters {
        inner.remove(&parameter.name);
        inner.remove(parameter.name.trim_start_matches('*'));
    }
    inner
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

fn param_arg_mentions_any(argument: &ParamArg, names: &[String]) -> bool {
    match argument {
        ParamArg::Type(ty) => type_mentions_any(ty, names),
        ParamArg::Value(expression) => expr_mentions_any(expression, names),
        ParamArg::Named { value, .. } => param_arg_mentions_any(value, names),
    }
}

/// Whether a runtime parameter's type spells the compile-time `parameter`,
/// so a call can infer it from its arguments.
fn parameter_spelled(parameter: &TypeParam, params: &[FnParam]) -> bool {
    let spelled = std::slice::from_ref(&parameter.name);
    params
        .iter()
        .any(|param| type_mentions_any(&param.ty, spelled))
}

fn type_mentions_any(ty: &Type, names: &[String]) -> bool {
    match ty {
        Type::SelfParam(name) => names.iter().any(|candidate| candidate == name),
        Type::Named(name, arguments) => {
            names
                .iter()
                .any(|candidate| candidate == name.trim_start_matches('*'))
                || arguments
                    .iter()
                    .any(|argument| param_arg_mentions_any(argument, names))
        }
        Type::Func { params, ret, .. } => {
            params
                .iter()
                .any(|param| type_mentions_any(&param.ty, names))
                || type_mentions_any(ret, names)
        }
        _ => false,
    }
}

fn expr_mentions_any(expression: &Expr, names: &[String]) -> bool {
    match &expression.kind {
        ExprKind::Identifier(name) => names
            .iter()
            .any(|candidate| candidate == name.trim_start_matches('*')),
        ExprKind::TypeValue(ty) => type_mentions_any(ty, names),
        ExprKind::TypeApply { name, args } => {
            names.iter().any(|candidate| candidate == name)
                || args
                    .iter()
                    .any(|argument| param_arg_mentions_any(argument, names))
        }
        _ => false,
    }
}
