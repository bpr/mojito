//! Source validation of compile-time control flow (`validate_comptime_templates`):
//! a `comptime if` condition is typed as a compile-time `Bool`, every arm and
//! `comptime for` body is checked in its own scope with the declaration's
//! parameters symbolic, and function-local `comptime` bindings the elaborator
//! would otherwise consume are bound here. Extracted from `checker.rs`; see
//! `docs/symbol-map.md`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_ast::ast::{ParamArg, SourceType};
use mojito_types::types::TyKind;
use mojito_types::types::{ConstraintOperand, GenericConstraint};

/// The bundled `ParameterList`'s linked name, its implicit public identity.
pub(super) const PARAMETER_LIST: &str = "ParameterList";

/// The bundled `TypeList`'s linked name, its implicit public identity.
pub(super) const TYPE_LIST: &str = "TypeList";

/// A local `comptime` binding of a display the elaborator evaluates per
/// instance (`comptime L = [n, n + 1]`).
#[derive(Debug, Clone)]
pub(in crate::checker) struct BoundDisplay {
    /// The display's own type, which a runtime read would materialize.
    ty: Ty,
    /// What the binding denotes in a type or a parameter argument: the
    /// application of the function MIR lifts for the display to the binders
    /// in scope, a list of the display's elements.
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
    owner: String,
    expression: Expr,
    binders: Vec<ParamExpr>,
    bindings: Vec<Option<mojito_types::origin::OwnerId>>,
    application: ParamExpr,
}

/// Where a binder-free `comptime` binding is declared, which decides
/// whether a collection display initializer is lifted
/// ([`super::lifts_comptime_value`]): a module constant's is, typed by its
/// annotation (`comptime E: Dict[String, Int] = {}`), while a local display
/// binding keeps its display route and only a local constructor call is
/// lifted.
#[derive(Clone, Copy, Debug)]
pub(super) enum ConstantScope<'a> {
    Module(Option<&'a SourceType>),
    Local,
}

/// The compile-time test a condition belongs to, which words its
/// diagnostics as the pin does for each.
#[derive(Clone, Copy, Debug)]
pub(super) enum ComptimeTest {
    If,
    Assert,
    /// A `comptime match` subject, and each test its patterns compile to.
    MatchSubject,
}

impl ComptimeTest {
    pub(super) const fn dynamic_value(self) -> &'static str {
        match self {
            Self::If => "cannot use a dynamic value in 'comptime if' condition",
            Self::Assert => "cannot use a dynamic value in 'comptime assert' expression",
            Self::MatchSubject => "cannot use a dynamic value in 'match' subject",
        }
    }

    const fn context(self) -> &'static str {
        match self {
            Self::If => "comptime if condition",
            Self::Assert => "comptime assert condition",
            Self::MatchSubject => "match subject",
        }
    }

    const fn raising_call(self) -> &'static str {
        match self {
            Self::If => "cannot call raising function in 'comptime if' condition",
            Self::Assert => "cannot call raising function in 'comptime assert' expression",
            Self::MatchSubject => "cannot call raising function in boolean condition",
        }
    }
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
/// an `Int`, a `UInt`, a `Bool`, or a `Float64`, and which returns one or a
/// named struct.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
    /// The declared result: an `Int`, a `UInt`, a `Bool`, or a `Float64`, or
    /// a named struct the check resolves where a call applies it.
    result: SourceType,
    /// Whether it is declared `raises`, which no type or parameter argument
    /// may call.
    raises: bool,
}

/// The rejection of a raw parameter list (`KGENParamList[T]`, a pack's
/// `vs.values`) read as a runtime value, which the pin materializes and
/// Mojito does not.
pub(super) fn raw_list_runtime_read(meta: &mojito_types::param_expr::MetaTy) -> TypeError {
    let spelled = meta
        .value_ty()
        .map_or_else(|| meta.to_string(), |ty| ty.to_string());
    TypeError::Unsupported(format!(
        "a raw parameter list ('{spelled}') read at run time is not supported; \
         read it through 'ParameterList[...]'"
    ))
}

/// The rejection of a KGEN string (a string literal's `.value`, a
/// `!kgen.string` binder) read as a runtime value, which the pin
/// materializes and Mojito does not.
pub(super) fn kgen_string_runtime_read() -> TypeError {
    TypeError::Unsupported(format!(
        "a '{}' value read at run time is not supported",
        mojito_types::types::KGEN_STRING_TYPE_SPELLING
    ))
}

