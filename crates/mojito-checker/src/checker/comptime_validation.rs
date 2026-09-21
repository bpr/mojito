//! Source validation of compile-time control flow (`validate_comptime_templates`):
//! a `comptime if` condition is typed as a compile-time `Bool`, every arm and
//! `comptime for` body is checked in its own scope with the declaration's
//! parameters symbolic, and function-local `comptime` bindings the elaborator
//! would otherwise consume are bound here. Extracted from `checker.rs`; see
//! `docs/symbol-map.md`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_ast::ast::ParamArg;
use mojito_types::types::{ConstraintOperand, GenericConstraint};

impl Checker {
    /// Check the method bodies of a struct that hold compile-time control
    /// flow, each with `self` bound at the struct's own parameters.
    pub(super) fn validate_comptime_method_bodies(
        &mut self,
        declaration: &StructDeclaration<'_>,
        self_ty: &Ty,
    ) -> Result<(), TypeError> {
        let mut overload_indices = HashMap::<String, usize>::new();
        for (method_index, m) in declaration.methods.iter().enumerate() {
            let method_name = lifecycle_method_name(m).to_string();
            let overload_index = *overload_indices.entry(method_name.clone()).or_default();
            *overload_indices
                .get_mut(&method_name)
                .expect("inserted above") += 1;
            if !validates_body(
                declaration.type_params,
                &m.type_params,
                &m.body,
                body_keys_rebind(&m.body, &self.rebind_keyed_bodies),
            ) {
                continue;
            }
            let scopes = self.scopes.len();
            self.pack_element_views.borrow_mut().clear();
            let checked = self.check_method(
                self_ty,
                m,
                declaration.module.clone().as_ref(),
                declaration.name,
                method_index,
                overload_index,
            );
            self.pack_verdict(
                &format!("{}.{method_name}", declaration.name),
                &m.body,
                scopes,
                checked,
            )?;
        }
        Ok(())
    }

    /// Keep a pack-keyed body's validation result, unless it ended at a use
    /// of the unbound pack with no symbolic rule: that is no verdict, so the
    /// body is recorded and left to its per-instantiation check. `scopes` is
    /// the scope depth before the body, restored past the abandoned check.
    pub(super) fn pack_verdict(
        &mut self,
        name: &str,
        body: &[Stmt],
        scopes: usize,
        checked: Result<(), TypeError>,
    ) -> Result<(), TypeError> {
        match checked {
            Err(TypeError::SymbolicPackBoundary(what)) if self.source_validation => {
                while self.scopes.len() > scopes {
                    self.pop_scope();
                }
                timing::count("templates.pack_no_verdict", 1);
                self.no_verdict_bodies
                    .extend(body.first().map(Stmt::source_span));
                self.template_catalog
                    .borrow_mut()
                    .stats_mut()
                    .no_verdict
                    .push((name.to_string(), what));
                Ok(())
            }
            checked => checked,
        }
    }

    /// A `comptime if` condition must be a compile-time `Bool`: a generic
    /// constraint over the parameters in scope (`T == Int`, `n == 0`,
    /// `conforms_to(T, Copyable)`, a `TypeList` proposition, a predicate
    /// alias), or an ordinary `Bool` expression over compile-time bindings
    /// (a `comptime for` variable, a `Bool` parameter). It is typed, never
    /// evaluated: selection is the elaborator's.
    pub(super) fn check_comptime_condition(&mut self, cond: &Expr) -> Result<(), TypeError> {
        let cond = self.inline_local_comptime_values(cond);
        self.check_ct_bool(&cond)
    }

    /// The recursive form of [`Self::check_comptime_condition`]: a
    /// connective recurses so a concrete conformance fact can sit beside a
    /// symbolic constraint; a leaf is a generic constraint over the
    /// parameters in scope, a conformance of a concrete type, or a `Bool`
    /// value expression.
    fn check_ct_bool(&mut self, cond: &Expr) -> Result<(), TypeError> {
        match &cond.kind {
            ExprKind::Bool(_) => return Ok(()),
            ExprKind::Prefix(PrefixOp::Not, inner) => return self.check_ct_bool(inner),
            ExprKind::Infix(InfixOp::And | InfixOp::Or, left, right) => {
                self.check_ct_bool(left)?;
                return self.check_ct_bool(right);
            }
            // `conforms_to(MaybeUninit[Int], RegisterPassable)`: a fact about
            // a concrete type, which the constraint compiler reserves for
            // parameters.
            ExprKind::Call { name, args, .. }
                if name == "conforms_to"
                    && args.len() == 2
                    && self.comptime_type_operand(&args[0])?.is_some() =>
            {
                let ExprKind::Identifier(trait_name) = &args[1].kind else {
                    return Err(TypeError::Unsupported(
                        "conforms_to takes a trait name as its second argument".to_string(),
                    ));
                };
                return self.check_trait_name(trait_name);
            }
            // An application of an undeclared name (`TriviallyCopyable[Int]`)
            // is neither a predicate nor a type.
            ExprKind::TypeApply { name, .. }
                if mojito_types::types::trivial_predicate_name(name).is_none()
                    && self.comptime_name_resolves(name).is_err() =>
            {
                return Err(TypeError::UnknownType(name.clone()));
            }
            _ => {}
        }
        let constraint = self
            .compile_generic_constraint(cond)
            .and_then(|constraint| {
                self.constraint_operands_resolve(&constraint)
                    .map(|()| constraint)
            });
        match constraint {
            Ok(_) => Ok(()),
            Err(constraint_error) => match self.expect_bool(cond, "comptime if condition") {
                Ok(()) => Ok(()),
                // A condition over types or a `TypeList` has no value
                // reading; its constraint diagnosis is the one that names
                // the problem.
                Err(_) if self.condition_is_compile_time_shaped(cond) => Err(constraint_error),
                Err(error) => Err(error),
            },
        }
    }

