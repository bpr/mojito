//! The `Elab` elaboration driver methods.

use super::*;

impl<'a> Elab<'a> {
    pub(super) fn burn(&self) -> Result<(), ComptimeError> {
        let f = self
            .fuel
            .get()
            .checked_sub(1)
            .ok_or(ComptimeError::QuotaExceeded)?;
        self.fuel.set(f);
        Ok(())
    }

    /// Elaborate a block, resolving `comptime` constructs. `in_fn` is true inside a
    /// function/method body (so a comptime constant there is *not* module-level).
    pub(super) fn block(
        &self,
        stmts: &[Stmt],
        env: &mut HashMap<String, CtValue>,
        in_fn: bool,
    ) -> Result<Vec<Stmt>, ComptimeError> {
        let mut out = Vec::new();
        // Type handles bound by `comptime T = ...` in this block have no
        // runtime representation, so every later statement of the block
        // materializes its uses of the alias — `isa[T]()`, `x: T`, `T()` —
        // as the bound type (an unrolled `comptime for` body is its own block,
        // so each iteration's `comptime T = Self.Ts[i]` binds separately).
        let mut type_aliases: HashMap<String, CtValue> = HashMap::new();
        let mut source_aliases: HashMap<String, Type> = HashMap::new();
        for stmt in stmts {
            let first_new = out.len();
            self.stmt(stmt, env, in_fn, &mut out)?;
            if !type_aliases.is_empty() {
                let subs: Subs = &|name| type_aliases.get(name).cloned();
                for statement in &mut out[first_new..] {
                    *statement = rewrite_stmt_cloned(statement, subs, true);
                }
                substitute_type_bindings_in_block(&mut out[first_new..], &source_aliases);
            }
            if let StmtKind::Comptime {
                name, type_params, ..
            } = &stmt.kind
                && type_params.is_empty()
                && let Some(value @ CtValue::Type(ty)) = env.get(name)
                && let Some(source) = source_type_from_ty(ty)
            {
                type_aliases.insert(name.clone(), value.clone());
                source_aliases.insert(name.clone(), source);
            }
            if let Some(source) = stmt.module.as_deref() {
                mojito_ast::ast::stamp_source(&mut out[first_new..], source);
            }
        }
        Ok(out)
    }

