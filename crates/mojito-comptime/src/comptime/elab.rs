//! The `Elab` elaboration driver methods.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_ast::ast::Method;
use mojito_ast::visit::{MutVisitor, walk_block_mut};
use mojito_common::error::TypeError;
use mojito_types::types::TyKind;

impl Elab<'_> {
    /// Elaborate a block, resolving `comptime` constructs. `in_fn` is true inside a
    /// function/method body (so a comptime constant there is *not* module-level).
    pub(super) fn block(
        &self,
        stmts: &[Stmt],
        env: &mut ConstEnv<'_>,
        in_fn: bool,
    ) -> Result<Vec<Stmt>, ComptimeError> {
        // A local `comptime` a template body keeps is a binder of its own
        // block alone ([`Self::keep_template_comptime_binding`]).
        let binders = self.template_binders.borrow().last().cloned();
        let Some(binders) = binders else {
            return self.block_statements(stmts, env, in_fn);
        };
        self.template_binders.borrow_mut().push(binders);
        let block = self.block_statements(stmts, env, in_fn);
        self.template_binders.borrow_mut().pop();
        block
    }

    /// Elaborate the statements of a block.
    pub(super) fn block_statements(
        &self,
        stmts: &[Stmt],
        env: &mut ConstEnv<'_>,
        in_fn: bool,
    ) -> Result<Vec<Stmt>, ComptimeError> {
        let mut out = Vec::new();
        // The pack elements the template's aliases denote: their source
        // spellings, and their parameter expressions as written.
        let mut element_sources: HashMap<String, Type> = HashMap::new();
        let mut element_aliases: HashMap<String, Expr> = HashMap::new();
        for stmt in stmts {
            // `comptime T = Ts[i]` over a template's own type pack is the
            // dependent element it denotes: later statements spell it
            // `Ts[i]`, and the elaborator below MIR closes it per instance.
            if let Some((name, element)) = self.template_pack_element_alias(&stmt.kind) {
                // The alias names a binder from here on, so a `comptime if`
                // over it stays in the template.
                if let Some(binders) = self.template_binders.borrow_mut().last_mut() {
                    binders.insert(name.clone());
                }
                if let StmtKind::Comptime { value, .. } = &stmt.kind {
                    element_aliases.insert(name.clone(), (*value).clone());
                }
                element_sources.insert(name, element);
                // The check binds the alias too, so a read of it in an
                // expression (`F == Int`) is the element it names.
                out.push(stmt.clone());
                continue;
            }
            let first_new = out.len();
            let resolved = contextual_comptime(stmt);
            self.stmt(resolved.as_ref().unwrap_or(stmt), env, in_fn, &mut out)?;
            // Runtime uses of the bindings elaborated so far cross explicitly
            // (`materialize[X]()`, `comptime(e)`) or are rejected.
            self.fold_runtime_crossings(&mut out[first_new..], env)?;
            if !element_sources.is_empty() {
                substitute_type_bindings_in_block(&mut out[first_new..], &|name| {
                    element_sources.get(name).cloned()
                });
            }
            if !element_aliases.is_empty() {
                spell_aliases_in_conditions(&mut out[first_new..], &element_aliases);
            }
            if let Some(source) = stmt.module.as_deref() {
                mojito_ast::ast::stamp_source(&mut out[first_new..], source);
            }
        }
        Ok(out)
    }

    /// The alias a statement of a template body declares of an element of
    /// the template's own type pack ([`pack_element_alias`]).
    fn template_pack_element_alias(&self, kind: &StmtKind) -> Option<(String, Type)> {
        let binders = self.template_binders.borrow();
        let binders = binders.last()?;
        pack_element_alias(kind, &|base| binders.contains(&format!("*{base}")))
    }

    pub(super) fn stmt(
        &self,
        stmt: &Stmt,
        env: &mut ConstEnv<'_>,
        in_fn: bool,
        out: &mut Vec<Stmt>,
    ) -> Result<(), ComptimeError> {
        match &stmt.kind {
            StmtKind::Comptime { .. } => self.comptime_stmt(stmt, env, in_fn, out)?,
            // A module-scope `comptime if` or `comptime for` has no
            // declaration form: it passes through for `validate_module_scope`
            // to reject.
            StmtKind::ComptimeIf { .. } => {
                if !self.keep_template_comptime_if(stmt, env, in_fn, out)? {
                    out.push(stmt.clone());
                }
            }
            StmtKind::ComptimeFor { .. } => {
                if !self.keep_template_comptime_for(stmt, env, in_fn, out)? {
                    out.push(stmt.clone());
                }
            }
            StmtKind::ComptimeMatch { .. } => {
                if !self.keep_template_comptime_match(stmt, env, in_fn, out)? {
                    out.push(stmt.clone());
                }
            }
            StmtKind::VarDecl { name, ty, value } => {
                let ty = ty
                    .as_ref()
                    .map(|ty| self.resolve_reflected_type(ty, env))
                    .transpose()?;
                out.push(rebuilt(
                    stmt,
                    StmtKind::VarDecl {
                        name: name.clone(),
                        ty,
                        value: value.clone(),
                    },
                ));
            }
            StmtKind::If { branches, orelse } => {
                let branches = branches
                    .iter()
                    .map(|(c, b)| Ok((c.clone(), self.block(b, env, in_fn)?)))
                    .collect::<Result<Vec<_>, ComptimeError>>()?;
                let orelse = self.opt_block(orelse, env, in_fn)?;
                out.push(rebuilt(stmt, StmtKind::If { branches, orelse }));
            }
            StmtKind::While { cond, body, orelse } => {
                let body = self.block(body, env, in_fn)?;
                let orelse = self.opt_block(orelse, env, in_fn)?;
                out.push(rebuilt(
                    stmt,
                    StmtKind::While {
                        cond: cond.clone(),
                        body,
                        orelse,
                    },
                ));
            }
            // A `match` is checked whole; its case bodies elaborate as a
            // branch's do.
            StmtKind::Match { subject, cases } => {
                let cases = self.match_cases(cases, env, in_fn)?;
                out.push(rebuilt(
                    stmt,
                    StmtKind::Match {
                        subject: subject.clone(),
                        cases,
                    },
                ));
            }
            StmtKind::For {
                target,
                iter,
                body,
                orelse,
            } => {
                let body = self.block(body, env, in_fn)?;
                let orelse = self.opt_block(orelse, env, in_fn)?;
                out.push(rebuilt(
                    stmt,
                    StmtKind::For {
                        target: target.clone(),
                        iter: iter.clone(),
                        body,
                        orelse,
                    },
                ));
            }
            StmtKind::Try { .. } => out.push(self.try_stmt(stmt, env, in_fn)?),
            // The context-manager protocol is a checker desugar (the manager
            // type selects the `__enter__`/`__exit__` shape); elaborate the
            // body and keep the statement, like `While`.
            StmtKind::With { items, body } => {
                let body = self.block(body, env, in_fn)?;
                out.push(rebuilt(
                    stmt,
                    StmtKind::With {
                        items: items.clone(),
                        body,
                    },
                ));
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
                let body = self.def_body(&[], type_params, params, body, env)?;
                let params = fold_default_bindings(params, env, &|name| self.module_constant(name));
                out.push(rebuilt(
                    stmt,
                    StmtKind::Def {
                        name: name.clone(),
                        decorators: decorators.clone(),
                        type_params: type_params.clone(),
                        params,
                        positional_only: *positional_only,
                        keyword_only: *keyword_only,
                        captures: captures.clone(),
                        raises: *raises,
                        raises_type: raises_type.clone(),
                        ret: ret.clone(),
                        where_clauses: where_clauses.clone(),
                        body,
                    },
                ));
            }
            StmtKind::Struct {
                name,
                type_params,
                methods,
                ..
            } => {
                let methods = methods
                    .iter()
                    .map(|m| {
                        let mut m = m.clone();
                        // A `comptime if` or `comptime for` over the
                        // struct's binders or the method's own stays in its
                        // template, as a generic `def`'s does: the
                        // elaborator below MIR decides it per instance and
                        // per call.
                        let body =
                            self.def_body(type_params, &m.type_params, &m.params, &m.body, env);
                        m.body = match body {
                            Ok(body) => body,
                            // A method whose body fails over the struct's
                            // binders or its own fails where a call reaches
                            // an instance of it, as upstream instantiates a
                            // method only there.
                            Err(error)
                                if names_struct_parameter(&error, type_params)
                                    || names_method_parameter(&error, &m) =>
                            {
                                vec![instantiation_failure_stub(name, &m.name, &m, &error)]
                            }
                            Err(error) => return Err(error),
                        };
                        Ok(m)
                    })
                    .collect::<Result<Vec<_>, ComptimeError>>()?;
                // The struct keeps every declaration part but its methods'
                // elaborated bodies.
                let mut kind = stmt.kind.clone();
                if let StmtKind::Struct {
                    methods: elaborated,
                    ..
                } = &mut kind
                {
                    *elaborated = methods;
                }
                out.push(rebuilt(stmt, kind));
            }
            // A default is its trait's member, a function over the trait's
            // `Self` (`_Self`): its body is elaborated in place as that
            // function's, so a `comptime if` or `comptime for` over `_Self`
            // or the member's own binders stays in its template.
            StmtKind::Trait { methods, .. } => {
                let elaborated = methods
                    .iter()
                    .enumerate()
                    .map(|(index, method)| {
                        let member = mojito_checker::checker::trait_member_declaration(
                            stmt,
                            index,
                            method.name.clone(),
                        );
                        match member.as_ref().map(|member| &member.kind) {
                            Some(StmtKind::Def {
                                type_params,
                                params,
                                body,
                                ..
                            }) => self.def_body(&[], type_params, params, body, env).map(Some),
                            _ => Ok(None),
                        }
                    })
                    .collect::<Result<Vec<_>, ComptimeError>>()?;
                let mut stmt = stmt.clone();
                if let StmtKind::Trait { methods, .. } = &mut stmt.kind {
                    for (method, body) in methods.iter_mut().zip(elaborated) {
                        if let Some(body) = body {
                            method.params = fold_default_bindings(&method.params, env, &|name| {
                                self.module_constant(name)
                            });
                            method.default_body = Some(body);
                        }
                    }
                }
                out.push(stmt);
            }
            _ => out.push(stmt.clone()),
        }
        Ok(())
    }

    /// Elaborate a `comptime` declaration: a generic alias, a type alias,
    /// and a binding the check binds are kept as written, and a body's
    /// reflection handle stays in the environment alone.
    fn comptime_stmt(
        &self,
        stmt: &Stmt,
        env: &mut ConstEnv<'_>,
        in_fn: bool,
        out: &mut Vec<Stmt>,
    ) -> Result<(), ComptimeError> {
        let span = stmt.span;
        let StmtKind::Comptime {
            name,
            type_params,
            ty,
            where_clauses,
            value,
        } = &stmt.kind
        else {
            return Ok(());
        };
        // A generic alias is the check's alone.
        if !type_params.is_empty() {
            out.push(stmt.clone());
            return Ok(());
        }
        if self.keep_template_comptime_binding(stmt, value, env, in_fn, out) {
            return Ok(());
        }
        // A closed field-name list (`reflect[P].field_names()`) is a
        // static method's result the check binds, module or local.
        let field_names = super::crossing::lists_field_names(value, env);
        if field_names && in_fn {
            if let Some(binders) = self.template_binders.borrow_mut().last_mut() {
                binders.insert(name.clone());
            }
            out.push(stmt.clone());
            return Ok(());
        }
        // A module constant whose initializer applies a callable,
        // asks a layout, or reads such a constant is the parameter
        // expression the check binds (decision D3): it has no value
        // here, and the elaborator below MIR evaluates it on first
        // demand. So is a collection display or constructor
        // ([`mojito_checker::checker::lifts_comptime_value`]) and a
        // field-name list.
        if !in_fn
            && (field_names
                || mojito_checker::checker::lifts_comptime_value(value, self)
                || self.applied_initializer(value, env))
        {
            let marker = CtValue::Marker(CtMarker::Applied);
            self.top_consts
                .borrow_mut()
                .insert(name.clone(), marker.clone());
            env.insert(name.clone(), marker);
            out.push(stmt.clone());
            return Ok(());
        }
        let evaluated = self.eval(value, env);
        // A module constant this evaluator does not fold
        // (`Int32(4)`, `max(3, 4)`) is the parameter expression the
        // check binds, as upstream resolves every module `comptime`.
        if !in_fn
            && check_binds_value(value)
            && matches!(evaluated, Err(ComptimeError::NotComptime(_)))
        {
            let marker = CtValue::Marker(CtMarker::Applied);
            self.top_consts
                .borrow_mut()
                .insert(name.clone(), marker.clone());
            env.insert(name.clone(), marker);
            out.push(stmt.clone());
            return Ok(());
        }
        let mut v = evaluated?;
        // Every collection-valued binding is the check's to bind: a
        // display or constructor is an application, a field-name
        // list a static method's result. None folds here.
        if v.is_runtime_collection() {
            return Err(ComptimeError::NotComptime(format!(
                "comptime '{name}' folds to the collection {v}, which only the check binds"
            )));
        }
        if let Some(annotation) = ty {
            v = self.typed_by_annotation(v, annotation, env);
        }
        // A closed scalar or string module constant is the check's
        // to bind as its parameter expression too.
        if !in_fn && is_closed_scalar(&v) && check_binds_value(value) {
            out.push(stmt.clone());
            return Ok(());
        }
        if !in_fn {
            self.top_consts.borrow_mut().insert(name.clone(), v.clone());
        }
        // A closed scalar or string binding of a body is the check's
        // to bind as its parameter expression.
        if in_fn && is_closed_scalar(&v) && check_binds_value(value) {
            env.shadow(name);
            out.push(stmt.clone());
            return Ok(());
        }
        // Fold the definition to its literal value, so the checker and
        // runtime see a constant (and a CTFE-computed `Int`, which the
        // checker's own folder can't evaluate, becomes usable as a value
        // parameter and materializes cleanly).
        env.insert(name.clone(), v);
        // A raw list read off a struct's parameter
        // (`ParameterList.of[4, 5].values`) has no literal form: the
        // binding stays for the check.
        if self.reads_raw_list(value, env) {
            out.push(stmt.clone());
            return Ok(());
        }
        // A type alias, module or body, is the check's to bind, as
        // upstream resolves the alias by name lookup to its parameter
        // value; so is a body's reflection handle, and a body's
        // `TypeList` value, whose proposition a kept condition reads
        // (`tl.contains[Int]()`).
        let kept = |value: &CtValue| match value {
            CtValue::Type(_) => true,
            CtValue::Reflected(_) => in_fn,
            value => in_fn && value.typelist_elements().is_some(),
        };
        if env.get(name).is_some_and(kept) {
            out.push(stmt.clone());
            return Ok(());
        }
        // Type and reflection handles have no runtime representation.
        // Keep those only in the elaboration environment; subsequent
        // comptime expressions consume them before checking/lowering.
        if let Some(value) = env.get(name).and_then(|value| value.materialize(span)) {
            out.push(rebuilt(
                stmt,
                StmtKind::Comptime {
                    name: name.clone(),
                    type_params: type_params.clone(),
                    ty: ty.clone(),
                    where_clauses: where_clauses.clone(),
                    value,
                },
            ));
        }
        Ok(())
    }

    /// A `try` statement with each of its parts elaborated.
    fn try_stmt(
        &self,
        stmt: &Stmt,
        env: &mut ConstEnv<'_>,
        in_fn: bool,
    ) -> Result<Stmt, ComptimeError> {
        let StmtKind::Try {
            body,
            except,
            orelse,
            finalbody,
        } = &stmt.kind
        else {
            return Ok(stmt.clone());
        };
        let body = self.block(body, env, in_fn)?;
        let except = match except {
            Some((n, b)) => Some((n.clone(), self.block(b, env, in_fn)?)),
            None => None,
        };
        let orelse = self.opt_block(orelse, env, in_fn)?;
        let finalbody = self.opt_block(finalbody, env, in_fn)?;
        Ok(rebuilt(
            stmt,
            StmtKind::Try {
                body,
                except,
                orelse,
                finalbody,
            },
        ))
    }

    /// The cases of a runtime `match`, their bodies elaborated.
    fn match_cases(
        &self,
        cases: &[mojito_ast::ast::MatchCase],
        env: &mut ConstEnv<'_>,
        in_fn: bool,
    ) -> Result<Vec<mojito_ast::ast::MatchCase>, ComptimeError> {
        cases
            .iter()
            .map(|case| {
                Ok(mojito_ast::ast::MatchCase {
                    pattern: case.pattern.clone(),
                    guard: case.guard.clone(),
                    body: self.block(&case.body, env, in_fn)?,
                    span: case.span,
                })
            })
            .collect()
    }

    #[allow(clippy::ref_option, reason = "TODO: take Option<&T>")]
    pub(super) fn opt_block(
        &self,
        block: &Option<Vec<Stmt>>,
        env: &mut ConstEnv<'_>,
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
        scope: &ConstEnv<'_>,
    ) -> Result<CtValue, ComptimeError> {
        match decl {
            ParamDecl::Type { name, .. } => match arg {
                ParamArg::Type(ty) => self.type_from_anno(ty, scope).map(CtValue::Type),
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
                // `Self.n` names the enclosing struct's own value parameter;
                // a walk that bound it to a value materializes that value.
                ParamArg::Type(Type::SelfParam(param)) => match scope
                    .get(param)
                    .cloned()
                    .map(Ok)
                    .or_else(|| self.self_sibling(param, scope))
                    .transpose()?
                {
                    Some(CtValue::Type(_)) | None => Err(ComptimeError::NotComptime(format!(
                        "value parameter '{name}' expects a compile-time {ty}, got Self.{param}"
                    ))),
                    Some(value) => materialize_ct_value(value.clone(), ty).ok_or_else(|| {
                        ComptimeError::NotComptime(format!(
                            "value parameter '{name}' expects {ty}, got {value}"
                        ))
                    }),
                },
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
        scope: &ConstEnv<'_>,
    ) -> Result<CtValue, ComptimeError> {
        self.type_from_name(name, args, scope).map(CtValue::Type)
    }

    /// Resolve a `[...]` argument that is expected to be a **type** (a type
    /// annotation, a bare type name, or a parameterized type) to a `Ty`.
    pub(super) fn param_arg_type(
        &self,
        arg: &ParamArg,
        scope: &ConstEnv<'_>,
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
                CtValue::Type(ty) => Ok(ty),
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
        scope: &ConstEnv<'_>,
    ) -> Result<Ty, ComptimeError> {
        match ty {
            Type::Int => Ok(Ty::int()),
            Type::UInt => Ok(Ty::uint()),
            Type::Bool => Ok(Ty::bool()),
            Type::StringLiteral | Type::ClosedStringLiteral => Ok(Ty::string_literal()),
            Type::Float64 => Ok(Ty::float64()),
            Type::None => Ok(Ty::none()),
            Type::Named(name, args) => self.type_from_name(name, args, scope),
            Type::Value(_) => Err(ComptimeError::NotComptime(
                "expected a type, not a value".to_string(),
            )),
            Type::TypeOf(operand) => any_trait_source_type(operand).ok_or_else(|| {
                ComptimeError::NotComptime(
                    "'type_of' of a value or a struct type is not supported yet".to_string(),
                )
            }),
            Type::SelfParam(name) => match scope
                .get(name)
                .cloned()
                .map(Ok)
                .or_else(|| self.self_sibling(name, scope))
                .transpose()?
            {
                Some(CtValue::Type(ty)) => Ok(ty),
                Some(_) => Err(ComptimeError::NotComptime(format!(
                    "Self.{name} is not type-valued"
                ))),
                None => Err(ComptimeError::NotComptime(format!(
                    "unknown compile-time type Self.{name}"
                ))),
            },
            Type::Assoc { base, name, args } => {
                if let Type::Named(binding, base_args) = &**base
                    && base_args.is_empty()
                    && name == "T"
                    && let Some(CtValue::Reflected(ty)) = scope.get(binding)
                {
                    return Ok(ty.clone());
                }
                let generator = match &**base {
                    Type::Named(base, base_args)
                        if base_args.is_empty() && self.unbound_generator(base, scope) =>
                    {
                        Some(base.as_str())
                    }
                    _ => None,
                };
                let base = match generator {
                    Some(generator) => Self::generator(generator),
                    None => self.type_from_anno(base, scope)?,
                };
                let member_args = args
                    .iter()
                    .map(|arg| match arg {
                        ParamArg::Type(ty) => self.type_from_anno(ty, scope).map(CtValue::Type),
                        ParamArg::Value(value) => self.eval(value, scope),
                        ParamArg::Named { .. } => Err(ComptimeError::NotComptime(
                            "a keyword argument of a member application".to_string(),
                        )),
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let value = match generator {
                    Some(generator) => self.generator_member(generator, name, &member_args)?,
                    None => self.associated_value_at(&base, name, &member_args)?,
                };
                match value {
                    CtValue::Type(ty) => Ok(ty),
                    _ => Err(ComptimeError::NotComptime(format!(
                        "{base}.{name} is not type-valued"
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
                let (CtValue::Tuple(values) | CtValue::List(values)) =
                    self.associated_value(&base_ty, name)?
                else {
                    return Err(ComptimeError::NotComptime(format!(
                        "{base_ty}.{name} is not a type sequence"
                    )));
                };
                let index = self.eval(index, scope)?.as_int("dependent type index")?;
                match usize::try_from(index)
                    .ok()
                    .and_then(|position| values.get(position))
                {
                    Some(CtValue::Type(ty)) => Ok(ty.clone()),
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
                let referent = self.type_from_anno(referent, scope)?;
                if origin_name == "UntrackedOrigin" {
                    return Ok(TyKind::Ref(mojito_types::origin::RefTy {
                        referent,
                        origin: mojito_types::origin::Origin::Untracked { mutable: false },
                        mutability: mojito_types::origin::Mutability::Immutable,
                    })
                    .into());
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
                Ok(TyKind::Ref(reference).into())
            }
            Type::SelfType | Type::Func { .. } => Err(ComptimeError::NotComptime(
                "unsupported compile-time type argument".to_string(),
            )),
        }
    }

    pub(super) fn type_from_name(
        &self,
        name: &str,
        args: &[ParamArg],
        scope: &ConstEnv<'_>,
    ) -> Result<Ty, ComptimeError> {
        if args.is_empty() {
            if let Some(CtValue::Type(ty)) = scope.get(name) {
                return Ok(ty.clone());
            }
            if let Some(ty) = scalar_type_name(name) {
                return Ok(ty);
            }
        }
        // `SIMD[DType.d, w]` is a compile-time type (a vector alias's value),
        // its arguments spelled or read from constants (`SIMD[D, 4]`).
        if name == "SIMD"
            && let Some((dtype, width)) =
                simd_source_dims(args).or_else(|| self.evaluated_simd_dims(args, scope))
        {
            return Ok(TyKind::Simd {
                dtype: mojito_types::types::SimdDtype::Known(dtype),
                width: mojito_types::types::SimdWidth::Known(width),
            }
            .into());
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
                Some(CtValue::Type(ty)) => Ok(ty.clone()),
                Some(_) => Err(ComptimeError::NotComptime(format!(
                    "'{name}[{index}]' is not type-valued"
                ))),
                None => Err(ComptimeError::BadArithmetic(format!(
                    "type-list index {index} out of range"
                ))),
            };
        }
        let Some(info) = self.ct_struct(name) else {
            return Err(ComptimeError::NotComptime(format!(
                "'{name}' is not a compile-time type"
            )));
        };
        let (decls, source_params) = (info.decls(), info.source_params());
        // The compiler-known public Tuple's single `*Ts` declaration absorbs
        // every argument: a nested application (`Tuple[Int, Tuple[Int,
        // Bool]]`, `Optional[Tuple[Int, Bool]]`, a `TypeNames` element) stays
        // the nominal `Tuple[...]` type — the spelling the checker canonicalizes
        // onto the minted specialization — rather than tripping the fixed-arity
        // check below.
        if name == mojito_types::types::TUPLE_TYPE_NAME
            && let [ParamDecl::Type { variadic: true, .. }] = decls
        {
            let elements = args
                .iter()
                .map(|argument| self.param_arg_type(argument, scope).map(TyArg::Ty))
                .collect::<Result<Vec<_>, _>>()?;
            return Ok(TyKind::Struct(name.to_string(), elements).into());
        }
        // Omitted trailing arguments fill from declared type-parameter
        // defaults (`Set[Int]` is `Set[Int, default_hasher]`), matching the
        // def-template default fill in `resolve_spec_args_for`. A default
        // classification could not resolve (a module alias) evaluates from
        // its source expression instead.
        let source_defaults: Vec<Option<&Expr>> = source_params
            .iter()
            .filter(|tp| !retained_specialization_param(tp, source_params))
            .map(|tp| tp.default.as_ref())
            .collect();
        let defaults_fill =
            args.len() < decls.len()
                && decls.iter().zip(&source_defaults).skip(args.len()).all(
                    |(decl, source_default)| {
                        matches!(
                            decl,
                            ParamDecl::Type {
                                default: Some(_),
                                ..
                            }
                        ) || (matches!(decl, ParamDecl::Type { .. }) && source_default.is_some())
                    },
                );
        if args.len() > decls.len() || (args.len() < decls.len() && !defaults_fill) {
            return Err(ComptimeError::Arity(format!(
                "type '{name}' expects {} compile-time argument(s), got {}",
                decls.len(),
                args.len()
            )));
        }
        let tyargs = decls
            .iter()
            .enumerate()
            .map(|(index, decl)| {
                let Some(arg) = args.get(index) else {
                    if let ParamDecl::Type {
                        default: Some(default),
                        ..
                    } = decl
                    {
                        return Ok(TyArg::Ty(default.clone()));
                    }
                    let default = source_defaults
                        .get(index)
                        .copied()
                        .flatten()
                        .expect("defaults_fill established trailing type defaults");
                    return match self.eval(default, scope)? {
                        CtValue::Type(ty) => Ok(TyArg::Ty(ty)),
                        other => Err(ComptimeError::NotComptime(format!(
                            "default for type parameter '{}' of '{name}' is not a type: {other}",
                            decl.name()
                        ))),
                    };
                };
                let value = self.resolve_ct_arg(decl, arg, scope)?;
                match (decl, value) {
                    (ParamDecl::Type { .. }, CtValue::Type(ty)) => Ok(TyArg::Ty(ty)),
                    (ParamDecl::Type { name, .. }, _) => Err(ComptimeError::NotComptime(format!(
                        "type parameter '{name}' needs a type argument"
                    ))),
                    (ParamDecl::Value { .. }, value) => Ok(TyArg::Val(value)),
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(TyKind::Struct(name.to_string(), tyargs).into())
    }

    /// The struct name a member read's base `object` spells when it is a
    /// bare parameterized struct, read as its generator (`S` in `S.n`), as
    /// the pin reads it, no default filling a parameter. `None` when the
    /// name is bound in `scope` or names no parameterized struct.
    pub(super) fn generator_base<'x>(
        &self,
        object: &'x Expr,
        scope: &ConstEnv<'_>,
    ) -> Option<&'x str> {
        let ExprKind::Identifier(name) = &object.kind else {
            return None;
        };
        self.unbound_generator(name, scope).then_some(name.as_str())
    }

    /// Whether `name` is a bare parameterized struct read as its generator
    /// ([`Self::generator_base`]).
    pub(super) fn unbound_generator(&self, name: &str, scope: &ConstEnv<'_>) -> bool {
        matches!(
            scope.get(name),
            None | Some(CtValue::Marker(CtMarker::TypeName))
        ) && self
            .ct_struct(name)
            .is_some_and(|info| !info.decls().is_empty())
    }

    /// The compile-time member `member` of struct `name`'s generator,
    /// applied to the closed `member_args` (none for an unparameterized
    /// member). As at the pin, a member that does not depend on the unbound
    /// parameters reads, and one that does, spelled or through a sibling it
    /// reads, is rejected.
    pub(super) fn generator_member(
        &self,
        name: &str,
        member: &str,
        member_args: &[CtValue],
    ) -> Result<CtValue, ComptimeError> {
        match self.ct_struct(name) {
            Some(StructView::Own(info)) => {
                if let Some(parameter) = member_parameter(info, member, &mut HashSet::new()) {
                    return Err(ComptimeError::Type(TypeError::UnboundParameterMember {
                        member: member.to_string(),
                        parameter: format!("{name}.{}", parameter.name().trim_start_matches('*')),
                    }));
                }
                self.associated_value_at(&Self::generator(name), member, member_args)
            }
            Some(StructView::Package(_)) => self
                .conformance
                .package_generator_member(name, member, member_args)
                .unwrap_or_else(|| {
                    Err(TypeError::NoSuchAssociatedType {
                        object_type: name.to_string(),
                        member: member.to_string(),
                    })
                })
                .map_err(|error| match error {
                    TypeError::UnboundParameterMember { .. } => ComptimeError::Type(error),
                    error => ComptimeError::NotComptime(error.to_string()),
                }),
            None => Err(ComptimeError::NotComptime(format!(
                "unknown compile-time struct '{name}'"
            ))),
        }
    }

    /// The type package struct `name`'s parameterized type member `member`
    /// names through its generator, its bracket list `arguments` (keyword
    /// names beside them) evaluated here and bound by the check's rule:
    /// `TypeList.of[Trait=AnyType, Int, Bool]`.
    pub(super) fn generator_type_member(
        &self,
        name: &str,
        member: &str,
        arguments: &[(Option<String>, ParamArg)],
        scope: &ConstEnv<'_>,
    ) -> Result<Ty, ComptimeError> {
        let values = arguments
            .iter()
            .map(|(keyword, argument)| Ok((keyword.clone(), self.bracket_value(argument, scope)?)))
            .collect::<Result<Vec<_>, ComptimeError>>()?;
        self.conformance
            .package_generator_type_member(name, member, &values)
            .unwrap_or_else(|| {
                Err(TypeError::NoSuchAssociatedType {
                    object_type: name.to_string(),
                    member: member.to_string(),
                })
            })
            .map_err(|error| match error {
                TypeError::UnboundParameterMember { .. } => ComptimeError::Type(error),
                error => ComptimeError::NotComptime(error.to_string()),
            })
    }

    /// The generator of struct `name`, no argument bound, as the base the
    /// member reads of [`Self::generator_member`] take.
    pub(super) fn generator(name: &str) -> Ty {
        TyKind::Struct(name.to_string(), Vec::new()).into()
    }

    pub(super) fn associated_value(
        &self,
        base: &Ty,
        member: &str,
    ) -> Result<CtValue, ComptimeError> {
        self.associated_value_at(base, member, &[])
    }

    /// The compile-time member `member` of `ty`: through the generator
    /// when `ty` is a parameterized struct no argument binds
    /// ([`Self::generator_member`]), else the instance's.
    pub(super) fn member_of(&self, ty: &Ty, member: &str) -> Result<CtValue, ComptimeError> {
        match ty.kind() {
            TyKind::Struct(name, args)
                if args.is_empty()
                    && self
                        .ct_struct(name)
                        .is_some_and(|info| !info.decls().is_empty()) =>
            {
                self.generator_member(name, member, &[])
            }
            _ => self.associated_value(ty, member),
        }
    }

    /// `Self.<name>` where `scope` binds no parameter of the name: the
    /// sibling member of the struct `scope` binds `Self` to, while a
    /// member's body is evaluated ([`Self::associated_value_at`]). `None`
    /// when `scope` binds no `Self`.
    pub(super) fn self_sibling(
        &self,
        name: &str,
        scope: &ConstEnv<'_>,
    ) -> Option<Result<CtValue, ComptimeError>> {
        match scope.get("Self")? {
            CtValue::Type(ty) => Some(self.member_of(ty, name)),
            _ => None,
        }
    }

    /// Whether the struct `base` names declares a parameterized `comptime`
    /// value member `member`, the program's own or the imported package's.
    pub(super) fn declares_parameterized_member(&self, base: &Ty, member: &str) -> bool {
        let TyKind::Struct(name, _) = base.kind() else {
            return false;
        };
        match self.ct_struct(name) {
            Some(StructView::Own(info)) => info
                .associated
                .iter()
                .any(|associated| associated.name == member && !associated.params.is_empty()),
            Some(StructView::Package(_)) => self
                .conformance
                .package_member_application(base, member, &[])
                .is_some(),
            None => false,
        }
    }

    /// The compile-time member `member` of `base` applied to the closed
    /// `member_args` (none for an unparameterized member).
    pub(super) fn associated_value_at(
        &self,
        base: &Ty,
        member: &str,
        member_args: &[CtValue],
    ) -> Result<CtValue, ComptimeError> {
        // A `SIMD` type's `dtype` parameter; `Int` and `Float64` are width-1
        // vectors.
        if member == "dtype" {
            match base.kind() {
                TyKind::Int => return Ok(CtValue::Dtype(mojito_ast::ast::Dtype::Int)),
                TyKind::Float64 => return Ok(CtValue::Dtype(mojito_ast::ast::Dtype::Float64)),
                TyKind::Simd { dtype, .. } => {
                    // A symbolic lane has no constant yet: the elaborator
                    // only folds a bound instance.
                    return dtype.known().map(CtValue::Dtype).ok_or_else(|| {
                        ComptimeError::NotComptime(format!("'{base}' has no concrete dtype"))
                    });
                }
                TyKind::UInt => {
                    return Err(ComptimeError::NotComptime(
                        "DType.uint is not supported yet".to_string(),
                    ));
                }
                _ => {}
            }
        }
        let TyKind::Struct(name, args) = base.kind() else {
            return Err(ComptimeError::NotComptime(format!(
                "type '{base}' has no compile-time member '{member}'"
            )));
        };
        // A struct's own parameter is a member of each instance
        // (`S[3].n`, `ParameterList.of[4, 5].values`).
        if member_args.is_empty()
            && let Some(view) = self.ct_struct(name)
            && let Some(argument) = view
                .decls()
                .iter()
                .position(|decl| decl.name().trim_start_matches('*') == member)
                .and_then(|slot| args.get(slot))
        {
            return match argument {
                TyArg::Ty(ty) => Ok(CtValue::Type(ty.clone())),
                TyArg::Val(value) => Ok(value.clone()),
                TyArg::Origin(_) => Err(ComptimeError::NotComptime(format!(
                    "the origin parameter '{member}' of '{base}' is not a compile-time value"
                ))),
            };
        }
        let info = match self.ct_struct(name) {
            Some(StructView::Own(info)) => info,
            Some(StructView::Package(_)) if !member_args.is_empty() => {
                return self
                    .conformance
                    .package_member_application(base, member, member_args)
                    .unwrap_or_else(|| {
                        Err(mojito_common::error::TypeError::NotComptime(format!(
                            "type '{base}' has no parameterized compile-time member '{member}'"
                        )))
                    })
                    .map_err(|error| ComptimeError::NotComptime(error.to_string()));
            }
            Some(StructView::Package(_)) => {
                return self
                    .conformance
                    .package_associated_value(base, member)
                    .map_err(|error| ComptimeError::NotComptime(error.to_string()));
            }
            None => {
                return Err(ComptimeError::NotComptime(format!(
                    "unknown compile-time struct '{name}'"
                )));
            }
        };
        let assoc = info
            .associated
            .iter()
            .find(|a| a.name == member)
            .ok_or_else(|| {
                ComptimeError::NotComptime(format!(
                    "type '{base}' has no compile-time member '{member}'"
                ))
            })?;
        if member_args.len() != assoc.params.len() {
            return Err(ComptimeError::Arity(format!(
                "'{base}.{member}' takes exactly {} compile-time argument(s)",
                assoc.params.len()
            )));
        }
        let mut env = ConstEnv::default();
        for (decl, arg) in info.decls.iter().zip(args) {
            match (decl, arg) {
                (ParamDecl::Type { name, .. }, TyArg::Ty(ty)) => {
                    env.insert(name.clone(), CtValue::Type(ty.clone()));
                }
                (ParamDecl::Value { name, .. }, TyArg::Val(value)) => {
                    env.insert(name.trim_start_matches('*').to_string(), value.clone());
                }
                _ => {}
            }
        }
        for (param, value) in assoc.params.iter().zip(member_args) {
            env.insert(param.name.clone(), value.clone());
        }
        // A sibling member reads through `Self`, the instance or generator
        // the member is read on.
        env.insert("Self".to_string(), CtValue::Type(base.clone()));
        if !assoc.params.is_empty() {
            self.check_where_clauses(&format!("{base}.{member}"), &assoc.where_clauses, &env)?;
        }
        self.eval(&assoc.value, &env)
    }

    /// The `(dtype, width)` of `SIMD[d, w]` arguments that evaluate to a
    /// `DType` and a power-of-two width in `scope`.
    fn evaluated_simd_dims(
        &self,
        args: &[ParamArg],
        scope: &ConstEnv<'_>,
    ) -> Option<(mojito_ast::ast::Dtype, i64)> {
        let [ParamArg::Value(dtype), ParamArg::Value(width)] = args else {
            return None;
        };
        let CtValue::Dtype(dtype) = self.eval(dtype, scope).ok()? else {
            return None;
        };
        let width = self.eval(width, scope).ok()?.as_int("SIMD width").ok()?;
        (width >= 1 && (width & (width - 1)) == 0).then_some((dtype, width))
    }

    /// Whether `expression` applies a callable this elaborator does not
    /// fold itself ([`mojito_checker::checker::applies_callable`]).
    pub(super) fn applies_callable(&self, expression: &Expr) -> bool {
        mojito_checker::checker::applies_callable(expression, self)
    }

    /// Whether a module constant's initializer is an application the check
    /// binds ([`CtMarker::Applied`]): it applies a callable, asks a layout
    /// (`size_of[T]()`), reads an applied constant, or calls a method of a
    /// value binding (`s.byte_length()`), which the pin's interpreter runs.
    fn applied_initializer(&self, expression: &Expr, env: &ConstEnv<'_>) -> bool {
        self.applies_callable(expression)
            || asks_layout(expression)
            || self.reads_applied_member(expression)
            || expression_names_any(expression, &|name| {
                matches!(env.get(name), Some(CtValue::Marker(CtMarker::Applied)))
            })
            || calls_value_method(expression, env)
    }

    /// Whether `expression` reads a `comptime` member of one of the
    /// program's structs (`S[3].V`, `S.k`) whose body applies a callable or
    /// asks a layout, itself or through a sibling it reads: the parameter
    /// expression the check binds for the member, which the elaborator
    /// below MIR runs per instance.
    fn reads_applied_member(&self, expression: &Expr) -> bool {
        struct Finder<'e, 'x> {
            elab: &'e Elab<'x>,
            found: bool,
        }
        impl mojito_ast::visit::Visitor for Finder<'_, '_> {
            fn visit_expr(&mut self, expr: &Expr) {
                let ExprKind::Member { object, field } = &expr.kind else {
                    return;
                };
                let owner = match &object.kind {
                    ExprKind::Identifier(name) | ExprKind::TypeApply { name, .. } => name,
                    ExprKind::Index { object, .. } => match &object.kind {
                        ExprKind::Identifier(name) => name,
                        _ => return,
                    },
                    _ => return,
                };
                if let Some(StructView::Own(info)) = self.elab.ct_struct(owner) {
                    self.found |= self.elab.member_applies(info, field, &mut HashSet::new());
                }
            }
        }
        let mut finder = Finder {
            elab: self,
            found: false,
        };
        mojito_ast::visit::walk_expr(&mut finder, expression);
        finder.found
    }

    /// Whether the body of `info`'s member `member` applies a callable or
    /// asks a layout, itself or through a sibling it reads. `seen` holds the
    /// members already walked.
    fn member_applies(
        &self,
        info: &CtStruct<'_>,
        member: &str,
        seen: &mut HashSet<String>,
    ) -> bool {
        if !seen.insert(member.to_string()) {
            return false;
        }
        let Some(body) = info
            .associated
            .iter()
            .find(|a| a.name == member)
            .map(|a| &a.value)
        else {
            return false;
        };
        self.applies_callable(body)
            || asks_layout(body)
            || info.associated.iter().any(|sibling| {
                let qualified = self_qualified(&sibling.name);
                expression_names_any(body, &|spelled| spelled == qualified)
                    && self.member_applies(info, &sibling.name, seen)
            })
    }

    /// Whether `ty` is an application of a struct over a raw parameter list
    /// (`ParameterList`'s `values: _MLIR.KGENParamListType[type]`), which no
    /// source spelling binds.
    pub(super) fn binds_raw_list(&self, ty: &Ty) -> bool {
        let TyKind::Struct(name, _) = ty.kind() else {
            return false;
        };
        self.ct_struct(name).is_some_and(|info| {
            info.decls().iter().any(|decl| {
                matches!(decl, ParamDecl::Value { ty, .. }
                    if matches!(ty.kind(), TyKind::ParamList(_)))
            })
        })
    }

    /// Whether `expression` reads a raw-list parameter of a struct type
    /// (`ParameterList.of[4, 5].values`), a list whose closed form is a
    /// tuple no source spelling binds back to the list.
    fn reads_raw_list(&self, expression: &Expr, env: &ConstEnv<'_>) -> bool {
        let ExprKind::Member { object, field } = &expression.kind else {
            return false;
        };
        let Ok(CtValue::Type(ty)) = self.eval(object, env) else {
            return false;
        };
        let TyKind::Struct(name, _) = ty.kind() else {
            return false;
        };
        self.ct_struct(name).is_some_and(|info| {
            info.decls().iter().any(|decl| {
                matches!(decl, ParamDecl::Value { name, ty, .. }
                    if name.trim_start_matches('*') == field
                        && matches!(ty.kind(), TyKind::ParamList(_)))
            })
        })
    }

    /// One bracket argument as a compile-time value: a type, a value, or a
    /// trait (`AnyType`, `Copyable & Movable`) a trait-valued binder takes.
    fn bracket_value(
        &self,
        argument: &ParamArg,
        scope: &ConstEnv<'_>,
    ) -> Result<CtValue, ComptimeError> {
        let expr = match argument {
            ParamArg::Named { value, .. } => return self.bracket_value(value, scope),
            ParamArg::Type(_) => return self.param_arg_type(argument, scope).map(CtValue::Type),
            ParamArg::Value(expr) => expr,
        };
        let named = matches!(
            expr.kind,
            ExprKind::Identifier(_) | ExprKind::TypeApply { .. }
        );
        if named && let Ok(ty) = self.param_arg_type(argument, scope) {
            return Ok(CtValue::Type(ty));
        }
        self.eval(expr, scope).or_else(|error| {
            mojito_ast::ast::trait_conjunction_names(expr)
                .map(|names| CtValue::Trait(names.into_iter().map(str::to_string).collect()))
                .ok_or(error)
        })
    }
}

impl mojito_checker::checker::CalleeOracle for Elab<'_> {
    fn callee(&self, name: &str) -> mojito_checker::checker::Callee {
        if self.declares_function(name) {
            mojito_checker::checker::Callee::Function
        } else if self.declares_struct(name) {
            mojito_checker::checker::Callee::Struct
        } else {
            mojito_checker::checker::Callee::Other
        }
    }

    fn names_struct(&self, name: &str) -> bool {
        self.declares_struct(name)
    }
}

impl Elab<'_> {
    /// A scalar under a SIMD-valued annotation other than `Int` takes the
    /// annotation's dtype, splatted across its width, so every materialized
    /// use carries the declared type. Any other value is returned as is.
    pub(super) fn typed_by_annotation(
        &self,
        value: CtValue,
        annotation: &Type,
        env: &ConstEnv<'_>,
    ) -> CtValue {
        if !matches!(
            value,
            CtValue::Int(_)
                | CtValue::UInt(_)
                | CtValue::Float(_)
                | CtValue::IntLiteral(_)
                | CtValue::FloatLiteral(_)
                | CtValue::Bool(_)
        ) {
            return value;
        }
        self.param_arg_type(&ParamArg::Type(annotation.clone()), env)
            .ok()
            .filter(|target| {
                !matches!(
                    target.kind(),
                    TyKind::Int | TyKind::IntLiteral | TyKind::UInt
                )
            })
            .and_then(|target| mojito_types::types::simd_shape(&target))
            .and_then(|(dtype, width)| {
                let width = usize::try_from(width).unwrap_or(1);
                mojito_types::ct::simd_construction_lanes(
                    dtype,
                    width,
                    std::slice::from_ref(&value),
                )
                .ok()
                .map(|lanes| CtValue::Simd { dtype, lanes })
            })
            .unwrap_or(value)
    }
}

/// `params` with each default reading an enclosing body's compile-time
/// binding (`x: Int = k * 2` under a local `comptime k = 3`) spelled at its
/// value: a default is evaluated at compile time. A module constant
/// (`module`) stays a name, which the check reads as its parameter value.
fn fold_default_bindings(
    params: &[mojito_ast::ast::FnParam],
    env: &ConstEnv<'_>,
    module: &dyn Fn(&str) -> Option<CtValue>,
) -> Vec<mojito_ast::ast::FnParam> {
    let subs: rewrite::Subs = &|name| {
        env.get(name)
            .filter(|value| module(name).as_ref() != Some(*value))
            .cloned()
    };
    params
        .iter()
        .map(|param| {
            let mut param = param.clone();
            if let Some(default) = &mut param.default {
                rewrite::rewrite_expr(default, subs);
            }
            param
        })
        .collect()
}

impl Elab<'_> {
    /// Elaborate a `def`'s or a method's body. Every body is a template over
    /// its binders, a plain body over none: its binders are in scope for
    /// [`Self::keep_template_comptime_binding`], and every `comptime if` and
    /// `comptime for` stays for the check and the elaborator below MIR. A
    /// method's struct parameters (`struct_params`) are binders of its body
    /// as `Self.`-qualified names only, which is how a method reads them; a
    /// bare name equal to one is a module constant or a local.
    fn def_body(
        &self,
        struct_params: &[TypeParam],
        type_params: &[TypeParam],
        params: &[FnParam],
        body: &[Stmt],
        env: &ConstEnv<'_>,
    ) -> Result<Vec<Stmt>, ComptimeError> {
        // A body's local `comptime` bindings are its own: a later
        // declaration never reads them.
        let env = &mut env.clone();
        // A pack's collector stands for the pack in a bound (`len(args)`),
        // so it is a binder of the body too.
        // A type pack is a binder under both spellings: `Ts` (or
        // `Self.Ts`) as the body names it, and `*Ts` (`*Self.Ts`) marking
        // it a type pack ([`Self::template_pack_element_alias`]). A value
        // pack is a `ParameterList` value, never marked.
        let mut binders: HashSet<String> = type_params
            .iter()
            .map(|parameter| parameter.name.trim_start_matches('*').to_string())
            .collect();
        binders.extend(def_pack_names(type_params, params));
        binders.extend(struct_pack_collectors(struct_params, params));
        binders.extend(
            struct_params
                .iter()
                .map(|parameter| self_qualified(&parameter.name)),
        );
        binders.extend(type_packs(type_params).map(|parameter| parameter.name.clone()));
        binders.extend(
            type_packs(struct_params)
                .map(|parameter| format!("*{}", self_qualified(&parameter.name))),
        );
        self.template_binders.borrow_mut().push(binders);
        let body = self.block(body, env, true);
        self.template_binders.borrow_mut().pop();
        body
    }

    /// Keep a local `comptime` binding whose value names a binder of the
    /// generic `def` being elaborated as a template (`comptime m = N + 1`),
    /// or applies a callable or constructs a collection in any body
    /// (`comptime x = f(1)`, `comptime l = [1, 2]`,
    /// [`mojito_checker::checker::lifts_comptime_value`]), or is a closed
    /// value this evaluator does not fold ([`Self::unfolded_value_binding`]):
    /// the check binds it with the binders symbolic, the elaborator below MIR
    /// evaluates an application on demand, and the binding is a binder of
    /// the rest of its block, so a `comptime if` over it stays too.
    /// Whether the statement was kept.
    fn keep_template_comptime_binding(
        &self,
        stmt: &Stmt,
        value: &Expr,
        env: &ConstEnv<'_>,
        in_fn: bool,
        out: &mut Vec<Stmt>,
    ) -> bool {
        let StmtKind::Comptime { name, .. } = &stmt.kind else {
            return false;
        };
        if !in_fn {
            return false;
        }
        let Some(names_binder) = self
            .template_binders
            .borrow()
            .last()
            .map(|binders| expression_names_any(value, &|name| binders.contains(name)))
        else {
            return false;
        };
        if !(names_binder
            || self.applied_initializer(value, env)
            || mojito_checker::checker::lifts_comptime_value(value, self)
            || self.unfolded_value_binding(value, env))
        {
            return false;
        }
        if let Some(binders) = self.template_binders.borrow_mut().last_mut() {
            binders.insert(name.clone());
        }
        out.push(stmt.clone());
        true
    }

    /// Whether a local `comptime` binding over no binder is an expression
    /// this evaluator does not fold (`t[1] if c else t[0]`, `UInt8(250)`),
    /// which the check binds as its parameter expression, as upstream
    /// builds every local `comptime` (`emitExprPValue`).
    fn unfolded_value_binding(&self, value: &Expr, env: &ConstEnv<'_>) -> bool {
        check_binds_value(value)
            && matches!(self.eval(value, env), Err(ComptimeError::NotComptime(_)))
    }

    /// Keep every `comptime if` of a function body, as upstream keeps its
    /// `kgen.param.if`: every arm is elaborated in the block's environment
    /// (a binding an arm declares stays the arm's), and the statement is
    /// rebuilt for the check, which types every arm with the binders
    /// symbolic and records the condition for the MIR branch the elaborator
    /// below MIR decides. Returns `false`, emitting nothing, at module scope.
    fn keep_template_comptime_if(
        &self,
        stmt: &Stmt,
        env: &ConstEnv<'_>,
        in_fn: bool,
        out: &mut Vec<Stmt>,
    ) -> Result<bool, ComptimeError> {
        let StmtKind::ComptimeIf { branches, orelse } = &stmt.kind else {
            return Ok(false);
        };
        if !in_fn {
            return Ok(false);
        }
        // An arm whose closed arithmetic fails to evaluate keeps its source:
        // the pin reports the failure only for an instance that takes it.
        let arm = |body: &[Stmt]| match self.block(body, &mut env.clone(), true) {
            Err(ComptimeError::BadArithmetic(_) | ComptimeError::NotCollection(_)) => {
                Ok(body.to_vec())
            }
            elaborated => elaborated,
        };
        let branches = branches
            .iter()
            .map(|(cond, body)| Ok((cond.clone(), arm(body)?)))
            .collect::<Result<Vec<_>, ComptimeError>>()?;
        let orelse = orelse.as_deref().map(arm).transpose()?;
        out.push(rebuilt(stmt, StmtKind::ComptimeIf { branches, orelse }));
        Ok(true)
    }

    /// Keep every `comptime match` of a function body, as upstream emits it
    /// as one `hlcf.comptime.if`: each case body is elaborated with the
    /// case's pattern bindings binders, as a `comptime for` body is with its
    /// index, and the subject, patterns, and guards stay as written for the
    /// check, which builds each case's condition and the elaborator below
    /// MIR decides. Returns `false`, emitting nothing, at module scope.
    fn keep_template_comptime_match(
        &self,
        stmt: &Stmt,
        env: &ConstEnv<'_>,
        in_fn: bool,
        out: &mut Vec<Stmt>,
    ) -> Result<bool, ComptimeError> {
        let StmtKind::ComptimeMatch { subject, cases } = &stmt.kind else {
            return Ok(false);
        };
        if !in_fn {
            return Ok(false);
        }
        let Some(binders) = self.template_binders.borrow().last().cloned() else {
            return Ok(false);
        };
        let cases = cases
            .iter()
            .map(|case| {
                let mut binders = binders.clone();
                let mut inner = env.clone();
                for (name, _) in case.pattern.bindings() {
                    binders.insert(name.to_string());
                    inner.shadow(name);
                }
                self.template_binders.borrow_mut().push(binders);
                let body = self.block(&case.body, &mut inner, true);
                self.template_binders.borrow_mut().pop();
                Ok(mojito_ast::ast::MatchCase {
                    pattern: case.pattern.clone(),
                    guard: case.guard.clone(),
                    body: body?,
                    span: case.span,
                })
            })
            .collect::<Result<Vec<_>, ComptimeError>>()?;
        out.push(rebuilt(
            stmt,
            StmtKind::ComptimeMatch {
                subject: subject.clone(),
                cases,
            },
        ));
        Ok(true)
    }

    /// Keep every `comptime for` of a function body, as upstream keeps its
    /// `kgen.param.for`: the body is elaborated with the loop variable a
    /// binder too, so a `comptime if` or a loop over it stays, and the
    /// statement is rebuilt for the check, which types the body once with
    /// the index symbolic and records the sequence for the MIR loop header
    /// the elaborator below MIR unrolls — a compile-time `break` and
    /// `continue` are that loop's. Returns `false`, emitting nothing, at
    /// module scope.
    fn keep_template_comptime_for(
        &self,
        stmt: &Stmt,
        env: &ConstEnv<'_>,
        in_fn: bool,
        out: &mut Vec<Stmt>,
    ) -> Result<bool, ComptimeError> {
        let StmtKind::ComptimeFor { var, iter, body } = &stmt.kind else {
            return Ok(false);
        };
        if !in_fn {
            return Ok(false);
        }
        let Some(mut binders) = self.template_binders.borrow().last().cloned() else {
            return Ok(false);
        };
        binders.insert(var.clone());
        self.template_binders.borrow_mut().push(binders);
        // The index shadows a module constant of its name in the body.
        let mut inner = env.clone();
        inner.shadow(var);
        let body = self.block(body, &mut inner, true);
        self.template_binders.borrow_mut().pop();
        out.push(rebuilt(
            stmt,
            StmtKind::ComptimeFor {
                var: var.clone(),
                iter: iter.clone(),
                body: body?,
            },
        ));
        Ok(true)
    }
}

/// Whether an initializer asks a layout query (`size_of[T]()`).
fn asks_layout(expression: &Expr) -> bool {
    struct Finder(bool);

    impl mojito_ast::visit::Visitor for Finder {
        fn visit_expr(&mut self, expr: &Expr) {
            self.0 |= matches!(&expr.kind, ExprKind::Call { name, .. }
                if name == mojito_types::param_expr::SIZE_OF_FUNCTION);
        }
    }

    let mut finder = Finder(false);
    mojito_ast::visit::walk_expr(&mut finder, expression);
    finder.0
}

/// Whether `expression` calls a method of a binding of `env` that holds a
/// value (`S.byte_length()`, `w.twice()`), not a type or a handle.
fn calls_value_method(expression: &Expr, env: &ConstEnv<'_>) -> bool {
    struct Finder<'e, 'a> {
        env: &'e ConstEnv<'a>,
        found: bool,
    }

    impl mojito_ast::visit::Visitor for Finder<'_, '_> {
        fn visit_expr(&mut self, expr: &Expr) {
            if let ExprKind::MethodCall { object, .. } = &expr.kind
                && let ExprKind::Identifier(name) = &object.kind
                && let Some(value) = self.env.get(name)
            {
                self.found |= !matches!(
                    value,
                    CtValue::Type(_)
                        | CtValue::Reflected(_)
                        | CtValue::Expr(_)
                        | CtValue::Deferred(_)
                        | CtValue::Marker(_)
                        | CtValue::Dtype(_)
                ) && value.materialize(object.span).is_some();
            }
        }
    }

    let mut finder = Finder { env, found: false };
    mojito_ast::visit::walk_expr(&mut finder, expression);
    finder.found
}

/// Whether `expression` spells one of `names` as an identifier, a call, a
/// type, or a `Self.`-qualified parameter (an entry [`self_qualified`]).
pub(super) fn expression_names_any(expression: &Expr, names: &dyn Fn(&str) -> bool) -> bool {
    struct Finder<'a> {
        names: &'a dyn Fn(&str) -> bool,
        found: bool,
    }

    impl mojito_ast::visit::Visitor for Finder<'_> {
        fn visit_expr(&mut self, expr: &Expr) {
            match &expr.kind {
                ExprKind::Identifier(name) | ExprKind::Call { name, .. } => {
                    self.found |= (self.names)(name);
                }
                ExprKind::Member { object, field } if matches!(&object.kind, ExprKind::Identifier(base) if base == "Self") =>
                {
                    self.found |= (self.names)(&self_qualified(field));
                }
                _ => {}
            }
        }

        fn visit_type(&mut self, ty: &mojito_ast::ast::Type) {
            match ty {
                mojito_ast::ast::Type::Named(name, _) => {
                    self.found |= (self.names)(name.trim_start_matches('*'));
                }
                mojito_ast::ast::Type::SelfParam(name) => {
                    self.found |= (self.names)(&self_qualified(name));
                }
                _ => {}
            }
        }
    }

    let mut finder = Finder {
        names,
        found: false,
    };
    mojito_ast::visit::walk_expr(&mut finder, expression);
    finder.found
}

/// Spell each pack-element alias a kept `comptime if` condition names
/// (`comptime if T == Int` under `comptime T = Self.Ts[i]`) as the element
/// it denotes, which the check types with the pack symbolic.
fn spell_aliases_in_conditions(statements: &mut [Stmt], aliases: &HashMap<String, Expr>) {
    struct Spell<'a>(&'a HashMap<String, Expr>);

    impl mojito_ast::visit::MutVisitor for Spell<'_> {
        fn visit_expr_mut(&mut self, expr: &mut Expr) {
            if let ExprKind::Identifier(name) = &expr.kind
                && let Some(element) = self.0.get(name)
            {
                expr.kind = element.kind.clone();
            }
        }
    }

    struct Conditions<'a>(&'a HashMap<String, Expr>);

    impl mojito_ast::visit::MutVisitor for Conditions<'_> {
        fn visit_stmt_mut(&mut self, statement: &mut Stmt) {
            if let StmtKind::ComptimeIf { branches, .. } = &mut statement.kind {
                for (condition, _) in branches {
                    mojito_ast::visit::walk_condition_mut(&mut Spell(self.0), condition);
                }
            }
        }
    }

    mojito_ast::visit::walk_block_mut(&mut Conditions(aliases), statements);
}