    /// Check a `comptime for` body once, its variable bound to the element
    /// type of the iterable: `Int` for `range(...)`, the element of a
    /// compile-time list or a value pack. Unrolling is the elaborator's; the
    /// body may run zero times, so definite initialization is unchanged.
    pub(super) fn check_comptime_for(
        &mut self,
        var: &str,
        iter: &Expr,
        body: &[Stmt],
        ret: Option<&Ty>,
        in_loop: bool,
    ) -> Result<(), TypeError> {
        let iter = self.inline_local_comptime_values(iter);
        let element = self.comptime_iteration_element(&iter)?;
        let before = self.uninitialized.borrow().clone();
        self.push_scope();
        if let Some(bindings) = self.compile_time_bindings.last_mut() {
            bindings.insert(var.to_string());
        }
        let binds_index = element == Ty::Int;
        let shadowed = binds_index
            .then(|| comptime_index_binder(var, &iter))
            .and_then(|binder| {
                self.innermost_value_scope()
                    .and_then(|scope| scope.insert(var.to_string(), binder))
            });
        let result = self
            .declare_immutable(var, element)
            .and_then(|()| self.check_block(body, ret, in_loop));
        if binds_index && let Some(scope) = self.innermost_value_scope() {
            match shadowed {
                Some(previous) => scope.insert(var.to_string(), previous),
                None => scope.remove(var),
            };
        }
        self.pop_scope();
        *self.uninitialized.borrow_mut() = before;
        result
    }

    /// The value-parameter scope beside the innermost open type-parameter
    /// scope. A level past it is dead: `tparams.pop()` closes both.
    fn innermost_value_scope(&mut self) -> Option<&mut HashMap<String, ParamExpr>> {
        self.tparams
            .len()
            .checked_sub(1)
            .and_then(|level| self.vparams.get_mut(level))
    }

    /// Bind a `comptime NAME = value` constant under source validation. A
    /// type-valued binding becomes a scoped type alias; an annotated
    /// binding takes its annotation; a compile-time-only value the checker
    /// cannot type as a runtime value (a `TypeList` construction) is
    /// recorded for inlining at its compile-time uses. Returns `false` when
    /// the ordinary binding path — a compile-time `Int` or an inferable
    /// runtime value — applies.
    pub(super) fn bind_local_comptime(
        &mut self,
        stmt: &Stmt,
        name: &str,
        annotation: Option<&SourceType>,
        value: &Expr,
    ) -> Result<bool, TypeError> {
        let value = self.inline_local_comptime_values(value);
        if self.comptime_aliases.contains_key(name) {
            return Ok(true);
        }
        if let Some(annotation) = annotation
            && !super::declarations::is_string_literal_annotation(annotation)
        {
            let ty = self.ty_from_anno(annotation)?;
            self.declare_immutable(name, ty)?;
            self.record_statement_binding(stmt, name);
            return Ok(true);
        }
        if let Some(ty) = self.comptime_type_operand(&value)? {
            self.local_type_aliases
                .last_mut()
                .ok_or_else(|| {
                    TypeError::InvariantViolation("checker scope stack is empty".to_string())
                })?
                .insert(name.to_string(), ty);
            self.record_statement_binding(stmt, name);
            return Ok(true);
        }
        if self.eval_ct(&value).is_ok() {
            return Ok(false);
        }
        if self.infer(&value).is_ok() {
            return Ok(false);
        }
        // A nominal construction the elaborator evaluates itself (a
        // compile-time `Dict[K, V, H](keys, values, None)`) has the
        // constructed type whether or not its arguments type as a runtime
        // call; the binding takes that type and the elaborator checks the
        // construction.
        if let ExprKind::Call {
            name: callee,
            param_args,
            ..
        } = &value.kind
            && self.structs.contains_key(callee)
        {
            let ty = self.ty_from_anno(&SourceType::Named(callee.clone(), param_args.clone()))?;
            self.declare_immutable(name, ty)?;
            self.record_statement_binding(stmt, name);
            return Ok(true);
        }
        self.local_comptime_values
            .last_mut()
            .ok_or_else(|| {
                TypeError::InvariantViolation("checker scope stack is empty".to_string())
            })?
            .insert(name.to_string(), value);
        Ok(true)
    }

    /// The type an expression denotes in a compile-time position: a type
    /// parameter or local type alias, a nominal or scalar type, `Self.T`,
    /// or a type application.
    pub(super) fn comptime_type_operand(&self, expr: &Expr) -> Result<Option<Ty>, TypeError> {
        Ok(match &expr.kind {
            ExprKind::Identifier(name) => {
                if let Some(ty) = self.lookup_tparam(name) {
                    Some(ty)
                } else if scalar_type_name(name).is_some() || self.structs.contains_key(name) {
                    Some(self.ty_from_anno(&SourceType::Named(name.clone(), Vec::new()))?)
                } else {
                    None
                }
            }
            ExprKind::TypeValue(ty) => Some(self.ty_from_anno(ty)?),
            ExprKind::TypeApply { name, args }
                if !self.comptime_aliases.contains_key(name)
                    && mojito_types::types::trivial_predicate_name(name).is_none() =>
            {
                Some(self.ty_from_anno(&SourceType::Named(name.clone(), args.clone()))?)
            }
            ExprKind::Member { object, field } if matches!(&object.kind, ExprKind::Identifier(name) if name == "Self") => {
                Some(self.ty_from_anno(&SourceType::SelfParam(field.clone()))?)
            }
            ExprKind::Index { object, index }
                if matches!(&object.kind, ExprKind::Member { object, .. }
                    if matches!(&object.kind, ExprKind::Identifier(name) if name == "Self")) =>
            {
                let ExprKind::Member { field, .. } = &object.kind else {
                    unreachable!("guarded above");
                };
                Some(self.ty_from_anno(&SourceType::IndexedProjection {
                    base: Box::new(SourceType::SelfParam(field.clone())),
                    index: index.clone(),
                })?)
            }
            // `Ts[i]` over a `def`'s or method's own pack.
            ExprKind::Index { object, index }
                if matches!(&object.kind, ExprKind::Identifier(_))
                    && self.unbound_pack_named(object).is_some() =>
            {
                let ExprKind::Identifier(name) = &object.kind else {
                    unreachable!("guarded above");
                };
                Some(self.ty_from_anno(&SourceType::IndexedProjection {
                    base: Box::new(SourceType::Named(name.clone(), Vec::new())),
                    index: index.clone(),
                })?)
            }
            _ => None,
        })
    }