    pub(super) fn stmt(
        &self,
        stmt: &Stmt,
        env: &mut HashMap<String, CtValue>,
        in_fn: bool,
        out: &mut Vec<Stmt>,
    ) -> Result<(), ComptimeError> {
        let span = stmt.span;
        match &stmt.kind {
            StmtKind::Comptime {
                name,
                type_params,
                ty,
                where_clauses,
                value,
            } => {
                if !type_params.is_empty() {
                    // A generic alias registers for the checker; record the
                    // module-scope declaration here too so an application in a
                    // later `comptime if` condition can evaluate before the
                    // branches are pruned.
                    if !in_fn {
                        self.generic_aliases
                            .borrow_mut()
                            .insert(name.clone(), (type_params.clone(), (*value).clone()));
                    }
                    out.push(stmt.clone());
                    return Ok(());
                }
                let v = self.eval(value, env)?;
                if !in_fn {
                    self.top_consts.borrow_mut().insert(name.clone(), v.clone());
                }
                // Fold the definition to its literal value, so the checker and
                // runtime see a constant (and a CTFE-computed `Int`, which the
                // checker's own folder can't evaluate, becomes usable as a value
                // parameter and materializes cleanly).
                env.insert(name.clone(), v);
                // Type and reflection handles have no runtime representation.
                // Keep them only in the elaboration environment; subsequent
                // comptime expressions consume them before checking/lowering.
                if let Some(value) = env[name].materialize(span) {
                    out.push(mk(
                        StmtKind::Comptime {
                            name: name.clone(),
                            type_params: type_params.clone(),
                            ty: ty.clone(),
                            where_clauses: where_clauses.clone(),
                            value,
                        },
                        span,
                    ));
                }
            }
            StmtKind::ComptimeIf { branches, orelse } => {
                for (cond, body) in branches {
                    if self.eval(cond, env)?.as_bool("comptime if condition")? {
                        out.extend(self.block(body, env, in_fn)?);
                        return Ok(());
                    }
                }
                if let Some(body) = orelse {
                    out.extend(self.block(body, env, in_fn)?);
                }
            }
            StmtKind::ComptimeFor { var, iter, body } => {
                for v in self.eval_iter(iter, env)? {
                    self.burn()?;
                    let subs: Subs = &|n| (n == var).then(|| v.clone());
                    let substituted: Vec<Stmt> = body
                        .iter()
                        .map(|s| rewrite_stmt_cloned(s, subs, false))
                        .collect();
                    out.extend(self.block(&substituted, env, in_fn)?);
                }
            }
            StmtKind::VarDecl { name, ty, value } => {
                let ty = ty
                    .as_ref()
                    .map(|ty| self.resolve_reflected_type(ty, env))
                    .transpose()?;
                out.push(mk(
                    StmtKind::VarDecl {
                        name: name.clone(),
                        ty,
                        value: value.clone(),
                    },
                    span,
                ));
            }
            StmtKind::If { branches, orelse } => {
                let branches = branches
                    .iter()
                    .map(|(c, b)| Ok((c.clone(), self.block(b, env, in_fn)?)))
                    .collect::<Result<Vec<_>, ComptimeError>>()?;
                let orelse = self.opt_block(orelse, env, in_fn)?;
                out.push(mk(StmtKind::If { branches, orelse }, span));
            }
            StmtKind::While { cond, body, orelse } => {
                let body = self.block(body, env, in_fn)?;
                let orelse = self.opt_block(orelse, env, in_fn)?;
                out.push(mk(
                    StmtKind::While {
                        cond: cond.clone(),
                        body,
                        orelse,
                    },
                    span,
                ));
            }
            StmtKind::For {
                var,
                binding,
                iter,
                body,
                orelse,
            } => {
                let body = self.block(body, env, in_fn)?;
                let orelse = self.opt_block(orelse, env, in_fn)?;
                out.push(mk(
                    StmtKind::For {
                        var: var.clone(),
                        binding: *binding,
                        iter: iter.clone(),
                        body,
                        orelse,
                    },
                    span,
                ));
            }
            StmtKind::Try {
                body,
                except,
                orelse,
                finalbody,
            } => {
                let body = self.block(body, env, in_fn)?;
                let except = match except {
                    Some((n, b)) => Some((n.clone(), self.block(b, env, in_fn)?)),
                    None => None,
                };
                let orelse = self.opt_block(orelse, env, in_fn)?;
                let finalbody = self.opt_block(finalbody, env, in_fn)?;
                out.push(mk(
                    StmtKind::Try {
                        body,
                        except,
                        orelse,
                        finalbody,
                    },
                    span,
                ));
            }
            StmtKind::With { items, body } => {
                let mut nested = self.block(body, env, in_fn)?;
                for (index, item) in items.iter().enumerate().rev() {
                    let manager = format!("$with{}_{}", span.0, index);
                    let manager_expr = Expr::new(ExprKind::Identifier(manager.clone()), span);
                    let enter = Expr::new(
                        ExprKind::MethodCall {
                            object: Box::new(manager_expr.clone()),
                            method: "__enter__".to_string(),
                            args: Vec::new(),
                            kwargs: Vec::new(),
                        },
                        span,
                    );
                    let enter_statement = match &item.var {
                        Some(name) => mk(
                            StmtKind::VarDecl {
                                name: name.clone(),
                                ty: None,
                                value: enter,
                            },
                            span,
                        ),
                        None => mk(StmtKind::Expr(enter), span),
                    };
                    let exit = Expr::new(
                        ExprKind::MethodCall {
                            object: Box::new(manager_expr),
                            method: "__exit__".to_string(),
                            args: Vec::new(),
                            kwargs: Vec::new(),
                        },
                        span,
                    );
                    nested = vec![
                        mk(
                            StmtKind::VarDecl {
                                name: manager,
                                ty: None,
                                value: item.context.clone(),
                            },
                            span,
                        ),
                        enter_statement,
                        mk(
                            StmtKind::Try {
                                body: nested,
                                except: None,
                                orelse: None,
                                finalbody: Some(vec![mk(StmtKind::Expr(exit), span)]),
                            },
                            span,
                        ),
                    ];
                }
                out.extend(nested);
            }
            StmtKind::Def {
                name,
                decorators,
                type_params,
                params,
                positional_only,
                keyword_only,
                captures,
                raises,
                raises_type,
                ret,
                where_clauses,
                body,
            } => {
                // A comptime-dependent generic template can't be elaborated now (its
                // parameter value is unknown); keep it verbatim for monomorphization.
                if is_specializable_declaration(stmt) {
                    out.push(stmt.clone());
                    return Ok(());
                }
                let body = self.block(body, env, true)?;
                out.push(mk(
                    StmtKind::Def {
                        name: name.clone(),
                        decorators: decorators.clone(),
                        type_params: type_params.clone(),
                        params: params.clone(),
                        positional_only: *positional_only,
                        keyword_only: *keyword_only,
                        captures: captures.clone(),
                        raises: *raises,
                        raises_type: raises_type.clone(),
                        ret: ret.clone(),
                        where_clauses: where_clauses.clone(),
                        body,
                    },
                    span,
                ));
            }
            StmtKind::Struct {
                name,
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
                template_shell,
            } => {
                // A variadic struct template's members reference the unbound pack;
                // keep it verbatim for monomorphization (mirrors def templates).
                // DType-/struct-valued parameter templates are kept the same way.
                if self.is_specializable(stmt) {
                    out.push(stmt.clone());
                    return Ok(());
                }
                let mut methods = methods
                    .iter()
                    .map(|m| {
                        let mut m = m.clone();
                        m.body = match self.block(&m.body, env, true) {
                            Ok(body) => body,
                            // A method whose body only elaborates with the
                            // struct's parameters (a `comptime if` on
                            // `Self.T`) or its own (`comptime if U == Int`,
                            // a `comptime for` over a method pack) bound
                            // becomes a trap stub on the template; every
                            // concrete call retargets to a per-instantiation
                            // or per-call clone, which folds it bound.
                            Err(error)
                                if names_struct_parameter(&error, type_params)
                                    || names_struct_parameter(&error, &m.type_params) =>
                            {
                                vec![super::specialize::unspecialized_method_stub(name, &m)]
                            }
                            Err(error) => return Err(error),
                        };
                        Ok(m)
                    })
                    .collect::<Result<Vec<_>, ComptimeError>>()?;
                // Checker-discovered instantiations of the struct's own
                // generic methods (`f.fields(1, "a")`, `b.kind[Int]()` on a
                // non-generic struct) mint per-call clones with the method's
                // parameters baked; a closed instance of a generic struct
                // mints its clones in `generate_instance_clones` instead.
                if !self.is_specializable(stmt) {
                    let requests = self.method_requests.get(name.as_str());
                    for method in &stmt_methods(stmt) {
                        methods.extend(self.per_call_method_clones(
                            method,
                            requests.map(Vec::as_slice).unwrap_or(&[]),
                            &[],
                            &[],
                            None,
                            env,
                        ));
                    }
                }
                out.push(mk(
                    StmtKind::Struct {
                        name: name.clone(),
                        decorators: decorators.clone(),
                        type_params: type_params.clone(),
                        conforms: conforms.clone(),
                        callable_conformance: callable_conformance.clone(),
                        conformance_conditions: conformance_conditions.clone(),
                        where_clauses: where_clauses.clone(),
                        fields: fields.clone(),
                        associated: associated.clone(),
                        methods,
                        fieldwise_init: *fieldwise_init,
                        template_shell: *template_shell,
                    },
                    span,
                ));
            }
            _ => out.push(stmt.clone()),
        }
        Ok(())
    }

