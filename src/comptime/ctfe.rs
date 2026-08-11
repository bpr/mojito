//! Compile-time function evaluation via the VM (`ctfe_call`/`vm_ctfe_call`), the
//! VM-CTFE program rewrite, and the CTFE-safety analysis.
//! Extracted from `comptime.rs`; see `docs/symbol-map.md`.

use super::*;

/// The synthesized root of a struct-construction/static-method CTFE run.
const CTFE_STRUCT_ENTRY: &str = "$ctfe$struct$entry";

impl<'a> Elab<'a> {
    /// Registry-aware specializability: recognizes struct-typed value
    /// parameters through the collected struct set.
    pub(super) fn is_specializable(&self, statement: &Stmt) -> bool {
        is_specializable_declaration_in(statement, &|name| self.structs.contains_key(name))
    }

    pub(super) fn ctfe_call(
        &self,
        name: &str,
        param_args: &[ParamArg],
        args: Vec<CtValue>,
        scope: &HashMap<String, CtValue>,
    ) -> Result<CtValue, ComptimeError> {
        let f = self.fns.get(name).ok_or_else(|| {
            ComptimeError::NotComptime(format!("'{name}' is not a compile-time-callable function"))
        })?;
        if f.ct_params.len() != param_args.len() {
            return Err(ComptimeError::Arity(format!(
                "'{name}' expects {} compile-time argument(s), got {}",
                f.ct_params.len(),
                param_args.len()
            )));
        }
        if f.params.len() != args.len() {
            return Err(ComptimeError::Arity(format!(
                "'{name}' expects {} argument(s), got {}",
                f.params.len(),
                args.len()
            )));
        }
        self.burn()?;
        let mut locals: HashMap<String, CtValue> = HashMap::new();
        let mut value_params = Vec::new();
        for (decl, arg) in f.ct_params.iter().zip(param_args) {
            let value = self.resolve_ct_arg(decl, arg, scope)?;
            if let ParamDecl::Value { name, .. } = decl
                && !matches!(value, CtValue::Type(_))
            {
                value_params.push((name.clone(), ct_to_vm(&value)?));
            }
            locals.insert(decl.name().to_string(), value);
        }
        locals.extend(f.params.iter().cloned().zip(args));
        let mut visiting = HashSet::new();
        let mut needed = HashSet::new();
        let safe = self.vm_ctfe_safe_fn(name, &mut visiting, &mut needed);
        if safe && let Some(value) = self.vm_ctfe_call(name, &locals, &value_params, &needed)? {
            return Ok(value);
        }
        Err(ComptimeError::NotComptime(format!(
            "'{name}' is not safe for VM-backed compile-time execution"
        )))
    }

    /// Build the bounded free-declaration graph required by a VM-CTFE entry.
    /// The purity walk seeds the executed root and its direct free helpers. The
    /// ordinary nominal declarations retained for type checking contribute
    /// their own bare free-function callees (for example Range's length helper),
    /// and a work queue follows those callees transitively. Unrelated top-level
    /// functions never enter the set, so bindings that have not yet been
    /// materialized cannot leak into the checked CTFE subprogram.
    pub(super) fn vm_ctfe_declaration_closure(&self, needed: &HashSet<String>) -> HashSet<String> {
        let available: HashSet<String> = self
            .program
            .iter()
            .filter_map(|statement| match &statement.kind {
                StmtKind::Def { name, .. } if !self.is_specializable(statement) => {
                    Some(name.clone())
                }
                _ => None,
            })
            .collect();
        let mut pending: VecDeque<String> = needed.iter().cloned().collect();

        // The ordinary checker validates retained nominal method/default bodies
        // even when the CTFE entry does not invoke them. Seed their actual free
        // callees rather than retaining every `$`-qualified linked symbol.
        for statement in self.program {
            if matches!(&statement.kind, StmtKind::Trait { .. })
                || matches!(&statement.kind, StmtKind::Struct { .. })
                    && !self.is_specializable(statement)
            {
                let mut calls = HashSet::new();
                collect_vm_ctfe_stmt_calls(statement, &mut calls);
                pending.extend(calls);
            }
        }

        let mut closure = HashSet::new();
        while let Some(name) = pending.pop_front() {
            if !available.contains(&name) || !closure.insert(name.clone()) {
                continue;
            }
            let mut calls = HashSet::new();
            for statement in self.program {
                if matches!(&statement.kind, StmtKind::Def { name: candidate, .. } if candidate == &name)
                    && !self.is_specializable(statement)
                {
                    collect_vm_ctfe_stmt_calls(statement, &mut calls);
                }
            }
            pending.extend(calls);
        }
        closure
    }

