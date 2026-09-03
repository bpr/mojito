//! Compile-time evaluation and generic-constraint compilation/evaluation
//! for the checker: `comptime`-int folding, associated-const evaluation,
//! and `where`-clause constraint lowering and checking.
//! Extracted from `checker.rs`; see `docs/symbol-map.md`.

use super::*;

impl Checker {
    /// Evaluate a compile-time `Int` expression: literals, `comptime` constants,
    /// and `+ - * // % **` / unary `-`. Rejects anything non-comptime (a value
    /// parameter, a call, a non-`Int` operation).
    pub(super) fn eval_ct(
        &self,
        expr: &Expr,
    ) -> Result<mojito_common::literal::IntLiteral, TypeError> {
        match &expr.kind {
            ExprKind::Int(n) => Ok(n.clone()),
            ExprKind::Identifier(name) => self
                .comptimes
                .get(name)
                .cloned()
                .ok_or_else(|| TypeError::NotComptime(name.clone())),
            ExprKind::Prefix(PrefixOp::Neg, e) => Ok(self.eval_ct(e)?.neg()),
            ExprKind::Infix(op, l, r) => {
                let (a, b) = (self.eval_ct(l)?, self.eval_ct(r)?);
                match op {
                    InfixOp::Add => Ok(a.add(&b)),
                    InfixOp::Sub => Ok(a.sub(&b)),
                    InfixOp::Mul => Ok(a.mul(&b)),
                    InfixOp::FloorDiv => a.floor_div(&b).ok_or_else(|| {
                        TypeError::NotComptime("compile-time division by zero".to_string())
                    }),
                    InfixOp::Mod => a.floor_mod(&b).ok_or_else(|| {
                        TypeError::NotComptime("compile-time modulo by zero".to_string())
                    }),
                    InfixOp::Pow => a.pow(&b).ok_or_else(|| {
                        TypeError::NotComptime(
                            "invalid or resource-limited compile-time power".to_string(),
                        )
                    }),
                    InfixOp::Shl => a.shl(&b).ok_or_else(|| {
                        TypeError::NotComptime(
                            "invalid or resource-limited compile-time shift".to_string(),
                        )
                    }),
                    InfixOp::Shr => a.shr(&b).ok_or_else(|| {
                        TypeError::NotComptime(
                            "invalid or resource-limited compile-time shift".to_string(),
                        )
                    }),
                    InfixOp::BitAnd => Ok(a.bitand(&b)),
                    InfixOp::BitOr => Ok(a.bitor(&b)),
                    InfixOp::BitXor => Ok(a.bitxor(&b)),
                    _ => Err(TypeError::NotComptime(
                        "unsupported comptime operation".to_string(),
                    )),
                }
            }
            _ => Err(TypeError::NotComptime(
                "not a comptime Int expression".to_string(),
            )),
        }
    }

    /// Classify a trait comptime-member annotation. In Mojo terms,
    /// `comptime count: Int` requires an integer compile-time value, while
    /// `comptime Element: AnyType` requires a type-valued member whose type
    /// conforms to `AnyType`.
    pub(super) fn ct_member_req_from_anno(
        &self,
        params: &[mojito_ast::ast::TypeParam],
        ty: &SourceType,
    ) -> Result<CtMemberReq, TypeError> {
        if let SourceType::Named(name, args) = ty
            && name == "$trait_composition"
        {
            let mut bounds = Vec::with_capacity(args.len());
            for argument in args {
                let mojito_ast::ast::ParamArg::Type(SourceType::Named(bound, bound_args)) =
                    argument
                else {
                    return Err(TypeError::Unsupported(
                        "associated type bounds must be trait names".to_string(),
                    ));
                };
                if !bound_args.is_empty() {
                    return Err(TypeError::Unsupported(
                        "associated type bounds cannot take arguments".to_string(),
                    ));
                }
                let bound = mojito_ast::ast::canonical_trait_name(bound);
                self.check_trait_name(bound)?;
                if !bounds.iter().any(|existing| existing == bound) {
                    bounds.push(bound.to_string());
                }
            }
            return Ok(CtMemberReq::Type {
                bounds,
                params: params.to_vec(),
            });
        }
        if let SourceType::Named(name, args) = ty
            && args.is_empty()
        {
            let name = mojito_ast::ast::canonical_trait_name(name);
            if BUILTIN_TRAITS.contains(&name) || self.traits.contains_key(name) {
                self.check_trait_name(name)?;
                return Ok(CtMemberReq::Type {
                    bounds: vec![name.to_string()],
                    params: params.to_vec(),
                });
            }
        }
        if !params.is_empty() {
            return Err(TypeError::Unsupported(
                "a compile-time value member cannot take parameters; only an \
                 associated type may be parameterized"
                    .to_string(),
            ));
        }
        Ok(CtMemberReq::Value(Box::new(self.ty_from_anno(ty)?)))
    }

    pub(super) fn check_struct_associated(
        &mut self,
        associated: &[StructComptime],
    ) -> Result<StructAssociatedMembers, TypeError> {
        let mut out = HashMap::new();
        let mut constraints = HashMap::new();
        let mut parameterized = HashMap::new();
        for member in associated {
            if out.contains_key(&member.name) || parameterized.contains_key(&member.name) {
                return Err(TypeError::Redeclaration(member.name.clone()));
            }
            if let Some(annotation) = &member.ty {
                // Resolve and classify the annotation now even when the member's
                // symbolic body cannot be checked until application. This keeps
                // associated `comptime NAME: Type where ... = ...` declarations
                // from silently discarding an invalid declared type.
                self.ct_member_req_from_anno(&member.params, annotation)?;
            }
            if member.params.is_empty() {
                let value = self.eval_associated_ct(&member.value, &out)?;
                if !member.where_clauses.is_empty() {
                    let compiled = member
                        .where_clauses
                        .iter()
                        .map(|condition| self.compile_where_clause(condition))
                        .collect::<Result<Vec<_>, _>>()?;
                    constraints.insert(member.name.clone(), compiled);
                }
                out.insert(member.name.clone(), value);
            } else {
                let param_base = self.enclosing_type_params.len();
                let source_ty = assoc_body_source_type(&member.value)?;
                let template = self.lower_parameterized_member(&member.params, &source_ty)?;
                parameterized.insert(
                    member.name.clone(),
                    ParameterizedMember {
                        params: member.params.clone(),
                        template,
                        availability: member
                            .where_clauses
                            .iter()
                            .map(|condition| self.compile_where_clause(condition))
                            .collect::<Result<_, _>>()?,
                        param_base,
                    },
                );
            }
        }
        Ok((out, constraints, parameterized))
    }

