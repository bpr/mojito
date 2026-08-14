//! Declaration collection, signature construction, and body-checking support.

use super::*;

pub(super) fn definitely_returns(body: &[Stmt]) -> bool {
    body.iter().any(stmt_returns)
}

pub(super) fn stmt_returns(stmt: &Stmt) -> bool {
    match &stmt.kind {
        StmtKind::Return(_) => true,
        // A `raise` diverges (it never falls through to the end), so for
        // reachability it behaves like a `return`.
        StmtKind::Raise(_) => true,
        StmtKind::If { branches, orelse } => {
            orelse.as_ref().is_some_and(|e| definitely_returns(e))
                && branches.iter().all(|(_, b)| definitely_returns(b))
        }
        // A `try` definitely diverges when: a `finally` does (it overrides every
        // path); or the **normal-completion** path diverges (the body — or, if the
        // body may complete, the `else`) *and* the **exceptional** path does (every
        // `except` handler diverges; with no handler, an uncaught raise itself
        // exits, so only the normal path can fall through).
        StmtKind::Try {
            body,
            except,
            orelse,
            finalbody,
        } => {
            if finalbody.as_ref().is_some_and(|fb| definitely_returns(fb)) {
                return true;
            }
            let normal = match orelse {
                Some(else_) => definitely_returns(body) || definitely_returns(else_),
                None => definitely_returns(body),
            };
            let exceptional = match except {
                Some((_, handler)) => definitely_returns(handler),
                None => true,
            };
            normal && exceptional
        }
        _ => false,
    }
}

/// Conservative definite-initialization check for a named `out` result. A
/// value-returning path supplies the result directly; a fallthrough or bare
/// return path must assign the named result first.
pub(super) fn definitely_initializes_named_result(body: &[Stmt], name: &str) -> bool {
    let mut initialized = false;
    for stmt in body {
        match &stmt.kind {
            StmtKind::Assign { name: target, .. } if target == name => {
                initialized = true;
            }
            StmtKind::Return(Some(_)) | StmtKind::Raise(_) => return true,
            StmtKind::Return(None) => return initialized,
            StmtKind::If { branches, orelse } => {
                let Some(orelse) = orelse else { continue };
                if branches
                    .iter()
                    .all(|(_, branch)| definitely_initializes_named_result(branch, name))
                    && definitely_initializes_named_result(orelse, name)
                {
                    return true;
                }
            }
            _ => {}
        }
    }
    initialized
}

/// Whether every normally completing path initializes `self.field`. A raised
/// path does not produce a value and therefore need not initialize the field;
/// every explicit return and every fallthrough path does. Loops are treated as
/// possibly executing zero times.
pub(super) fn definitely_initializes_self_field(body: &[Stmt], field: &str) -> bool {
    let flow = init_field_flow(body, field, false);
    flow.valid && flow.normal.is_none_or(|initialized| initialized)
}

#[derive(Clone, Copy)]
struct InitFieldFlow {
    /// Initialization state on normal fallthrough, or `None` when no path falls
    /// through (all paths returned or raised).
    normal: Option<bool>,
    /// Every value-producing exit seen so far was initialized.
    valid: bool,
}

fn init_field_flow(body: &[Stmt], field: &str, mut initialized: bool) -> InitFieldFlow {
    let mut valid = true;
    for stmt in body {
        match &stmt.kind {
            StmtKind::SetPlace { place, .. }
                if matches!(
                    &place.kind,
                    ExprKind::Member { object, field: assigned }
                        if assigned == field
                            && matches!(&object.kind, ExprKind::Identifier(name) if name == "self")
                ) =>
            {
                initialized = true;
            }
            StmtKind::Return(_) => {
                return InitFieldFlow {
                    normal: None,
                    valid: valid && initialized,
                };
            }
            StmtKind::Raise(_) => {
                return InitFieldFlow {
                    normal: None,
                    valid,
                };
            }
            StmtKind::If { branches, orelse } => {
                let mut flows: Vec<_> = branches
                    .iter()
                    .map(|(_, branch)| init_field_flow(branch, field, initialized))
                    .collect();
                flows.push(match orelse {
                    Some(orelse) => init_field_flow(orelse, field, initialized),
                    None => InitFieldFlow {
                        normal: Some(initialized),
                        valid: true,
                    },
                });
                valid &= flows.iter().all(|flow| flow.valid);
                let mut normal_paths = flows.iter().filter_map(|flow| flow.normal);
                initialized = match normal_paths.next() {
                    Some(first) => normal_paths.fold(first, |all, state| all && state),
                    None => {
                        return InitFieldFlow {
                            normal: None,
                            valid,
                        };
                    }
                };
            }
            // A loop may execute zero times, so assignments in its body cannot
            // establish initialization after the loop. Returns inside it still
            // have to be safe.
            StmtKind::While { body, .. } | StmtKind::For { body, .. } => {
                valid &= init_field_flow(body, field, initialized).valid;
            }
            StmtKind::Try {
                body,
                except,
                orelse,
                finalbody,
            } => {
                let body_flow = init_field_flow(body, field, initialized);
                valid &= body_flow.valid;
                let normal_flow = body_flow.normal.map(|state| match orelse {
                    Some(orelse) => init_field_flow(orelse, field, state),
                    None => InitFieldFlow {
                        normal: Some(state),
                        valid: true,
                    },
                });
                let exceptional_flow = except
                    .as_ref()
                    .map(|(_, handler)| init_field_flow(handler, field, initialized));
                valid &= normal_flow.is_none_or(|flow| flow.valid)
                    && exceptional_flow.is_none_or(|flow| flow.valid);

                let mut exits: Vec<bool> = normal_flow
                    .and_then(|flow| flow.normal)
                    .into_iter()
                    .chain(exceptional_flow.and_then(|flow| flow.normal))
                    .collect();
                if except.is_none() && body_flow.normal.is_none() {
                    exits.clear();
                }
                if let Some(finalbody) = finalbody {
                    let starts = if exits.is_empty() {
                        vec![initialized]
                    } else {
                        exits
                    };
                    let final_flows: Vec<_> = starts
                        .into_iter()
                        .map(|state| init_field_flow(finalbody, field, state))
                        .collect();
                    valid &= final_flows.iter().all(|flow| flow.valid);
                    exits = final_flows.iter().filter_map(|flow| flow.normal).collect();
                }
                if exits.is_empty() {
                    return InitFieldFlow {
                        normal: None,
                        valid,
                    };
                }
                initialized = exits.into_iter().all(|state| state);
            }
            _ => {}
        }
    }
    InitFieldFlow {
        normal: Some(initialized),
        valid,
    }
}