    /// `materialize[X]()` under source validation: the runtime value of a
    /// compile-time binding has the binding's own checked type (the display
    /// the elaborator materializes types the same way).
    pub(super) fn infer_materialize_crossing(
        &self,
        param_args: &[ParamArg],
    ) -> Result<Ty, TypeError> {
        let unsupported = || {
            TypeError::Unsupported("materialize[...]() takes one compile-time binding".to_string())
        };
        let [target] = param_args else {
            return Err(unsupported());
        };
        let (ParamArg::Value(Expr {
            kind: ExprKind::Identifier(name),
            ..
        })
        | ParamArg::Type(SourceType::Named(name, _))) = target
        else {
            return Err(unsupported());
        };
        self.lookup(name)
            .cloned()
            .ok_or_else(|| TypeError::UndefinedVariable(name.clone()))
    }

    /// The parameter-list reference of the variadic pack a spread names: a
    /// pack of an enclosing `def` or method first, then the enclosing
    /// struct's own.
    pub(super) fn pack_reference(&self, pack: &str) -> Option<ParamExpr> {
        let bare = pack.trim_start_matches('*');
        self.pack_parameter_in_scope(bare).or_else(|| {
            let owner = match &self.self_ty {
                Some(Ty::Struct(owner, _)) => owner.as_str(),
                _ => "Self",
            };
            pack_scope(owner, &self.self_decls)
                .remove(bare)
                .map(|reference| self.param_context.intern(&reference))
        })
    }

    /// The pack an expression names by itself: a bare `Ts` of an enclosing
    /// `def` or method, or the enclosing struct's `Self.Ts`.
    pub(super) fn unbound_pack_named(&self, expr: &Expr) -> Option<ParamExpr> {
        match &expr.kind {
            ExprKind::Identifier(name) if self.lookup(name).is_none() => {
                self.pack_parameter_in_scope(name)
            }
            ExprKind::Member { object, field }
                if matches!(&object.kind, ExprKind::Identifier(name) if name == "Self")
                    && self.self_decls.iter().any(|decl| {
                        matches!(decl, ParamDecl::Type { name, variadic: true, .. }
                            if name.trim_start_matches('*') == field)
                    }) =>
            {
                self.pack_reference(field)
            }
            _ => None,
        }
    }

    /// The type of element `index` of a variadic pack that is still a
    /// parameter: the dependent `Ts[index]`, whose index is a compile-time
    /// expression over the parameters and `comptime for` variables in scope.
    pub(super) fn pack_element_type(&self, pack: &Ty, index: &Expr) -> Result<Ty, TypeError> {
        let Ty::Param { name, .. } = pack else {
            return Err(TypeError::InvariantViolation(format!(
                "'{pack}' is not a variadic pack parameter"
            )));
        };
        let list = self.pack_reference(name).ok_or_else(|| {
            TypeError::SymbolicPackBoundary(format!("pack '{name}' is not in scope"))
        })?;
        let index = self
            .compile_dependent_ct_expr(index)
            .map_err(|_| TypeError::TypeMismatch {
                expected: "a compile-time Int index".to_string(),
                found: "a runtime value".to_string(),
                context: "variadic-pack index".to_string(),
            })?;
        self.param_context
            .list_get(&list, &index)
            .map(mojito_types::types::DependentType::resolve)
            .map_err(param_error)
    }

    /// Whether a spread's operand (`*args`, `*args^`) is a binding of a pack
    /// that is still a parameter.
    pub(super) fn spreads_unbound_pack(&self, spread: &Expr) -> bool {
        let source = match &spread.kind {
            ExprKind::Transfer(inner) => inner,
            _ => spread,
        };
        matches!(&source.kind, ExprKind::Identifier(binding)
            if matches!(self.lookup(binding), Some(Ty::VariadicPack(element))
                if mojito_types::types::pack_spread(std::slice::from_ref(&**element)).is_some()))
    }