    /// Lower the type-valued body of a parameterized associated type — or a
    /// generic top-level comptime alias — to its symbolic template. The
    /// declaration's own parameters are put in scope (any enclosing struct's
    /// parameters already are), so type parameters resolve to `Ty::Param`, value
    /// parameters to `CtValue::Param`, and origin parameters to `Origin::Param`;
    /// concrete resolution substitutes an application's arguments into the result.
    pub(super) fn lower_parameterized_member(
        &mut self,
        params: &[mojito_ast::ast::TypeParam],
        source_ty: &SourceType,
    ) -> Result<Ty, TypeError> {
        // Member type parameters resolve as `Ty::Param` (via the `tparams` scope,
        // like a generic def's parameters); origin parameters resolve to
        // `Origin::Param` via `enclosing_type_params`; value parameters resolve to
        // a symbolic `CtValue::Param`.
        let scope: HashMap<String, Ty> = params
            .iter()
            .filter(|p| matches!(assoc_param_kind(p), AssocParamKind::Type))
            .map(|p| {
                (
                    p.name.clone(),
                    Ty::Param {
                        name: p.name.clone(),
                        bounds: p.bounds.clone(),
                        callable_bound: None,
                    },
                )
            })
            .collect();
        self.tparams.push(scope);
        let saved = self.enclosing_type_params.len();
        self.enclosing_type_params.extend(params.iter().cloned());
        let template = self.ty_from_anno(source_ty);
        self.enclosing_type_params.truncate(saved);
        self.tparams.pop();
        template
    }

    /// Evaluate a struct-level associated comptime value. This intentionally
    /// accepts type-valued expressions in addition to runtime-materializable
    /// constants because associated facts are type metadata, not executable code.
    pub(super) fn eval_associated_ct(
        &self,
        expr: &Expr,
        associated: &HashMap<String, CtValue>,
    ) -> Result<CtValue, TypeError> {
        match &expr.kind {
            ExprKind::Int(n) => Ok(CtValue::IntLiteral(n.clone())),
            ExprKind::Float(value) => Ok(CtValue::FloatLiteral(value.clone())),
            ExprKind::Bool(b) => Ok(CtValue::Bool(*b)),
            ExprKind::Str(s) => Ok(CtValue::Str(s.clone())),
            ExprKind::TypeValue(ty) => self.ty_from_anno(ty).map(Box::new).map(CtValue::Type),
            ExprKind::Identifier(name) => {
                if let Some(n) = self.comptimes.get(name) {
                    return Ok(CtValue::IntLiteral(n.clone()));
                }
                // The enclosing struct's parameters are in scope by bare name,
                // exactly like their `Self.<name>` spelling below.
                if let Some(value) = self.self_param_ct_value(name) {
                    return Ok(value);
                }
                self.ty_value_from_name(name, &[])
                    .ok_or_else(|| TypeError::NotComptime(name.clone()))
            }
            ExprKind::TypeApply { name, args } => self
                .ty_value_from_name(name, args)
                .ok_or_else(|| TypeError::NotComptime(name.clone())),
            // A type application whose bracket argument parses as runtime
            // indexing (`Scalar[DType.int32]` — the standing Index-vs-TypeApply
            // parse split for a single non-scalar argument).
            ExprKind::Index { object, index }
                if matches!(&object.kind, ExprKind::Identifier(_)) =>
            {
                let ExprKind::Identifier(name) = &object.kind else {
                    unreachable!("guarded above");
                };
                let args: Vec<mojito_ast::ast::ParamArg> = match &index.kind {
                    ExprKind::TupleLit(elements) => elements
                        .iter()
                        .cloned()
                        .map(mojito_ast::ast::ParamArg::Value)
                        .collect(),
                    _ => vec![mojito_ast::ast::ParamArg::Value((**index).clone())],
                };
                self.ty_value_from_name(name, &args)
                    .ok_or_else(|| TypeError::NotComptime(name.clone()))
            }
            ExprKind::Member { object, field } => {
                if let ExprKind::Identifier(s) = &object.kind
                    && s == "Self"
                {
                    if let Some(value) = self.self_param_ct_value(field) {
                        return Ok(value);
                    }
                    if let Some(value) = associated.get(field) {
                        return Ok(value.clone());
                    }
                    return Err(TypeError::UnknownSelfParam(field.clone()));
                }
                Err(TypeError::NotComptime(
                    "unsupported associated comptime member access".to_string(),
                ))
            }
            ExprKind::Prefix(PrefixOp::Neg, e) => match self.eval_associated_ct(e, associated)? {
                CtValue::Int(n) => n.checked_neg().map(CtValue::Int).ok_or_else(|| {
                    TypeError::NotComptime("compile-time integer overflow".to_string())
                }),
                CtValue::IntLiteral(n) => Ok(CtValue::IntLiteral(n.neg())),
                CtValue::FloatLiteral(value) => Ok(CtValue::FloatLiteral(value.neg())),
                _ => Err(TypeError::NotComptime(
                    "unary '-' expects a comptime numeric value".to_string(),
                )),
            },
            ExprKind::Infix(op, l, r) => self.eval_associated_ct_infix(
                *op,
                self.eval_associated_ct(l, associated)?,
                self.eval_associated_ct(r, associated)?,
            ),
            ExprKind::TupleLit(elems) => elems
                .iter()
                .map(|e| self.eval_associated_ct(e, associated))
                .collect::<Result<Vec<_>, _>>()
                .map(CtValue::Tuple),
            ExprKind::ListLit(elems) => elems
                .iter()
                .map(|e| self.eval_associated_ct(e, associated))
                .collect::<Result<Vec<_>, _>>()
                .map(CtValue::List),
            _ => Err(TypeError::NotComptime(
                "not an associated comptime expression".to_string(),
            )),
        }
    }

