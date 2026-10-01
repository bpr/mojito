//! Certificate grammar for control forms: `raise`, nested `def`, `with`,
//! runtime `for`, tuple unpacking, and comprehensions.

use super::{BodyShape, LocalKind, closed_scalar, fact_at, grammar_scalar};
use mojito_ast::ast::{Expr, ExprKind, Stmt, StmtKind};
use mojito_checked::templates::{MethodFeatures, OccurrenceId};
use mojito_types::types::Ty;

impl BodyShape<'_> {
    /// The compiler-private trap `_mojito_abort("message")`, as a statement.
    /// The built-in types its literal and selects nothing, so it records no
    /// parameters and no binding; a declaration of that name would record
    /// both, and is not this.
    pub(super) fn abort(&self, expr: &Expr) -> bool {
        let id = self.occurrence(expr);
        let admitted = matches!(&expr.kind, ExprKind::Call { name, param_args, args, kwargs }
            if name == "_mojito_abort"
                && param_args.is_empty()
                && kwargs.is_empty()
                && matches!(args.as_slice(), [message] if matches!(message.kind, ExprKind::Str(_))))
            && self.facts.is_none_or(|facts| {
                fact_at(&facts.call_parameters, id).is_none()
                    && fact_at(&facts.expression_bindings, id).is_none()
            });
        admitted && self.holds(MethodFeatures::STATEMENTS)
    }

    /// The operand of a `raise`: a construction, or `Error` made from a
    /// string literal or from a whole value of a closed type
    /// (`Error(String("…") + fspath)`).
    ///
    /// `require_error` asks whether the operand is a string, which a
    /// constructed struct's name and the builtin `Error` settle, and whether
    /// its type is the declared error type. Both types are functions of the
    /// same parameters, so the instance's answer is the template's.
    pub(super) fn raised(&self, value: &Expr) -> bool {
        let id = self.occurrence(value);
        let message = |message: &Expr| {
            matches!(message.kind, ExprKind::Str(_))
                || (!self.keyed && self.whole_value(message) && self.closed(message))
        };
        let error = matches!(&value.kind, ExprKind::Call { name, param_args, args, kwargs }
            if name == "Error"
                && !self.structs.contains_key(name)
                && param_args.is_empty()
                && kwargs.is_empty()
                && matches!(args.as_slice(), [argument] if message(argument)))
            && self.facts.is_none_or(|facts| {
                fact_at(&facts.expression_types, id) == Some(&Ty::Error)
                    && fact_at(&facts.call_parameters, id).is_none()
                    && fact_at(&facts.expression_bindings, id).is_none()
            });
        let literal = matches!(value.kind, ExprKind::Str(_))
            && self.facts.is_none_or(|facts| {
                fact_at(&facts.expression_types, id) == Some(&Ty::StringLiteral)
            });
        let reraised = matches!(&value.kind, ExprKind::Identifier(name)
            if self.error_binders.borrow().contains(name))
            && self
                .facts
                .is_none_or(|facts| fact_at(&facts.expression_types, id) == Some(&Ty::Error));
        error || literal || reraised || self.construction(value)
    }

    /// A nested `def` (`NESTED_DEFS`), at any depth.
    ///
    /// It declares no compile-time parameters, decorators, or `where`
    /// clause, and takes regular parameters, each read, `mut`, `var`, `out`,
    /// or a bare `ref`, with at most a closed-scalar, string-literal, or
    /// `None` default, or one constructing a declared struct
    /// ([`Self::construction`]), which its lowered default function runs.
    /// A raising one, bare or typed, keeps its effect in the
    /// recipe, and an `out` one is a local of its result type. Its recorded parameter
    /// and result types substitute in the recipe, where they may mention the
    /// struct's parameters, and each parameter's deletability is judged at
    /// the instance's type. Each capture, listed or reached through a
    /// capture-all default, names a local, a parameter, or `self`, by any
    /// convention, which an instance maps to its own binding; an owned one
    /// owes its capability again at the instance's type. Its body is judged
    /// in place with its parameters as locals: a `ref` one of a whole value
    /// a handle, a `mut` or `var` one of a whole value a `var` local, and
    /// any other read where it lies, a read one holding a struct as a
    /// method's parameter is ([`Self::parameter_receiver`]). It returns a closed scalar or a whole
    /// value, and its name is a local the body may only call.
    pub(super) fn nested_def(&self, statement: &Stmt) -> bool {
        use mojito_ast::ast::ArgConvention;
        let StmtKind::Def {
            name,
            decorators,
            type_params,
            params,
            positional_only,
            keyword_only,
            captures,
            where_clauses,
            body,
            ..
        } = &statement.kind
        else {
            return false;
        };
        let declaration = !mojito_ast::ast::has_body_decorator(decorators)
            && type_params.is_empty()
            && positional_only.is_none()
            && keyword_only.is_none()
            && where_clauses.is_empty()
            && params.iter().all(|parameter| {
                parameter.kind == mojito_ast::ast::ParamKind::Regular
                    && matches!(
                        parameter.convention,
                        None | Some(
                            ArgConvention::Imm
                                | ArgConvention::Mut
                                | ArgConvention::Var
                                | ArgConvention::Out
                                | ArgConvention::Ref
                        )
                    )
                    && parameter.origin.is_none()
                    && parameter.default.as_ref().is_none_or(|default| {
                        (self.expression(default) && self.scalar(default))
                            || matches!(default.kind, ExprKind::Str(_) | ExprKind::None)
                            || self.construction(default)
                    })
            });
        let captured = captures.as_ref().is_none_or(|list| {
            list.entries.iter().all(|capture| {
                self.declared(&capture.name)
                    || self.params.contains(&capture.name.as_str())
                    || (self.receiver && capture.name == "self")
            })
        });
        let recipe = self
            .facts
            .map(|facts| fact_at(&facts.nested_defs, self.occurrence_of(statement)));
        if !declaration || !captured || recipe.is_some_and(|recipe| recipe.is_none()) {
            return false;
        }
        let scope = self.locals.borrow().len();
        let read_scope = self.nested_params.borrow().len();
        let kinds: Vec<_> = params
            .iter()
            .enumerate()
            .map(|(index, parameter)| {
                let whole = recipe.flatten().is_some_and(|recipe| {
                    recipe
                        .param_types
                        .get(index)
                        .is_some_and(|ty| !grammar_scalar(ty))
                });
                let kind = match parameter.convention {
                    Some(ArgConvention::Ref) if whole => LocalKind::Reference,
                    Some(ArgConvention::Mut | ArgConvention::Var | ArgConvention::Out) if whole => {
                        LocalKind::Value
                    }
                    None | Some(ArgConvention::Imm) if whole => {
                        self.nested_params.borrow_mut().push(scope + index);
                        LocalKind::Scalar
                    }
                    _ => LocalKind::Scalar,
                };
                (parameter.name.clone(), kind)
            })
            .collect();
        self.locals.borrow_mut().extend(kinds);
        self.nested_depth.set(self.nested_depth.get() + 1);
        let admitted = self.block(body);
        self.nested_depth.set(self.nested_depth.get() - 1);
        self.locals.borrow_mut().truncate(scope);
        self.nested_params.borrow_mut().truncate(read_scope);
        self.locals
            .borrow_mut()
            .push((name.clone(), LocalKind::Callable));
        admitted
            && self.holds(MethodFeatures::STATEMENTS)
            && self.holds(MethodFeatures::NESTED_DEFS)
    }

    /// A `with` statement. Before capture it is judged from its syntax: each
    /// context a whole value, each `as` name a local scoped to the block,
    /// and the block. After capture the desugar the check recorded stands
    /// in its place, judged as the body's own statements are: the manager a
    /// `var` local, its `__enter__` and `__exit__` sibling calls, the
    /// guarding `try`, and the liveness anchor. Its form is the manager
    /// struct's declaration, which an instance builds its own desugar from
    /// (`Checker::instance_with_desugars`).
    pub(super) fn with_statement(
        &self,
        statement: &Stmt,
        items: &[mojito_ast::ast::WithItem],
        body: &[Stmt],
    ) -> bool {
        let Some(facts) = self.facts else {
            let scope = self.locals.borrow().len();
            let admitted = items.iter().all(|item| {
                let context = self.whole_value(&item.context);
                if let Some(name) = &item.var {
                    let kind = self.entered_kind(&item.context);
                    self.locals.borrow_mut().push((name.clone(), kind));
                }
                context
            }) && self.block(body);
            self.locals.borrow_mut().truncate(scope);
            return admitted;
        };
        let Some(desugar) = self.desugars.get(&statement.source_span()) else {
            return false;
        };
        if fact_at(&facts.with_forms, self.occurrence_of(statement)) != Some(&desugar.form) {
            return false;
        }
        self.desugar_depth.set(self.desugar_depth.get() + 1);
        let admitted = self.block(&desugar.statements);
        self.desugar_depth.set(self.desugar_depth.get() - 1);
        admitted
    }

    /// The `with` desugar's liveness anchor, `_mojito_keep_alive(name)` of
    /// a local: it selects no callee and records only the local's read.
    pub(super) fn keep_alive(&self, expr: &Expr) -> bool {
        let id = self.occurrence(expr);
        matches!(&expr.kind, ExprKind::Call { name, param_args, args, kwargs }
            if name == crate::checker::with_stmt::KEEP_ALIVE_BUILTIN
                && param_args.is_empty()
                && kwargs.is_empty()
                && matches!(args.as_slice(), [local]
                    if matches!(&local.kind, ExprKind::Identifier(name) if self.declared(name))))
            && self.facts.is_none_or(|facts| {
                fact_at(&facts.call_parameters, id).is_none()
                    && fact_at(&facts.expression_bindings, id).is_none()
            })
    }

    /// Whether `expr` names the innermost `comptime for` variable.
    pub(super) fn at_loop(&self, expr: &Expr) -> bool {
        matches!(&expr.kind, ExprKind::Identifier(name)
            if self.loop_vars.borrow().last() == Some(name))
    }

    /// The iterable of a runtime `for`: a place the loop borrows (`self`, a
    /// field of it, a parameter, a local), the `^` transfer of a place the
    /// body owns, a sibling call's result, or a direct call's (`range(n)`). The loop records its protocol
    /// at the iterable, which a recipe must then hold
    /// ([`Checker::realize_iterations`]).
    pub(super) fn iterable(&self, iter: &Expr) -> bool {
        let admitted = match &iter.kind {
            ExprKind::Transfer(_) => self.whole_value(iter),
            ExprKind::Identifier(name) => {
                self.receiver_itself(iter)
                    || self.params.contains(&name.as_str())
                    || self.local_kind(name).is_some()
            }
            ExprKind::Member { .. } => self.receiver_field(iter) || self.reference_member(iter),
            ExprKind::Call { .. } => self.expression(iter),
            _ => self.call_result(iter),
        };
        admitted
            && self
                .facts
                .is_none_or(|facts| fact_at(&facts.iterations, self.occurrence(iter)).is_some())
    }

    /// A tuple unpacked from a parameter, a local, a field of `self`, or a
    /// sibling call's result, into `_` and `var` locals: each declared by the
    /// statement, or declared before it.
    ///
    /// The element reads are synthesized from the value's type and place,
    /// which an instance derives them from again
    /// ([`Checker::realize_tuple_unpacks`]); the value records no conversion
    /// or adjustment an instance could not repeat. A declared target is a
    /// scalar local where its recorded binding type is a closed scalar, and a
    /// whole value otherwise, which only a method that may move whole values
    /// holds.
    pub(super) fn tuple_unpack(&self, targets: &[Expr], value: &Expr, declares: bool) -> bool {
        let id = self.occurrence(value);
        let source = match &value.kind {
            ExprKind::Identifier(name) => {
                self.params.contains(&name.as_str()) || self.declared(name)
            }
            ExprKind::Member { .. } => self.receiver_field(value),
            ExprKind::Call { param_args, .. } if param_args.is_empty() => self.expression(value),
            _ => self.call_result(value),
        };
        let recorded = self.facts.is_none_or(|facts| {
            fact_at(&facts.tuple_unpacks, id).is_some()
                && fact_at(&facts.conversions, id).is_none()
                && fact_at(&facts.operation_adjustments, id).is_none()
        });
        let bound = targets.iter().all(|target| match &target.kind {
            ExprKind::Identifier(name) if name == "_" => true,
            ExprKind::Identifier(name) if declares => {
                let kind = self.target_local(target);
                self.locals.borrow_mut().push((name.clone(), kind));
                kind == LocalKind::Scalar || self.moved_result.is_some()
            }
            ExprKind::Identifier(name) => self.declared(name),
            _ => false,
        });
        source && recorded && bound && self.holds(MethodFeatures::TUPLE_UNPACKS)
    }

    /// What a loop variable is, from the binding type its loop recorded: a
    /// handle where the loop binds a reference, a scalar local where a
    /// closed scalar, and a whole value otherwise. With no facts yet, the
    /// syntax alone never rules a use out.
    pub(super) fn loop_local(&self, statement: &Stmt) -> LocalKind {
        let declaration = OccurrenceId {
            syntax: self.origins.origin(statement.syntax_id),
            copy: 0,
        };
        self.facts.map_or(LocalKind::Scalar, |facts| {
            // A temporary source leaves the reference's declared origin
            // unrooted, so its type stays among the plain binding types.
            let binding = fact_at(&facts.binding_types, declaration);
            if fact_at(&facts.reference_binding_types, declaration).is_some()
                || matches!(binding, Some(Ty::Ref(_)))
            {
                LocalKind::Reference
            } else if binding.is_some_and(grammar_scalar) {
                LocalKind::Scalar
            } else {
                LocalKind::Value
            }
        })
    }

    /// A list, set, or dict comprehension: a temporary of the collection
    /// type, built through its insert method (`ConstructCollection`).
    ///
    /// Each generator clause iterates what a runtime `for` may, and records
    /// its protocol at the iterable ([`Self::iterable`]). Its binder is a
    /// local scoped to the clauses after it and to the produced elements, of
    /// the kind its recorded binding type makes it, and is declared from the
    /// protocol's binding plan, which an instance selects again
    /// ([`Checker::install_comprehension_bindings`]). A filter is a runtime
    /// condition, and each produced key or value a closed scalar or a whole
    /// value the collection consumes.
    pub(super) fn comprehension(&self, expr: &Expr) -> bool {
        let ExprKind::Comprehension {
            key,
            value,
            clauses,
            ..
        } = &expr.kind
        else {
            return false;
        };
        let binders = self
            .facts
            .map(|facts| fact_at(&facts.comprehension_bindings, self.occurrence(expr)));
        if self.keyed || binders.is_some_and(|binders| binders.is_none()) {
            return false;
        }
        let kind = |index: usize| {
            binders
                .flatten()
                .map_or(Some(LocalKind::Scalar), |binders| {
                    binders.get(index).map(|binder| {
                        if binder.reference {
                            LocalKind::Reference
                        } else if grammar_scalar(&binder.ty) {
                            LocalKind::Scalar
                        } else {
                            LocalKind::Value
                        }
                    })
                })
        };
        let element = |element: &Expr| {
            (self.expression(element) && self.scalar(element)) || self.whole_value(element)
        };
        let scope = self.locals.borrow().len();
        let mut declared = 0;
        let admitted = clauses.iter().all(|clause| match clause {
            mojito_ast::ast::ComprehensionClause::For { var, iter, .. } => {
                let iterable = self.iterable(iter);
                let binder = kind(declared);
                declared += 1;
                binder.is_some_and(|binder| {
                    self.locals.borrow_mut().push((var.clone(), binder));
                    iterable
                })
            }
            mojito_ast::ast::ComprehensionClause::If(condition) => self.condition(condition),
        }) && key.as_deref().is_none_or(element)
            && element(value);
        self.locals.borrow_mut().truncate(scope);
        admitted && self.holds(MethodFeatures::COMPREHENSIONS)
    }

    /// The kind of local a `with` binds its context's `__enter__` result
    /// to, read from the manager struct's declaration when the context
    /// constructs one: the facts that decide it are not captured yet.
    fn entered_kind(&self, context: &Expr) -> LocalKind {
        let scalar = matches!(&context.kind, ExprKind::Call { name, .. }
            if self.structs.get(name).and_then(|info| info.methods.get("__enter__"))
                .is_some_and(|sigs| sigs.iter().any(|sig| sig.has_self && closed_scalar(&sig.ret))));
        if scalar {
            LocalKind::Scalar
        } else {
            LocalKind::Value
        }
    }

    /// What a local an unpacking declares is, from the binding type recorded
    /// at its target: a scalar local where a closed scalar, and a whole value
    /// otherwise. With no facts yet, the syntax alone never rules a use out.
    fn target_local(&self, target: &Expr) -> LocalKind {
        let id = self.occurrence(target);
        self.facts.map_or(LocalKind::Scalar, |facts| {
            if fact_at(&facts.binding_types, id).is_some_and(grammar_scalar) {
                LocalKind::Scalar
            } else {
                LocalKind::Value
            }
        })
    }
}