/// The callables a compile-time call applies by name
/// ([`Checker::called_application`]), every overload under the name a call
/// spells: `h` for a module `def`, `S.f` for a static method.
pub(super) fn applicable_functions(
    stmts: &[Stmt],
    overloads: &impl mojito_symbol::symbol::OverloadLookup,
    constants: &dyn mojito_symbol::symbol::ModuleNames,
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
                let symbol = mojito_symbol::symbol::lowered_def_name(
                    name,
                    type_params,
                    params,
                    overloads,
                    constants,
                );
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
                        constants,
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
    /// A `comptime if` condition must be a compile-time `Bool`: a generic
    /// constraint over the parameters in scope (`T == Int`, `n == 0`,
    /// `conforms_to(T, Copyable)`, a `TypeList` proposition, a predicate
    /// alias), or an ordinary `Bool` expression over compile-time bindings
    /// (a `comptime for` variable, a `Bool` parameter). It is typed, never
    /// evaluated: selection is the elaborator's. Every leaf under
    /// `not`/`and`/`or`, and the whole condition, is recorded on its span
    /// (`SemanticAdjustment::ComptimeCondition`) for the MIR branch: a leaf
    /// the constraint compiler does not close — an application, a `Bool`
    /// binding — is its parameter expression compared with `True`
    /// ([`Self::evaluated_condition`]). The recorded constraint, its
    /// operands bound, when the whole condition closes.
    pub(super) fn check_comptime_condition(
        &self,
        cond: &Expr,
        test: ComptimeTest,
    ) -> Result<Option<GenericConstraint>, TypeError> {
        let _position = self.comptime_position();
        let inlined = self.inline_local_comptime_values(cond);
        Ok(self.check_ct_bool(&inlined, test)?.map(|constraint| {
            self.record_comptime_condition(cond, &constraint);
            self.bind_constraint(&constraint, &[])
        }))
    }

    /// A `comptime assert`: its condition is checked as a `comptime if`
    /// condition is, never judged here — the elaborator judges it per
    /// reached instance. The message is a string literal. The condition's
    /// bound reading, when it closes, for the knowledge it introduces.
    pub(super) fn check_comptime_assert(
        &self,
        cond: &Expr,
        message: Option<&Expr>,
    ) -> Result<Option<GenericConstraint>, TypeError> {
        if message.is_some_and(|message| !matches!(message.kind, ExprKind::Str(_))) {
            return Err(TypeError::Unsupported(
                "a 'comptime assert' message other than a string literal".to_string(),
            ));
        }
        self.check_comptime_condition(cond, ComptimeTest::Assert)
    }

    /// Whether the condition of a conditional expression is a parameter
    /// value, which upstream's `IfElseOpNode` lowers as `hlcf.comptime.if`
    /// (`getIfPValue`): a `Bool` literal, a `Bool` compile-time binding, a
    /// `comptime(e)`, a conformance or trivial-type predicate, a `TypeList`
    /// proposition, `not` of one, or a comparison of compile-time scalars or
    /// of types. `and`/`or` and a nested conditional are run-time `hlcf.if`s, a call of
    /// an ordinary function a run-time value, so none of those is one.
    pub(super) fn parameter_valued_condition(&self, cond: &Expr) -> bool {
        match &cond.kind {
            ExprKind::Bool(_) => true,
            ExprKind::Prefix(PrefixOp::Not, inner) => self.parameter_valued_condition(inner),
            ExprKind::Identifier(_) | ExprKind::Member { .. } => self
                .parameter_value_ty(cond)
                .is_some_and(|ty| matches!(ty.kind(), TyKind::Bool)),
            ExprKind::Call { name, args, .. } => {
                (name == "comptime" || name == "conforms_to") && !args.is_empty()
            }
            ExprKind::TypeApply { .. } | ExprKind::Index { .. } => {
                self.predicate_alias_application(cond).is_some()
                    || predicate_spelling(cond).is_some_and(|name| {
                        mojito_types::types::trivial_predicate_name(name).is_some()
                    })
            }
            ExprKind::Invoke { .. } => {
                matches!(self.compile_typelist_proposition(cond), Ok(Some(_)))
            }
            ExprKind::Infix(
                InfixOp::Eq | InfixOp::Ne | InfixOp::Lt | InfixOp::Le | InfixOp::Gt | InfixOp::Ge,
                left,
                right,
            ) => {
                (self.parameter_valued_scalar(left) && self.parameter_valued_scalar(right))
                    || [left, right]
                        .iter()
                        .all(|operand| matches!(self.comptime_type_operand(operand), Ok(Some(_))))
            }
            _ => false,
        }
    }

    /// The type of the parameter value `operand` names: a value binder, a
    /// `comptime for` variable, a local or module `comptime` binding, or the
    /// enclosing struct's value parameter (`Self.flag`).
    pub(super) fn parameter_value_ty(&self, operand: &Expr) -> Option<&Ty> {
        match &operand.kind {
            ExprKind::Identifier(name)
                if self.is_compile_time_binding(name)
                    || self.lookup_owner(name).is_some_and(|owner| {
                        self.value_parameter_owners.contains(&owner)
                            || self.is_comptime_binding_owner(owner)
                    }) =>
            {
                self.lookup(name)
            }
            ExprKind::Member { object, field } if matches!(&object.kind, ExprKind::Identifier(name) if name == "Self") => {
                self.self_decls.iter().find_map(|decl| match decl {
                    ParamDecl::Value {
                        name,
                        ty,
                        variadic: false,
                        ..
                    } if name == field => Some(ty),
                    _ => None,
                })
            }
            _ => None,
        }
    }

    /// Record `constraint` as the compile-time reading of `cond`, each
    /// operand naming its binder by identity: the innermost open scope's,
    /// else the enclosing struct's.
    fn record_comptime_condition(&self, cond: &Expr, constraint: &GenericConstraint) {
        let bound = self.bind_constraint(constraint, &[]);
        self.record_at(cond.source_span()).adjustment = Some(Box::new(
            mojito_checked::checked::SemanticAdjustment::ComptimeCondition(bound),
        ));
    }

    /// Whether `operand` of a comparison is a parameter value whose builtin
    /// operators fold at compile time: an integer or `Bool` literal, or an
    /// `Int`, `UInt`, or `Bool` parameter value, under arithmetic. A float's
    /// comparison is a run-time value, as the pin has it.
    fn parameter_valued_scalar(&self, operand: &Expr) -> bool {
        match &operand.kind {
            ExprKind::Int(_) | ExprKind::Bool(_) => true,
            ExprKind::Identifier(_) | ExprKind::Member { .. } => {
                self.parameter_value_ty(operand).is_some_and(|ty| {
                    matches!(
                        ty.kind(),
                        TyKind::Int | TyKind::UInt | TyKind::Bool | TyKind::IntLiteral
                    )
                })
            }
            ExprKind::Prefix(PrefixOp::Neg | PrefixOp::Invert, inner) => {
                self.parameter_valued_scalar(inner)
            }
            ExprKind::Infix(
                InfixOp::Add
                | InfixOp::Sub
                | InfixOp::Mul
                | InfixOp::FloorDiv
                | InfixOp::Mod
                | InfixOp::Shl
                | InfixOp::Shr
                | InfixOp::BitAnd
                | InfixOp::BitOr
                | InfixOp::BitXor,
                left,
                right,
            ) => self.parameter_valued_scalar(left) && self.parameter_valued_scalar(right),
            _ => false,
        }
    }

    /// The recursive form of [`Self::check_comptime_condition`]: a
    /// connective recurses so a concrete conformance fact can sit beside a
    /// symbolic constraint; a leaf is a generic constraint over the
    /// parameters in scope, a conformance of a concrete type, or a `Bool`
    /// value expression. The constraint the condition compiles to, when every
    /// leaf does.
    fn check_ct_bool(
        &self,
        cond: &Expr,
        test: ComptimeTest,
    ) -> Result<Option<GenericConstraint>, TypeError> {
        match &cond.kind {
            ExprKind::Bool(value) => return Ok(Some(GenericConstraint::Bool(*value))),
            ExprKind::Prefix(PrefixOp::Not, inner) => {
                return Ok(self
                    .check_ct_bool(inner, test)?
                    .map(|inner| GenericConstraint::Not(Box::new(inner))));
            }
            ExprKind::Infix(op @ (InfixOp::And | InfixOp::Or), left, right) => {
                let left = self.check_ct_bool(left, test)?;
                let right = self.check_ct_bool(right, test)?;
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
                        .is_some_and(|operand| !matches!(operand.kind(), TyKind::Param { .. })) =>
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
                // A concrete type's conformance is a fact decided here.
                let operand = self.comptime_type_operand(&args[0])?;
                let Some(TyKind::Dependent(dependent)) = operand.as_ref().map(Ty::kind) else {
                    let Some(operand) = operand else {
                        return Ok(None);
                    };
                    let conforms = !mojito_types::conformance::reflects_no_conformance(&operand)
                        && trait_names
                            .iter()
                            .all(|trait_name| self.conforms_to(&operand, trait_name));
                    let constraint = GenericConstraint::Bool(conforms);
                    self.record_comptime_condition(cond, &constraint);
                    return Ok(Some(constraint));
                };
                let mut propositions = Vec::with_capacity(trait_names.len());
                for trait_name in trait_names {
                    let conforms = self
                        .param_context
                        .conforms(dependent.expr(), trait_name)
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
            // e = L[1]`) is no parameter: the condition runs with it. A
            // runtime local compiles to a parameter no binder declares.
            Ok(constraint)
                if self.names_evaluated_binding(&constraint)
                    || self.names_unbound_parameter(&constraint) =>
            {
                self.expect_evaluated_bool(cond, test)?;
                self.evaluated_condition(cond, test).map(Some)
            }
            Ok(constraint) => {
                let constraint = self.fold_closed_constraint(constraint);
                self.record_comptime_condition(cond, &constraint);
                Ok(Some(constraint))
            }
            Err(constraint_error) => match self.expect_evaluated_bool(cond, test) {
                Ok(()) => self.evaluated_condition(cond, test).map(Some),
                // A condition over types or a `TypeList` has no value
                // reading; its constraint diagnosis is the one that names
                // the problem.
                Err(_) if self.condition_is_compile_time_shaped(cond) => Err(constraint_error),
                Err(error) => Err(error),
            },
        }
    }

    /// A leaf of a compile-time test that compiles to no constraint:
    /// `h(n) > 4`, `b`, `Self.flag`, `t[0] > 1`, or `n` over an `Int`. A
    /// `Bool` leaf is the one parameter expression a bracket argument
    /// spelling it is, compared with `True`: built structurally where the
    /// parameter domain expresses it, a call it cannot express applying the
    /// function the check lifts for that call (`lt(2, $c0(n))` for
    /// `abs(n) > 2`), and otherwise the application of the function lifted
    /// for the whole leaf. An integer leaf is its `Bool(x)`, which
    /// `Int.__bool__` spells `x != 0`; any other leaf that converts is the
    /// application of the function lifted for its conversion. The
    /// elaborator decides it per instance.
    fn evaluated_condition(
        &self,
        cond: &Expr,
        test: ComptimeTest,
    ) -> Result<GenericConstraint, TypeError> {
        use mojito_types::ct::CtValue;
        self.reject_dynamic_condition(cond, test)?;
        let ty = self.infer(cond)?;
        let structural = || {
            let _lifting = self.lifting_position();
            self.compile_dependent_ct_expr(cond).ok()
        };
        let operand = |expression: ParamExpr| match expression.as_constant() {
            Some(value) => ConstraintOperand::Value(value.clone()),
            None => expression.as_decl_ref().map_or_else(
                || ConstraintOperand::Expr(expression.clone()),
                |binder| ConstraintOperand::Param(binder.clone()),
            ),
        };
        let integer = matches!(ty.kind(), TyKind::Int | TyKind::UInt | TyKind::IntLiteral);
        let constraint = match ty.kind() {
            TyKind::Bool => {
                structural()
                    .or_else(|| self.lifted_application(cond))
                    .map(|expression| {
                        GenericConstraint::Eq(
                            operand(expression),
                            ConstraintOperand::Value(CtValue::Bool(true)),
                        )
                    })
            }
            _ if integer && let Some(expression) = structural() => Some(GenericConstraint::Ne(
                operand(expression),
                ConstraintOperand::Value(CtValue::Int(0)),
            )),
            _ => self.converted_condition(cond).map(|expression| {
                GenericConstraint::Eq(
                    operand(expression),
                    ConstraintOperand::Value(CtValue::Bool(true)),
                )
            }),
        }
        .ok_or_else(|| {
            TypeError::Unsupported(format!(
                "a {} that compiles to no parameter expression",
                test.context()
            ))
        })?;
        let constraint = self.fold_closed_constraint(constraint);
        self.record_comptime_condition(cond, &constraint);
        Ok(constraint)
    }

    /// The application of the function lifted for the `Bool(x)` conversion
    /// of the condition leaf `cond`, which MIR lowers as the leaf's
    /// truthiness (`comptime if y >= 1.5:` over a `Float32`). A bare name
    /// lifts nothing: the condition's record takes the span its read's
    /// parameter value holds.
    fn converted_condition(&self, cond: &Expr) -> Option<ParamExpr> {
        if matches!(cond.kind, ExprKind::Identifier(_) | ExprKind::Member { .. }) {
            return None;
        }
        let application = self.named_application(cond, &names_read(cond), Ty::bool())?;
        self.lifted_expressions
            .borrow_mut()
            .insert(cond.source_span(), application.clone());
        Some(application)
    }

    /// A condition over no parameter is a fact: the closed verdict of
    /// `constraint`, as the pin folds a constant parameter expression, or
    /// the constraint itself while a leaf stays open for the elaborator.
    fn fold_closed_constraint(&self, constraint: GenericConstraint) -> GenericConstraint {
        if !constraint_is_closed(&constraint) {
            return constraint;
        }
        match self.constraint_verdict(&constraint, &ConstraintEnvironment::default()) {
            Ok(mojito_types::param_expr::ConstraintVerdict::Proven) => {
                GenericConstraint::Bool(true)
            }
            Ok(mojito_types::param_expr::ConstraintVerdict::Disproven) => {
                GenericConstraint::Bool(false)
            }
            _ => constraint,
        }
    }

    /// Require the leaf `cond` of a compile-time test, which the
    /// elaborator runs, to be a `Bool` that calls no raising function: a
    /// raising call (a `raises` `__eq__`) is the pin's rejection.
    fn expect_evaluated_bool(&self, cond: &Expr, test: ComptimeTest) -> Result<(), TypeError> {
        let (checked, raised) = self.observing_raises(|| self.expect_bool(cond, test.context()));
        checked?;
        if raised {
            return Err(TypeError::Unsupported(test.raising_call().to_string()));
        }
        Ok(())
    }

    /// A `comptime if` condition the constraint compiler does not close
    /// runs in a function the elaborator evaluates, so it reads compile-time
    /// bindings alone; one over a runtime local is the pin's rejection.
    fn reject_dynamic_condition(&self, cond: &Expr, test: ComptimeTest) -> Result<(), TypeError> {
        if self.reads_compile_time_alone(cond) {
            return Ok(());
        }
        Err(TypeError::Unsupported(test.dynamic_value().to_string()))
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
                let key = match self.comptime_type_operand(&args[0])?.as_ref().map(Ty::kind) {
                    Some(TyKind::Param { binder, .. }) => AssumedSubject::Binder(binder.id.clone()),
                    Some(TyKind::Dependent(dependent)) => {
                        AssumedSubject::Element(dependent.expr().clone())
                    }
                    _ => return Ok(()),
                };
                proved.extend(
                    trait_names
                        .into_iter()
                        .map(|trait_name| (key.clone(), trait_name.to_string())),
                );
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// Check a `comptime for` by upstream's iterator protocol
    /// ([`Self::check_comptime_iterator_for`]) over its iterable with every
    /// local compile-time value inlined.
    pub(super) fn check_comptime_for(
        &mut self,
        stmt: &Stmt,
        var: &str,
        iter: &Expr,
        body: &[Stmt],
        ret: Option<&Ty>,
    ) -> Result<(), TypeError> {
        let inlined = self.inline_local_comptime_values(iter);
        self.check_comptime_iterator_for(stmt, var, (iter, &inlined), body, ret)
    }

    /// Whether every local `expr` names is a compile-time binding: a value
    /// binder, a `comptime for` variable, or a local `comptime` constant or
    /// display. Such an expression can run in a function the elaborator
    /// evaluates per instance.
    pub(super) fn reads_compile_time_alone(&self, expr: &Expr) -> bool {
        let mut names = names_read(expr);
        // A collector's `args.__len__()` is its pack's length.
        for collector in self.pack_length_receivers(expr) {
            if let Some(position) = names.iter().position(|name| *name == collector) {
                names.swap_remove(position);
            }
        }
        names.iter().all(|name| {
            self.binding_scope(name).is_none_or(|scope| scope == 0)
                || self.is_compile_time_binding(name)
                || self.lookup_owner(name).is_some_and(|owner| {
                    self.value_parameter_owners.contains(&owner)
                        || self.is_comptime_binding_owner(owner)
                })
        })
    }

    /// The collectors `expr` reads only for their pack's length, once per
    /// `args.__len__()` it spells.
    fn pack_length_receivers(&self, expr: &Expr) -> Vec<String> {
        struct Receivers<'c> {
            checker: &'c Checker,
            names: Vec<String>,
        }
        impl mojito_ast::visit::Visitor for Receivers<'_> {
            fn visit_expr(&mut self, expr: &Expr) {
                if let ExprKind::MethodCall { object, .. } = &expr.kind
                    && let ExprKind::Identifier(name) = &object.kind
                    && self.checker.pack_length_binder(expr).is_some()
                {
                    self.names.push(name.clone());
                }
            }
        }
        let mut receivers = Receivers {
            checker: self,
            names: Vec::new(),
        };
        mojito_ast::visit::walk_expr(&mut receivers, expr);
        receivers.names
    }

    /// [`Self::lifted_application`] where the check stands in a lifting
    /// position ([`Self::lifting_position`]).
    pub(super) fn positioned_application(&self, expr: &Expr) -> Option<ParamExpr> {
        (self.lifting_positions.get() > 0)
            .then(|| self.lifted_application(expr))
            .flatten()
    }

    /// The parameter expression of a compile-time `Int`, `UInt`, `Bool`,
    /// `Float64`, or `String` expression that compiles to none (`h(L[0])`,
    /// `n > 2`, `min(L[0], L[1])`, `half(n)`, `pick(n) + "!"`), written in a
    /// type or a parameter argument or bound by a local `comptime`.
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
        if called.is_none() && !self.reads_compile_time_alone(expr) {
            return None;
        }
        // The expression's own arguments are typed where they stand. One
        // that raises (`M["a"]` over a dictionary) lifts nothing: its
        // binding reports the pin's rejection ([`Self::requested_binding`]).
        let lifting = self.lifting_positions.replace(0);
        let (ty, raised) = self.observing_raises(|| {
            let _position = self.comptime_position();
            self.infer(expr)
        });
        self.lifting_positions.set(lifting);
        if raised {
            return None;
        }
        let ty = ty.ok()?;
        let ty = match ty.kind() {
            TyKind::Int | TyKind::IntLiteral => Ty::int(),
            TyKind::UInt => Ty::uint(),
            TyKind::Bool => Ty::bool(),
            TyKind::Float64 | TyKind::FloatLiteral => Ty::float64(),
            // A struct-valued call is an application the elaborator below MIR
            // runs and materializes, and so is any `String` expression, which it
            // freezes to a compile-time string; no other struct expression
            // lifts.
            TyKind::Struct(..) if called.is_some() => ty,
            TyKind::Struct(name, args)
                if args.is_empty() && mojito_types::types::is_stdlib_string_struct(name) =>
            {
                ty
            }
            _ => return None,
        };
        let application = match called {
            Some(application) => application,
            None => self.named_application(expr, &names, ty)?,
        };
        self.lifted_expressions
            .borrow_mut()
            .insert(expr.source_span(), application.clone());
        Some(application)
    }

    /// Record on `expr`, a compile-time `Int` or `Bool` expression of a body,
    /// the parameter expression the check compiled it to when that applies
    /// a function (`add(list_length(L), 1)` for `len(L) + 1`, `lt(4, h(n))`
    /// for `h(n) > 4`): MIR reads it wherever the expression's value is needed, as
    /// upstream reads sema's `kgen.param.constant`, and lifts nothing of its
    /// own. Arithmetic over binders alone (`n + 1`, a `comptime for`
    /// variable's `paramfor_next_value` among them) MIR builds from the
    /// expression's node. A lifted function's application is recorded only
    /// where the function is lifted ([`Self::lifted_application`]): MIR
    /// lifts it from the node that carries it.
    pub(super) fn record_compiled_expression(&self, expr: &Expr, expression: &ParamExpr) {
        if let mojito_types::param_expr::ParamKind::Apply { function, .. } = expression.kind()
            && mojito_symbol::symbol::lifted_owner(function).is_some()
        {
            return;
        }
        let stub = super::builtin_stub("");
        let mut applies = false;
        expression.visit(&mut |node| {
            applies |= matches!(
                node.kind(),
                mojito_types::param_expr::ParamKind::Apply { function, .. }
                    if !function.starts_with(&stub)
            );
        });
        let scalar = expression
            .meta()
            .as_value()
            .is_some_and(|ty| matches!(ty.kind(), TyKind::Int | TyKind::Bool));
        if self.scopes.len() < 2 || !applies || !scalar || expression.as_constant().is_some() {
            return;
        }
        self.lifted_expressions
            .borrow_mut()
            .insert(expr.source_span(), expression.clone());
    }

    /// The application a call of a module function or a static method
    /// denotes when every argument is a compile-time `Int`, `UInt`, `Bool`,
    /// or `Float64` (`h(n)`, `h(Self.n + 1)`, `twice[n]()`, `S.f(n)`, `d(n, m=4)`): the
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
        let result = self.ty_from_anno(&function.result).ok().filter(|ty| {
            matches!(
                ty.kind(),
                TyKind::Int | TyKind::UInt | TyKind::Bool | TyKind::Float64 | TyKind::Struct(..)
            )
        })?;
        (!function.raises).then(|| {
            self.param_context.apply(
                &function.symbol,
                &arguments,
                mojito_types::param_expr::MetaTy::value(result),
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
                |ty| matches!(ty.kind(), TyKind::Struct(owner, _) if applicable(template_name(owner))),
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
        match instance.kind() {
            TyKind::Struct(owner, _) => Some((template_name(owner).to_string(), instance.clone())),
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
    pub(super) fn named_application(
        &self,
        expr: &Expr,
        names: &[String],
        ty: Ty,
    ) -> Option<ParamExpr> {
        // The function is over the binders the expression reads, so that it
        // is one function under a `comptime for` and outside it.
        let mut binders = self.binders_in_scope();
        if let Some(mut read) = self.binders_read(expr, names) {
            // A binder typed over another (a loop's iterator over `T`)
            // brings that one too.
            loop {
                let typed_over: Vec<_> = binders
                    .iter()
                    .filter_map(ParamExpr::as_decl_ref)
                    .filter(|candidate| {
                        !read.contains(&candidate.id)
                            && binders.iter().any(|kept| {
                                kept.as_decl_ref()
                                    .is_some_and(|kept| read.contains(&kept.id))
                                    && meta_names_binder(kept.meta(), candidate)
                            })
                    })
                    .map(|candidate| candidate.id.clone())
                    .collect();
                if typed_over.is_empty() {
                    break;
                }
                read.extend(typed_over);
            }
            binders.retain(|binder| {
                binder
                    .as_decl_ref()
                    .is_some_and(|reference| read.contains(&reference.id))
            });
        }
        let bindings: Vec<_> = names.iter().map(|name| self.lookup_owner(name)).collect();
        let owner = self.lifted_function_owner();
        let known = self
            .lifted_applications
            .borrow()
            .iter()
            .find(|known| {
                known.owner == owner
                    && known.expression == *expr
                    && known.binders == binders
                    && known.bindings == bindings
                    && known.application.meta().as_value() == Some(&ty)
            })
            .map(|known| known.application.clone());
        known.or_else(|| {
            let name = self.lifted_function_name(&owner);
            let application = self.param_context.apply(
                &name,
                &binders,
                mojito_types::param_expr::MetaTy::value(ty),
            );
            self.lifted_applications
                .borrow_mut()
                .push(LiftedApplication {
                    owner,
                    expression: expr.clone(),
                    binders,
                    bindings,
                    application: application.clone(),
                });
            Some(application)
        })
    }

    /// The declaration that owns a function the check lifts here: the one
    /// whose body is being checked, or `__toplevel__` at module scope.
    fn lifted_function_owner(&self) -> String {
        self.lifting_owner
            .clone()
            .unwrap_or_else(|| "__toplevel__".to_string())
    }

    /// A new function `owner` lifts, named by `owner` and the count of
    /// those it lifted before, so the name is the same in every
    /// compilation.
    fn lifted_function_name(&self, owner: &str) -> String {
        let mut counts = self.lifted_counts.borrow_mut();
        let index = counts.entry(owner.to_string()).or_default();
        let name = mojito_symbol::symbol::lifted_check_name(owner, *index);
        *index += 1;
        name
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
            match type_parameter.map(Ty::kind) {
                Some(TyKind::Param { binder, .. }) => {
                    read.insert(binder.id.clone());
                    continue;
                }
                Some(_) => return None,
                None => {}
            }
            let denoted = match self.display_named(name) {
                Some(display) => Some(display.sequence.clone()),
                None => self
                    .value_parameter_in_scope(name)
                    .or_else(|| self.pack_parameter_in_scope(name)),
            };
            match denoted {
                Some(denoted) => denoted.visit(&mut |node| match node.kind() {
                    mojito_types::param_expr::ParamKind::DeclRef(reference)
                    | mojito_types::param_expr::ParamKind::PackQuery {
                        pack: reference, ..
                    } => {
                        read.insert(reference.id.clone());
                    }
                    _ => {}
                }),
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
        for (span, application) in self.lifted_expressions.borrow().iter() {
            let mut record = self.record_at(span.clone());
            let operation = match record.adjustment.take().map(|operation| *operation) {
                Some(SemanticAdjustment::ComptimeApplication { operation, .. }) => operation,
                operation => operation.map(Box::new),
            };
            record.adjustment = Some(Box::new(SemanticAdjustment::ComptimeApplication {
                operation,
                application: application.clone(),
            }));
        }
    }

    /// Whether `element` is a nominal `Tuple`, or a non-generic fieldwise
    /// struct, whose elements or fields are numbers, booleans, strings, or
    /// such aggregates: a parameter value a loop binder holds, which a
    /// backend materializes as a closed constant, or constructs where a
    /// string leaf makes it no constant.
    fn parameter_aggregate_element(&self, element: &Ty) -> bool {
        let leaf = |ty: &Ty| {
            matches!(
                ty.kind(),
                TyKind::Int
                    | TyKind::Float64
                    | TyKind::Bool
                    | TyKind::IntLiteral
                    | TyKind::FloatLiteral
                    | TyKind::StringLiteral
            ) || matches!(ty.kind(), TyKind::Struct(name, args)
                if args.is_empty() && mojito_types::types::is_stdlib_string_struct(name))
                || self.parameter_aggregate_element(ty)
        };
        if let Some(elements) = mojito_types::types::tuple_elements(element) {
            return !elements.is_empty() && elements.into_iter().all(leaf);
        }
        matches!(element.kind(), TyKind::Struct(name, args) if args.is_empty()
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

    /// The value-parameter scope beside the innermost open type-parameter
    /// scope. A level past it is dead: `tparams.pop()` closes both.
    pub(super) fn innermost_value_scope(&mut self) -> Option<&mut HashMap<String, ParamExpr>> {
        self.tparams
            .len()
            .checked_sub(1)
            .and_then(|level| self.vparams.get_mut(level))
    }

    /// What a `comptime NAME = value` with no binders in scope denotes
    /// where it is read at compile time: its parameter expression, or the
    /// application of the function the check lifts for its initializer. A
    /// module constant's lifted function is named by the constant
    /// (`$comptime$NAME$module`), the same in every pass, so a display
    /// constant is one application, evaluated once, whatever reads it.
    /// `None` when the initializer denotes nothing.
    pub(super) fn binderless_comptime_expression(
        &self,
        name: &str,
        value: &Expr,
        scope: ConstantScope<'_>,
    ) -> Result<Option<ParamExpr>, TypeError> {
        // A closed module constant is the attribute its declaration folded
        // to, which every read denotes, and a type-valued one is an alias:
        // neither requests anything.
        if matches!(scope, ConstantScope::Module(_))
            && (self.closed_module_attribute(value).is_some()
                || self
                    .compile_dependent_ct_expr(value)
                    .is_ok_and(|built| *built.meta() == mojito_types::param_expr::MetaTy::Type))
        {
            return Ok(None);
        }
        if let Some(expression) = self.comptime_value_expression(value) {
            return Ok(Some(expression));
        }
        Ok(self
            .requested_binding(value, scope)?
            .map(|application| self.module_lifted_application(name, value, application)))
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
        // A string literal's KGEN string, or a closed raw list read off a
        // struct's parameter, is compile-time only before any probe below
        // types it as a template's expression.
        if self.reads_raw_list(value) {
            self.check_raw_list_annotation(name, annotation, value)?;
            return self.bind_compile_time_only_value(name, value);
        }
        if self.reads_literal_value(value) {
            return self.bind_compile_time_only_value(name, value);
        }
        // A reflected field type (`comptime F = r.field_at[1].T`) is a type
        // alias where a type is read, and the `AnyType` value it spells
        // where a binder takes it.
        if !self.function_bases.is_empty()
            && let Some(ty) = self.reflected_type_operand(value)?
        {
            self.bind_local_type_alias(name, ty, value)?;
            self.local_comptime_values
                .last_mut()
                .ok_or_else(|| {
                    TypeError::InvariantViolation("checker scope stack is empty".to_string())
                })?
                .insert(name.to_string(), value.clone());
            return Ok(true);
        }
        // A module generator constant is bound ahead of its statement, or,
        // reading a struct's member, by it.
        if self.function_bases.is_empty() && self.scopes.len() == 1 {
            if matches!(self.module_values.get(name), Some(CtValue::Generator(_))) {
                return Ok(true);
            }
            if let Ok(generator @ CtValue::Generator(_)) = self.eval_associated_ct(value) {
                self.module_values.insert(name.to_string(), generator);
                return Ok(true);
            }
        }
        // A parameterized declaration read unapplied (`comptime g = T.get`,
        // `comptime t = Twice`) is a generator, compile-time data with no
        // runtime form: the binding is its parameter expression alone.
        if !self.function_bases.is_empty()
            && let Ok(generator) = self.compile_dependent_ct_expr(value)
            && generator
                .meta()
                .as_value()
                .is_some_and(|ty| matches!(ty.kind(), TyKind::Generator(_)))
        {
            if let Some(annotation) = annotation {
                let expected = self
                    .resolve_storage_annotation(annotation, super::StorageStrictness::AllowBare)?;
                let found = generator.meta().to_string();
                if !generator
                    .meta()
                    .is_generator_of(&MetaTy::value(expected.clone()))
                {
                    return Err(TypeError::TypeMismatch {
                        expected: expected.to_string(),
                        found,
                        context: format!("comptime '{name}'"),
                    });
                }
            }
            let level = self.tparams.len().saturating_sub(1);
            self.local_comptime_parameters
                .last_mut()
                .ok_or_else(|| {
                    TypeError::InvariantViolation("checker scope stack is empty".to_string())
                })?
                .insert(name.to_string(), (level, generator));
            return Ok(true);
        }
        let Some(level) = self.tparams.len().checked_sub(1) else {
            if self.bind_compile_time_only_value(name, value)? {
                return Ok(true);
            }
            // A body with no binders keeps a binding only when it applies a
            // callable (`comptime x = f(1)`): its reads in a compile-time
            // position are the application, and the rest of the statement
            // is typed as any binding is.
            let scope = if self.function_bases.is_empty() {
                // A closed constant folds to its attribute where its
                // initializer first builds, which may read a constant bound
                // only by its own statement (`NAMES[1]`).
                if let Some(folded) = self.closed_module_attribute(value)
                    && !self.has_module_constant_entry(name)
                {
                    self.module_values.insert(name.to_string(), folded);
                }
                ConstantScope::Module(annotation)
            } else {
                ConstantScope::Local
            };
            if let Some(expression) = self.binderless_comptime_expression(name, value, scope)?
                && let Some(scope) = self.local_comptime_parameters.last_mut()
            {
                scope.insert(name.to_string(), (0, expression));
            }
            return Ok(false);
        };
        if let Some(ty) = self.comptime_type_operand(value)? {
            self.bind_local_type_alias(name, ty, value)?;
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
        let denoted = self.comptime_value_expression(value);
        if let Some(expression) = denoted.clone() {
            // An annotation names the parameter expression's own type: no
            // conversion applies to one.
            if let (Some(annotation), Some(found)) = (annotation, expression.meta().as_value()) {
                let expected = self
                    .resolve_storage_annotation(annotation, super::StorageStrictness::AllowBare)?;
                // Any other annotation converts the value, as a runtime
                // binding's does (`comptime x: Float64 = i`).
                if &expected != found && !matches!(expected.kind(), TyKind::Int | TyKind::Bool) {
                    return Ok(false);
                }
                if &expected != found {
                    return Err(TypeError::TypeMismatch {
                        expected: expected.to_string(),
                        found: found.to_string(),
                        context: format!("comptime '{name}'"),
                    });
                }
            }
            // A raw parameter list (`comptime raw = vs.values`) is
            // compile-time data with no runtime form: the binding is its
            // parameter expression alone, which every read names.
            let raw_list = self.value_pack_named(value).is_none()
                && expression.as_decl_ref().is_some()
                && matches!(
                    expression.meta(),
                    mojito_types::param_expr::MetaTy::ParamList(_)
                );
            if let Some(scope) = self.local_comptime_parameters.last_mut() {
                scope.insert(name.to_string(), (level, expression));
            }
            if raw_list {
                return Ok(true);
            }
        }
        if let Some((element, ty, construction)) = self.evaluated_display_binding(value) {
            // The name is typed for the compile-time expressions that read
            // it, and its statement binding is the one a lifted function
            // binds the display to.
            self.declare_immutable(name, ty.clone())?;
            self.record_statement_binding(stmt, name);
            let display = self.record_display_binding(name, value, &element, ty, construction)?;
            self.local_comptime_displays
                .last_mut()
                .ok_or_else(|| {
                    TypeError::InvariantViolation("checker scope stack is empty".to_string())
                })?
                .insert(name.to_string(), display);
            return Ok(true);
        }
        if self.bind_compile_time_only_value(name, value)? {
            return Ok(true);
        }
        // A closed binding is the application it requests, else the
        // constant it builds to (`comptime s = "Lo"`), which an annotation
        // would convert.
        let expression = match denoted {
            Some(_) => None,
            None => self
                .requested_binding(value, ConstantScope::Local)?
                .or_else(|| {
                    annotation
                        .is_none()
                        .then(|| self.closed_constant_expression(value))
                        .flatten()
                }),
        };
        if let Some(expression) = expression
            && let Some(scope) = self.local_comptime_parameters.last_mut()
        {
            scope.insert(name.to_string(), (level, expression));
        }
        Ok(false)
    }

    /// Require a raw list binding's annotation, if any, to name the list's
    /// type (`comptime raw: _MLIR.KGENParamListType[Int] =
    /// ParameterList.of[4, 5].values`).
    fn check_raw_list_annotation(
        &self,
        name: &str,
        annotation: Option<&SourceType>,
        value: &Expr,
    ) -> Result<(), TypeError> {
        let (Some(annotation), Some(found)) = (annotation, self.member_value_type(value)) else {
            return Ok(());
        };
        let expected =
            self.resolve_storage_annotation(annotation, super::StorageStrictness::AllowBare)?;
        if coerces(&found, &expected) || self.param_list_converts(&found, &expected) {
            return Ok(());
        }
        Err(TypeError::TypeMismatch {
            expected: expected.to_string(),
            found: found.to_string(),
            context: format!("comptime '{name}'"),
        })
    }

    /// Bind a reflection handle (`reflect[T]`, `r.field["x"]`), a reflected
    /// list (`r.field_names()`), a
    /// type list computed from the body's packs
    /// (`TypeList._concat[Self.Ts.values, OtherTs.values]()`), a string
    /// literal's KGEN string (`"None".value`), or a closed raw list
    /// (`ParameterList.of[4, 5].values`): a compile-time-only value inlined
    /// at its uses. `false` when `value` is none of these.
    fn bind_compile_time_only_value(
        &mut self,
        name: &str,
        value: &Expr,
    ) -> Result<bool, TypeError> {
        // A handle binds as written: a bad selection
        // (`reflect[P].field["missing"]`) is reported where it is read, as
        // the pin runs an unread binding.
        if !(super::reflection::spells_reflection_handle(value)
            || matches!(self.reflection_handle(value), Ok(Some(_)))
            || matches!(self.reflection_list(value), Ok(Some(_)))
            || matches!(self.type_list_operand(value), Ok(Some(_)))
            || self.reads_literal_value(value)
            || self.reads_raw_list(value))
        {
            return Ok(false);
        }
        self.local_comptime_values
            .last_mut()
            .ok_or_else(|| {
                TypeError::InvariantViolation("checker scope stack is empty".to_string())
            })?
            .insert(name.to_string(), value.clone());
        Ok(true)
    }

    /// The application a local `comptime` binding of a body denotes when
    /// its initializer applies a callable at a type no parameter expression
    /// spells (`comptime s = label(3)`, a `String`): the function MIR lifts
    /// for it, at the initializer's own type, which the elaborator below MIR
    /// demands and runs once per instance. A module constant's collection
    /// display, and a dictionary or set constructor call in either scope, is
    /// one too ([`super::lifts_comptime_value`]). `None` for any other
    /// initializer.
    fn requested_binding(
        &self,
        value: &Expr,
        scope: ConstantScope<'_>,
    ) -> Result<Option<ParamExpr>, TypeError> {
        let (lifted, annotation) = match scope {
            // A module constant the one builder does not fold closed
            // (`max(3, 4)`) is the application its initializer computes.
            ConstantScope::Module(annotation) => (
                super::lifts_comptime_value(value, self)
                    || (self.calls_builtin_function(value)
                        && self.closed_module_constant(value).is_none()),
                annotation,
            ),
            // A local display keeps its display route
            // (`evaluated_display_binding`); a constructor call is lifted.
            ConstantScope::Local => (
                matches!(value.kind, ExprKind::Call { .. })
                    && super::lifts_comptime_value(value, self),
                None,
            ),
        };
        if !(lifted || self.applies_callable(value)) {
            return Ok(None);
        }
        // A display takes its annotation as its contextual type, as a `var`
        // initializer does.
        let expected = annotation
            .filter(|_| matches!(value.kind, ExprKind::ListLit(_) | ExprKind::BraceLit(_)))
            .map(|annotation| {
                self.resolve_storage_annotation(annotation, super::StorageStrictness::AllowBare)
            })
            .transpose()?;
        if let ExprKind::Call { name, .. } = &value.kind
            && self
                .lookup(name)
                .and_then(Ty::callable)
                .is_some_and(|func| func.raises)
        {
            return Err(TypeError::Unsupported(
                "cannot call raising function in comptime initializer".to_string(),
            ));
        }
        if !self.reads_compile_time_alone(value) {
            return Err(TypeError::Unsupported(
                "cannot use a dynamic value in comptime initializer".to_string(),
            ));
        }
        // A raising operation anywhere in it (`{"a": 1}["a"]`) is the
        // pin's rejection too.
        let (ty, raised) = self.observing_raises(|| {
            let _position = self.comptime_position();
            match &expected {
                Some(expected) => self.infer_with_expected(value, expected, true),
                None => self.infer(value),
            }
        });
        if raised {
            return Err(TypeError::Unsupported(
                "cannot call raising function in comptime initializer".to_string(),
            ));
        }
        let ty = mojito_types::types::default_literal(&ty?);
        self.copy_compile_time_result(value, &ty);
        let Some(application) = self.named_application(value, &names_read(value), ty) else {
            return Ok(None);
        };
        self.lifted_expressions
            .borrow_mut()
            .insert(value.source_span(), application.clone());
        Ok(Some(application))
    }

    /// The attribute a closed module constant other than a collection
    /// display folds to ([`Self::closed_module_constant`]); a display is
    /// the application the check lifts.
    fn closed_module_attribute(&self, value: &Expr) -> Option<CtValue> {
        if super::lifts_comptime_value(value, self) {
            return None;
        }
        self.closed_module_constant(value)
    }

    /// Whether `value` calls a builtin function the check types itself
    /// (`max(3, 4)`, `abs(n)`): an application the elaborator does not
    /// fold, as upstream's interpreter runs the library function. The
    /// queries the elaborator folds (`len`, `range`, `reflect`,
    /// `conforms_to`, `materialize`) are not.
    fn calls_builtin_function(&self, value: &Expr) -> bool {
        struct Finder<'c> {
            checker: &'c Checker,
            found: bool,
        }
        impl mojito_ast::visit::Visitor for Finder<'_> {
            fn visit_expr(&mut self, expr: &Expr) {
                if let ExprKind::Call { name, .. } = &expr.kind {
                    self.found |= !matches!(
                        name.as_str(),
                        "len" | "range" | "reflect" | "conforms_to" | "materialize"
                    ) && self.checker.binding_scope(name).is_none()
                        && super::CalleeOracle::callee(self.checker, name) == super::Callee::Other;
                }
            }
        }
        let mut finder = Finder {
            checker: self,
            found: false,
        };
        mojito_ast::visit::walk_expr(&mut finder, value);
        finder.found
    }

    /// Run `check`, reporting whether it typed a raising operation that no
    /// `try` inside it handles.
    fn observing_raises<T>(&self, check: impl FnOnce() -> T) -> (T, bool) {
        self.raise_observation_frames
            .borrow_mut()
            .push((self.handled_raise_depth, false));
        let checked = check();
        let raised = self
            .raise_observation_frames
            .borrow_mut()
            .pop()
            .is_some_and(|(_, raised)| raised);
        (checked, raised)
    }

    /// Whether `value` applies a callable ([`super::applies_callable`]), as
    /// the elaborator keeps a binding of one for the check, or reads a
    /// module constant or a local display binding that does. A module
    /// constant folded to a scalar or a string is an attribute, which
    /// applies nothing.
    fn applies_callable(&self, value: &Expr) -> bool {
        super::applies_callable(value, self)
            || names_read(value).iter().any(|name| {
                (self.binding_scope(name) == Some(0)
                    && self.module_constant_value(name).is_some_and(|expression| {
                        !expression.as_constant().is_some_and(is_scalar_attribute)
                    }))
                    || self.display_named(name).is_some()
            })
    }

    /// `application`, the function lifted for the module constant `name`'s
    /// initializer `value`, renamed by the constant: a body's facts carried
    /// from an earlier pass name it, and MIR lifts it from the toplevel
    /// under that name. A function-local binding keeps its declaration's
    /// name.
    fn module_lifted_application(
        &self,
        name: &str,
        value: &Expr,
        application: ParamExpr,
    ) -> ParamExpr {
        if !self.function_bases.is_empty() {
            return application;
        }
        let mojito_types::param_expr::ParamKind::Apply { args, .. } = application.kind() else {
            return application;
        };
        // A display constant is a parameter list, which a subscript or a
        // loop header reads element by element; one its annotation types as
        // another collection (`comptime L: List[Int] = [1, 2]`) is that
        // collection's value.
        let annotated = matches!(application.meta().as_value().map(Ty::kind),
            Some(TyKind::Struct(collection, _)) if collection != mojito_types::types::ARRAY_TYPE_NAME);
        let meta = match &value.kind {
            ExprKind::ListLit(_) | ExprKind::TupleLit(_) if !annotated => {
                self.display_listed_element(value).map_or_else(
                    || application.meta().clone(),
                    |element| {
                        mojito_types::param_expr::MetaTy::ParamList(Box::new(
                            mojito_types::param_expr::MetaTy::value(string_element_binder(element)),
                        ))
                    },
                )
            }
            _ => application.meta().clone(),
        };
        let renamed = self.param_context.apply(
            &mojito_symbol::symbol::lifted_constant_name(name),
            args,
            meta,
        );
        self.lifted_expressions
            .borrow_mut()
            .insert(value.source_span(), renamed.clone());
        renamed
    }

    /// What the value of a local `comptime` binding over a template body's
    /// binders denotes wherever the body reads it at compile time: its
    /// parameter expression, or the application of the function lifted for
    /// an `Int` or a `Bool` that compiles to none (`comptime e = h(n)`).
    /// `None` for a constant, or a value that denotes nothing.
    fn comptime_value_expression(&self, value: &Expr) -> Option<ParamExpr> {
        let _lifting = self.lifting_position();
        let expression = self
            .compile_dependent_ct_expr(value)
            .ok()
            .or_else(|| self.lifted_application(value))
            .filter(|expression| expression.as_constant().is_none())?;
        self.record_compiled_expression(value, &expression);
        Some(expression)
    }

    /// The constant value a closed local `comptime` binding builds to
    /// (`"Lo"`, `2.5`, `n > 2` over a closed `n`). `None` for a type, and
    /// for an expression the builder does not fold.
    fn closed_constant_expression(&self, value: &Expr) -> Option<ParamExpr> {
        self.compile_dependent_ct_expr(value)
            .ok()
            .filter(|expression| {
                expression.as_constant().is_some() && expression.meta().as_value().is_some()
            })
    }

    /// Type the display a local `comptime` binding of a template body holds
    /// (`comptime L = [n, n + 1]`): a collection display of scalar elements,
    /// or a list display of closed aggregates (`[(1, n), (n, 2)]`),
    /// which the elaborator evaluates per instance. Gives the element its
    /// sequence lists, the display's own type, and its checked construction;
    /// `None` for any other binding, which the ordinary path types.
    fn evaluated_display_binding(
        &self,
        value: &Expr,
    ) -> Option<(Ty, Ty, mojito_checked::checked::SemanticAdjustment)> {
        let element = self.display_listed_element(value)?;
        if !self.evaluated_display(value, &element) {
            return None;
        }
        let ty = self.infer(value).ok()?;
        let construction = self.fact(&value.source_span(), |facts| facts.adjustment.as_deref())?;
        Some((string_element_binder(element), ty, construction))
    }

    /// Record the display the declared local `comptime` binding `name` holds
    /// for MIR to lift (`SemanticAdjustment::ComptimeDisplay`). It is a
    /// compile-time value with no runtime form: a type or a parameter
    /// argument reads an element or the length as a parameter expression
    /// over its sequence, any other compile-time expression (a `comptime
    /// for`'s `L.__iter__()` among them) reads it in a function the
    /// elaborator runs, and
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
        element: &Ty,
        ty: Ty,
        construction: mojito_checked::checked::SemanticAdjustment,
    ) -> Result<BoundDisplay, TypeError> {
        let owner = self.lookup_owner(name).ok_or_else(|| {
            TypeError::InvariantViolation(format!("comptime binding '{name}' has no owner"))
        })?;
        let sequence = self.display_sequence(owner, element);
        self.record_at(value.source_span()).adjustment = Some(Box::new(
            mojito_checked::checked::SemanticAdjustment::ComptimeDisplay {
                construction: Box::new(construction),
                sequence: sequence.clone(),
            },
        ));
        Ok(BoundDisplay {
            ty,
            sequence,
            positional: matches!(value.kind, ExprKind::ListLit(_)),
        })
    }

    /// The sequence the display binding `identity` denotes, a list of
    /// `element`s: the application of a function its declaration lifts to
    /// every binder in scope, the one the binding was first given.
    fn display_sequence(&self, identity: mojito_types::origin::OwnerId, element: &Ty) -> ParamExpr {
        use mojito_types::param_expr::MetaTy;
        let owner = self.lifted_function_owner();
        let key = (owner, identity);
        if let Some(sequence) = self.lifted_displays.borrow().get(&key) {
            return sequence.clone();
        }
        let sequence = self.param_context.apply(
            &self.lifted_function_name(&key.0),
            &self.binders_in_scope(),
            MetaTy::ParamList(Box::new(MetaTy::value(element.clone()))),
        );
        self.lifted_displays
            .borrow_mut()
            .insert(key, sequence.clone());
        sequence
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
                } if !matches!(ty.kind(), TyKind::Func(_) | TyKind::GenericFunc(_)) => {
                    Some(super::annotations::value_parameter_expr(decl, ty))
                }
                ParamDecl::Value { .. } => None,
            })
            .collect();
        for (level, types) in self.tparams.iter().enumerate() {
            let packs = self.pack_params.get(level);
            let mut types: Vec<ParamExpr> = types
                .values()
                .filter_map(|ty| match ty.kind() {
                    TyKind::Param { binder, .. } => Some(
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
        if !self.crosses_to_runtime() {
            return Ok(());
        }
        // A field read of a compile-time value is a projection, as at the
        // pin: the value itself never crosses.
        if self.projected_object.borrow().as_ref() == Some(&expr.source_span()) {
            return Ok(());
        }
        if let Some(display) = self.bound_display(expr) {
            return Err(TypeError::ComptimeCrossing(
                materialized_collection_spelling(&display.ty),
            ));
        }
        // A collection a request builds (`comptime l = mk(n)`) is never
        // implicitly copyable: it has no runtime form to read.
        let ExprKind::Identifier(name) = &expr.kind else {
            return Ok(());
        };
        // A display constant the check lifted is a parameter list, read
        // whole at its declared type.
        let requested = self
            .local_comptime_parameters
            .iter()
            .rev()
            .find_map(|scope| scope.get(name))
            .or_else(|| self.installed_comptime_parameter(name))
            .and_then(|(_, expression)| match expression.meta() {
                mojito_types::param_expr::MetaTy::ParamList(_) => self.lookup(name).cloned(),
                meta => meta.as_value().cloned(),
            })
            .filter(|ty| {
                self.lookup(name) == Some(ty)
                    && (mojito_types::types::list_element(ty).is_some()
                        || mojito_types::types::set_element(ty).is_some()
                        || mojito_types::types::dict_elements(ty).is_some()
                        || !self.is_implicitly_copyable(ty))
            });
        match requested {
            Some(ty) => Err(TypeError::ComptimeCrossing(
                materialized_collection_spelling(&ty),
            )),
            None => Ok(()),
        }
    }

    /// The type an expression denotes in a compile-time position, as the
    /// one builder reads it ([`Self::type_operand_expression`]); `None` for
    /// a value, or for an expression that denotes nothing at compile time.
    /// An unknown type argument (`List[Foo]`) is the diagnostic.
    pub(super) fn comptime_type_operand(&self, expr: &Expr) -> Result<Option<Ty>, TypeError> {
        match &expr.kind {
            // A reflection handle is a compile-time value, not a type, and a
            // `comptime` alias or a trivial predicate applies to a `Bool`.
            ExprKind::TypeApply { name, .. }
                if name == "reflect"
                    || self.comptime_aliases.contains_key(name)
                    || mojito_types::types::trivial_predicate_name(name).is_some() =>
            {
                Ok(None)
            }
            _ if let Some(reflected) = self.reflected_type_operand(expr).transpose() => {
                reflected.map(Some)
            }
            _ => match self.type_operand_expression(expr) {
                Ok(Some(ty)) => Ok(Some(ty)),
                // An element of a compile-time `TypeList` value past its
                // end is the read's own failure.
                Err(error)
                    if matches!(error, TypeError::UnknownType(_))
                        || matches!(
                            expr.kind,
                            ExprKind::TypeValue(_) | ExprKind::TypeApply { .. }
                        )
                        || matches!(&expr.kind, ExprKind::Index { object, .. }
                            if matches!(self.type_list_operand(object), Ok(Some(_)))) =>
                {
                    Err(error)
                }
                Ok(None) | Err(_) => Ok(None),
            },
        }
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
                        self.record_at(span).adjustment = Some(Box::new(
                            mojito_checked::checked::SemanticAdjustment::ConstructPackElement {
                                pack: pack.clone(),
                            },
                        ));
                        self.record_element_index(index);
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

    /// Record the parameter expression a pack element construction's index
    /// denotes on the index (`i` of `Ts[i]()`, a local `comptime` binding,
    /// or the alias a `comptime for` over an iterator binds), the element
    /// the elaborator selects per instance.
    fn record_element_index(&self, index: &Expr) {
        let Ok(value) = self.compile_dependent_ct_expr(index) else {
            return;
        };
        let mut record = self.record_at(index.source_span());
        if record.adjustment.is_none() && value.as_constant().is_none() {
            record.adjustment = Some(Box::new(
                mojito_checked::checked::SemanticAdjustment::ParamValue {
                    value,
                    materialized: None,
                },
            ));
        }
    }

    /// `materialize[X]()` in the executable check: the crossing pass folds
    /// every operand it evaluates, so `X` is over a binder of a template
    /// body, and the call is the runtime value of that operand at its type.
    /// A whole field-name list (`materialize[names]()`) is the `Array` the
    /// elaborator constructs per instance.
    pub(super) fn infer_template_materialize(
        &self,
        span: &SourceSpan,
        param_args: &[ParamArg],
    ) -> Result<Ty, TypeError> {
        let [ParamArg::Value(operand)] = param_args else {
            return Err(TypeError::Unsupported(
                "materialize[...]() takes one compile-time value".to_string(),
            ));
        };
        if let Some(ty) = self.materialized_reflection_list(span, operand)? {
            return Ok(ty);
        }
        self.record_crossing_operand(operand);
        let _position = self.comptime_position();
        self.infer(operand)
    }

    /// Record the parameter expression a crossing's compile-time operand
    /// compiles to (`list_get(L, 0)` for `comptime(L[0])`,
    /// [`Self::record_compiled_expression`]), which the elaborator
    /// evaluates; a display crossing whole is built where it crosses.
    /// Whether the operand compiled.
    fn record_crossing_operand(&self, operand: &Expr) -> bool {
        if self.bound_display(operand).is_some() {
            return false;
        }
        let compiled = {
            let _lifting = self.lifting_position();
            self.compile_dependent_ct_expr(operand).ok()
        };
        compiled
            .map(|expression| self.record_compiled_expression(operand, &expression))
            .is_some()
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
        // An operand that applies a callable and compiles to no parameter
        // expression is the application MIR lifts, which the elaborator
        // below MIR evaluates.
        if !self.record_crossing_operand(operand)
            && self.applies_callable(operand)
            && self.lifted_application(operand).is_none()
        {
            self.requested_binding(operand, ConstantScope::Local)?;
        }
        let ty = {
            let _position = self.comptime_position();
            self.infer(operand)?
        };
        if let Some(display) = self.bound_display(operand) {
            return Err(TypeError::ComptimeCrossing(
                materialized_collection_spelling(&display.ty),
            ));
        }
        // The runtime value of a compile-time operand is its implicit copy,
        // as the pin materializes one: a collection or a struct that is
        // not `ImplicitlyCopyable` does not cross.
        let resolved = mojito_types::types::default_literal(&ty);
        if self.crosses_to_runtime()
            && !mojito_types::types::is_symbolic(&resolved)
            && !self.is_implicitly_copyable(&resolved)
        {
            return Err(TypeError::ComptimeCrossing(
                materialized_collection_spelling(&resolved),
            ));
        }
        Ok(ty)
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

    /// Whether `list` is the parameter-list reference of a variadic pack in
    /// scope ([`Self::pack_reference`] of its spelling).
    pub(super) fn pack_in_scope(&self, list: &ParamExpr) -> bool {
        list.as_decl_ref().is_some_and(|reference| {
            self.pack_reference(&reference.name).is_some_and(|found| {
                found
                    .as_decl_ref()
                    .is_some_and(|found| found.id == reference.id)
            })
        })
    }

    /// The raw list type a type pack's `Ts.values` has, as upstream types
    /// it: a list of the pack's bound used as a type
    /// (`KGENParamList[AnyType]`), or of the trait-valued binder bounding it
    /// (`*Ts: Tr` lists `Tr`). `None` when `pack` names no type pack.
    pub(super) fn type_pack_list_ty(&self, pack: &ParamExpr) -> Option<Ty> {
        let element = match self.type_pack_trait(pack)? {
            CtValue::Expr(reference) => mojito_types::types::DependentType::resolve(reference),
            CtValue::Trait(set) => TyKind::Trait(mojito_types::types::trait_set(set)).into(),
            _ => return None,
        };
        Some(TyKind::ParamList(element).into())
    }

    /// The `Trait` argument of the `TypeList` a type pack is: the
    /// trait-valued binder bounding it (`*Ts: Tr`), else the trait value its
    /// declared bound composes. `None` when `pack` names no type pack.
    pub(super) fn type_pack_trait(&self, pack: &ParamExpr) -> Option<CtValue> {
        let binder = pack.as_decl_ref()?;
        if pack.meta() != &mojito_types::param_expr::MetaTy::type_list() {
            return None;
        }
        if let Some(trait_binder) = self.trait_bounded_binders.get(&binder.id) {
            let reference = self
                .value_parameter_in_scope(&trait_binder.name)
                .filter(|reference| {
                    reference
                        .as_decl_ref()
                        .is_some_and(|reference| reference.id == trait_binder.id)
                })
                .map(CtValue::Expr)
                .or_else(|| self.self_param_value(&trait_binder.name))?;
            return Some(match reference {
                CtValue::Expr(expr) => CtValue::Expr(expr),
                value => CtValue::Expr(self.param_context.constant(value).ok()?),
            });
        }
        let bounds = self
            .tparams
            .iter()
            .flat_map(HashMap::values)
            .find_map(|ty| match ty.kind() {
                TyKind::Param {
                    binder: declared,
                    bounds,
                    ..
                } if declared.id == binder.id => Some(bounds.clone()),
                _ => None,
            })
            .or_else(|| {
                self.self_decls.iter().find_map(|decl| match decl {
                    ParamDecl::Type { id, bounds, .. } if id == &binder.id => Some(bounds.clone()),
                    _ => None,
                })
            })?;
        Some(CtValue::Trait(self.canonical_trait_set(&bounds)))
    }

    /// The bundled `TypeList` a type pack is, as upstream's variadic type
    /// parameter is one: `TypeList[Trait, Ts.values]`. `None` when `pack`
    /// names no type pack.
    pub(super) fn type_pack_type_list(&self, pack: &ParamExpr) -> Option<Ty> {
        let trait_value = self.type_pack_trait(pack)?;
        Some(
            TyKind::Struct(
                TYPE_LIST.to_string(),
                vec![
                    TyArg::Val(trait_value),
                    TyArg::Val(CtValue::Expr(pack.clone())),
                ],
            )
            .into(),
        )
    }

    /// Element `index` of a type pack that is still a parameter, as upstream
    /// reads `Ts[index]`: the bundled `TypeList`'s `__getitem_param__`
    /// applied at the index.
    pub(super) fn type_pack_element(
        &self,
        pack: &ParamExpr,
        index: &ParamExpr,
    ) -> Result<Ty, TypeError> {
        let type_list = self
            .type_pack_type_list(pack)
            .ok_or_else(|| TypeError::InvariantViolation(format!("'{pack}' is not a type pack")))?;
        let index = index
            .as_constant()
            .cloned()
            .unwrap_or_else(|| CtValue::Expr(index.clone()));
        self.associated_type_from_base(&type_list, "__getitem_param__", &[TyArg::Val(index)])
    }

    /// The type pack an expression names by itself while it is still a
    /// parameter: a bare `Ts` of an enclosing `def` or method, or the
    /// enclosing struct's `Self.Ts`. The counterpart of
    /// [`Self::value_pack_named`].
    pub(super) fn type_pack_named(&self, expr: &Expr) -> Option<ParamExpr> {
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
        // A signature binds no local: there the name is the binder alone.
        match self.lookup(name).map(Ty::kind) {
            Some(TyKind::VariadicPack(element))
                if mojito_types::types::pack_spread(std::slice::from_ref(element)).is_none() => {}
            Some(_) => return None,
            None => {}
        }
        self.value_pack_in_scope(name)
    }

    /// The raw parameter list a bare name denotes when it is no runtime
    /// binding and no pack: a `values: KGENParamList[T]` parameter, or a
    /// local `comptime raw = vs.values`.
    pub(super) fn raw_list_named(&self, name: &str) -> Option<ParamExpr> {
        if self.lookup(name).is_some() || self.value_pack_in_scope(name).is_some() {
            return None;
        }
        self.value_parameter_in_scope(name)
            .filter(|raw| matches!(raw.meta(), mojito_types::param_expr::MetaTy::ParamList(_)))
    }

    /// A value pack read as a runtime value (`for v in values`, `var l =
    /// values`, `values[i]` at a runtime index): the zero-sized
    /// `ParameterList[values.values]()` the pin types the read as, spelled in
    /// the read's place and checked as any construction is.
    pub(super) fn infer_value_pack_read(&self, expression: &Expr) -> Result<Ty, TypeError> {
        if !self.structs.contains_key(PARAMETER_LIST) {
            return Err(TypeError::Unsupported(format!(
                "a value pack read at run time needs the bundled '{VARIADICS_MODULE}' module"
            )));
        }
        if !matches!(
            expression.kind,
            ExprKind::Member { .. } | ExprKind::Identifier(_)
        ) {
            return Err(TypeError::InvariantViolation(
                "a value pack read names no pack".to_string(),
            ));
        }
        let pack = super::initializer_list::spelled_node(
            expression,
            mojito_ast::ast::VALUE_PACK_OPERAND_ORDINAL,
            expression.kind.clone(),
            expression.span,
        );
        let raw = super::initializer_list::spelled_node(
            expression,
            mojito_ast::ast::VALUE_PACK_RAW_LIST_ORDINAL,
            ExprKind::Member {
                object: Box::new(pack),
                field: "values".to_string(),
            },
            expression.span,
        );
        let construction = super::initializer_list::spelled_node(
            expression,
            mojito_ast::ast::VALUE_PACK_READ_ORDINAL,
            ExprKind::Call {
                name: PARAMETER_LIST.to_string(),
                param_args: vec![ParamArg::Value(raw)],
                args: Vec::new(),
                kwargs: Vec::new(),
            },
            expression.span,
        );
        let ty = self.infer(&construction)?;
        self.record_spelled_construction(expression, construction, &ty);
        Ok(ty)
    }

    /// The raw parameter list `expr` reads where `len` of it stands for
    /// upstream's `#kgen.param_list.size` attribute: a struct's own
    /// `Self.values` in the bundled `std/builtin/variadics.mojo`, as
    /// `__param_list_address` there stands for its static array.
    pub(super) fn bundled_raw_list_operand(&self, expr: &Expr) -> Option<ParamExpr> {
        let ExprKind::Member { object, field } = &expr.kind else {
            return None;
        };
        let bundled = matches!(&object.kind, ExprKind::Identifier(name) if name == "Self")
            && super::overload_support::is_bundled_variadics_source(
                expr.source_span().source.as_deref(),
            );
        bundled.then(|| self.self_raw_list(field)).flatten()
    }

    /// The raw parameter list a bracket argument names: the enclosing
    /// struct's own `Self.values`, or any compile-time value that is one
    /// (`vs.values`, a `raw: KGENParamList[T]` parameter).
    pub(super) fn raw_list_argument(&self, argument: &ParamArg) -> Option<ParamExpr> {
        match argument {
            ParamArg::Type(SourceType::SelfParam(name)) => self.self_raw_list(name),
            ParamArg::Value(expr) => self.compile_dependent_ct_expr(expr).ok().filter(|raw| {
                raw.as_decl_ref().is_some()
                    && matches!(raw.meta(), mojito_types::param_expr::MetaTy::ParamList(_))
            }),
            _ => None,
        }
    }

    /// The element type and raw list of a `ParameterList` construction a
    /// bracket argument spreads (`*ParameterList[Self.values]()`), which
    /// binds a variadic value parameter to that list whole, as upstream's
    /// `_ParameterListIter[*ParameterList[...]()]` does. `None` for any other
    /// argument.
    pub(super) fn parameter_list_spread(
        &self,
        argument: &ParamArg,
    ) -> Option<Result<(Ty, CtValue), TypeError>> {
        let ParamArg::Value(Expr {
            kind: ExprKind::Spread(operand),
            ..
        }) = argument
        else {
            return None;
        };
        let ExprKind::Call {
            name,
            param_args,
            args,
            kwargs,
        } = &operand.kind
        else {
            return None;
        };
        if name != PARAMETER_LIST || !args.is_empty() || !kwargs.is_empty() {
            return None;
        }
        Some(
            self.applied_type(name, param_args)
                .and_then(|(ty, _)| match ty.kind() {
                    TyKind::Struct(_, arguments) => match arguments.as_slice() {
                        [TyArg::Ty(element), TyArg::Val(raw), ..] => {
                            Ok((element.clone(), raw.clone()))
                        }
                        _ => Err(TypeError::InvariantViolation(format!(
                            "'{ty}' binds no raw parameter list"
                        ))),
                    },
                    _ => Err(TypeError::InvariantViolation(format!(
                        "'{ty}' is not a 'ParameterList'"
                    ))),
                }),
        )
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

    /// The enclosing struct's own value pack or raw parameter list
    /// `Self.<field>`, as the parameter-list reference its binder declares.
    pub(super) fn self_raw_list(&self, field: &str) -> Option<ParamExpr> {
        self.self_decls.iter().find_map(|decl| match decl {
            ParamDecl::Value { ty, .. }
                if decl.name().trim_start_matches('*') == field
                    && matches!(
                        decl.binder_meta(),
                        mojito_types::param_expr::MetaTy::ParamList(_)
                    ) =>
            {
                Some(
                    self.param_context
                        .intern(&super::annotations::value_parameter_expr(decl, ty)),
                )
            }
            _ => None,
        })
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

    /// Record a query of a pack that is still a parameter, read as a runtime
    /// value at `span`: the constant MIR carries and the elaborator folds.
    pub(super) fn record_pack_query_value(
        &self,
        span: SourceSpan,
        pack: &ParamExpr,
        query: mojito_types::param_expr::PackQuery,
    ) {
        if let Some(pack) = pack.as_decl_ref() {
            self.record_at(span).adjustment = Some(Box::new(
                mojito_checked::checked::SemanticAdjustment::ParamValue {
                    value: self.param_context.pack_query(pack, query),
                    materialized: None,
                },
            ));
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
    /// parameter: [`Self::type_pack_element`] at an index that is a
    /// compile-time expression over the parameters and `comptime for`
    /// variables in scope.
    pub(super) fn pack_element_type(&self, pack: &Ty, index: &Expr) -> Result<Ty, TypeError> {
        let TyKind::Param { binder, .. } = pack.kind() else {
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
        self.type_pack_element(&list, &index)
    }

    /// Whether a spread's operand (`*args`, `*args^`) is a binding of a pack
    /// that is still a parameter.
    pub(super) fn spreads_unbound_pack(&self, spread: &Expr) -> bool {
        self.forwarded_pack_operand(spread).is_some()
    }

    /// Note a positional collector declared `var *args` once it is bound.
    pub(super) fn record_owned_pack(
        &mut self,
        name: &str,
        kind: mojito_ast::ast::ParamKind,
        convention: Option<ArgConvention>,
    ) {
        if kind == mojito_ast::ast::ParamKind::Variadic
            && matches!(convention, Some(ArgConvention::Var))
            && let Some(owner) = self.lookup_owner(name)
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
        let Some(TyKind::VariadicPack(element)) = self.lookup(binding).map(Ty::kind) else {
            return None;
        };
        let element = mojito_types::types::pack_spread(std::slice::from_ref(element))?;
        let TyKind::Param { binder, .. } = element.kind() else {
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
        let TyKind::Param {
            bounds: expected, ..
        } = collector.kind()
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
        let retyped = matches!((collector.kind(), forwarded.pack.kind()),
            (TyKind::Param { binder, bounds, .. }, TyKind::Param { binder: own, bounds: own_bounds, .. })
                if binder.id == own.id && !same_bounds(bounds, own_bounds));
        if retyped
            || expected
                .iter()
                .any(|bound| !self.conforms_to(&forwarded.pack, bound))
        {
            let declared = match forwarded.pack.kind() {
                TyKind::Param { bounds, .. } if !bounds.is_empty() => bounds.join(" & "),
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
        self.record_at(argument.source_span()).ty = Some(forwarded.pack.clone());
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
        let Some(TyKind::VariadicPack(element)) = self.lookup(binding).map(Ty::kind) else {
            return None;
        };
        let pack = mojito_types::types::pack_spread(std::slice::from_ref(element))?.clone();
        let boundary = || {
            TypeError::SymbolicBoundary(format!(
                "'{name}' called with a spread of the unbound pack '{binding}'"
            ))
        };
        let constructed = match name {
            "__RuntimeTuple" => Ty::new(TyKind::Tuple(vec![pack])),
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

    /// Close the pack elements a callee's type names (`Self.Ts[index]`)
    /// under a use's arguments: a value argument binds its parameter, a
    /// concrete pack its list, and a pack forwarded as a spread the caller's
    /// own pack. A type with no pack element is returned as it is. `scopes`
    /// pair declaration lists with their arguments; a later scope's binder
    /// shadows an earlier one's of the same spelling, the key the dependent
    /// type's value environment reads.
    pub(super) fn close_pack_elements(&self, ty: Ty, scopes: &[(&[ParamDecl], &[TyArg])]) -> Ty {
        let names_element = |ty: &Ty| {
            matches!(ty.kind(), TyKind::Dependent(dependent) if dependent.pack_element().is_some())
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
                    TyArg::Ty(ty)
                        if let TyKind::Param { binder, .. } = ty.kind()
                            && binder.name.starts_with('*') =>
                    {
                        self.pack_reference(&binder.name)
                            .map(|reference| (name, CtValue::Expr(reference)))
                    }
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

    /// The bounded type parameter an element of an unbound variadic pack, of
    /// a bound's raw-list member (`TypeList[Trait=AnyType, T._types]()[0]`),
    /// or a reflected field type of a symbolic subject, behaves as: the
    /// pack's declared bounds, or the trait the member's declared list holds
    /// (a field type has none), plus every trait a `where` clause guarantees
    /// of the pack's elements or a `comptime if conforms_to` arm proves of
    /// this element. The dependent type stays the element's identity; this
    /// is what its capabilities are read from.
    pub(super) fn opaque_element(&self, ty: &Ty) -> Option<Ty> {
        let TyKind::Dependent(dependent) = ty.kind() else {
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
        let member_traits = list.and_then(|list| self.member_list_traits(list));
        if pack.is_none()
            && member_traits.is_none()
            && !reflected(dependent.expr())
            && !list.is_some_and(reflected)
        {
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
        let mut bounds = match declared.as_ref().map(Ty::kind) {
            Some(TyKind::Param { bounds, .. }) => bounds.clone(),
            _ => member_traits.unwrap_or_default(),
        };
        let spelled = dependent.expr().to_string();
        for (subject, guaranteed) in self.assumed_conformances.borrow().iter().flatten() {
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
        Some(
            TyKind::Param {
                binder: pack_element_view_binder(&name),
                bounds,
                callable_bound: None,
            }
            .into(),
        )
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
                .is_some_and(|owner| self.is_comptime_binding_owner(owner))
                && !self
                    .local_comptime_parameters
                    .iter()
                    .any(|scope| scope.contains_key(name))
                && !self.installs(interface::Table::ComptimeParameters, name);
            if evaluated {
                Err(TypeError::NotComptime(name.to_string()))
            } else {
                Ok(())
            }
        })
        .is_err()
    }

    /// Whether a compiled condition names, as a parameter, a name no binder
    /// in scope declares once its operands are bound.
    fn names_unbound_parameter(&self, constraint: &GenericConstraint) -> bool {
        let bound = |reference: &ParamRef| {
            if reference.is_unbound() {
                Err(())
            } else {
                Ok(reference.clone())
            }
        };
        self.bind_constraint(constraint, &[])
            .try_map(&|operand| match operand {
                ConstraintOperand::Param(reference) => bound(reference).map(|_| operand.clone()),
                _ => Ok(operand.clone()),
            })
            .is_err()
    }

    /// Visit the name of every `Param` operand of a compiled condition,
    /// stopping at the first one `visit` rejects.
    fn each_constraint_parameter(
        constraint: &GenericConstraint,
        visit: &dyn Fn(&str) -> Result<(), TypeError>,
    ) -> Result<(), TypeError> {
        constraint
            .try_map(&|operand| match operand {
                ConstraintOperand::Param(param) => visit(&param.name).map(|()| operand.clone()),
                // An arithmetic operand resolved its names when it was
                // compiled.
                _ => Ok(operand.clone()),
            })
            .map(|_| ())
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

    /// The element a compile-time collection display lists: a dictionary
    /// display's keys, a set display's elements, a list display's elements
    /// (a fixed-size array of literals here) at their default type. `None`
    /// for any other value.
    fn display_listed_element(&self, value: &Expr) -> Option<Ty> {
        match self.infer(value).ok()?.kind() {
            dict if let Some((key, _)) = mojito_types::types::dict_elements(dict) => {
                Some(key.clone())
            }
            set if let Some(element) = mojito_types::types::set_element(set) => {
                Some(element.clone())
            }
            TyKind::Struct(name, args)
                if (name == mojito_types::types::LIST_TYPE_NAME
                    || name == mojito_types::types::ARRAY_TYPE_NAME)
                    && let Some(TyArg::Ty(element)) = args.first() =>
            {
                Some(default_literal(element))
            }
            _ => None,
        }
    }

    /// Substitute the defining expression of every function-local
    /// compile-time value binding named in `expr`.
    fn inline_local_comptime_values(&self, expr: &Expr) -> Expr {
        if self.local_comptime_values.iter().all(HashMap::is_empty)
            && !self.installs_any(interface::Table::ComptimeValues)
        {
            return expr.clone();
        }
        let lookup = |name: &str| self.local_comptime_value(name);
        substitute_identifiers(expr, &lookup)
    }

    /// The element traits of the raw list a bound's value member is declared
    /// as (`comptime _types: _MLIR.KGENParamListType[Copyable]` read as
    /// `T._types`); `None` when `list` reads no such member.
    fn member_list_traits(&self, list: &ParamExpr) -> Option<Vec<String>> {
        let mojito_types::param_expr::ParamKind::TypeMember {
            subject,
            name,
            args,
        } = list.kind()
        else {
            return None;
        };
        let mojito_types::param_expr::ParamKind::TypeShape(subject) = subject.kind() else {
            return None;
        };
        let TyKind::Param { bounds, .. } = subject.kind() else {
            return None;
        };
        let declared = args
            .is_empty()
            .then(|| self.lookup_trait_assoc_value_ty(bounds, name))
            .flatten()?;
        let TyKind::ParamList(element) = declared.kind() else {
            return None;
        };
        match element.kind() {
            TyKind::Trait(traits) => Some(traits.clone()),
            _ => None,
        }
    }
}

/// Whether `constraint` names no parameter: every operand, the list a pack
/// query asks of included, is a value or a concrete type.
fn constraint_is_closed(constraint: &GenericConstraint) -> bool {
    constraint
        .try_map(&|operand| {
            let closed = match operand {
                ConstraintOperand::Param(_)
                | ConstraintOperand::PackLength(_)
                | ConstraintOperand::Expr(_) => false,
                ConstraintOperand::Value(value) => value_is_closed(value),
                ConstraintOperand::Type(ty) => !mojito_types::types::is_symbolic(ty),
            };
            closed.then(|| operand.clone()).ok_or(())
        })
        .is_ok()
}

/// Whether a constant names no parameter: no residual expression, and no
/// symbolic type, an element of a type list included.
fn value_is_closed(value: &mojito_types::ct::CtValue) -> bool {
    use mojito_types::ct::CtValue;
    match value {
        CtValue::Expr(_) => false,
        CtValue::Type(ty) => !mojito_types::types::is_symbolic(ty),
        CtValue::Tuple(elements) => elements.iter().all(value_is_closed),
        _ => true,
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
            TyArg::Ty(ty) => Some(CtValue::Type(ty.clone())),
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

/// The owner of a `comptime for`'s own binder, named by the loop's
/// iterable: each loop owns its binder.
pub(super) fn comptime_for_owner(iter: &Expr) -> String {
    let span = iter.source_span();
    format!(
        "{}{}:{}..{}",
        mojito_types::param_expr::COMPTIME_FOR_OWNER,
        super::overload_support::library_relative(span.source.as_deref().unwrap_or_default()),
        span.span.0,
        span.span.1
    )
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
            SourceType::Int => Some(Ty::int()),
            SourceType::UInt => Some(Ty::uint()),
            SourceType::Bool => Some(Ty::bool()),
            SourceType::Float64 => Some(Ty::float64()),
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
                    [bound] if plain && bound == "Int" => Ty::int(),
                    [bound] if plain && bound == "UInt" => Ty::uint(),
                    [bound] if plain && bound == "Bool" => Ty::bool(),
                    [bound] if plain && bound == "Float64" => Ty::float64(),
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
            result: match self.ret {
                SourceType::Int
                | SourceType::UInt
                | SourceType::Bool
                | SourceType::Float64
                | SourceType::Named(..) => self.ret.clone(),
                _ => return None,
            },
            raises: self.raises,
        })
    }
}

/// The name a struct was declared under, whichever instance clone `name`
/// spells.
fn template_name(name: &str) -> &str {
    mojito_symbol::symbol::specialization_template(name).unwrap_or(name)
}

/// The spelling of a compile-time collection's type in the rejection of its
/// runtime read, as the pin spells it: a list display is a fixed-size
/// `Array[Int, Int(2)]`, a set `Set[Int]`, a dictionary `Dict[String, Int]`.
fn materialized_collection_spelling(ty: &Ty) -> String {
    fn scalar(element: &Ty) -> String {
        match element.kind() {
            TyKind::Int | TyKind::IntLiteral => "Int".to_string(),
            TyKind::Float64 | TyKind::FloatLiteral => "Float64".to_string(),
            TyKind::StringLiteral => "String".to_string(),
            _ => match mojito_types::types::tuple_elements(element) {
                Some(elements) => format!(
                    "Tuple[{}]",
                    elements
                        .into_iter()
                        .map(scalar)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                None => mojito_types::types::unqualified_type_name(element),
            },
        }
    }
    if let Some((key, value)) = mojito_types::types::dict_elements(ty) {
        return format!("Dict[{}, {}]", scalar(key), scalar(value));
    }
    if let Some(element) = mojito_types::types::list_element(ty) {
        return format!("List[{}]", scalar(element));
    }
    if let Some(element) = mojito_types::types::set_element(ty) {
        return format!("Set[{}]", scalar(element));
    }
    if let TyKind::Struct(name, args) = ty.kind()
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
        && (matches!(element.kind(), TyKind::Int | TyKind::Bool | TyKind::Float64)
            || matches!(element.kind(), TyKind::Struct(name, args)
                if args.is_empty() && mojito_types::types::is_stdlib_string_struct(name)))
}

/// The element a display of `element`s binds: a string element binds as a
/// `String` value parameter does.
pub(super) fn string_element_binder(element: Ty) -> Ty {
    match element.kind() {
        TyKind::Struct(name, args)
            if args.is_empty() && mojito_types::types::is_stdlib_string_struct(name) =>
        {
            Ty::string_literal()
        }
        _ => element,
    }
}

/// Whether a type `meta` holds names the declared parameter `binder` (a
/// `*vs: T` pack's element type naming `T`).
fn meta_names_binder(
    meta: &mojito_types::param_expr::MetaTy,
    binder: &mojito_types::param_expr::ParamRef,
) -> bool {
    use mojito_types::param_expr::MetaTy;
    match meta {
        MetaTy::Value(ty) => mojito_types::types::names_binder(ty, binder),
        MetaTy::Type => false,
        MetaTy::ParamList(element) => meta_names_binder(element, binder),
        MetaTy::Tuple(elements) | MetaTy::List(elements) | MetaTy::Set(elements) => elements
            .iter()
            .any(|element| meta_names_binder(element, binder)),
        MetaTy::Dict(entries) => entries
            .iter()
            .any(|(key, value)| meta_names_binder(key, binder) || meta_names_binder(value, binder)),
    }
}

/// Record `value` for `name` unless a different value is already recorded,
/// in which case the disagreement leaves `name` unrecorded.
fn record_agreed<V: PartialEq>(
    recorded: &mut declaration_table::DeclarationTable<V>,
    name: &str,
    value: V,
) {
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
pub(super) fn names_read(expr: &Expr) -> Vec<String> {
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

/// Whether `value` is a scalar or a string attribute.
const fn is_scalar_attribute(value: &CtValue) -> bool {
    matches!(
        value,
        CtValue::Int(_)
            | CtValue::UInt(_)
            | CtValue::Float(_)
            | CtValue::IntLiteral(_)
            | CtValue::FloatLiteral(_)
            | CtValue::Bool(_)
            | CtValue::Str(_)
    )
}

/// The predicate name a bracket application spells (`IsTriviallyCopyable`
/// in `IsTriviallyCopyable[T]`, which may parse as an index).
fn predicate_spelling(expr: &Expr) -> Option<&str> {
    match &expr.kind {
        ExprKind::TypeApply { name, .. } => Some(name),
        ExprKind::Index { object, .. } => match &object.kind {
            ExprKind::Identifier(name) => Some(name),
            _ => None,
        },
        _ => None,
    }
}

/// Whether two bound lists name the same traits, in any order.
fn same_bounds(left: &[String], right: &[String]) -> bool {
    left.len() == right.len() && left.iter().all(|bound| right.contains(bound))
}

const VARIADICS_MODULE: &str = "std.builtin.variadics";