    pub(super) fn eval_associated_ct_infix(
        &self,
        op: InfixOp,
        left: CtValue,
        right: CtValue,
    ) -> Result<CtValue, TypeError> {
        let unsupported =
            || TypeError::NotComptime("unsupported associated comptime operation".to_string());
        match (left, right) {
            (CtValue::Int(left), CtValue::Int(right)) => {
                let value = match op {
                    InfixOp::Add => left.checked_add(right),
                    InfixOp::Sub => left.checked_sub(right),
                    InfixOp::Mul => left.checked_mul(right),
                    InfixOp::FloorDiv if right != 0 => left.checked_div_euclid(right),
                    InfixOp::Mod if right != 0 => left.checked_rem_euclid(right),
                    InfixOp::Pow if right >= 0 => u32::try_from(right)
                        .ok()
                        .and_then(|exponent| left.checked_pow(exponent)),
                    _ => return Err(unsupported()),
                };
                value
                    .map(CtValue::Int)
                    .ok_or_else(|| TypeError::NotComptime("compile-time integer overflow".into()))
            }
            (CtValue::IntLiteral(left), CtValue::IntLiteral(right)) => {
                let value = match op {
                    InfixOp::Add => Some(CtValue::IntLiteral(left.add(&right))),
                    InfixOp::Sub => Some(CtValue::IntLiteral(left.sub(&right))),
                    InfixOp::Mul => Some(CtValue::IntLiteral(left.mul(&right))),
                    InfixOp::Div => mojito_common::literal::FloatLiteral::from_int(&left)
                        .div(&mojito_common::literal::FloatLiteral::from_int(&right))
                        .map(CtValue::FloatLiteral),
                    InfixOp::FloorDiv => left.floor_div(&right).map(CtValue::IntLiteral),
                    InfixOp::Mod => left.floor_mod(&right).map(CtValue::IntLiteral),
                    InfixOp::Pow => left.pow(&right).map(CtValue::IntLiteral),
                    _ => return Err(unsupported()),
                };
                value.ok_or_else(|| {
                    TypeError::NotComptime("invalid exact compile-time arithmetic".into())
                })
            }
            (CtValue::FloatLiteral(left), CtValue::FloatLiteral(right)) => {
                let value = match op {
                    InfixOp::Add => Some(left.add(&right)),
                    InfixOp::Sub => Some(left.sub(&right)),
                    InfixOp::Mul => Some(left.mul(&right)),
                    InfixOp::Div => left.div(&right),
                    InfixOp::FloorDiv => left.floor_div(&right),
                    InfixOp::Mod => left.floor_mod(&right),
                    InfixOp::Pow => right
                        .to_int_if_whole()
                        .and_then(|exponent| left.pow_int(&exponent)),
                    _ => return Err(unsupported()),
                };
                value.map(CtValue::FloatLiteral).ok_or_else(|| {
                    TypeError::NotComptime("invalid exact compile-time arithmetic".into())
                })
            }
            (CtValue::Int(value), CtValue::IntLiteral(literal)) => self.eval_associated_ct_infix(
                op,
                CtValue::IntLiteral(value.into()),
                CtValue::IntLiteral(literal),
            ),
            (CtValue::IntLiteral(literal), CtValue::Int(value)) => self.eval_associated_ct_infix(
                op,
                CtValue::IntLiteral(literal),
                CtValue::IntLiteral(value.into()),
            ),
            (CtValue::IntLiteral(integer), CtValue::FloatLiteral(float)) => self
                .eval_associated_ct_infix(
                    op,
                    CtValue::FloatLiteral(mojito_common::literal::FloatLiteral::from_int(&integer)),
                    CtValue::FloatLiteral(float),
                ),
            (CtValue::FloatLiteral(float), CtValue::IntLiteral(integer)) => self
                .eval_associated_ct_infix(
                    op,
                    CtValue::FloatLiteral(float),
                    CtValue::FloatLiteral(mojito_common::literal::FloatLiteral::from_int(&integer)),
                ),
            _ => Err(unsupported()),
        }
    }

    pub(super) fn self_param_ct_value(&self, name: &str) -> Option<CtValue> {
        self.self_decls.iter().find_map(|decl| match decl {
            ParamDecl::Type {
                name: n,
                bounds,
                callable_bound,
                ..
            } if n == name => Some(CtValue::Type(Box::new(Ty::Param {
                name: n.clone(),
                bounds: bounds.clone(),
                callable_bound: callable_bound.clone(),
            }))),
            ParamDecl::Value { name: n, .. } if n == name => Some(CtValue::Param(n.clone())),
            _ => None,
        })
    }

    pub(super) fn ty_value_from_name(
        &self,
        name: &str,
        args: &[mojito_ast::ast::ParamArg],
    ) -> Option<CtValue> {
        if args.is_empty() {
            if let Some(ty) = scalar_type_name(name) {
                return Some(CtValue::Type(Box::new(ty)));
            }
            if name == "None" {
                return Some(CtValue::Type(Box::new(Ty::None)));
            }
        }
        self.ty_from_anno(&SourceType::Named(name.to_string(), args.to_vec()))
            .ok()
            .map(|ty| CtValue::Type(Box::new(ty)))
    }

    pub(super) fn compile_dependent_ct_expr(&self, expr: &Expr) -> Result<CtExpr, TypeError> {
        let pair = |left: &Expr, right: &Expr| {
            Ok((
                Box::new(self.compile_dependent_ct_expr(left)?),
                Box::new(self.compile_dependent_ct_expr(right)?),
            ))
        };
        Ok(match &expr.kind {
            ExprKind::Int(value) => CtExpr::Value(CtValue::IntLiteral(value.clone())),
            ExprKind::Float(value) => CtExpr::Value(CtValue::FloatLiteral(value.clone())),
            ExprKind::Bool(value) => CtExpr::Value(CtValue::Bool(*value)),
            ExprKind::Str(value) => CtExpr::Value(CtValue::Str(value.clone())),
            ExprKind::Identifier(name) => {
                if let Some(value) = self.comptimes.get(name) {
                    CtExpr::Value(CtValue::IntLiteral(value.clone()))
                } else {
                    CtExpr::Param(name.clone())
                }
            }
            // `DType.<dt>` — a dtype value parameter's default (`dtype: DType
            // = DType.int`), mirroring the comptime evaluator's spelling.
            ExprKind::Member { object, field } if matches!(&object.kind, ExprKind::Identifier(name) if name == "DType") =>
            {
                let dtype = mojito_ast::ast::Dtype::from_name(field).ok_or_else(|| {
                    TypeError::Unsupported(format!("unknown DType member '{field}'"))
                })?;
                CtExpr::Value(CtValue::Dtype(dtype))
            }
            ExprKind::TupleLit(values) => CtExpr::Value(CtValue::Tuple(
                values
                    .iter()
                    .map(|value| self.eval_associated_ct(value, &HashMap::new()))
                    .collect::<Result<_, _>>()?,
            )),
            ExprKind::ListLit(values) => CtExpr::Value(CtValue::List(
                values
                    .iter()
                    .map(|value| self.eval_associated_ct(value, &HashMap::new()))
                    .collect::<Result<_, _>>()?,
            )),
            ExprKind::Prefix(PrefixOp::Neg, value) => {
                CtExpr::Neg(Box::new(self.compile_dependent_ct_expr(value)?))
            }
            ExprKind::Infix(op, left, right) => {
                let (left, right) = pair(left, right)?;
                match op {
                    InfixOp::Add => CtExpr::Add(left, right),
                    InfixOp::Sub => CtExpr::Sub(left, right),
                    InfixOp::Mul => CtExpr::Mul(left, right),
                    InfixOp::FloorDiv => CtExpr::FloorDiv(left, right),
                    InfixOp::Mod => CtExpr::Mod(left, right),
                    InfixOp::Pow => CtExpr::Pow(left, right),
                    _ => {
                        return Err(TypeError::Unsupported(
                            "unsupported dependent parameter expression".to_string(),
                        ));
                    }
                }
            }
            _ => {
                return Err(TypeError::Unsupported(
                    "unsupported dependent parameter expression".to_string(),
                ));
            }
        })
    }