/// The binder-set entry of a struct parameter, which a method body names
/// only as `Self.<name>`.
/// The type packs among `parameters`: a variadic parameter the check does
/// not classify as a value.
fn type_packs(parameters: &[TypeParam]) -> impl Iterator<Item = &TypeParam> {
    parameters.iter().filter(|parameter| {
        parameter.name.starts_with('*')
            && !matches!(
                super::params::classify_ct_param(parameter, parameters, ""),
                Some(mojito_types::types::ParamDecl::Value { .. })
            )
    })
}

fn self_qualified(name: &str) -> String {
    format!("Self.{}", name.trim_start_matches('*'))
}

/// The parameter of `info` the body of its member `member` reads: spelled
/// (`Self.T`), or through a sibling member it reads (`Self.a` over
/// `comptime a = Self.T`). `seen` holds the members already walked.
fn member_parameter<'i>(
    info: &'i CtStruct<'_>,
    member: &str,
    seen: &mut HashSet<String>,
) -> Option<&'i ParamDecl> {
    if !seen.insert(member.to_string()) {
        return None;
    }
    let body = &info.associated.iter().find(|a| a.name == member)?.value;
    let reads = |name: &str| {
        let qualified = self_qualified(name);
        expression_names_any(body, &|spelled| spelled == qualified)
    };
    info.decls
        .iter()
        .find(|decl| reads(decl.name()))
        .or_else(|| {
            info.associated
                .iter()
                .filter(|sibling| sibling.params.is_empty() && reads(&sibling.name))
                .find_map(|sibling| member_parameter(info, &sibling.name, seen))
        })
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

