//! Compile-time evaluation and generic-constraint compilation/evaluation
//! for the checker: `comptime`-int folding, associated-const evaluation,
//! and `where`-clause constraint lowering and checking.
//! Extracted from `checker.rs`; see `docs/symbol-map.md`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
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
            ExprKind::Identifier(name) => self.comptimes.get(name).cloned().ok_or_else(|| {
                // A bare struct value parameter (`SIMD[d, length]`) is
                // upstream's `use 'Self.length'` error, not a missing constant.
                if self.is_enclosing_struct_param(name) {
                    TypeError::UnqualifiedStructParam(name.clone())
                } else {
                    TypeError::NotComptime(name.clone())
                }
            }),
            ExprKind::Prefix(PrefixOp::Neg, e) => Ok(self.eval_ct(e)?.neg()),
            ExprKind::Infix(op, l, r) => {
                let (a, b) = (self.eval_ct(l)?, self.eval_ct(r)?);
                let folded = mojito_types::param_expr::fold::fold_infix(
                    *op,
                    &CtValue::IntLiteral(a),
                    &CtValue::IntLiteral(b),
                )
                .map_err(param_error)?;
                match folded {
                    CtValue::IntLiteral(value) => Ok(value),
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

    /// `conformance_conditions` are the struct's conditional conformances: a
    /// member that a conditional trait requires (`Iterable where
    /// conforms_to(T, Copyable)` → `IteratorType`) resolves under that
    /// condition's atoms as well as under its own `where` clause.
    pub(super) fn check_struct_associated(
        &mut self,
        associated: &[StructComptime],
        conformance_conditions: &[(String, Expr)],
    ) -> Result<StructAssociatedMembers, TypeError> {
        let mut out = HashMap::new();
        let mut constraints = HashMap::new();
        let mut parameterized = HashMap::new();
        let struct_name = match &self.self_ty {
            Some(Ty::Struct(name, _)) => name.clone(),
            _ => "Self".to_string(),
        };
        for member in associated {
            if out.contains_key(&member.name) || parameterized.contains_key(&member.name) {
                return Err(TypeError::Redeclaration(member.name.clone()));
            }
            let member_owner = format!("{struct_name}.{}", member.name);
            if let Some(annotation) = &member.ty {
                // Resolve and classify the annotation now even when the member's
                // symbolic body cannot be checked until application. This keeps
                // associated `comptime NAME: Type where ... = ...` declarations
                // from silently discarding an invalid declared type.
                self.ct_member_req_from_anno(&member.params, annotation)?;
            }
            // The member's own clause and the conditions of the conditional
            // traits requiring it are assumed while its body resolves; every
            // application re-checks the clause (`availability`), and a
            // conditional conformance is verified to imply the member's clause.
            let compiled = member
                .where_clauses
                .iter()
                .map(|condition| self.compile_where_clause(condition))
                .collect::<Result<Vec<_>, _>>()?;
            let mut facts = Vec::new();
            for constraint in &compiled {
                guaranteed_conformance_atoms(constraint, &mut facts);
            }
            for (trait_name, condition) in conformance_conditions {
                if self.trait_requires_comptime_member(trait_name, &member.name) {
                    let constraint = self.compile_where_clause(condition)?;
                    guaranteed_conformance_atoms(&constraint, &mut facts);
                }
            }
            self.assume_propositions(Vec::new());
            self.assumed_conformances.push(
                facts
                    .into_iter()
                    .map(|(parameter, trait_name)| {
                        (parameter.trim_start_matches('*').to_string(), trait_name)
                    })
                    .collect(),
            );
            let resolved = (|| {
                if member.params.is_empty() {
                    let value = self.eval_associated_ct(&member.value, &out)?;
                    Ok::<_, TypeError>((Some(value), None))
                } else {
                    let source_ty = assoc_body_source_type(&member.value)?;
                    let template = self.lower_parameterized_member(
                        member_type_scope(&member_owner, &member.params),
                        &member.params,
                        &source_ty,
                    )?;
                    Ok((None, Some(template)))
                }
            })();
            self.assumed_conformances.pop();
            match resolved? {
                (Some(value), _) => {
                    if !compiled.is_empty() {
                        constraints.insert(member.name.clone(), compiled);
                    }
                    out.insert(member.name.clone(), value);
                }
                (_, Some(template)) => {
                    let param_base = self.enclosing_type_params.len();
                    parameterized.insert(
                        member.name.clone(),
                        ParameterizedMember {
                            binder_owner: member_owner.clone(),
                            params: member.params.clone(),
                            template,
                            availability: compiled,
                            param_base,
                        },
                    );
                }
                (None, None) => unreachable!("an associated member is a value or a template"),
            }
        }
        Ok((out, constraints, parameterized))
    }

    /// Whether `trait_name` (or a trait it refines) declares the comptime
    /// member `member` — the member a conditional conformance to it requires.
    fn trait_requires_comptime_member(&self, trait_name: &str, member: &str) -> bool {
        let Some(info) = self.traits.get(trait_name) else {
            return false;
        };
        info.comptime_members.contains_key(member)
            || info
                .refines
                .iter()
                .any(|parent| self.trait_requires_comptime_member(parent, member))
    }

    /// Lower the type-valued body of a parameterized associated type — or a
    /// generic top-level comptime alias — to its symbolic template. The
    /// declaration's own parameters are put in scope (any enclosing struct's
    /// parameters already are), so type parameters resolve to `Ty::Param`, value
    /// parameters to `CtValue::Expr`, and origin parameters to `Origin::Param`;
    /// concrete resolution substitutes an application's arguments into the result.
    pub(super) fn lower_parameterized_member(
        &mut self,
        scope: HashMap<String, Ty>,
        params: &[mojito_ast::ast::TypeParam],
        source_ty: &SourceType,
    ) -> Result<Ty, TypeError> {
        // Member type parameters resolve as `Ty::Param` (via the `tparams` scope
        // the caller built, like a generic def's parameters); origin parameters
        // resolve to `Origin::Param` via `enclosing_type_params`; value
        // parameters resolve to a symbolic `CtValue::Expr`.
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
                // A value parameter of an enclosing declaration is its typed
                // reference: `Buf[n + 1]` stays symbolic in the template.
                if let Some(reference) = self.value_parameter_in_scope(name) {
                    return Ok(CtValue::Expr(reference));
                }
                if let Some(n) = self.comptimes.get(name) {
                    return Ok(CtValue::IntLiteral(n.clone()));
                }
                // The enclosing struct's own parameter is spelled `Self.<name>`
                // inside the body (the `Member` arm below); the bare spelling
                // is upstream's error.
                if self.is_enclosing_struct_param(name) {
                    return Err(TypeError::UnqualifiedStructParam(name.clone()));
                }
                self.ty_value_from_name(name, &[])?
                    .ok_or_else(|| TypeError::NotComptime(name.clone()))
            }
            ExprKind::TypeApply { name, args } => self
                .ty_value_from_name(name, args)?
                .ok_or_else(|| TypeError::NotComptime(name.clone())),
            // `Int(3)`: a scalar conversion of a compile-time value, which is
            // the declared type's ordinary literal materialization. It makes
            // `Array[Int, Int(3)]` and `Array[Int, 3]` one type.
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } if kwargs.is_empty()
                && param_args.is_empty()
                && args.len() == 1
                && matches!(
                    scalar_type_name(name),
                    Some(Ty::Int | Ty::UInt | Ty::Float64 | Ty::Bool)
                ) =>
            {
                let target = scalar_type_name(name).expect("guard established a scalar type");
                let value = self.eval_associated_ct(&args[0], associated)?;
                if let CtValue::Expr(expr) = &value {
                    return (*expr.meta() == mojito_types::param_expr::MetaTy::value(target))
                        .then_some(value.clone())
                        .ok_or_else(|| {
                            TypeError::NotComptime(format!(
                                "'{name}' conversion of a symbolic '{}' value",
                                expr.meta()
                            ))
                        });
                }
                let rendered = value.to_string();
                value.materialize_as(&target).ok_or_else(|| {
                    TypeError::NotComptime(format!(
                        "'{name}({rendered})' is not a compile-time {name}"
                    ))
                })
            }
            // `SIMD[DType.d, w](lanes...)`, or a vector alias's application
            // (`U256(0)`), is a compile-time vector — a hasher key: one lane
            // per element, or one element splatted across the width.
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } if kwargs.is_empty()
                && let Some((dtype, width)) = self.simd_constructor_shape(name, param_args) =>
            {
                let values = args
                    .iter()
                    .map(|argument| self.eval_associated_ct(argument, associated))
                    .collect::<Result<Vec<_>, _>>()?;
                let width = width as usize;
                let values = if values.len() == 1 && width != 1 {
                    vec![values[0].clone(); width]
                } else if values.len() == width {
                    values
                } else {
                    return Err(TypeError::NotComptime(format!(
                        "SIMD[DType.{}, {width}] construction expects one lane or {width} lanes",
                        dtype.name()
                    )));
                };
                let lanes = values
                    .iter()
                    .map(|value| mojito_types::ct::CtLane::from_value(value, dtype))
                    .collect::<Option<Vec<_>>>()
                    .ok_or_else(|| {
                        TypeError::NotComptime(format!(
                            "a SIMD[DType.{}, {width}] lane needs a compile-time scalar",
                            dtype.name()
                        ))
                    })?;
                Ok(CtValue::Simd { dtype, lanes })
            }
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
                self.ty_value_from_name(name, &args)?
                    .ok_or_else(|| TypeError::NotComptime(name.clone()))
            }
            // `DType.int`: a compile-time element-type value, as it appears
            // in a `dtype: DType` argument (`_StridedRange[DType.int]`).
            ExprKind::Member { object, field }
                if matches!(&object.kind, ExprKind::Identifier(name) if name == "DType")
                    && let Some(dtype) = Dtype::from_name(field) =>
            {
                Ok(CtValue::Dtype(dtype))
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
                    if let Some(error) = self.instance_field_without_instance(field) {
                        return Err(error);
                    }
                    return Err(TypeError::UnknownSelfParam(field.clone()));
                }
                Err(TypeError::NotComptime(
                    "unsupported associated comptime member access".to_string(),
                ))
            }
            ExprKind::Prefix(PrefixOp::Neg, e) => self
                .fold_ct_neg(&self.eval_associated_ct(e, associated)?)
                .map_err(param_error),
            ExprKind::Infix(op, l, r) => self.eval_associated_ct_infix(
                *op,
                &self.eval_associated_ct(l, associated)?,
                &self.eval_associated_ct(r, associated)?,
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
            _ => self.eval_reflection_expr(expr)?.ok_or_else(|| {
                TypeError::NotComptime("not an associated comptime expression".to_string())
            }),
        }
    }

    /// The `(dtype, width)` a call constructs when it names `SIMD[DType.d, w]`
    /// explicitly or applies a registered vector-type alias (`U256(0)`).
    fn simd_constructor_shape(
        &self,
        name: &str,
        param_args: &[mojito_ast::ast::ParamArg],
    ) -> Option<(Dtype, i64)> {
        if name == "SIMD" {
            if param_args.len() != 2 {
                return None;
            }
            return mojito_types::types::simd_shape(&self.simd_type(param_args).ok()?);
        }
        if !param_args.is_empty() {
            return None;
        }
        match self.ty_value_from_name(name, &[]).ok().flatten()? {
            CtValue::Type(ty) => match *ty {
                Ty::Simd { .. } => mojito_types::types::simd_shape(&ty),
                _ => None,
            },
            _ => None,
        }
    }

    /// `left op right` over two associated compile-time values. Concrete
    /// operands fold through the shared folder; a residual operand builds the
    /// canonical expression, so `Self.n + 1` stays symbolic in a template.
    pub(super) fn eval_associated_ct_infix(
        &self,
        op: InfixOp,
        left: &CtValue,
        right: &CtValue,
    ) -> Result<CtValue, TypeError> {
        if matches!(left, CtValue::Expr(_)) || matches!(right, CtValue::Expr(_)) {
            let context = &self.param_context;
            return context
                .constant(left.clone())
                .and_then(|left| Ok((left, context.constant(right.clone())?)))
                .and_then(|(left, right)| context.infix(op, &left, &right))
                .map(ParamExpr::into_value)
                .map_err(param_error);
        }
        mojito_types::param_expr::fold::fold_infix(op, left, right).map_err(param_error)
    }

    /// Unary `-` over an associated compile-time value; see
    /// [`Self::eval_associated_ct_infix`].
    fn fold_ct_neg(
        &self,
        value: &CtValue,
    ) -> Result<CtValue, mojito_types::param_expr::ParamError> {
        match value {
            CtValue::Expr(expr) => self.param_context.neg(expr).map(ParamExpr::into_value),
            concrete => mojito_types::param_expr::fold::fold_neg(concrete),
        }
    }

    /// The typed reference a bare name denotes when it is a value parameter
    /// of an enclosing declaration, innermost first.
    pub(super) fn value_parameter_in_scope(&self, name: &str) -> Option<ParamExpr> {
        self.vparams
            .iter()
            .take(self.tparams.len())
            .rev()
            .find_map(|scope| scope.get(name))
            .map(|reference| self.param_context.intern(reference))
    }

    /// Open the scope of a declaration's own binders: its type parameters and,
    /// level for level beside them, its value parameters. `tparams.pop()`
    /// closes both, since a value scope deeper than the type scopes is dead.
    pub(super) fn push_param_scope(&mut self, decls: &[ParamDecl]) {
        let depth = self.tparams.len();
        self.vparams.truncate(depth);
        self.vparams.resize_with(depth, HashMap::new);
        self.vparams.push(value_scope(decls));
        self.pack_params.truncate(depth);
        self.pack_params.resize_with(depth, HashMap::new);
        self.pack_params.push(pack_scope(decls));
        self.tparams.push(type_scope(decls));
    }

    /// The parameter-list reference a bare name denotes when it is a variadic
    /// type pack of an enclosing declaration, innermost first.
    pub(super) fn pack_parameter_in_scope(&self, name: &str) -> Option<ParamExpr> {
        self.pack_params
            .iter()
            .take(self.tparams.len())
            .rev()
            .find_map(|scope| scope.get(name.trim_start_matches('*')))
            .map(|reference| self.param_context.intern(reference))
    }

    /// The value `Self.<name>` denotes: the instance's bound value inside a
    /// per-instantiation clone, else the template's own symbolic binder
    /// ([`Self::self_param_ct_value`]).
    pub(super) fn self_param_value(&self, name: &str) -> Option<CtValue> {
        if let Some(Ty::Struct(_, arguments)) = &self.self_ty
            && arguments.len() >= self.self_decls.len()
            && let Some(index) = self
                .self_decls
                .iter()
                .position(|d| d.name() == name && matches!(d, ParamDecl::Value { .. }))
            && let Some(TyArg::Val(bound)) = arguments.get(index)
            && !matches!(bound, CtValue::Expr(_))
        {
            return Some(bound.clone());
        }
        self.self_param_ct_value(name)
    }

    pub(super) fn self_param_ct_value(&self, name: &str) -> Option<CtValue> {
        self.self_decls.iter().find_map(|decl| match decl {
            ParamDecl::Value { name: n, ty, .. } if n == name => Some(value_parameter(decl, ty)),
            // `Self.Ts` names the struct's pack by its unstarred spelling.
            ParamDecl::Type {
                name: n, variadic, ..
            } if n == name || (*variadic && n.trim_start_matches('*') == name) => {
                type_parameter(decl).map(|ty| CtValue::Type(Box::new(ty)))
            }
            _ => None,
        })
    }

    /// The type value a name (with bracket arguments) denotes, `None` when
    /// the name is not a type at all. Any other resolution failure — a
    /// misspelled argument, a field in a value slot — is the diagnostic.
    pub(super) fn ty_value_from_name(
        &self,
        name: &str,
        args: &[mojito_ast::ast::ParamArg],
    ) -> Result<Option<CtValue>, TypeError> {
        if args.is_empty() {
            // A type value names the nominal `String`, not the compile-time
            // literal type a `[text: String]` value parameter binds.
            if let Some(ty) = scalar_type_name(name).filter(|ty| *ty != Ty::StringLiteral) {
                return Ok(Some(CtValue::Type(Box::new(ty))));
            }
            if name == "None" {
                return Ok(Some(CtValue::Type(Box::new(Ty::None))));
            }
        }
        match self.ty_from_anno(&SourceType::Named(name.to_string(), args.to_vec())) {
            Ok(ty) => Ok(Some(CtValue::Type(Box::new(ty)))),
            Err(TypeError::UnknownType(_)) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Whether `name` is one of the enclosing struct's own parameters: its
    /// declared parameters, or — inside a value specialization's clone, whose
    /// body may still spell a baked value parameter bare — the template's.
    pub(super) fn is_enclosing_struct_param(&self, name: &str) -> bool {
        let declares = |decls: &[ParamDecl]| {
            decls
                .iter()
                .any(|decl| decl.name().trim_start_matches('*') == name)
        };
        if declares(&self.self_decls) {
            return true;
        }
        let Some(Ty::Struct(struct_name, _)) = &self.self_ty else {
            return false;
        };
        mojito_symbol::symbol::specialization_template(struct_name)
            .and_then(|template| self.structs.get(template))
            .is_some_and(|info| declares(&info.decls))
    }

    /// Upstream's rejection of `Self.<field>` in a compile-time position:
    /// the enclosing struct declares that field, and a field needs an
    /// instance.
    pub(super) fn instance_field_without_instance(&self, field: &str) -> Option<TypeError> {
        let Some(self_ty @ Ty::Struct(name, _)) = &self.self_ty else {
            return None;
        };
        self.structs
            .get(name)
            .is_some_and(|info| info.declared_field_names.iter().any(|f| f == field))
            .then(|| TypeError::InstanceFieldWithoutInstance {
                field: field.to_string(),
                ty: self_ty.to_string(),
            })
    }

    /// Compile a dependent parameter expression through the shared typed
    /// builder. Name resolution is the checker's; operator semantics and
    /// canonical form are [`ParamContext`]'s.
    pub(super) fn compile_dependent_ct_expr(&self, expr: &Expr) -> Result<ParamExpr, TypeError> {
        let context = &self.param_context;
        let constant = |value: CtValue| context.constant(value).map_err(param_error);
        let aggregate = |values: &[Expr]| {
            values
                .iter()
                .map(|value| self.eval_associated_ct(value, &HashMap::new()))
                .collect::<Result<Vec<_>, _>>()
        };
        match &expr.kind {
            ExprKind::Int(value) => constant(CtValue::IntLiteral(value.clone())),
            ExprKind::Float(value) => constant(CtValue::FloatLiteral(value.clone())),
            ExprKind::Bool(value) => constant(CtValue::Bool(*value)),
            ExprKind::Str(value) => constant(CtValue::Str(value.clone())),
            ExprKind::Identifier(name) => match self.comptimes.get(name) {
                Some(value) => constant(CtValue::IntLiteral(value.clone())),
                None => self.value_parameter_in_scope(name).ok_or_else(|| {
                    if self.is_enclosing_struct_param(name) {
                        TypeError::UnqualifiedStructParam(name.clone())
                    } else {
                        TypeError::NotComptime(name.clone())
                    }
                }),
            },
            ExprKind::Member { object, field } if matches!(&object.kind, ExprKind::Identifier(name) if name == "Self") => {
                match self.self_param_ct_value(field) {
                    Some(CtValue::Expr(expr)) => Ok(context.intern(&expr)),
                    Some(value) => constant(value),
                    None => Err(TypeError::UnknownSelfParam(field.clone())),
                }
            }
            // `DType.<dt>` — a dtype value parameter's default (`dtype: DType
            // = DType.int`), mirroring the comptime evaluator's spelling.
            ExprKind::Member { object, field } if matches!(&object.kind, ExprKind::Identifier(name) if name == "DType") =>
            {
                let dtype = mojito_ast::ast::Dtype::from_name(field).ok_or_else(|| {
                    TypeError::Unsupported(format!("unknown DType member '{field}'"))
                })?;
                constant(CtValue::Dtype(dtype))
            }
            ExprKind::TupleLit(values) => constant(CtValue::Tuple(aggregate(values)?)),
            ExprKind::ListLit(values) => constant(CtValue::List(aggregate(values)?)),
            ExprKind::Prefix(PrefixOp::Neg, value) => context
                .neg(&self.compile_dependent_ct_expr(value)?)
                .map_err(param_error),
            ExprKind::Infix(
                op @ (InfixOp::Add
                | InfixOp::Sub
                | InfixOp::Mul
                | InfixOp::FloorDiv
                | InfixOp::Mod
                | InfixOp::Pow
                | InfixOp::Shl),
                left,
                right,
            ) => context
                .infix(
                    *op,
                    &self.compile_dependent_ct_expr(left)?,
                    &self.compile_dependent_ct_expr(right)?,
                )
                .map_err(param_error),
            // `reflect[T].field_count()`: a constant for a struct subject, a
            // query node for a symbolic one.
            _ => match self.eval_reflection_expr(expr)? {
                Some(CtValue::Expr(query)) => Ok(context.intern(&query)),
                Some(value) => constant(value),
                None => Err(TypeError::Unsupported(
                    "unsupported dependent parameter expression".to_string(),
                )),
            },
        }
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
                let (param, pack) = match (pack_values_projection(&args[0]), &args[0].kind) {
                    (Some(pack), _) => (pack.to_string(), true),
                    (None, ExprKind::Identifier(param)) => (param.clone(), false),
                    (None, ExprKind::Member { object, field }) if matches!(&object.kind, ExprKind::Identifier(name) if name == "Self") => {
                        (field.clone(), false)
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

    /// A `TypeList` receiver in a constraint position: an enclosing pack
    /// parameter itself (upstream's `Ts.all_conforms_to[..]()` / `Self.Ts`),
    /// the pack adapter (`TypeList[Ts.values]()`) naming a symbolic pack
    /// parameter, or the concrete constructor
    /// (`TypeList.of[Trait=..., T1, ..., Tn]()`) whose element types resolve
    /// immediately.
    pub(super) fn typelist_receiver(
        &self,
        expr: &Expr,
    ) -> Result<Option<TypeListReceiver>, TypeError> {
        let enclosing_pack = |name: &str| {
            self.enclosing_type_params
                .iter()
                .any(|parameter| parameter.name.strip_prefix('*') == Some(name))
                .then(|| TypeListReceiver::Pack(name.to_string()))
        };
        match &expr.kind {
            ExprKind::Identifier(name) => Ok(enclosing_pack(name)),
            ExprKind::Member { object, field } if matches!(&object.kind, ExprKind::Identifier(name) if name == "Self") => {
                Ok(enclosing_pack(field))
            }
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } if name == "TypeList" && args.is_empty() && kwargs.is_empty() => {
                let [mojito_ast::ast::ParamArg::Value(projection)] = param_args.as_slice() else {
                    return Err(TypeError::Unsupported(
                        "TypeList[...] takes a pack projection ('Ts.values')".to_string(),
                    ));
                };
                let Some(pack) = pack_values_projection(projection) else {
                    return Err(TypeError::Unsupported(
                        "TypeList[...] takes a pack projection ('Ts.values')".to_string(),
                    ));
                };
                Ok(Some(TypeListReceiver::Pack(pack.to_string())))
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
        let declared: HashSet<&str> = decls
            .iter()
            .map(mojito_types::types::ParamDecl::name)
            .collect();
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
    /// the name is unknown or names a type- or value-bodied alias.
    pub(super) fn predicate_alias(&self, name: &str) -> Option<(&[ParamDecl], &GenericConstraint)> {
        let alias = self.comptime_aliases.get(name)?;
        match &alias.body {
            AliasBody::Predicate(constraint) => Some((&alias.decls, constraint)),
            AliasBody::Type(_) | AliasBody::Value => None,
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
        use GenericConstraint::{
            And, Bool, Conforms, ConformsPack, Eq, Ge, Gt, Le, Lt, Ne, Not, Or, PackContains,
            PackPredicate, Trivial, WithMessage,
        };
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
                    ConstraintOperand::Value(_)
                    | ConstraintOperand::PackLength(_)
                    | ConstraintOperand::Expr(_) => {
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
                scalar_type_name(name).map_or_else(
                    || ConstraintOperand::Param(name.clone()),
                    ConstraintOperand::Type,
                )
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
        // `TypeList[Ts.values]().length`, or `len(...)` of the same `Sized`
        // list, is an Int operand (the removed `size` alias rejects like any
        // unknown member).
        let length_receiver = match &expr.kind {
            ExprKind::Member { object, field } if field == "length" => Some(&**object),
            ExprKind::Call { name, args, .. } if name == "len" && args.len() == 1 => Some(&args[0]),
            _ => None,
        };
        // Any other member of a `TypeList` value is unknown, `size` included.
        if let ExprKind::Member { object, field } = &expr.kind
            && field != "length"
            && self.typelist_receiver(object)?.is_some()
        {
            return Err(TypeError::Unsupported(format!(
                "compile-time 'TypeList' value has no field '{field}'"
            )));
        }
        if let Some(object) = length_receiver
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
            ExprKind::Identifier(name) => scalar_type_name(name).map_or_else(
                || ConstraintOperand::Param(name.clone()),
                ConstraintOperand::Type,
            ),
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
            // A pack element (`Self.Ts[i]`) is a type operand.
            ExprKind::Index { .. } if let Some(ty) = self.comptime_type_operand(expr)? => {
                ConstraintOperand::Type(ty)
            }
            // Arithmetic over value parameters (`n + 1`), through the same
            // builder a dependent type argument uses. A name it cannot type —
            // a parameter whose scope is not open here — is the explicit
            // unsupported operand, never an untyped symbol.
            ExprKind::Infix(
                InfixOp::Add
                | InfixOp::Sub
                | InfixOp::Mul
                | InfixOp::FloorDiv
                | InfixOp::Mod
                | InfixOp::Pow
                | InfixOp::Shl,
                _,
                _,
            )
            | ExprKind::Prefix(PrefixOp::Neg, _) => match self.compile_dependent_ct_expr(expr) {
                Ok(expression) => match expression.as_constant() {
                    Some(value) => ConstraintOperand::Value(value.clone()),
                    None => ConstraintOperand::Expr(expression),
                },
                Err(TypeError::NotComptime(_)) => {
                    return Err(TypeError::Unsupported(
                        "unsupported generic constraint operand".to_string(),
                    ));
                }
                Err(error) => return Err(error),
            },
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

    /// Record what the declaration `owner` assumes inside its own body: each
    /// of its `where` clauses that is still a proposition once its own
    /// parameters stand for themselves. It is recorded beside the
    /// conformance assumptions about to be pushed, so the same pop closes it.
    pub(super) fn assume_declared_propositions(&mut self, decls: &[ParamDecl]) {
        let arguments = params_as_args(decls);
        let environment: HashMap<&str, &TyArg> = decls
            .iter()
            .zip(&arguments)
            .map(|(decl, argument)| (decl.name().trim_start_matches('*'), argument))
            .collect();
        let mut assumed = Vec::new();
        for constraint in decls.iter().flat_map(|decl| match decl {
            ParamDecl::Type { constraints, .. } | ParamDecl::Value { constraints, .. } => {
                constraints.as_slice()
            }
        }) {
            if let Ok(ConstraintVerdict::Residual(proposition)) =
                self.constraint_verdict(constraint, &environment)
            {
                assumed.extend(conjuncts(&proposition));
            }
        }
        self.assume_propositions(assumed);
    }

    /// [`Self::assume_declared_propositions`] for a method: its own `where`
    /// clauses, over the struct's parameters (`Self.n`) and its own.
    pub(super) fn assume_method_propositions(
        &mut self,
        method: &mojito_ast::ast::Method,
        method_decls: &[ParamDecl],
    ) {
        let struct_decls = self.self_decls.clone();
        let mut arguments = params_as_args(&struct_decls);
        arguments.extend(params_as_args(method_decls));
        let environment: HashMap<&str, &TyArg> = struct_decls
            .iter()
            .chain(method_decls)
            .zip(&arguments)
            .map(|(decl, argument)| (decl.name().trim_start_matches('*'), argument))
            .collect();
        let assumed = method
            .where_clauses
            .iter()
            .filter_map(|condition| self.compile_where_clause(condition).ok())
            .filter_map(
                |constraint| match self.constraint_verdict(&constraint, &environment) {
                    Ok(ConstraintVerdict::Residual(proposition)) => Some(conjuncts(&proposition)),
                    _ => None,
                },
            )
            .flatten()
            .collect();
        self.assume_propositions(assumed);
    }

    /// Open the proposition level of the conformance assumptions about to be
    /// pushed. Every push site calls this, so a closed level never lingers
    /// under a later declaration.
    pub(super) fn assume_propositions(&mut self, assumed: Vec<ParamExpr>) {
        let depth = self.assumed_conformances.len();
        self.assumed_propositions.truncate(depth);
        self.assumed_propositions.resize_with(depth, Vec::new);
        self.assumed_propositions.push(assumed);
    }

    /// Whether every conjunct of a residual proposition is one an enclosing
    /// declaration assumes. Canonical node identity is the whole proof: there
    /// is no solver behind it.
    fn assumptions_prove(&self, proposition: &ParamExpr) -> bool {
        conjuncts(proposition).iter().all(|conjunct| {
            self.assumed_propositions
                .iter()
                .take(self.assumed_conformances.len())
                .flatten()
                .any(|assumed| assumed == conjunct)
        })
    }

    /// Require `constraint` at one application. Proven passes and Disproven
    /// is the violated constraint. A residual is neither, and the pinned Mojo
    /// requires a proof even under symbolic arguments
    /// (`assets/type_error/param_expr_where_residual.mojo`): it passes only
    /// when the enclosing declaration's own `where` assumes the same
    /// proposition, and is otherwise reported as lacking evidence, never as
    /// false.
    pub(super) fn validate_constraint_in_environment(
        &self,
        name: &str,
        constraint: &GenericConstraint,
        environment: &HashMap<&str, &TyArg>,
    ) -> Result<(), TypeError> {
        let reason = match self.constraint_verdict(constraint, environment)? {
            ConstraintVerdict::Proven => return Ok(()),
            ConstraintVerdict::Residual(proposition) if self.assumptions_prove(&proposition) => {
                return Ok(());
            }
            ConstraintVerdict::Residual(_) => format!(
                "lacking evidence to prove correctness; cannot prove constraint '{constraint}'"
            ),
            ConstraintVerdict::Disproven => match constraint {
                GenericConstraint::WithMessage(_, message) => {
                    format!("constraint failed: {message}")
                }
                violated => super::generics::violated_constraint_reason(violated),
            },
        };
        Err(TypeError::BadCall {
            func: name.to_string(),
            reason,
        })
    }

    /// Validate a constraint on a declaration with no generic argument list,
    /// such as a non-generic `comptime` constant. Parameterized declarations
    /// attach the same constraint to their final [`ParamDecl`] instead so it is
    /// evaluated against each concrete application. A closed declaration has
    /// no later instantiation, so its verdict must be concrete.
    pub(super) fn validate_declaration_constraint(
        &self,
        name: &str,
        constraint: &GenericConstraint,
    ) -> Result<(), TypeError> {
        self.validate_constraint_in_environment(name, constraint, &HashMap::new())
    }

    /// Whether `constraint` is proven under `environment`. A residual is not
    /// a proof, and neither is its negation: a consumer that must choose
    /// (an overload, a conditional conformance) does not choose on it.
    pub(super) fn eval_generic_constraint(
        &self,
        constraint: &GenericConstraint,
        environment: &HashMap<&str, &TyArg>,
    ) -> bool {
        self.constraint_verdict(constraint, environment)
            .is_ok_and(|verdict| verdict.is_proven())
    }

    /// The three-valued verdict of `constraint` under `environment`.
    pub(super) fn constraint_verdict(
        &self,
        constraint: &GenericConstraint,
        environment: &HashMap<&str, &TyArg>,
    ) -> Result<ConstraintVerdict, TypeError> {
        self.constraint_proposition(constraint, environment)
            .map(ConstraintVerdict::from)
            .map_err(param_error)
    }

    /// Lower `constraint` to its Bool proposition under `environment`. Every
    /// leaf the bindings decide is a constant; a leaf over a residual value is
    /// its canonical expression; a leaf whose parameter has no binding is an
    /// unknown proposition. The connectives are the context's, so there is one
    /// three-valued `and`/`or`/`not`.
    fn constraint_proposition(
        &self,
        constraint: &GenericConstraint,
        environment: &HashMap<&str, &TyArg>,
    ) -> Result<ParamExpr, ParamError> {
        use GenericConstraint::{
            And, Bool, Conforms, ConformsPack, Eq, Ge, Gt, Le, Lt, Ne, Not, Or, PackContains,
            PackPredicate, Trivial, WithMessage,
        };
        let context = &self.param_context;
        let unknown = || context.hole(HoleKind::Unknown, MetaTy::bool());
        // A parameter bound to the wrong kind of argument decides the leaf
        // false; only a parameter with no binding at all is unknown.
        // A pack bound to another declaration's pack that is still a
        // parameter (`Tuple[*Self.Ts]`): its elements are that pack's.
        let forwarded = |param: &str| match environment.get(param) {
            Some(TyArg::Ty(pack @ Ty::Param { binder, .. })) if binder.name.starts_with('*') => {
                Some(pack)
            }
            _ => None,
        };
        let pack = |param: &str, holds: &dyn Fn(&[Ty]) -> bool| match (
            bound_pack_types(environment, param),
            environment.contains_key(param),
        ) {
            (Some(types), _) => context.boolean(holds(&types)),
            (None, true) if forwarded(param).is_some() => unknown(),
            (None, true) => context.boolean(false),
            (None, false) => unknown(),
        };
        Ok(match constraint {
            WithMessage(condition, _) => self.constraint_proposition(condition, environment)?,
            Bool(value) => context.boolean(*value),
            Not(value) => context.not(&self.constraint_proposition(value, environment)?)?,
            And(left, right) => context.op(
                ParamOp::BoolAnd,
                &[
                    self.constraint_proposition(left, environment)?,
                    self.constraint_proposition(right, environment)?,
                ],
            )?,
            Or(left, right) => context.op(
                ParamOp::BoolOr,
                &[
                    self.constraint_proposition(left, environment)?,
                    self.constraint_proposition(right, environment)?,
                ],
            )?,
            Conforms { param, trait_name } => match environment.get(param.as_str()) {
                Some(TyArg::Ty(ty)) => context.boolean(self.conforms_to(ty, trait_name)),
                Some(TyArg::Val(_) | TyArg::Origin(_)) => context.boolean(false),
                None => unknown(),
            },
            Trivial(kind, operand) => match self.constraint_value(operand, environment) {
                Some(TyArg::Ty(ty)) => context.boolean(self.is_trivially(*kind, &ty)),
                Some(_) => context.boolean(false),
                None => unknown(),
            },
            // Every element of a forwarded pack conforms when that pack's
            // bound or an enclosing `where` guarantees the trait; otherwise
            // nothing is known of them.
            ConformsPack { param, trait_name }
                if forwarded(param).is_some_and(|pack| self.conforms_to(pack, trait_name)) =>
            {
                context.boolean(true)
            }
            ConformsPack { param, trait_name } => pack(param, &|types| {
                types.iter().all(|ty| self.conforms_to(ty, trait_name))
            }),
            PackPredicate {
                param,
                predicate,
                all,
            } => pack(param, &|types| {
                let mut holds = types
                    .iter()
                    .map(|ty| self.eval_pack_predicate(predicate, ty));
                if *all {
                    holds.all(|held| held)
                } else {
                    holds.any(|held| held)
                }
            }),
            PackContains { param, element } => match self.constraint_value(element, environment) {
                Some(TyArg::Ty(needle)) => pack(param, &|types| types.contains(&needle)),
                Some(_) => context.boolean(false),
                None => unknown(),
            },
            Eq(left, right) | Ne(left, right) => {
                let equal = match (
                    self.constraint_value(left, environment),
                    self.constraint_value(right, environment),
                ) {
                    (Some(TyArg::Val(left)), Some(TyArg::Val(right)))
                        if matches!(left, CtValue::Expr(_))
                            || matches!(right, CtValue::Expr(_)) =>
                    {
                        context.infix(
                            InfixOp::Eq,
                            &context.constant(left)?,
                            &context.constant(right)?,
                        )?
                    }
                    (Some(left), Some(right)) => context.boolean(ty_args_equal(&left, &right)),
                    _ => unknown(),
                };
                if matches!(constraint, Ne(..)) {
                    context.not(&equal)?
                } else {
                    equal
                }
            }
            Lt(left, right) | Le(left, right) | Gt(left, right) | Ge(left, right) => {
                let op = match constraint {
                    Lt(..) => InfixOp::Lt,
                    Le(..) => InfixOp::Le,
                    Gt(..) => InfixOp::Gt,
                    _ => InfixOp::Ge,
                };
                match (
                    self.constraint_value(left, environment),
                    self.constraint_value(right, environment),
                ) {
                    (Some(TyArg::Val(left)), Some(TyArg::Val(right)))
                        if matches!(left, CtValue::Expr(_))
                            || matches!(right, CtValue::Expr(_)) =>
                    {
                        context.infix(op, &context.constant(left)?, &context.constant(right)?)?
                    }
                    (Some(TyArg::Val(left)), Some(TyArg::Val(right))) => {
                        context.boolean(compare_ct_integers(op, &left, &right).unwrap_or(false))
                    }
                    (Some(_), Some(_)) => context.boolean(false),
                    _ => unknown(),
                }
            }
        })
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
            // Replacement binds the application's values by name and re-folds;
            // what stays symbolic is returned residual.
            ConstraintOperand::Expr(expression) => {
                let context = &self.param_context;
                let mut bindings = mojito_types::param_expr::ParamBindings::new();
                for (name, argument) in environment {
                    if let TyArg::Val(value) = argument
                        && let Ok(value) = context.constant(value.clone())
                    {
                        bindings.bind_name(name, value);
                    }
                }
                // As for a type argument, an opaque atom stays unfolded: the
                // pin finds no evidence for `n // -2 == -4` even at `n = 7`
                // (`assets/type_error/param_expr_where_unfolded_atom.mojo`).
                context
                    .replace(expression, &bindings)
                    .ok()
                    .map(|replaced| TyArg::Val(replaced.into_value()))
            }
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
                    .is_ok_and(|constraint| {
                        self.eval_generic_constraint(&constraint, &HashMap::new())
                    })
            }
        }
    }
}

/// The conjuncts of a proposition: the operands of a canonical `and`, or the
/// proposition itself.
fn conjuncts(proposition: &ParamExpr) -> Vec<ParamExpr> {
    match proposition.kind() {
        mojito_types::param_expr::ParamKind::Op {
            op: ParamOp::BoolAnd,
            operands,
        } => operands.clone(),
        _ => vec![proposition.clone()],
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
pub(super) enum TypeListReceiver {
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

/// The pack a `.values` projection names: `Ts.values` (a `def`'s own pack)
/// or `Self.Ts.values` (a struct's pack inside its members).
pub(super) fn pack_values_projection(expression: &Expr) -> Option<&str> {
    let ExprKind::Member { object, field } = &expression.kind else {
        return None;
    };
    if field != "values" {
        return None;
    }
    match &object.kind {
        ExprKind::Identifier(pack) => Some(pack),
        ExprKind::Member {
            object: base,
            field: pack,
        } if matches!(&base.kind, ExprKind::Identifier(name) if name == "Self") => Some(pack),
        _ => None,
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
    use GenericConstraint::{
        And, Bool, Conforms, ConformsPack, Eq, Ge, Gt, Le, Lt, Ne, Not, Or, PackContains,
        PackPredicate, Trivial, WithMessage,
    };
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
        ConstraintOperand::Expr(_) => Err(TypeError::Unsupported(
            "an arithmetic operand in a Bool-bodied comptime alias".to_string(),
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