    /// Compile a declaration-level `where` clause, retaining the optional
    /// diagnostic carried by current Mojo's `(condition, "message")` form.
    /// Only the outer clause may carry a message; tuple expressions nested
    /// inside a proposition remain unsupported constraint syntax.
    pub(super) fn compile_where_clause(&self, expr: &Expr) -> Result<GenericConstraint, TypeError> {
        let ExprKind::TupleLit(elements) = &expr.kind else {
            return self.compile_generic_constraint(expr);
        };
        let [condition, message] = elements.as_slice() else {
            return Err(TypeError::Unsupported(
                "a diagnostic where clause must be `(condition, \"message\")`".to_string(),
            ));
        };
        let ExprKind::Str(message) = &message.kind else {
            return Err(TypeError::Unsupported(
                "a where-clause diagnostic message must be a string literal".to_string(),
            ));
        };
        Ok(GenericConstraint::WithMessage(
            Box::new(self.compile_generic_constraint(condition)?),
            message.clone(),
        ))
    }

    pub(super) fn compile_generic_constraint(
        &self,
        expr: &Expr,
    ) -> Result<GenericConstraint, TypeError> {
        // A predicate-alias application (`MyPred[T]`) inlines the alias's
        // compiled Bool body with the arguments substituted, so the consuming
        // proposition needs no new constraint form. (Registration forbids an
        // alias shadowing the builtin `IsTrivially*` spellings, so this check
        // never preempts them.)
        if let Some((name, args)) = self.predicate_alias_application(expr) {
            let name = name.to_string();
            return self.apply_predicate_alias(&name, &args);
        }
        // A `TypeList` proposition (`TypeList[Ts.values]().all[P]()`,
        // `TypeList.of[...]().contains[T]()`, ...): symbolic pack receivers
        // lower to pack constraint forms, concrete receivers fold eagerly.
        if let Some(constraint) = self.compile_typelist_proposition(expr)? {
            return Ok(constraint);
        }
        let binary = |left: &Expr, right: &Expr| {
            Ok((
                self.constraint_operand(left)?,
                self.constraint_operand(right)?,
            ))
        };
        Ok(match &expr.kind {
            ExprKind::Bool(value) => GenericConstraint::Bool(*value),
            ExprKind::TypeApply { name, args }
                if mojito_types::types::trivial_predicate_name(name).is_some() =>
            {
                let kind = mojito_types::types::trivial_predicate_name(name).expect("guarded");
                if args.len() != 1 {
                    return Err(TypeError::Unsupported(format!(
                        "{name}[T] takes exactly one type argument"
                    )));
                }
                GenericConstraint::Trivial(kind, self.predicate_operand(&args[0])?)
            }
            // A single non-scalar bracket argument (`IsTriviallyMovable[T]`)
            // parses as runtime indexing; recognize the predicate here too.
            ExprKind::Index { object, index }
                if matches!(
                    &object.kind,
                    ExprKind::Identifier(name)
                        if mojito_types::types::trivial_predicate_name(name).is_some()
                ) =>
            {
                let ExprKind::Identifier(name) = &object.kind else {
                    unreachable!("guarded above");
                };
                let kind = mojito_types::types::trivial_predicate_name(name).expect("guarded");
                GenericConstraint::Trivial(kind, self.constraint_operand(index)?)
            }
            ExprKind::Prefix(PrefixOp::Not, value) => {
                GenericConstraint::Not(Box::new(self.compile_generic_constraint(value)?))
            }
            ExprKind::Infix(InfixOp::And, left, right) => GenericConstraint::And(
                Box::new(self.compile_generic_constraint(left)?),
                Box::new(self.compile_generic_constraint(right)?),
            ),
            ExprKind::Infix(InfixOp::Or, left, right) => GenericConstraint::Or(
                Box::new(self.compile_generic_constraint(left)?),
                Box::new(self.compile_generic_constraint(right)?),
            ),
            ExprKind::Infix(op, left, right) => {
                let (left, right) = binary(left, right)?;
                match op {
                    InfixOp::Eq => GenericConstraint::Eq(left, right),
                    InfixOp::Ne => GenericConstraint::Ne(left, right),
                    InfixOp::Lt => GenericConstraint::Lt(left, right),
                    InfixOp::Le => GenericConstraint::Le(left, right),
                    InfixOp::Gt => GenericConstraint::Gt(left, right),
                    InfixOp::Ge => GenericConstraint::Ge(left, right),
                    _ => {
                        return Err(TypeError::Unsupported(
                            "unsupported generic where proposition".to_string(),
                        ));
                    }
                }
            }
            ExprKind::Call {
                name, args, kwargs, ..
            } if name == "conforms_to" && kwargs.is_empty() && args.len() == 2 => {
                let (param, pack) = match &args[0].kind {
                    ExprKind::Identifier(param) => (param.clone(), false),
                    ExprKind::Member { object, field } if matches!(&object.kind, ExprKind::Identifier(name) if name == "Self") => {
                        (field.clone(), false)
                    }
                    ExprKind::Member { object, field }
                        if field == "values" && matches!(&object.kind, ExprKind::Identifier(_)) =>
                    {
                        let ExprKind::Identifier(param) = &object.kind else {
                            unreachable!()
                        };
                        (param.clone(), true)
                    }
                    _ => {
                        return Err(TypeError::Unsupported(
                            "conforms_to requires a parameter name".to_string(),
                        ));
                    }
                };
                let ExprKind::Identifier(trait_name) = &args[1].kind else {
                    return Err(TypeError::Unsupported(
                        "conforms_to requires a trait name".to_string(),
                    ));
                };
                let trait_name = mojito_ast::ast::canonical_trait_name(trait_name);
                self.check_trait_name(trait_name)?;
                if pack {
                    GenericConstraint::ConformsPack {
                        param,
                        trait_name: trait_name.to_string(),
                    }
                } else {
                    GenericConstraint::Conforms {
                        param,
                        trait_name: trait_name.to_string(),
                    }
                }
            }
            _ => {
                return Err(TypeError::Unsupported(
                    "unsupported generic where proposition".to_string(),
                ));
            }
        })
    }

