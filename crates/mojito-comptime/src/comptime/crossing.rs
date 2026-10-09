//! The compile-time → runtime crossing of elaborated bindings: `materialize[X]()`
//! and `comptime(e)` fold to the literal form of their value, and a bare runtime
//! use of a compile-time collection (`Array`, `Dict`, `Set` are not implicitly
//! copyable) is rejected with upstream's diagnostic.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_ast::ast::{ComprehensionClause, SubscriptArg};

impl Elab<'_> {
    /// Fold every crossing in `stmts` against the bindings in `env`. A def
    /// (or method) whose body declares a runtime local of a binding's name
    /// refers to that local, never to the compile-time binding.
    pub(super) fn fold_runtime_crossings(
        &self,
        stmts: &mut [Stmt],
        env: &HashMap<String, CtValue>,
    ) -> Result<(), ComptimeError> {
        let shadowed = HashSet::new();
        for stmt in stmts {
            self.cross_stmt(stmt, env, &shadowed)?;
        }
        Ok(())
    }

    fn cross_block(
        &self,
        stmts: &mut [Stmt],
        env: &HashMap<String, CtValue>,
        shadowed: &HashSet<String>,
    ) -> Result<(), ComptimeError> {
        for stmt in stmts {
            self.cross_stmt(stmt, env, shadowed)?;
        }
        Ok(())
    }

    fn cross_body(
        &self,
        params: &[FnParam],
        body: &mut [Stmt],
        env: &HashMap<String, CtValue>,
        shadowed: &HashSet<String>,
    ) -> Result<(), ComptimeError> {
        let mut shadowed = shadowed.clone();
        shadowed.extend(params.iter().map(|param| param.name.clone()));
        collect_runtime_locals(body, &mut shadowed);
        self.cross_block(body, env, &shadowed)
    }

    /// Cross the body of a `def` or a method, every one a template body
    /// ([`Self::in_template_body`]).
    fn cross_declaration_body(
        &self,
        params: &[FnParam],
        body: &mut [Stmt],
        env: &HashMap<String, CtValue>,
        shadowed: &HashSet<String>,
    ) -> Result<(), ComptimeError> {
        let depth = self.crossing_templates.get();
        self.crossing_templates.set(depth + 1);
        let crossed = self.cross_body(params, body, env, shadowed);
        self.crossing_templates.set(depth);
        crossed
    }

    /// Whether the statement being crossed sits in a function body: one
    /// being elaborated as a template, or one this pass has descended into.
    fn in_template_body(&self) -> bool {
        self.crossing_templates.get() > 0 || !self.template_binders.borrow().is_empty()
    }

    fn cross_stmt(
        &self,
        stmt: &mut Stmt,
        env: &HashMap<String, CtValue>,
        shadowed: &HashSet<String>,
    ) -> Result<(), ComptimeError> {
        match &mut stmt.kind {
            StmtKind::VarDecl { value, .. }
            | StmtKind::RefDecl { value, .. }
            | StmtKind::Assign { value, .. }
            | StmtKind::Expr(value)
            | StmtKind::Raise(value)
            | StmtKind::Return(Some(value)) => self.cross_expr(value, env, shadowed),
            StmtKind::AugAssign { place, value, .. } | StmtKind::SetPlace { place, value } => {
                self.cross_expr(place, env, shadowed)?;
                self.cross_expr(value, env, shadowed)
            }
            StmtKind::Unpack { targets, value, .. } => {
                for target in targets {
                    self.cross_expr(target, env, shadowed)?;
                }
                self.cross_expr(value, env, shadowed)
            }
            // A binding a template body keeps is a compile-time position: a
            // named collection its value subscripts is its literal display
            // there (`comptime q = xs[i]`), as in a loop header.
            StmtKind::Comptime { value, .. } if self.in_template_body() => {
                let mut subscripts = SubscriptedCollections {
                    env,
                    shadowed,
                    folded: Ok(()),
                };
                mojito_ast::visit::MutVisitor::visit_expr_mut(&mut subscripts, value);
                mojito_ast::visit::walk_expr_mut(&mut subscripts, value);
                subscripts.folded
            }
            // Elsewhere, a compile-time binding's initializer was consumed by
            // elaboration.
            StmtKind::Comptime { .. }
            | StmtKind::Return(None)
            | StmtKind::Pass
            | StmtKind::Break
            | StmtKind::Continue
            | StmtKind::Import { .. }
            | StmtKind::FromImport { .. }
            | StmtKind::Trait { .. } => Ok(()),
            StmtKind::If { branches, orelse } => {
                for (cond, body) in branches {
                    self.cross_expr(cond, env, shadowed)?;
                    self.cross_block(body, env, shadowed)?;
                }
                self.cross_opt_block(orelse, env, shadowed)
            }
            // A condition is a compile-time position: a named collection it
            // reads (`XS[0] == 1`, `3 in L`) is its literal display there,
            // which the check closes.
            StmtKind::ComptimeIf { branches, orelse } => {
                for (cond, body) in branches {
                    let mut collections = NamedCollections {
                        env,
                        shadowed,
                        folded: Ok(()),
                    };
                    mojito_ast::visit::MutVisitor::visit_expr_mut(&mut collections, cond);
                    mojito_ast::visit::walk_expr_mut(&mut collections, cond);
                    collections.folded?;
                    self.cross_expr(cond, env, shadowed)?;
                    self.cross_block(body, env, shadowed)?;
                }
                self.cross_opt_block(orelse, env, shadowed)
            }
            StmtKind::While { cond, body, orelse } => {
                self.cross_expr(cond, env, shadowed)?;
                self.cross_block(body, env, shadowed)?;
                self.cross_opt_block(orelse, env, shadowed)
            }
            StmtKind::For {
                iter, body, orelse, ..
            } => {
                self.cross_expr(iter, env, shadowed)?;
                self.cross_block(body, env, shadowed)?;
                self.cross_opt_block(orelse, env, shadowed)
            }
            // The iterable is a compile-time position: a named collection
            // is its literal display there, which the check closes into the
            // loop header's sequence, as is one a display's element
            // subscripts (`[L[n], n]`).
            StmtKind::ComptimeFor { iter, body, .. } => {
                *iter = self.spell_applied_displays(iter);
                if matches!(iter.kind, ExprKind::ListLit(_) | ExprKind::BraceLit(_)) {
                    let mut subscripts = SubscriptedCollections {
                        env,
                        shadowed,
                        folded: Ok(()),
                    };
                    mojito_ast::visit::walk_expr_mut(&mut subscripts, iter);
                    subscripts.folded?;
                }
                match named_collection(iter, env, shadowed) {
                    Some(value) => *iter = collection_literal(value, iter)?,
                    None => self.cross_expr(iter, env, shadowed)?,
                }
                self.cross_block(body, env, shadowed)
            }
            StmtKind::Try {
                body,
                except,
                orelse,
                finalbody,
            } => {
                self.cross_block(body, env, shadowed)?;
                if let Some((_, body)) = except {
                    self.cross_block(body, env, shadowed)?;
                }
                self.cross_opt_block(orelse, env, shadowed)?;
                self.cross_opt_block(finalbody, env, shadowed)
            }
            StmtKind::Scope(body) => self.cross_block(body, env, shadowed),
            StmtKind::With { items, body } => {
                for item in items {
                    self.cross_expr(&mut item.context, env, shadowed)?;
                }
                self.cross_block(body, env, shadowed)
            }
            StmtKind::Def { params, body, .. } => {
                self.cross_declaration_body(params, body, env, shadowed)
            }
            StmtKind::Struct { methods, .. } => {
                for method in methods {
                    self.cross_declaration_body(&method.params, &mut method.body, env, shadowed)?;
                }
                Ok(())
            }
        }
    }

    fn cross_opt_block(
        &self,
        block: &mut Option<Vec<Stmt>>,
        env: &HashMap<String, CtValue>,
        shadowed: &HashSet<String>,
    ) -> Result<(), ComptimeError> {
        match block {
            Some(body) => self.cross_block(body, env, shadowed),
            None => Ok(()),
        }
    }

    fn cross_expr(
        &self,
        expr: &mut Expr,
        env: &HashMap<String, CtValue>,
        shadowed: &HashSet<String>,
    ) -> Result<(), ComptimeError> {
        // A generic comptime alias applied in a runtime position
        // (`Twice[3]`) folds to the literal form of its compile-time value;
        // a type-valued application is left for the checker.
        let alias_application = match &expr.kind {
            ExprKind::Index { object, .. } => match &object.kind {
                ExprKind::Identifier(alias) => Some(alias.as_str()),
                _ => None,
            },
            ExprKind::TypeApply { name, .. } => Some(name.as_str()),
            _ => None,
        }
        .is_some_and(|alias| {
            !shadowed.contains(alias) && self.generic_aliases.borrow().contains_key(alias)
        });
        if alias_application && let Some(mut literal) = self.eval(expr, env)?.materialize(expr.span)
        {
            literal.source.clone_from(&expr.source);
            *expr = literal;
            return Ok(());
        }
        let binding = |name: &str| {
            if shadowed.contains(name) {
                None
            } else {
                env.get(name)
            }
        };
        // A reflection query in a runtime position (`r.field_count()`,
        // `reflect[T].field_index["x"]()`) is upstream's call of a static
        // `Reflected[T]` method whose body returns a compile-time answer:
        // it folds to the literal form of that answer where it stands. A
        // handle over a subject still a parameter does not evaluate here, and
        // its query is left for the checker to type symbolically.
        // A subscript of a closed list answer (`r.field_names()[1]`) folds
        // with it, to the element the runtime `__getitem__` would read.
        let query = match &expr.kind {
            ExprKind::Index { object, .. } => reflection_query(object, &binding),
            _ => reflection_query(expr, &binding),
        };
        if let Some(handle) = query
            && self.eval(handle, env).is_ok()
        {
            let mut literal = lit_result(&self.eval(expr, env)?, expr.span)?;
            literal.source.clone_from(&expr.source);
            literal.syntax_id = expr.syntax_id;
            *expr = literal;
            return Ok(());
        }
        // The length of a `TypeList` binding in a runtime position
        // (`tl.length`, `len(tl)`) folds to its count where it stands.
        let measured = match &expr.kind {
            ExprKind::Member { object, field } if field == "length" => Some(&**object),
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } if name == "len" && param_args.is_empty() && kwargs.is_empty() => {
                match args.as_slice() {
                    [list] => Some(list),
                    _ => None,
                }
            }
            _ => None,
        };
        if let Some(ExprKind::Identifier(list)) = measured.map(|list| &list.kind)
            && let Some(value @ CtValue::Struct { name, .. }) = binding(list)
            && name == "TypeList"
            && let Some(elements) = value.typelist_elements()
        {
            let mut literal = lit_result(&CtValue::Int(elements.len() as i64), expr.span)?;
            literal.source.clone_from(&expr.source);
            literal.syntax_id = expr.syntax_id;
            *expr = literal;
            return Ok(());
        }
        match &mut expr.kind {
            // `materialize[X]()`: the literal form of a binding, or of a
            // compile-time expression over the bindings (`names[i]`). An
            // operand that does not evaluate here is left for the checker.
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } if name == "materialize" && args.is_empty() && kwargs.is_empty() => {
                let [ParamArg::Value(argument)] = param_args.as_mut_slice() else {
                    return Ok(());
                };
                // An applied display constant is its display here; any other
                // applied constant is the check's to bind.
                *argument = self.spell_applied_displays(argument);
                let value = match &argument.kind {
                    ExprKind::Identifier(bound) => binding(bound).cloned(),
                    // An operand that applies a callable is the check's to
                    // lift and the elaborator below MIR's to evaluate.
                    _ if self.applies_callable(argument) => None,
                    _ => self.eval(argument, env).ok(),
                };
                if let Some(value) = value.filter(|value| !matches!(value, CtValue::Marker(_))) {
                    *expr = lit_result(&value, expr.span)?;
                }
                Ok(())
            }
            // `comptime(e)`: evaluate now, materialize the result.
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } if name == "comptime"
                && args.len() == 1
                && param_args.is_empty()
                && kwargs.is_empty() =>
            {
                // An operand that applies a callable is a request the
                // elaborator below MIR serves, a value binding it calls a
                // method of spelled by its literal form.
                args[0] = self.spell_applied_displays(&args[0]);
                if self.applies_callable(&args[0]) {
                    return Ok(());
                }
                if let Some(spelled) = self
                    .spelled_value_reads(&args[0], env)
                    .filter(|spelled| self.applies_callable(spelled))
                {
                    args[0] = spelled;
                    return Ok(());
                }
                let value = match self.eval(&args[0], env) {
                    Ok(value) => value,
                    // An operand over the binders of a generic body is the
                    // check's to type and the elaborator below MIR's to
                    // evaluate per instance.
                    Err(_) if self.in_template_body() => return Ok(()),
                    Err(error) => return Err(error),
                };
                if value.is_runtime_collection() {
                    return Err(not_implicitly_copyable(&value));
                }
                *expr = lit_result(&value, expr.span)?;
                Ok(())
            }
            ExprKind::Identifier(name) => match binding(name) {
                Some(value) if value.is_runtime_collection() => Err(not_implicitly_copyable(value)),
                _ => Ok(()),
            },
            ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Bool(_)
            | ExprKind::Str(_)
            | ExprKind::None
            | ExprKind::Uninitialized
            | ExprKind::EmptySubscript
            | ExprKind::TypeValue(_)
            | ExprKind::TypeApply { .. } => Ok(()),
            ExprKind::Prefix(_, inner)
            | ExprKind::Transfer(inner)
            | ExprKind::Spread(inner)
            | ExprKind::Named { value: inner, .. }
            | ExprKind::Member { object: inner, .. } => self.cross_expr(inner, env, shadowed),
            ExprKind::Infix(_, left, right)
            | ExprKind::Index {
                object: left,
                index: right,
            } => {
                self.cross_expr(left, env, shadowed)?;
                self.cross_expr(right, env, shadowed)
            }
            ExprKind::Call { args, kwargs, .. } => {
                for argument in args {
                    self.cross_expr(argument, env, shadowed)?;
                }
                for argument in kwargs {
                    self.cross_expr(&mut argument.value, env, shadowed)?;
                }
                Ok(())
            }
            ExprKind::Invoke {
                callee,
                args,
                kwargs,
                ..
            } => {
                self.cross_expr(callee, env, shadowed)?;
                for argument in args {
                    self.cross_expr(argument, env, shadowed)?;
                }
                for argument in kwargs {
                    self.cross_expr(&mut argument.value, env, shadowed)?;
                }
                Ok(())
            }
            ExprKind::MethodCall {
                object,
                args,
                kwargs,
                ..
            } => {
                self.cross_expr(object, env, shadowed)?;
                for argument in args {
                    self.cross_expr(argument, env, shadowed)?;
                }
                for argument in kwargs {
                    self.cross_expr(&mut argument.value, env, shadowed)?;
                }
                Ok(())
            }
            ExprKind::ListLit(items) | ExprKind::TupleLit(items) => {
                for item in items {
                    self.cross_expr(item, env, shadowed)?;
                }
                Ok(())
            }
            ExprKind::BraceLit(entries) => {
                for (key, value) in entries {
                    self.cross_expr(key, env, shadowed)?;
                    if let Some(value) = value {
                        self.cross_expr(value, env, shadowed)?;
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
                // A generator binder shadows within the comprehension.
                let mut shadowed = shadowed.clone();
                for clause in clauses.iter() {
                    if let ComprehensionClause::For { var, .. } = clause {
                        shadowed.insert(var.clone());
                    }
                }
                for clause in clauses {
                    match clause {
                        ComprehensionClause::For { iter, .. } => {
                            self.cross_expr(iter, env, &shadowed)?;
                        }
                        ComprehensionClause::If(condition) => {
                            self.cross_expr(condition, env, &shadowed)?;
                        }
                    }
                }
                if let Some(key) = key {
                    self.cross_expr(key, env, &shadowed)?;
                }
                self.cross_expr(value, env, &shadowed)
            }
            ExprKind::Lambda { def } => self.cross_stmt(def, env, shadowed),
            ExprKind::IfExpr {
                cond,
                then_branch,
                else_branch,
            } => {
                self.cross_expr(cond, env, shadowed)?;
                self.cross_expr(then_branch, env, shadowed)?;
                self.cross_expr(else_branch, env, shadowed)
            }
            ExprKind::Compare { first, rest } => {
                self.cross_expr(first, env, shadowed)?;
                for (_, operand) in rest {
                    self.cross_expr(operand, env, shadowed)?;
                }
                Ok(())
            }
            ExprKind::Slice {
                object,
                lower,
                upper,
                step,
                ..
            } => {
                self.cross_expr(object, env, shadowed)?;
                for bound in [lower, upper, step].into_iter().flatten() {
                    self.cross_expr(bound, env, shadowed)?;
                }
                Ok(())
            }
            ExprKind::MultiIndex { object, args } => {
                self.cross_expr(object, env, shadowed)?;
                for argument in args {
                    match argument {
                        SubscriptArg::Index(value) | SubscriptArg::Keyword { value, .. } => {
                            self.cross_expr(value, env, shadowed)?;
                        }
                        SubscriptArg::Slice {
                            lower, upper, step, ..
                        }
                        | SubscriptArg::KeywordSlice {
                            lower, upper, step, ..
                        } => {
                            for bound in [lower, upper, step].into_iter().flatten() {
                                self.cross_expr(bound, env, shadowed)?;
                            }
                        }
                    }
                }
                Ok(())
            }
            ExprKind::TString { parts, .. } => {
                for part in parts {
                    if let TStringPart::Expr(value) = part {
                        self.cross_expr(value, env, shadowed)?;
                    }
                }
                Ok(())
            }
        }
    }
}