/// Whether an elaboration error names one of the method's own compile-time
/// parameters or its type-pack collector (`a.__len__()` for `*a: *Ts`), which
/// only a clone binds.
fn names_method_parameter(error: &ComptimeError, method: &mojito_ast::ast::Method) -> bool {
    if names_struct_parameter(error, &method.type_params) {
        return true;
    }
    let text = error.to_string();
    method.params.iter().any(|parameter| {
        parameter.kind == ParamKind::Variadic
            && matches!(&parameter.ty, Type::Named(name, arguments)
                if name.starts_with('*') && arguments.is_empty())
            && text.contains(&format!("'{}'", parameter.name))
    })
}

/// The collectors a method types by its struct's type pack
/// (`*b: *Self.Ts`, or `*b: *Ts` as the template spells it), which stand for the pack in a bound as a `def`'s own
/// collector does ([`def_pack_names`]).
fn struct_pack_collectors<'a>(
    struct_params: &'a [TypeParam],
    params: &'a [FnParam],
) -> impl Iterator<Item = String> + 'a {
    params
        .iter()
        .filter(move |parameter| {
            parameter.kind == ParamKind::Variadic
                && matches!(&parameter.ty,
                    Type::SelfParam(spread) | Type::Named(spread, _)
                        if spread.starts_with('*')
                            && struct_params.iter().any(|binder| binder.name == *spread))
        })
        .map(|parameter| parameter.name.clone())
}