    /// Recognize and lower a `TypeList` Bool proposition in a constraint
    /// position, or `None` when the expression is not TypeList-shaped. The
    /// supported members are the current-vocabulary subset: `any`/`all`
    /// (per-element predicates), `all_conforms_to` (the trait form), and
    /// `contains`; `length` (with its deprecated `size` alias) is an operand,
    /// handled by `constraint_operand`.
    pub(super) fn compile_typelist_proposition(
        &self,
        expr: &Expr,
    ) -> Result<Option<GenericConstraint>, TypeError> {
        let ExprKind::Invoke {
            callee,
            param_args,
            args,
            kwargs,
        } = &expr.kind
        else {
            return Ok(None);
        };
        if !args.is_empty() || !kwargs.is_empty() {
            return Ok(None);
        }
        let ExprKind::Member { object, field } = &callee.kind else {
            return Ok(None);
        };
        let Some(receiver) = self.typelist_receiver(object)? else {
            return Ok(None);
        };
        let single = |what: &str| -> Result<&mojito_ast::ast::ParamArg, TypeError> {
            match param_args.as_slice() {
                [only] => Ok(only),
                _ => Err(TypeError::Unsupported(format!(
                    "TypeList.{what} takes exactly one compile-time argument"
                ))),
            }
        };
        Ok(Some(match field.as_str() {
            "all_conforms_to" => {
                let trait_name = match single("all_conforms_to")? {
                    mojito_ast::ast::ParamArg::Value(Expr {
                        kind: ExprKind::Identifier(name),
                        ..
                    })
                    | mojito_ast::ast::ParamArg::Type(SourceType::Named(name, _)) => {
                        mojito_ast::ast::canonical_trait_name(name)
                    }
                    _ => {
                        return Err(TypeError::Unsupported(
                            "TypeList.all_conforms_to requires a trait name".to_string(),
                        ));
                    }
                };
                self.check_trait_name(trait_name)?;
                match receiver {
                    TypeListReceiver::Pack(param) => GenericConstraint::ConformsPack {
                        param,
                        trait_name: trait_name.to_string(),
                    },
                    TypeListReceiver::Concrete(types) => GenericConstraint::Bool(
                        types.iter().all(|ty| self.conforms_to(ty, trait_name)),
                    ),
                }
            }
            member @ ("any" | "all") => {
                let all = member == "all";
                let predicate = match single(member)? {
                    mojito_ast::ast::ParamArg::Value(Expr {
                        kind: ExprKind::Identifier(name),
                        ..
                    })
                    | mojito_ast::ast::ParamArg::Type(SourceType::Named(name, _)) => {
                        if let Some(kind) = mojito_types::types::trivial_predicate_name(name) {
                            mojito_types::types::PackPredicateRef::Trivial(kind)
                        } else if matches!(self.predicate_alias(name), Some((decls, _)) if decls.len() == 1)
                        {
                            mojito_types::types::PackPredicateRef::Alias(name.clone())
                        } else {
                            return Err(TypeError::Unsupported(format!(
                                "TypeList.{member} requires an IsTrivially* predicate or a \
                                 one-parameter Bool-bodied comptime alias"
                            )));
                        }
                    }
                    _ => {
                        return Err(TypeError::Unsupported(format!(
                            "TypeList.{member} requires a predicate name"
                        )));
                    }
                };
                match receiver {
                    TypeListReceiver::Pack(param) => GenericConstraint::PackPredicate {
                        param,
                        predicate,
                        all,
                    },
                    TypeListReceiver::Concrete(types) => {
                        let holds = |ty: &Ty| self.eval_pack_predicate(&predicate, ty);
                        GenericConstraint::Bool(if all {
                            types.iter().all(holds)
                        } else {
                            types.iter().any(holds)
                        })
                    }
                }
            }
            "contains" => {
                let element = self.predicate_operand(single("contains")?)?;
                match receiver {
                    TypeListReceiver::Pack(param) => {
                        GenericConstraint::PackContains { param, element }
                    }
                    TypeListReceiver::Concrete(types) => match element {
                        ConstraintOperand::Type(needle) => {
                            GenericConstraint::Bool(types.contains(&needle))
                        }
                        _ => {
                            return Err(TypeError::Unsupported(
                                "TypeList.contains on a concrete list requires a concrete type"
                                    .to_string(),
                            ));
                        }
                    },
                }
            }
            _ => return Ok(None),
        }))
    }