/// Upstream's rejection of an implicit runtime use of a compile-time
/// collection.
/// The closed collection `expr` names, unless a runtime local shadows it.
fn named_collection<'a>(
    expr: &Expr,
    env: &'a HashMap<String, CtValue>,
    shadowed: &HashSet<String>,
) -> Option<&'a CtValue> {
    match &expr.kind {
        ExprKind::Identifier(name) if !shadowed.contains(name) => {
            env.get(name).filter(|value| value.is_runtime_collection())
        }
        _ => None,
    }
}

/// The literal display of a collection `value`, standing where `named` did.
fn collection_literal(value: &CtValue, named: &Expr) -> Result<Expr, ComptimeError> {
    let mut literal = lit_result(value, named.span)?;
    literal.source.clone_from(&named.source);
    literal.syntax_id = named.syntax_id;
    Ok(literal)
}

/// Folds each closed collection a compile-time display subscripts to its
/// literal display.
struct SubscriptedCollections<'a> {
    env: &'a HashMap<String, CtValue>,
    shadowed: &'a HashSet<String>,
    folded: Result<(), ComptimeError>,
}

impl mojito_ast::visit::MutVisitor for SubscriptedCollections<'_> {
    fn visit_expr_mut(&mut self, expr: &mut Expr) {
        let ExprKind::Index { object, .. } = &mut expr.kind else {
            return;
        };
        if let Some(value) = named_collection(object, self.env, self.shadowed) {
            match collection_literal(value, object) {
                Ok(literal) => **object = literal,
                Err(error) => self.folded = Err(error),
            }
        }
    }
}

