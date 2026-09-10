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
            // A compile-time binding's initializer was consumed by elaboration.
            StmtKind::Comptime { .. }
            | StmtKind::Return(None)
            | StmtKind::Pass
            | StmtKind::Break
            | StmtKind::Continue
            | StmtKind::Import { .. }
            | StmtKind::FromImport { .. }
            | StmtKind::Trait { .. } => Ok(()),
            StmtKind::If { branches, orelse } | StmtKind::ComptimeIf { branches, orelse } => {
                for (cond, body) in branches {
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
            StmtKind::ComptimeFor { iter, body, .. } => {
                self.cross_expr(iter, env, shadowed)?;
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
            StmtKind::With { items, body } => {
                for item in items {
                    self.cross_expr(&mut item.context, env, shadowed)?;
                }
                self.cross_block(body, env, shadowed)
            }
            StmtKind::Def { params, body, .. } => self.cross_body(params, body, env, shadowed),
            StmtKind::Struct { methods, .. } => {
                for method in methods {
                    self.cross_body(&method.params, &mut method.body, env, shadowed)?;
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
        let binding = |name: &str| {
            if shadowed.contains(name) {
                None
            } else {
                env.get(name)
            }
        };
        match &mut expr.kind {
            // `materialize[NAME]()`: the binding's literal form. Any other
            // spelling is left for the checker to report.
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } if name == "materialize" && args.is_empty() && kwargs.is_empty() => {
                if let [ParamArg::Value(argument)] = param_args.as_slice()
                    && let ExprKind::Identifier(bound) = &argument.kind
                    && let Some(value) = binding(bound)
                {
                    *expr = lit_result(value, expr.span)?;
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
                let value = self.eval(&args[0], env)?;
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
fn not_implicitly_copyable(value: &CtValue) -> ComptimeError {
    ComptimeError::Crossing(format!(
        "cannot materialize comptime value of type '{}' to runtime because it is not \
         'ImplicitlyCopyable'; use materialize[...]() to cross explicitly",
        value
            .runtime_type_text()
            .unwrap_or_else(|| value.to_string())
    ))
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
            StmtKind::ComptimeFor { body, .. } => collect_runtime_locals(body, out),
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
