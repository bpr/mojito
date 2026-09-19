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
            // A method keyed on its own pack (`def params[*Ts: Writable](self,
            // *args: *Ts)`) checks only per specialization, like a pack def.
            if !validates_body(
                &m.type_params,
                &m.body,
                body_keys_rebind(&m.body, &self.rebind_keyed_bodies),
            ) {
                continue;
            }
            self.check_method(
                self_ty,
                m,
                declaration.module.clone().as_ref(),
                declaration.name,
                method_index,
                overload_index,
            )?;
        }
        Ok(())
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
        let result = self
            .declare_immutable(var, element)
            .and_then(|()| self.check_block(body, ret, in_loop));
        self.pop_scope();
        *self.uninitialized.borrow_mut() = before;
        result
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

    /// Whether a validation error marks the validator's own blind spot
    /// rather than a verdict: a constructor, method, or operator of a struct
    /// registered only as a template shell (a `Tuple`, a variadic or
    /// `DType`-keyed struct), whose members exist only per specialization.
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
            ConstraintOperand::Value(_) | ConstraintOperand::Type(_) => Ok(()),
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

/// Whether a declaration has a `DType` parameter.
pub(super) fn dtype_keyed(type_params: &[mojito_ast::ast::TypeParam]) -> bool {
    type_params
        .iter()
        .any(|parameter| matches!(parameter.bounds.as_slice(), [only] if only == "DType"))
}

/// Whether a struct declaration checks only per specialization, so source
/// validation registers it as a template shell: a variadic pack, a `DType`
/// parameter, a struct-typed value parameter (`is_value_struct` names the
/// declared structs), or a vector-typed value parameter — the shapes the
/// elaborator monomorphizes per application.
pub(super) fn concrete_only_struct(
    type_params: &[mojito_ast::ast::TypeParam],
    is_value_struct: &dyn Fn(&str) -> bool,
) -> bool {
    is_variadic_template(type_params) || type_params.iter().any(|parameter| {
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
/// compile-time control flow, or a `rebind` over the declaration's own
/// parameters (`keys_rebind`, from `rebind::rebind_keyed_bodies`) — both
/// leave the template stubbed, so validation is the only check it gets.
/// Either way a body that checks only per instantiation is left out: a
/// variadic template, or one reading a reflection handle (`reflect[T]`),
/// whose field facts only the elaborator evaluates.
pub(super) fn validates_body(
    type_params: &[mojito_ast::ast::TypeParam],
    body: &[Stmt],
    keys_rebind: bool,
) -> bool {
    (block_has_comptime(body) || keys_rebind)
        && !is_variadic_template(type_params)
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
