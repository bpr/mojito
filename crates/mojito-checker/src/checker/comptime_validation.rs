//! Source validation of compile-time control flow (`validate_comptime_templates`):
//! a `comptime if` condition is typed as a compile-time `Bool`, every arm and
//! `comptime for` body is checked in its own scope with the declaration's
//! parameters symbolic, and function-local `comptime` bindings the elaborator
//! would otherwise consume are bound here. Extracted from `checker.rs`; see
//! `docs/symbol-map.md`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_ast::ast::{ParamArg, SourceType};
use mojito_types::types::{ConstraintOperand, GenericConstraint};

/// A local `comptime` binding of a display the elaborator evaluates per
/// instance (`comptime L = [n, n + 1]`).
#[derive(Debug, Clone)]
pub(in crate::checker) struct BoundDisplay {
    /// The display's span, which holds its
    /// `SemanticAdjustment::ComptimeDisplay`.
    span: SourceSpan,
    /// What a `comptime for` over the binding binds.
    element: Ty,
    /// The display's own type, which a runtime read would materialize.
    ty: Ty,
    /// What the binding denotes in a type or a parameter argument: the
    /// application of the function MIR lifts for the display to the binders
    /// in scope, a list of `element`s.
    sequence: ParamExpr,
    /// Whether a subscript reads an element by position, as a list
    /// display's does.
    positional: bool,
}

impl BoundDisplay {
    /// The parameter expression of element `index` of the binding
    /// (`L[0]`), for a display a subscript indexes by position.
    pub(in crate::checker) fn element(
        &self,
        context: &ParamContext,
        index: &ParamExpr,
    ) -> Option<Result<ParamExpr, TypeError>> {
        self.positional.then(|| {
            context
                .list_get(&self.sequence, index)
                .map_err(super::annotations::param_error)
        })
    }

    /// The parameter expression of the binding's length (`len(L)`).
    pub(in crate::checker) fn length(
        &self,
        context: &ParamContext,
    ) -> Result<ParamExpr, TypeError> {
        context
            .list_length(&self.sequence)
            .map_err(super::annotations::param_error)
    }
}

/// A compile-time expression the check named a lifted function for
/// ([`Checker::lifted_application`]), with what tells it from another
/// occurrence of the same syntax: the binders it is applied to and the
/// bindings its names resolve to.
#[derive(Debug)]
pub(in crate::checker) struct LiftedApplication {
    expression: Expr,
    binders: Vec<ParamExpr>,
    bindings: Vec<Option<mojito_types::origin::OwnerId>>,
    validated: bool,
    application: ParamExpr,
}

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

/// A callable a compile-time call applies by name
/// ([`Checker::called_application`]): a module `def` or a static method of a
/// struct, whose own compile-time parameters and runtime parameters are each
/// an `Int` or a `Bool`, and which returns one.
#[derive(Debug, Clone)]
pub(in crate::checker) struct ApplicableFunction {
    /// The MIR declaration the elaborator runs.
    symbol: String,
    /// Whether it is a static method of a generic struct, which is applied
    /// to the instance it is called on before any other argument.
    on_instance: bool,
    /// The callable's own compile-time parameters, in declared order.
    binders: Vec<(String, Ty)>,
    /// Its runtime parameters, in declared order.
    params: Vec<ApplicableParameter>,
    positional_only: Option<usize>,
    keyword_only: Option<usize>,
    result: Ty,
    /// Whether it is declared `raises`, which no type or parameter argument
    /// may call.
    raises: bool,
}

/// The callables a compile-time call applies by name
/// ([`Checker::called_application`]), every overload under the name a call
/// spells: `h` for a module `def`, `S.f` for a static method.
pub(super) fn applicable_functions(
    stmts: &[Stmt],
    overloads: &mojito_symbol::symbol::OverloadSets,
) -> HashMap<String, Vec<ApplicableFunction>> {
    let mut functions = HashMap::<String, Vec<ApplicableFunction>>::new();
    for statement in stmts {
        match &statement.kind {
            StmtKind::Def {
                name,
                type_params,
                params,
                positional_only,
                keyword_only,
                captures: None,
                raises,
                ret: Some(ret),
                where_clauses,
                ..
            } if where_clauses.is_empty() => {
                let shape = ApplicableShape {
                    type_params,
                    params,
                    positional_only: *positional_only,
                    keyword_only: *keyword_only,
                    raises: *raises,
                    ret,
                };
                let symbol =
                    mojito_symbol::symbol::lowered_def_name(name, type_params, params, overloads);
                functions
                    .entry(name.clone())
                    .or_default()
                    .extend(shape.applicable(symbol, false));
            }
            StmtKind::Struct {
                name,
                type_params,
                methods,
                ..
            } => {
                for method in methods {
                    let Some(ret) = method
                        .ret
                        .as_ref()
                        .filter(|_| !method.has_self && method.where_clauses.is_empty())
                    else {
                        continue;
                    };
                    let shape = ApplicableShape {
                        type_params: &method.type_params,
                        params: &method.params,
                        positional_only: method.positional_only,
                        keyword_only: method.keyword_only,
                        raises: method.raises,
                        ret,
                    };
                    let source = format!("{name}.{}", method.name);
                    let symbol = mojito_symbol::symbol::lowered_method_name(
                        &source,
                        type_params,
                        mojito_symbol::symbol::MethodShape::of(method),
                        overloads,
                    );
                    functions
                        .entry(source)
                        .or_default()
                        .extend(shape.applicable(symbol, !type_params.is_empty()));
                }
            }
            _ => {}
        }
    }
    functions
}

impl Checker {
    /// The calls and method calls of `program` this validation typed as a
    /// scalar a `comptime for` binder takes (`Int`, `Bool`, `Float64`, a
    /// string) without raising, by the syntax identity the elaborated
    /// program keeps. A node copied more than once (a trait default per
    /// conformer) qualifies only when every copy did.
    pub(super) fn scalar_calls(&self, program: &[Stmt]) -> HashSet<mojito_common::token::SyntaxId> {
        self.typed_display_elements(program, false, &|ty| match ty {
            Ty::Int | Ty::Bool | Ty::Float64 | Ty::StringLiteral => true,
            Ty::Struct(name, args) => {
                args.is_empty() && mojito_types::types::is_stdlib_string_struct(name)
            }
            _ => false,
        })
    }

    /// The tuple displays, calls, and method calls of `program` this
    /// validation typed as a parameter aggregate a `comptime for` binder takes
    /// ([`Self::parameter_aggregate_element`]) without raising, under the
    /// verdict rule of [`Self::scalar_calls`].
    pub(super) fn aggregate_elements(
        &self,
        program: &[Stmt],
    ) -> HashSet<mojito_common::token::SyntaxId> {
        self.typed_display_elements(program, true, &|ty| self.parameter_aggregate_element(ty))
    }