    pub(super) fn vm_ctfe_call(
        &self,
        name: &str,
        locals: &HashMap<String, CtValue>,
        value_params: &[(String, Value)],
        needed: &HashSet<String>,
    ) -> Result<Option<CtValue>, ComptimeError> {
        let Some(f) = self.fns.get(name) else {
            return Ok(None);
        };
        let mut args = Vec::with_capacity(f.params.len());
        for pname in &f.params {
            let value = locals.get(pname).ok_or_else(|| {
                ComptimeError::NotComptime(format!(
                    "missing compile-time argument '{pname}' for VM CTFE call"
                ))
            })?;
            args.push(ct_to_vm(value)?);
        }
        let mut vm = VmBackend::new();
        let declarations = self.vm_ctfe_declaration_closure(needed);
        let mut program = self
            .program
            .iter()
            // The checked boundary needs the declaration environment, not just
            // the transitively executed bodies. Retain all declarations so trait
            // bounds, struct types, overloads, and helper calls resolve exactly;
            // the VM still executes only the requested function.
            .filter(|stmt| match &stmt.kind {
                // Only the semantic free-callee graph crosses this checked
                // boundary. Linked `$` spelling is neither a dependency nor a
                // specialization test.
                StmtKind::Def { name, .. } => declarations.contains(name),
                // A variadic struct template is a monomorphizer input and cannot
                // cross the ordinary checked boundary. Concrete CTFE uses have
                // already been specialized; an unused public `Tuple[*Ts]`
                // template must not invalidate an otherwise scalar subprogram.
                StmtKind::Struct { .. } => !self.is_specializable(stmt),
                StmtKind::Trait { .. } => true,
                _ => false,
            })
            .cloned()
            .collect::<Vec<_>>();
        self.rewrite_vm_ctfe_program(&mut program, name, locals)?;
        if program.is_empty() {
            return Err(ComptimeError::NotComptime(format!(
                "missing compile-time function '{name}' for VM CTFE"
            )));
        }
        // Preserve declaration order: the ordinary checker intentionally uses
        // source-order visibility for traits and sibling helpers. Execution
        // selects `name` explicitly and does not require it to be first.
        let (value, remaining_fuel) = vm
            .run_function_value(&program, name, args, value_params, self.fuel.get())
            .map_err(|e| ComptimeError::NotComptime(format!("VM CTFE failed for '{name}': {e}")))?;
        self.fuel.set(remaining_fuel);
        Ok(Some(self.vm_value_to_ct(value)?))
    }

    /// Convert a CTFE result back into a compile-time value, freezing a
    /// fieldwise-constructible struct instance (recursively) where the plain
    /// scalar/collection conversion cannot.
    pub(super) fn vm_value_to_ct(&self, value: Value) -> Result<CtValue, ComptimeError> {
        match value {
            Value::Struct {
                name,
                fields,
                value_params,
            } if value_params.is_empty() => {
                let Some(info) = self.structs.get(&name) else {
                    return Err(ComptimeError::NotComptime(format!(
                        "VM CTFE returned an unregistered struct '{name}'"
                    )));
                };
                if !info.fieldwise {
                    return Err(ComptimeError::NotComptime(format!(
                        "a compile-time '{name}' value needs fieldwise construction \
                         (@fieldwise_init or a field-mirroring __init__)"
                    )));
                }
                Ok(CtValue::Struct {
                    name,
                    fields: fields
                        .into_iter()
                        .map(|(field, value)| Ok((field, self.vm_value_to_ct(value)?)))
                        .collect::<Result<Vec<_>, ComptimeError>>()?,
                })
            }
            other => vm_to_ct(other),
        }
    }