/// Folds each closed collection a compile-time condition names to its
/// literal display.
struct NamedCollections<'a> {
    env: &'a HashMap<String, CtValue>,
    shadowed: &'a HashSet<String>,
    folded: Result<(), ComptimeError>,
}

impl mojito_ast::visit::MutVisitor for NamedCollections<'_> {
    fn visit_expr_mut(&mut self, expr: &mut Expr) {
        if let Some(value) = named_collection(expr, self.env, self.shadowed) {
            match collection_literal(value, expr) {
                Ok(literal) => *expr = literal,
                Err(error) => self.folded = Err(error),
            }
        }
    }
}

fn not_implicitly_copyable(value: &CtValue) -> ComptimeError {
    ComptimeError::Crossing(format!(
        "cannot materialize comptime value of type '{}' to runtime because it is not \
         'ImplicitlyCopyable'; use materialize[...]() to cross explicitly",
        value
            .runtime_type_text()
            .unwrap_or_else(|| value.to_string())
    ))
}

/// The reflection handle `expr` calls a `Reflected[T]` method on —
/// `is_struct()`, `field_count()`, `field_names()`, `field_types()`,
/// `field_index[name]()` — if it calls one.
pub(super) fn reflection_query<'e, 'a>(
    expr: &'e Expr,
    binding: &dyn Fn(&str) -> Option<&'a CtValue>,
) -> Option<&'e Expr> {
    let object = match &expr.kind {
        ExprKind::MethodCall {
            object,
            args,
            kwargs,
            ..
        } if args.is_empty() && kwargs.is_empty() => object,
        ExprKind::Invoke {
            callee,
            args,
            kwargs,
            ..
        } if args.is_empty() && kwargs.is_empty() => match &callee.kind {
            ExprKind::Member { object, .. } => object,
            _ => return None,
        },
        _ => return None,
    };
    reflection_handle(object, binding).then_some(&**object)
}