    /// Check the method bodies of a struct that hold compile-time control
    /// flow, each with `self` bound at the struct's own parameters, and every
    /// body constructing a vector at a lane its method's own binders spell
    /// (`method_constructs_at_own_lane`), whose elaborated template is a
    /// trap stub.
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
            let validated = validates_body(
                declaration.type_params,
                &m.type_params,
                &m.body,
                body_keys_rebind(&m.body, &self.rebind_keyed_bodies),
            );
            let own_lane = mojito_ast::simd_width::method_constructs_at_own_lane(m);
            if !(validated || own_lane) {
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
            // A method constructing a vector at a lane of its own binders is
            // validated only to produce its template: each per-call clone is
            // still checked, or derived, so a body the symbolic check cannot
            // type gets no verdict.
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
        let _position = self.comptime_position();
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
            // a type that is not a parameter, which the constraint compiler
            // reserves for parameters.
            ExprKind::Call { name, args, .. }
                if name == "conforms_to"
                    && args.len() == 2
                    && self
                        .comptime_type_operand(&args[0])?
                        .is_some_and(|operand| !matches!(operand, Ty::Param { .. })) =>
            {
                let Some(trait_names) = mojito_ast::ast::trait_conjunction_names(&args[1]) else {
                    return Err(TypeError::Unsupported(
                        "conforms_to takes a trait name, or a '&' conjunction of trait names, \
                         as its second argument"
                            .to_string(),
                    ));
                };
                trait_names
                    .iter()
                    .try_for_each(|trait_name| self.check_trait_name(trait_name))?;
                // A dependent element (a reflected field type at a loop
                // index) conforms per instance: the condition is the
                // conjunction of its conformance propositions, which the
                // elaborator below MIR decides once the element is closed.
                let Some(Ty::Dependent(dependent)) = self.comptime_type_operand(&args[0])? else {
                    return Ok(None);
                };
                let mut propositions = Vec::with_capacity(trait_names.len());
                for trait_name in trait_names {
                    let conforms = self
                        .param_context
                        .conforms(
                            dependent.expr(),
                            mojito_ast::ast::canonical_trait_name(trait_name),
                        )
                        .map_err(param_error)?;
                    propositions.push(GenericConstraint::Eq(
                        ConstraintOperand::Expr(conforms),
                        ConstraintOperand::Value(mojito_types::ct::CtValue::Bool(true)),
                    ));
                }
                let Some(constraint) = propositions.into_iter().reduce(|all, proposition| {
                    GenericConstraint::And(Box::new(all), Box::new(proposition))
                }) else {
                    return Ok(None);
                };
                self.record_comptime_condition(cond, &constraint);
                return Ok(Some(constraint));
            }
            // A `comptime for` variable of `Bool` elements is the binder
            // itself, compared with `True`: a thunk over the owner's binders
            // could not take the loop's.
            ExprKind::Identifier(name)
                if let Some(binder) = self
                    .value_parameter_in_scope(name)
                    .filter(is_bool_loop_binder) =>
            {
                let constraint = GenericConstraint::Eq(
                    ConstraintOperand::Expr(binder),
                    ConstraintOperand::Value(mojito_types::ct::CtValue::Bool(true)),
                );
                self.record_comptime_condition(cond, &constraint);
                return Ok(Some(constraint));
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
            // A local `comptime` binding the elaborator evaluates (`comptime
            // e = L[1]`) is no parameter: the condition runs with it.
            Ok(constraint) if self.names_evaluated_binding(&constraint) => {
                self.expect_bool(cond, "comptime if condition")?;
                Ok(None)
            }
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
        let position = self.comptime_position();
        let iter = self.inline_local_comptime_values(iter);
        // A local binding of an evaluated display is iterated where it is
        // declared.
        let bound = self
            .bound_display(&iter)
            .filter(|_| !self.source_validation)
            .cloned();
        // A reflected field-name list is its own sequence of strings: the
        // names of a registered struct, or the query over a subject that is
        // still a parameter.
        let names = match self.reflection_list(&iter)? {
            _ if bound.is_some() => None,
            Some((mojito_types::param_expr::ReflectQuery::FieldNames, list)) => Some(match list {
                mojito_types::ct::CtValue::Expr(query) => query,
                closed => self
                    .param_context
                    .constant(closed)
                    .map_err(|error| TypeError::Unsupported(error.to_string()))?,
            }),
            _ => None,
        };
        let element = match (&bound, &names) {
            (Some(display), _) => display.element.clone(),
            (None, Some(_)) => Ty::StringLiteral,
            (None, None) => self.comptime_iteration_element(&iter)?,
        };
        let before = self.uninitialized.borrow().clone();
        self.push_scope();
        if let Some(bindings) = self.compile_time_bindings.last_mut() {
            bindings.insert(var.to_string());
        }
        let elements = self.closed_iteration_elements(&iter, &element);
        // A string element binds as a `String` value parameter does.
        let element = match (&elements, element) {
            (Some(_), Ty::Struct(name, args))
                if args.is_empty() && mojito_types::types::is_stdlib_string_struct(&name) =>
            {
                Ty::StringLiteral
            }
            (_, element) => element,
        };
        // A value pack still a parameter is its own sequence.
        let sequence = match elements {
            Some(elements) => Some(
                self.param_context
                    .constant(mojito_types::ct::CtValue::List(elements))
                    .map_err(|error| TypeError::Unsupported(error.to_string()))?,
            ),
            None => names.or_else(|| self.value_pack_named(&iter)),
        };
        // A display the check does not close is evaluated per instance.
        let evaluated = sequence.is_none() && self.evaluated_display(&iter, &element);
        let element = match element {
            Ty::Struct(name, args)
                if evaluated
                    && args.is_empty()
                    && mojito_types::types::is_stdlib_string_struct(&name) =>
            {
                Ty::StringLiteral
            }
            element => element,
        };
        let binds_index = element == Ty::Int || sequence.is_some() || evaluated || bound.is_some();
        // The executable check sees only a loop the elaborator kept, whose
        // variable must be a binder MIR carries.
        if !binds_index && !self.source_validation {
            return Err(TypeError::Unsupported(format!(
                "'comptime for' over elements of type '{element}' in a generic body: its \
                 variable binds an 'Int', 'Float64', 'Bool', or 'String' element, or a \
                 tuple or struct of them"
            )));
        }
        let binder = binds_index.then(|| comptime_index_binder(var, &iter, &element));
        match (&binder, &bound) {
            (Some(binder), Some(display)) => self.record_bound_iteration(source, binder, display),
            (Some(binder), None) => {
                self.record_comptime_iteration(source, &iter, binder, sequence, evaluated)?;
            }
            (None, _) => {}
        }
        drop(position);
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

    /// Record the loop header's sequence over the binders in scope
    /// (`SemanticAdjustment::ComptimeIteration`): `elements`, the sequence
    /// of an iterable that is not a range (a closed collection display's
    /// constant, a value pack), or a `range(...)` iterable's bounds, each
    /// compiled as a parameter expression; an `evaluated` display records
    /// its element alone. A bound the compiler does not
    /// close — a pack length, a compile-time list's — is left unrecorded
    /// under source validation, where the loop is the cloner's to unroll,
    /// and is the explicit boundary in the executable check, which only sees
    /// a loop the elaborator kept.
    fn record_comptime_iteration(
        &self,
        source: &Expr,
        iter: &Expr,
        binder: &ParamExpr,
        elements: Option<ParamExpr>,
        evaluated: bool,
    ) -> Result<(), TypeError> {
        use mojito_checked::checked::ComptimeSource;
        let element = binder.meta().clone();
        let Some(binder) = binder.as_decl_ref() else {
            return Ok(());
        };
        let record_source = |source_form| {
            self.operation_adjustments.borrow_mut().insert(
                source.source_span(),
                mojito_checked::checked::SemanticAdjustment::ComptimeIteration(Box::new(
                    mojito_checked::checked::ComptimeIteration {
                        binder: binder.clone(),
                        source: source_form,
                    },
                )),
            );
        };
        if evaluated {
            let construction = self
                .operation_adjustments
                .borrow()
                .get(&source.source_span())
                .cloned();
            if let Some(construction) = construction {
                record_source(ComptimeSource::Evaluated {
                    element,
                    construction: Box::new(construction),
                });
            }
            return Ok(());
        }
        let record = |sequence| record_source(ComptimeSource::Sequence(sequence));
        if let Some(elements) = elements {
            record(mojito_checked::checked::ComptimeSequence::Elements(
                elements,
            ));
            return Ok(());
        }
        let ExprKind::Call { name, args, .. } = &iter.kind else {
            return Ok(());
        };
        if name != "range" {
            return Ok(());
        }
        let bounds = args
            .iter()
            .map(|arg| self.compile_dependent_ct_expr(arg))
            .collect::<Vec<_>>();
        // A bound the compiler does not close that reads compile-time
        // bindings alone (`len(L)` over a local display binding) is
        // evaluated per instance.
        if !self.source_validation
            && (1..=3).contains(&args.len())
            && bounds.iter().any(Result::is_err)
            && bounds
                .iter()
                .zip(args)
                .all(|(bound, arg)| bound.is_ok() || self.reads_compile_time_alone(arg))
        {
            record_source(ComptimeSource::EvaluatedRange {
                bounds: bounds.into_iter().map(Result::ok).collect(),
            });
            return Ok(());
        }
        let bounds = match bounds.into_iter().collect::<Result<Vec<_>, _>>() {
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
        record(mojito_checked::checked::ComptimeSequence::Range { start, stop, step });
        Ok(())
    }

    /// Whether every local `expr` names is a compile-time binding: a value
    /// binder, a `comptime for` variable, or a local `comptime` constant or
    /// display. Such an expression can run in a function the elaborator
    /// evaluates per instance.
    fn reads_compile_time_alone(&self, expr: &Expr) -> bool {
        names_read(expr).iter().all(|name| {
            self.binding_scope(name).is_none_or(|scope| scope == 0)
                || self.is_compile_time_binding(name)
                || self.lookup_owner(name).is_some_and(|owner| {
                    self.value_parameter_owners.contains(&owner)
                        || self.comptime_binding_owners.contains(&owner)
                })
        })
    }

    /// [`Self::lifted_application`] where the check stands in a lifting
    /// position ([`Self::lifting_position`]).
    pub(super) fn positioned_application(&self, expr: &Expr) -> Option<ParamExpr> {
        (self.lifting_positions.get() > 0)
            .then(|| self.lifted_application(expr))
            .flatten()
    }

    /// The parameter expression of a compile-time `Int` or `Bool`
    /// expression that compiles to none (`h(L[0])`, `n > 2`,
    /// `min(L[0], L[1])`), written in a type or a parameter argument or
    /// bound by a local `comptime`.
    ///
    /// A call of a module function is the application of that function to
    /// what its arguments denote ([`Self::called_application`]), wherever
    /// it is spelled: a signature, a field type, or a body. Any other
    /// expression of a body is the application of the function MIR lifts
    /// for it, named here, to the binders it reads
    /// (`SemanticAdjustment::ComptimeApplication`, [`Self::binders_read`]).
    /// Such an expression reads compile-time bindings alone. Source
    /// validation lifts one that names a binder or a local binding, and
    /// leaves a closed one to the elaborator.
    ///
    /// Two occurrences of one lifted expression over the same bindings
    /// denote one application, as the pin identifies them by structure, and
    /// it is not the value it computes: `h(n)` is not `n * 2`. `None` for
    /// any other expression.
    pub(super) fn lifted_application(&self, expr: &Expr) -> Option<ParamExpr> {
        let called = self.called_application(expr);
        // A signature or a field type has no body to lift a function from,
        // and no compile-time argument calls a raising function.
        if self.scopes.len() < 2 || self.calls_raising_application(expr).is_some() {
            return called;
        }
        let names = names_read(expr);
        let symbolic = names.iter().any(|name| {
            name == "Self"
                || self.binding_scope(name).is_some_and(|scope| scope > 0)
                || self.value_parameter_in_scope(name).is_some()
        });
        if called.is_none()
            && ((!symbolic && self.source_validation) || !self.reads_compile_time_alone(expr))
        {
            return None;
        }
        // The expression's own arguments are typed where they stand.
        let lifting = self.lifting_positions.replace(0);
        let ty = {
            let _position = self.comptime_position();
            self.infer(expr)
        };
        self.lifting_positions.set(lifting);
        let ty = match ty.ok()? {
            Ty::Int | Ty::IntLiteral => Ty::Int,
            Ty::Bool => Ty::Bool,
            _ => return None,
        };
        let application = match called {
            Some(application) => application,
            None => self.named_application(expr, &names, ty)?,
        };
        if !self.source_validation {
            self.lifted_expressions
                .borrow_mut()
                .insert(expr.source_span(), application.clone());
        }
        Some(application)
    }

    /// The application a call of a module function or a static method
    /// denotes when every argument is a compile-time `Int` or `Bool`
    /// (`h(n)`, `h(Self.n + 1)`, `twice[n]()`, `S.f(n)`, `d(n, m=4)`): the
    /// callable applied to the parameter expressions of its compile-time
    /// arguments, then of its runtime ones in declared order, a default
    /// standing where the call omits one, as the pin's call node is. It
    /// names no declaration's binders, so a caller that binds them spells
    /// the same value (`h(2)` for `h(n)` at `n = 2`, `d(1, 3)` for `d(1)`),
    /// and the elaborator runs the callable on the argument values. `None`
    /// for a callee that is no such callable ([`applicable_functions`]), a
    /// call that selects no one overload, or an argument that denotes no
    /// parameter expression.
    fn called_application(&self, expr: &Expr) -> Option<ParamExpr> {
        let (function, arguments) = self.selected_application(expr)?;
        (!function.raises).then(|| {
            self.param_context.apply(
                &function.symbol,
                &arguments,
                mojito_types::param_expr::MetaTy::value(function.result.clone()),
            )
        })
    }

    /// Whether `expr` calls a `raises` callable a compile-time call would
    /// otherwise apply, which the pin rejects in a type or a parameter
    /// argument.
    pub(super) fn calls_raising_application(&self, expr: &Expr) -> Option<&str> {
        let (function, _) = self.selected_application(expr)?;
        function.raises.then_some(function.symbol.as_str())
    }

    /// The callable a compile-time call applies and the parameter
    /// expressions of its arguments: the one overload under the spelled
    /// name the arguments bind and type.
    fn selected_application(&self, expr: &Expr) -> Option<(&ApplicableFunction, Vec<ParamExpr>)> {
        let (name, instance, param_args, args, kwargs) = match &expr.kind {
            // A local of the name shadows the module function.
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } if self.binding_scope(name).is_none_or(|scope| scope == 0) => {
                (name.clone(), None, param_args.as_slice(), args, kwargs)
            }
            ExprKind::MethodCall {
                object,
                method,
                args,
                kwargs,
            } => {
                let (owner, instance) = self.applied_owner(object, method)?;
                let name = format!("{owner}.{method}");
                (name, Some(instance), [].as_slice(), args, kwargs)
            }
            // A static method given its own compile-time arguments
            // (`S.t[n]()`).
            ExprKind::Invoke {
                callee,
                param_args,
                args,
                kwargs,
            } => {
                let ExprKind::Member { object, field } = &callee.kind else {
                    return None;
                };
                let (owner, instance) = self.applied_owner(object, field)?;
                let name = format!("{owner}.{field}");
                (name, Some(instance), param_args.as_slice(), args, kwargs)
            }
            _ => return None,
        };
        let mut selected = self
            .applicable_functions
            .get(&name)?
            .iter()
            .filter_map(|function| {
                let mut arguments = Vec::new();
                if function.on_instance {
                    arguments.push(self.param_context.type_shape(instance.clone()?));
                }
                arguments.extend(self.applied_arguments(function, param_args, args, kwargs)?);
                Some((function, arguments))
            });
        let application = selected.next()?;
        selected.next().is_none().then_some(application)
    }

    /// The struct whose static method `method` a call on `object` names
    /// (`S.f(n)`, `G[n].f()`, `Self.f()`), with the instance `object`
    /// spells; `None` for a receiver that is a value, or a struct with no
    /// applicable `method`.
    fn applied_owner(&self, object: &Expr, method: &str) -> Option<(String, Ty)> {
        let applicable = |owner: &str| {
            self.applicable_functions
                .contains_key(&format!("{owner}.{method}"))
        };
        let named = |name: &String, args: Vec<ParamArg>| {
            (self.binding_scope(name).is_none() && applicable(name))
                .then(|| {
                    self.ty_from_anno(&SourceType::Named(name.clone(), args))
                        .ok()
                })
                .flatten()
        };
        let instance = match &object.kind {
            ExprKind::Identifier(name) if name == "Self" => self.self_ty.clone().filter(
                |ty| matches!(ty, Ty::Struct(owner, _) if applicable(template_name(owner))),
            ),
            ExprKind::Identifier(name) => named(name, Vec::new()),
            ExprKind::TypeApply { name, args } => named(name, args.clone()),
            // A lone value argument parses as a subscript (`G[n]`).
            ExprKind::Index { object, index } => match (&object.kind, &index.kind) {
                (ExprKind::Identifier(name), ExprKind::TupleLit(elements)) => named(
                    name,
                    elements.iter().cloned().map(ParamArg::Value).collect(),
                ),
                (ExprKind::Identifier(name), _) => {
                    named(name, vec![ParamArg::Value((**index).clone())])
                }
                _ => None,
            },
            _ => None,
        }?;
        match &instance {
            Ty::Struct(owner, _) => Some((template_name(owner).to_string(), instance.clone())),
            _ => None,
        }
    }

    /// The parameter expressions a call's arguments give `function`, its
    /// compile-time parameters first; `None` when they do not bind its
    /// parameters or one is not a compile-time value of its parameter's
    /// type.
    fn applied_arguments(
        &self,
        function: &ApplicableFunction,
        param_args: &[ParamArg],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Option<Vec<ParamExpr>> {
        use mojito_ast::call::{ArgSlot, CallVariadics, match_call_slots};
        if param_args.len() != function.binders.len() {
            return None;
        }
        let mut arguments = Vec::with_capacity(function.binders.len() + function.params.len());
        for (index, (binder, ty)) in function.binders.iter().enumerate() {
            let named = param_args.iter().find_map(|arg| match arg {
                ParamArg::Named { name, value } if name == binder => Some(&**value),
                _ => None,
            });
            let positional = param_args
                .get(index)
                .filter(|arg| !matches!(arg, ParamArg::Named { .. }));
            let argument = match named.or(positional)? {
                ParamArg::Value(value) => self.applied_argument(value, ty)?,
                ParamArg::Type(SourceType::SelfParam(parameter)) => {
                    self.applied_value(self.self_param_value(parameter)?, ty)?
                }
                _ => return None,
            };
            arguments.push(argument);
        }
        let names: Vec<String> = function
            .params
            .iter()
            .map(|param| param.name.clone())
            .collect();
        let required: Vec<bool> = function
            .params
            .iter()
            .map(|param| param.default.is_none())
            .collect();
        let keywords: Vec<&str> = kwargs.iter().map(|kwarg| kwarg.name.as_str()).collect();
        let matched = match_call_slots(
            &names,
            &required,
            function.positional_only,
            function.keyword_only,
            args.len(),
            &keywords,
            CallVariadics {
                positional: false,
                keyword: false,
            },
        )
        .ok()?;
        for (slot, param) in matched.slots.iter().zip(&function.params) {
            arguments.push(match slot {
                ArgSlot::Positional(index) => self.applied_argument(&args[*index], &param.ty)?,
                ArgSlot::Keyword(index) => {
                    self.applied_argument(&kwargs[*index].value, &param.ty)?
                }
                ArgSlot::Default => self.applied_default(param.default.as_ref()?, &param.ty)?,
            });
        }
        Some(arguments)
    }

    /// The parameter expression of a call argument for a parameter of type
    /// `ty`.
    fn applied_argument(&self, argument: &Expr, ty: &Ty) -> Option<ParamExpr> {
        let _lifting = self.lifting_position();
        let argument = self.compile_dependent_ct_expr(argument).ok()?;
        self.applied_value(argument.into_value(), ty)
    }

    /// The parameter expression of a declared default a call omits, for a
    /// parameter of type `ty`. The default is its declaration's, so it
    /// stands only where every name it reads means what it means there: no
    /// local and no binder of the calling declaration.
    fn applied_default(&self, default: &Expr, ty: &Ty) -> Option<ParamExpr> {
        names_read(default)
            .iter()
            .all(|name| {
                name != "Self"
                    && self.binding_scope(name).is_none_or(|scope| scope == 0)
                    && self.value_parameter_in_scope(name).is_none()
            })
            .then(|| self.applied_argument(default, ty))
            .flatten()
    }

    /// `value` as the argument of a parameter of type `ty`. A literal is the
    /// value the parameter holds, so that `h(2)` is `h(n)` at `n = 2`.
    fn applied_value(&self, value: CtValue, ty: &Ty) -> Option<ParamExpr> {
        let argument = match value {
            CtValue::Expr(argument) => argument,
            closed => self
                .param_context
                .constant(closed.materialize_as(ty)?)
                .ok()?,
        };
        (argument.meta().as_value() == Some(ty)).then_some(argument)
    }

    /// The application of the function MIR lifts for `expr`, an expression
    /// of a body of type `ty` that reads `names`: one function per
    /// expression over the same binders and bindings.
    fn named_application(&self, expr: &Expr, names: &[String], ty: Ty) -> Option<ParamExpr> {
        // The function is over the binders the expression reads, so that it
        // is one function under a `comptime for` and outside it.
        let mut binders = self.binders_in_scope();
        if let Some(read) = self.binders_read(expr, names) {
            binders.retain(|binder| {
                binder
                    .as_decl_ref()
                    .is_some_and(|reference| read.contains(&reference.id))
            });
        }
        let bindings: Vec<_> = names.iter().map(|name| self.lookup_owner(name)).collect();
        let validated = self.source_validation;
        let known = self
            .lifted_applications
            .borrow()
            .iter()
            .find(|known| {
                known.validated == validated
                    && known.expression == *expr
                    && known.binders == binders
                    && known.bindings == bindings
            })
            .map(|known| known.application.clone());
        known.or_else(|| {
            // An identity of its own names the function, as a binding's
            // names its display's; the empty owner keeps the name apart
            // from one MIR gives a function it lifts itself.
            let name = if validated {
                format!("$comptime$$at{}", expr.span.0)
            } else {
                format!("$comptime$${}", self.fresh_owner().ok()?.0)
            };
            let application = self.param_context.apply(
                &name,
                &binders,
                mojito_types::param_expr::MetaTy::value(ty),
            );
            self.lifted_applications
                .borrow_mut()
                .push(LiftedApplication {
                    expression: expr.clone(),
                    binders,
                    bindings,
                    validated,
                    application: application.clone(),
                });
            Some(application)
        })
    }

    /// The binders `expr` reads through the `names` it spells: each binder
    /// it names, those a local display binding or a local `comptime`
    /// parameter expression it names is over, and the enclosing struct's
    /// for `Self`. `None` when the binders are not known (a type-shaped
    /// argument, a local value that denotes no parameter expression), where
    /// every binder in scope stands in.
    fn binders_read(
        &self,
        expr: &Expr,
        names: &[String],
    ) -> Option<HashSet<mojito_types::param_expr::ParamId>> {
        struct Types(bool);
        impl mojito_ast::visit::Visitor for Types {
            fn visit_type(&mut self, _ty: &SourceType) {
                self.0 = true;
            }
        }
        let mut types = Types(false);
        mojito_ast::visit::walk_expr(&mut types, expr);
        if types.0 {
            return None;
        }
        let mut read = HashSet::new();
        for name in names {
            if name == "Self" {
                read.extend(self.self_decls.iter().map(|decl| decl.id().clone()));
                continue;
            }
            // A bare type argument (`width[T]()`) names its binder.
            let type_parameter = self.tparams.iter().rev().find_map(|scope| scope.get(name));
            match type_parameter {
                Some(Ty::Param { binder, .. }) => {
                    read.insert(binder.id.clone());
                    continue;
                }
                Some(_) => return None,
                None => {}
            }
            let denoted = match self.display_named(name) {
                Some(display) => Some(display.sequence.clone()),
                None => self.value_parameter_in_scope(name),
            };
            match denoted {
                Some(denoted) => read.extend(
                    denoted
                        .free_parameters()
                        .into_iter()
                        .map(|reference| reference.id),
                ),
                None if self.binding_scope(name).is_none_or(|scope| scope == 0) => {}
                None => return None,
            }
        }
        Some(read)
    }

    /// Record on each expression the executable check lifted the
    /// application it denotes, beside the expression's own checked
    /// operation (`SemanticAdjustment::ComptimeApplication`).
    pub(super) fn record_lifted_applications(&self) {
        use mojito_checked::checked::SemanticAdjustment;
        let mut operations = self.operation_adjustments.borrow_mut();
        for (span, application) in self.lifted_expressions.borrow().iter() {
            let operation = match operations.get(span) {
                Some(SemanticAdjustment::ComptimeApplication { operation, .. }) => {
                    operation.clone()
                }
                operation => operation.cloned().map(Box::new),
            };
            operations.insert(
                span.clone(),
                SemanticAdjustment::ComptimeApplication {
                    operation,
                    application: application.clone(),
                },
            );
        }
    }

    /// Record the header of a loop over a local binding of an evaluated
    /// display (`ComptimeSource::Bound`): the sequence is the one MIR lifts
    /// at the binding.
    fn record_bound_iteration(&self, source: &Expr, binder: &ParamExpr, display: &BoundDisplay) {
        use mojito_checked::checked::{ComptimeIteration, ComptimeSource, SemanticAdjustment};
        let Some(reference) = binder.as_decl_ref() else {
            return;
        };
        self.operation_adjustments.borrow_mut().insert(
            source.source_span(),
            SemanticAdjustment::ComptimeIteration(Box::new(ComptimeIteration {
                binder: reference.clone(),
                source: ComptimeSource::Bound {
                    element: binder.meta().clone(),
                    display: display.span.clone(),
                },
            })),
        );
    }

    /// The elements a `comptime for` over a closed collection display binds,
    /// in order, each materialized at the loop variable's `element` type: a
    /// list display's elements, a set display's distinct elements, a
    /// dictionary display's distinct keys. `None` for any other iterable, or
    /// a display with an element that is not a literal of a type a loop
    /// binder takes: a scalar (`Int`, `Float64`, `Bool`, `String`), or a
    /// tuple or struct of them ([`Self::parameter_aggregate_element`]).
    fn closed_iteration_elements(
        &self,
        iter: &Expr,
        element: &Ty,
    ) -> Option<Vec<mojito_types::ct::CtValue>> {
        let scalar = matches!(element, Ty::Int | Ty::Float64 | Ty::Bool)
            || matches!(element, Ty::Struct(name, args)
                if args.is_empty() && mojito_types::types::is_stdlib_string_struct(name));
        if !scalar && !self.parameter_aggregate_element(element) {
            return None;
        }
        let (leaves, distinct): (Vec<&Expr>, bool) = match &iter.kind {
            ExprKind::ListLit(items) => (items.iter().collect(), false),
            ExprKind::BraceLit(entries) if !entries.is_empty() => {
                (entries.iter().map(|(key, _)| key).collect(), true)
            }
            _ => return None,
        };
        let mut elements = Vec::with_capacity(leaves.len());
        for leaf in leaves {
            let value = self
                .eval_associated_ct(leaf, &HashMap::new())
                .ok()?
                .materialize_as(element)?;
            if !(distinct && elements.contains(&value)) {
                elements.push(value);
            }
        }
        Some(elements)
    }

    /// Whether `element` is a nominal `Tuple`, or a non-generic fieldwise
    /// struct, whose elements or fields are numbers, booleans, strings, or
    /// such aggregates: a parameter value a loop binder holds, which a
    /// backend materializes as a closed constant, or constructs where a
    /// string leaf makes it no constant.
    fn parameter_aggregate_element(&self, element: &Ty) -> bool {
        let leaf = |ty: &Ty| {
            matches!(
                ty,
                Ty::Int
                    | Ty::Float64
                    | Ty::Bool
                    | Ty::IntLiteral
                    | Ty::FloatLiteral
                    | Ty::StringLiteral
            ) || matches!(ty, Ty::Struct(name, args)
                if args.is_empty() && mojito_types::types::is_stdlib_string_struct(name))
                || self.parameter_aggregate_element(ty)
        };
        if let Some(elements) = mojito_types::types::tuple_elements(element) {
            return !elements.is_empty() && elements.into_iter().all(leaf);
        }
        matches!(element, Ty::Struct(name, args) if args.is_empty()
        && self.structs.get(name).is_some_and(|info| {
            info.fieldwise_init
                && info.decls.is_empty()
                && !info.fields.is_empty()
                && info.fields.iter().all(|(_, ty)| leaf(ty))
        }))
    }

    /// Whether `iter` is a display the elaborator evaluates per instance
    /// when the check does not close it: a collection display of scalars
    /// ([`evaluated_display`]), or a list display of parameter aggregates
    /// ([`Self::parameter_aggregate_element`]).
    fn evaluated_display(&self, iter: &Expr, element: &Ty) -> bool {
        evaluated_display(iter, element)
            || (matches!(&iter.kind, ExprKind::ListLit(items) if !items.is_empty())
                && self.parameter_aggregate_element(element))
    }

    /// The calls and method calls of `program`, and its tuple displays when
    /// `tuples` holds, this validation typed as `accept` takes without
    /// raising, by the syntax identity the elaborated program keeps. A node
    /// copied more than once qualifies only when every copy did.
    fn typed_display_elements(
        &self,
        program: &[Stmt],
        tuples: bool,
        accept: &dyn Fn(&Ty) -> bool,
    ) -> HashSet<mojito_common::token::SyntaxId> {
        struct Elements<'a> {
            checker: &'a Checker,
            tuples: bool,
            accept: &'a dyn Fn(&Ty) -> bool,
            verdicts: HashMap<mojito_common::token::SyntaxId, bool>,
        }
        impl mojito_ast::visit::Visitor for Elements<'_> {
            fn visit_expr(&mut self, expression: &Expr) {
                let candidate = match expression.kind {
                    ExprKind::Call { .. } | ExprKind::MethodCall { .. } => true,
                    ExprKind::TupleLit(_) => self.tuples,
                    _ => false,
                };
                if !candidate {
                    return;
                }
                let span = expression.source_span();
                let accepted = self
                    .checker
                    .expression_types
                    .borrow()
                    .get(&span)
                    .is_some_and(|ty| (self.accept)(ty));
                let raises = self
                    .checker
                    .expression_effects
                    .borrow()
                    .get(&span)
                    .is_some_and(|effects| effects.raises.is_some());
                *self
                    .verdicts
                    .entry(self.checker.syntax_origins.origin(expression.syntax_id))
                    .or_insert(true) &= accepted && !raises;
            }
        }
        let mut elements = Elements {
            checker: self,
            tuples,
            accept,
            verdicts: HashMap::new(),
        };
        mojito_ast::visit::walk_block(&mut elements, program);
        elements
            .verdicts
            .into_iter()
            .filter_map(|(element, accepted)| accepted.then_some(element))
            .collect()
    }

    /// The value-parameter scope beside the innermost open type-parameter
    /// scope. A level past it is dead: `tparams.pop()` closes both.
    fn innermost_value_scope(&mut self) -> Option<&mut HashMap<String, ParamExpr>> {
        self.tparams
            .len()
            .checked_sub(1)
            .and_then(|level| self.vparams.get_mut(level))
    }

    /// Bind a local `comptime NAME = value` over the binders of the template
    /// body being checked, which the elaborator keeps (`comptime m = N + 1`):
    /// a type-valued one is a scoped type alias, and a value-valued one names
    /// its parameter expression wherever the body reads a compile-time value
    /// (a type argument, a `comptime if` condition), as upstream's
    /// `comptime` alias does. A type alias records no statement binding, so
    /// MIR gives it no runtime form. Returns `false` when the binding is not
    /// symbolic, or when the ordinary binding path still types its runtime
    /// reads.
    pub(super) fn bind_template_comptime(
        &mut self,
        stmt: &Stmt,
        name: &str,
        annotation: Option<&SourceType>,
        value: &Expr,
    ) -> Result<bool, TypeError> {
        let Some(level) = self.tparams.len().checked_sub(1) else {
            return Ok(false);
        };
        if let Some(ty) = self.comptime_type_operand(value)? {
            self.local_type_aliases
                .last_mut()
                .ok_or_else(|| {
                    TypeError::InvariantViolation("checker scope stack is empty".to_string())
                })?
                .insert(name.to_string(), ty);
            return Ok(true);
        }
        if let Some((target, display)) = self.aliased_display(value) {
            // An alias of a display binding (`comptime A = L`) is another
            // name of that binding: it denotes the same sequence, and MIR
            // sees a read of it, and the statement, as the display's.
            self.declare_alias(name, target)?;
            self.record_statement_binding(stmt, name);
            self.local_comptime_displays
                .last_mut()
                .ok_or_else(|| {
                    TypeError::InvariantViolation("checker scope stack is empty".to_string())
                })?
                .insert(name.to_string(), display);
            return Ok(true);
        }
        if let Some(expression) = self.comptime_value_expression(value) {
            // An annotation names the parameter expression's own type: no
            // conversion applies to one.
            if let (Some(annotation), Some(found)) = (annotation, expression.meta().as_value()) {
                let expected = self
                    .resolve_storage_annotation(annotation, super::StorageStrictness::AllowBare)?;
                if &expected != found {
                    return Err(TypeError::TypeMismatch {
                        expected: expected.to_string(),
                        found: found.to_string(),
                        context: format!("comptime '{name}'"),
                    });
                }
            }
            if let Some(scope) = self.local_comptime_parameters.last_mut() {
                scope.insert(name.to_string(), (level, expression));
            }
        }
        if let Some((element, ty, construction)) = self.evaluated_display_binding(value) {
            // The name is typed for the compile-time expressions that read
            // it, and its statement binding is the one a lifted function
            // binds the display to.
            self.declare_immutable(name, ty.clone())?;
            self.record_statement_binding(stmt, name);
            let display = self.record_display_binding(name, value, element, ty, construction)?;
            self.local_comptime_displays
                .last_mut()
                .ok_or_else(|| {
                    TypeError::InvariantViolation("checker scope stack is empty".to_string())
                })?
                .insert(name.to_string(), display);
            return Ok(true);
        }
        // A reflection handle, a reflected list (`r.field_names()`), or a
        // type list computed from the body's packs
        // (`TypeList._concat[Self.Ts.values, OtherTs.values]()`), is a
        // compile-time-only value inlined at its uses.
        if matches!(&value.kind, ExprKind::TypeApply { name, .. } if name == "reflect")
            || matches!(self.reflection_list(value), Ok(Some(_)))
            || matches!(self.type_list_operand(value), Ok(Some(_)))
        {
            self.local_comptime_values
                .last_mut()
                .ok_or_else(|| {
                    TypeError::InvariantViolation("checker scope stack is empty".to_string())
                })?
                .insert(name.to_string(), value.clone());
            return Ok(true);
        }
        Ok(false)
    }

    /// What the value of a local `comptime` binding over a template body's
    /// binders denotes wherever the body reads it at compile time: its
    /// parameter expression, or the application of the function lifted for
    /// an `Int` or a `Bool` that compiles to none (`comptime e = h(n)`).
    /// `None` for a constant, or a value that denotes nothing.
    fn comptime_value_expression(&self, value: &Expr) -> Option<ParamExpr> {
        let _lifting = self.lifting_position();
        self.compile_dependent_ct_expr(value)
            .ok()
            .or_else(|| self.lifted_application(value))
            .filter(|expression| expression.as_constant().is_none())
    }

    /// Type the display a local `comptime` binding of a template body holds
    /// (`comptime L = [n, n + 1]`): a collection display of scalar elements,
    /// or a list display of closed aggregates (`[(1, n), (n, 2)]`),
    /// which the elaborator evaluates per instance as it does one written in
    /// a loop header. Gives the element a loop over it binds, the display's
    /// own type, and its checked construction; `None` for any other
    /// binding, which the ordinary path types.
    fn evaluated_display_binding(
        &self,
        value: &Expr,
    ) -> Option<(Ty, Ty, mojito_checked::checked::SemanticAdjustment)> {
        let element = self.comptime_iteration_element(value).ok()?;
        if !self.evaluated_display(value, &element) {
            return None;
        }
        let ty = self.infer(value).ok()?;
        let construction = self
            .operation_adjustments
            .borrow()
            .get(&value.source_span())
            .cloned()?;
        Some((string_element_binder(element), ty, construction))
    }

    /// Record the display the declared local `comptime` binding `name` holds
    /// for MIR to lift (`SemanticAdjustment::ComptimeDisplay`). It is a
    /// compile-time value with no runtime form: a `comptime for` iterates
    /// it, a type or a parameter argument reads an element or the length as
    /// a parameter expression over its sequence, any other compile-time
    /// expression reads it in a function the elaborator runs, and
    /// `materialize[L]()` builds it where it is written.
    ///
    /// The sequence is the application of the function MIR lifts for the
    /// display, named here by the binding, to every binder in scope: the pin
    /// keeps a read of the binding symbolic the same way, so `L[0]` is not
    /// the element the display spells.
    fn record_display_binding(
        &self,
        name: &str,
        value: &Expr,
        element: Ty,
        ty: Ty,
        construction: mojito_checked::checked::SemanticAdjustment,
    ) -> Result<BoundDisplay, TypeError> {
        let owner = self.lookup_owner(name).ok_or_else(|| {
            TypeError::InvariantViolation(format!("comptime binding '{name}' has no owner"))
        })?;
        let sequence = self.display_sequence(name, owner.0, &element);
        let span = value.source_span();
        self.operation_adjustments.borrow_mut().insert(
            span.clone(),
            mojito_checked::checked::SemanticAdjustment::ComptimeDisplay {
                construction: Box::new(construction),
                sequence: sequence.clone(),
            },
        );
        Ok(BoundDisplay {
            span,
            element,
            ty,
            sequence,
            positional: matches!(value.kind, ExprKind::ListLit(_)),
        })
    }

    /// The sequence the display binding `name` denotes, a list of
    /// `element`s: the application of the function named by the binding and
    /// `identity`, which tells two bindings of one name apart, to every
    /// binder in scope.
    fn display_sequence(
        &self,
        name: &str,
        identity: impl std::fmt::Display,
        element: &Ty,
    ) -> ParamExpr {
        use mojito_types::param_expr::MetaTy;
        self.param_context.apply(
            &format!("$comptime${name}${identity}"),
            &self.binders_in_scope(),
            MetaTy::ParamList(Box::new(MetaTy::value(element.clone()))),
        )
    }

    /// Under source validation, note that the local `comptime` binding
    /// `name` holds a display the elaborator evaluates per instance, for
    /// the types and parameter arguments that read an element or the
    /// length. The binding itself stays the ordinary path's; the executable
    /// check binds the display ([`Self::bind_template_comptime`]).
    fn note_validated_display(&mut self, name: &str, value: &Expr) {
        if self.tparams.is_empty() {
            return;
        }
        if let Some((_, display)) = self.aliased_display(value) {
            if let Some(scope) = self.local_comptime_displays.last_mut() {
                scope.insert(name.to_string(), display);
            }
            return;
        }
        let Some((element, ty)) = self
            .comptime_iteration_element(value)
            .ok()
            .filter(|element| self.evaluated_display(value, element))
            .zip(self.infer(value).ok())
        else {
            return;
        };
        let element = string_element_binder(element);
        let display = BoundDisplay {
            span: value.source_span(),
            sequence: self.display_sequence(name, format_args!("at{}", value.span.0), &element),
            element,
            ty,
            positional: matches!(value.kind, ExprKind::ListLit(_)),
        };
        if let Some(scope) = self.local_comptime_displays.last_mut() {
            scope.insert(name.to_string(), display);
        }
    }

    /// The display binding a local `comptime` alias's `value` names
    /// (`comptime A = L`), which the alias denotes whole, with its name.
    fn aliased_display<'v>(&self, value: &'v Expr) -> Option<(&'v str, BoundDisplay)> {
        let ExprKind::Identifier(target) = &value.kind else {
            return None;
        };
        Some((target, self.display_named(target)?.clone()))
    }