    /// A `TypeList` receiver in a constraint position: the pack adapter
    /// (`TypeList[Ts.values]()`) naming a symbolic pack parameter, or the
    /// concrete constructor (`TypeList.of[Trait=..., T1, ..., Tn]()`) whose
    /// element types resolve immediately.
    fn typelist_receiver(&self, expr: &Expr) -> Result<Option<TypeListReceiver>, TypeError> {
        match &expr.kind {
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } if name == "TypeList" && args.is_empty() && kwargs.is_empty() => {
                let [
                    mojito_ast::ast::ParamArg::Value(Expr {
                        kind: ExprKind::Member { object, field },
                        ..
                    }),
                ] = param_args.as_slice()
                else {
                    return Err(TypeError::Unsupported(
                        "TypeList[...] takes a pack projection ('Ts.values')".to_string(),
                    ));
                };
                let (ExprKind::Identifier(pack), "values") = (&object.kind, field.as_str()) else {
                    return Err(TypeError::Unsupported(
                        "TypeList[...] takes a pack projection ('Ts.values')".to_string(),
                    ));
                };
                Ok(Some(TypeListReceiver::Pack(pack.clone())))
            }
            ExprKind::Invoke {
                callee,
                param_args,
                args,
                kwargs,
            } if args.is_empty() && kwargs.is_empty() => {
                let ExprKind::Member { object, field } = &callee.kind else {
                    return Ok(None);
                };
                if !matches!(&object.kind, ExprKind::Identifier(name) if name == "TypeList")
                    || field != "of"
                {
                    return Ok(None);
                }
                let mut types = Vec::new();
                for argument in param_args {
                    let annotation = match argument {
                        // The optional `Trait=` keyword names the common
                        // bound; membership is not re-checked here (the
                        // elements' own uses enforce their capabilities).
                        mojito_ast::ast::ParamArg::Named { name, .. } if name == "Trait" => {
                            continue;
                        }
                        mojito_ast::ast::ParamArg::Type(annotation) => annotation.clone(),
                        mojito_ast::ast::ParamArg::Value(Expr {
                            kind: ExprKind::Identifier(name),
                            ..
                        }) => SourceType::Named(name.clone(), Vec::new()),
                        _ => {
                            return Err(TypeError::Unsupported(
                                "TypeList.of takes type arguments".to_string(),
                            ));
                        }
                    };
                    types.push(self.ty_from_anno(&annotation).map_err(|_| {
                        TypeError::Unsupported(
                            "a TypeList.of element must be a concrete type in this position"
                                .to_string(),
                        )
                    })?);
                }
                Ok(Some(TypeListReceiver::Concrete(types)))
            }
            _ => Ok(None),
        }
    }

    /// Lower a Bool-bodied generic comptime alias (a predicate alias). The
    /// body compiles through the ordinary constraint algebra with the alias's
    /// own parameters symbolic. The declaration must stay inside the predicate
    /// subset — no packs, no defaults, no bounds beyond `AnyType` — because an
    /// application inlines only the body: a declared bound the inlined
    /// proposition would not re-check cannot be silently dropped.
    pub(super) fn compile_predicate_alias_body(
        &self,
        decls: &[ParamDecl],
        value: &Expr,
    ) -> Result<GenericConstraint, TypeError> {
        for decl in decls {
            let unsupported = |what: &str| {
                TypeError::Unsupported(format!("{what} on a Bool-bodied comptime alias parameter"))
            };
            match decl {
                ParamDecl::Type {
                    bounds,
                    callable_bound,
                    default,
                    variadic,
                    ..
                } => {
                    if *variadic {
                        return Err(unsupported("a variadic pack"));
                    }
                    if callable_bound.is_some() {
                        return Err(unsupported("a callable bound"));
                    }
                    if default.is_some() {
                        return Err(unsupported("a default"));
                    }
                    if bounds.iter().any(|bound| bound != "AnyType") {
                        return Err(TypeError::Unsupported(
                            "a Bool-bodied comptime alias parameter takes no bound beyond \
                             AnyType; spell the requirement in the body"
                                .to_string(),
                        ));
                    }
                }
                ParamDecl::Value {
                    default,
                    callable_default,
                    variadic,
                    ..
                } => {
                    if *variadic {
                        return Err(unsupported("a variadic pack"));
                    }
                    if default.is_some() || callable_default.is_some() {
                        return Err(unsupported("a default"));
                    }
                }
            }
        }
        let constraint = self
            .compile_generic_constraint(value)
            .map_err(|error| match &error {
                TypeError::Unsupported(message)
                    if message == "unsupported generic where proposition" =>
                {
                    TypeError::Unsupported(
                        "a generic comptime alias must be defined by a type or a Bool proposition"
                            .to_string(),
                    )
                }
                _ => error,
            })?;
        let declared: HashSet<&str> = decls.iter().map(|decl| decl.name()).collect();
        validate_predicate_params(&constraint, &declared)?;
        Ok(constraint)
    }

    /// Recognize a predicate-alias application expression (`MyPred[T]`,
    /// spelled as a type application or as bracket indexing, with a tuple
    /// index carrying multiple arguments), returning the alias name and its
    /// arguments. `None` when the expression is not an application of a
    /// registered predicate alias.
    pub(super) fn predicate_alias_application<'e>(
        &self,
        expr: &'e Expr,
    ) -> Option<(&'e str, Vec<mojito_ast::ast::ParamArg>)> {
        match &expr.kind {
            ExprKind::TypeApply { name, args } if self.predicate_alias(name).is_some() => {
                Some((name, args.clone()))
            }
            ExprKind::Index { object, index } => {
                let ExprKind::Identifier(name) = &object.kind else {
                    return None;
                };
                self.predicate_alias(name)?;
                let args = match &index.kind {
                    ExprKind::TupleLit(elements) => elements
                        .iter()
                        .cloned()
                        .map(mojito_ast::ast::ParamArg::Value)
                        .collect(),
                    _ => vec![mojito_ast::ast::ParamArg::Value((**index).clone())],
                };
                Some((name, args))
            }
            _ => None,
        }
    }

    /// The compiled Bool body of a registered predicate alias, or `None` when
    /// the name is unknown or names a type-bodied alias.
    pub(super) fn predicate_alias(&self, name: &str) -> Option<(&[ParamDecl], &GenericConstraint)> {
        let alias = self.comptime_aliases.get(name)?;
        match &alias.body {
            AliasBody::Predicate(constraint) => Some((&alias.decls, constraint)),
            AliasBody::Type(_) => None,
        }
    }

    /// Expand a predicate-alias application into the consuming proposition:
    /// bind each argument to the declared parameter and substitute it through
    /// the alias's compiled body. A concrete type binding folds `conforms_to`
    /// eagerly (the param-only `Conforms` form cannot carry it symbolically).
    pub(super) fn apply_predicate_alias(
        &self,
        name: &str,
        args: &[mojito_ast::ast::ParamArg],
    ) -> Result<GenericConstraint, TypeError> {
        let (decls, template) = self
            .predicate_alias(name)
            .expect("guarded by predicate_alias");
        if args.len() != decls.len() {
            return Err(TypeError::Unsupported(format!(
                "{name}[...] takes exactly {} argument(s)",
                decls.len()
            )));
        }
        let (decls, template) = (decls.to_vec(), template.clone());
        let mut bindings = HashMap::new();
        for (decl, argument) in decls.iter().zip(args) {
            bindings.insert(decl.name().to_string(), self.predicate_operand(argument)?);
        }
        self.substitute_predicate(&template, &bindings)
    }

    /// Substitute predicate-alias application bindings through the compiled
    /// body. Operand parameters rename or bake in the bound operand; a
    /// `Conforms` on a concretely bound parameter folds to its truth value.
    fn substitute_predicate(
        &self,
        constraint: &GenericConstraint,
        bindings: &HashMap<String, ConstraintOperand>,
    ) -> Result<GenericConstraint, TypeError> {
        use GenericConstraint::*;
        let operand = |operand: &ConstraintOperand| match operand {
            ConstraintOperand::Param(param) => bindings.get(param).cloned().ok_or_else(|| {
                TypeError::Unsupported(format!(
                    "a Bool-bodied comptime alias may reference only its own parameters \
                     ('{param}' is not declared)"
                ))
            }),
            other => Ok(other.clone()),
        };
        Ok(match constraint {
            WithMessage(inner, message) => WithMessage(
                Box::new(self.substitute_predicate(inner, bindings)?),
                message.clone(),
            ),
            Bool(value) => Bool(*value),
            Not(inner) => Not(Box::new(self.substitute_predicate(inner, bindings)?)),
            And(left, right) => And(
                Box::new(self.substitute_predicate(left, bindings)?),
                Box::new(self.substitute_predicate(right, bindings)?),
            ),
            Or(left, right) => Or(
                Box::new(self.substitute_predicate(left, bindings)?),
                Box::new(self.substitute_predicate(right, bindings)?),
            ),
            Conforms { param, trait_name } => {
                match operand(&ConstraintOperand::Param(param.clone()))? {
                    ConstraintOperand::Param(param) => Conforms {
                        param,
                        trait_name: trait_name.clone(),
                    },
                    ConstraintOperand::Type(ty) => Bool(self.conforms_to(&ty, trait_name)),
                    ConstraintOperand::Value(_) | ConstraintOperand::PackLength(_) => {
                        return Err(TypeError::Unsupported(format!(
                            "conforms_to in a comptime alias requires a type argument for '{param}'"
                        )));
                    }
                }
            }
            ConformsPack { .. } | PackPredicate { .. } | PackContains { .. } => {
                return Err(TypeError::Unsupported(
                    "a pack projection in a Bool-bodied comptime alias".to_string(),
                ));
            }
            Trivial(kind, inner) => Trivial(*kind, operand(inner)?),
            Eq(left, right) => Eq(operand(left)?, operand(right)?),
            Ne(left, right) => Ne(operand(left)?, operand(right)?),
            Lt(left, right) => Lt(operand(left)?, operand(right)?),
            Le(left, right) => Le(operand(left)?, operand(right)?),
            Gt(left, right) => Gt(operand(left)?, operand(right)?),
            Ge(left, right) => Ge(operand(left)?, operand(right)?),
        })
    }

    /// A single argument of a comptime predicate (`IsTrivially*` or a
    /// predicate-alias application): a bare name is a generic parameter
    /// (mirroring `conforms_to`'s param-only operand) unless it names a
    /// scalar; any other annotation resolves as a concrete type.
    fn predicate_operand(
        &self,
        argument: &mojito_ast::ast::ParamArg,
    ) -> Result<ConstraintOperand, TypeError> {
        Ok(match argument {
            mojito_ast::ast::ParamArg::Value(expr) => self.constraint_operand(expr)?,
            mojito_ast::ast::ParamArg::Type(SourceType::Named(name, args)) if args.is_empty() => {
                scalar_type_name(name)
                    .map(ConstraintOperand::Type)
                    .unwrap_or_else(|| ConstraintOperand::Param(name.clone()))
            }
            mojito_ast::ast::ParamArg::Type(ty) => ConstraintOperand::Type(self.ty_from_anno(ty)?),
            mojito_ast::ast::ParamArg::Named { .. } => {
                return Err(TypeError::Unsupported(
                    "a comptime predicate takes positional arguments".to_string(),
                ));
            }
        })
    }

    pub(super) fn constraint_operand(&self, expr: &Expr) -> Result<ConstraintOperand, TypeError> {
        // `TypeList[Ts.values]().length` is an Int operand (the removed
        // `size` alias rejects like any unknown member).
        if let ExprKind::Member { object, field } = &expr.kind
            && field == "length"
            && let Some(receiver) = self.typelist_receiver(object)?
        {
            return Ok(match receiver {
                TypeListReceiver::Pack(param) => ConstraintOperand::PackLength(param),
                TypeListReceiver::Concrete(types) => {
                    ConstraintOperand::Value(CtValue::Int(types.len() as i64))
                }
            });
        }
        Ok(match &expr.kind {
            ExprKind::Identifier(name) => scalar_type_name(name)
                .map(ConstraintOperand::Type)
                .unwrap_or_else(|| ConstraintOperand::Param(name.clone())),
            ExprKind::Member { object, field } if matches!(&object.kind, ExprKind::Identifier(name) if name == "Self") => {
                ConstraintOperand::Param(field.clone())
            }
            ExprKind::Int(value) => ConstraintOperand::Value(CtValue::IntLiteral(value.clone())),
            ExprKind::Bool(value) => ConstraintOperand::Value(CtValue::Bool(*value)),
            ExprKind::Str(value) => ConstraintOperand::Value(CtValue::Str(value.clone())),
            ExprKind::TypeValue(ty) => ConstraintOperand::Type(self.ty_from_anno(ty)?),
            ExprKind::TypeApply { name, args } => ConstraintOperand::Type(
                self.ty_from_anno(&SourceType::Named(name.clone(), args.clone()))?,
            ),
            _ => {
                return Err(TypeError::Unsupported(
                    "unsupported generic constraint operand".to_string(),
                ));
            }
        })
    }

    pub(super) fn validate_generic_constraints(
        &self,
        name: &str,
        decls: &[ParamDecl],
        arguments: &[TyArg],
    ) -> Result<(), TypeError> {
        let environment: HashMap<&str, &TyArg> = decls
            .iter()
            .zip(arguments)
            .map(|(decl, argument)| (decl.name().trim_start_matches('*'), argument))
            .collect();
        for constraint in decls.iter().flat_map(|decl| match decl {
            ParamDecl::Type { constraints, .. } | ParamDecl::Value { constraints, .. } => {
                constraints.as_slice()
            }
        }) {
            self.validate_constraint_in_environment(name, constraint, &environment)?;
        }
        Ok(())
    }

    pub(super) fn validate_constraint_in_environment(
        &self,
        name: &str,
        constraint: &GenericConstraint,
        environment: &HashMap<&str, &TyArg>,
    ) -> Result<(), TypeError> {
        if self.eval_generic_constraint(constraint, environment) {
            return Ok(());
        }
        let reason = match constraint {
            GenericConstraint::WithMessage(_, message) => {
                format!("constraint failed: {message}")
            }
            _ => format!("generic constraint is not satisfied: {constraint:?}"),
        };
        Err(TypeError::BadCall {
            func: name.to_string(),
            reason,
        })
    }

    /// Validate a constraint on a declaration with no generic argument list,
    /// such as a non-generic `comptime` constant. Parameterized declarations
    /// attach the same constraint to their final [`ParamDecl`] instead so it is
    /// evaluated against each concrete application.
    pub(super) fn validate_declaration_constraint(
        &self,
        name: &str,
        constraint: &GenericConstraint,
    ) -> Result<(), TypeError> {
        self.validate_constraint_in_environment(name, constraint, &HashMap::new())
    }

    pub(super) fn eval_generic_constraint(
        &self,
        constraint: &GenericConstraint,
        environment: &HashMap<&str, &TyArg>,
    ) -> bool {
        use GenericConstraint::*;
        match constraint {
            WithMessage(condition, _) => self.eval_generic_constraint(condition, environment),
            Bool(value) => *value,
            Not(value) => !self.eval_generic_constraint(value, environment),
            And(left, right) => {
                self.eval_generic_constraint(left, environment)
                    && self.eval_generic_constraint(right, environment)
            }
            Or(left, right) => {
                self.eval_generic_constraint(left, environment)
                    || self.eval_generic_constraint(right, environment)
            }
            Conforms { param, trait_name } => environment
                .get(param.as_str())
                .and_then(|argument| match argument {
                    TyArg::Ty(ty) => Some(self.conforms_to(ty, trait_name)),
                    TyArg::Val(_) | TyArg::Origin(_) => None,
                })
                .unwrap_or(false),
            Trivial(kind, operand) => match self.constraint_value(operand, environment) {
                Some(TyArg::Ty(ty)) => self.is_trivially(*kind, &ty),
                _ => false,
            },
            ConformsPack { param, trait_name } => environment
                .get(param.as_str())
                .and_then(|argument| {
                    match argument {
                    TyArg::Val(CtValue::Tuple(values)) => Some(values.iter().all(|value| {
                        matches!(value, CtValue::Type(ty) if self.conforms_to(ty, trait_name))
                    })),
                    _ => None,
                }
                })
                .unwrap_or(false),
            PackPredicate {
                param,
                predicate,
                all,
            } => bound_pack_types(environment, param).is_some_and(|types| {
                let mut holds = types
                    .iter()
                    .map(|ty| self.eval_pack_predicate(predicate, ty));
                if *all {
                    holds.all(|held| held)
                } else {
                    holds.any(|held| held)
                }
            }),
            PackContains { param, element } => {
                let Some(TyArg::Ty(needle)) = self.constraint_value(element, environment) else {
                    return false;
                };
                bound_pack_types(environment, param).is_some_and(|types| types.contains(&needle))
            }
            Eq(left, right) => {
                match (
                    self.constraint_value(left, environment),
                    self.constraint_value(right, environment),
                ) {
                    (Some(left), Some(right)) => ty_args_equal(&left, &right),
                    _ => false,
                }
            }
            Ne(left, right) => {
                match (
                    self.constraint_value(left, environment),
                    self.constraint_value(right, environment),
                ) {
                    (Some(left), Some(right)) => !ty_args_equal(&left, &right),
                    _ => false,
                }
            }
            Lt(left, right) | Le(left, right) | Gt(left, right) | Ge(left, right) => {
                let (Some(TyArg::Val(left)), Some(TyArg::Val(right))) = (
                    self.constraint_value(left, environment),
                    self.constraint_value(right, environment),
                ) else {
                    return false;
                };
                let op = match constraint {
                    Lt(_, _) => InfixOp::Lt,
                    Le(_, _) => InfixOp::Le,
                    Gt(_, _) => InfixOp::Gt,
                    Ge(_, _) => InfixOp::Ge,
                    _ => unreachable!(),
                };
                compare_ct_integers(op, &left, &right).unwrap_or(false)
            }
        }
    }

    pub(super) fn constraint_value<'b>(
        &self,
        operand: &'b ConstraintOperand,
        environment: &HashMap<&str, &'b TyArg>,
    ) -> Option<TyArg> {
        match operand {
            ConstraintOperand::Param(name) => {
                environment.get(name.as_str()).map(|value| (*value).clone())
            }
            ConstraintOperand::Value(value) => Some(TyArg::Val(value.clone())),
            ConstraintOperand::Type(ty) => Some(TyArg::Ty(ty.clone())),
            ConstraintOperand::PackLength(param) => bound_pack_types(environment, param)
                .map(|types| TyArg::Val(CtValue::Int(types.len() as i64))),
        }
    }

    /// Evaluate a `TypeList` per-element predicate against one concrete
    /// element type.
    fn eval_pack_predicate(
        &self,
        predicate: &mojito_types::types::PackPredicateRef,
        ty: &Ty,
    ) -> bool {
        match predicate {
            mojito_types::types::PackPredicateRef::Trivial(kind) => self.is_trivially(*kind, ty),
            mojito_types::types::PackPredicateRef::Alias(name) => {
                let Some((decls, template)) = self.predicate_alias(name) else {
                    return false;
                };
                let [only] = decls else {
                    return false;
                };
                let bindings =
                    HashMap::from([(only.name().to_string(), ConstraintOperand::Type(ty.clone()))]);
                let template = template.clone();
                self.substitute_predicate(&template, &bindings)
                    .map(|constraint| self.eval_generic_constraint(&constraint, &HashMap::new()))
                    .unwrap_or(false)
            }
        }
    }
}