/// Method/function signature classification and body checking moved from `checker.rs`.
impl Checker {
    pub(super) fn method_sig(
        &self,
        method: &Method,
        decls: Vec<ParamDecl>,
        all_types: &[Ty],
    ) -> Result<MethodSig, TypeError> {
        let error = self.declared_error(method.raises, method.raises_type.as_ref())?;
        let variadic_idx = method
            .params
            .iter()
            .position(|p| p.kind == crate::ast::ParamKind::Variadic);
        let kw_variadic_idx = method
            .params
            .iter()
            .position(|p| p.kind == crate::ast::ParamKind::KwVariadic);
        let regular: Vec<_> = method
            .params
            .iter()
            .enumerate()
            .filter(|(_, p)| p.kind == crate::ast::ParamKind::Regular)
            .collect();
        let keyword_only =
            effective_keyword_only_index(&method.params, method.keyword_only, variadic_idx);
        let regular_params: Vec<&FnParam> = regular.iter().map(|(_, param)| *param).collect();
        Ok(MethodSig {
            decls,
            availability: method
                .where_clauses
                .iter()
                .map(|condition| self.compile_where_clause(condition))
                .collect::<Result<_, _>>()?,
            has_self: method.has_self,
            params: regular
                .iter()
                .map(|(index, _)| all_types[*index].clone())
                .collect(),
            names: regular.iter().map(|(_, p)| p.name.clone()).collect(),
            required: required_mask(
                &regular.iter().map(|(_, p)| *p).collect::<Vec<_>>(),
                keyword_only,
            )?,
            variadic: variadic_idx.map(|index| Box::new(all_types[index].clone())),
            variadic_index: regular_marker_index(&method.params, variadic_idx),
            kw_variadic: kw_variadic_idx.map(|index| Box::new(all_types[index].clone())),
            kw_variadic_index: kw_variadic_idx,
            positional_only: regular_marker_index(&method.params, method.positional_only),
            keyword_only,
            conventions: regular.iter().map(|(_, p)| p.convention).collect(),
            ret: match &method.ret {
                Some(SourceType::Ref { referent, .. }) => self.ty_from_anno(referent)?,
                Some(ret) => self.ty_from_anno(ret)?,
                None => Ty::None,
            },
            raises: error.as_ref().is_some_and(|ty| *ty != Ty::Never),
            error: error.map(Box::new),
            self_convention: method.self_convention,
            ref_params: lower_ref_param_sigs(&self.enclosing_type_params, &regular_params)?,
            ref_return: match &method.ret {
                Some(SourceType::Ref { origin, .. }) => Some(lower_ref_sig(
                    origin.as_ref().ok_or_else(|| {
                        TypeError::Unsupported("reference return requires an origin".to_string())
                    })?,
                    &self.enclosing_type_params,
                    &regular_params,
                )?),
                _ => None,
            },
            implicit: method
                .decorators
                .iter()
                .any(|decorator| decorator.path.len() == 1 && decorator.path[0] == "implicit"),
        })
    }

