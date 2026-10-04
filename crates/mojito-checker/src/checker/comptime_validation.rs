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

/// A call argument that forwards a variadic pack whole (`*args`, `*args^`)
/// while the pack is still a parameter.
pub(super) struct ForwardedPack {
    /// The collector binding the spread names.
    pub(super) binding: String,
    /// The pack as its bounded binder (`*Ts` with its declared bounds).
    pub(super) pack: Ty,
    /// Whether the spread carries the `^`.
    pub(super) transferred: bool,
    /// Whether the collector was declared `var`.
    pub(super) owned: bool,
}

impl Checker {
    /// Check the method bodies of a struct that hold compile-time control
    /// flow, each with `self` bound at the struct's own parameters, every
    /// method body of a value-keyed struct (`value_keyed_struct`), and every
    /// body constructing a vector at a lane its method's own binders spell
    /// (`method_constructs_at_own_lane`), whose elaborated template is a
    /// trap stub.
    pub(super) fn validate_comptime_method_bodies(
        &mut self,
        declaration: &StructDeclaration<'_>,
        self_ty: &Ty,
    ) -> Result<(), TypeError> {
        let mut overload_indices = HashMap::<String, usize>::new();
        let value_keyed = value_keyed_struct(&self.self_decls, declaration);
        for (method_index, m) in declaration.methods.iter().enumerate() {
            let method_name = lifecycle_method_name(m).to_string();
            let overload_index = *overload_indices.entry(method_name.clone()).or_default();
            *overload_indices
                .get_mut(&method_name)
                .expect("inserted above") += 1;
            let validated = validates_body(
                declaration.type_params,
                &m.type_params,
                &m.body,
                body_keys_rebind(&m.body, &self.rebind_keyed_bodies),
            );
            let own_lane = mojito_ast::simd_width::method_constructs_at_own_lane(m);
            if !(validated || value_keyed || own_lane) {
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
            // A value-keyed member, and a method constructing a vector at a
            // lane of its own binders, is validated only to produce its
            // template: each specialization or per-call clone is still
            // checked, or derived, so a body the symbolic check cannot type
            // gets no verdict.
            let checked = match checked {
                Err(error) if !validated && !matches!(error, TypeError::SymbolicBoundary(_)) => {
                    Err(TypeError::SymbolicBoundary(error.to_string()))
                }
                checked => checked,
            };
            self.symbolic_verdict(
                &format!("{}.{method_name}", declaration.name),
                &m.body,
                scopes,
                checked,
            )?;
        }
        Ok(())
    }

    /// Keep a body's validation result, unless it ended at a use of an
    /// unbound pack or a reflected symbolic type with no symbolic rule: that
    /// is no verdict, so the body is recorded and left to its
    /// per-instantiation check. `scopes` is the scope depth before the body,
    /// restored past the abandoned check.
    pub(super) fn symbolic_verdict(
        &mut self,
        name: &str,
        body: &[Stmt],
        scopes: usize,
        checked: Result<(), TypeError>,
    ) -> Result<(), TypeError> {
        match checked {
            Err(TypeError::SymbolicBoundary(what)) if self.source_validation => {
                while self.scopes.len() > scopes {
                    self.pop_scope();
                }
                timing::count("templates.no_verdict", 1);
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
    /// evaluated: selection is the elaborator's. A condition, or a leaf of
    /// one under `not`/`and`/`or`, that the constraint compiler closes is
    /// recorded on its span (`SemanticAdjustment::ComptimeCondition`) for the
    /// MIR branch; a leaf it does not close — an application, a `Bool`
    /// binding — MIR lowers as a thunk the elaborator runs.
    pub(super) fn check_comptime_condition(&mut self, cond: &Expr) -> Result<(), TypeError> {
        let inlined = self.inline_local_comptime_values(cond);
        if let Some(constraint) = self.check_ct_bool(&inlined)? {
            self.record_comptime_condition(cond, &constraint);
        }
        Ok(())
    }

    /// Record `constraint` as the compile-time reading of `cond`, each
    /// operand naming its binder by identity: the innermost open scope's,
    /// else the enclosing struct's.
    fn record_comptime_condition(&self, cond: &Expr, constraint: &GenericConstraint) {
        let bound = self.bind_constraint(constraint, &[]);
        self.operation_adjustments.borrow_mut().insert(
            cond.source_span(),
            mojito_checked::checked::SemanticAdjustment::ComptimeCondition(bound),
        );
    }

    /// The recursive form of [`Self::check_comptime_condition`]: a
    /// connective recurses so a concrete conformance fact can sit beside a
    /// symbolic constraint; a leaf is a generic constraint over the
    /// parameters in scope, a conformance of a concrete type, or a `Bool`
    /// value expression. The constraint the condition compiles to, when every
    /// leaf does.
    fn check_ct_bool(&mut self, cond: &Expr) -> Result<Option<GenericConstraint>, TypeError> {
        match &cond.kind {
            ExprKind::Bool(value) => return Ok(Some(GenericConstraint::Bool(*value))),
            ExprKind::Prefix(PrefixOp::Not, inner) => {
                return Ok(self
                    .check_ct_bool(inner)?
                    .map(|inner| GenericConstraint::Not(Box::new(inner))));
            }
            ExprKind::Infix(op @ (InfixOp::And | InfixOp::Or), left, right) => {
                let left = self.check_ct_bool(left)?;
                let right = self.check_ct_bool(right)?;
                return Ok(left.zip(right).map(|(left, right)| {
                    if *op == InfixOp::And {
                        GenericConstraint::And(Box::new(left), Box::new(right))
                    } else {
                        GenericConstraint::Or(Box::new(left), Box::new(right))
                    }
                }));
            }
            // `conforms_to(MaybeUninit[Int], RegisterPassable)`: a fact about
            // a concrete type, which the constraint compiler reserves for
            // parameters.
            ExprKind::Call { name, args, .. }
                if name == "conforms_to"
                    && args.len() == 2
                    && self.comptime_type_operand(&args[0])?.is_some() =>
            {
                let Some(trait_names) = mojito_ast::ast::trait_conjunction_names(&args[1]) else {
                    return Err(TypeError::Unsupported(
                        "conforms_to takes a trait name, or a '&' conjunction of trait names, \
                         as its second argument"
                            .to_string(),
                    ));
                };
                trait_names
                    .into_iter()
                    .try_for_each(|trait_name| self.check_trait_name(trait_name))?;
                return Ok(None);
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
            Ok(constraint) => {
                self.record_comptime_condition(cond, &constraint);
                Ok(Some(constraint))
            }
            Err(constraint_error) => match self.expect_bool(cond, "comptime if condition") {
                Ok(()) => Ok(None),
                // A condition over types or a `TypeList` has no value
                // reading; its constraint diagnosis is the one that names
                // the problem.
                Err(_) if self.condition_is_compile_time_shaped(cond) => Err(constraint_error),
                Err(error) => Err(error),
            },
        }
    }

    /// The conformances a `comptime if` condition proves of its operands for
    /// the arm it guards, as the pinned Mojo licenses them: every
    /// `conforms_to(X, A & B)` atom under `and`, keyed by what `X` denotes — a
    /// parameter's binder, a dependent element's canonical expression — so
    /// the proof reaches exactly that element, and only below the condition.
    /// `or`, `not`, and any other leaf prove nothing.
    pub(super) fn conformance_arm_assumptions(
        &self,
        cond: &Expr,
    ) -> Result<HashSet<(AssumedSubject, String)>, TypeError> {
        let cond = self.inline_local_comptime_values(cond);
        let mut proved = HashSet::new();
        self.collect_arm_assumptions(&cond, &mut proved)?;
        Ok(proved)
    }

    fn collect_arm_assumptions(
        &self,
        cond: &Expr,
        proved: &mut HashSet<(AssumedSubject, String)>,
    ) -> Result<(), TypeError> {
        match &cond.kind {
            ExprKind::Infix(InfixOp::And, left, right) => {
                self.collect_arm_assumptions(left, proved)?;
                self.collect_arm_assumptions(right, proved)
            }
            ExprKind::Call { name, args, .. } if name == "conforms_to" && args.len() == 2 => {
                let Some(trait_names) = mojito_ast::ast::trait_conjunction_names(&args[1]) else {
                    return Ok(());
                };
                let key = match self.comptime_type_operand(&args[0])? {
                    Some(Ty::Param { binder, .. }) => AssumedSubject::Binder(binder.id),
                    Some(Ty::Dependent(dependent)) => {
                        AssumedSubject::Element(dependent.expr().clone())
                    }
                    _ => return Ok(()),
                };
                proved.extend(trait_names.into_iter().map(|trait_name| {
                    (
                        key.clone(),
                        mojito_ast::ast::canonical_trait_name(trait_name).to_string(),
                    )
                }));
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// Check a `comptime for` body once, its variable bound to the element
    /// type of the iterable: `Int` for `range(...)`, the element of a
    /// compile-time list or a value pack. The body is a loop body — a
    /// compile-time `break` and `continue` are a loop's — that may run zero
    /// times, so definite initialization is unchanged. A `range` over
    /// parameter expressions is recorded on the iterable for the MIR loop
    /// header; the elaborator unrolls it.
    pub(super) fn check_comptime_for(
        &mut self,
        stmt: &Stmt,
        var: &str,
        iter: &Expr,
        body: &[Stmt],
        ret: Option<&Ty>,
    ) -> Result<(), TypeError> {
        let source = iter;
        let iter = self.inline_local_comptime_values(iter);
        let element = self.comptime_iteration_element(&iter)?;
        let before = self.uninitialized.borrow().clone();
        self.push_scope();
        if let Some(bindings) = self.compile_time_bindings.last_mut() {
            bindings.insert(var.to_string());
        }
        let binds_index = element == Ty::Int;
        let binder = binds_index.then(|| comptime_index_binder(var, &iter));
        if let Some(binder) = &binder {
            self.record_comptime_iteration(source, &iter, binder)?;
        }
        let shadowed = binder.and_then(|binder| {
            self.innermost_value_scope()
                .and_then(|scope| scope.insert(var.to_string(), binder))
        });
        let result = self.declare_immutable(var, element).and_then(|()| {
            // The loop variable's binding, which the MIR header's slot is
            // minted from as a `for` statement's is.
            self.record_statement_binding(stmt, var);
            self.check_block(body, ret, true)
        });
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

    /// Record a `range(...)` iterable as the loop header's parameter
    /// expressions, each bound compiled over the binders in scope
    /// (`SemanticAdjustment::ComptimeIteration`). A bound the compiler does
    /// not close — a pack length, a compile-time list's — is left unrecorded
    /// under source validation, where the loop is the cloner's to unroll, and
    /// is the explicit boundary in the executable check, which only sees a
    /// loop the elaborator kept.
    fn record_comptime_iteration(
        &self,
        source: &Expr,
        iter: &Expr,
        binder: &ParamExpr,
    ) -> Result<(), TypeError> {
        let (ExprKind::Call { name, args, .. }, Some(index)) = (&iter.kind, binder.as_decl_ref())
        else {
            return Ok(());
        };
        if name != "range" {
            return Ok(());
        }
        let bounds = args
            .iter()
            .map(|arg| self.compile_dependent_ct_expr(arg))
            .collect::<Result<Vec<_>, _>>();
        let bounds = match bounds {
            Ok(bounds) => bounds,
            Err(_) if self.source_validation => return Ok(()),
            Err(error) => {
                return Err(TypeError::Unsupported(format!(
                    "comptime for bound is not a parameter expression: {error}"
                )));
            }
        };
        let constant = |value: i64| {
            self.param_context
                .constant(mojito_types::ct::CtValue::Int(value))
                .map_err(|error| TypeError::Unsupported(error.to_string()))
        };
        let (start, stop, step) = match bounds.as_slice() {
            [stop] => (constant(0)?, stop.clone(), constant(1)?),
            [start, stop] => (start.clone(), stop.clone(), constant(1)?),
            [start, stop, step] => (start.clone(), stop.clone(), step.clone()),
            _ => return Ok(()),
        };
        self.operation_adjustments.borrow_mut().insert(
            source.source_span(),
            mojito_checked::checked::SemanticAdjustment::ComptimeIteration(Box::new(
                mojito_checked::checked::ComptimeIteration {
                    index: index.clone(),
                    start,
                    stop,
                    step,
                },
            )),
        );
        Ok(())
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
            // A reflection handle is a compile-time value, not a type.
            ExprKind::TypeApply { name, .. } if name == "reflect" => None,
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
            // `types[i]`, `r.field_at[i].T`: a reflected field type.
            _ => self.reflected_type_operand(expr)?,
        })
    }

    /// Type a pack element's default construction while the pack is still a
    /// parameter: `Self.Ts[i]()`, or a `def`'s own `Ts[i]()`, is the
    /// dependent element type, and the element must be `Defaultable` by the
    /// pack's bound or a `where` clause. `None` when `pack` (read through
    /// `Self` when `through_self`) names no unbound pack or the call is not a
    /// nullary construction at one index.
    pub(super) fn infer_pack_element_construction(
        &self,
        pack: &str,
        through_self: bool,
        param_args: &[ParamArg],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Option<Result<Ty, TypeError>> {
        let [ParamArg::Value(index)] = param_args else {
            return None;
        };
        if !args.is_empty() || !kwargs.is_empty() {
            return None;
        }
        let (base, spelled) = if through_self {
            let declared = self.self_decls.iter().any(|decl| {
                matches!(decl, ParamDecl::Type { name, variadic: true, .. }
                    if name.trim_start_matches('*') == pack)
            });
            self.pack_reference(pack).filter(|_| declared)?;
            (
                SourceType::SelfParam(pack.to_string()),
                format!("Self.{pack}"),
            )
        } else {
            self.pack_parameter_in_scope(pack)
                .filter(|_| self.lookup(pack).is_none())?;
            (
                SourceType::Named(pack.to_string(), Vec::new()),
                pack.to_string(),
            )
        };
        Some(
            self.ty_from_anno(&SourceType::IndexedProjection {
                base: Box::new(base),
                index: Box::new(index.clone()),
            })
            .and_then(|element| {
                let view = self
                    .opaque_element(&element)
                    .unwrap_or_else(|| element.clone());
                if self.conforms_to(&view, "Defaultable") {
                    Ok(element)
                } else {
                    Err(TypeError::TraitNotSatisfied {
                        param: spelled,
                        ty: element.to_string(),
                        trait_name: "Defaultable".to_string(),
                        reason: self.trait_failure_reason(&view, "Defaultable"),
                    })
                }
            }),
        )
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
            pack_scope(&self.self_decls)
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
        let Ty::Param { binder, .. } = pack else {
            return Err(TypeError::InvariantViolation(format!(
                "'{pack}' is not a variadic pack parameter"
            )));
        };
        let name = &binder.name;
        let list = self
            .pack_reference(name)
            .ok_or_else(|| TypeError::SymbolicBoundary(format!("pack '{name}' is not in scope")))?;
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
        self.forwarded_pack_operand(spread).is_some()
    }

    /// Note a positional collector declared `var *args` once it is bound.
    pub(super) fn record_owned_pack(&mut self, param: &FnParam) {
        if param.kind == mojito_ast::ast::ParamKind::Variadic
            && matches!(param.convention, Some(ArgConvention::Var))
            && let Some(owner) = self.lookup_owner(&param.name)
        {
            self.owned_packs.insert(owner);
        }
    }

    /// Note a function's positional collector as it is declared (`None` when
    /// it has no collector): its parameter name and whether it is `var`.
    /// Overloads of one name that disagree leave the name unrecorded.
    pub(super) fn record_positional_collector(
        &mut self,
        name: &str,
        collector: Option<(&str, bool)>,
    ) {
        let Some((parameter, owned)) = collector else {
            return;
        };
        record_agreed(&mut self.owned_collectors, name, owned);
        record_agreed(&mut self.collector_names, name, parameter.to_string());
    }

    /// A callee with no pack collector cannot take a forwarded pack: the
    /// binding's own diagnostics say why (no collector, or a homogeneous
    /// one).
    pub(super) fn reject_forwarded_pack(
        &self,
        callee: &str,
        args: &[Expr],
        collector: Option<&Ty>,
    ) -> Result<(), TypeError> {
        if let Some((position, forwarded)) =
            self.forwarded_pack_argument(callee, args, collector.is_some())?
            && let Some(collector) = collector
        {
            self.bind_forwarded_pack(callee, &args[position], &forwarded, collector, None)?;
        }
        Ok(())
    }

    /// The pack a call argument forwards whole (`*args`, `*args^`) when that
    /// pack is still a parameter, or `None` for any other argument.
    pub(super) fn forwarded_pack(&self, argument: &Expr) -> Option<ForwardedPack> {
        match &argument.kind {
            ExprKind::Spread(spread) => self.forwarded_pack_operand(spread),
            _ => None,
        }
    }

    /// [`Self::forwarded_pack`] for the spread's operand (`args`, `args^`).
    fn forwarded_pack_operand(&self, spread: &Expr) -> Option<ForwardedPack> {
        let (source, transferred) = match &spread.kind {
            ExprKind::Transfer(inner) => (&**inner, true),
            _ => (spread, false),
        };
        let ExprKind::Identifier(binding) = &source.kind else {
            return None;
        };
        let Some(Ty::VariadicPack(element)) = self.lookup(binding) else {
            return None;
        };
        let element = mojito_types::types::pack_spread(std::slice::from_ref(&**element))?;
        let Ty::Param { binder, .. } = element else {
            return None;
        };
        // The binder in scope carries the declared bounds; a pack reached
        // through a capture keeps the bounds its binding type has.
        let pack = self
            .lookup_tparam(&binder.name)
            .unwrap_or_else(|| element.clone());
        Some(ForwardedPack {
            binding: binding.clone(),
            pack,
            transferred,
            owned: self
                .lookup_owner(binding)
                .is_some_and(|owner| self.owned_packs.contains(&owner)),
        })
    }

    /// The pack a call forwards among its positional arguments, with its
    /// place in the argument list checked: one spread, last, and only where
    /// the callee has a positional collector. `None` when no argument
    /// forwards a pack that is still a parameter.
    pub(super) fn forwarded_pack_argument(
        &self,
        callee: &str,
        args: &[Expr],
        has_collector: bool,
    ) -> Result<Option<(usize, ForwardedPack)>, TypeError> {
        let spreads: Vec<usize> = args
            .iter()
            .enumerate()
            .filter(|(_, argument)| self.forwarded_pack(argument).is_some())
            .map(|(position, _)| position)
            .collect();
        let Some(position) = mojito_ast::call::spread_position(&spreads, args.len())
            .map_err(|error| error.into_type_error(callee))?
        else {
            return Ok(None);
        };
        if !has_collector {
            return Err(mojito_ast::call::MatchError::SpreadOutsideVariadic.into_type_error(callee));
        }
        let forwarded = self
            .forwarded_pack(&args[position])
            .expect("selected among the forwarded arguments");
        Ok(Some((position, forwarded)))
    }

    /// Bind a forwarded pack to a callee's positional collector: the
    /// collector is itself a pack (`*args: *Us`), its ownership agrees with
    /// the caller's (`var` needs the `^`, a read pack cannot be transferred,
    /// and a collector known to be `var` or read — `collector_owned` — takes
    /// the same kind of pack), and every bound it declares holds of the
    /// caller's pack. The whole pack is the collector's one actual, and
    /// `argument` (the spread) is typed as it so a later reading of the
    /// call's arguments finds the binding rather than the bare spread.
    pub(super) fn bind_forwarded_pack(
        &self,
        callee: &str,
        argument: &Expr,
        forwarded: &ForwardedPack,
        collector: &Ty,
        collector_owned: Option<bool>,
    ) -> Result<Ty, TypeError> {
        let Ty::Param {
            bounds: expected, ..
        } = collector
        else {
            return Err(TypeError::TypeMismatch {
                expected: format!("VariadicList[{collector}]"),
                found: format!("a variadic pack ('{}')", forwarded.binding),
                context: format!("argument to '{callee}'"),
            });
        };
        if !forwarded.owned && forwarded.transferred {
            return Err(TypeError::BadCall {
                func: callee.to_string(),
                reason: format!(
                    "cannot transfer out of the read pack '{}'",
                    forwarded.binding
                ),
            });
        }
        if collector_owned.is_some_and(|owned| owned != forwarded.owned) {
            return Err(TypeError::BadCall {
                func: callee.to_string(),
                reason: "cannot unpack a variadic pack into a call that requires a different \
                         ownership"
                    .to_string(),
            });
        }
        if forwarded.owned && !forwarded.transferred {
            return Err(TypeError::ImplicitCopy {
                ty: format!("VariadicPack[{}]", forwarded.pack),
                context: format!("argument to '{callee}'"),
                transferable: true,
                copyable: false,
            });
        }
        if expected
            .iter()
            .any(|bound| !self.conforms_to(&forwarded.pack, bound))
        {
            let declared = match &forwarded.pack {
                Ty::Param { bounds, .. } if !bounds.is_empty() => bounds.join(" & "),
                _ => "AnyType".to_string(),
            };
            return Err(TypeError::BadCall {
                func: callee.to_string(),
                reason: format!(
                    "cannot unpack a pack of type '{declared}' into a call that expects a pack of \
                     type '{}'",
                    expected.join(" & ")
                ),
            });
        }
        self.expression_types
            .borrow_mut()
            .insert(argument.source_span(), forwarded.pack.clone());
        Ok(forwarded.pack.clone())
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
            TypeError::SymbolicBoundary(format!(
                "'{name}' called with a spread of the unbound pack '{binding}'"
            ))
        };
        let constructed = match name {
            "__RuntimeTuple" => Ty::Tuple(vec![pack]),
            mojito_types::types::TUPLE_TYPE_NAME => mojito_types::types::tuple_type(vec![pack]),
            // Any other callee binds the forwarded pack to its own collector
            // (`forwarded_pack_argument`, `bind_forwarded_pack`).
            _ => return None,
        };
        if !kwargs.is_empty() {
            return Some(Err(boundary()));
        }
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

    /// A public `Tuple` constructed inside a validated body types as the
    /// tuple it spells — a bare `Tuple(1, "one")` as the display it is, an
    /// explicit `Tuple[Int, String](1, "one")` against its element list —
    /// because its checked identity is the element-by-element nominal
    /// spelling, not the template's bound pack. Every other variadic struct
    /// (`Pair[Int, Bool](1, True)`) is matched against its template's
    /// constructor with the pack bound from the `[...]` arguments, the path
    /// a bare construction takes once its pack is solved. `None` outside
    /// validation and for every callee this does not type.
    pub(super) fn infer_validated_variadic_construction(
        &self,
        call: &Expr,
        name: &str,
        param_args: &[ParamArg],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Option<Result<Ty, TypeError>> {
        if !self.source_validation
            || self.lookup(name).is_some()
            || !kwargs.is_empty()
            || !self.structs.contains_key(name)
            || (name != mojito_types::types::TUPLE_TYPE_NAME
                && name != mojito_types::types::TSTRING_TYPE_NAME)
        {
            return None;
        }
        if !param_args.is_empty() {
            return Some(self.infer_tuple_construction(param_args, args));
        }
        let mut display = Expr::new(ExprKind::TupleLit(args.to_vec()), call.span);
        display.source.clone_from(&call.source);
        Some(self.infer(&display))
    }

    /// Close the pack elements a callee's type names (`Self.Ts[index]`)
    /// under a use's arguments: a value argument binds its parameter, a
    /// concrete pack its list, and a pack forwarded as a spread the caller's
    /// own pack. A type with no pack element is returned as it is. `scopes`
    /// pair declaration lists with their arguments; a later scope's binder
    /// shadows an earlier one's of the same spelling, the key the dependent
    /// type's value environment reads.
    pub(super) fn close_pack_elements(&self, ty: Ty, scopes: &[(&[ParamDecl], &[TyArg])]) -> Ty {
        let names_element =
            |ty: &Ty| matches!(ty, Ty::Dependent(dependent) if dependent.pack_element().is_some());
        if !mojito_types::types::mentions(&ty, &names_element) {
            return ty;
        }
        let values: HashMap<String, CtValue> = scopes
            .iter()
            .flat_map(|(decls, arguments)| decls.iter().zip(arguments.iter()))
            .filter_map(|(decl, argument)| {
                let name = decl.name().trim_start_matches('*').to_string();
                match argument {
                    TyArg::Val(value) => Some((name, value.clone())),
                    TyArg::Ty(Ty::Param { binder, .. }) if binder.name.starts_with('*') => self
                        .pack_reference(&binder.name)
                        .map(|reference| (name, CtValue::Expr(reference))),
                    TyArg::Ty(_) | TyArg::Origin(_) => None,
                }
            })
            .collect();
        self.resolve_dependent_ty(&ty, &values).unwrap_or(ty)
    }

    /// The bounded type parameter an element of an unbound variadic pack, or
    /// a reflected field type of a symbolic subject, behaves as: the pack's
    /// declared bounds (a field type has none), plus every trait a `where`
    /// clause guarantees of the pack's elements or a `comptime if
    /// conforms_to` arm proves of this element. The dependent type stays the
    /// element's identity; this is what its capabilities are read from.
    pub(super) fn opaque_element(&self, ty: &Ty) -> Option<Ty> {
        let Ty::Dependent(dependent) = ty else {
            return None;
        };
        let reflected = |node: &ParamExpr| {
            matches!(
                node.kind(),
                mojito_types::param_expr::ParamKind::Reflect { .. }
            )
        };
        let list = dependent.pack_element().map(|(list, _)| list);
        let pack = list.and_then(ParamExpr::as_decl_ref);
        if pack.is_none() && !reflected(dependent.expr()) && !list.is_some_and(reflected) {
            return None;
        }
        let pack_name = pack.map(|reference| reference.name.to_string());
        let declared = pack_name.as_deref().and_then(|pack| {
            self.lookup_tparam(&format!("*{pack}")).or_else(|| {
                self.self_decls
                    .iter()
                    .filter(|decl| {
                        matches!(decl, ParamDecl::Type { variadic: true, .. })
                            && decl.name().trim_start_matches('*') == pack
                    })
                    .find_map(type_parameter)
            })
        });
        let mut bounds = match declared {
            Some(Ty::Param { bounds, .. }) => bounds,
            _ => Vec::new(),
        };
        let spelled = dependent.expr().to_string();
        for (subject, guaranteed) in self.assumed_conformances.iter().flatten() {
            let names_element = match subject {
                AssumedSubject::Element(element) => element == dependent.expr(),
                AssumedSubject::Binder(id) => pack.is_some_and(|reference| &reference.id == id),
            };
            if names_element && !bounds.contains(guaranteed) {
                bounds.push(guaranteed.clone());
            }
        }
        // Two packs may share a spelling (`Tuple.Ts`, `Bag.Ts`); a view's name
        // is its element's alone, so the owner qualifies the later one.
        let mut views = self.pack_element_views.borrow_mut();
        let name = match (views.get(&spelled), pack) {
            (Some(viewed), Some(reference)) if viewed != ty => {
                format!("{}.{spelled}", reference.id.owner)
            }
            _ => spelled,
        };
        views.insert(name.clone(), ty.clone());
        Some(Ty::Param {
            binder: pack_element_view_binder(&name),
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
        let elements: TySubst = views
            .iter()
            .map(|(name, element)| (pack_element_view_binder(name).id, element.clone()))
            .collect();
        substitute(&ty, &elements)
    }

    /// Whether a validation error marks the validator's own blind spot
    /// rather than a verdict: a constructor, method, or operator of a struct
    /// registered only as a template shell (a struct-value-keyed struct),
    /// whose members exist only per specialization. The executable check
    /// still covers the arm elaboration selects.
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
    /// template shell (a struct-value-keyed struct under source validation).
    /// Its iteration and subscript contracts come from the family, not from
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
            ConstraintOperand::Param(param) | ConstraintOperand::PackLength(param) => {
                self.comptime_name_resolves(&param.name)
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
            | GenericConstraint::PackPredicate { param, .. } => {
                self.comptime_name_resolves(&param.name)
            }
            GenericConstraint::PackContains { param, element } => {
                self.comptime_name_resolves(&param.name)?;
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
    // A module alias of a vector type (`comptime U256 = SIMD[...]`) types a
    // vector value parameter as the spelled `SIMD` does.
    let vector_aliases: HashSet<&str> = stmts
        .iter()
        .filter_map(|statement| match &statement.kind {
            StmtKind::Comptime {
                name,
                type_params,
                value,
                ..
            } if type_params.is_empty()
                && matches!(&value.kind, ExprKind::TypeApply { name, .. } if name == "SIMD") =>
            {
                Some(name.as_str())
            }
            _ => None,
        })
        .collect();
    let value_keyed = |type_params: &[mojito_ast::ast::TypeParam]| {
        let keyed = |parameter: &mojito_ast::ast::TypeParam| {
            matches!(parameter.bounds.as_slice(), [only]
                if only == "DType" || vector_aliases.contains(only.as_str()))
                || matches!(&parameter.value_type,
                    Some(mojito_ast::ast::Type::Named(name, _)) if name == "SIMD")
        };
        let scalar = |parameter: &mojito_ast::ast::TypeParam| {
            matches!(parameter.bounds.as_slice(), [only]
                if matches!(only.as_str(), "Int" | "UInt" | "Bool" | "Float64"))
        };
        type_params.iter().any(keyed)
            && type_params
                .iter()
                .all(|parameter| keyed(parameter) || scalar(parameter))
    };
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
                let class = if value_keyed_def(statement) {
                    TemplateClass::ValidatedKeyed
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
                    struct_valued_template(type_params, &|bound| declared_structs.contains(bound));
                for method in methods {
                    let class = if shell {
                        TemplateClass::ConcreteOnly
                    } else if value_keyed(type_params)
                        || mojito_ast::simd_width::method_constructs_at_own_lane(method)
                    {
                        TemplateClass::ValidatedKeyed
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

/// A struct's arguments as its declarations bind them: a variadic struct
/// applied element by element binds its lone pack to the whole list (see
/// [`positional_pack_binding`]); any other argument list binds position by
/// position as it is.
pub(super) fn positional_pack_arguments<'a>(
    decls: &[ParamDecl],
    arguments: &'a [TyArg],
) -> std::borrow::Cow<'a, [TyArg]> {
    positional_pack_binding(decls, arguments)
        .map_or(std::borrow::Cow::Borrowed(arguments), |(_, pack)| {
            std::borrow::Cow::Owned(vec![pack])
        })
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

/// Whether a struct declaration checks only per specialization, so source
/// validation registers it as a template shell: one with a struct-typed
/// value parameter (`is_value_struct` names the declared structs), a shape
/// the elaborator monomorphizes per application and the checker has no
/// symbolic form for. A `DType` or vector-width parameter is not one (a
/// `Ty::Simd` slot may be symbolic), nor is a variadic pack (its element has
/// a dependent type).
pub(super) fn struct_valued_template(
    type_params: &[mojito_ast::ast::TypeParam],
    is_value_struct: &dyn Fn(&str) -> bool,
) -> bool {
    type_params
        .iter()
        .any(|parameter| matches!(parameter.bounds.as_slice(), [only] if is_value_struct(only)))
}

/// Whether source validation checks a declaration's body: one holding
/// compile-time control flow, a `rebind` over the declaration's own
/// parameters (`keys_rebind`, from `rebind::rebind_keyed_bodies`), keyed
/// on a variadic pack — the declaration's own, or that of the struct
/// `enclosing` it — or keyed on the wildcard vector parameter a `Hasher`'s
/// `_update_with_simd(mut self, value: SIMD[_, _])` desugars to
/// (`simd_wildcard_binder`, checked over a lane-shaped view of the
/// parameter) — each leaves the template stubbed, so validation is the
/// only check it gets.
pub(super) fn validates_body(
    enclosing: &[mojito_ast::ast::TypeParam],
    type_params: &[mojito_ast::ast::TypeParam],
    body: &[Stmt],
    keys_rebind: bool,
) -> bool {
    block_has_comptime(body)
        || keys_rebind
        || is_variadic_template(type_params)
        || is_variadic_template(enclosing)
        || type_params.iter().any(simd_wildcard_binder)
}

/// Whether a module-level `def` keys a lane on a `DType` binder of its own
/// or uses a parameter as a lane width (`Scalar[dt](v)`, `SIMD[DType.int32,
/// w]`): the elaborator specializes such a def per call and drops its
/// template, so source validation checks its body with the parameters
/// symbolic.
pub(super) fn value_keyed_def(statement: &Stmt) -> bool {
    matches!(&statement.kind, StmtKind::Def { type_params, .. }
    if type_params.iter().any(|parameter| {
        matches!(parameter.bounds.as_slice(), [only] if only == "DType")
    })) || mojito_ast::simd_width::def_uses_layout_dependent_param(statement)
}

/// Whether a block holds a `comptime if`/`comptime for` anywhere below it,
/// nested function bodies included.
pub(super) fn block_has_comptime(stmts: &[Stmt]) -> bool {
    stmts.iter().any(stmt_has_comptime)
}

/// Whether a block names `reflect[...]` anywhere below it. Such a body is
/// validated symbolically like any other, but its instances keep the clone
/// check: the field facts the elaborator evaluates are its own
/// (`template_facts/certificate.rs:template_certificate`).
pub(super) fn reads_reflection(stmts: &[Stmt]) -> bool {
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

/// Whether a struct's binders are all compile-time values and one is a
/// `DType` or a vector (`_SequentialRange[dtype: DType]`,
/// `AHasher[key: U256]`), or one is a lane width (`Width[n: Int]` spelling
/// `SIMD[dt, Self.n]`): the elaborator specializes such a struct whole per
/// value and drops its template, so source validation checks every method
/// body with the values symbolic.
fn value_keyed_struct(decls: &[ParamDecl], declaration: &StructDeclaration<'_>) -> bool {
    let values_only = decls.iter().all(|decl| {
        matches!(
            decl,
            ParamDecl::Value {
                variadic: false,
                ..
            }
        )
    });
    values_only
        && (decls.iter().any(|decl| {
            matches!(decl, ParamDecl::Value { ty, .. } if matches!(**ty, Ty::Dtype | Ty::Simd { .. }))
        }) || mojito_ast::simd_width::struct_members_use_layout_dependent_param(
            declaration.type_params,
            declaration.fields,
            declaration.methods,
        ))
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

/// The binder of a bounded pack-element view ([`Checker::opaque_element`]):
/// the view's qualified spelling is its whole identity, so
/// [`Checker::restore_pack_elements`] finds it again.
fn pack_element_view_binder(name: &str) -> ParamRef {
    ParamRef {
        id: ParamId::new(&format!("$view:{name}"), 0),
        name: name.into(),
    }
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
    value_binder_expr(ParamId::new(&owner, 0), var, &Ty::Int)
}

/// Record `value` for `name` unless a different value is already recorded,
/// in which case the disagreement leaves `name` unrecorded.
fn record_agreed<V: PartialEq>(recorded: &mut HashMap<String, V>, name: &str, value: V) {
    match recorded.get(name) {
        Some(known) if *known != value => {
            recorded.remove(name);
        }
        Some(_) => {}
        None => {
            recorded.insert(name.to_string(), value);
        }
    }
}
