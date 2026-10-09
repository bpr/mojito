//! The `Elab` elaboration driver methods.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_ast::ast::Method;
use mojito_ast::visit::{MutVisitor, walk_block_mut};

impl Elab<'_> {
    /// Elaborate a block, resolving `comptime` constructs. `in_fn` is true inside a
    /// function/method body (so a comptime constant there is *not* module-level).
    pub(super) fn block(
        &self,
        stmts: &[Stmt],
        env: &mut HashMap<String, CtValue>,
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

    fn block_statements(
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
        // The pack elements the template's aliases denote, as written.
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
                source_aliases.insert(name, element);
                continue;
            }
            let first_new = out.len();
            self.stmt(stmt, env, in_fn, &mut out)?;
            // Runtime uses of the bindings elaborated so far cross explicitly
            // (`materialize[X]()`, `comptime(e)`) or are rejected.
            self.fold_runtime_crossings(&mut out[first_new..], env)?;
            if !type_aliases.is_empty() {
                let subs: Subs = &|name| type_aliases.get(name).cloned();
                for statement in &mut out[first_new..] {
                    *statement = rewrite_stmt_cloned(statement, subs, true);
                }
            }
            if !source_aliases.is_empty() {
                substitute_type_bindings_in_block(&mut out[first_new..], &source_aliases);
            }
            if !element_aliases.is_empty() {
                spell_aliases_in_conditions(&mut out[first_new..], &element_aliases);
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

    /// The alias a statement of a template body declares of an element of
    /// the template's own type pack ([`pack_element_alias`]).
    fn template_pack_element_alias(&self, kind: &StmtKind) -> Option<(String, Type)> {
        let binders = self.template_binders.borrow();
        let binders = binders.last()?;
        pack_element_alias(kind, &|base| {
            binders.contains(&format!("*{base}"))
                || (base.starts_with("Self.") && binders.contains(base))
        })
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
                if self.keep_template_comptime_binding(stmt, value, env, in_fn, out) {
                    return Ok(());
                }
                // A module constant whose initializer applies a callable,
                // asks a layout, or reads such a constant is the parameter
                // expression the check binds (decision D3): it has no value
                // here, and the elaborator below MIR evaluates it on first
                // demand.
                if !in_fn && self.applied_initializer(value, env) {
                    let marker = CtValue::Marker(CtMarker::Applied);
                    self.top_consts
                        .borrow_mut()
                        .insert(name.clone(), marker.clone());
                    env.insert(name.clone(), marker);
                    // A display is spelled where it is read, an applied
                    // display it reads spelled inside it.
                    let value = self.spell_applied_displays(value);
                    if matches!(
                        value.kind,
                        ExprKind::ListLit(_) | ExprKind::TupleLit(_) | ExprKind::BraceLit(_)
                    ) {
                        self.applied_displays
                            .borrow_mut()
                            .insert(name.clone(), value.clone());
                    }
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
                    return Ok(());
                }
                let mut v = self.eval(value, env)?;
                // A collection display takes the binding's annotation as its
                // spelled type; an empty `{}` has no other typing.
                if matches!(v, CtValue::Dict { .. } | CtValue::Set { .. }) {
                    match ty {
                        Some(annotation) => {
                            let target =
                                self.param_arg_type(&ParamArg::Type(annotation.clone()), env)?;
                            v = v.clone().materialize_as(&target).ok_or_else(|| {
                                ComptimeError::NotComptime(format!(
                                    "comptime '{name}' is a {v} display, not a '{target}'"
                                ))
                            })?;
                        }
                        None if matches!(&value.kind, ExprKind::BraceLit(entries) if entries.is_empty()) =>
                        {
                            return Err(ComptimeError::NotComptime(
                                "an empty '{}' display needs a Dict[K, V] type annotation"
                                    .to_string(),
                            ));
                        }
                        None => {}
                    }
                }
                if let Some(annotation) = ty {
                    v = self.typed_by_annotation(v, annotation, env);
                }
                if !in_fn {
                    self.top_consts.borrow_mut().insert(name.clone(), v.clone());
                }
                // Fold the definition to its literal value, so the checker and
                // runtime see a constant (and a CTFE-computed `Int`, which the
                // checker's own folder can't evaluate, becomes usable as a value
                // parameter and materializes cleanly).
                env.insert(name.clone(), v);
                // A `TypeList` value has no literal form: the binding stays
                // for the check, which reads a kept condition's proposition
                // over it (`tl.contains[Int]()`) from its initializer, as does
                // a reflection handle such an initializer may name
                // (`comptime types = r.field_types()`).
                if in_fn
                    && (env[name].typelist_elements().is_some()
                        || matches!(env[name], CtValue::Reflected(_)))
                {
                    out.push(stmt.clone());
                    return Ok(());
                }
                // Type and reflection handles have no runtime representation,
                // and a collection is not implicitly copyable (it crosses only
                // through `materialize[...]()`). Keep those only in the
                // elaboration environment; subsequent comptime expressions
                // consume them before checking/lowering.
                if !env[name].is_runtime_collection()
                    && let Some(value) = env[name].materialize(span)
                {
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
            }
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
            StmtKind::For {
                var,
                binding,
                iter,
                body,
                orelse,
            } => {
                let body = self.block(body, env, in_fn)?;
                let orelse = self.opt_block(orelse, env, in_fn)?;
                out.push(rebuilt(
                    stmt,
                    StmtKind::For {
                        var: var.clone(),
                        binding: *binding,
                        iter: iter.clone(),
                        body,
                        orelse,
                    },
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
                out.push(rebuilt(
                    stmt,
                    StmtKind::Try {
                        body,
                        except,
                        orelse,
                        finalbody,
                    },
                ));
            }
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
                let params = fold_default_bindings(params, env);
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
                out.push(rebuilt(
                    stmt,
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
                    },
                ));
            }
            _ => out.push(stmt.clone()),
        }
        Ok(())
    }

    #[allow(clippy::ref_option, reason = "TODO: take Option<&T>")]
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
                // `Self.n` names the enclosing struct's own value parameter;
                // a walk that bound it to a value materializes that value.
                ParamArg::Type(Type::SelfParam(param)) => match scope.get(param) {
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
        scope: &HashMap<String, CtValue>,
    ) -> Result<CtValue, ComptimeError> {
        self.type_from_name(name, args, scope)
            .map(|ty| CtValue::Type(Box::new(ty)))
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
            Type::StringLiteral | Type::ClosedStringLiteral => Ok(Ty::StringLiteral),
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
            Type::SelfType | Type::Func { .. } => Err(ComptimeError::NotComptime(
                "unsupported compile-time type argument".to_string(),
            )),
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
        // `SIMD[DType.d, w]` is a compile-time type (a vector alias's value).
        if name == "SIMD"
            && let Some((dtype, width)) = simd_source_dims(args)
        {
            return Ok(Ty::Simd {
                dtype: mojito_types::types::SimdDtype::Known(dtype),
                width: mojito_types::types::SimdWidth::Known(width),
            });
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
        // The compiler-known public Tuple's single `*Ts` declaration absorbs
        // every argument: a nested application (`Tuple[Int, Tuple[Int,
        // Bool]]`, `Optional[Tuple[Int, Bool]]`, a `TypeNames` element) stays
        // the nominal `Tuple[...]` type — the spelling the checker canonicalizes
        // onto the minted specialization — rather than tripping the fixed-arity
        // check below.
        if name == mojito_types::types::TUPLE_TYPE_NAME
            && let [ParamDecl::Type { variadic: true, .. }] = info.decls.as_slice()
        {
            let elements = args
                .iter()
                .map(|argument| self.param_arg_type(argument, scope).map(TyArg::Ty))
                .collect::<Result<Vec<_>, _>>()?;
            return Ok(Ty::Struct(name.to_string(), elements.into()));
        }
        // Omitted trailing arguments fill from declared type-parameter
        // defaults (`Set[Int]` is `Set[Int, default_hasher]`), matching the
        // def-template default fill in `resolve_spec_args_for`. A default
        // classification could not resolve (a module alias) evaluates from
        // its source expression instead.
        let source_defaults: Vec<Option<&Expr>> = info
            .source_params
            .iter()
            .filter(|tp| !retained_specialization_param(tp, info.source_params))
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
        Ok(Ty::Struct(name.to_string(), tyargs.into()))
    }

    pub(super) fn associated_value(
        &self,
        base: &Ty,
        member: &str,
    ) -> Result<CtValue, ComptimeError> {
        // A `SIMD` type's `dtype` parameter; `Int` and `Float64` are width-1
        // vectors.
        if member == "dtype" {
            match base {
                Ty::Int => return Ok(CtValue::Dtype(mojito_ast::ast::Dtype::Int)),
                Ty::Float64 => return Ok(CtValue::Dtype(mojito_ast::ast::Dtype::Float64)),
                Ty::Simd { dtype, .. } => {
                    // A symbolic lane has no constant yet: the elaborator
                    // only folds a bound instance.
                    return dtype.known().map(CtValue::Dtype).ok_or_else(|| {
                        ComptimeError::NotComptime(format!("'{base}' has no concrete dtype"))
                    });
                }
                Ty::UInt => {
                    return Err(ComptimeError::NotComptime(
                        "DType.uint is not supported yet".to_string(),
                    ));
                }
                _ => {}
            }
        }
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

    /// Whether `expression` applies a callable this elaborator does not
    /// fold itself ([`mojito_checker::checker::applies_callable`]).
    pub(super) fn applies_callable(&self, expression: &Expr) -> bool {
        mojito_checker::checker::applies_callable(expression, self)
    }

    /// Whether a module constant's initializer is an application the check
    /// binds ([`CtMarker::Applied`]): it applies a callable, asks a layout
    /// (`size_of[T]()`), or reads an applied constant.
    fn applied_initializer(&self, expression: &Expr, env: &HashMap<String, CtValue>) -> bool {
        let applied: HashSet<String> = env
            .iter()
            .filter(|(_, value)| matches!(value, CtValue::Marker(CtMarker::Applied)))
            .map(|(name, _)| name.clone())
            .collect();
        self.applies_callable(expression)
            || asks_layout(expression)
            || (!applied.is_empty() && expression_names_any(expression, &applied))
    }

    /// `expression` with each value binding of `env` that a method call
    /// reads (`S.byte_length()`, `M.get("a")`), or a dictionary or set
    /// binding a subscript reads (`M["a"]`), spelled by its literal form,
    /// and a pending module constant a method call or a subscript reads
    /// spelled by its initializer, so that the request the elaborator below MIR serves
    /// constructs the value itself, as the pin's parameter expression does.
    /// `None` when it reads none.
    pub(super) fn spelled_value_reads(
        &self,
        expression: &Expr,
        env: &HashMap<String, CtValue>,
    ) -> Option<Expr> {
        struct Spell<'e> {
            env: &'e HashMap<String, CtValue>,
            displays: &'e HashMap<String, Expr>,
            spelled: bool,
        }

        impl mojito_ast::visit::MutVisitor for Spell<'_> {
            fn visit_expr_mut(&mut self, expr: &mut Expr) {
                let subscripted = matches!(expr.kind, ExprKind::Index { .. });
                let (ExprKind::MethodCall { object, .. } | ExprKind::Index { object, .. }) =
                    &mut expr.kind
                else {
                    return;
                };
                let ExprKind::Identifier(name) = &object.kind else {
                    return;
                };
                if let Some(value) = self.env.get(name)
                    && spelled_receiver(value, subscripted)
                    && let Some(mut display) = value.materialize(object.span)
                {
                    display.source.clone_from(&object.source);
                    **object = display;
                    self.spelled = true;
                } else if let Some(display) = self.displays.get(name) {
                    **object = super::requests::located(display.clone(), object);
                    self.spelled = true;
                }
            }
        }

        let mut spelled = expression.clone();
        let displays = self.applied_displays.borrow();
        let mut spell = Spell {
            env,
            displays: &displays,
            spelled: false,
        };
        mojito_ast::visit::walk_expr_mut(&mut spell, &mut spelled);
        spell.spelled.then_some(spelled)
    }
}