    pub(super) fn opt_block(
        &self,
        block: &Option<Vec<Stmt>>,
        env: &mut HashMap<String, CtValue>,
        in_fn: bool,
    ) -> Result<Option<Vec<Stmt>>, ComptimeError> {
        match block {
            Some(b) => Ok(Some(self.block(b, env, in_fn)?)),
            None => Ok(None),
        }
    }

    pub(super) fn resolve_ct_arg(
        &self,
        decl: &ParamDecl,
        arg: &ParamArg,
        scope: &HashMap<String, CtValue>,
    ) -> Result<CtValue, ComptimeError> {
        match decl {
            ParamDecl::Type { name, .. } => match arg {
                ParamArg::Type(ty) => self
                    .type_from_anno(ty, scope)
                    .map(|ty| CtValue::Type(Box::new(ty))),
                ParamArg::Value(Expr {
                    kind: ExprKind::Identifier(id),
                    ..
                }) => self.type_value(id, &[], scope),
                ParamArg::Value(Expr {
                    kind: ExprKind::TypeApply { name, args },
                    ..
                }) => self.type_value(name, args, scope),
                ParamArg::Value(expr) => Err(ComptimeError::NotComptime(format!(
                    "type parameter '{name}' needs a type argument, got {expr:?}"
                ))),
                ParamArg::Named { value, .. } => self.resolve_ct_arg(decl, value, scope),
            },
            ParamDecl::Value { name, ty, .. } => match arg {
                ParamArg::Value(expr) => {
                    let value = self.eval(expr, scope)?;
                    materialize_ct_value(value.clone(), ty).ok_or_else(|| {
                        ComptimeError::NotComptime(format!(
                            "value parameter '{name}' expects {ty}, got {value}"
                        ))
                    })
                }
                ParamArg::Type(_) => Err(ComptimeError::NotComptime(format!(
                    "value parameter '{name}' expects a compile-time {ty}, got a type argument"
                ))),
                ParamArg::Named { value, .. } => self.resolve_ct_arg(decl, value, scope),
            },
        }
    }