    /// Evaluate a struct construction (`method` = `None`) or a static method
    /// call on a struct at compile time: run a synthesized entry through VM
    /// CTFE and freeze the result. The arguments were evaluated by the caller
    /// and arrive as scope-free literal expressions.
    pub(super) fn ctfe_struct_entry(
        &self,
        struct_name: &str,
        method: Option<&str>,
        literal_args: Vec<Expr>,
        span: Span,
    ) -> Result<CtValue, ComptimeError> {
        self.burn()?;
        let mut visiting = HashSet::new();
        let mut needed = HashSet::new();
        if !self.vm_ctfe_safe_struct_entry(struct_name, method, &mut visiting, &mut needed) {
            let target = method
                .map(|m| format!("{struct_name}.{m}"))
                .unwrap_or_else(|| struct_name.to_string());
            return Err(ComptimeError::NotComptime(format!(
                "'{target}' is not safe for VM-backed compile-time execution"
            )));
        }
        let ret = match method {
            None => Type::Named(struct_name.to_string(), Vec::new()),
            Some(method) => self
                .struct_static_method_ret(struct_name, method)
                .ok_or_else(|| {
                    ComptimeError::NotComptime(format!(
                        "'{struct_name}.{method}' is not a value-returning static method"
                    ))
                })?,
        };
        let expr = |kind: ExprKind| Expr {
            kind,
            span,
            source: None,
            syntax_id: crate::token::SyntaxId::fresh(),
        };
        let call = match method {
            None => expr(ExprKind::Call {
                name: struct_name.to_string(),
                param_args: Vec::new(),
                args: literal_args,
                kwargs: Vec::new(),
            }),
            Some(method) => expr(ExprKind::MethodCall {
                object: Box::new(expr(ExprKind::Identifier(struct_name.to_string()))),
                method: method.to_string(),
                args: literal_args,
                kwargs: Vec::new(),
            }),
        };
        let entry = mk(
            StmtKind::Def {
                name: CTFE_STRUCT_ENTRY.to_string(),
                decorators: Vec::new(),
                type_params: Vec::new(),
                params: Vec::new(),
                positional_only: None,
                keyword_only: None,
                captures: None,
                raises: false,
                raises_type: None,
                ret: Some(ret),
                where_clauses: Vec::new(),
                body: vec![mk(StmtKind::Return(Some(call)), span)],
            },
            span,
        );
        let mut vm = VmBackend::new();
        let declarations = self.vm_ctfe_declaration_closure(&needed);
        let mut program = self
            .program
            .iter()
            .filter(|stmt| match &stmt.kind {
                StmtKind::Def { name, .. } => declarations.contains(name),
                StmtKind::Struct { .. } => !self.is_specializable(stmt),
                StmtKind::Trait { .. } => true,
                _ => false,
            })
            .cloned()
            .collect::<Vec<_>>();
        program.push(entry);
        let (value, remaining_fuel) = vm
            .run_function_value(
                &program,
                CTFE_STRUCT_ENTRY,
                Vec::new(),
                &[],
                self.fuel.get(),
            )
            .map_err(|e| {
                ComptimeError::NotComptime(format!(
                    "VM CTFE failed for '{struct_name}' construction: {e}"
                ))
            })?;
        self.fuel.set(remaining_fuel);
        self.vm_value_to_ct(value)
    }

    /// The declared return type of a `@staticmethod`, with `Self` resolved to
    /// the owning struct.
    fn struct_static_method_ret(&self, struct_name: &str, method: &str) -> Option<Type> {
        self.program.iter().find_map(|stmt| match &stmt.kind {
            StmtKind::Struct { name, methods, .. } if name == struct_name => methods
                .iter()
                .find(|candidate| candidate.name == method && !candidate.has_self)
                .and_then(|candidate| candidate.ret.clone())
                .map(|ret| match ret {
                    Type::SelfType => Type::Named(struct_name.to_string(), Vec::new()),
                    other => other,
                }),
            _ => None,
        })
    }

    /// Whether a struct's constructors (and, when named, one of its static
    /// methods) pass the CTFE purity walk.
    pub(super) fn vm_ctfe_safe_struct_entry(
        &self,
        struct_name: &str,
        method: Option<&str>,
        visiting: &mut HashSet<String>,
        needed: &mut HashSet<String>,
    ) -> bool {
        if !self.vm_ctfe_safe_struct_ctors(struct_name, visiting, needed) {
            return false;
        }
        let Some(target) = method else {
            return true;
        };
        self.program.iter().any(|stmt| match &stmt.kind {
            StmtKind::Struct { name, methods, .. } if name == struct_name => {
                let overloads: Vec<_> = methods
                    .iter()
                    .filter(|candidate| candidate.name == target && !candidate.has_self)
                    .collect();
                !overloads.is_empty()
                    && overloads
                        .iter()
                        .all(|candidate| self.vm_ctfe_safe_block(&candidate.body, visiting, needed))
            }
            _ => false,
        })
    }