impl mojito_checker::checker::CalleeOracle for Elab<'_> {
    fn callee(&self, name: &str) -> mojito_checker::checker::Callee {
        if self.fns.contains(name) {
            mojito_checker::checker::Callee::Function
        } else if self.structs.contains_key(name) {
            mojito_checker::checker::Callee::Struct
        } else {
            mojito_checker::checker::Callee::Other
        }
    }

    fn names_struct(&self, name: &str) -> bool {
        self.structs.contains_key(name)
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
        env: &HashMap<String, CtValue>,
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
            .filter(|target| !matches!(target, Ty::Int | Ty::IntLiteral | Ty::UInt))
            .and_then(|target| mojito_types::types::simd_shape(&target))
            .and_then(|(dtype, width)| {
                mojito_types::ct::CtLane::from_value(&value, dtype).map(|lane| CtValue::Simd {
                    dtype,
                    lanes: vec![lane; usize::try_from(width).unwrap_or(1)],
                })
            })
            .unwrap_or(value)
    }
}

/// `params` with each default reading an enclosing compile-time binding
/// (`x: Int = k * 2` under `comptime k = 3`) spelled at its value: a default
/// is evaluated at compile time.
fn fold_default_bindings(
    params: &[mojito_ast::ast::FnParam],
    env: &HashMap<String, CtValue>,
) -> Vec<mojito_ast::ast::FnParam> {
    let subs: rewrite::Subs = &|name| {
        env.get(name)
            .filter(|value| !value.is_runtime_collection())
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
        env: &HashMap<String, CtValue>,
    ) -> Result<Vec<Stmt>, ComptimeError> {
        // A body's local `comptime` bindings are its own: a later
        // declaration never reads them.
        let env = &mut env.clone();
        // A pack's collector stands for the pack in a bound (`len(args)`),
        // so it is a binder of the body too.
        // A type pack is a binder under both spellings: `Ts` as the body
        // names it, and `*Ts` marking it a pack ([`Self::block`]).
        let mut binders: HashSet<String> = type_params
            .iter()
            .flat_map(|parameter| {
                [
                    parameter.name.trim_start_matches('*').to_string(),
                    parameter.name.clone(),
                ]
            })
            .collect();
        binders.extend(def_pack_names(type_params, params));
        binders.extend(struct_pack_collectors(struct_params, params));
        binders.extend(
            struct_params
                .iter()
                .map(|parameter| self_qualified(&parameter.name)),
        );
        self.template_binders.borrow_mut().push(binders);
        let body = self.block(body, env, true);
        self.template_binders.borrow_mut().pop();
        body
    }

    /// Keep a local `comptime` binding whose value names a binder of the
    /// generic `def` being elaborated as a template (`comptime m = N + 1`),
    /// or applies a callable in any body (`comptime x = f(1)`), once the
    /// value bindings a method reads are spelled
    /// ([`Self::spelled_value_reads`]): the check
    /// binds it with the binders symbolic, the elaborator below MIR
    /// evaluates an application on demand, and the binding is a binder of
    /// the rest of its block, so a `comptime if` over it stays too.
    /// Whether the statement was kept.
    fn keep_template_comptime_binding(
        &self,
        stmt: &Stmt,
        value: &Expr,
        env: &HashMap<String, CtValue>,
        in_fn: bool,
        out: &mut Vec<Stmt>,
    ) -> bool {
        let StmtKind::Comptime {
            name,
            type_params,
            ty,
            where_clauses,
            ..
        } = &stmt.kind
        else {
            return false;
        };
        if !in_fn {
            return false;
        }
        let mut binders = self.template_binders.borrow_mut();
        let Some(binders) = binders.last_mut() else {
            return false;
        };
        // A value binding read by a method is spelled by its literal form
        // in a request.
        if let Some(spelled) = self
            .spelled_value_reads(value, env)
            .filter(|spelled| self.applies_callable(spelled))
        {
            binders.insert(name.clone());
            let kind = StmtKind::Comptime {
                name: name.clone(),
                type_params: type_params.clone(),
                ty: ty.clone(),
                where_clauses: where_clauses.clone(),
                value: spelled,
            };
            out.push(rebuilt(stmt, kind));
            return true;
        }
        if !(expression_names_any(value, binders) || self.applied_initializer(value, env)) {
            return false;
        }
        binders.insert(name.clone());
        out.push(stmt.clone());
        true
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
        env: &HashMap<String, CtValue>,
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
            Err(ComptimeError::BadArithmetic(_) | ComptimeError::BadRange(_)) => Ok(body.to_vec()),
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
        env: &HashMap<String, CtValue>,
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
        inner.remove(var);
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

/// Whether `expression` spells one of `names` as an identifier, a call, a
/// type, or a `Self.`-qualified parameter (an entry [`self_qualified`]).
pub(super) fn expression_names_any(expression: &Expr, names: &HashSet<String>) -> bool {
    struct Finder<'a> {
        names: &'a HashSet<String>,
        found: bool,
    }

    impl mojito_ast::visit::Visitor for Finder<'_> {
        fn visit_expr(&mut self, expr: &Expr) {
            match &expr.kind {
                ExprKind::Identifier(name) | ExprKind::Call { name, .. } => {
                    self.found |= self.names.contains(name);
                }
                ExprKind::Member { object, field } if matches!(&object.kind, ExprKind::Identifier(base) if base == "Self") =>
                {
                    self.found |= self.names.contains(&self_qualified(field));
                }
                _ => {}
            }
        }

        fn visit_type(&mut self, ty: &mojito_ast::ast::Type) {
            match ty {
                mojito_ast::ast::Type::Named(name, _) => {
                    self.found |= self.names.contains(name.trim_start_matches('*'));
                }
                mojito_ast::ast::Type::SelfParam(name) => {
                    self.found |= self.names.contains(&self_qualified(name));
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
                    mojito_ast::visit::walk_expr_mut(&mut Spell(self.0), condition);
                }
            }
        }
    }

    mojito_ast::visit::walk_block_mut(&mut Conditions(aliases), statements);
}

/// The binder-set entry of a struct parameter, which a method body names
/// only as `Self.<name>`.
fn self_qualified(name: &str) -> String {
    format!("Self.{}", name.trim_start_matches('*'))
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

/// Whether a method call (or, `subscripted`, a subscript) of a binding of
/// `value` is spelled with the value's literal form in a request: any value
/// with one, but a subscript only of a dictionary or a set, whose
/// `__getitem__` raises; a list's is folded here.
const fn spelled_receiver(value: &CtValue, subscripted: bool) -> bool {
    if subscripted {
        return matches!(value, CtValue::Dict { .. } | CtValue::Set { .. });
    }
    !matches!(
        value,
        CtValue::Type(_)
            | CtValue::Reflected(_)
            | CtValue::Expr(_)
            | CtValue::Deferred(_)
            | CtValue::Marker(_)
            | CtValue::Dtype(_)
    )
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
        consts: &HashMap<String, CtValue>,
    ) {
        struct Freezer<'e, 'a> {
            elab: &'e Elab<'a>,
            consts: &'e HashMap<String, CtValue>,
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

    fn freeze_arguments(
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