/// The element types of a pack parameter bound in a constraint environment.
fn bound_pack_types(environment: &HashMap<&str, &TyArg>, param: &str) -> Option<Vec<Ty>> {
    let TyArg::Val(CtValue::Tuple(values)) = environment.get(param)? else {
        return None;
    };
    values
        .iter()
        .map(|value| match value {
            CtValue::Type(ty) => Some((**ty).clone()),
            _ => None,
        })
        .collect()
}

/// A recognized `TypeList` receiver expression in a constraint position.
enum TypeListReceiver {
    /// The pack adapter `TypeList[Ts.values]()`, naming the pack parameter.
    Pack(String),
    /// The concrete constructor `TypeList.of[...]()`, with resolved elements.
    Concrete(Vec<Ty>),
}

/// The type a parameterized associated type's body denotes. The body parses as
/// an ordinary compile-time expression; the type-valued forms are a bare name
/// (`Element`), a type application (`List[T]`, `Fixed[n]`), an explicit type
/// value, or a `Self.` member.
pub(super) fn assoc_body_source_type(value: &Expr) -> Result<SourceType, TypeError> {
    match &value.kind {
        ExprKind::TypeValue(ty) => Ok(ty.clone()),
        // `comptime IteratorType[...] = Self` — a self-iterating struct
        // (current Mojo's range family) names itself as the member's body.
        ExprKind::Identifier(name) if name == "Self" => Ok(SourceType::SelfType),
        ExprKind::Identifier(name) => Ok(SourceType::Named(name.clone(), Vec::new())),
        ExprKind::TypeApply { name, args } => Ok(SourceType::Named(name.clone(), args.clone())),
        // A type application such as `List[T]` parses as a subscript over the
        // type's name; recover the parameter-argument list from the index.
        ExprKind::Index { object, index } => {
            let ExprKind::Identifier(name) = &object.kind else {
                return Err(unsupported_assoc_body());
            };
            let args = match &index.kind {
                ExprKind::TupleLit(elements) => elements
                    .iter()
                    .cloned()
                    .map(mojito_ast::ast::ParamArg::Value)
                    .collect(),
                _ => vec![mojito_ast::ast::ParamArg::Value((**index).clone())],
            };
            Ok(SourceType::Named(name.clone(), args))
        }
        ExprKind::Member { object, field } if matches!(&object.kind, ExprKind::Identifier(s) if s == "Self") => {
            Ok(SourceType::SelfParam(field.clone()))
        }
        _ => Err(unsupported_assoc_body()),
    }
}