    /// A construction from a whole pack that is still a parameter
    /// (`Tuple(*args^)`, `__RuntimeTuple(*args^)`): storage over that pack.
    /// The elaborator expands a spread per specialization, so any other
    /// callee of one has no symbolic rule. `None` when the call spreads no
    /// unbound pack.
    pub(super) fn infer_unbound_pack_construction(
        &self,
        name: &str,
        param_args: &[ParamArg],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Option<Result<Ty, TypeError>> {
        let [
            Expr {
                kind: ExprKind::Spread(spread),
                ..
            },
        ] = args
        else {
            return None;
        };
        let source = match &spread.kind {
            ExprKind::Transfer(inner) => inner,
            _ => spread,
        };
        let ExprKind::Identifier(binding) = &source.kind else {
            return None;
        };
        let Some(Ty::VariadicPack(element)) = self.lookup(binding) else {
            return None;
        };
        let pack = mojito_types::types::pack_spread(std::slice::from_ref(&**element))?.clone();
        let boundary = || {
            TypeError::SymbolicPackBoundary(format!(
                "'{name}' called with a spread of the unbound pack '{binding}'"
            ))
        };
        if !kwargs.is_empty() {
            return Some(Err(boundary()));
        }
        let constructed = match name {
            "__RuntimeTuple" => Ty::Tuple(vec![pack]),
            mojito_types::types::TUPLE_TYPE_NAME => mojito_types::types::tuple_type(vec![pack]),
            _ => return Some(Err(boundary())),
        };
        if param_args.is_empty() {
            return Some(Ok(constructed));
        }
        Some(
            self.ty_from_anno(&SourceType::Named(name.to_string(), param_args.to_vec()))
                .and_then(|declared| {
                    if declared == constructed {
                        Ok(constructed)
                    } else {
                        Err(TypeError::TypeMismatch {
                            expected: declared.to_string(),
                            found: constructed.to_string(),
                            context: format!("spread of '{binding}'"),
                        })
                    }
                }),
        )
    }

    /// A construction of a variadic struct over concrete types inside a
    /// validated body (`Tuple(1, "one")`, `Pair[Int, Bool](1, True)`). The
    /// instance and its constructor exist only once the elaborator mints
    /// them, so validation types the construction from its type arguments —
    /// a bare `Tuple(...)` as the tuple display it is — and checks the
    /// arguments as expressions; matching them against the instance's
    /// constructor is the executable check's. `None` outside validation and
    /// for every other callee.
    pub(super) fn infer_validated_variadic_construction(
        &self,
        call: &Expr,
        name: &str,
        param_args: &[ParamArg],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Option<Result<Ty, TypeError>> {
        if !self.source_validation || self.lookup(name).is_some() {
            return None;
        }
        let variadic = self.structs.get(name).is_some_and(|info| {
            info.decls
                .iter()
                .any(|decl| matches!(decl, ParamDecl::Type { variadic: true, .. }))
        });
        if !variadic {
            return None;
        }
        if param_args.is_empty() {
            if name != mojito_types::types::TUPLE_TYPE_NAME || !kwargs.is_empty() {
                return None;
            }
            let mut display = Expr::new(ExprKind::TupleLit(args.to_vec()), call.span);
            display.source.clone_from(&call.source);
            return Some(self.infer(&display));
        }
        Some(
            args.iter()
                .chain(kwargs.iter().map(|kwarg| &kwarg.value))
                .try_for_each(|argument| self.infer(argument).map(|_| ()))
                .and_then(|()| {
                    self.ty_from_anno(&SourceType::Named(name.to_string(), param_args.to_vec()))
                }),
        )
    }

    /// Close the pack elements a callee's type names (`Self.Ts[index]`)
    /// under a use's arguments: a value argument binds its parameter, a
    /// concrete pack its list, and a pack forwarded as a spread the caller's
    /// own pack. A type with no pack element is returned as it is.
    pub(super) fn close_pack_elements(&self, ty: Ty, arguments: &HashMap<String, TyArg>) -> Ty {
        let names_element =
            |ty: &Ty| matches!(ty, Ty::Dependent(dependent) if dependent.pack_element().is_some());
        if !mojito_types::types::mentions(&ty, &names_element) {
            return ty;
        }
        let values: HashMap<String, CtValue> = arguments
            .iter()
            .filter_map(|(name, argument)| match argument {
                TyArg::Val(value) => Some((name.clone(), value.clone())),
                TyArg::Ty(Ty::Param { name: pack, .. }) if pack.starts_with('*') => self
                    .pack_reference(pack)
                    .map(|reference| (name.clone(), CtValue::Expr(reference))),
                TyArg::Ty(_) | TyArg::Origin(_) => None,
            })
            .collect();
        self.resolve_dependent_ty(&ty, &values).unwrap_or(ty)
    }

    /// The bounded type parameter an element of an unbound variadic pack
    /// behaves as: the pack's declared bounds, plus every trait the enclosing
    /// declarations' `where` clauses guarantee of its elements. The dependent
    /// type stays the element's identity; this is what its capabilities are
    /// read from.
    pub(super) fn opaque_element(&self, ty: &Ty) -> Option<Ty> {
        let Ty::Dependent(dependent) = ty else {
            return None;
        };
        let (list, _) = dependent.pack_element()?;
        let pack = list.as_decl_ref()?.name.to_string();
        let declared = self.lookup_tparam(&format!("*{pack}")).or_else(|| {
            self.self_decls.iter().find_map(|decl| match decl {
                ParamDecl::Type {
                    name,
                    bounds,
                    callable_bound,
                    variadic: true,
                    ..
                } if name.trim_start_matches('*') == pack => Some(Ty::Param {
                    name: name.clone(),
                    bounds: bounds.clone(),
                    callable_bound: callable_bound.clone(),
                }),
                _ => None,
            })
        });
        let mut bounds = match declared {
            Some(Ty::Param { bounds, .. }) => bounds,
            _ => Vec::new(),
        };
        for (parameter, guaranteed) in self.assumed_conformances.iter().flatten() {
            if parameter.trim_start_matches('*') == pack && !bounds.contains(guaranteed) {
                bounds.push(guaranteed.clone());
            }
        }
        // Two packs may share a spelling (`Tuple.Ts`, `Bag.Ts`); a view's name
        // is its element's alone, so the owner qualifies the later one.
        let spelled = dependent.expr().to_string();
        let mut views = self.pack_element_views.borrow_mut();
        let name = match views.get(&spelled) {
            Some(viewed) if viewed != ty => {
                format!("{}.{spelled}", list.as_decl_ref()?.id.owner)
            }
            _ => spelled,
        };
        views.insert(name.clone(), ty.clone());
        Some(Ty::Param {
            name,
            bounds,
            callable_bound: None,
        })
    }

    /// Reject a type-argument list that spreads a pack beside other
    /// arguments (`Tuple[Int, *Self.Ts]`), as the pinned Mojo does.
    pub(super) fn reject_mixed_spread(name: &str, arguments: &[Ty]) -> Result<(), TypeError> {
        let spreads = arguments
            .iter()
            .any(|ty| mojito_types::types::pack_spread(std::slice::from_ref(ty)).is_some());
        if spreads && arguments.len() > 1 {
            return Err(TypeError::BadCall {
                func: name.to_string(),
                reason: "a variadic pack spread must be the only argument it binds".to_string(),
            });
        }
        Ok(())
    }

    /// Give a type computed over bounded views of pack elements its
    /// dependent elements back: a view is how an element's capabilities are
    /// read, never a type a program's values have.
    pub(super) fn restore_pack_elements(&self, ty: Ty) -> Ty {
        let views = self.pack_element_views.borrow();
        if views.is_empty() {
            return ty;
        }
        substitute(&ty, &views)
    }

    /// Whether a validation error marks the validator's own blind spot
    /// rather than a verdict: a constructor, method, or operator of a struct
    /// registered only as a template shell (a `DType`-, vector-, or
    /// struct-value-keyed struct), whose members exist only per specialization.
    /// The executable check still covers the arm elaboration selects.
    pub(super) fn is_template_shell_member_error(&self, error: &TypeError) -> bool {
        let names_shell = |spelling: &str| {
            let head = spelling.split('[').next().unwrap_or(spelling).trim();
            self.structs
                .get(head)
                .is_some_and(|info| info.template_shell)
        };
        match error {
            TypeError::NoConstructor(name) | TypeError::BadCall { func: name, .. } => {
                names_shell(name)
            }
            TypeError::NoSuchMethod { object_type, .. } => names_shell(object_type),
            TypeError::BadOperator { operands, .. } => operands.split(" and ").any(names_shell),
            _ => false,
        }
    }

    /// Whether a nominal type has no checkable declaration here: unregistered
    /// (a discovery-round abstract scalar range) or registered only as a
    /// template shell (the same family under source validation). Its
    /// iteration and subscript contracts come from the family, not from
    /// method lookup.
    pub(super) fn is_abstract_struct(&self, name: &str) -> bool {
        self.structs
            .get(name)
            .is_none_or(|info| info.template_shell)
    }

    /// Every `Param` operand of a compiled condition must name a parameter,
    /// binding, type, trait, or alias in scope: the constraint compiler
    /// reads any bare identifier as a parameter name.
    fn constraint_operands_resolve(&self, constraint: &GenericConstraint) -> Result<(), TypeError> {
        let operand = |operand: &ConstraintOperand| match operand {
            ConstraintOperand::Param(name) | ConstraintOperand::PackLength(name) => {
                self.comptime_name_resolves(name)
            }
            // An arithmetic operand resolved its names when it was compiled.
            ConstraintOperand::Value(_)
            | ConstraintOperand::Type(_)
            | ConstraintOperand::Expr(_) => Ok(()),
        };
        match constraint {
            GenericConstraint::WithMessage(inner, _) | GenericConstraint::Not(inner) => {
                self.constraint_operands_resolve(inner)
            }
            GenericConstraint::Conforms { param, .. }
            | GenericConstraint::ConformsPack { param, .. }
            | GenericConstraint::PackPredicate { param, .. } => self.comptime_name_resolves(param),
            GenericConstraint::PackContains { param, element } => {
                self.comptime_name_resolves(param)?;
                operand(element)
            }
            GenericConstraint::Trivial(_, value) => operand(value),
            GenericConstraint::Eq(a, b)
            | GenericConstraint::Ne(a, b)
            | GenericConstraint::Lt(a, b)
            | GenericConstraint::Le(a, b)
            | GenericConstraint::Gt(a, b)
            | GenericConstraint::Ge(a, b) => {
                operand(a)?;
                operand(b)
            }
            GenericConstraint::And(a, b) | GenericConstraint::Or(a, b) => {
                self.constraint_operands_resolve(a)?;
                self.constraint_operands_resolve(b)
            }
            GenericConstraint::Bool(_) => Ok(()),
        }
    }

    fn comptime_name_resolves(&self, name: &str) -> Result<(), TypeError> {
        let name = name.trim_start_matches('*');
        let known = self.lookup_tparam(name).is_some()
            || self.lookup(name).is_some()
            || self.comptimes.contains_key(name)
            || self.comptime_aliases.contains_key(name)
            || self.structs.contains_key(name)
            || self.traits.contains_key(name)
            || self.self_decls.iter().any(|decl| decl.name() == name)
            || self
                .enclosing_type_params
                .iter()
                .any(|parameter| parameter.name.trim_start_matches('*') == name);
        if known {
            Ok(())
        } else {
            Err(TypeError::UndefinedVariable(name.to_string()))
        }
    }

    /// Whether a condition reads a type-valued operand (`T == Int`,
    /// `Self.Ts[i] == T`) or a `TypeList` value (`tl.length == 2`,
    /// `tl.contains[Int]()`), so its diagnosis belongs to the constraint
    /// compiler rather than to value typing.
    fn condition_is_compile_time_shaped(&self, cond: &Expr) -> bool {
        let compile_time = |expr: &Expr| match &expr.kind {
            ExprKind::Member { object, .. } | ExprKind::MethodCall { object, .. } => {
                matches!(self.typelist_receiver(object), Ok(Some(_)))
            }
            ExprKind::Invoke { callee, .. } => {
                matches!(&callee.kind,
                ExprKind::Member { object, .. } | ExprKind::Index { object, .. }
                    if matches!(self.typelist_receiver(object), Ok(Some(_))))
                    || matches!(&callee.kind, ExprKind::Index { object, .. }
                    if matches!(&object.kind, ExprKind::Member { object, .. }
                        if matches!(self.typelist_receiver(object), Ok(Some(_)))))
            }
            _ => matches!(self.comptime_type_operand(expr), Ok(Some(_))),
        };
        match &cond.kind {
            ExprKind::Infix(_, left, right) => {
                self.condition_is_compile_time_shaped(left)
                    || self.condition_is_compile_time_shaped(right)
            }
            ExprKind::Compare { first, rest } => {
                compile_time(first) || rest.iter().any(|(_, operand)| compile_time(operand))
            }
            ExprKind::Prefix(_, inner) => self.condition_is_compile_time_shaped(inner),
            _ => compile_time(cond),
        }
    }

    /// The element type a `comptime for` iterable yields.
    fn comptime_iteration_element(&self, iter: &Expr) -> Result<Ty, TypeError> {
        if let ExprKind::Call { name, args, .. } = &iter.kind
            && name == "range"
        {
            self.infer_range(args)?;
            return Ok(Ty::Int);
        }
        let ty = self.infer(iter)?;
        let not_iterable = |elements: &[Ty]| {
            TypeError::Unsupported(format!(
                "'Tuple[{}]' does not implement the '__iter__' method",
                elements
                    .iter()
                    .map(materialized_element_spelling)
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        };
        match ty {
            Ty::ComptimeList(element) | Ty::VariadicPack(element) => Ok(*element),
            // A compile-time dictionary iterates its keys, a set its elements.
            ref dict if let Some((key, _)) = mojito_types::types::dict_elements(dict) => {
                Ok(key.clone())
            }
            ref set if let Some(element) = mojito_types::types::set_element(set) => {
                Ok(element.clone())
            }
            // A list display of literals is a fixed-size array here; the
            // elaborator's list value iterates by element type.
            Ty::Struct(ref name, ref args)
                if (name == mojito_types::types::LIST_TYPE_NAME
                    || name == mojito_types::types::ARRAY_TYPE_NAME)
                    && let Some(TyArg::Ty(element)) = args.first() =>
            {
                Ok(default_literal(element))
            }
            // A compile-time `Tuple` has no `__iter__`, as upstream; the
            // rejection spells the tuple's element types as the elaborator
            // does for the same program.
            Ty::Tuple(elements) | Ty::RuntimePack(elements) => Err(not_iterable(&elements)),
            Ty::Struct(ref name, ref args) if name == mojito_types::types::TUPLE_TYPE_NAME => {
                let elements: Vec<Ty> = args
                    .iter()
                    .filter_map(|argument| match argument {
                        TyArg::Ty(element) => Some(element.clone()),
                        TyArg::Val(_) | TyArg::Origin(_) => None,
                    })
                    .collect();
                Err(not_iterable(&elements))
            }
            other => Err(TypeError::Unsupported(format!(
                "'comptime for' iterates a range, a compile-time list, or a pack; found '{other}'"
            ))),
        }
    }

    /// Substitute the defining expression of every function-local
    /// compile-time value binding named in `expr`.
    fn inline_local_comptime_values(&self, expr: &Expr) -> Expr {
        if self.local_comptime_values.iter().all(HashMap::is_empty) {
            return expr.clone();
        }
        let lookup = |name: &str| {
            self.local_comptime_values
                .iter()
                .rev()
                .find_map(|scope| scope.get(name))
        };
        substitute_identifiers(expr, &lookup)
    }
}

/// How the pipeline checks one parameterized module-level declaration's
/// body, as the predicates below decide it. A class belongs to a declaration,
/// never to a name: overloads of one name can sit in different classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TemplateClass {
    /// Checked abstractly by the executable pass and kept as an erased body.
    SurvivingTraitBound,
    /// Checked symbolically by source validation, then stubbed.
    ValidatedKeyed,
    /// Checked only per specialization.
    ConcreteOnly,
}

impl TemplateClass {
    pub(super) const fn counter(self) -> &'static str {
        match self {
            Self::SurvivingTraitBound => "templates.surviving_trait_bound",
            Self::ValidatedKeyed => "templates.validated_keyed",
            Self::ConcreteOnly => "templates.concrete_only",
        }
    }
}

/// Report the template classes of a prepared program's module-level
/// declarations (a generic struct counts once per method) as timing counters.
pub(super) fn count_template_classes(stmts: &[Stmt], rebind_keyed: &HashSet<SourceSpan>) {
    if !timing::enabled() {
        return;
    }
    let declared_structs: HashSet<&str> = stmts
        .iter()
        .filter_map(|statement| match &statement.kind {
            StmtKind::Struct { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    let body_class = |enclosing: &[mojito_ast::ast::TypeParam],
                      type_params: &[mojito_ast::ast::TypeParam],
                      body: &[Stmt]| {
        let keys_rebind = body_keys_rebind(body, rebind_keyed);
        if validates_body(enclosing, type_params, body, keys_rebind) {
            TemplateClass::ValidatedKeyed
        } else if keys_rebind || block_has_comptime(body) {
            TemplateClass::ConcreteOnly
        } else {
            TemplateClass::SurvivingTraitBound
        }
    };
    for statement in stmts {
        match &statement.kind {
            StmtKind::Def {
                name,
                type_params,
                body,
                ..
            } if !type_params.is_empty() => {
                let class = if concrete_only_def(statement) {
                    TemplateClass::ConcreteOnly
                } else {
                    body_class(&[], type_params, body)
                };
                timing::count(class.counter(), 1);
                timing::note(class.counter(), || name.clone());
            }
            StmtKind::Struct {
                name,
                type_params,
                methods,
                ..
            } if !type_params.is_empty() => {
                let shell =
                    concrete_only_struct(type_params, &|bound| declared_structs.contains(bound));
                for method in methods {
                    let class = if shell {
                        TemplateClass::ConcreteOnly
                    } else {
                        body_class(type_params, &method.type_params, &method.body)
                    };
                    timing::count(class.counter(), 1);
                    timing::note(class.counter(), || format!("{name}.{}", method.name));
                }
            }
            _ => {}
        }
    }
}

/// The pack binding of a variadic struct applied element by element
/// (`Tuple[Int, String]`, whose arguments are its element types): the pack's
/// name and the list of those types. A pack bound whole — a list value, or a
/// spread of another pack — is its own argument already.
pub(super) fn positional_pack_binding(
    decls: &[ParamDecl],
    arguments: &[TyArg],
) -> Option<(String, TyArg)> {
    let [
        ParamDecl::Type {
            name,
            variadic: true,
            ..
        },
    ] = decls
    else {
        return None;
    };
    if mojito_types::types::pack_spread_argument(arguments).is_some() {
        return None;
    }
    arguments
        .iter()
        .map(|argument| match argument {
            TyArg::Ty(ty) => Some(CtValue::Type(Box::new(ty.clone()))),
            TyArg::Val(_) | TyArg::Origin(_) => None,
        })
        .collect::<Option<Vec<_>>>()
        .map(|types| {
            (
                name.trim_start_matches('*').to_string(),
                TyArg::Val(CtValue::Tuple(types)),
            )
        })
}

/// Whether a declaration has a `DType` parameter.
pub(super) fn dtype_keyed(type_params: &[mojito_ast::ast::TypeParam]) -> bool {
    type_params
        .iter()
        .any(|parameter| matches!(parameter.bounds.as_slice(), [only] if only == "DType"))
}

/// Whether a struct declaration checks only per specialization, so source
/// validation registers it as a template shell: a `DType` parameter, a
/// struct-typed value parameter (`is_value_struct` names the declared
/// structs), or a vector-typed value parameter — shapes the elaborator
/// monomorphizes per application and the checker has no symbolic form for. A
/// variadic pack is not one: its element has a dependent type.
pub(super) fn concrete_only_struct(
    type_params: &[mojito_ast::ast::TypeParam],
    is_value_struct: &dyn Fn(&str) -> bool,
) -> bool {
    type_params.iter().any(|parameter| {
        matches!(parameter.bounds.as_slice(), [only] if only == "DType" || is_value_struct(only))
            || matches!(&parameter.value_type, Some(SourceType::Named(name, _)) if name == "SIMD")
    })
}

/// Whether a function declaration checks only per specialization — a
/// `DType` parameter, or a vector width naming one of its own parameters —
/// so source validation neither declares nor checks it: `Ty::Simd` holds a
/// concrete element type and width, and the elaborator retargets every call
/// to a clone before the executable check.
pub(super) fn concrete_only_def(stmt: &Stmt) -> bool {
    let StmtKind::Def {
        type_params,
        params,
        ret,
        body,
        ..
    } = &stmt.kind
    else {
        return false;
    };
    if type_params.is_empty() {
        return false;
    }
    if dtype_keyed(type_params) {
        return true;
    }
    let mut finder = ParamSimdWidthFinder {
        names: type_params
            .iter()
            .map(|parameter| parameter.name.as_str())
            .collect(),
        found: false,
    };
    for parameter in params {
        mojito_ast::visit::walk_type(&mut finder, &parameter.ty);
    }
    if let Some(ret) = ret {
        mojito_ast::visit::walk_type(&mut finder, ret);
    }
    mojito_ast::visit::walk_block(&mut finder, body);
    finder.found
}

/// Whether source validation checks a declaration's body: one holding
/// compile-time control flow, a `rebind` over the declaration's own
/// parameters (`keys_rebind`, from `rebind::rebind_keyed_bodies`), or keyed
/// on a variadic pack — the declaration's own, or that of the struct
/// `enclosing` it — each leaves the template stubbed, so validation is the
/// only check it gets. Either way a body that checks
/// only per instantiation is left out: one reading a reflection handle
/// (`reflect[T]`), whose field facts only the elaborator evaluates.
pub(super) fn validates_body(
    enclosing: &[mojito_ast::ast::TypeParam],
    type_params: &[mojito_ast::ast::TypeParam],
    body: &[Stmt],
    keys_rebind: bool,
) -> bool {
    (block_has_comptime(body)
        || keys_rebind
        || is_variadic_template(type_params)
        || is_variadic_template(enclosing))
        && !reads_reflection(body)
}

/// Whether a block holds a `comptime if`/`comptime for` anywhere below it,
/// nested function bodies included.
pub(super) fn block_has_comptime(stmts: &[Stmt]) -> bool {
    stmts.iter().any(stmt_has_comptime)
}

/// Finds a `SIMD[_, width]` whose width names one of a declaration's own
/// parameters, in type or expression position.
struct ParamSimdWidthFinder<'a> {
    names: Vec<&'a str>,
    found: bool,
}

impl ParamSimdWidthFinder<'_> {
    fn width_names_param(&self, args: &[ParamArg]) -> bool {
        matches!(args.get(1), Some(ParamArg::Value(width))
            if matches!(&width.kind, ExprKind::Identifier(name) if self.names.contains(&name.as_str())))
    }
}

impl mojito_ast::visit::Visitor for ParamSimdWidthFinder<'_> {
    fn visit_expr(&mut self, expr: &Expr) {
        match &expr.kind {
            ExprKind::TypeApply { name, args }
            | ExprKind::Call {
                name,
                param_args: args,
                ..
            } if name == "SIMD" && self.width_names_param(args) => self.found = true,
            _ => {}
        }
    }

    fn visit_type(&mut self, ty: &SourceType) {
        if let SourceType::Named(name, args) = ty
            && name == "SIMD"
            && self.width_names_param(args)
        {
            self.found = true;
        }
    }
}

/// Whether a block names `reflect[...]` anywhere below it.
fn reads_reflection(stmts: &[Stmt]) -> bool {
    let mut finder = ReflectionFinder { found: false };
    mojito_ast::visit::walk_block(&mut finder, stmts);
    finder.found
}

struct ReflectionFinder {
    found: bool,
}

impl mojito_ast::visit::Visitor for ReflectionFinder {
    fn visit_expr(&mut self, expr: &Expr) {
        if matches!(&expr.kind, ExprKind::TypeApply { name, .. } | ExprKind::Call { name, .. } if name == "reflect")
        {
            self.found = true;
        }
    }
}

fn stmt_has_comptime(stmt: &Stmt) -> bool {
    match &stmt.kind {
        StmtKind::ComptimeIf { .. } | StmtKind::ComptimeFor { .. } => true,
        StmtKind::If { branches, orelse } => {
            branches.iter().any(|(_, body)| block_has_comptime(body))
                || orelse.as_ref().is_some_and(|body| block_has_comptime(body))
        }
        StmtKind::While { body, orelse, .. } | StmtKind::For { body, orelse, .. } => {
            block_has_comptime(body) || orelse.as_ref().is_some_and(|body| block_has_comptime(body))
        }
        StmtKind::With { body, .. } | StmtKind::Def { body, .. } => block_has_comptime(body),
        StmtKind::Try {
            body,
            except,
            orelse,
            finalbody,
        } => {
            block_has_comptime(body)
                || except
                    .as_ref()
                    .is_some_and(|(_, body)| block_has_comptime(body))
                || orelse.as_ref().is_some_and(|body| block_has_comptime(body))
                || finalbody
                    .as_ref()
                    .is_some_and(|body| block_has_comptime(body))
        }
        _ => false,
    }
}

/// The element spelling of a materialized compile-time tuple: a literal
/// element is named by the type it materializes to, as the elaborator
/// spells the same rejection.
fn materialized_element_spelling(ty: &Ty) -> String {
    match ty {
        Ty::IntLiteral => "Int".to_string(),
        Ty::StringLiteral => "String".to_string(),
        Ty::FloatLiteral => "Float64".to_string(),
        Ty::Struct(name, args) if args.is_empty() => name.clone(),
        other => other.to_string(),
    }
}

/// Clone `expr` with every identifier that `lookup` binds replaced by its
/// binding, recursing through the expression forms a compile-time condition
/// or iterable can take.
fn substitute_identifiers<'a>(expr: &Expr, lookup: &dyn Fn(&str) -> Option<&'a Expr>) -> Expr {
    let sub = |inner: &Expr| substitute_identifiers(inner, lookup);
    let sub_box = |inner: &Expr| Box::new(sub(inner));
    let kind = match &expr.kind {
        ExprKind::Identifier(name) => match lookup(name) {
            Some(bound) => return bound.clone(),
            None => return expr.clone(),
        },
        ExprKind::Prefix(op, inner) => ExprKind::Prefix(*op, sub_box(inner)),
        ExprKind::Infix(op, left, right) => ExprKind::Infix(*op, sub_box(left), sub_box(right)),
        ExprKind::Compare { first, rest } => ExprKind::Compare {
            first: sub_box(first),
            rest: rest
                .iter()
                .map(|(op, operand)| (*op, sub(operand)))
                .collect(),
        },
        ExprKind::Call {
            name,
            param_args,
            args,
            kwargs,
        } => ExprKind::Call {
            name: name.clone(),
            param_args: param_args.clone(),
            args: args.iter().map(sub).collect(),
            kwargs: kwargs
                .iter()
                .map(|kwarg| mojito_ast::ast::KwArg {
                    name: kwarg.name.clone(),
                    value: sub(&kwarg.value),
                })
                .collect(),
        },
        ExprKind::Invoke {
            callee,
            param_args,
            args,
            kwargs,
        } => ExprKind::Invoke {
            callee: sub_box(callee),
            param_args: param_args.clone(),
            args: args.iter().map(sub).collect(),
            kwargs: kwargs
                .iter()
                .map(|kwarg| mojito_ast::ast::KwArg {
                    name: kwarg.name.clone(),
                    value: sub(&kwarg.value),
                })
                .collect(),
        },
        ExprKind::Member { object, field } => ExprKind::Member {
            object: sub_box(object),
            field: field.clone(),
        },
        ExprKind::MethodCall {
            object,
            method,
            args,
            kwargs,
        } => ExprKind::MethodCall {
            object: sub_box(object),
            method: method.clone(),
            args: args.iter().map(sub).collect(),
            kwargs: kwargs
                .iter()
                .map(|kwarg| mojito_ast::ast::KwArg {
                    name: kwarg.name.clone(),
                    value: sub(&kwarg.value),
                })
                .collect(),
        },
        ExprKind::Index { object, index } => ExprKind::Index {
            object: sub_box(object),
            index: sub_box(index),
        },
        ExprKind::TupleLit(elements) => ExprKind::TupleLit(elements.iter().map(sub).collect()),
        ExprKind::ListLit(elements) => ExprKind::ListLit(elements.iter().map(sub).collect()),
        _ => return expr.clone(),
    };
    let mut rewritten = Expr::new(kind, expr.span);
    rewritten.source.clone_from(&expr.source);
    rewritten
}

/// The compile-time binder of an integer `comptime for` variable, so a
/// dependent index over it (`Ts[i]`, `args[i]`) has a node. The loop's
/// iterable names the binder: each loop owns its variable.
fn comptime_index_binder(var: &str, iter: &Expr) -> ParamExpr {
    let span = iter.source_span();
    let owner = format!(
        "$comptime_for@{}:{}..{}",
        span.source.as_deref().unwrap_or_default(),
        span.span.0,
        span.span.1
    );
    value_parameter_expr(&owner, 0, var, &Ty::Int)
}