    pub(super) fn type_value(
        &self,
        name: &str,
        args: &[ParamArg],
        scope: &HashMap<String, CtValue>,
    ) -> Result<CtValue, ComptimeError> {
        self.type_from_name(name, args, scope)
            .map(|ty| CtValue::Type(Box::new(ty)))
    }

    /// The built-in type predicate `is_same_type[T, U]()` (roadmap milestone 7): resolve both
    /// type parameters and compare them for equality, yielding a compile-time
    /// `Bool`. Takes exactly two type parameters and no value arguments.
    pub(super) fn eval_is_same_type(
        &self,
        param_args: &[ParamArg],
        args: &[Expr],
        scope: &HashMap<String, CtValue>,
    ) -> Result<CtValue, ComptimeError> {
        if param_args.len() != 2 || !args.is_empty() {
            return Err(ComptimeError::Arity(
                "is_same_type[T, U]() takes two type parameters and no arguments".to_string(),
            ));
        }
        let a = self.param_arg_type(&param_args[0], scope)?;
        let b = self.param_arg_type(&param_args[1], scope)?;
        Ok(CtValue::Bool(a == b))
    }

    /// Resolve a `[...]` argument that is expected to be a **type** (a type
    /// annotation, a bare type name, or a parameterized type) to a `Ty`.
    pub(super) fn param_arg_type(
        &self,
        arg: &ParamArg,
        scope: &HashMap<String, CtValue>,
    ) -> Result<Ty, ComptimeError> {
        match arg {
            ParamArg::Type(t) => self.type_from_anno(t, scope),
            ParamArg::Value(Expr {
                kind: ExprKind::Identifier(id),
                ..
            }) => self.type_from_name(id, &[], scope),
            ParamArg::Value(Expr {
                kind: ExprKind::TypeApply { name, args },
                ..
            }) => self.type_from_name(name, args, scope),
            ParamArg::Value(expr) => match self.eval(expr, scope)? {
                CtValue::Type(ty) => Ok(*ty),
                _ => Err(ComptimeError::NotComptime(
                    "expected a type argument".to_string(),
                )),
            },
            ParamArg::Named { value, .. } => self.param_arg_type(value, scope),
        }
    }