    /// Every binder in scope as the reference a parameter expression names
    /// it by, outermost declaration first: the enclosing struct's
    /// parameters, then each open declaration's type parameters and
    /// non-callable value parameters, a `comptime for` index among them.
    fn binders_in_scope(&self) -> Vec<ParamExpr> {
        use mojito_types::param_expr::MetaTy;
        let mut binders: Vec<ParamExpr> = self
            .self_decls
            .iter()
            .filter_map(|decl| match decl {
                ParamDecl::Type { name, variadic, .. } => Some(self.param_context.decl_ref(
                    decl.id().clone(),
                    name.trim_start_matches('*'),
                    if *variadic {
                        MetaTy::type_list()
                    } else {
                        MetaTy::Type
                    },
                )),
                ParamDecl::Value {
                    ty,
                    callable_default: None,
                    ..
                } if !matches!(ty.as_ref(), Ty::Func { .. } | Ty::GenericFunc { .. }) => {
                    Some(super::annotations::value_parameter_expr(decl, ty))
                }
                ParamDecl::Value { .. } => None,
            })
            .collect();
        for (level, types) in self.tparams.iter().enumerate() {
            let packs = self.pack_params.get(level);
            let mut types: Vec<ParamExpr> = types
                .values()
                .filter_map(|ty| match ty {
                    Ty::Param { binder, .. } => Some(
                        packs
                            .and_then(|packs| packs.get(binder.name.trim_start_matches('*')))
                            .cloned()
                            .unwrap_or_else(|| {
                                self.param_context.decl_ref(
                                    binder.id.clone(),
                                    &binder.name,
                                    MetaTy::Type,
                                )
                            }),
                    ),
                    _ => None,
                })
                .collect();
            types.sort();
            binders.extend(types);
            let mut values: Vec<ParamExpr> = self
                .vparams
                .get(level)
                .into_iter()
                .flat_map(HashMap::values)
                .filter(|value| value.as_decl_ref().is_some())
                .cloned()
                .collect();
            values.sort();
            binders.extend(values);
        }
        let mut seen = HashSet::new();
        binders.retain(|binder| {
            binder
                .as_decl_ref()
                .is_some_and(|reference| seen.insert(reference.id.clone()))
        });
        binders
    }

