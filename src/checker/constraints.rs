//! Compile-time evaluation and generic-constraint compilation/evaluation
//! for the checker: `comptime`-int folding, associated-const evaluation,
//! and `where`-clause constraint lowering and checking.
//! Extracted from `checker.rs`; see `docs/symbol-map.md`.

use super::*;

impl Checker {
    /// Evaluate a compile-time `Int` expression: literals, `comptime` constants,
    /// and `+ - * // % **` / unary `-`. Rejects anything non-comptime (a value
    /// parameter, a call, a non-`Int` operation).
    pub(super) fn eval_ct(&self, expr: &Expr) -> Result<crate::literal::IntLiteral, TypeError> {
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
        params: &[crate::ast::TypeParam],
        ty: &SourceType,
    ) -> Result<CtMemberReq, TypeError> {
        if let SourceType::Named(name, args) = ty
            && name == "$trait_composition"
        {
            let mut bounds = Vec::with_capacity(args.len());
            for argument in args {
                let crate::ast::ParamArg::Type(SourceType::Named(bound, bound_args)) = argument
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
                self.check_trait_name(bound)?;
                if !bounds.contains(bound) {
                    bounds.push(bound.clone());
                }
            }
            return Ok(CtMemberReq::Type {
                bounds,
                params: params.to_vec(),
            });
        }
        if let SourceType::Named(name, args) = ty
            && args.is_empty()
            && (BUILTIN_TRAITS.contains(&name.as_str()) || self.traits.contains_key(name))
        {
            self.check_trait_name(name)?;
            return Ok(CtMemberReq::Type {
                bounds: vec![name.clone()],
                params: params.to_vec(),
            });
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
        let mut parameterized = HashMap::new();
        for member in associated {
            if out.contains_key(&member.name) || parameterized.contains_key(&member.name) {
                return Err(TypeError::Redeclaration(member.name.clone()));
            }
            if member.params.is_empty() {
                let value = self.eval_associated_ct(&member.value, &out)?;
                out.insert(member.name.clone(), value);
            } else {
                let param_base = self.enclosing_type_params.len();
                let template = self.lower_parameterized_member(&member.params, &member.value)?;
                parameterized.insert(
                    member.name.clone(),
                    ParameterizedMember {
                        params: member.params.clone(),
                        template,
                        param_base,
                    },
                );
            }
        }
        Ok((out, parameterized))
    }

    /// Lower the body of a parameterized associated type to its symbolic template.
    /// The member's own parameters are put in scope (as the enclosing struct's
    /// parameters already are), so type parameters resolve to `Ty::Param`, value
    /// parameters to `CtValue::Param`, and origin parameters to `Origin::Param`;
    /// concrete resolution substitutes an application's arguments into the result.
    fn lower_parameterized_member(
        &mut self,
        params: &[crate::ast::TypeParam],
        value: &Expr,
    ) -> Result<Ty, TypeError> {
        let source_ty = assoc_body_source_type(value)?;
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
        let template = self.ty_from_anno(&source_ty);
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
                self.ty_value_from_name(name, &[])
                    .ok_or_else(|| TypeError::NotComptime(name.clone()))
            }
            ExprKind::TypeApply { name, args } => self
                .ty_value_from_name(name, args)
                .ok_or_else(|| TypeError::NotComptime(name.clone())),
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
                    InfixOp::Div => crate::literal::FloatLiteral::from_int(&left)
                        .div(&crate::literal::FloatLiteral::from_int(&right))
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
                    CtValue::FloatLiteral(crate::literal::FloatLiteral::from_int(&integer)),
                    CtValue::FloatLiteral(float),
                ),
            (CtValue::FloatLiteral(float), CtValue::IntLiteral(integer)) => self
                .eval_associated_ct_infix(
                    op,
                    CtValue::FloatLiteral(float),
                    CtValue::FloatLiteral(crate::literal::FloatLiteral::from_int(&integer)),
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
        args: &[crate::ast::ParamArg],
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

    pub(super) fn compile_generic_constraint(
        &self,
        expr: &Expr,
    ) -> Result<GenericConstraint, TypeError> {
        let binary = |left: &Expr, right: &Expr| {
            Ok((
                self.constraint_operand(left)?,
                self.constraint_operand(right)?,
            ))
        };
        Ok(match &expr.kind {
            ExprKind::Bool(value) => GenericConstraint::Bool(*value),
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
                self.check_trait_name(trait_name)?;
                if pack {
                    GenericConstraint::ConformsPack {
                        param,
                        trait_name: trait_name.clone(),
                    }
                } else {
                    GenericConstraint::Conforms {
                        param,
                        trait_name: trait_name.clone(),
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

    pub(super) fn constraint_operand(&self, expr: &Expr) -> Result<ConstraintOperand, TypeError> {
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
            if !self.eval_generic_constraint(constraint, &environment) {
                return Err(TypeError::BadCall {
                    func: name.to_string(),
                    reason: format!("generic constraint is not satisfied: {constraint:?}"),
                });
            }
        }
        Ok(())
    }

    pub(super) fn eval_generic_constraint(
        &self,
        constraint: &GenericConstraint,
        environment: &HashMap<&str, &TyArg>,
    ) -> bool {
        use GenericConstraint::*;
        match constraint {
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
        }
    }
}

/// The type a parameterized associated type's body denotes. The body parses as
/// an ordinary compile-time expression; the type-valued forms are a bare name
/// (`Element`), a type application (`List[T]`, `Fixed[n]`), an explicit type
/// value, or a `Self.` member.
fn assoc_body_source_type(value: &Expr) -> Result<SourceType, TypeError> {
    match &value.kind {
        ExprKind::TypeValue(ty) => Ok(ty.clone()),
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
                    .map(crate::ast::ParamArg::Value)
                    .collect(),
                _ => vec![crate::ast::ParamArg::Value((**index).clone())],
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

pub(super) fn assoc_param_kind(param: &crate::ast::TypeParam) -> AssocParamKind {
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