    /// Whether every constructor body of a registered, non-specializable
    /// struct passes the purity walk (a struct construction inside CTFE code).
    pub(super) fn vm_ctfe_safe_struct_ctors(
        &self,
        struct_name: &str,
        visiting: &mut HashSet<String>,
        needed: &mut HashSet<String>,
    ) -> bool {
        let guard = format!("$struct${struct_name}");
        if !visiting.insert(guard.clone()) {
            return true;
        }
        let safe = self.program.iter().any(|stmt| match &stmt.kind {
            StmtKind::Struct { name, methods, .. }
                if name == struct_name && !self.is_specializable(stmt) =>
            {
                methods
                    .iter()
                    .filter(|method| method.name == "__init__")
                    .all(|method| self.vm_ctfe_safe_block(&method.body, visiting, needed))
            }
            _ => false,
        });
        visiting.remove(&guard);
        safe
    }

    pub(super) fn rewrite_vm_ctfe_program(
        &self,
        program: &mut [Stmt],
        root: &str,
        root_scope: &HashMap<String, CtValue>,
    ) -> Result<(), ComptimeError> {
        for stmt in program {
            let scope = match &stmt.kind {
                StmtKind::Def { name, .. } if name == root => root_scope,
                _ => {
                    // Non-root helpers with only runtime-value parameters need no
                    // type-fact substitution; recursive value-parameter calls are
                    // handled by the VM's normal value-param reification.
                    continue;
                }
            };
            self.rewrite_vm_ctfe_stmt(stmt, scope)?;
        }
        Ok(())
    }

    pub(super) fn rewrite_vm_ctfe_block(
        &self,
        stmts: &mut [Stmt],
        scope: &HashMap<String, CtValue>,
    ) -> Result<(), ComptimeError> {
        for stmt in stmts {
            self.rewrite_vm_ctfe_stmt(stmt, scope)?;
        }
        Ok(())
    }

    pub(super) fn rewrite_vm_ctfe_stmt(
        &self,
        stmt: &mut Stmt,
        scope: &HashMap<String, CtValue>,
    ) -> Result<(), ComptimeError> {
        // Rewrite only type/comptime facts that the VM cannot evaluate from
        // runtime values; preserve ordinary executable structure.
        match &mut stmt.kind {
            StmtKind::Def { body, .. } => self.rewrite_vm_ctfe_block(body, scope),
            StmtKind::VarDecl { value, .. }
            | StmtKind::RefDecl { value, .. }
            | StmtKind::Assign { value, .. } => self.rewrite_vm_ctfe_expr(value, scope),
            StmtKind::AugAssign { place, value, .. } | StmtKind::SetPlace { place, value } => {
                self.rewrite_vm_ctfe_expr(place, scope)?;
                self.rewrite_vm_ctfe_expr(value, scope)
            }
            StmtKind::Return(Some(value)) | StmtKind::Expr(value) => {
                self.rewrite_vm_ctfe_expr(value, scope)
            }
            StmtKind::If { branches, orelse } => {
                for (cond, body) in branches {
                    self.rewrite_vm_ctfe_expr(cond, scope)?;
                    self.rewrite_vm_ctfe_block(body, scope)?;
                }
                if let Some(body) = orelse {
                    self.rewrite_vm_ctfe_block(body, scope)?;
                }
                Ok(())
            }
            StmtKind::While { cond, body, .. } => {
                self.rewrite_vm_ctfe_expr(cond, scope)?;
                self.rewrite_vm_ctfe_block(body, scope)
            }
            StmtKind::For { iter, body, .. } => {
                self.rewrite_vm_ctfe_expr(iter, scope)?;
                self.rewrite_vm_ctfe_block(body, scope)
            }
            StmtKind::Return(None) | StmtKind::Pass => Ok(()),
            _ => Ok(()),
        }
    }