    pub(super) fn type_from_anno(
        &self,
        ty: &Type,
        scope: &HashMap<String, CtValue>,
    ) -> Result<Ty, ComptimeError> {
        match ty {
            Type::Int => Ok(Ty::Int),
            Type::UInt => Ok(Ty::UInt),
            Type::Bool => Ok(Ty::Bool),
            Type::StringLiteral => Ok(Ty::StringLiteral),
            Type::Float64 => Ok(Ty::Float64),
            Type::None => Ok(Ty::None),
            Type::Named(name, args) => self.type_from_name(name, args, scope),
            Type::SelfParam(name) => match scope.get(name) {
                Some(CtValue::Type(ty)) => Ok((**ty).clone()),
                Some(_) => Err(ComptimeError::NotComptime(format!(
                    "Self.{name} is not type-valued"
                ))),
                None => Err(ComptimeError::NotComptime(format!(
                    "unknown compile-time type Self.{name}"
                ))),
            },
            Type::Assoc { base, name, .. } => {
                if let Type::Named(binding, args) = &**base
                    && args.is_empty()
                    && name == "T"
                    && let Some(CtValue::Reflected(ty)) = scope.get(binding)
                {
                    return Ok((**ty).clone());
                }
                let base = self.type_from_anno(base, scope)?;
                match self.associated_value(&base, name)? {
                    CtValue::Type(ty) => Ok(*ty),
                    _ => Err(ComptimeError::NotComptime(format!(
                        "{}.{name} is not type-valued",
                        base
                    ))),
                }
            }
            Type::IndexedProjection { base, index } => {
                let Type::Assoc {
                    base: associated_base,
                    name,
                    ..
                } = base.as_ref()
                else {
                    return Err(ComptimeError::NotComptime(
                        "dependent type indexing requires an associated type sequence".to_string(),
                    ));
                };
                let base_ty = self.type_from_anno(associated_base, scope)?;
                let values = match self.associated_value(&base_ty, name)? {
                    CtValue::Tuple(values) | CtValue::List(values) => values,
                    _ => {
                        return Err(ComptimeError::NotComptime(format!(
                            "{base_ty}.{name} is not a type sequence"
                        )));
                    }
                };
                let index = self.eval(index, scope)?.as_int("dependent type index")?;
                match usize::try_from(index)
                    .ok()
                    .and_then(|position| values.get(position))
                {
                    Some(CtValue::Type(ty)) => Ok((**ty).clone()),
                    Some(_) => Err(ComptimeError::NotComptime(format!(
                        "{base_ty}.{name}[{index}] is not type-valued"
                    ))),
                    None => Err(ComptimeError::BadArithmetic(format!(
                        "dependent type index {index} out of range"
                    ))),
                }
            }
            Type::Ref { referent, origin } => {
                let [origin] = origin.as_deref().ok_or_else(|| {
                    ComptimeError::NotComptime(
                        "reference type arguments require one explicit origin".to_string(),
                    )
                })?
                else {
                    return Err(ComptimeError::NotComptime(
                        "reference type arguments require one explicit origin".to_string(),
                    ));
                };
                let ExprKind::Identifier(origin_name) = &origin.kind else {
                    return Err(ComptimeError::NotComptime(
                        "reference type arguments require a named origin".to_string(),
                    ));
                };
                let referent = Box::new(self.type_from_anno(referent, scope)?);
                if origin_name == "UntrackedOrigin" {
                    return Ok(Ty::Ref(mojito_types::origin::RefTy {
                        referent,
                        origin: mojito_types::origin::Origin::Untracked { mutable: false },
                        mutability: mojito_types::origin::Mutability::Immutable,
                    }));
                }
                let mut reference = scope
                    .get(origin_name)
                    .and_then(decode_ct_origin_marker)
                    .ok_or_else(|| {
                        ComptimeError::NotComptime(format!(
                            "unknown compile-time origin '{origin_name}' in reference type argument"
                        ))
                    })?;
                reference.referent = referent;
                Ok(Ty::Ref(reference))
            }
            Type::SelfType | Type::Func { .. } | Type::MaterializedCallable(_) => Err(
                ComptimeError::NotComptime("unsupported compile-time type argument".to_string()),
            ),
        }
    }

