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
        let declaration_site = s.source_span();
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
                // A generic `def` declared directly in another body runs only
                // through the instances the elaborator below MIR mints, so it
                // owns the references its body leaves abstract, as a
                // top-level bound-generic body does.
                let nested_generic = mono.def_depth == 1 && !type_params.is_empty();
                let enclosing_owner = nested_generic
                    .then(|| {
                        mono.abstract_owner
                            .replace(nested_body_owner(&declaration_site))
                    })
                    .flatten();
                mono.def_depth += 1;
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
                mono.def_depth -= 1;
                if nested_generic {
                    mono.abstract_owner = enclosing_owner;
                }
                result
            }
            StmtKind::Struct {
                name,
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
                        // An erased method body of a user struct that mints
                        // clones runs only on the paths
                        // `Elab::unserved_template_uses` accounts for, so a
                        // reference it leaves abstract is owned by the method
                        // rather than unserved. A bundled struct, and a struct
                        // whose parameters mint no clones, keeps no owner.
                        let enclosing = mono.abstract_owner.take();
                        if !mono.in_bundled
                            && (self.instance_template(name) || !m.type_params.is_empty())
                        {
                            mono.abstract_owner = Some(method_owner(name, &m.name));
                        }
                        let walked = self.mono_method(m, &struct_consts, mono);
                        mono.abstract_owner = enclosing;
                        walked?;
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
        mono.def_depth += 1;
        let result = self.mono_block_contents(&mut m.body, method_consts, mono);
        mono.def_depth -= 1;
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

    pub(super) fn instance_template(&self, name: &str) -> bool {
        if self.specializable.contains_key(name) {
            return false;
        }
        let Some(info) = self.structs.get(name) else {
            return false;
        };
        let type_params = info.source_params;
        !type_params.is_empty()
            && classify_ct_params(type_params, name).iter().all(|decl| {
                matches!(
                    decl,
                    ParamDecl::Type {
                        variadic: false,
                        callable_bound: None,
                        ..
                    } | ParamDecl::Value {
                        variadic: false,
                        ..
                    }
                )
            })
            && type_params.iter().all(|parameter| {
                super::specialize::method_parameter_is_baked(parameter, type_params)
            })
    }

    /// Queue a closed application of an instance template for clone minting;
    /// a symbolic application (inside a template body) is left alone.
    pub(super) fn request_instance(
        &self,
        name: &str,
        param_args: &[ParamArg],
        consts: &HashMap<String, CtValue>,
        mono: &mut Mono,
    ) {
        if mono.in_bundled {
            return;
        }
        let Some(info) = self.structs.get(name) else {
            return;
        };
        if param_args.len() > info.source_params.len() {
            return;
        }
        let mut values = Vec::with_capacity(info.source_params.len());
        for argument in param_args {
            let Ok(ty) = self.param_arg_type(argument, consts) else {
                return;
            };
            if !closed_instance_argument(&ty) {
                return;
            }
            values.push(CtValue::Type(Box::new(ty)));
        }
        // Trailing defaulted parameters (`H: Hasher = default_hasher`) fill
        // from their declared defaults.
        for parameter in &info.source_params[param_args.len()..] {
            let Some(default) = &parameter.default else {
                return;
            };
            let Ok(CtValue::Type(ty)) = self.eval(default, consts) else {
                return;
            };
            if !closed_instance_argument(&ty) {
                return;
            }
            values.push(CtValue::Type(ty));
        }
        // Every value is a closed type here, so the key always forms.
        if let Ok(key) = mangle(name, &values)
            && mono.instances_done.insert(key)
        {
            mono.instance_jobs.push_back((name.to_string(), values));
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
                if self.instance_template(name) {
                    self.request_instance(name, arguments, consts, mono);
                } else {
                    self.freeze_struct_value_arguments(name, arguments, consts);
                }
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
                    mono.retain_abstract(name, &source_span, true);
                }
                Ok(())
            }
            ExprKind::TypeApply { name, args } => {
                if self.instance_template(name) {
                    // A static call through an explicit instance
                    // (`Box[Int].filled(7)`) mints that instance's clones.
                    self.request_instance(name, args, consts, mono);
                } else {
                    self.freeze_struct_value_arguments(name, args, consts);
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
                    // A closed constructor application of an ordinary generic
                    // struct (`Optional[Int](5)`) mints that instance's
                    // method clones in this elaboration.
                    if self.instance_template(name) {
                        self.request_instance(name, param_args, consts, mono);
                    }
                }
                self.freeze_struct_value_arguments(name, param_args, consts);
                // A checker-selected scalar `range(...)`: the construction of
                // the range-family struct at its dtype, as the pin's
                // `range[dtype: DType, //]` overloads return it.
                if name == "range"
                    && let Some((template, vals)) = mono
                        .struct_call_targets
                        .get(&source_span.clone().without_syntax())
                        .cloned()
                {
                    let arguments = vals
                        .iter()
                        .map(|value| value.materialize(source_span.span).map(ParamArg::Value))
                        .collect::<Option<Vec<_>>>()
                        .ok_or_else(|| {
                            ComptimeError::NotComptime(format!(
                                "range dtype of '{template}' has no source spelling"
                            ))
                        })?;
                    *name = template;
                    *param_args = arguments;
                    return Ok(());
                }
                if mono.resolves_top_template(name) && self.specializable.contains_key(name) {
                    // Which declaration of an overloaded template family
                    // this call selected; `None` on every other path.
                    let mut selected_decl = None;
                    let (vals, kept_type_args, whole_pack_abi) = if self.overload_family(name) {
                        // A call selecting a member the template serves is
                        // left as written, as a uniquely named served
                        // `def`'s call is: the elaborator instantiates that
                        // declaration's MIR. The checker's selection decides
                        // this even while the call's arguments are symbolic
                        // (a served body's call over its own binder).
                        if self.family_call_is_served(name, &source_span, mono) {
                            return Ok(());
                        }
                        // An overloaded template name cannot be
                        // resolved syntactically at all: explicit `[...]`
                        // arguments name type arguments, not an overload, and
                        // overload selection is the checker's. Inferred and
                        // explicit calls alike are specialized only from the
                        // checker's closed recorded instantiation.
                        // A clone forwarding its own specialized pack
                        // whole (`tally(*a)`) is the one call no check can
                        // record: the checker sees the spread only once it
                        // is expanded, so the collector the call binds
                        // structurally is the declaration.
                        let target =
                            match self.def_request_target(name, &source_span, param_args, mono) {
                                Some((values, kept, decl)) => Some((values, kept, decl, false)),
                                None => self
                                    .forwarded_family_target(
                                        name,
                                        SpecRequest {
                                            param_args,
                                            call_args: args,
                                            kwargs,
                                            consts,
                                            request_site: &request_site,
                                            forwarded_pack_types: None,
                                        },
                                        mono,
                                    )
                                    .map(|(values, kept, decl)| (values, kept, Some(decl), true)),
                            };
                        let Some((values, kept, decl, whole_pack_abi)) = target else {
                            mono.retain_abstract(name, &source_span, false);
                            return Ok(());
                        };
                        selected_decl = decl;
                        (values, kept, whole_pack_abi)
                    } else if self.bound_generics.contains(name.as_str()) {
                        // A call the template serves is left as written,
                        // explicit application included: the elaborator
                        // instantiates the template's MIR.
                        let template = self.specializable[name.as_str()];
                        if self.template_serves_def(name, template, mono) {
                            mono.retain_abstract(name, &source_span, false);
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
                            // call or symbolic arguments): consult the
                            // checker-discovered request for this occurrence
                            // before falling back to the abstract path.
                            Err(_) => {
                                if let Some((values, kept, _)) =
                                    self.def_request_target(name, &source_span, param_args, mono)
                                {
                                    (values, kept, false)
                                } else {
                                    mono.retain_abstract(name, &source_span, false);
                                    return Ok(());
                                }
                            }
                        }
                    } else if self.pack_generics.contains(name.as_str()) {
                        // A type-pack call: element types read syntactically
                        // specialize at once; a call whose elements are not
                        // statically evident (a local, a generic construction,
                        // an origin-bearing temporary) consults the
                        // checker-recorded instantiation for this occurrence
                        // and otherwise keeps the template for the discovery
                        // check. A bound violation is a real error either way.
                        let template = self.specializable[name.as_str()];
                        let whole_pack_abi = top_level_whole_pack_forwarding_call(template, args)?;
                        let resolved =
                            top_level_forwarded_pack_types(template, name, args, kwargs, mono)
                                .and_then(|forwarded| {
                                    self.resolve_spec_args_for(
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
                                    )
                                });
                        match resolved {
                            Ok((values, kept)) if self.pack_values_statically_evident(&values) => {
                                (values, kept, whole_pack_abi)
                            }
                            Err(
                                error @ (ComptimeError::GenericBound(_)
                                | ComptimeError::PackBound(_)),
                            ) => return Err(error),
                            // A syntactic guess that names a generic struct
                            // bare (`Box(7)`, `Named("k", w)`) is not
                            // evident either: its arguments are the
                            // checker's to solve.
                            Ok(_) | Err(_) => {
                                if let Some((values, kept, _)) =
                                    self.def_request_target(name, &source_span, param_args, mono)
                                {
                                    (values, kept, whole_pack_abi)
                                } else {
                                    mono.retained.insert(name.clone());
                                    return Ok(());
                                }
                            }
                        }
                    } else if self.comptime_generics.contains(name.as_str())
                        && omits_required_param(self.specializable[name.as_str()], param_args)
                    {
                        // An inferred application of a compile-time-keyed
                        // template: only the checker can solve its arguments,
                        // so consult the recorded instantiation for this
                        // occurrence, else keep the template as a stub for the
                        // discovery check.
                        let Some((values, kept, _)) =
                            self.def_request_target(name, &source_span, param_args, mono)
                        else {
                            mono.retain_abstract(name, &source_span, false);
                            return Ok(());
                        };
                        (values, kept, false)
                    } else {
                        let template = self.specializable[name.as_str()];
                        let whole_pack_abi = top_level_whole_pack_forwarding_call(template, args)?;
                        let forwarded =
                            top_level_forwarded_pack_types(template, name, args, kwargs, mono)?;
                        let resolved = self.resolve_spec_args_for(
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
                        );
                        let (values, kept) = match resolved {
                            // An explicit application of a compile-time-keyed
                            // template over an enclosing body's own parameters
                            // (`show[T](x)`) stays on the stub, as an inferred
                            // one does.
                            Err(_)
                                if self.comptime_generics.contains(name.as_str())
                                    && param_args_mention_any(
                                        param_args,
                                        &mono.symbolic_type_params,
                                    ) =>
                            {
                                let Some((values, kept, _)) =
                                    self.def_request_target(name, &source_span, param_args, mono)
                                else {
                                    mono.retain_abstract(name, &source_span, false);
                                    return Ok(());
                                };
                                (values, kept)
                            }
                            resolved => resolved?,
                        };
                        (values, kept, whole_pack_abi)
                    };
                    let original = name.clone();
                    let mut output_name = mangle(name, &vals)?;
                    if whole_pack_abi {
                        output_name.push_str("$whole_pack");
                    }
                    if mono.queue_specialization(&output_name, selected_decl) {
                        mono.queue.push_back(Job {
                            orig: original,
                            decl: selected_decl,
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
                method,
                args,
                kwargs,
                ..
            } => {
                mono.record_method_edge(method);
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
        let StmtKind::Def {
            name, type_params, ..
        } = &template.kind
        else {
            return None;
        };
        let mut vals = Vec::new();
        let mut origin_binders = CloneOriginBinders::over_enclosing();
        // The checker's origin tail has no elaborator slot: origins erase from
        // every clone.
        let mut cursor = arguments
            .iter()
            .filter(|argument| !matches!(argument, TyArg::Origin(_)));
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
                    TyArg::Val(CtValue::Expr(_) | CtValue::Deferred(_) | CtValue::Marker(_)) => {
                        continue;
                    }
                    _ => return None,
                }
            }
            let decl = classify_ct_param(parameter, type_params, name)?;
            let value = match (&decl, argument) {
                (
                    ParamDecl::Type {
                        variadic: false, ..
                    },
                    TyArg::Ty(ty),
                ) => {
                    // An origin-slotted struct argument binds its slots to the
                    // clone's own origin binders, a bundled template's as a
                    // user template's. One with no binder to stand for a slot
                    // (`_ListIter[Int]`) keeps the abstract path. The call
                    // infers the binders from its arguments, or supplies
                    // them explicitly where no runtime parameter spells the
                    // type parameter (`request_kept_param_args`), a slot
                    // bound to an enclosing origin parameter by its name.
                    if self.ty_mentions_origin_slotted_struct(ty) {
                        let (bound, _) = self.clone_binding(ty, &mut origin_binders)?;
                        CtValue::Type(Box::new(bound))
                    } else {
                        CtValue::Type(Box::new(ty.clone()))
                    }
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
                    value.clone()
                }
                // A checker-inferred type pack (`show(w)` on `def show[*Ts:
                // Writable](*args: *Ts)`): every element is a checked type
                // satisfying the pack's bounds.
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
                        if bounds
                            .iter()
                            .any(|bound| self.conformance.require(ty, bound).is_err())
                        {
                            return None;
                        }
                    }
                    value.clone()
                }
                // Any other pairing is a drift signal.
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
    /// rewritten call keeps. `None` leaves the call on the abstract path, as
    /// a call its template serves is left ([`Elab::template_serves_def`]).
    fn def_request_target(
        &self,
        name: &str,
        source_span: &SourceSpan,
        param_args: &[ParamArg],
        mono: &Mono,
    ) -> Option<(Vec<CtValue>, Vec<ParamArg>, Option<usize>)> {
        let target = mono
            .def_call_targets
            .get(&source_span.clone().without_syntax())?;
        if target.template != name {
            // A span collision with a different callee (duplicated source
            // provenance): stay abstract.
            return None;
        }
        // An overloaded template name resolves to the declaration
        // the request selected, not to the registry's name-level entry.
        let template = match target.decl {
            Some(_) => self.selected_declaration(name, target.decl),
            None => *self.specializable.get(name)?,
        };
        if self.template_serves_def(name, template, mono) {
            return None;
        }
        let kept = self.request_kept_param_args(template, name, param_args, &target.vals)?;
        Some((target.vals.clone(), kept, target.decl))
    }

    /// Whether the checker's selection for this call of the overload family
    /// `name` — its recorded instantiation, closed or not — is a declaration
    /// the template serves (one that is not specializable), so the call
    /// stays as written.
    fn family_call_is_served(&self, name: &str, source_span: &SourceSpan, mono: &Mono) -> bool {
        let occurrence = source_span.clone().without_syntax();
        let closed = mono
            .def_call_targets
            .get(&occurrence)
            .filter(|target| target.template == name)
            .and_then(|target| target.decl);
        let unclosed = || {
            mono.family_selections
                .get(&occurrence)
                .filter(|(family, _)| family == name)
                .map(|(_, decl)| *decl)
        };
        closed
            .or_else(unclosed)
            .is_some_and(|decl| !self.is_specializable(self.selected_declaration(name, Some(decl))))
    }

    /// The one declaration of the overload family `name` whose positional
    /// collector a whole forward of a specialized runtime pack binds
    /// (`request.call_args` spreads it after exactly the fixed positional
    /// prefix), with its specialization values. `None` when the call
    /// forwards no specialized pack or when several declarations bind it,
    /// which leaves the call to the checker's recorded instantiation.
    fn forwarded_family_target(
        &self,
        name: &str,
        request: SpecRequest<'_>,
        mono: &Mono,
    ) -> Option<(Vec<CtValue>, Vec<ParamArg>, usize)> {
        let forwards_specialized_pack = request.call_args.iter().any(|argument| {
            runtime_pack_spread_source(argument)
                .is_some_and(|pack| mono.resolve_runtime_pack(pack).is_some())
        });
        if !forwards_specialized_pack {
            return None;
        }
        let mut bound = self
            .overload_families
            .get(name)?
            .iter()
            .enumerate()
            .filter(|(_, template)| {
                top_level_whole_pack_forwarding_call(template, request.call_args)
                    .is_ok_and(|forwards| forwards)
            })
            .filter_map(|(index, template)| {
                let forwarded = top_level_forwarded_pack_types(
                    template,
                    name,
                    request.call_args,
                    request.kwargs,
                    mono,
                )
                .ok()??;
                let (values, kept) = self
                    .resolve_spec_args_for(
                        template,
                        name,
                        SpecRequest {
                            forwarded_pack_types: Some(&forwarded),
                            ..request
                        },
                    )
                    .ok()?;
                Some((values, kept, index))
            });
        let only = bound.next()?;
        bound.next().is_none().then_some(only)
    }

    /// The source arguments a request-rewritten call retains: arguments bound
    /// to symbolically retained parameters and to residual kept type
    /// parameters. Dropped parameters' arguments are baked into the clone; a
    /// kept parameter with no source argument contributes nothing (the
    /// checker re-infers it against the clone's residual signature, and the
    /// mangle already discriminates the identity).
    #[allow(
        clippy::unused_self,
        reason = "TODO: make an associated function or use the receiver"
    )]
    pub(super) fn request_kept_param_args(
        &self,
        template: &Stmt,
        display_name: &str,
        param_args: &[ParamArg],
        vals: &[CtValue],
    ) -> Option<Vec<ParamArg>> {
        let StmtKind::Def {
            name,
            type_params,
            params,
            ..
        } = &template.kind
        else {
            return None;
        };
        let bound = bind_spec_param_args(type_params, param_args, display_name).ok()?;
        let mut kept = Vec::new();
        let mut origins = Vec::new();
        let mut values = vals.iter();
        for (parameter, arguments) in type_params.iter().zip(bound) {
            if retained_specialization_param(parameter, type_params) {
                kept.extend(arguments.into_iter().cloned());
                continue;
            }
            let decl = classify_ct_param(parameter, type_params, name)?;
            let value = values.next()?;
            if let CtValue::Type(ty) = value
                && !parameter_spelled(parameter, params)
                && self.ty_mentions_origin_slotted_struct(ty)
            {
                let source = match arguments.as_slice() {
                    [ParamArg::Type(source)] => source,
                    [ParamArg::Named { value, .. }] => match value.as_ref() {
                        ParamArg::Type(source) => source,
                        _ => return None,
                    },
                    _ => return None,
                };
                self.clone_binder_arguments(ty, source, &mut origins)?;
            }
            if matches!(decl, ParamDecl::Type { .. })
                && spec_type_param_substitution(&decl, value).is_none()
            {
                kept.extend(arguments.into_iter().cloned());
            }
        }
        if values.next().is_some() {
            return None;
        }
        // The clone declares an explicit binder for each origin slot of a
        // type argument no runtime parameter spells, and the call supplies
        // the origin the application spelled there, in binder order.
        origins.sort_by_key(|(index, _)| *index);
        let explicit = self.unspelled_clone_binders(template, vals);
        if origins.iter().map(|(index, _)| *index).ne(explicit) {
            return None;
        }
        kept.splice(
            0..0,
            origins
                .into_iter()
                .map(|(_, origin)| ParamArg::Value(origin)),
        );
        Some(kept)
    }

    /// Pair each clone binder `bound` names with the origin `source`, the
    /// application's own spelling of the same type, gives that slot
    /// (`Span[Int, __clone_origin0]` against `Span[Int, origin_of(xs)]`).
    /// `None` when the spelling does not line up with the checked type, as
    /// through an alias.
    fn clone_binder_arguments(
        &self,
        bound: &Ty,
        source: &Type,
        out: &mut Vec<(u32, Expr)>,
    ) -> Option<()> {
        // A pointer's origin argument spells its binder's origin
        // (`Pointer[Int, __clone_origin0]` against `Pointer[Int,
        // origin_of(x)]`).
        if let Ty::Pointer { element, origin } = bound {
            let Type::Named(source_name, source_arguments) = source else {
                return None;
            };
            let [ParamArg::Type(element_source), origin_source] = source_arguments.as_slice()
            else {
                return None;
            };
            if !matches!(source_name.as_str(), "Pointer" | "UnsafePointer") {
                return None;
            }
            self.clone_binder_arguments(element, element_source, out)?;
            if let mojito_types::origin::PointerOrigin::Param {
                id,
                interior,
                subtree,
                ..
            } = origin
                && CloneOriginBinders::name(*id).is_some()
            {
                out.push((
                    u32::MAX - id.0,
                    projected_origin_base(origin_source, interior, *subtree)?,
                ));
            }
            return Some(());
        }
        let (Ty::Struct(name, arguments), Type::Named(source_name, source_arguments)) =
            (bound, source)
        else {
            return (!self.ty_mentions_origin_slotted_struct(bound)).then_some(());
        };
        if source_name != name {
            return None;
        }
        let declared = self.structs.get(name.as_str())?.source_params;
        let slots = bind_spec_param_args(declared, source_arguments, name).ok()?;
        let is_origin = |parameter: &TypeParam| matches!(parameter.bounds.as_slice(), [only] if only == "Origin" || only == "OriginSet");
        let mut origins = arguments.iter().filter_map(|argument| match argument {
            TyArg::Origin(origin) => Some(origin),
            _ => None,
        });
        let mut others = arguments
            .iter()
            .filter(|argument| !matches!(argument, TyArg::Origin(_)));
        for (parameter, spelled) in declared.iter().zip(slots) {
            let spelled = match spelled.as_slice() {
                [ParamArg::Named { value, .. }] => Some(value.as_ref()),
                [only] => Some(*only),
                _ => None,
            };
            if is_origin(parameter) {
                if parameter.infer_only {
                    continue;
                }
                let origin = origins.next()?;
                if let mojito_types::origin::Origin::Param(id) = origin
                    && CloneOriginBinders::name(*id).is_some()
                {
                    out.push((u32::MAX - id.0, origin_argument_expression(spelled?)?));
                }
            } else if !parameter.is_origin_mutability_binder(declared)
                && let Some(TyArg::Ty(inner)) = others.next()
                && self.ty_mentions_origin_slotted_struct(inner)
            {
                let Some(ParamArg::Type(inner_source)) = spelled else {
                    return None;
                };
                self.clone_binder_arguments(inner, inner_source, out)?;
            }
        }
        (origins.next().is_none() && others.next().is_none()).then_some(())
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

/// Whether a call leaves a parameter of `template` that needs an argument
/// without one.
fn omits_required_param(template: &Stmt, param_args: &[ParamArg]) -> bool {
    !omitted_required_params(template, param_args).is_empty()
}

/// The parameters of `template` a call leaves without an argument: no source
/// argument, no default, not infer-only, and not a symbolically retained
/// binder. A source list that does not bind at all omits nothing; it keeps its
/// binding diagnostic.
fn omitted_required_params<'t>(template: &'t Stmt, param_args: &[ParamArg]) -> Vec<&'t TypeParam> {
    let StmtKind::Def { type_params, .. } = &template.kind else {
        return Vec::new();
    };
    let Ok(bound) = bind_spec_param_args(type_params, param_args, "") else {
        return Vec::new();
    };
    type_params
        .iter()
        .zip(bound)
        .filter(|(parameter, arguments)| {
            arguments.is_empty()
                && !parameter.infer_only
                && parameter.default.is_none()
                && !retained_specialization_param(parameter, type_params)
        })
        .map(|(parameter, _)| parameter)
        .collect()
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

/// Whether a type is a closed instance argument: a scalar, or a struct
/// application whose type arguments are closed. Anything else — a type
/// parameter, an associated or dependent type — keeps the erased path
/// (conservative: no clone is minted for it). An origin tail never opens an
/// instance: origins erase from the runtime ABI, so every origin-differing
/// instance shares one clone.
fn closed_instance_argument(ty: &Ty) -> bool {
    match ty {
        Ty::Int | Ty::UInt | Ty::Bool | Ty::Float64 | Ty::StringLiteral | Ty::None => true,
        Ty::Simd { dtype, width } => !dtype.is_symbolic() && !width.is_symbolic(),
        Ty::Struct(_, arguments) => arguments.iter().all(|argument| match argument {
            TyArg::Ty(ty) => closed_instance_argument(ty),
            TyArg::Val(value) => !matches!(
                value,
                CtValue::Expr(_) | CtValue::Deferred(_) | CtValue::Marker(_)
            ),
            TyArg::Origin(_) => true,
        }),
        _ => false,
    }
}

/// Whether any compile-time argument spells one of `names` (an enclosing
/// declaration's type parameters, packs included) anywhere in a type or
/// type-valued expression position.
fn param_args_mention_any(param_args: &[ParamArg], names: &[String]) -> bool {
    param_args
        .iter()
        .any(|argument| param_arg_mentions_any(argument, names))
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

/// The origin an application spells in an origin slot, as the expression an
/// explicit clone binder is supplied with: a value (`origin_of(xs)`, `o`) or
/// a bare name the parser read as a type (`__clone_origin0` in a clone body).
fn origin_argument_expression(spelled: &ParamArg) -> Option<Expr> {
    match spelled {
        ParamArg::Value(expression) => Some(expression.clone()),
        ParamArg::Type(Type::Named(name, arguments)) if arguments.is_empty() => {
            Some(Expr::new(ExprKind::Identifier(name.clone()), (0, 0)))
        }
        _ => None,
    }
}

/// The origin a binder pointer's spelled origin argument binds its binder
/// to: the argument with the binder's trailing `._subtree` and
/// `._get_owned_interior["tag"]` projections peeled, in type-annotation or
/// expression spelling (`origin_of(x)._get_owned_interior["element"]`
/// binds `origin_of(x)`).
fn projected_origin_base(spelled: &ParamArg, interior: &[String], subtree: bool) -> Option<Expr> {
    let mut spelled = spelled.clone();
    if subtree {
        spelled = match spelled {
            ParamArg::Type(Type::Assoc { base, name, args })
                if name == "_subtree" && args.is_empty() =>
            {
                ParamArg::Type(*base)
            }
            ParamArg::Value(Expr {
                kind: ExprKind::Member { object, field },
                ..
            }) if field == "_subtree" => ParamArg::Value(*object),
            _ => return None,
        };
    }
    for tag in interior.iter().rev() {
        spelled = match spelled {
            ParamArg::Type(Type::IndexedProjection { base, index }) => match (*base, &index.kind) {
                (Type::Assoc { base, name, args }, ExprKind::Str(spelled_tag))
                    if name == "_get_owned_interior" && args.is_empty() && spelled_tag == tag =>
                {
                    ParamArg::Type(*base)
                }
                _ => return None,
            },
            ParamArg::Value(Expr {
                kind: ExprKind::Index { object, index },
                ..
            }) => match (object.kind, &index.kind) {
                (ExprKind::Member { object, field }, ExprKind::Str(spelled_tag))
                    if field == "_get_owned_interior" && spelled_tag == tag =>
                {
                    ParamArg::Value(*object)
                }
                _ => return None,
            },
            _ => return None,
        };
    }
    origin_argument_expression(&spelled)
}