impl Elab<'_> {
    /// Freeze each computed argument of a struct-typed value parameter of a
    /// generator, a struct or a uniquely named `def`
    /// (`Tagged[Extent.square(4)]`), into the fieldwise construction of its
    /// compile-time value (`Tagged[Extent(4, 4)]`), the form the checker
    /// reads as a frozen struct value, wherever `program` applies one. An
    /// argument over a binder, which has no value yet, is left as written.
    pub(super) fn freeze_struct_value_arguments(
        &self,
        program: &mut [Stmt],
        consts: &ConstEnv<'_>,
    ) {
        struct Freezer<'e, 'a> {
            elab: &'e Elab<'a>,
            consts: &'e ConstEnv<'a>,
        }
        impl MutVisitor for Freezer<'_, '_> {
            fn visit_expr_mut(&mut self, expr: &mut Expr) {
                if let ExprKind::TypeApply { name, args }
                | ExprKind::Call {
                    name,
                    param_args: args,
                    ..
                } = &mut expr.kind
                {
                    self.elab.freeze_arguments(name, args, self.consts);
                }
            }
            fn visit_type_mut(&mut self, ty: &mut Type) {
                if let Type::Named(name, arguments) = ty {
                    self.elab.freeze_arguments(name, arguments, self.consts);
                }
            }
        }
        walk_block_mut(&mut Freezer { elab: self, consts }, program);
    }

    fn freeze_arguments(&self, name: &str, arguments: &mut [ParamArg], consts: &ConstEnv<'_>) {
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
        let Some(explicit) = self.explicit_parameters(name) else {
            return;
        };
        if !explicit.iter().any(|(_, struct_valued)| *struct_valued) {
            return;
        }
        for (index, argument) in arguments.iter_mut().enumerate() {
            let (struct_valued, expression) = match argument {
                ParamArg::Value(expression) => (
                    explicit
                        .get(index)
                        .is_some_and(|(_, struct_valued)| *struct_valued),
                    expression,
                ),
                ParamArg::Named { name, value } => match &mut **value {
                    ParamArg::Value(expression) => (
                        explicit
                            .iter()
                            .any(|(parameter, struct_valued)| *struct_valued && parameter == name),
                        expression,
                    ),
                    _ => continue,
                },
                ParamArg::Type(_) => continue,
            };
            if !struct_valued {
                continue;
            }
            if let Ok(value @ CtValue::Struct { .. }) = self.eval(expression, consts)
                && let Some(frozen) = value.materialize(expression.span)
            {
                *expression = frozen;
            }
        }
    }

    /// The explicit compile-time parameters of struct or `def` `name`, each
    /// beside whether it is struct-valued: a struct's or an own `def`'s from
    /// its parameter list, a package `def`'s as its check resolved it. `None`
    /// for an overload family, or a name declaring neither.
    fn explicit_parameters(&self, name: &str) -> Option<Vec<(String, bool)>> {
        let from_source = |type_params: &[TypeParam]| {
            type_params
                .iter()
                .filter(|parameter| {
                    !parameter.infer_only
                        && !matches!(parameter.bounds.as_slice(),
                            [only] if only == "Origin" || only == "OriginSet")
                })
                .map(|parameter| {
                    let struct_valued = matches!(parameter.bounds.as_slice(),
                        [only] if self.declares_struct(only));
                    (parameter.name.clone(), struct_valued)
                })
                .collect()
        };
        if let Some(info) = self.ct_struct(name) {
            return Some(from_source(info.source_params()));
        }
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
            (Some(type_params), None) => return Some(from_source(type_params)),
            (Some(_), Some(_)) => return None,
            (None, _) => {}
        }
        let decls = self.conformance.package_function_params(name)?;
        Some(
            decls
                .iter()
                .filter(|decl| {
                    !matches!(
                        decl,
                        ParamDecl::Type {
                            infer_only: true,
                            ..
                        } | ParamDecl::Value {
                            infer_only: true,
                            ..
                        }
                    )
                })
                .map(|decl| {
                    let struct_valued = matches!(decl, ParamDecl::Value { ty, .. }
                        if matches!(*ty.kind(), TyKind::Struct(..)));
                    (decl.name().to_string(), struct_valued)
                })
                .collect(),
        )
    }
}