    /// The name of the first advanced parameter feature used by a signature (a
    /// default value, a `*args`/`**kwargs` variadic, or an argument convention, or
    /// `None` if the signature is supported by this checking path. `/` and bare
    /// `*` markers are modeled by call matching and are not advanced anymore.
    pub(super) fn advanced_param_feature(
        params: &[crate::ast::FnParam],
        _positional_only: Option<usize>,
        _keyword_only: Option<usize>,
        flag_defaults: bool,
        flag_variadic: bool,
        flag_kw_variadic: bool,
    ) -> Option<&'static str> {
        use crate::ast::ParamKind;
        if flag_defaults && params.iter().any(|p| p.default.is_some()) {
            return Some("default argument values");
        }
        if flag_variadic && params.iter().any(|p| p.kind == ParamKind::Variadic) {
            return Some("variadic '*args' parameters");
        }
        if flag_kw_variadic && params.iter().any(|p| p.kind == ParamKind::KwVariadic) {
            return Some("variadic '**kwargs' parameters");
        }
        None
    }

    /// Classify a `[...]` parameter list into type and value parameters, and
    /// validate them: names must be distinct; a single bound naming a concrete
    /// type is a **value** parameter (must be `Int`); otherwise the bounds must
    /// all name traits (built-in or user), giving a **type** parameter. The
    /// parser guarantees each parameter carries at least one `: bound` (Mojo has
    /// no unconstrained parameters).
    pub(super) fn classify_params(
        &mut self,
        tps: &[crate::ast::TypeParam],
    ) -> Result<Vec<ParamDecl>, TypeError> {
        let mut decls = Vec::new();
        let mut seen = HashSet::new();
        for tp in tps {
            if !seen.insert(tp.name.clone()) {
                return Err(TypeError::Redeclaration(tp.name.clone()));
            }
            // Origin and OriginSet parameters are semantic-only and erased before
            // runtime generic argument binding. `Origin` participates in ref
            // signatures; `OriginSet` names a capturing callable's environment.
            // Both are inferred from places/callable values rather than occupying
            // a source-visible value-parameter slot. An infer-only `Bool` that
            // binds a sibling origin parameter's `mut=` erases with it.
            if matches!(tp.bounds.as_slice(), [only] if only == "Origin" || only == "OriginSet") {
                continue;
            }
            if tp.is_origin_mutability_binder(tps) {
                continue;
            }
            if let Some(value_type) = &tp.value_type {
                let ty = self.ty_from_anno(value_type)?;
                let default = tp
                    .default
                    .as_ref()
                    .map(|expr| self.compile_dependent_ct_expr(expr))
                    .transpose()?;
                decls.push(ParamDecl::Value {
                    name: tp.name.clone(),
                    ty: Box::new(ty),
                    default,
                    callable_default: None,
                    infer_only: tp.infer_only,
                    variadic: tp.name.starts_with('*'),
                    constraints: tp
                        .constraints
                        .iter()
                        .map(|condition| self.compile_where_clause(condition))
                        .collect::<Result<_, _>>()?,
                });
                continue;
            }
            // A lone bound that names a scalar type marks a value parameter.
            if let [only] = tp.bounds.as_slice()
                && let Some(vty) = scalar_type_name(only)
            {
                if !matches!(
                    vty,
                    Ty::Int | Ty::UInt | Ty::Bool | Ty::StringLiteral | Ty::Float64 | Ty::Dtype
                ) {
                    return Err(TypeError::BadValueParamType {
                        name: tp.name.clone(),
                        ty: only.clone(),
                    });
                }
                decls.push(ParamDecl::Value {
                    name: tp.name.clone(),
                    ty: Box::new(vty),
                    default: tp
                        .default
                        .as_ref()
                        .map(|expr| self.compile_dependent_ct_expr(expr))
                        .transpose()?,
                    callable_default: None,
                    infer_only: tp.infer_only,
                    variadic: tp.name.starts_with('*'),
                    constraints: tp
                        .constraints
                        .iter()
                        .map(|condition| self.compile_where_clause(condition))
                        .collect::<Result<_, _>>()?,
                });
                continue;
            }
            // A lone bound naming a registered struct is a struct-typed
            // value parameter (`[layout: Layout]`); such declarations are
            // monomorphized before checking, so the checker only ever sees
            // this classification, never a symbolic body.
            if let [only] = tp.bounds.as_slice()
                && self.structs.contains_key(only)
            {
                decls.push(ParamDecl::Value {
                    name: tp.name.clone(),
                    ty: Box::new(Ty::Struct(only.clone(), Vec::new())),
                    default: tp
                        .default
                        .as_ref()
                        .map(|expr| self.compile_dependent_ct_expr(expr))
                        .transpose()?,
                    callable_default: None,
                    infer_only: tp.infer_only,
                    variadic: tp.name.starts_with('*'),
                    constraints: tp
                        .constraints
                        .iter()
                        .map(|condition| self.compile_where_clause(condition))
                        .collect::<Result<_, _>>()?,
                });
                continue;
            }
            let trait_bounds = tp
                .bounds
                .iter()
                .filter(|bound| bound.as_str() != "<function type>")
                .cloned()
                .collect::<Vec<_>>();
            for bound in &trait_bounds {
                self.check_trait_name(bound)?;
            }
            decls.push(ParamDecl::Type {
                name: tp.name.clone(),
                bounds: trait_bounds,
                callable_bound: None,
                // A callable RHS is initially represented by this temporary
                // type declaration, but its default is a function value rather
                // than a type. Compile it only after the callable contract has
                // been lowered below.
                default: if tp.callable_bound.is_some() {
                    None
                } else {
                    tp.default
                        .as_ref()
                        .map(|value| self.type_default_from_expr(value))
                        .transpose()?
                        .map(Box::new)
                },
                infer_only: tp.infer_only,
                variadic: tp.name.starts_with('*'),
                constraints: tp
                    .constraints
                    .iter()
                    .map(|condition| self.compile_where_clause(condition))
                    .collect::<Result<_, _>>()?,
            });
        }

        // Callable constraints may depend on any type parameter in this list
        // (`F: def(T) -> T`), so lower them only after the complete preliminary
        // parameter scope exists. An explicit `thin`/`capturing[...]` spelling is
        // instead a compile-time callable-value parameter in current Mojo.
        self.tparams.push(type_scope(&decls));
        let result = (|| {
            for source in tps {
                let Some(callable) = &source.callable_bound else {
                    continue;
                };
                let SourceType::Func {
                    thin, capturing, ..
                } = callable
                else {
                    return Err(TypeError::InvariantViolation(
                        "retained callable parameter bound is not a function type".to_string(),
                    ));
                };
                let checked = self.lower_anonymous_callable_type(callable, tps)?;
                let Some(index) = decls.iter().position(|decl| decl.name() == source.name) else {
                    return Err(TypeError::InvariantViolation(format!(
                        "callable constraint parameter '{}' was not classified",
                        source.name
                    )));
                };
                let ParamDecl::Type {
                    constraints,
                    infer_only,
                    variadic,
                    ..
                } = &decls[index]
                else {
                    return Err(TypeError::InvariantViolation(
                        "callable constraint was classified as a value parameter".to_string(),
                    ));
                };
                let constraints = constraints.clone();
                let infer_only = *infer_only;
                let variadic = *variadic;
                if *thin || capturing.is_some() {
                    let callable_default = source
                        .default
                        .as_ref()
                        .map(|default| {
                            self.compile_callable_default(default, &checked, &decls[..index])
                        })
                        .transpose()?;
                    decls[index] = ParamDecl::Value {
                        name: source.name.clone(),
                        ty: Box::new(checked),
                        default: None,
                        callable_default,
                        infer_only,
                        variadic,
                        constraints,
                    };
                } else {
                    let ParamDecl::Type { callable_bound, .. } = &mut decls[index] else {
                        unreachable!("callable type parameter changed classification")
                    };
                    *callable_bound = Some(Box::new(checked));
                }
            }
            Ok(decls)
        })();
        self.tparams.pop();
        result
    }

    /// Lower a callable contract with its own `def[...]` binders. The
    /// anonymous scope is nested inside the surrounding declaration's scope,
    /// so alpha-renamed binders retain their own identity while a signature may
    /// still depend on an outer type. Origin declarations stay in the source
    /// context used by reference/capture lowering, but are erased from the
    /// ordinary `GenericFunc::decls` just like named defs.
    pub(super) fn lower_anonymous_callable_type(
        &mut self,
        callable: &SourceType,
        outer_type_params: &[crate::ast::TypeParam],
    ) -> Result<Ty, TypeError> {
        let SourceType::Func { type_params, .. } = callable else {
            return Err(TypeError::InvariantViolation(
                "anonymous callable lowering received a non-function type".to_string(),
            ));
        };
        let decls = self.classify_params(type_params)?;
        self.tparams.push(type_scope(&decls));

        let mut contextual_callable = callable.clone();
        let SourceType::Func {
            type_params: callable_context,
            ..
        } = &mut contextual_callable
        else {
            unreachable!("callable source was matched above")
        };
        // Own declarations remain first so any own Origin/OriginSet indexes
        // have the same source-relative positions they do on a named generic
        // def. The appended outer declarations are only a lookup context.
        callable_context.extend_from_slice(outer_type_params);
        let checked = self.ty_from_anno(&contextual_callable);
        self.tparams.pop();
        let checked = checked?;

        if decls.is_empty() {
            return Ok(checked);
        }
        let Ty::Func {
            environment,
            params,
            names,
            ret,
            required,
            variadic,
            kw_variadic,
            positional_only,
            keyword_only,
            raises,
            error,
            conventions,
            ref_params,
            ref_return,
            transfers,
        } = checked
        else {
            return Err(TypeError::InvariantViolation(
                "anonymous callable signature did not lower to a function type".to_string(),
            ));
        };
        Ok(Ty::GenericFunc {
            environment,
            decls,
            params,
            names,
            ret,
            required,
            variadic,
            kw_variadic,
            positional_only,
            keyword_only,
            raises,
            error,
            conventions,
            ref_params,
            ref_return,
            transfers,
        })
    }

    pub(super) fn type_default_from_expr(&self, value: &Expr) -> Result<Ty, TypeError> {
        match &value.kind {
            ExprKind::Identifier(name) => {
                if let Some(ty) = scalar_type_name(name) {
                    Ok(ty)
                } else {
                    self.ty_from_anno(&SourceType::Named(name.clone(), Vec::new()))
                }
            }
            ExprKind::TypeApply { name, args } => {
                self.ty_from_anno(&SourceType::Named(name.clone(), args.clone()))
            }
            ExprKind::TypeValue(ty) => self.ty_from_anno(ty),
            _ => Err(TypeError::TypeMismatch {
                expected: "a type".to_string(),
                found: "a value".to_string(),
                context: "type parameter default".to_string(),
            }),
        }
    }

    pub(super) fn compile_callable_default(
        &self,
        expression: &Expr,
        expected: &Ty,
        earlier: &[ParamDecl],
    ) -> Result<CallableDefault, TypeError> {
        if let ExprKind::IfExpr {
            cond,
            then_branch,
            else_branch,
        } = &expression.kind
        {
            let condition = self.compile_dependent_ct_expr(cond)?;
            if let ExprKind::Identifier(name) = &cond.kind {
                let is_bool_parameter = earlier.iter().any(|declaration| {
                    matches!(declaration,
                        ParamDecl::Value { name: parameter, ty, .. }
                            if parameter == name && ty.as_ref() == &Ty::Bool)
                });
                if !is_bool_parameter && !self.comptimes.contains_key(name) {
                    return Err(TypeError::TypeMismatch {
                        expected: Ty::Bool.to_string(),
                        found: format!("compile-time parameter '{name}'"),
                        context: "callable default condition".to_string(),
                    });
                }
            }
            return Ok(CallableDefault::If {
                condition,
                then_value: Box::new(self.compile_callable_default(
                    then_branch,
                    expected,
                    earlier,
                )?),
                else_value: Box::new(self.compile_callable_default(
                    else_branch,
                    expected,
                    earlier,
                )?),
            });
        }

        if let ExprKind::Identifier(name) = &expression.kind
            && let Some(ParamDecl::Value { ty, .. }) = earlier
                .iter()
                .find(|declaration| declaration.name() == name)
        {
            if !self.value_coerces(ty, expected) {
                return Err(TypeError::TypeMismatch {
                    expected: expected.to_string(),
                    found: ty.to_string(),
                    context: format!("default for callable parameter '{name}'"),
                });
            }
            return Ok(CallableDefault::Parameter(name.clone()));
        }

        let (name, arguments) = match &expression.kind {
            ExprKind::Identifier(name) => (name.as_str(), &[][..]),
            ExprKind::TypeApply { name, args } => (name.as_str(), args.as_slice()),
            _ => {
                return Err(TypeError::Unsupported(
                    "a callable default must be a function, an earlier callable parameter, or a conditional of those values"
                        .to_string(),
                ));
            }
        };
        let actual = self
            .infer_specialized_callable_value(
                expression.source_span(),
                name,
                arguments,
                Some(expected),
                true,
            )?
            .ok_or_else(|| TypeError::NotCallable {
                name: name.to_string(),
                ty: self
                    .lookup(name)
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "undefined".to_string()),
            })?;
        if !self.value_coerces(&actual, expected) {
            return Err(TypeError::TypeMismatch {
                expected: expected.to_string(),
                found: actual.to_string(),
                context: "callable parameter default".to_string(),
            });
        }
        let symbol = self
            .overload_targets
            .borrow()
            .get(&expression.source_span())
            .cloned()
            .unwrap_or_else(|| name.to_string());
        Ok(CallableDefault::Symbol(symbol))
    }

    pub(super) fn check_method(
        &mut self,
        self_ty: &Ty,
        m: &Method,
        module: Option<String>,
        declaration: &str,
        method_index: usize,
    ) -> Result<(), TypeError> {
        let decls = self.classify_params(&m.type_params)?;
        self.tparams.push(type_scope(&decls));
        let saved = self.enclosing_type_params.clone();
        self.enclosing_type_params.extend(m.type_params.clone());
        let assumptions = (|| {
            let mut facts = Vec::new();
            for condition in &m.where_clauses {
                let constraint = self.compile_where_clause(condition)?;
                guaranteed_conformance_atoms(&constraint, &mut facts);
            }
            Ok(facts
                .into_iter()
                .map(|(parameter, trait_name)| {
                    (parameter.trim_start_matches('*').to_string(), trait_name)
                })
                .collect::<HashSet<_>>())
        })();
        let result = match assumptions {
            Ok(assumptions) => {
                self.assumed_conformances.push(assumptions);
                let result = (|| {
                    for param in 0..m.params.len() {
                        let site = crate::checked::AnnotationSite::MethodParam {
                            module: module.clone(),
                            declaration: declaration.to_string(),
                            method: method_index,
                            param,
                        };
                        let ty = self
                            .declaration_types
                            .borrow()
                            .get(&site)
                            .cloned()
                            .ok_or_else(|| {
                                TypeError::InvariantViolation(format!(
                                    "method parameter {} for '{}.{}' has no checked type",
                                    param, declaration, m.name
                                ))
                            })?;
                        if self.is_deinitable(&ty) {
                            self.explicit_destroy_deletability
                                .borrow_mut()
                                .declarations
                                .insert(site);
                        }
                    }
                    self.check_method_inner(self_ty, m)
                })();
                self.assumed_conformances.pop();
                result
            }
            Err(error) => Err(error),
        };
        self.enclosing_type_params = saved;
        self.tparams.pop();
        result
    }

    pub(super) fn check_method_inner(&mut self, self_ty: &Ty, m: &Method) -> Result<(), TypeError> {
        let is_implicit = m
            .decorators
            .iter()
            .any(|decorator| decorator.path.len() == 1 && decorator.path[0] == "implicit");
        if is_implicit
            && (m.name != "__init__"
                || !m.has_self
                || m.self_convention != Some(ArgConvention::Out)
                || m.params.len() != 1
                || m.params[0].kind != crate::ast::ParamKind::Regular
                || m.params[0].default.is_some()
                || m.params[0].convention.is_some()
                || m.ret.is_some()
                || m.raises)
        {
            return Err(TypeError::Unsupported(
                "@implicit requires a non-raising single-argument '__init__(out self, value: T)'"
                    .to_string(),
            ));
        }
        self.validate_origin_signature(
            &self.enclosing_type_params,
            &m.params,
            m.self_origin.as_ref(),
        )?;
        if is_legacy_bare_move_constructor(m) {
            return Err(TypeError::Unsupported(
                "the move initializer requires the consuming 'deinit' convention: write \
                 '__init__(out self, *, deinit move: Self)'; a bare 'move:' parameter is \
                 not accepted"
                    .to_string(),
            ));
        }
        if !is_mojo_copy_constructor(m)
            && !is_mojo_move_constructor(m)
            && let Some(feature) = Self::advanced_param_feature(
                &m.params,
                m.positional_only,
                m.keyword_only,
                false,
                false,
                false,
            )
        {
            return Err(TypeError::Unsupported(feature.to_string()));
        }
        // `out self` initializes the receiver: it is allowed on the **`__init__`**
        // lifecycle method (a hand-written constructor), where `self`'s fields are
        // assigned in the body. `ref self` (parametric-mutability references), and
        // `out self` on any other method, still need semantics we don't model, so
        // they stay flagged. A plain `self`, `read self`, `mut self`, or `var
        // self` consuming method is fine.
        // `out self` initializes the receiver — allowed on the lifecycle methods
        // `__init__` (constructor), `__copyinit__` (copy), and `__moveinit__` (move),
        // whose bodies assign `self`'s fields. `ref self`, and `out self` elsewhere,
        // stay flagged.
        let is_lifecycle_init = matches!(
            m.name.as_str(),
            "__init__" | "__copyinit__" | "__moveinit__"
        );
        let out_init =
            matches!(m.self_convention, Some(crate::ast::ArgConvention::Out)) && is_lifecycle_init;
        if matches!(m.self_convention, Some(crate::ast::ArgConvention::Out)) && !out_init {
            return Err(TypeError::Unsupported(
                "'out self' receiver outside a lifecycle initializer".to_string(),
            ));
        }
        let ret_ty = match &m.ret {
            Some(SourceType::Ref { referent, .. }) => self.ty_from_anno(referent)?,
            Some(t) => self.ty_from_anno(t)?,
            None => Ty::None,
        };
        let regular: Vec<&FnParam> = m
            .params
            .iter()
            .filter(|param| param.kind == crate::ast::ParamKind::Regular)
            .collect();
        let ref_return = match &m.ret {
            Some(SourceType::Ref { origin, .. }) => Some(lower_ref_sig(
                origin.as_ref().ok_or_else(|| {
                    TypeError::Unsupported("reference return requires an origin".to_string())
                })?,
                &self.enclosing_type_params,
                &regular,
            )?),
            _ => None,
        };
        for param in &m.params {
            if let Some(default) = &param.default {
                let expected = self.ty_from_anno(&param.ty)?;
                let found = self.infer(default)?;
                if !coerces(&found, &expected) {
                    return Err(TypeError::TypeMismatch {
                        expected: expected.to_string(),
                        found: found.to_string(),
                        context: format!("default value of method parameter '{}'", param.name),
                    });
                }
            }
        }
        self.push_scope();
        self.raising_context
            .push(self.declared_error(m.raises, m.raises_type.as_ref())?);
        let mut result = self.bind_and_check_method(self_ty, m, &ret_ty, ref_return);
        // Definite initialization (conservative, flow-insensitive first pass): an
        // `__init__` must assign every declared field somewhere in its body, so a
        // constructed value has no unset fields. Path-sensitive DI (assign exactly
        // once, before any read, on every path) is left for a later refinement.
        if result.is_ok()
            && out_init
            && let Ty::Struct(sname, _) = self_ty
        {
            result = self.check_definite_init(sname, &m.name, &m.body);
        }
        self.raising_context.pop();
        self.pop_scope();
        result
    }

    /// Verify an `out self` lifecycle method (`method`) assigns every declared field
    /// of `sname` (flow-insensitive: assigned *somewhere*). Reports the first missing
    /// field.
    pub(super) fn check_definite_init(
        &self,
        sname: &str,
        method: &str,
        body: &[Stmt],
    ) -> Result<(), TypeError> {
        let info = self.structs.get(sname).ok_or_else(|| {
            TypeError::InvariantViolation(format!("struct '{sname}' was not registered"))
        })?;
        for (field, _) in &info.fields {
            if !definitely_initializes_self_field(body, field) {
                return Err(TypeError::UninitializedField {
                    struct_name: sname.to_string(),
                    method: method.to_string(),
                    field: field.clone(),
                });
            }
        }
        Ok(())
    }

    pub(super) fn bind_and_check_method(
        &mut self,
        self_ty: &Ty,
        m: &Method,
        ret_ty: &Ty,
        ref_return: Option<crate::origin::RefSig>,
    ) -> Result<(), TypeError> {
        // Compile-time callable/scalar value parameters occupy named runtime
        // slots in a method body, just as they do in a generic free function.
        // Type parameters remain type-only and are available through `tparams`.
        let method_decls = self.classify_params(&m.type_params)?;
        for declaration in &method_decls {
            if let ParamDecl::Value {
                name, ty, variadic, ..
            } = declaration
            {
                self.declare_immutable(
                    name.trim_start_matches('*'),
                    if *variadic {
                        Ty::VariadicPack(ty.clone())
                    } else {
                        (**ty).clone()
                    },
                )?;
            }
        }
        let mut reference_type_params = self.enclosing_type_params.clone();
        reference_type_params.extend(m.type_params.iter().cloned());
        let self_writable = ref_binding_is_writable(
            m.self_convention,
            m.self_origin.as_deref(),
            &reference_type_params,
        );
        if m.has_self {
            self.declare_with_mutability("self", self_ty.clone(), self_writable)?;
            if self.type_carries_loans(self_ty)
                && let Some(owner) = self.lookup_owner("self")
            {
                self.set_aggregate_origins(
                    "self",
                    vec![crate::origin::Origin::Place(crate::origin::OriginPlace {
                        root: owner,
                        path: Vec::new(),
                    })],
                );
            }
        }
        for p in &m.params {
            let mut pty = self.ty_from_anno(&p.ty)?;
            pty = match p.kind {
                // A specialized heterogeneous pack (`$pack` → RuntimePack)
                // binds as the tuple itself; an ordinary variadic collects into
                // source-inexpressible homogeneous pack storage.
                crate::ast::ParamKind::Variadic => match pty {
                    Ty::RuntimePack(elements) => Ty::Tuple(elements),
                    _ => Ty::VariadicPack(Box::new(pty)),
                },
                crate::ast::ParamKind::KwVariadic => {
                    self.kwargs_collector_ty(pty, &format!("keyword collector '{}'", p.name))?
                }
                crate::ast::ParamKind::Regular => pty,
            };
            self.declare_with_mutability(
                &p.name,
                pty.clone(),
                p.kind == crate::ast::ParamKind::KwVariadic
                    || ref_parameter_is_writable(p, &reference_type_params),
            )?;
            if matches!(p.convention, Some(crate::ast::ArgConvention::Ref)) {
                self.register_reference_parameter(
                    &p.name,
                    pty.clone(),
                    ref_parameter_is_writable(p, &reference_type_params),
                );
            }
            if !matches!(pty, Ty::Ref(_))
                && self.type_may_carry_loans(&pty)
                && let Some(owner) = self.lookup_owner(&p.name)
            {
                self.set_aggregate_origins(
                    &p.name,
                    vec![crate::origin::Origin::Place(crate::origin::OriginPlace {
                        root: owner,
                        path: Vec::new(),
                    })],
                );
            }
        }
        // `self` is writable in a `mut self` method, or an `out self` `__init__`
        // (which assigns its fields). Restored after the body.
        let saved = std::mem::replace(&mut self.self_mutable, self_writable);
        let initializing = matches!(m.self_convention, Some(crate::ast::ArgConvention::Out))
            && matches!(
                lifecycle_method_name(m),
                "__init__" | "__copyinit__" | "__moveinit__"
            );
        let saved_initializing = std::mem::replace(&mut self.self_initializing, initializing);
        let owners: Vec<_> = m
            .params
            .iter()
            .filter(|param| param.kind == crate::ast::ParamKind::Regular)
            .map(|param| {
                self.lookup_owner(&param.name)
                    .expect("bound method parameter")
            })
            .collect();
        let self_owner = self.lookup_owner("self");
        let mut allowed: HashSet<_> = owners.iter().copied().collect();
        allowed.extend(self_owner);
        // Variadic collectors are parameters too: a loan rooted at the
        // callee-owned pack moves outward with the stored value (the
        // generated Tuple constructor stores its `*args` collector into
        // `self.storage`).
        allowed.extend(
            m.params
                .iter()
                .filter(|param| param.kind != crate::ast::ParamKind::Regular)
                .filter_map(|param| self.lookup_owner(&param.name)),
        );
        self.aggregate_escape_contexts
            .push((self.scopes.len().saturating_sub(1), allowed));
        let method_key = match self_ty {
            Ty::Struct(struct_name, _) => format!("{struct_name}.{}", m.name),
            _ => m.name.clone(),
        };
        self.transfer_frames.borrow_mut().push(TransferFrame {
            callable: method_key,
            param_owners: owners.clone(),
            param_borrowed: m
                .params
                .iter()
                .filter(|param| param.kind == crate::ast::ParamKind::Regular)
                .map(|param| {
                    matches!(
                        param.convention,
                        Some(crate::ast::ArgConvention::Mut | crate::ast::ArgConvention::Ref)
                    )
                })
                .collect(),
            self_owner,
            value_callables: method_decls
                .iter()
                .filter_map(|decl| match decl {
                    ParamDecl::Value { name, ty, .. }
                        if matches!(**ty, Ty::Func { .. } | Ty::GenericFunc { .. }) =>
                    {
                        Some(name.trim_start_matches('*').to_string())
                    }
                    _ => None,
                })
                .collect(),
            effects: Vec::new(),
            call_throughs: Vec::new(),
        });
        self.raise_observation_frames
            .borrow_mut()
            .push((self.handled_raise_depth, false));
        self.return_ref_contracts.push(ref_return.map(|signature| {
            (
                signature,
                owners,
                self_owner.map(|root| crate::origin::OriginPlace {
                    root,
                    path: Vec::new(),
                }),
            )
        }));
        // A method body is a function scope for nested closures just as a
        // top-level `def` body is. In particular, an explicit capture list on a
        // method-local function may name `self`, parameters, and method locals.
        self.function_bases.push(self.scopes.len() - 1);
        let result = self.check_block(&m.body, Some(ret_ty), false);
        self.function_bases.pop();
        self.return_ref_contracts.pop();
        self.raise_observation_frames.borrow_mut().pop();
        if let Some(frame) = self.transfer_frames.borrow_mut().pop() {
            if !frame.effects.is_empty() {
                self.transfer_effects
                    .borrow_mut()
                    .insert(frame.callable.clone(), frame.effects);
            }
            if !frame.call_throughs.is_empty() {
                self.call_through_effects
                    .borrow_mut()
                    .insert(frame.callable, frame.call_throughs);
            }
        }
        self.aggregate_escape_contexts.pop();
        self.self_mutable = saved;
        self.self_initializing = saved_initializing;
        result?;
        if *ret_ty != Ty::None && !definitely_returns(&m.body) {
            return Err(TypeError::MissingReturn(m.name.clone()));
        }
        Ok(())
    }

    /// Type a struct construction `Name[param_args](args)` (the fieldwise
    /// constructor). Type parameters are supplied explicitly or inferred from the
    /// field arguments; value parameters must be supplied explicitly.
    /// Record the `ref [origin]` constructor-argument borrows for a
    /// construction call: the constructed aggregate keeps each lent place
    /// alive through the binding's loans (a view constructor's contract).
    /// Positional slots only; keyword-bound reference parameters have no
    /// current stdlib surface.
    fn record_constructor_reference_borrows(
        &self,
        span: &SourceSpan,
        ref_params: &[Option<crate::origin::RefSig>],
        slots: &[ArgSlot],
    ) {
        let mut arguments = Vec::new();
        for (parameter, signature) in ref_params.iter().enumerate() {
            let Some(signature) = signature else { continue };
            let Some(ArgSlot::Positional(index)) = slots.get(parameter) else {
                continue;
            };
            arguments.push((
                *index,
                signature.mutability == crate::origin::SigMutability::Mutable,
            ));
        }
        if !arguments.is_empty() {
            self.operation_adjustments.borrow_mut().insert(
                span.clone(),
                crate::checked::SemanticAdjustment::BorrowRefArguments { arguments },
            );
        }
    }

    pub(super) fn infer_construction(
        &self,
        span: SourceSpan,
        name: &str,
        param_args: &[crate::ast::ParamArg],
        args: &[Expr],
        kwargs: &[crate::ast::KwArg],
    ) -> Result<Ty, TypeError> {
        let info = self.structs.get(name).ok_or_else(|| {
            TypeError::InvariantViolation(format!("constructor target '{name}' is not registered"))
        })?;
        if !kwargs.is_empty() && args.is_empty() && kwargs.len() == 1 && kwargs[0].name == "copy" {
            let Some(sig) = info
                .methods
                .get("__copyinit__")
                .and_then(|sigs| sigs.iter().find(|sig| sig.params.len() == 1))
            else {
                return Err(TypeError::BadCall {
                    func: name.to_string(),
                    reason: "no matching copy constructor".to_string(),
                });
            };
            let params = sig.params.clone();
            let decls = info.decls.clone();
            let arg_ty = self.infer(&kwargs[0].value)?;
            let (subst, tyargs) = self.resolve_use_params(
                name,
                &decls,
                param_args,
                &params,
                std::slice::from_ref(&arg_ty),
            )?;
            let expected = substitute_assoc(
                &params[0],
                &AssocBindings {
                    types: subst,
                    values: solved_value_bindings(&decls, &tyargs),
                    origins: HashMap::new(),
                },
            );
            if !coerces(&arg_ty, &expected) {
                return Err(TypeError::TypeMismatch {
                    expected: expected.to_string(),
                    found: arg_ty.to_string(),
                    context: format!("argument 'copy' to '{}.__init__'", name),
                });
            }
            return Ok(self.struct_instance_type(name, tyargs));
        }
        // A hand-written `def __init__(out self, …)` is the constructor: check the
        // call arguments against its parameters (the `self` receiver is implicit).
        // Takes precedence over `@fieldwise_init`. On a **generic** struct, the type
        // parameters are solved by unifying `__init__`'s parameter types against the
        // argument types — exactly as the fieldwise path unifies field types.
        if let Some(sigs) = info.methods.get("__init__") {
            if info.decls.is_empty() {
                let mut matches = Vec::new();
                for sig in sigs {
                    if let Ok(scored) = self.score_method_call(
                        sig,
                        &sig.params,
                        sig.variadic.as_deref(),
                        sig.kw_variadic.as_deref(),
                        args,
                        kwargs,
                    ) {
                        matches.push(MethodCallResolution {
                            conversion_score: scored.rank,
                            slots: scored.slots,
                            positional_overflow: scored.positional_overflow,
                            keyword_overflow: scored.keyword_overflow,
                            variadic_element: sig.variadic.as_deref().cloned(),
                            keyword_element: sig.kw_variadic.as_deref().cloned(),
                            conventions: sig.conventions.clone(),
                            self_convention: sig.self_convention,
                            return_type: self.struct_instance_type(name, Vec::new()),
                            result_adapter: None,
                            raises: sig.raises,
                            error: sig.error.clone(),
                            mutates_receiver: false,
                            consumes_receiver: false,
                            lowered_name: (sigs.len() > 1)
                                .then(|| method_lowered_name(name, "__init__", sig)),
                            ref_params: sig.ref_params.clone(),
                            ref_return: None,
                            param_types: sig.params.clone(),
                            param_decls: sig.decls.clone(),
                        });
                    }
                }
                let selected =
                    select_method_overload("__init__", matches, None).map_err(|kind| {
                        TypeError::BadCall {
                            func: name.to_string(),
                            reason: match kind {
                                OverloadSelect::NoMatch => {
                                    "no constructor overload matches the supplied arguments"
                                }
                                OverloadSelect::Ambiguous => {
                                    "ambiguous overloaded constructor call"
                                }
                            }
                            .to_string(),
                        }
                    })?;
                if let Some(target) = &selected.lowered_name {
                    self.overload_targets
                        .borrow_mut()
                        .insert(span.clone(), target.clone());
                }
                self.record_selected_method_conversions("__init__", &selected, args, kwargs)?;
                // Constructor calls use the same reference-parameter handles as
                // ordinary calls. Record their retained caller places after
                // overload selection so MIR does not have to inspect the
                // constructor declaration (and rejected candidates cannot leak
                // facts into the selected call).
                self.solve_call_origins(
                    &selected.slots,
                    &selected.conventions,
                    &selected.ref_params,
                    None,
                    args,
                    kwargs,
                )?;
                self.record_constructor_reference_borrows(
                    &span,
                    &selected.ref_params,
                    &selected.slots,
                );
                for (index, slot) in selected.slots.iter().enumerate() {
                    let Some(Some(convention @ (ArgConvention::Var | ArgConvention::Deinit))) =
                        selected.conventions.get(index)
                    else {
                        continue;
                    };
                    let kind = if *convention == ArgConvention::Deinit {
                        super::traits::ConsumeKind::Deinit
                    } else {
                        super::traits::ConsumeKind::Move
                    };
                    let argument = match slot {
                        ArgSlot::Positional(position) => &args[*position],
                        ArgSlot::Keyword(position) => &kwargs[*position].value,
                        ArgSlot::Default => continue,
                    };
                    let ty = self.infer(argument)?;
                    self.check_consuming_as(
                        argument,
                        &ty,
                        &format!("argument {} to '{name}'", index + 1),
                        kind,
                    )?;
                }
                return Ok(self.struct_instance_type(name, Vec::new()));
            }
            if sigs.len() == 1 && kwargs.is_empty() {
                let sig = &sigs[0];
                let params = sig.params.clone();
                let decls = info.decls.clone();
                let arg_tys = args
                    .iter()
                    .map(|a| self.infer(a))
                    .collect::<Result<Vec<_>, _>>()?;
                let (subst, tyargs) =
                    self.resolve_use_params(name, &decls, param_args, &params, &arg_tys)?;
                for (i, (aty, pty)) in arg_tys.iter().zip(&params).enumerate() {
                    let expected = substitute(pty, &subst);
                    if !coerces(aty, &expected)
                        && !self.record_constructor_conversion(&args[i], aty, &expected)?
                    {
                        return Err(TypeError::TypeMismatch {
                            expected: expected.to_string(),
                            found: aty.to_string(),
                            context: format!("argument {} to '{}.__init__'", i + 1, name),
                        });
                    }
                    if let Some(Some(convention @ (ArgConvention::Var | ArgConvention::Deinit))) =
                        sig.conventions.get(i)
                    {
                        let kind = if *convention == ArgConvention::Deinit {
                            super::traits::ConsumeKind::Deinit
                        } else {
                            super::traits::ConsumeKind::Move
                        };
                        self.check_consuming_as(
                            &args[i],
                            aty,
                            &format!("argument {} to '{}'", i + 1, name),
                            kind,
                        )?;
                    }
                }
                let slots = (0..args.len()).map(ArgSlot::Positional).collect::<Vec<_>>();
                self.solve_call_origins(
                    &slots,
                    &sig.conventions,
                    &sig.ref_params,
                    None,
                    args,
                    kwargs,
                )?;
                self.record_constructor_reference_borrows(&span, &sig.ref_params, &slots);
                return Ok(self.struct_instance_type(name, tyargs));
            }
            let decls = info.decls.clone();
            let overloaded = sigs.len() > 1;
            let keyword_names: Vec<&str> = kwargs.iter().map(|k| k.name.as_str()).collect();
            // Map arguments (positional and keyword) to each candidate's
            // parameter slots with the shared structural matcher, then solve
            // the struct's generic parameters from the bound slots.
            let mut matches = Vec::new();
            for sig in sigs {
                let Ok(matched) = crate::call::match_call_slots(
                    &sig.names,
                    &sig.required,
                    sig.positional_only,
                    sig.keyword_only,
                    args.len(),
                    &keyword_names,
                    crate::call::CallVariadics {
                        positional: sig.variadic.is_some(),
                        keyword: sig.kw_variadic.is_some(),
                    },
                ) else {
                    continue;
                };
                // Variadic constructors (the compiler-driven display literal
                // path) and keyword collectors are not explicit-call surface.
                if !matched.positional_overflow.is_empty() || !matched.keyword_overflow.is_empty() {
                    continue;
                }
                let mut bound: Vec<(&Expr, Ty, Option<ArgConvention>)> = Vec::new();
                for (index, slot) in matched.slots.iter().enumerate() {
                    let expression = match slot {
                        ArgSlot::Positional(position) => &args[*position],
                        ArgSlot::Keyword(position) => &kwargs[*position].value,
                        ArgSlot::Default => continue,
                    };
                    bound.push((
                        expression,
                        sig.params[index].clone(),
                        sig.conventions.get(index).cloned().flatten(),
                    ));
                }
                let arg_tys = bound
                    .iter()
                    .map(|(expression, ..)| self.infer(expression))
                    .collect::<Result<Vec<_>, _>>()?;
                let patterns: Vec<Ty> = bound
                    .iter()
                    .map(|(_, pattern, _)| pattern.clone())
                    .collect();
                if let Ok((subst, tyargs)) =
                    self.resolve_use_params(name, &decls, param_args, &patterns, &arg_tys)
                {
                    let bindings = AssocBindings {
                        types: subst,
                        values: solved_value_bindings(&decls, &tyargs),
                        origins: HashMap::new(),
                    };
                    let mut score = 0;
                    let mut ok = true;
                    let mut conversions = Vec::new();
                    for (index, (aty, pty)) in arg_tys.iter().zip(&patterns).enumerate() {
                        let expected = substitute_assoc(pty, &bindings);
                        if coerces(aty, &expected) {
                            if *aty != expected {
                                score += 1;
                            }
                        } else if self.implicit_conversion_target(aty, &expected)?.is_some() {
                            // An implicit conversion ranks below any direct
                            // coercion so exact overloads keep winning.
                            score += 2;
                            conversions.push((index, expected));
                        } else {
                            ok = false;
                            break;
                        }
                    }
                    if ok {
                        matches.push((score, sig.clone(), tyargs, conversions, matched.slots));
                    }
                }
            }
            let best = matches.iter().map(|(score, ..)| *score).min();
            if let Some(best) = best {
                let mut best_matches = matches
                    .into_iter()
                    .filter(|(score, ..)| *score == best)
                    .collect::<Vec<_>>();
                if best_matches.len() != 1 {
                    return Err(TypeError::BadCall {
                        func: name.to_string(),
                        reason: "ambiguous overloaded constructor call".to_string(),
                    });
                }
                let (_, sig, tyargs, conversions, slots) = best_matches.remove(0);
                let bound: Vec<(&Expr, Option<ArgConvention>)> = slots
                    .iter()
                    .enumerate()
                    .filter_map(|(index, slot)| {
                        let expression = match slot {
                            ArgSlot::Positional(position) => &args[*position],
                            ArgSlot::Keyword(position) => &kwargs[*position].value,
                            ArgSlot::Default => return None,
                        };
                        Some((expression, sig.conventions.get(index).cloned().flatten()))
                    })
                    .collect();
                for (index, expected) in &conversions {
                    let (expression, _) = bound[*index];
                    let actual = self.infer(expression)?;
                    self.record_implicit_conversion(expression, &actual, expected)?;
                }
                for (i, (expression, convention)) in bound.iter().enumerate() {
                    if let Some(convention @ (ArgConvention::Var | ArgConvention::Deinit)) =
                        convention
                    {
                        let kind = if *convention == ArgConvention::Deinit {
                            super::traits::ConsumeKind::Deinit
                        } else {
                            super::traits::ConsumeKind::Move
                        };
                        let aty = self.infer(expression)?;
                        self.check_consuming_as(
                            expression,
                            &aty,
                            &format!("argument {} to '{}'", i + 1, name),
                            kind,
                        )?;
                    }
                }
                if overloaded {
                    self.overload_targets
                        .borrow_mut()
                        .insert(span.clone(), method_lowered_name(name, "__init__", &sig));
                }
                self.solve_call_origins(
                    &slots,
                    &sig.conventions,
                    &sig.ref_params,
                    None,
                    args,
                    kwargs,
                )?;
                self.record_constructor_reference_borrows(&span, &sig.ref_params, &slots);
                return Ok(self.struct_instance_type(name, tyargs));
            }
            return Err(TypeError::BadCall {
                func: name.to_string(),
                reason: "no constructor overload matches the supplied arguments".to_string(),
            });
        }
        if info.methods.contains_key("__init__") {
            return Err(TypeError::ArityMismatch {
                name: name.to_string(),
                expected: info
                    .methods
                    .get("__init__")
                    .and_then(|sigs| sigs.first())
                    .map(|sig| sig.params.len())
                    .unwrap_or(0),
                got: args.len(),
            });
        }
        if !info.fieldwise_init {
            return Err(TypeError::NoConstructor(name.to_string()));
        }
        let decls = info.decls.clone();
        let field_tys: Vec<Ty> = info.fields.iter().map(|(_, t)| t.clone()).collect();
        if field_tys.len() != args.len() {
            return Err(TypeError::ArityMismatch {
                name: name.to_string(),
                expected: field_tys.len(),
                got: args.len(),
            });
        }
        let arg_tys = args
            .iter()
            .zip(&field_tys)
            .map(|(argument, field)| {
                if self.type_contains_reference(field) {
                    self.infer_storage_value(argument, field)
                } else if matches!(field, Ty::Func { .. } | Ty::GenericFunc { .. }) {
                    // Callable fields keep uncontextualized inference: the
                    // storage rule below must still see a capturing closure's
                    // full environment rather than an adapted contract.
                    self.infer(argument)
                } else {
                    // Contextual: a display argument checks its elements
                    // against the field's element types, so a string literal
                    // converts where a nominal String element is expected.
                    // Generic (parameter-typed) fields fall through to plain
                    // inference inside `infer_with_expected`.
                    self.infer_with_expected(argument, field, true)
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        let (subst, tyargs) =
            self.resolve_use_params(name, &decls, param_args, &field_tys, &arg_tys)?;
        for (i, (aty, fty)) in arg_tys.iter().zip(&field_tys).enumerate() {
            let expected = substitute(fty, &subst);
            if !Self::storage_value_coerces(aty, &expected)
                && !self.record_constructor_conversion(&args[i], aty, &expected)?
            {
                return Err(TypeError::TypeMismatch {
                    expected: expected.to_string(),
                    found: aty.to_string(),
                    context: format!("field {} of '{}'", i + 1, name),
                });
            }
            if self.type_contains_reference(&expected) {
                self.mark_reference_storage_uses(&args[i], &expected);
            }
            if matches!(expected, Ty::Ref(_)) {
                continue;
            }
            // A constructor stores each argument in a field by value — a consuming
            // position.
            self.check_consuming(&args[i], aty, &format!("field {} of '{}'", i + 1, name))?;
        }
        Ok(self.struct_instance_type(name, tyargs))
    }

    /// Resolve a generic use site's parameters, returning a type-parameter
    /// substitution and the full argument list (types + values) for the struct's
    /// identity. When `param_args` is non-empty the parameters are supplied
    /// explicitly (positionally); otherwise the type parameters are inferred from
    /// `patterns`/`actuals` (a value parameter cannot be inferred).
    pub(super) fn resolve_use_params(
        &self,
        name: &str,
        decls: &[ParamDecl],
        param_args: &[crate::ast::ParamArg],
        patterns: &[Ty],
        actuals: &[Ty],
    ) -> Result<(HashMap<String, Ty>, Vec<TyArg>), TypeError> {
        let mut subst: HashMap<String, Ty> = HashMap::new();
        if decls.is_empty() {
            if !param_args.is_empty() {
                return Err(TypeError::WrongTypeArgCount {
                    name: name.to_string(),
                    expected: 0,
                    got: param_args.len(),
                });
            }
            return Ok((subst, Vec::new()));
        }
        if !param_args.is_empty() {
            let mut bound: Vec<Vec<&crate::ast::ParamArg>> = vec![Vec::new(); decls.len()];
            let mut positional = 0;
            let mut saw_keyword = false;
            for argument in param_args {
                match argument {
                    crate::ast::ParamArg::Named {
                        name: keyword,
                        value,
                    } => {
                        saw_keyword = true;
                        let Some(index) = decls
                            .iter()
                            .position(|decl| decl.name().trim_start_matches('*') == keyword)
                        else {
                            return Err(TypeError::Unsupported(format!(
                                "generic '{name}' has no parameter named '{keyword}'"
                            )));
                        };
                        if !bound[index].is_empty() {
                            return Err(TypeError::Redeclaration(keyword.clone()));
                        }
                        bound[index].push(value);
                    }
                    positional_argument => {
                        if saw_keyword {
                            return Err(TypeError::Unsupported(
                                "positional compile-time argument follows a keyword argument"
                                    .to_string(),
                            ));
                        }
                        while positional < decls.len()
                            && !bound[positional].is_empty()
                            && !matches!(
                                decls[positional],
                                ParamDecl::Type { variadic: true, .. }
                                    | ParamDecl::Value { variadic: true, .. }
                            )
                        {
                            positional += 1;
                        }
                        if positional >= decls.len() {
                            return Err(TypeError::WrongTypeArgCount {
                                name: name.to_string(),
                                expected: decls.len(),
                                got: param_args.len(),
                            });
                        }
                        bound[positional].push(positional_argument);
                        if !matches!(
                            decls[positional],
                            ParamDecl::Type { variadic: true, .. }
                                | ParamDecl::Value { variadic: true, .. }
                        ) {
                            positional += 1;
                        }
                    }
                }
            }
            let mut tyargs = Vec::with_capacity(decls.len());
            let mut value_environment = HashMap::new();
            for (decl, arguments) in decls.iter().zip(bound) {
                let infer_only = matches!(
                    decl,
                    ParamDecl::Type {
                        infer_only: true,
                        ..
                    } | ParamDecl::Value {
                        infer_only: true,
                        ..
                    }
                );
                if infer_only && !arguments.is_empty() {
                    return Err(TypeError::Unsupported(format!(
                        "infer-only parameter '{}' cannot be supplied explicitly",
                        decl.name().trim_start_matches('*')
                    )));
                }
                let variadic = matches!(
                    decl,
                    ParamDecl::Type { variadic: true, .. }
                        | ParamDecl::Value { variadic: true, .. }
                );
                if variadic {
                    let values = arguments
                        .into_iter()
                        .map(|argument| self.resolve_param_arg(decl, argument))
                        .map(|result| {
                            result?.ct_value().ok_or_else(|| {
                                TypeError::Unsupported(
                                    "an origin argument cannot bind a type or value parameter"
                                        .to_string(),
                                )
                            })
                        })
                        .collect::<Result<Vec<_>, TypeError>>()?;
                    let value = CtValue::Tuple(values);
                    value_environment.insert(
                        decl.name().trim_start_matches('*').to_string(),
                        value.clone(),
                    );
                    tyargs.push(TyArg::Val(value));
                    continue;
                }
                let tyarg = if let Some(argument) = arguments.first() {
                    self.resolve_param_arg(decl, argument)?
                } else if let ParamDecl::Value {
                    callable_default: Some(_),
                    name,
                    ..
                } = decl
                {
                    // The VM evaluates the symbolic default after reifying all
                    // preceding scalar/callable parameters.  Generic identity
                    // records only that this runtime value occupies the slot.
                    TyArg::Val(CtValue::Param(name.clone()))
                } else if let ParamDecl::Value {
                    default: Some(value),
                    ty,
                    ..
                } = decl
                {
                    let value = value.evaluate(&value_environment).ok_or_else(|| {
                        TypeError::NotComptime(format!("default for parameter '{}'", decl.name()))
                    })?;
                    let rendered = value.to_string();
                    TyArg::Val(
                        value
                            .materialize_as(ty)
                            .ok_or_else(|| TypeError::TypeMismatch {
                                expected: ty.to_string(),
                                found: rendered,
                                context: format!("default for parameter '{}'", decl.name()),
                            })?,
                    )
                } else if let ParamDecl::Type {
                    default: Some(ty), ..
                } = decl
                {
                    TyArg::Ty((**ty).clone())
                } else {
                    return Err(TypeError::CannotInferTypeParam {
                        name: name.to_string(),
                        param: decl.name().to_string(),
                    });
                };
                if let (ParamDecl::Type { name, .. }, TyArg::Ty(t)) = (decl, &tyarg) {
                    subst.insert(name.clone(), t.clone());
                }
                tyargs.push(tyarg);
                if let Some(TyArg::Val(value)) = tyargs.last() {
                    value_environment.insert(
                        decl.name().trim_start_matches('*').to_string(),
                        value.clone(),
                    );
                }
            }
            self.validate_callable_parameter_bounds(name, decls, &tyargs)?;
            self.validate_generic_constraints(name, decls, &tyargs)?;
            return Ok((subst, tyargs));
        }
        // Inference: type parameters (and value parameters occupying a solved
        // argument slot, `Array[T, length]` against `Array[Int, 3]`) from the
        // argument types.
        let mut value_solutions = HashMap::new();
        for (pat, act) in patterns.iter().zip(actuals) {
            solve_value_args(pat, act, &mut value_solutions);
        }
        for (pat, act) in patterns.iter().zip(actuals) {
            if let Ty::Param { name, bounds, .. } = pat
                && name.starts_with('*')
            {
                for bound in bounds {
                    if !self.conforms_to(act, bound) {
                        return Err(TypeError::TraitNotSatisfied {
                            param: name.clone(),
                            ty: act.to_string(),
                            trait_name: bound.clone(),
                            reason: self.trait_failure_reason(act, bound),
                        });
                    }
                }
                subst.entry(name.clone()).or_insert_with(|| pat.clone());
            } else {
                unify(pat, act, &mut subst)?;
            }
        }
        let inferred_packs: HashMap<String, Vec<CtValue>> = patterns
            .iter()
            .zip(actuals)
            .filter_map(|(pattern, actual)| match pattern {
                Ty::Param { name, .. } if name.starts_with('*') => {
                    Some((name.trim_start_matches('*').to_string(), actual.clone()))
                }
                _ => None,
            })
            .fold(HashMap::new(), |mut packs, (name, ty)| {
                packs
                    .entry(name)
                    .or_insert_with(Vec::new)
                    .push(CtValue::Type(Box::new(ty)));
                packs
            });
        let mut tyargs = Vec::with_capacity(decls.len());
        let mut value_environment = HashMap::new();
        for decl in decls {
            match decl {
                ParamDecl::Value {
                    name: pname,
                    default,
                    callable_default,
                    ty,
                    ..
                } => {
                    if let Some(value) = value_solutions.get(pname) {
                        value_environment
                            .insert(pname.trim_start_matches('*').to_string(), value.clone());
                        tyargs.push(TyArg::Val(value.clone()));
                    } else if let Some(value) = default {
                        let value = value.evaluate(&value_environment).ok_or_else(|| {
                            TypeError::NotComptime(format!("default for parameter '{}'", pname))
                        })?;
                        let rendered = value.to_string();
                        let value =
                            value
                                .materialize_as(ty)
                                .ok_or_else(|| TypeError::TypeMismatch {
                                    expected: ty.to_string(),
                                    found: rendered,
                                    context: format!("default for parameter '{}'", pname),
                                })?;
                        value_environment
                            .insert(pname.trim_start_matches('*').to_string(), value.clone());
                        tyargs.push(TyArg::Val(value));
                    } else if callable_default.is_some() {
                        tyargs.push(TyArg::Val(CtValue::Param(pname.clone())));
                    } else {
                        return Err(TypeError::CannotInferTypeParam {
                            name: name.to_string(),
                            param: pname.clone(),
                        });
                    }
                }
                ParamDecl::Type {
                    name: pname,
                    bounds,
                    default,
                    variadic,
                    ..
                } => {
                    if *variadic {
                        tyargs.push(TyArg::Val(CtValue::Tuple(
                            inferred_packs
                                .get(pname.trim_start_matches('*'))
                                .cloned()
                                .unwrap_or_default(),
                        )));
                        continue;
                    }
                    let solved = subst
                        .get(pname)
                        .cloned()
                        .or_else(|| default.as_ref().map(|default| (**default).clone()))
                        .ok_or_else(|| TypeError::CannotInferTypeParam {
                            name: name.to_string(),
                            param: pname.clone(),
                        })?;
                    subst.insert(pname.clone(), solved.clone());
                    for bound in bounds {
                        if !self.conforms_to(&solved, bound) {
                            return Err(TypeError::TraitNotSatisfied {
                                param: pname.clone(),
                                ty: solved.to_string(),
                                trait_name: bound.clone(),
                                reason: self.trait_failure_reason(&solved, bound),
                            });
                        }
                    }
                    tyargs.push(TyArg::Ty(solved));
                }
            }
        }
        self.validate_callable_parameter_bounds(name, decls, &tyargs)?;
        self.validate_generic_constraints(name, decls, &tyargs)?;
        Ok((subst, tyargs))
    }
}