    /// The local binding of an evaluated display `expr` names: the binding
    /// its name resolves to, when that one is a display's.
    pub(super) fn bound_display(&self, expr: &Expr) -> Option<&BoundDisplay> {
        let ExprKind::Identifier(name) = &expr.kind else {
            return None;
        };
        self.display_named(name)
    }

    /// The local binding of an evaluated display `name` resolves to.
    pub(super) fn display_named(&self, name: &str) -> Option<&BoundDisplay> {
        self.local_comptime_displays
            .get(self.binding_scope(name)?)?
            .get(name)
    }

    /// Reject a read of a local display binding outside every compile-time
    /// position: it would materialize the whole collection, which is not
    /// implicitly copyable, as the pin rejects it.
    pub(super) fn reject_display_crossing(&self, expr: &Expr) -> Result<(), TypeError> {
        match self.bound_display(expr) {
            Some(display) if self.crosses_to_runtime() => Err(TypeError::ComptimeCrossing(
                materialized_collection_spelling(&display.ty),
            )),
            _ => Ok(()),
        }
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
        // A value over the body's binders (`comptime n = Self.Ts.length`)
        // names its parameter expression in a compile-time position, as it
        // does in the executable check ([`Self::bind_template_comptime`]).
        if let Some(level) = self.tparams.len().checked_sub(1)
            && let Some(expression) = self.comptime_value_expression(&value)
            && let Some(scope) = self.local_comptime_parameters.last_mut()
        {
            scope.insert(name.to_string(), (level, expression));
        }
        if self.infer(&value).is_ok() {
            self.note_validated_display(name, &value);
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
            // `tl[i]` over a closed or computed list, then `types[i]`,
            // `r.field_at[i].T`: a reflected field type.
            _ => match self.type_list_element(expr)? {
                Some(element) => Some(element),
                None => self.reflected_type_operand(expr)?,
            },
        })
    }

    /// The type `list[index]` denotes over a `TypeList` value or a list
    /// computed from packs; a pack itself is [`Self::pack_element_type`]'s.
    fn type_list_element(&self, expr: &Expr) -> Result<Option<Ty>, TypeError> {
        let ExprKind::Index { object, index } = &expr.kind else {
            return Ok(None);
        };
        let list = match self.typelist_receiver(object)? {
            None | Some(super::constraints::TypeListReceiver::Pack(_)) => return Ok(None),
            Some(receiver) => self.receiver_list(receiver)?,
        };
        let index = self
            .compile_dependent_ct_expr(index)
            .map_err(|_| TypeError::TypeMismatch {
                expected: "a compile-time Int index".to_string(),
                found: "a runtime value".to_string(),
                context: "TypeList index".to_string(),
            })?;
        self.param_context
            .list_get(&list, &index)
            .map(mojito_types::types::DependentType::resolve)
            .map(Some)
            .map_err(param_error)
    }

    /// Type a pack element's default construction while the pack is still a
    /// parameter: `Self.Ts[i]()`, or a `def`'s own `Ts[i]()`, is the
    /// dependent element type, and the element must be `Defaultable` by the
    /// pack's bound or a `where` clause. `None` when `pack` (read through
    /// `Self` when `through_self`) names no unbound pack or the call is not a
    /// nullary construction at one index.
    pub(super) fn infer_pack_element_construction(
        &self,
        span: SourceSpan,
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
        let (base, spelled, reference) = if through_self {
            let declared = self.self_decls.iter().any(|decl| {
                matches!(decl, ParamDecl::Type { name, variadic: true, .. }
                    if name.trim_start_matches('*') == pack)
            });
            let reference = self.pack_reference(pack).filter(|_| declared)?;
            (
                SourceType::SelfParam(pack.to_string()),
                format!("Self.{pack}"),
                reference,
            )
        } else {
            let reference = self
                .pack_parameter_in_scope(pack)
                .filter(|_| self.lookup(pack).is_none())?;
            (
                SourceType::Named(pack.to_string(), Vec::new()),
                pack.to_string(),
                reference,
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
                    // A symbolic element is the template's construction,
                    // which the elaborator resolves per instance; a closed
                    // one is an ordinary construction of its type.
                    if mojito_types::types::is_symbolic(&element)
                        && let Some(pack) = reference.as_decl_ref()
                    {
                        self.operation_adjustments.borrow_mut().insert(
                            span,
                            mojito_checked::checked::SemanticAdjustment::ConstructPackElement {
                                pack: pack.clone(),
                            },
                        );
                    }
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

    /// `materialize[X]()` under source validation: upstream's
    /// `materialize[value: T]() -> T`, so the runtime value has the type of
    /// its compile-time operand — a binding's own checked type (a bound
    /// `field_names()` list is no declared binding), or that of a
    /// compile-time expression over the bindings in scope (`names[i]`). The
    /// elaborator materializes the operand's display the same way.
    pub(super) fn infer_materialize_crossing(
        &self,
        param_args: &[ParamArg],
    ) -> Result<Ty, TypeError> {
        let _position = self.comptime_position();
        match param_args {
            [
                ParamArg::Value(
                    operand @ Expr {
                        kind: ExprKind::Identifier(name),
                        ..
                    },
                ),
            ] => match self.lookup(name) {
                Some(ty) => Ok(ty.clone()),
                None => self
                    .materialized_field_names(operand)?
                    .ok_or_else(|| TypeError::UndefinedVariable(name.clone())),
            },
            [ParamArg::Type(SourceType::Named(name, _))] => self
                .lookup(name)
                .cloned()
                .ok_or_else(|| TypeError::UndefinedVariable(name.clone())),
            [ParamArg::Value(operand)] => self.infer(operand),
            _ => Err(TypeError::Unsupported(
                "materialize[...]() takes one compile-time value".to_string(),
            )),
        }
    }

    /// `materialize[X]()` in the executable check: the crossing pass folds
    /// every operand it evaluates, so `X` is over a binder of a template
    /// body, and the call is the runtime value of that operand at its type.
    /// A whole reflected list (`materialize[names]()`) has no runtime form a
    /// template carries.
    pub(super) fn infer_template_materialize(
        &self,
        param_args: &[ParamArg],
    ) -> Result<Ty, TypeError> {
        let [ParamArg::Value(operand)] = param_args else {
            return Err(TypeError::Unsupported(
                "materialize[...]() takes one compile-time value".to_string(),
            ));
        };
        if self.reflection_list(operand)?.is_some() {
            return Err(TypeError::Unsupported(
                "materialize[...]() of a reflected list over a type parameter: a template \
                 carries one element of it (materialize[names[i]]()), not the list"
                    .to_string(),
            ));
        }
        let _position = self.comptime_position();
        self.infer(operand)
    }

    /// `comptime(e)` in the executable check: one the crossing pass left is
    /// over a binder of a template body, and the call is the runtime value
    /// of its compile-time operand, which reads compile-time bindings alone.
    /// A display binding does not cross whole this way.
    pub(super) fn infer_template_comptime(&self, operand: &Expr) -> Result<Ty, TypeError> {
        if !self.reads_compile_time_alone(operand) {
            return Err(TypeError::NotComptime(
                "a 'comptime(...)' operand that reads a runtime value".to_string(),
            ));
        }
        let _position = self.comptime_position();
        let ty = self.infer(operand)?;
        match self
            .bound_display(operand)
            .filter(|_| !self.source_validation)
        {
            Some(display) => Err(TypeError::ComptimeCrossing(
                materialized_collection_spelling(&display.ty),
            )),
            None => Ok(ty),
        }
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

    /// The value pack an expression names while it is still a parameter: a
    /// `def`'s own `*values: Int`, typed as the pack of its element, which
    /// no instance has bound yet, or the enclosing struct's `Self.values`.
    pub(super) fn value_pack_named(&self, expr: &Expr) -> Option<ParamExpr> {
        if let ExprKind::Member { object, field } = &expr.kind
            && matches!(&object.kind, ExprKind::Identifier(name) if name == "Self")
        {
            return self.self_value_pack(field);
        }
        let ExprKind::Identifier(name) = &expr.kind else {
            return None;
        };
        let Some(Ty::VariadicPack(element)) = self.lookup(name) else {
            return None;
        };
        if mojito_types::types::pack_spread(std::slice::from_ref(element.as_ref())).is_some() {
            return None;
        }
        self.value_parameter_in_scope(name).filter(|pack| {
            pack.as_decl_ref().is_some()
                && matches!(pack.meta(), mojito_types::param_expr::MetaTy::ParamList(_))
        })
    }

    /// The value pack a bracket argument spreads whole: `*vs` of a `def`'s
    /// or method's own value pack, or `*Self.vs` of the enclosing struct's.
    pub(super) fn value_pack_spread(&self, argument: &ParamArg) -> Option<ParamExpr> {
        match argument {
            // The struct's own pack arrives here too: pack qualification
            // folds `*Self.vs` onto the bare spread.
            ParamArg::Type(SourceType::Named(name, args)) if args.is_empty() => {
                let name = name.strip_prefix('*')?;
                self.value_pack_named(&Expr::new(
                    ExprKind::Identifier(name.to_string()),
                    mojito_common::token::DUMMY_SPAN,
                ))
                .or_else(|| self.self_value_pack(name))
            }
            ParamArg::Type(SourceType::SelfParam(name)) => {
                self.self_value_pack(name.strip_prefix('*')?)
            }
            _ => None,
        }
    }

    /// The enclosing struct's own value pack `Self.<field>`, as the
    /// parameter-list reference its binder declares.
    pub(super) fn self_value_pack(&self, field: &str) -> Option<ParamExpr> {
        self.self_decls.iter().find_map(|decl| match decl {
            ParamDecl::Value {
                name,
                ty,
                variadic: true,
                ..
            } if name.trim_start_matches('*') == field => Some(
                self.param_context
                    .intern(&super::annotations::value_parameter_expr(decl, ty)),
            ),
            _ => None,
        })
    }

    /// Type `values[index]` over a value pack that is still a parameter: the
    /// element at a compile-time index, recorded at `span` as the parameter
    /// constant the elaborator folds per instance.
    pub(super) fn infer_value_pack_element(
        &self,
        span: SourceSpan,
        pack: &ParamExpr,
        index: &Expr,
    ) -> Result<Ty, TypeError> {
        let index = self
            .compile_dependent_ct_expr(index)
            .map_err(|_| TypeError::TypeMismatch {
                expected: "a compile-time Int index".to_string(),
                found: "a runtime value".to_string(),
                context: "value-pack index".to_string(),
            })?;
        let element = self
            .param_context
            .list_get(pack, &index)
            .map_err(param_error)?;
        let ty = element.meta().as_value().cloned().ok_or_else(|| {
            TypeError::InvariantViolation(format!("value-pack element '{element}' is not a value"))
        })?;
        self.operation_adjustments.borrow_mut().insert(
            span,
            mojito_checked::checked::SemanticAdjustment::ParamValue {
                value: element,
                materialized: None,
            },
        );
        Ok(ty)
    }

    /// Record a query of a pack that is still a parameter, read as a runtime
    /// value at `span`: the constant MIR carries and the elaborator folds.
    pub(super) fn record_pack_query_value(
        &self,
        span: SourceSpan,
        pack: &ParamExpr,
        query: mojito_types::param_expr::PackQuery,
    ) {
        if let Some(pack) = pack.as_decl_ref() {
            self.operation_adjustments.borrow_mut().insert(
                span,
                mojito_checked::checked::SemanticAdjustment::ParamValue {
                    value: self.param_context.pack_query(pack, query),
                    materialized: None,
                },
            );
        }
    }

    /// The pack query a `TypeList` proposition over a pack asks, when its
    /// operand names a type.
    pub(super) fn typelist_proposition_query(
        &self,
        proposition: GenericConstraint,
    ) -> Option<mojito_types::param_expr::PackQuery> {
        use mojito_types::param_expr::PackQuery;
        Some(match proposition {
            GenericConstraint::ConformsPack { trait_name, .. } => PackQuery::Conforms(trait_name),
            GenericConstraint::PackPredicate { predicate, all, .. } => {
                PackQuery::Predicate { predicate, all }
            }
            GenericConstraint::PackContains { element, .. } => {
                let element = match element {
                    ConstraintOperand::Type(ty) => ty,
                    ConstraintOperand::Param(param) => self
                        .ty_from_anno(&mojito_ast::ast::Type::Named(
                            param.name.to_string(),
                            Vec::new(),
                        ))
                        .ok()?,
                    _ => return None,
                };
                PackQuery::Contains(self.param_context.type_shape(element))
            }
            _ => return None,
        })
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
        // The collector is the forwarded pack itself retyped by a struct's
        // pack (`*b: *Self.Ts` on `V[*Ts]`): pack types are invariant in
        // their bounds.
        let retyped = matches!((collector, &forwarded.pack),
            (Ty::Param { binder, bounds, .. }, Ty::Param { binder: own, bounds: own_bounds, .. })
                if binder.id == own.id && !same_bounds(bounds, own_bounds));
        if retyped
            || expected
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

    /// The compiler-private storage aggregate constructed from a whole pack
    /// that is still a parameter (`__RuntimeTuple(*args^)`, in `Tuple`'s own
    /// `__init__`): storage over that pack. A declared struct's construction
    /// from a forwarded pack selects its `__init__` (`infer_construction`).
    /// `None` for every other call.
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
            // Any other callee binds the forwarded pack to its own collector
            // (`forwarded_pack_argument`, `bind_forwarded_pack`), a struct
            // through its declared `__init__` (`infer_construction`).
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
    /// a bare construction takes once its pack is solved, as is a
    /// construction spreading a forwarded pack. `None` outside validation
    /// and for every callee this does not type.
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
            || args
                .iter()
                .any(|argument| self.forwarded_pack(argument).is_some())
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
        let names_element = |ty: &Ty| {
            matches!(ty, Ty::Dependent(dependent) if dependent.pack_element().is_some())
                || mojito_types::types::list_spread(ty).is_some()
        };
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
                    // A pack bound to a list computed from other packs.
                    TyArg::Ty(ty) if let Some(list) = mojito_types::types::list_spread(ty) => {
                        Some((name, CtValue::Expr(list.clone())))
                    }
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

    /// Whether a nominal type has no checkable declaration here: unregistered
    /// (a discovery-round abstract scalar range). Its iteration and subscript
    /// contracts come from the family, not from method lookup.
    pub(super) fn is_abstract_struct(&self, name: &str) -> bool {
        !self.structs.contains_key(name)
    }

    /// Every `Param` operand of a compiled condition must name a parameter,
    /// binding, type, trait, or alias in scope: the constraint compiler
    /// reads any bare identifier as a parameter name.
    fn constraint_operands_resolve(&self, constraint: &GenericConstraint) -> Result<(), TypeError> {
        Self::each_constraint_parameter(constraint, &|name| self.comptime_name_resolves(name))
    }

    /// Whether a compiled condition names, as a parameter, a local
    /// `comptime` binding of the executable check that denotes no parameter
    /// expression.
    fn names_evaluated_binding(&self, constraint: &GenericConstraint) -> bool {
        Self::each_constraint_parameter(constraint, &|name| {
            let evaluated = self
                .lookup_owner(name)
                .is_some_and(|owner| self.comptime_binding_owners.contains(&owner))
                && !self
                    .local_comptime_parameters
                    .iter()
                    .any(|scope| scope.contains_key(name));
            if evaluated {
                Err(TypeError::NotComptime(name.to_string()))
            } else {
                Ok(())
            }
        })
        .is_err()
    }

    /// Visit the name of every `Param` operand of a compiled condition,
    /// stopping at the first one `visit` rejects.
    fn each_constraint_parameter(
        constraint: &GenericConstraint,
        visit: &dyn Fn(&str) -> Result<(), TypeError>,
    ) -> Result<(), TypeError> {
        let operand = |operand: &ConstraintOperand| match operand {
            ConstraintOperand::Param(param) | ConstraintOperand::PackLength(param) => {
                visit(&param.name)
            }
            // An arithmetic operand resolved its names when it was compiled.
            ConstraintOperand::Value(_)
            | ConstraintOperand::Type(_)
            | ConstraintOperand::Expr(_) => Ok(()),
        };
        match constraint {
            GenericConstraint::WithMessage(inner, _) | GenericConstraint::Not(inner) => {
                Self::each_constraint_parameter(inner, visit)
            }
            GenericConstraint::Conforms { param, .. }
            | GenericConstraint::ConformsPack { param, .. }
            | GenericConstraint::PackPredicate { param, .. } => visit(&param.name),
            GenericConstraint::PackContains { param, element } => {
                visit(&param.name)?;
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
                Self::each_constraint_parameter(a, visit)?;
                Self::each_constraint_parameter(b, visit)
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
                for method in methods {
                    let class = if mojito_ast::simd_width::method_constructs_at_own_lane(method) {
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
    if mojito_types::types::pack_spread_argument(arguments).is_some()
        || mojito_types::types::list_spread_argument(arguments).is_some()
    {
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

/// Whether source validation checks a declaration's body: one holding
/// compile-time control flow, a `rebind` over the declaration's own
/// parameters (`keys_rebind`, from `rebind::rebind_keyed_bodies`), keyed
/// on a variadic pack — the declaration's own, or that of the struct
/// `enclosing` it — each leaves the template stubbed, so validation is the
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

#[derive(Debug, Clone)]
struct ApplicableParameter {
    name: String,
    ty: Ty,
    default: Option<Expr>,
}

/// The signature of a `def` or a static method, as its declaration spells
/// it.
struct ApplicableShape<'a> {
    type_params: &'a [mojito_ast::ast::TypeParam],
    params: &'a [mojito_ast::ast::FnParam],
    positional_only: Option<usize>,
    keyword_only: Option<usize>,
    raises: bool,
    ret: &'a SourceType,
}

impl ApplicableShape<'_> {
    /// The callable under `symbol`, when a compile-time call can apply it.
    fn applicable(&self, symbol: String, on_instance: bool) -> Option<ApplicableFunction> {
        let scalar = |ty: &SourceType| match ty {
            SourceType::Int => Some(Ty::Int),
            SourceType::Bool => Some(Ty::Bool),
            _ => None,
        };
        let binders = self
            .type_params
            .iter()
            .map(|binder| {
                let plain = binder.value_type.is_none()
                    && binder.callable_bound.is_none()
                    && binder.origin_mutability.is_none()
                    && binder.default.is_none()
                    && binder.constraints.is_empty()
                    && !binder.infer_only;
                let ty = match binder.bounds.as_slice() {
                    [bound] if plain && bound == "Int" => Ty::Int,
                    [bound] if plain && bound == "Bool" => Ty::Bool,
                    _ => return None,
                };
                Some((binder.name.clone(), ty))
            })
            .collect::<Option<Vec<_>>>()?;
        let params = self
            .params
            .iter()
            .map(|param| {
                let passed_by_value = param.kind == mojito_ast::ast::ParamKind::Regular
                    && matches!(
                        param.convention,
                        None | Some(ArgConvention::Imm | ArgConvention::Var)
                    );
                Some(ApplicableParameter {
                    name: param.name.clone(),
                    ty: scalar(&param.ty).filter(|_| passed_by_value)?,
                    default: param.default.clone(),
                })
            })
            .collect::<Option<Vec<_>>>()?;
        Some(ApplicableFunction {
            symbol,
            on_instance,
            binders,
            params,
            positional_only: self.positional_only,
            keyword_only: self.keyword_only,
            result: scalar(self.ret)?,
            raises: self.raises,
        })
    }
}

/// The name a struct was declared under, whichever instance clone `name`
/// spells.
fn template_name(name: &str) -> &str {
    mojito_symbol::symbol::specialization_template(name).unwrap_or(name)
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

/// The spelling of a compile-time collection's type in the rejection of its
/// runtime read, as the pin spells it: a list display is a fixed-size
/// `Array[Int, Int(2)]`, a set `Set[Int]`, a dictionary `Dict[String, Int]`.
fn materialized_collection_spelling(ty: &Ty) -> String {
    fn scalar(element: &Ty) -> String {
        match element {
            Ty::Int | Ty::IntLiteral => "Int".to_string(),
            Ty::Float64 | Ty::FloatLiteral => "Float64".to_string(),
            Ty::StringLiteral => "String".to_string(),
            other => match mojito_types::types::tuple_elements(other) {
                Some(elements) => format!(
                    "Tuple[{}]",
                    elements
                        .into_iter()
                        .map(scalar)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                None => mojito_types::types::unqualified_type_name(other),
            },
        }
    }
    if let Some((key, value)) = mojito_types::types::dict_elements(ty) {
        return format!("Dict[{}, {}]", scalar(key), scalar(value));
    }
    if let Some(element) = mojito_types::types::set_element(ty) {
        return format!("Set[{}]", scalar(element));
    }
    if let Ty::Struct(name, args) = ty
        && name == mojito_types::types::ARRAY_TYPE_NAME
    {
        let mut arguments = args.iter();
        if let (Some(TyArg::Ty(element)), Some(TyArg::Val(length))) =
            (arguments.next(), arguments.next())
        {
            return format!("Array[{}, Int({length})]", scalar(element));
        }
    }
    mojito_types::types::unqualified_type_name(ty)
}

/// The element spelling of a materialized compile-time tuple: a literal
/// element is named by the type it materializes to, as the elaborator
/// spells the same rejection.
fn materialized_element_spelling(ty: &Ty) -> String {
    match ty {
        Ty::IntLiteral => "Int".to_string(),
        Ty::StringLiteral => "String".to_string(),
        Ty::FloatLiteral => "Float64".to_string(),
        other => mojito_types::types::unqualified_type_name(other),
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
    // The rebuilt node is the occurrence it was rebuilt from, so a fact
    // recorded on it is that occurrence's.
    let mut rewritten = Expr::new(kind, expr.span);
    rewritten.source.clone_from(&expr.source);
    rewritten.syntax_id = expr.syntax_id;
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

/// The compile-time binder of a `comptime for` variable of type `element`,
/// so a dependent expression over it (`Ts[i]`, `args[i]`, `x * n`) has a
/// node. The loop's iterable names the binder: each loop owns its variable.
fn comptime_index_binder(var: &str, iter: &Expr, element: &Ty) -> ParamExpr {
    let span = iter.source_span();
    let owner = format!(
        "{}{}:{}..{}",
        mojito_types::param_expr::COMPTIME_FOR_OWNER,
        span.source.as_deref().unwrap_or_default(),
        span.span.0,
        span.span.1
    );
    value_binder_expr(ParamId::new(&owner, 0), var, element)
}

/// Whether `iter` is a collection display of scalar `element`s a loop binder
/// takes (`Int`, `Bool`, `Float64`, `String`), which the elaborator evaluates per
/// instance when the check does not close it.
fn evaluated_display(iter: &Expr, element: &Ty) -> bool {
    let display = match &iter.kind {
        ExprKind::ListLit(items) => !items.is_empty(),
        ExprKind::BraceLit(entries) => !entries.is_empty(),
        _ => false,
    };
    display
        && (matches!(element, Ty::Int | Ty::Bool | Ty::Float64)
            || matches!(element, Ty::Struct(name, args)
                if args.is_empty() && mojito_types::types::is_stdlib_string_struct(name)))
}

/// The element a display of `element`s binds: a string element binds as a
/// `String` value parameter does.
pub(super) fn string_element_binder(element: Ty) -> Ty {
    match element {
        Ty::Struct(name, args)
            if args.is_empty() && mojito_types::types::is_stdlib_string_struct(&name) =>
        {
            Ty::StringLiteral
        }
        element => element,
    }
}

/// Whether `binder` is a `comptime for` variable of `Bool` elements
/// ([`comptime_index_binder`]).
fn is_bool_loop_binder(binder: &ParamExpr) -> bool {
    binder
        .as_decl_ref()
        .is_some_and(|reference| reference.id.is_comptime_for_binder())
        && binder.meta().as_value() == Some(&Ty::Bool)
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

/// Every name `expr` reads, in source order.
fn names_read(expr: &Expr) -> Vec<String> {
    struct Names(Vec<String>);
    impl mojito_ast::visit::Visitor for Names {
        fn visit_expr(&mut self, expr: &Expr) {
            if let ExprKind::Identifier(name) = &expr.kind {
                self.0.push(name.clone());
            }
        }
    }
    let mut names = Names(Vec::new());
    mojito_ast::visit::walk_expr(&mut names, expr);
    names.0
}

/// Whether two bound lists name the same traits, in any order.
fn same_bounds(left: &[String], right: &[String]) -> bool {
    left.len() == right.len() && left.iter().all(|bound| right.contains(bound))
}