/// The body of a method of `owner` whose elaboration failed with `error`:
/// the elaborator below MIR reports the failure if a reachable call
/// instantiates the method.
fn instantiation_failure_stub(
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

/// A `comptime` binding whose leading-dot initializer (`comptime shape: Shape
/// = .rect(2, 5)`) names a member of its annotated type, as it does in a typed
/// `var`, with that type spelled; `None` for any other statement.
fn contextual_comptime(stmt: &Stmt) -> Option<Stmt> {
    let StmtKind::Comptime {
        ty: Some(Type::Named(base, args)),
        value,
        ..
    } = &stmt.kind
    else {
        return None;
    };
    let resolved = with_contextual_base(value, base, args)?;
    let mut rewritten = stmt.clone();
    if let StmtKind::Comptime { value, .. } = &mut rewritten.kind {
        *value = resolved;
    }
    Some(rewritten)
}

/// `expression` with its leading-dot root (`$contextual`) named as the type
/// `base[args]`; `None` when it has no such root.
fn with_contextual_base(expression: &Expr, base: &str, args: &[ParamArg]) -> Option<Expr> {
    fn replace(expression: &mut Expr, root: &ExprKind) -> bool {
        match &mut expression.kind {
            ExprKind::Identifier(name) if name == mojito_ast::ast::CONTEXTUAL_SENTINEL => {
                expression.kind = root.clone();
                true
            }
            ExprKind::Member { object, .. }
            | ExprKind::MethodCall { object, .. }
            | ExprKind::Index { object, .. } => replace(object, root),
            ExprKind::Invoke { callee, .. } => replace(callee, root),
            _ => false,
        }
    }
    let root = if args.is_empty() {
        ExprKind::Identifier(base.to_string())
    } else {
        ExprKind::TypeApply {
            name: base.to_string(),
            args: args.to_vec(),
        }
    };
    let mut resolved = expression.clone();
    replace(&mut resolved, &root).then_some(resolved)
}

/// Whether the check binds a local `comptime` value spelled `value`: not
/// one it types only as a condition (`conforms_to(T, Trait)`), nor an
/// invoked lambda, whose nested declaration a lifted function does not
/// carry. Either keeps this evaluator's folding.
fn check_binds_value(value: &Expr) -> bool {
    let mut lambdas = Vec::new();
    mojito_ast::ast::lambdas_in_expr(value, &mut lambdas);
    lambdas.is_empty() && !expression_names_any(value, &|name| name == "conforms_to")
}