/// The parameter kind of one associated-type parameter, classified from its raw
/// declaration (origin parameters are erased by `classify_params`, so the arity
/// and template lowering read the raw `TypeParam` directly).
pub(super) enum AssocParamKind {
    Origin,
    Value,
    Type,
}

pub(super) fn assoc_param_kind(param: &mojito_ast::ast::TypeParam) -> AssocParamKind {
    if matches!(param.bounds.as_slice(), [only] if only == "Origin" || only == "OriginSet") {
        AssocParamKind::Origin
    } else if param.value_type.is_some()
        || matches!(param.bounds.as_slice(), [only] if scalar_type_name(only).is_some())
    {
        AssocParamKind::Value
    } else {
        AssocParamKind::Type
    }
}

fn unsupported_assoc_body() -> TypeError {
    TypeError::Unsupported("a parameterized associated type must be defined by a type".to_string())
}

/// Every parameter a predicate-alias body references must be declared, so an
/// application can never leave a dangling operand that would silently evaluate
/// false. Pack projections are rejected here because the param-only
/// `ConformsPack` form cannot carry a substituted single-type binding.
fn validate_predicate_params(
    constraint: &GenericConstraint,
    declared: &HashSet<&str>,
) -> Result<(), TypeError> {
    use GenericConstraint::*;
    let check_param = |param: &str| {
        if declared.contains(param) {
            Ok(())
        } else {
            Err(TypeError::Unsupported(format!(
                "a Bool-bodied comptime alias may reference only its own parameters \
                 ('{param}' is not declared)"
            )))
        }
    };
    let check_operand = |operand: &ConstraintOperand| match operand {
        ConstraintOperand::Param(param) => check_param(param),
        ConstraintOperand::Value(_) | ConstraintOperand::Type(_) => Ok(()),
        ConstraintOperand::PackLength(_) => Err(TypeError::Unsupported(
            "a pack projection in a Bool-bodied comptime alias".to_string(),
        )),
    };
    match constraint {
        WithMessage(inner, _) | Not(inner) => validate_predicate_params(inner, declared),
        And(left, right) | Or(left, right) => {
            validate_predicate_params(left, declared)?;
            validate_predicate_params(right, declared)
        }
        Conforms { param, .. } => check_param(param),
        ConformsPack { .. } | PackPredicate { .. } | PackContains { .. } => Err(
            TypeError::Unsupported("a pack projection in a Bool-bodied comptime alias".to_string()),
        ),
        Trivial(_, operand) => check_operand(operand),
        Eq(left, right)
        | Ne(left, right)
        | Lt(left, right)
        | Le(left, right)
        | Gt(left, right)
        | Ge(left, right) => {
            check_operand(left)?;
            check_operand(right)
        }
        Bool(_) => Ok(()),
    }
}
