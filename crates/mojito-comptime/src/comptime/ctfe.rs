//! Compile-time function evaluation via the VM (`ctfe_call`/`vm_ctfe_call`), the
//! VM-CTFE program rewrite, and the CTFE-safety analysis.
//! Extracted from `comptime.rs`; see `docs/symbol-map.md`.

use super::*;

/// The synthesized root of a struct-construction/static-method CTFE run.
const CTFE_STRUCT_ENTRY: &str = "$ctfe$struct$entry";
/// The synthesized root of a general compile-time expression run, and the
/// typing probe checked before it with the expression bound to a local.
const CTFE_EXPR_ENTRY: &str = "$ctfe$expr$entry";
const CTFE_PROBE: &str = "$ctfe$probe";
const CTFE_PROBE_RESULT: &str = "$ctfe$result";

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
        // A vector-keyed template crosses as its minted clones (`AHasher`
        // for `default_hasher`), whose bodies are the template's.
        for statement in self.program {
            if matches!(&statement.kind, StmtKind::Trait { .. })
                || matches!(&statement.kind, StmtKind::Struct { name, .. }
                    if !self.is_specializable(statement) || self.simd_keyed_struct_template(name))
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
        let mut program = self.vm_ctfe_subprogram(&declarations);
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
            syntax_id: mojito_common::token::SyntaxId::fresh(),
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
        let mut program = self.vm_ctfe_subprogram(&declarations);
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

    /// Evaluate a call to a type-parameterized free function
    /// (`hash[default_comp_time_hasher](Int(1))`) through a synthesized VM-CTFE
    /// entry: the entry spells the call with its type arguments folded to
    /// their bound types and its arguments as typed literals, so the checked
    /// boundary selects the overload, infers the inferred-only parameters,
    /// and reifies the constructible hasher type exactly as a runtime call.
    /// Every same-arity overload must pass the purity walk (the registry is
    /// name-keyed; the boundary picks among them).
    pub(super) fn ctfe_generic_def_entry(
        &self,
        name: &str,
        param_args: &[ParamArg],
        args: &[Expr],
        span: Span,
        scope: &HashMap<String, CtValue>,
    ) -> Result<CtValue, ComptimeError> {
        self.burn()?;
        let overloads: Vec<(&[TypeParam], &[Stmt], Option<&Type>)> = self
            .program
            .iter()
            .filter_map(|statement| match &statement.kind {
                StmtKind::Def {
                    name: candidate,
                    type_params,
                    params,
                    ret,
                    body,
                    ..
                } if candidate == name
                    && params
                        .iter()
                        .filter(|parameter| parameter.kind == ParamKind::Regular)
                        .count()
                        == args.len() =>
                {
                    Some((type_params.as_slice(), body.as_slice(), ret.as_ref()))
                }
                _ => None,
            })
            .collect();
        if overloads.is_empty() {
            return Err(ComptimeError::NotComptime(format!(
                "'{name}' has no overload taking {} argument(s)",
                args.len()
            )));
        }
        let mut visiting = HashSet::new();
        let mut needed = HashSet::new();
        needed.insert(name.to_string());
        for (_, body, _) in &overloads {
            if !self.vm_ctfe_safe_block(body, &mut visiting, &mut needed) {
                return Err(ComptimeError::NotComptime(format!(
                    "'{name}' is not safe for VM-backed compile-time execution"
                )));
            }
        }
        let ret = overloads
            .iter()
            .find_map(|(_, _, ret)| ret.cloned())
            .ok_or_else(|| {
                ComptimeError::NotComptime(format!("'{name}' returns no compile-time value"))
            })?;
        // The bound hasher types must also construct purely.
        for argument in param_args {
            if let Ok(Ty::Struct(struct_name, _)) = self.param_arg_type(argument, scope) {
                let template = self
                    .pending_struct_instances
                    .borrow()
                    .get(&struct_name)
                    .map(|(orig, _)| orig.clone())
                    .unwrap_or(struct_name);
                if !self.vm_ctfe_safe_struct_ctors(&template, &mut visiting, &mut needed) {
                    return Err(ComptimeError::NotComptime(format!(
                        "'{template}' is not safe for VM-backed compile-time execution"
                    )));
                }
            }
        }
        let expr = |kind: ExprKind| Expr {
            kind,
            span,
            source: None,
            syntax_id: mojito_common::token::SyntaxId::fresh(),
        };
        // Typed literals keep the argument's scalar type (`Int(1)` stays an
        // `Int`, not an exact literal the callee would re-materialize).
        let mut literal_args = Vec::with_capacity(args.len());
        for argument in args {
            let value = self.eval(argument, scope)?;
            let literal = value.materialize(span).ok_or_else(|| {
                ComptimeError::NotComptime(format!(
                    "compile-time argument {value} has no runtime form"
                ))
            })?;
            let wrapper = match value {
                CtValue::Int(_) => Some("Int"),
                CtValue::UInt(_) => Some("UInt"),
                CtValue::Float(_) => Some("Float64"),
                _ => None,
            };
            literal_args.push(match wrapper {
                Some(wrapper) => expr(ExprKind::Call {
                    name: wrapper.to_string(),
                    param_args: Vec::new(),
                    args: vec![literal],
                    kwargs: Vec::new(),
                }),
                None => literal,
            });
        }
        let mut folded_params = Vec::with_capacity(param_args.len());
        for argument in param_args {
            let ty = self.param_arg_type(argument, scope)?;
            let source = source_type_from_ty(&ty).ok_or_else(|| {
                ComptimeError::NotComptime(format!(
                    "compile-time type argument '{ty}' has no source spelling"
                ))
            })?;
            folded_params.push(ParamArg::Type(source));
        }
        let call = expr(ExprKind::Call {
            name: name.to_string(),
            param_args: folded_params,
            args: literal_args,
            kwargs: Vec::new(),
        });
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
        let mut program = self.vm_ctfe_subprogram(&declarations);
        program.push(entry);
        let (value, remaining_fuel) = vm
            .run_function_value(
                &program,
                CTFE_STRUCT_ENTRY,
                Vec::new(),
                &[],
                self.fuel.get(),
            )
            .map_err(|e| ComptimeError::NotComptime(format!("VM CTFE failed for '{name}': {e}")))?;
        self.fuel.set(remaining_fuel);
        self.vm_value_to_ct(value)
    }

    /// Evaluate a general expression over compile-time values — a method
    /// call, subscript, or free call whose receiver or argument is a
    /// compile-time collection or struct (`M.get("a").value()`) — through a
    /// synthesized VM-CTFE entry. Collection bindings become locals
    /// initialized from their materialized displays, so the checked boundary
    /// types and dispatches the expression exactly as a runtime body would;
    /// every other binding is inlined as its literal. The result type is not
    /// known before checking, so a probe definition binding the expression
    /// to a local is checked first: its inferred type spells the entry's
    /// return, and its non-raising signature is what reports a raising call
    /// (upstream's rule for a comptime initializer).
    pub(super) fn ctfe_expr_entry(
        &self,
        expr: &Expr,
        scope: &HashMap<String, CtValue>,
    ) -> Result<CtValue, ComptimeError> {
        self.burn()?;
        let span = expr.span;
        let collections: RefCell<Vec<String>> = RefCell::new(Vec::new());
        let subs: super::rewrite::Subs = &|name| {
            let value = scope.get(name)?;
            if value.is_runtime_collection() {
                let mut seen = collections.borrow_mut();
                if !seen.iter().any(|seen| seen == name) {
                    seen.push(name.to_string());
                }
                None
            } else {
                Some(value.clone())
            }
        };
        let mut body = expr.clone();
        super::rewrite::rewrite_expr(&mut body, subs);
        let mut prologue = Vec::new();
        for name in collections.into_inner() {
            let value = &scope[&name];
            let display = value.materialize(span).ok_or_else(|| {
                ComptimeError::NotComptime(format!(
                    "compile-time value {value} has no runtime form"
                ))
            })?;
            prologue.push(mk(
                StmtKind::VarDecl {
                    name,
                    ty: None,
                    value: display,
                },
                span,
            ));
        }
        let mut visiting = HashSet::new();
        let mut needed = HashSet::new();
        let safe = prologue
            .iter()
            .all(|stmt| self.vm_ctfe_safe_stmt(stmt, &mut visiting, &mut needed))
            && self.vm_ctfe_safe_expr(&body, &mut visiting, &mut needed);
        if !safe {
            return Err(ComptimeError::NotComptime(
                "a compile-time expression reaches an effectful builtin and is not safe for \
                 VM-backed compile-time execution"
                    .to_string(),
            ));
        }
        let declarations = self.vm_ctfe_declaration_closure(&needed);
        let mut program = self.vm_ctfe_subprogram(&declarations);
        // The typing probe: `var $r = <expr>` inside a non-raising def. The
        // expression's own node gets a fresh identity — a rewritten chain may
        // share syntax ids among its nodes, and the checker re-keys
        // duplicates — so the checked type is read back at exactly this node.
        body.syntax_id = mojito_common::token::SyntaxId::fresh();
        let key = body.source_span();
        let mut probe_body = prologue.clone();
        probe_body.push(mk(
            StmtKind::VarDecl {
                name: CTFE_PROBE_RESULT.to_string(),
                ty: None,
                value: body.clone(),
            },
            span,
        ));
        program.push(synthesized_entry(CTFE_PROBE, None, probe_body, span));
        let checked = mojito_checker::checker::check_program(&program).map_err(|error| {
            ComptimeError::NotComptime(match error {
                mojito_common::error::TypeError::UnhandledRaise(_) => {
                    "cannot call raising function in comptime initializer".to_string()
                }
                other => format!("a compile-time expression failed the checked boundary: {other}"),
            })
        })?;
        let ty = checked
            .expression_ids_at(&key)
            .iter()
            .find_map(|id| {
                let expression = checked.expression(*id)?;
                expression
                    .binding_ty
                    .clone()
                    .or_else(|| expression.ty.clone())
            })
            .ok_or_else(|| {
                ComptimeError::NotComptime(
                    "a compile-time expression has no checked type".to_string(),
                )
            })?;
        let ret = source_type_from_ty(&ty).ok_or_else(|| {
            ComptimeError::NotComptime(format!(
                "compile-time result type '{ty}' has no source spelling"
            ))
        })?;
        program.pop();
        let mut entry_body = prologue;
        entry_body.push(mk(StmtKind::Return(Some(body)), span));
        program.push(synthesized_entry(
            CTFE_EXPR_ENTRY,
            Some(ret),
            entry_body,
            span,
        ));
        let mut vm = VmBackend::new();
        let (value, remaining_fuel) = vm
            .run_function_value(&program, CTFE_EXPR_ENTRY, Vec::new(), &[], self.fuel.get())
            .map_err(|e| {
                ComptimeError::NotComptime(format!(
                    "VM CTFE failed for a compile-time expression: {e}"
                ))
            })?;
        self.fuel.set(remaining_fuel);
        self.vm_value_to_ct(value).map_err(|error| {
            ComptimeError::NotComptime(format!(
                "a compile-time '{ty}' result cannot cross back from VM CTFE ({error}); bind a \
                 scalar, Bool, String, tuple, fieldwise struct, or a display instead"
            ))
        })
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
    /// struct passes the effect walk (a struct construction inside CTFE
    /// code); a name that is not such a struct is not a construction.
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
        let safe = self.program.iter().all(|stmt| match &stmt.kind {
            // A vector-keyed template (`AHasher[key: U256]`) crosses as its
            // clones, whose constructors are the template's.
            StmtKind::Struct { name, methods, .. }
                if name == struct_name
                    && (!self.is_specializable(stmt)
                        || self.simd_keyed_struct_template(struct_name)) =>
            {
                methods
                    .iter()
                    .filter(|method| method.name == "__init__")
                    .all(|method| self.vm_ctfe_safe_block(&method.body, visiting, needed))
            }
            _ => true,
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
                        mojito_ast::ast::SubscriptArg::Index(value)
                        | mojito_ast::ast::SubscriptArg::Keyword { value, .. } => {
                            self.rewrite_vm_ctfe_expr(value, scope)?
                        }
                        mojito_ast::ast::SubscriptArg::Slice {
                            lower, upper, step, ..
                        }
                        | mojito_ast::ast::SubscriptArg::KeywordSlice {
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
                        mojito_ast::ast::ComprehensionClause::For { iter, .. } => {
                            self.rewrite_vm_ctfe_expr(iter, scope)?
                        }
                        mojito_ast::ast::ComprehensionClause::If(condition) => {
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
                    if let mojito_ast::ast::TStringPart::Expr(e) = part {
                        self.rewrite_vm_ctfe_expr(e, scope)?;
                    }
                }
                Ok(())
            }
            // A lambda's hidden definition body is a separate function scope,
            // mirroring the `StmtKind::Def` skip in the statement rewriter.
            ExprKind::Lambda { .. } => Ok(()),
            ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Bool(_)
            | ExprKind::Str(_)
            | ExprKind::None
            | ExprKind::Uninitialized
            | ExprKind::EmptySubscript
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
        // Not a free def: a builtin, a struct constructor, or a constructible
        // type parameter (`HasherType()`) — the checked boundary decides.
        let Some(f) = self.fns.get(name) else {
            visiting.remove(name);
            return true;
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

    /// The effect classifier over one statement; see `vm_ctfe_safe_expr`.
    pub(super) fn vm_ctfe_safe_stmt(
        &self,
        stmt: &Stmt,
        visiting: &mut HashSet<String>,
        needed: &mut HashSet<String>,
    ) -> bool {
        // This parallel walk is an effect classifier: it discovers the
        // transitive helper set but never mutates or specializes the AST.
        match &stmt.kind {
            StmtKind::VarDecl { value, .. }
            | StmtKind::RefDecl { value, .. }
            | StmtKind::Assign { value, .. }
            | StmtKind::Unpack { value, .. }
            | StmtKind::Return(Some(value))
            | StmtKind::Expr(value)
            | StmtKind::Raise(value)
            | StmtKind::Comptime { value, .. } => self.vm_ctfe_safe_expr(value, visiting, needed),
            StmtKind::AugAssign { place, value, .. } | StmtKind::SetPlace { place, value } => {
                self.vm_ctfe_safe_expr(place, visiting, needed)
                    && self.vm_ctfe_safe_expr(value, visiting, needed)
            }
            StmtKind::Return(None) | StmtKind::Pass | StmtKind::Break | StmtKind::Continue => true,
            StmtKind::If { branches, orelse } | StmtKind::ComptimeIf { branches, orelse } => {
                branches.iter().all(|(cond, body)| {
                    self.vm_ctfe_safe_expr(cond, visiting, needed)
                        && self.vm_ctfe_safe_block(body, visiting, needed)
                }) && orelse
                    .as_ref()
                    .is_none_or(|body| self.vm_ctfe_safe_block(body, visiting, needed))
            }
            StmtKind::While { cond, body, orelse } => {
                self.vm_ctfe_safe_expr(cond, visiting, needed)
                    && self.vm_ctfe_safe_block(body, visiting, needed)
                    && orelse
                        .as_ref()
                        .is_none_or(|body| self.vm_ctfe_safe_block(body, visiting, needed))
            }
            StmtKind::For {
                iter, body, orelse, ..
            } => {
                self.vm_ctfe_safe_expr(iter, visiting, needed)
                    && self.vm_ctfe_safe_block(body, visiting, needed)
                    && orelse
                        .as_ref()
                        .is_none_or(|body| self.vm_ctfe_safe_block(body, visiting, needed))
            }
            StmtKind::ComptimeFor { iter, body, .. } => {
                self.vm_ctfe_safe_expr(iter, visiting, needed)
                    && self.vm_ctfe_safe_block(body, visiting, needed)
            }
            StmtKind::Try {
                body,
                except,
                orelse,
                finalbody,
            } => {
                self.vm_ctfe_safe_block(body, visiting, needed)
                    && except
                        .as_ref()
                        .is_none_or(|(_, body)| self.vm_ctfe_safe_block(body, visiting, needed))
                    && orelse
                        .as_ref()
                        .is_none_or(|body| self.vm_ctfe_safe_block(body, visiting, needed))
                    && finalbody
                        .as_ref()
                        .is_none_or(|body| self.vm_ctfe_safe_block(body, visiting, needed))
            }
            StmtKind::With { items, body } => {
                items
                    .iter()
                    .all(|item| self.vm_ctfe_safe_expr(&item.context, visiting, needed))
                    && self.vm_ctfe_safe_block(body, visiting, needed)
            }
            StmtKind::Def { body, .. } => self.vm_ctfe_safe_block(body, visiting, needed),
            // Declarations never appear inside an executed body.
            StmtKind::Struct { .. }
            | StmtKind::Trait { .. }
            | StmtKind::Import { .. }
            | StmtKind::FromImport { .. } => false,
        }
    }

    /// The effect classifier over one expression. VM CTFE runs any
    /// deterministic body the fuel-bounded VM executes — collection displays,
    /// pointer-backed stdlib internals, loops, `try`/`raise` inside a callee
    /// — so the walk rejects only the effectful builtins (`print`, `input`);
    /// a raising call at the entry is reported by the checked boundary and
    /// a trap surfaces at execution. Along the way it discovers the free
    /// callees the subprogram must retain (`needed`); `visiting` carries the
    /// cycle guards.
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
            | ExprKind::EmptySubscript
            | ExprKind::Identifier(_)
            | ExprKind::TypeValue(_)
            | ExprKind::Uninitialized => true,
            ExprKind::Prefix(_, inner)
            | ExprKind::Transfer(inner)
            | ExprKind::Spread(inner)
            | ExprKind::Named { value: inner, .. } => {
                self.vm_ctfe_safe_expr(inner, visiting, needed)
            }
            ExprKind::Infix(_, left, right) => {
                self.vm_ctfe_safe_expr(left, visiting, needed)
                    && self.vm_ctfe_safe_expr(right, visiting, needed)
            }
            ExprKind::TupleLit(items) | ExprKind::ListLit(items) => items
                .iter()
                .all(|e| self.vm_ctfe_safe_expr(e, visiting, needed)),
            ExprKind::BraceLit(entries) => entries.iter().all(|(key, value)| {
                self.vm_ctfe_safe_expr(key, visiting, needed)
                    && value
                        .as_ref()
                        .is_none_or(|value| self.vm_ctfe_safe_expr(value, visiting, needed))
            }),
            ExprKind::Comprehension {
                key,
                value,
                clauses,
                ..
            } => {
                key.as_ref()
                    .is_none_or(|key| self.vm_ctfe_safe_expr(key, visiting, needed))
                    && self.vm_ctfe_safe_expr(value, visiting, needed)
                    && clauses.iter().all(|clause| match clause {
                        mojito_ast::ast::ComprehensionClause::For { iter, .. } => {
                            self.vm_ctfe_safe_expr(iter, visiting, needed)
                        }
                        mojito_ast::ast::ComprehensionClause::If(condition) => {
                            self.vm_ctfe_safe_expr(condition, visiting, needed)
                        }
                    })
            }
            ExprKind::Lambda { def } => self.vm_ctfe_safe_stmt(def, visiting, needed),
            ExprKind::TString { parts, .. } => parts.iter().all(|part| match part {
                mojito_ast::ast::TStringPart::Expr(value) => {
                    self.vm_ctfe_safe_expr(value, visiting, needed)
                }
                _ => true,
            }),
            ExprKind::TypeApply { args, .. } => {
                self.vm_ctfe_safe_param_args(args, visiting, needed)
            }
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
                    && [lower, upper, step]
                        .into_iter()
                        .flatten()
                        .all(|e| self.vm_ctfe_safe_expr(e, visiting, needed))
            }
            ExprKind::MultiIndex { object, args } => {
                self.vm_ctfe_safe_expr(object, visiting, needed)
                    && args.iter().all(|argument| match argument {
                        mojito_ast::ast::SubscriptArg::Index(value)
                        | mojito_ast::ast::SubscriptArg::Keyword { value, .. } => {
                            self.vm_ctfe_safe_expr(value, visiting, needed)
                        }
                        mojito_ast::ast::SubscriptArg::Slice {
                            lower, upper, step, ..
                        }
                        | mojito_ast::ast::SubscriptArg::KeywordSlice {
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
                !vm_ctfe_effectful_builtin(name)
                    && self.vm_ctfe_safe_param_args(param_args, visiting, needed)
                    && args
                        .iter()
                        .all(|e| self.vm_ctfe_safe_expr(e, visiting, needed))
                    && kwargs
                        .iter()
                        .all(|argument| self.vm_ctfe_safe_expr(&argument.value, visiting, needed))
                    && self.vm_ctfe_safe_fn(name, visiting, needed)
                    && self.vm_ctfe_safe_struct_ctors(name, visiting, needed)
            }
            ExprKind::MethodCall {
                object,
                method,
                args,
                kwargs,
            } => {
                self.vm_ctfe_safe_expr(object, visiting, needed)
                    && args
                        .iter()
                        .all(|e| self.vm_ctfe_safe_expr(e, visiting, needed))
                    && kwargs
                        .iter()
                        .all(|argument| self.vm_ctfe_safe_expr(&argument.value, visiting, needed))
                    && self.vm_ctfe_safe_method(method, visiting, needed)
            }
            ExprKind::Invoke {
                callee,
                param_args,
                args,
                kwargs,
            } => {
                self.vm_ctfe_safe_expr(callee, visiting, needed)
                    && self.vm_ctfe_safe_param_args(param_args, visiting, needed)
                    && args
                        .iter()
                        .all(|e| self.vm_ctfe_safe_expr(e, visiting, needed))
                    && kwargs
                        .iter()
                        .all(|argument| self.vm_ctfe_safe_expr(&argument.value, visiting, needed))
                    && match &callee.kind {
                        ExprKind::Member { field, .. } => {
                            self.vm_ctfe_safe_method(field, visiting, needed)
                        }
                        _ => true,
                    }
            }
        }
    }

    fn vm_ctfe_safe_param_args(
        &self,
        args: &[ParamArg],
        visiting: &mut HashSet<String>,
        needed: &mut HashSet<String>,
    ) -> bool {
        args.iter().all(|arg| match arg {
            ParamArg::Value(e) => self.vm_ctfe_safe_expr(e, visiting, needed),
            ParamArg::Type(_) => true,
            ParamArg::Named { value, .. } => match &**value {
                ParamArg::Value(e) => self.vm_ctfe_safe_expr(e, visiting, needed),
                ParamArg::Type(_) => true,
                ParamArg::Named { .. } => false,
            },
        })
    }

    /// Whether every retained struct's method bodies named `method` pass the
    /// effect walk: the receiver's struct is unknown before checking, so all
    /// same-named bodies are classified. Memoized in `needed` under a
    /// `$method$` prefix the declaration closure ignores (only free defs are
    /// retained by name).
    fn vm_ctfe_safe_method(
        &self,
        method: &str,
        visiting: &mut HashSet<String>,
        needed: &mut HashSet<String>,
    ) -> bool {
        let guard = format!("$method${method}");
        if needed.contains(&guard) || !visiting.insert(guard.clone()) {
            return true;
        }
        let safe = self.program.iter().all(|stmt| match &stmt.kind {
            StmtKind::Struct { name, methods, .. }
                if !self.is_specializable(stmt) || self.simd_keyed_struct_template(name) =>
            {
                methods
                    .iter()
                    .filter(|candidate| candidate.name == method)
                    .all(|candidate| self.vm_ctfe_safe_block(&candidate.body, visiting, needed))
            }
            _ => true,
        });
        visiting.remove(&guard);
        if safe {
            needed.insert(guard);
        }
        safe
    }
}

impl Elab<'_> {
    /// The declaration environment of a VM-CTFE subprogram. The checked
    /// boundary needs declarations, not just the transitively executed
    /// bodies: retain the free-callee closure plus every checkable struct,
    /// trait, and literal constant so trait bounds, struct types, overloads,
    /// and helper calls resolve exactly, then fold module-scope type aliases
    /// into the retained statements. The VM still executes only the entry.
    fn vm_ctfe_subprogram(&self, declarations: &HashSet<String>) -> Vec<Stmt> {
        let program = self
            .program
            .iter()
            .filter(|stmt| match &stmt.kind {
                // Only the semantic free-callee graph crosses this checked
                // boundary. Linked `$` spelling is neither a dependency nor a
                // specialization test.
                //
                // The nominal `std.range` defs return the DType-value-param
                // range-family structs, whose templates cannot cross this
                // boundary (they are monomorphizer inputs, excluded below)
                // and whose specializations do not exist yet during
                // elaboration. Dropping the defs routes a CTFE `range(...)`
                // call through the checker's focused Int intrinsic and the
                // VM's own compile-time range materialization instead.
                StmtKind::Def { name, .. } => {
                    declarations.contains(name)
                        && !stmt.module.as_deref().is_some_and(|module| {
                            std::path::Path::new(module).ends_with("std/range.mojo")
                        })
                }
                // A variadic struct template is a monomorphizer input and cannot
                // cross the ordinary checked boundary. Concrete CTFE uses have
                // already been specialized; an unused public `Tuple[*Ts]`
                // template must not invalidate an otherwise scalar subprogram.
                StmtKind::Struct {
                    name, type_params, ..
                } => {
                    !self.is_specializable(stmt)
                        || (type_params
                            .iter()
                            .any(|parameter| parameter.name.starts_with('*'))
                            && !matches!(
                                name.rsplit('$').next().unwrap_or(name),
                                "Tuple" | "TString"
                            ))
                }
                StmtKind::Trait { .. } => true,
                // Module-scope literal constants are part of the declaration
                // environment elaboration would otherwise fold into the
                // retained bodies (a hasher body names its multiplier). Type
                // aliases have no runtime statement form; they are folded
                // into the retained declarations below.
                StmtKind::Comptime {
                    type_params, value, ..
                } => {
                    type_params.is_empty()
                        && matches!(
                            value.kind,
                            ExprKind::Int(_)
                                | ExprKind::Float(_)
                                | ExprKind::Bool(_)
                                | ExprKind::Str(_)
                        )
                }
                _ => false,
            })
            .cloned()
            .map(|stmt| {
                // A variadic template a retained body applies over its own
                // parameters (`TypeNames[Self.T]()` in `List.write_repr_to`)
                // crosses as the shell the pre-check elaboration emits; the
                // public `Tuple`/`TString` keep their own machinery.
                if let StmtKind::Struct {
                    name, type_params, ..
                } = &stmt.kind
                    && type_params
                        .iter()
                        .any(|parameter| parameter.name.starts_with('*'))
                    && !matches!(name.rsplit('$').next().unwrap_or(name), "Tuple" | "TString")
                {
                    return super::specialize::template_shell(&stmt);
                }
                stmt
            })
            .collect::<Vec<_>>();
        // Module-scope literal constants fold into the retained bodies as the
        // production elaboration folds them: a retained `Comptime` statement
        // binds a module-level name the checker types but no function-level
        // MIR can read (a string constant a retained path helper names). The
        // retained statements themselves supply the values — a constant
        // declared in a module elaborated after this evaluation is not yet in
        // `top_consts` — beneath the already-elaborated constants.
        let mut consts: HashMap<String, CtValue> = program
            .iter()
            .filter_map(|statement| match &statement.kind {
                StmtKind::Comptime { name, value, .. } => {
                    let value = match &value.kind {
                        ExprKind::Int(value) => CtValue::IntLiteral(value.clone()),
                        ExprKind::Float(value) => CtValue::FloatLiteral(value.clone()),
                        ExprKind::Bool(value) => CtValue::Bool(*value),
                        ExprKind::Str(value) => CtValue::Str(value.clone()),
                        _ => return None,
                    };
                    Some((name.clone(), value))
                }
                _ => None,
            })
            .collect();
        consts.extend(self.top_consts.borrow().clone());
        let type_names: HashSet<String> = program
            .iter()
            .filter_map(|statement| match &statement.kind {
                StmtKind::Struct { name, .. } => Some(name.clone()),
                _ => None,
            })
            .collect();
        let mut program = super::rewrite::materialize_block(program, &consts, &type_names);
        // A retained struct's method that only elaborates with its own
        // compile-time parameters bound (a `comptime for` over a method pack,
        // `FormatStruct.params`) crosses the boundary as the trap stub the
        // pre-check elaboration installs; no compile-time program calls it
        // unspecialized.
        for statement in &mut program {
            let StmtKind::Struct { name, methods, .. } = &mut statement.kind else {
                continue;
            };
            for method in methods.iter_mut() {
                if !method.type_params.is_empty() && super::block_has_comptime(&method.body) {
                    method.body = vec![super::specialize::unspecialized_method_stub(name, method)];
                }
            }
        }
        // A SIMD-keyed hasher method crosses as its stub plus the eager
        // per-leaf clones: the subprogram has no discovery loop to mint them.
        let consts = self.top_consts.borrow().clone();
        for statement in &mut program {
            let requests = super::synth::hasher_leaf_requests(statement, &self.hash_leaf_types);
            let StmtKind::Struct { name, methods, .. } = &mut statement.kind else {
                continue;
            };
            let mut clones = Vec::new();
            for method in methods.iter_mut() {
                if super::synth::is_simd_keyed_method(method) {
                    clones.extend(self.per_call_method_clones(
                        method,
                        &requests,
                        &[],
                        &[],
                        None,
                        &consts,
                    ));
                    method.body = vec![super::specialize::unspecialized_method_stub(name, method)];
                }
            }
            methods.extend(clones);
        }
        // Evaluating the aliases registers the vector-keyed specializations
        // they name (`comptime default_hasher = AHasher[...]`); a retained
        // declaration reaches such a clone through the alias (`H: Hasher =
        // default_hasher`), so the clone crosses too — the template itself is
        // a monomorphizer input excluded above.
        let type_aliases = self.vm_ctfe_type_aliases();
        let pending: Vec<(String, Vec<CtValue>)> = self
            .pending_struct_instances
            .borrow()
            .values()
            .cloned()
            .collect();
        for (orig, vals) in pending {
            let Ok(spec) = self.generate_value_struct_spec(&orig, &vals) else {
                continue;
            };
            // The clone sits where its template was declared, so a retained
            // declaration that names it resolves in order at the boundary.
            let template_position = self
                .program
                .iter()
                .position(|statement| {
                    matches!(&statement.kind, StmtKind::Struct { name, .. } if *name == orig)
                })
                .unwrap_or(usize::MAX);
            let original_index = |statement: &Stmt| {
                self.program.iter().position(|original| {
                    original.span == statement.span && original.module == statement.module
                })
            };
            let insert_at = program
                .iter()
                .position(|statement| {
                    original_index(statement).is_some_and(|index| index > template_position)
                })
                .unwrap_or(program.len());
            program.insert(insert_at, spec);
        }
        if !type_aliases.is_empty() {
            let subs = |alias: &str| type_aliases.get(alias).cloned();
            for statement in &mut program {
                *statement = super::rewrite::rewrite_stmt_cloned(statement, &subs, true);
            }
        }
        program
    }

    /// The module-scope type aliases (`comptime default_hasher = AHasher`)
    /// elaboration folds into every use before the ordinary checker runs.
    /// The VM-CTFE subprogram is checked from the linked source, so the same
    /// fold is applied to its retained declarations.
    fn vm_ctfe_type_aliases(&self) -> HashMap<String, CtValue> {
        let mut aliases = HashMap::new();
        for statement in self.program {
            let StmtKind::Comptime {
                name,
                type_params,
                value,
                ..
            } = &statement.kind
            else {
                continue;
            };
            // A single non-scalar bracket argument parses as indexing
            // (`AHasher[SIMD[DType.uint64, 4](0)]`); that is a type alias too.
            if !type_params.is_empty()
                || !matches!(
                    value.kind,
                    ExprKind::Identifier(_)
                        | ExprKind::TypeApply { .. }
                        | ExprKind::TypeValue(_)
                        | ExprKind::Index { .. }
                )
            {
                continue;
            }
            let env = HashMap::new();
            if let Ok(ty @ CtValue::Type(_)) = self.eval(value, &env) {
                aliases.insert(name.clone(), ty);
            }
        }
        aliases
    }
}

/// A parameterless synthesized entry definition with the given body.
fn synthesized_entry(name: &str, ret: Option<Type>, body: Vec<Stmt>, span: Span) -> Stmt {
    mk(
        StmtKind::Def {
            name: name.to_string(),
            decorators: Vec::new(),
            type_params: Vec::new(),
            params: Vec::new(),
            positional_only: None,
            keyword_only: None,
            captures: None,
            raises: false,
            raises_type: None,
            ret,
            where_clauses: Vec::new(),
            body,
        },
        span,
    )
}