    pub(super) fn rewrite_vm_ctfe_expr(
        &self,
        expr: &mut Expr,
        scope: &HashMap<String, CtValue>,
    ) -> Result<(), ComptimeError> {
        match &mut expr.kind {
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } if name == "is_same_type" => {
                for arg in param_args.iter_mut() {
                    if let ParamArg::Value(e) = arg {
                        self.rewrite_vm_ctfe_expr(e, scope)?;
                    }
                }
                for arg in args.iter_mut() {
                    self.rewrite_vm_ctfe_expr(arg, scope)?;
                }
                for kw in kwargs.iter_mut() {
                    self.rewrite_vm_ctfe_expr(&mut kw.value, scope)?;
                }
                let value = self.eval_is_same_type(param_args, args, scope)?;
                *expr = lit_result(&value, expr.span)?;
                Ok(())
            }
            ExprKind::Member { object, .. } => {
                self.rewrite_vm_ctfe_expr(object, scope)?;
                if let Ok(value) = self.eval(expr, scope)
                    && let Some(materialized) = value.materialize(expr.span)
                {
                    *expr = materialized;
                }
                Ok(())
            }
            ExprKind::Prefix(_, inner) | ExprKind::Transfer(inner) | ExprKind::Spread(inner) => {
                self.rewrite_vm_ctfe_expr(inner, scope)
            }
            ExprKind::Infix(_, left, right) => {
                self.rewrite_vm_ctfe_expr(left, scope)?;
                self.rewrite_vm_ctfe_expr(right, scope)
            }
            ExprKind::Call {
                param_args,
                args,
                kwargs,
                ..
            } => {
                for arg in param_args.iter_mut() {
                    if let ParamArg::Value(e) = arg {
                        self.rewrite_vm_ctfe_expr(e, scope)?;
                    }
                }
                for arg in args.iter_mut() {
                    self.rewrite_vm_ctfe_expr(arg, scope)?;
                }
                for kw in kwargs.iter_mut() {
                    self.rewrite_vm_ctfe_expr(&mut kw.value, scope)?;
                }
                Ok(())
            }
            ExprKind::MethodCall {
                object,
                args,
                kwargs,
                ..
            } => {
                self.rewrite_vm_ctfe_expr(object, scope)?;
                for arg in args.iter_mut() {
                    self.rewrite_vm_ctfe_expr(arg, scope)?;
                }
                for kw in kwargs.iter_mut() {
                    self.rewrite_vm_ctfe_expr(&mut kw.value, scope)?;
                }
                Ok(())
            }
            ExprKind::Index { object, index } => {
                self.rewrite_vm_ctfe_expr(object, scope)?;
                self.rewrite_vm_ctfe_expr(index, scope)
            }
            ExprKind::Slice {
                object,
                lower,
                upper,
                step,
                ..
            } => {
                self.rewrite_vm_ctfe_expr(object, scope)?;
                for bound in [lower, upper, step].into_iter().flatten() {
                    self.rewrite_vm_ctfe_expr(bound, scope)?;
                }
                Ok(())
            }
            ExprKind::MultiIndex { object, args } => {
                self.rewrite_vm_ctfe_expr(object, scope)?;
                for argument in args {
                    match argument {
                        crate::ast::SubscriptArg::Index(value)
                        | crate::ast::SubscriptArg::Keyword { value, .. } => {
                            self.rewrite_vm_ctfe_expr(value, scope)?
                        }
                        crate::ast::SubscriptArg::Slice {
                            lower, upper, step, ..
                        } => {
                            for value in [lower, upper, step].into_iter().flatten() {
                                self.rewrite_vm_ctfe_expr(value, scope)?;
                            }
                        }
                    }
                }
                Ok(())
            }
            ExprKind::ListLit(items) | ExprKind::TupleLit(items) => {
                for item in items {
                    self.rewrite_vm_ctfe_expr(item, scope)?;
                }
                Ok(())
            }
            ExprKind::BraceLit(entries) => {
                for (key, value) in entries {
                    self.rewrite_vm_ctfe_expr(key, scope)?;
                    if let Some(value) = value {
                        self.rewrite_vm_ctfe_expr(value, scope)?;
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
                for clause in clauses {
                    match clause {
                        crate::ast::ComprehensionClause::For { iter, .. } => {
                            self.rewrite_vm_ctfe_expr(iter, scope)?
                        }
                        crate::ast::ComprehensionClause::If(condition) => {
                            self.rewrite_vm_ctfe_expr(condition, scope)?
                        }
                    }
                }
                if let Some(key) = key {
                    self.rewrite_vm_ctfe_expr(key, scope)?;
                }
                self.rewrite_vm_ctfe_expr(value, scope)
            }
            ExprKind::Named { value, .. } => self.rewrite_vm_ctfe_expr(value, scope),
            ExprKind::IfExpr {
                cond,
                then_branch,
                else_branch,
            } => {
                self.rewrite_vm_ctfe_expr(cond, scope)?;
                self.rewrite_vm_ctfe_expr(then_branch, scope)?;
                self.rewrite_vm_ctfe_expr(else_branch, scope)
            }
            ExprKind::Compare { first, rest } => {
                self.rewrite_vm_ctfe_expr(first, scope)?;
                for (_, e) in rest {
                    self.rewrite_vm_ctfe_expr(e, scope)?;
                }
                Ok(())
            }
            ExprKind::TString { parts, .. } => {
                for part in parts {
                    if let crate::ast::TStringPart::Expr(e) = part {
                        self.rewrite_vm_ctfe_expr(e, scope)?;
                    }
                }
                Ok(())
            }
            ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Bool(_)
            | ExprKind::Str(_)
            | ExprKind::None
            | ExprKind::Uninitialized
            | ExprKind::Identifier(_)
            | ExprKind::TypeValue(_)
            | ExprKind::Invoke { .. }
            | ExprKind::TypeApply { .. } => Ok(()),
        }
    }

    pub(super) fn vm_ctfe_safe_fn(
        &self,
        name: &str,
        visiting: &mut HashSet<String>,
        needed: &mut HashSet<String>,
    ) -> bool {
        if needed.contains(name) {
            return true;
        }
        if !visiting.insert(name.to_string()) {
            needed.insert(name.to_string());
            return true;
        }
        let Some(f) = self.fns.get(name) else {
            visiting.remove(name);
            return false;
        };
        let safe = self.vm_ctfe_safe_block(f.body, visiting, needed);
        visiting.remove(name);
        if safe {
            needed.insert(name.to_string());
        }
        safe
    }

    pub(super) fn vm_ctfe_safe_block(
        &self,
        stmts: &[Stmt],
        visiting: &mut HashSet<String>,
        needed: &mut HashSet<String>,
    ) -> bool {
        stmts
            .iter()
            .all(|s| self.vm_ctfe_safe_stmt(s, visiting, needed))
    }

    pub(super) fn vm_ctfe_safe_stmt(
        &self,
        stmt: &Stmt,
        visiting: &mut HashSet<String>,
        needed: &mut HashSet<String>,
    ) -> bool {
        // This parallel walk is a purity/effect classifier: it discovers the
        // transitive helper set but never mutates or specializes the AST.
        match &stmt.kind {
            StmtKind::VarDecl { value, .. }
            | StmtKind::RefDecl { value, .. }
            | StmtKind::Assign { value, .. } => self.vm_ctfe_safe_expr(value, visiting, needed),
            StmtKind::AugAssign { place, value, .. } | StmtKind::SetPlace { place, value } => {
                self.vm_ctfe_safe_expr(place, visiting, needed)
                    && self.vm_ctfe_safe_expr(value, visiting, needed)
            }
            StmtKind::Return(Some(value)) | StmtKind::Expr(value) => {
                self.vm_ctfe_safe_expr(value, visiting, needed)
            }
            StmtKind::Return(None) | StmtKind::Pass => true,
            StmtKind::If { branches, orelse } => {
                branches.iter().all(|(cond, body)| {
                    self.vm_ctfe_safe_expr(cond, visiting, needed)
                        && self.vm_ctfe_safe_block(body, visiting, needed)
                }) && orelse
                    .as_ref()
                    .is_none_or(|body| self.vm_ctfe_safe_block(body, visiting, needed))
            }
            StmtKind::While { cond, body, .. } => {
                self.vm_ctfe_safe_expr(cond, visiting, needed)
                    && self.vm_ctfe_safe_block(body, visiting, needed)
            }
            StmtKind::For { iter, body, .. } => {
                self.vm_ctfe_safe_expr(iter, visiting, needed)
                    && self.vm_ctfe_safe_block(body, visiting, needed)
            }
            StmtKind::ComptimeIf { .. }
            | StmtKind::ComptimeFor { .. }
            | StmtKind::Raise(_)
            | StmtKind::Break
            | StmtKind::Continue
            | StmtKind::Def { .. }
            | StmtKind::Struct { .. }
            | StmtKind::Trait { .. }
            | StmtKind::Import { .. }
            | StmtKind::FromImport { .. }
            | StmtKind::With { .. }
            | StmtKind::Try { .. }
            | StmtKind::Unpack { .. }
            | StmtKind::Comptime { .. } => false,
        }
    }

    pub(super) fn vm_ctfe_safe_expr(
        &self,
        expr: &Expr,
        visiting: &mut HashSet<String>,
        needed: &mut HashSet<String>,
    ) -> bool {
        match &expr.kind {
            ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Bool(_)
            | ExprKind::Str(_)
            | ExprKind::None
            | ExprKind::Identifier(_) => true,
            ExprKind::Prefix(_, inner) | ExprKind::Transfer(inner) | ExprKind::Spread(inner) => {
                self.vm_ctfe_safe_expr(inner, visiting, needed)
            }
            ExprKind::Infix(_, left, right) => {
                self.vm_ctfe_safe_expr(left, visiting, needed)
                    && self.vm_ctfe_safe_expr(right, visiting, needed)
            }
            ExprKind::TupleLit(items) | ExprKind::ListLit(items) => items
                .iter()
                .all(|e| self.vm_ctfe_safe_expr(e, visiting, needed)),
            ExprKind::Index { object, index } => {
                self.vm_ctfe_safe_expr(object, visiting, needed)
                    && self.vm_ctfe_safe_expr(index, visiting, needed)
            }
            ExprKind::Member { object, .. } => self.vm_ctfe_safe_expr(object, visiting, needed),
            ExprKind::IfExpr {
                cond,
                then_branch,
                else_branch,
            } => {
                self.vm_ctfe_safe_expr(cond, visiting, needed)
                    && self.vm_ctfe_safe_expr(then_branch, visiting, needed)
                    && self.vm_ctfe_safe_expr(else_branch, visiting, needed)
            }
            ExprKind::Compare { first, rest } => {
                self.vm_ctfe_safe_expr(first, visiting, needed)
                    && rest
                        .iter()
                        .all(|(_, e)| self.vm_ctfe_safe_expr(e, visiting, needed))
            }
            ExprKind::Slice {
                object,
                lower,
                upper,
                step,
                ..
            } => {
                self.vm_ctfe_safe_expr(object, visiting, needed)
                    && lower
                        .as_ref()
                        .is_none_or(|e| self.vm_ctfe_safe_expr(e, visiting, needed))
                    && upper
                        .as_ref()
                        .is_none_or(|e| self.vm_ctfe_safe_expr(e, visiting, needed))
                    && step
                        .as_ref()
                        .is_none_or(|e| self.vm_ctfe_safe_expr(e, visiting, needed))
            }
            ExprKind::MultiIndex { object, args } => {
                self.vm_ctfe_safe_expr(object, visiting, needed)
                    && args.iter().all(|argument| match argument {
                        crate::ast::SubscriptArg::Index(value)
                        | crate::ast::SubscriptArg::Keyword { value, .. } => {
                            self.vm_ctfe_safe_expr(value, visiting, needed)
                        }
                        crate::ast::SubscriptArg::Slice {
                            lower, upper, step, ..
                        } => [lower, upper, step]
                            .into_iter()
                            .flatten()
                            .all(|value| self.vm_ctfe_safe_expr(value, visiting, needed)),
                    })
            }
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } => {
                kwargs.is_empty()
                    && param_args.iter().all(|arg| match arg {
                        ParamArg::Value(e) => self.vm_ctfe_safe_expr(e, visiting, needed),
                        ParamArg::Type(_) => true,
                        ParamArg::Named { value, .. } => match &**value {
                            ParamArg::Value(e) => self.vm_ctfe_safe_expr(e, visiting, needed),
                            ParamArg::Type(_) => true,
                            ParamArg::Named { .. } => false,
                        },
                    })
                    && args
                        .iter()
                        .all(|e| self.vm_ctfe_safe_expr(e, visiting, needed))
                    && (name == "is_same_type"
                        || vm_ctfe_safe_builtin(name)
                        || self.vm_ctfe_safe_fn(name, visiting, needed)
                        // A struct construction is safe when its constructor
                        // bodies are.
                        || self.vm_ctfe_safe_struct_ctors(name, visiting, needed))
            }
            ExprKind::MethodCall { .. }
            | ExprKind::BraceLit(_)
            | ExprKind::Comprehension { .. }
            | ExprKind::Invoke { .. }
            | ExprKind::TypeValue(_)
            | ExprKind::TypeApply { .. }
            | ExprKind::Named { .. }
            | ExprKind::TString { .. }
            | ExprKind::Uninitialized => false,
        }
    }
}