/// Whether `expr` spells a reflection handle: `reflect[T]`, a name bound to
/// one, or a field handle selected from one (`.field["x"]`, `.field_at[i]`).
fn reflection_handle<'a>(expr: &Expr, binding: &dyn Fn(&str) -> Option<&'a CtValue>) -> bool {
    let reflect_unshadowed = |name: &str| name == "reflect" && binding(name).is_none();
    match &expr.kind {
        ExprKind::Identifier(name) => matches!(binding(name), Some(CtValue::Reflected(_))),
        ExprKind::TypeApply { name, .. } => reflect_unshadowed(name),
        ExprKind::Call {
            name,
            param_args,
            args,
            kwargs,
        } => {
            reflect_unshadowed(name)
                && param_args.len() == 1
                && args.is_empty()
                && kwargs.is_empty()
        }
        ExprKind::Index { object, .. } => match &object.kind {
            ExprKind::Identifier(name) => reflect_unshadowed(name),
            ExprKind::Member { object, field } => {
                matches!(field.as_str(), "field" | "field_at") && reflection_handle(object, binding)
            }
            _ => false,
        },
        _ => false,
    }
}

/// The names a body declares as runtime locals (anywhere in it, flat), which
/// shadow same-named compile-time bindings throughout that body.
fn collect_runtime_locals(stmts: &[Stmt], out: &mut HashSet<String>) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::VarDecl { name, .. }
            | StmtKind::RefDecl { name, .. }
            | StmtKind::Assign { name, .. } => {
                out.insert(name.clone());
            }
            StmtKind::Unpack { targets, .. } => {
                for target in targets {
                    if let ExprKind::Identifier(name) = &target.kind {
                        out.insert(name.clone());
                    }
                }
            }
            StmtKind::For {
                var, body, orelse, ..
            } => {
                out.insert(var.clone());
                collect_runtime_locals(body, out);
                if let Some(orelse) = orelse {
                    collect_runtime_locals(orelse, out);
                }
            }
            StmtKind::If { branches, orelse } | StmtKind::ComptimeIf { branches, orelse } => {
                for (_, body) in branches {
                    collect_runtime_locals(body, out);
                }
                if let Some(orelse) = orelse {
                    collect_runtime_locals(orelse, out);
                }
            }
            StmtKind::While { body, orelse, .. } => {
                collect_runtime_locals(body, out);
                if let Some(orelse) = orelse {
                    collect_runtime_locals(orelse, out);
                }
            }
            StmtKind::ComptimeFor { body, .. } | StmtKind::Scope(body) => {
                collect_runtime_locals(body, out);
            }
            StmtKind::Try {
                body,
                except,
                orelse,
                finalbody,
            } => {
                collect_runtime_locals(body, out);
                if let Some((name, body)) = except {
                    out.extend(name.clone());
                    collect_runtime_locals(body, out);
                }
                for block in [orelse, finalbody].into_iter().flatten() {
                    collect_runtime_locals(block, out);
                }
            }
            StmtKind::With { items, body } => {
                out.extend(items.iter().filter_map(|item| item.var.clone()));
                collect_runtime_locals(body, out);
            }
            _ => {}
        }
    }
}