    pub(super) fn type_from_name(
        &self,
        name: &str,
        args: &[ParamArg],
        scope: &HashMap<String, CtValue>,
    ) -> Result<Ty, ComptimeError> {
        if args.is_empty() {
            if let Some(CtValue::Type(ty)) = scope.get(name) {
                return Ok((**ty).clone());
            }
            if let Some(ty) = scalar_type_name(name) {
                return Ok(ty);
            }
        }
        // In type-argument grammar, `types[i]` is represented as a named type
        // application. A reflected `field_types()` result is a compile-time
        // sequence of type values, so interpret that spelling as dependent
        // type-list indexing.
        if let Some(CtValue::Tuple(values) | CtValue::List(values)) = scope.get(name)
            && let [ParamArg::Value(index)] = args
        {
            let index = self.eval(index, scope)?.as_int("type-list index")?;
            return match values.get(index as usize) {
                Some(CtValue::Type(ty)) => Ok((**ty).clone()),
                Some(_) => Err(ComptimeError::NotComptime(format!(
                    "'{name}[{index}]' is not type-valued"
                ))),
                None => Err(ComptimeError::BadArithmetic(format!(
                    "type-list index {index} out of range"
                ))),
            };
        }
        let Some(info) = self.structs.get(name) else {
            return Err(ComptimeError::NotComptime(format!(
                "'{name}' is not a compile-time type"
            )));
        };
        // Omitted trailing arguments fill from declared type-parameter
        // defaults (`Set[Int]` is `Set[Int, default_hasher]`), matching the
        // def-template default fill in `resolve_spec_args_for`. A default
        // classification could not resolve (a module alias) evaluates from
        // its source expression instead.
        let source_defaults: Vec<Option<&Expr>> = info
            .source_params
            .iter()
            .filter(|tp| classify_ct_param(tp, info.source_params).is_some())
            .map(|tp| tp.default.as_ref())
            .collect();
        let defaults_fill = args.len() < info.decls.len()
            && info
                .decls
                .iter()
                .zip(&source_defaults)
                .skip(args.len())
                .all(|(decl, source_default)| {
                    matches!(
                        decl,
                        ParamDecl::Type {
                            default: Some(_),
                            ..
                        }
                    ) || (matches!(decl, ParamDecl::Type { .. }) && source_default.is_some())
                });
        if args.len() > info.decls.len() || (args.len() < info.decls.len() && !defaults_fill) {
            return Err(ComptimeError::Arity(format!(
                "type '{name}' expects {} compile-time argument(s), got {}",
                info.decls.len(),
                args.len()
            )));
        }
        let tyargs = info
            .decls
            .iter()
            .enumerate()
            .map(|(index, decl)| {
                let Some(arg) = args.get(index) else {
                    if let ParamDecl::Type {
                        default: Some(default),
                        ..
                    } = decl
                    {
                        return Ok(TyArg::Ty((**default).clone()));
                    }
                    let default = source_defaults
                        .get(index)
                        .copied()
                        .flatten()
                        .expect("defaults_fill established trailing type defaults");
                    return match self.eval(default, scope)? {
                        CtValue::Type(ty) => Ok(TyArg::Ty(*ty)),
                        other => Err(ComptimeError::NotComptime(format!(
                            "default for type parameter '{}' of '{name}' is not a type: {other}",
                            decl.name()
                        ))),
                    };
                };
                let value = self.resolve_ct_arg(decl, arg, scope)?;
                match (decl, value) {
                    (ParamDecl::Type { .. }, CtValue::Type(ty)) => Ok(TyArg::Ty(*ty)),
                    (ParamDecl::Type { name, .. }, _) => Err(ComptimeError::NotComptime(format!(
                        "type parameter '{name}' needs a type argument"
                    ))),
                    (ParamDecl::Value { .. }, value) => Ok(TyArg::Val(value)),
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Ty::Struct(name.to_string(), tyargs))
    }

    pub(super) fn associated_value(
        &self,
        base: &Ty,
        member: &str,
    ) -> Result<CtValue, ComptimeError> {
        let Ty::Struct(name, args) = base else {
            return Err(ComptimeError::NotComptime(format!(
                "type '{base}' has no compile-time member '{member}'"
            )));
        };
        let info = self.structs.get(name).ok_or_else(|| {
            ComptimeError::NotComptime(format!("unknown compile-time struct '{name}'"))
        })?;
        let assoc = info
            .associated
            .iter()
            .find(|a| a.name == member)
            .ok_or_else(|| {
                ComptimeError::NotComptime(format!(
                    "type '{base}' has no compile-time member '{member}'"
                ))
            })?;
        let mut env = HashMap::new();
        for (decl, arg) in info.decls.iter().zip(args) {
            match (decl, arg) {
                (ParamDecl::Type { name, .. }, TyArg::Ty(ty)) => {
                    env.insert(name.clone(), CtValue::Type(Box::new(ty.clone())));
                }
                (ParamDecl::Value { name, .. }, TyArg::Val(value)) => {
                    env.insert(name.clone(), value.clone());
                }
                _ => {}
            }
        }
        self.eval(&assoc.value, &env)
    }
}

/// Whether an elaboration error names the enclosing struct's `Self` or one
/// of its compile-time parameters — a body that needs the parameters bound.
fn names_struct_parameter(error: &ComptimeError, type_params: &[TypeParam]) -> bool {
    if type_params.is_empty() {
        return false;
    }
    let text = error.to_string();
    text.contains("'Self'")
        || type_params.iter().any(|parameter| {
            text.contains(&format!("'{}'", parameter.name.trim_start_matches('*')))
        })
}

/// The source methods of a struct statement (empty for any other statement).
fn stmt_methods(stmt: &Stmt) -> Vec<mojito_ast::ast::Method> {
    match &stmt.kind {
        StmtKind::Struct { methods, .. } => methods.clone(),
        _ => Vec::new(),
    }
}
