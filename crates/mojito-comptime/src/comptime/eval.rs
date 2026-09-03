//! Compile-time expression evaluation: the `eval` dispatcher, reflection-method
//! evaluation, reflected-type resolution, and infix/iteration folding.
//! Extracted from `comptime.rs`; see `docs/symbol-map.md`.

use super::*;

impl<'a> Elab<'a> {
    /// Evaluate a compile-time expression to a `CtValue`. `scope` is the current
    /// variable environment (module constants, or a CTFE call frame's locals).
    pub(super) fn eval(
        &self,
        e: &Expr,
        scope: &HashMap<String, CtValue>,
    ) -> Result<CtValue, ComptimeError> {
        match &e.kind {
            ExprKind::Int(n) => Ok(CtValue::IntLiteral(n.clone())),
            ExprKind::Float(value) => Ok(CtValue::FloatLiteral(value.clone())),
            ExprKind::Bool(b) => Ok(CtValue::Bool(*b)),
            ExprKind::Str(s) => Ok(CtValue::Str(s.clone())),
            ExprKind::Identifier(name) => {
                if let Some(value) = scope.get(name) {
                    return Ok(value.clone());
                }
                self.type_value(name, &[], scope)
            }
            ExprKind::TypeValue(ty) => Ok(CtValue::Type(Box::new(
                self.param_arg_type(&ParamArg::Type(ty.clone()), scope)?,
            ))),
            ExprKind::TypeApply { name, args }
                if mojito_types::types::trivial_predicate_name(name).is_some() =>
            {
                let kind = mojito_types::types::trivial_predicate_name(name).expect("guarded");
                if args.len() != 1 {
                    return Err(ComptimeError::Arity(format!(
                        "{name}[T] takes exactly one type parameter"
                    )));
                }
                let ty = self.param_arg_type(&args[0], scope)?;
                Ok(CtValue::Bool(self.conformance.trivially(kind, &ty)))
            }
            ExprKind::TypeApply { name, args } if name == "reflect" => {
                if args.len() != 1 {
                    return Err(ComptimeError::Arity(
                        "reflect[T] takes exactly one type parameter".to_string(),
                    ));
                }
                Ok(CtValue::Reflected(Box::new(
                    self.param_arg_type(&args[0], scope)?,
                )))
            }
            // A module-scope generic comptime alias applied in a compile-time
            // position (typically a `comptime if` condition, pruned before the
            // checker's own alias registry could expand it).
            ExprKind::TypeApply { name, args }
                if self.generic_aliases.borrow().contains_key(name) =>
            {
                self.apply_generic_alias(name, args, scope)
            }
            ExprKind::TypeApply { name, args } => self.type_value(name, args, scope),
            ExprKind::TupleLit(elems) => Ok(CtValue::Tuple(self.eval_all(elems, scope)?)),
            ExprKind::ListLit(elems) => Ok(CtValue::List(self.eval_all(elems, scope)?)),
            ExprKind::Member { object, field } => {
                if let ExprKind::Identifier(name) = &object.kind
                    && name == "Self"
                    && let Some(value) = scope.get(field)
                {
                    return Ok(value.clone());
                }
                // `DType.<dt>` is a compile-time dtype value (the binding of
                // a `[dtype: DType]` parameter).
                if let ExprKind::Identifier(name) = &object.kind
                    && name == "DType"
                    && let Some(dtype) = mojito_ast::ast::Dtype::from_name(field)
                {
                    return Ok(CtValue::Dtype(dtype));
                }
                match self.eval(object, scope)? {
                    CtValue::Type(ty) => self.associated_value(&ty, field),
                    CtValue::Reflected(ty) if field == "T" => Ok(CtValue::Type(ty)),
                    // `tl.length` on a compile-time TypeList value (the
                    // removed `size` alias rejects like any unknown member).
                    value if value.typelist_elements().is_some() && field == "length" => Ok(
                        CtValue::Int(value.typelist_elements().expect("guarded").len() as i64),
                    ),
                    // A field read on a frozen struct instance folds to the
                    // frozen field value.
                    CtValue::Struct { name, fields } => fields
                        .iter()
                        .find(|(candidate, _)| candidate == field)
                        .map(|(_, value)| value.clone())
                        .ok_or_else(|| {
                            ComptimeError::NotComptime(format!(
                                "compile-time '{name}' value has no field '{field}'"
                            ))
                        }),
                    _ => Err(ComptimeError::NotComptime(format!(
                        "compile-time member access '.{field}' needs a type value"
                    ))),
                }
            }
            ExprKind::Index { object, index } => {
                // `IsTriviallyCopyable[Plain]`: a single non-scalar bracket
                // argument parses as runtime indexing, so the predicate is
                // recognized here as well as on the TypeApply path.
                if let ExprKind::Identifier(name) = &object.kind
                    && let Some(kind) = mojito_types::types::trivial_predicate_name(name)
                {
                    let ty = self.param_arg_type(&ParamArg::Value((**index).clone()), scope)?;
                    return Ok(CtValue::Bool(self.conformance.trivially(kind, &ty)));
                }
                // A generic-alias application whose single non-scalar bracket
                // argument parses as indexing (`MyPred[Plain]`), or a
                // multi-argument application (`MyPred[A, B]`, a tuple index).
                if let ExprKind::Identifier(name) = &object.kind
                    && self.generic_aliases.borrow().contains_key(name)
                {
                    let args: Vec<ParamArg> = match &index.kind {
                        ExprKind::TupleLit(elements) => {
                            elements.iter().cloned().map(ParamArg::Value).collect()
                        }
                        _ => vec![ParamArg::Value((**index).clone())],
                    };
                    return self.apply_generic_alias(name, &args, scope);
                }
                if let ExprKind::Member {
                    object: reflected,
                    field,
                } = &object.kind
                    && matches!(field.as_str(), "field" | "field_at" | "field_type")
                {
                    if field == "field_type" {
                        return Err(ComptimeError::NotComptime(
                            "Reflected.field_type was removed; use Reflected.field[name]"
                                .to_string(),
                        ));
                    }
                    let CtValue::Reflected(ty) = self.eval(reflected, scope)? else {
                        return Err(ComptimeError::NotComptime(format!(
                            "compile-time reflection selector '{field}' needs a reflect[T] handle"
                        )));
                    };
                    return self.eval_reflected_field_handle(&ty, field, index, scope);
                }
                let seq = self
                    .eval(object, scope)?
                    .as_sequence("indexing a comptime collection")?;
                let i = self.eval(index, scope)?.as_int("comptime index")?;
                seq.get(i as usize).cloned().ok_or_else(|| {
                    ComptimeError::BadArithmetic(format!("comptime index {i} out of range"))
                })
            }
            ExprKind::MethodCall {
                object,
                method,
                args,
                kwargs,
            } if method == "__len__" && args.is_empty() && kwargs.is_empty() => {
                let sequence = self
                    .eval(object, scope)?
                    .as_sequence("__len__() of a compile-time collection")?;
                Ok(CtValue::Int(sequence.len() as i64))
            }
            ExprKind::MethodCall {
                object,
                method,
                args,
                kwargs,
            } if args.is_empty() && kwargs.is_empty() => {
                let CtValue::Reflected(ty) = self.eval(object, scope)? else {
                    return Err(ComptimeError::NotComptime(format!(
                        "compile-time reflection method '{method}' needs a reflect[T] handle"
                    )));
                };
                self.eval_reflection_method(&ty, method, scope)
            }
            ExprKind::Invoke {
                callee,
                param_args,
                args,
                kwargs,
            } if args.is_empty() && kwargs.is_empty() => {
                let ExprKind::Member { object, field } = &callee.kind else {
                    return Err(ComptimeError::NotComptime(
                        "unsupported parameterized compile-time callable".to_string(),
                    ));
                };
                // `TypeList.of[...]()` — the concrete constructor. Checked
                // before evaluating the object: `TypeList` is not a value.
                if matches!(&object.kind, ExprKind::Identifier(name) if name == "TypeList")
                    && field == "of"
                {
                    return self.eval_typelist_of(param_args, scope);
                }
                let receiver = self.eval(object, scope)?;
                if receiver.typelist_elements().is_some() {
                    return self.eval_typelist_method(&receiver, field, param_args, scope);
                }
                let CtValue::Reflected(ty) = receiver else {
                    return Err(ComptimeError::NotComptime(format!(
                        "compile-time reflection method '{field}' needs a reflect[T] handle"
                    )));
                };
                self.eval_parameterized_reflection_method(&ty, field, param_args, scope)
            }
            ExprKind::Prefix(PrefixOp::Neg, inner) => match self.eval(inner, scope)? {
                CtValue::Int(value) => value.checked_neg().map(CtValue::Int).ok_or_else(|| {
                    ComptimeError::BadArithmetic("compile-time integer overflow".to_string())
                }),
                CtValue::IntLiteral(value) => Ok(CtValue::IntLiteral(value.neg())),
                CtValue::Float(value) => Ok(CtValue::Float((-f64::from_bits(value)).to_bits())),
                CtValue::FloatLiteral(value) => Ok(CtValue::FloatLiteral(value.neg())),
                _ => Err(ComptimeError::NotComptime(
                    "unary '-' expects a compile-time numeric value".to_string(),
                )),
            },
            ExprKind::Prefix(PrefixOp::Not, inner) => {
                Ok(CtValue::Bool(!self.eval(inner, scope)?.as_bool("'not'")?))
            }
            ExprKind::Infix(op, l, r) => self.eval_infix(*op, l, r, scope),
            ExprKind::Compare { first, rest } => {
                let mut left = self.eval(first, scope)?;
                for (op, right) in rest {
                    let r = self.eval(right, scope)?;
                    if !compare_numeric_values(*op, &left, &r)? {
                        return Ok(CtValue::Bool(false));
                    }
                    left = r;
                }
                Ok(CtValue::Bool(true))
            }
            // `TypeList[Ts.values]()` — the pack adapter, wrapping the bound
            // pack's element types as a compile-time TypeList value.
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } if name == "TypeList" && args.is_empty() && kwargs.is_empty() => {
                let [ParamArg::Value(projection)] = param_args.as_slice() else {
                    return Err(ComptimeError::NotComptime(
                        "TypeList[...] takes a pack projection ('Ts.values')".to_string(),
                    ));
                };
                let values = match &projection.kind {
                    ExprKind::Member { object, field }
                        if field == "values" && matches!(&object.kind, ExprKind::Identifier(_)) =>
                    {
                        let ExprKind::Identifier(pack) = &object.kind else {
                            unreachable!("guarded above");
                        };
                        scope.get(pack).cloned().ok_or_else(|| {
                            ComptimeError::NotComptime(format!(
                                "unknown compile-time type pack '{pack}'"
                            ))
                        })?
                    }
                    _ => self.eval(projection, scope)?,
                };
                let elements = values.as_sequence("TypeList element pack")?;
                Ok(make_typelist(elements))
            }
            ExprKind::Call {
                name, args, kwargs, ..
            } if name == "conforms_to" && kwargs.is_empty() && args.len() == 2 => {
                let ExprKind::Identifier(trait_name) = &args[1].kind else {
                    return Err(ComptimeError::NotComptime(
                        "conforms_to requires a trait name".to_string(),
                    ));
                };
                let trait_name = mojito_ast::ast::canonical_trait_name(trait_name);
                let values = match &args[0].kind {
                    ExprKind::Member { object, field }
                        if field == "values" && matches!(&object.kind, ExprKind::Identifier(_)) =>
                    {
                        let ExprKind::Identifier(pack) = &object.kind else {
                            unreachable!("guard established a pack identifier")
                        };
                        scope.get(pack).cloned().ok_or_else(|| {
                            ComptimeError::NotComptime(format!(
                                "unknown compile-time type pack '{pack}'"
                            ))
                        })?
                    }
                    _ => self.eval(&args[0], scope)?,
                };
                let types = match values {
                    CtValue::Type(ty) => vec![*ty],
                    CtValue::Tuple(values) | CtValue::List(values) => values
                        .into_iter()
                        .map(|value| match value {
                            CtValue::Type(ty) => Ok(*ty),
                            _ => Err(ComptimeError::NotComptime(
                                "conforms_to expects a type or type pack".to_string(),
                            )),
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                    _ => {
                        return Err(ComptimeError::NotComptime(
                            "conforms_to expects a type or type pack".to_string(),
                        ));
                    }
                };
                Ok(CtValue::Bool(types.iter().all(|ty| {
                    self.conformance.require(ty, trait_name).is_ok()
                })))
            }
            // A built-in compile-time **type predicate** (roadmap milestone 7): `is_same_type[T,
            // U]()` is `Bool` type equality, usable in a `comptime if`.
            ExprKind::Call {
                name,
                param_args,
                args,
                ..
            } if name == "is_same_type" => self.eval_is_same_type(param_args, args, scope),
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } if name == "reflect" && args.is_empty() && kwargs.is_empty() => {
                if param_args.len() != 1 {
                    return Err(ComptimeError::Arity(
                        "reflect[T]() takes exactly one type parameter".to_string(),
                    ));
                }
                Ok(CtValue::Reflected(Box::new(
                    self.param_arg_type(&param_args[0], scope)?,
                )))
            }
            ExprKind::Call { name, args, .. } if name == "len" && args.len() == 1 => {
                let sequence = self
                    .eval(&args[0], scope)?
                    .as_sequence("len() of a compile-time collection")?;
                Ok(CtValue::Int(sequence.len() as i64))
            }
            // Constructing a struct at compile time → VM CTFE through a
            // synthesized entry, freezing the resulting instance.
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } if kwargs.is_empty()
                && param_args.is_empty()
                && !self.fns.contains_key(name.as_str())
                && self.structs.contains_key(name.as_str()) =>
            {
                let literal_args = self.eval_to_literals(args, e.span, scope)?;
                self.ctfe_struct_entry(name, None, literal_args, e.span)
            }
            // A call into a pure top-level function → CTFE.
            ExprKind::Call {
                name,
                param_args,
                args,
                ..
            } => {
                let argv = self.eval_all(args, scope)?;
                self.ctfe_call(name, param_args, argv, scope)
            }
            // A static method on a struct (`Extent.square(4)`) → the
            // same synthesized-entry CTFE.
            ExprKind::MethodCall {
                object,
                method,
                args,
                kwargs,
            } if kwargs.is_empty()
                && matches!(&object.kind, ExprKind::Identifier(name)
                    if self.structs.contains_key(name.as_str())) =>
            {
                let ExprKind::Identifier(struct_name) = &object.kind else {
                    unreachable!("guard established an identifier receiver");
                };
                let literal_args = self.eval_to_literals(args, e.span, scope)?;
                self.ctfe_struct_entry(struct_name, Some(method), literal_args, e.span)
            }
            _ => Err(ComptimeError::NotComptime(
                "unsupported compile-time expression".to_string(),
            )),
        }
    }

    /// Evaluate arguments and materialize each back to a scope-free literal
    /// expression (the body of a synthesized CTFE entry).
    /// Apply a module-scope generic comptime alias: bind each argument by
    /// position and evaluate the body against the module-constant environment
    /// (the alias sees module scope, not the applier's locals). Arguments
    /// evaluate as compile-time values, falling back to type resolution for a
    /// bare type argument.
    fn apply_generic_alias(
        &self,
        name: &str,
        args: &[ParamArg],
        scope: &HashMap<String, CtValue>,
    ) -> Result<CtValue, ComptimeError> {
        let mut values = Vec::with_capacity(args.len());
        for argument in args {
            values.push(match argument {
                ParamArg::Type(annotation) => {
                    CtValue::Type(Box::new(self.type_from_anno(annotation, scope)?))
                }
                ParamArg::Value(expr) => match self.eval(expr, scope) {
                    Ok(value) => value,
                    Err(_) => CtValue::Type(Box::new(self.param_arg_type(argument, scope)?)),
                },
                ParamArg::Named { .. } => {
                    return Err(ComptimeError::NotComptime(
                        "a generic comptime alias takes positional arguments".to_string(),
                    ));
                }
            });
        }
        self.apply_generic_alias_values(name, values)
    }

    /// The value-level core of [`Self::apply_generic_alias`], shared with the
    /// TypeList `any`/`all` per-element predicate evaluation.
    fn apply_generic_alias_values(
        &self,
        name: &str,
        values: Vec<CtValue>,
    ) -> Result<CtValue, ComptimeError> {
        self.burn()?;
        let (params, body) = self
            .generic_aliases
            .borrow()
            .get(name)
            .cloned()
            .expect("guarded by generic_aliases lookup");
        if values.len() != params.len() {
            return Err(ComptimeError::Arity(format!(
                "{name}[...] takes exactly {} argument(s)",
                params.len()
            )));
        }
        let mut env = self.top_consts.borrow().clone();
        for (param, value) in params.iter().zip(values) {
            env.insert(param.name.clone(), value);
        }
        self.eval(&body, &env)
    }

    /// Evaluate `TypeList.of[Trait=..., T1, ..., Tn]()` to a compile-time
    /// TypeList value. The optional `Trait=` keyword names the common bound;
    /// membership is not re-checked here (each element's uses enforce their
    /// own capabilities).
    fn eval_typelist_of(
        &self,
        param_args: &[ParamArg],
        scope: &HashMap<String, CtValue>,
    ) -> Result<CtValue, ComptimeError> {
        let mut types = Vec::new();
        for argument in param_args {
            match argument {
                ParamArg::Named { name, .. } if name == "Trait" => {}
                other => types.push(CtValue::Type(Box::new(self.param_arg_type(other, scope)?))),
            }
        }
        Ok(make_typelist(types))
    }

    /// Evaluate a member call on a compile-time TypeList value:
    /// `any`/`all` (per-element predicates: IsTrivially* or a one-parameter
    /// Bool-bodied comptime alias), `all_conforms_to`, and `contains`.
    fn eval_typelist_method(
        &self,
        receiver: &CtValue,
        field: &str,
        param_args: &[ParamArg],
        scope: &HashMap<String, CtValue>,
    ) -> Result<CtValue, ComptimeError> {
        let elements = receiver
            .typelist_elements()
            .expect("guarded by the caller")
            .to_vec();
        let types = elements
            .iter()
            .map(|value| match value {
                CtValue::Type(ty) => Ok((**ty).clone()),
                _ => Err(ComptimeError::NotComptime(
                    "a TypeList holds types".to_string(),
                )),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let single_name = || -> Result<&str, ComptimeError> {
            match param_args {
                [
                    ParamArg::Value(Expr {
                        kind: ExprKind::Identifier(name),
                        ..
                    }),
                ]
                | [ParamArg::Type(Type::Named(name, _))] => Ok(name),
                _ => Err(ComptimeError::NotComptime(format!(
                    "TypeList.{field} takes exactly one compile-time name argument"
                ))),
            }
        };
        match field {
            "all_conforms_to" => {
                let trait_name = mojito_ast::ast::canonical_trait_name(single_name()?);
                Ok(CtValue::Bool(types.iter().all(|ty| {
                    self.conformance.require(ty, trait_name).is_ok()
                })))
            }
            "any" | "all" => {
                let predicate = single_name()?.to_string();
                let all = field == "all";
                let mut holds = Vec::with_capacity(types.len());
                for ty in &types {
                    let value = if let Some(kind) =
                        mojito_types::types::trivial_predicate_name(&predicate)
                    {
                        self.conformance.trivially(kind, ty)
                    } else if self.generic_aliases.borrow().contains_key(&predicate) {
                        self.apply_generic_alias_values(
                            &predicate,
                            vec![CtValue::Type(Box::new(ty.clone()))],
                        )?
                        .as_bool("TypeList predicate")?
                    } else {
                        return Err(ComptimeError::NotComptime(format!(
                            "TypeList.{field} requires an IsTrivially* predicate or a \
                                 Bool-bodied comptime alias"
                        )));
                    };
                    holds.push(value);
                }
                Ok(CtValue::Bool(if all {
                    holds.iter().all(|held| *held)
                } else {
                    holds.iter().any(|held| *held)
                }))
            }
            "contains" => {
                let [only] = param_args else {
                    return Err(ComptimeError::NotComptime(
                        "TypeList.contains takes exactly one type argument".to_string(),
                    ));
                };
                let needle = self.param_arg_type(only, scope)?;
                Ok(CtValue::Bool(types.contains(&needle)))
            }
            _ => Err(ComptimeError::NotComptime(format!(
                "unsupported TypeList member '{field}'"
            ))),
        }
    }

    pub(super) fn eval_to_literals(
        &self,
        exprs: &[Expr],
        span: Span,
        scope: &HashMap<String, CtValue>,
    ) -> Result<Vec<Expr>, ComptimeError> {
        exprs
            .iter()
            .map(|argument| {
                let value = self.eval(argument, scope)?;
                value.materialize(span).ok_or_else(|| {
                    ComptimeError::NotComptime(format!(
                        "compile-time argument {value} has no runtime form"
                    ))
                })
            })
            .collect()
    }

    pub(super) fn eval_all(
        &self,
        exprs: &[Expr],
        scope: &HashMap<String, CtValue>,
    ) -> Result<Vec<CtValue>, ComptimeError> {
        exprs.iter().map(|e| self.eval(e, scope)).collect()
    }

    pub(super) fn eval_reflection_method(
        &self,
        ty: &Ty,
        method: &str,
        outer_scope: &HashMap<String, CtValue>,
    ) -> Result<CtValue, ComptimeError> {
        if method == "is_struct" {
            return Ok(CtValue::Bool(matches!(ty, Ty::Struct(_, _))));
        }
        let Ty::Struct(name, arguments) = ty else {
            return Err(ComptimeError::NotComptime(format!(
                "reflect[{ty}].{method}() requires a struct type"
            )));
        };
        let info = self.structs.get(name).ok_or_else(|| {
            ComptimeError::NotComptime(format!("cannot reflect unknown struct '{name}'"))
        })?;
        match method {
            "field_count" => Ok(CtValue::Int(info.fields.len() as i64)),
            "field_names" => Ok(CtValue::Tuple(
                info.fields
                    .iter()
                    .map(|field| CtValue::Str(field.name.clone()))
                    .collect(),
            )),
            "field_types" => {
                let mut scope = outer_scope.clone();
                for (decl, argument) in info.decls.iter().zip(arguments) {
                    // Origins erase from runtime state; they bind no CTFE value.
                    let Some(value) = argument.ct_value() else {
                        continue;
                    };
                    scope.insert(decl.name().trim_start_matches('*').to_string(), value);
                }
                info.fields
                    .iter()
                    .map(|field| {
                        self.type_from_anno(&field.ty, &scope)
                            .map(|ty| CtValue::Type(Box::new(ty)))
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map(CtValue::Tuple)
            }
            _ => Err(ComptimeError::NotComptime(format!(
                "unsupported reflect[T] method '{method}'"
            ))),
        }
    }

    pub(super) fn eval_parameterized_reflection_method(
        &self,
        ty: &Ty,
        method: &str,
        parameters: &[ParamArg],
        scope: &HashMap<String, CtValue>,
    ) -> Result<CtValue, ComptimeError> {
        if method == "field_type" {
            return Err(ComptimeError::NotComptime(
                "Reflected.field_type was removed; use Reflected.field[name]".to_string(),
            ));
        }
        let Ty::Struct(name, _arguments) = ty else {
            return Err(ComptimeError::NotComptime(format!(
                "reflect[{ty}].{method} requires a struct type"
            )));
        };
        let info = self.structs.get(name).ok_or_else(|| {
            ComptimeError::NotComptime(format!("cannot reflect unknown struct '{name}'"))
        })?;
        let field_name = match parameters {
            [ParamArg::Value(expr)] => match self.eval(expr, scope)? {
                CtValue::Str(name) => name,
                other => {
                    return Err(ComptimeError::NotComptime(format!(
                        "reflection field name must be String, got {other}"
                    )));
                }
            },
            [
                ParamArg::Named {
                    name: parameter,
                    value,
                },
            ] if parameter == "name" => match self.resolve_ct_arg(
                &ParamDecl::Value {
                    name: "name".to_string(),
                    ty: Box::new(Ty::StringLiteral),
                    default: None,
                    callable_default: None,
                    infer_only: false,
                    variadic: false,
                    constraints: Vec::new(),
                },
                value,
                scope,
            )? {
                CtValue::Str(name) => name,
                _ => unreachable!(),
            },
            _ => {
                return Err(ComptimeError::Arity(format!(
                    "reflect[T].{method}[name]() takes one String parameter"
                )));
            }
        };
        let index = info
            .fields
            .iter()
            .position(|field| field.name == field_name)
            .ok_or_else(|| {
                ComptimeError::NotComptime(format!(
                    "struct '{name}' has no field named '{field_name}'"
                ))
            })?;
        match method {
            "field_index" => Ok(CtValue::Int(index as i64)),
            _ => Err(ComptimeError::NotComptime(format!(
                "unsupported parameterized reflect[T] method '{method}'"
            ))),
        }
    }

    /// Resolve the current type-valued reflected-field aliases.  Both selectors
    /// return another `Reflected` value, rather than the bare type, which makes
    /// nested selection (`reflect[Outer].field["inner"].field_at[0]`) and the
    /// terminal `.T` member use the same representation as the root handle.
    pub(super) fn eval_reflected_field_handle(
        &self,
        ty: &Ty,
        selector: &str,
        argument: &Expr,
        scope: &HashMap<String, CtValue>,
    ) -> Result<CtValue, ComptimeError> {
        let Ty::Struct(name, arguments) = ty else {
            return Err(ComptimeError::NotComptime(format!(
                "reflect[{ty}].{selector}[...] requires a struct type"
            )));
        };
        let info = self.structs.get(name).ok_or_else(|| {
            ComptimeError::NotComptime(format!("cannot reflect unknown struct '{name}'"))
        })?;
        let selected = self.eval(argument, scope)?;
        let index = match (selector, &selected) {
            ("field", CtValue::Str(field_name)) => info
                .fields
                .iter()
                .position(|field| field.name == *field_name)
                .ok_or_else(|| {
                    ComptimeError::NotComptime(format!(
                        "struct '{name}' has no field named '{field_name}'"
                    ))
                })?,
            ("field", other) => {
                return Err(ComptimeError::NotComptime(format!(
                    "Reflected.field expects a String field name, got {other}"
                )));
            }
            ("field_at", CtValue::Int(_) | CtValue::IntLiteral(_)) => {
                let raw_index = selected.as_int("reflection field index")?;
                if raw_index < 0 {
                    return Err(ComptimeError::NotComptime(format!(
                        "reflection field index {raw_index} is out of range for struct '{name}'"
                    )));
                }
                let index = usize::try_from(raw_index).map_err(|_| {
                    ComptimeError::NotComptime(format!(
                        "reflection field index {raw_index} is out of range for struct '{name}'"
                    ))
                })?;
                if index >= info.fields.len() {
                    return Err(ComptimeError::NotComptime(format!(
                        "reflection field index {index} is out of range for struct '{name}' with {} field(s)",
                        info.fields.len()
                    )));
                }
                index
            }
            ("field_at", other) => {
                return Err(ComptimeError::NotComptime(format!(
                    "Reflected.field_at expects an Int field index, got {other}"
                )));
            }
            _ => unreachable!("reflection selector filtered by the caller"),
        };

        let mut type_scope = scope.clone();
        for (decl, argument) in info.decls.iter().zip(arguments) {
            // Origins erase from runtime state; they bind no CTFE value.
            let Some(value) = argument.ct_value() else {
                continue;
            };
            type_scope.insert(decl.name().trim_start_matches('*').to_string(), value);
        }
        let field_ty = self.type_from_anno(&info.fields[index].ty, &type_scope)?;
        Ok(CtValue::Reflected(Box::new(field_ty)))
    }

    /// Replace a reflected handle's terminal `.T` with an ordinary source type
    /// before the handle-only comptime binding is erased. This is the handoff
    /// that makes the nightly pattern `comptime f = reflect[S].field["x"]`
    /// followed by `var value: f.T` visible to the regular checker.
    pub(super) fn resolve_reflected_type(
        &self,
        source: &Type,
        scope: &HashMap<String, CtValue>,
    ) -> Result<Type, ComptimeError> {
        if let Type::Assoc { base, name, .. } = source
            && name == "T"
            && let Type::Named(binding, arguments) = &**base
            && arguments.is_empty()
            && let Some(CtValue::Reflected(ty)) = scope.get(binding)
        {
            return source_type_from_ty(ty).ok_or_else(|| {
                ComptimeError::NotComptime(format!(
                    "reflected type '{ty}' cannot be represented in a source annotation"
                ))
            });
        }

        Ok(match source {
            Type::Named(name, arguments) => Type::Named(
                name.clone(),
                arguments
                    .iter()
                    .map(|argument| self.resolve_reflected_param_arg(argument, scope))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            Type::Assoc { base, name, args } => Type::Assoc {
                base: Box::new(self.resolve_reflected_type(base, scope)?),
                name: name.clone(),
                args: args
                    .iter()
                    .map(|argument| self.resolve_reflected_param_arg(argument, scope))
                    .collect::<Result<Vec<_>, _>>()?,
            },
            Type::Func {
                type_params,
                params,
                ret,
                thin,
                capturing,
                raises,
                raises_type,
                where_clauses,
            } => Type::Func {
                type_params: type_params
                    .iter()
                    .map(|parameter| {
                        let mut parameter = parameter.clone();
                        if let Some(value_type) = &mut parameter.value_type {
                            *value_type = self.resolve_reflected_type(value_type, scope)?;
                        }
                        if let Some(callable) = &mut parameter.callable_bound {
                            *callable = self.resolve_reflected_type(callable, scope)?;
                        }
                        Ok(parameter)
                    })
                    .collect::<Result<Vec<_>, ComptimeError>>()?,
                params: params
                    .iter()
                    .map(|param| {
                        let mut param = param.clone();
                        param.ty = self.resolve_reflected_type(&param.ty, scope)?;
                        Ok(param)
                    })
                    .collect::<Result<Vec<_>, _>>()?,
                ret: Box::new(self.resolve_reflected_type(ret, scope)?),
                thin: *thin,
                capturing: capturing.clone(),
                raises: *raises,
                raises_type: raises_type
                    .as_deref()
                    .map(|ty| self.resolve_reflected_type(ty, scope).map(Box::new))
                    .transpose()?,
                where_clauses: where_clauses.clone(),
            },
            Type::Ref { referent, origin } => Type::Ref {
                referent: Box::new(self.resolve_reflected_type(referent, scope)?),
                origin: origin.clone(),
            },
            scalar_or_symbolic => scalar_or_symbolic.clone(),
        })
    }

    pub(super) fn resolve_reflected_param_arg(
        &self,
        argument: &ParamArg,
        scope: &HashMap<String, CtValue>,
    ) -> Result<ParamArg, ComptimeError> {
        Ok(match argument {
            ParamArg::Type(ty) => ParamArg::Type(self.resolve_reflected_type(ty, scope)?),
            ParamArg::Value(value) => ParamArg::Value(value.clone()),
            ParamArg::Named { name, value } => ParamArg::Named {
                name: name.clone(),
                value: Box::new(self.resolve_reflected_param_arg(value, scope)?),
            },
        })
    }

    pub(super) fn eval_infix(
        &self,
        op: InfixOp,
        l: &Expr,
        r: &Expr,
        scope: &HashMap<String, CtValue>,
    ) -> Result<CtValue, ComptimeError> {
        match op {
            InfixOp::And => {
                return Ok(CtValue::Bool(
                    self.eval(l, scope)?.as_bool("'and'")?
                        && self.eval(r, scope)?.as_bool("'and'")?,
                ));
            }
            InfixOp::Or => {
                return Ok(CtValue::Bool(
                    self.eval(l, scope)?.as_bool("'or'")?
                        || self.eval(r, scope)?.as_bool("'or'")?,
                ));
            }
            _ => {}
        }
        // Type equality is a compile-time proposition used by trailing `where`
        // clauses after their type parameters have been specialized.
        if let (CtValue::Type(left), CtValue::Type(right)) =
            (self.eval(l, scope)?, self.eval(r, scope)?)
        {
            return match op {
                InfixOp::Eq => Ok(CtValue::Bool(left == right)),
                InfixOp::Ne => Ok(CtValue::Bool(left != right)),
                _ => Err(ComptimeError::NotComptime(
                    "only == and != are defined for compile-time types".to_string(),
                )),
            };
        }
        // String concatenation (`+`) and equality (`==`/`!=`) at compile time.
        if let (CtValue::Str(a), CtValue::Str(b)) = (self.eval(l, scope)?, self.eval(r, scope)?) {
            return match op {
                InfixOp::Add => Ok(CtValue::Str(a + &b)),
                InfixOp::Eq => Ok(CtValue::Bool(a == b)),
                InfixOp::Ne => Ok(CtValue::Bool(a != b)),
                _ => Err(ComptimeError::NotComptime(
                    "unsupported compile-time String operator".to_string(),
                )),
            };
        }
        let left = self.eval(l, scope)?;
        let right = self.eval(r, scope)?;
        use InfixOp::*;
        let bad = |m: &str| ComptimeError::BadArithmetic(m.to_string());
        if matches!(op, Eq | Ne | Lt | Gt | Le | Ge) {
            return Ok(CtValue::Bool(compare_numeric_values(op, &left, &right)?));
        }
        match (left, right) {
            (CtValue::Int(a), CtValue::Int(b)) => match op {
                Add => a
                    .checked_add(b)
                    .map(CtValue::Int)
                    .ok_or_else(|| bad("compile-time integer overflow")),
                Sub => a
                    .checked_sub(b)
                    .map(CtValue::Int)
                    .ok_or_else(|| bad("compile-time integer overflow")),
                Mul => a
                    .checked_mul(b)
                    .map(CtValue::Int)
                    .ok_or_else(|| bad("compile-time integer overflow")),
                FloorDiv if b != 0 => a
                    .checked_div_euclid(b)
                    .map(CtValue::Int)
                    .ok_or_else(|| bad("compile-time integer overflow")),
                Mod if b != 0 => a
                    .checked_rem_euclid(b)
                    .map(CtValue::Int)
                    .ok_or_else(|| bad("compile-time integer overflow")),
                FloorDiv | Mod => Err(bad("division by zero")),
                Pow if b >= 0 => u32::try_from(b)
                    .ok()
                    .and_then(|exponent| a.checked_pow(exponent))
                    .map(CtValue::Int)
                    .ok_or_else(|| bad("compile-time integer overflow")),
                Pow => Err(bad("negative exponent")),
                _ => Err(ComptimeError::NotComptime(
                    "unsupported compile-time operator".to_string(),
                )),
            },
            (CtValue::IntLiteral(a), CtValue::IntLiteral(b)) => {
                let value = match op {
                    Add => Some(CtValue::IntLiteral(a.add(&b))),
                    Sub => Some(CtValue::IntLiteral(a.sub(&b))),
                    Mul => Some(CtValue::IntLiteral(a.mul(&b))),
                    Div => mojito_common::literal::FloatLiteral::from_int(&a)
                        .div(&mojito_common::literal::FloatLiteral::from_int(&b))
                        .map(CtValue::FloatLiteral),
                    FloorDiv => a.floor_div(&b).map(CtValue::IntLiteral),
                    Mod => a.floor_mod(&b).map(CtValue::IntLiteral),
                    Pow => a.pow(&b).map(CtValue::IntLiteral),
                    Shl => a.shl(&b).map(CtValue::IntLiteral),
                    Shr => a.shr(&b).map(CtValue::IntLiteral),
                    BitAnd => Some(CtValue::IntLiteral(a.bitand(&b))),
                    BitOr => Some(CtValue::IntLiteral(a.bitor(&b))),
                    BitXor => Some(CtValue::IntLiteral(a.bitxor(&b))),
                    _ => {
                        return Err(ComptimeError::NotComptime(
                            "unsupported exact compile-time operator".to_string(),
                        ));
                    }
                };
                value.ok_or_else(|| bad("invalid exact compile-time arithmetic"))
            }
            (CtValue::FloatLiteral(a), CtValue::FloatLiteral(b)) => {
                let value = match op {
                    Add => Some(a.add(&b)),
                    Sub => Some(a.sub(&b)),
                    Mul => Some(a.mul(&b)),
                    Div => a.div(&b),
                    FloorDiv => a.floor_div(&b),
                    Mod => a.floor_mod(&b),
                    Pow => b.to_int_if_whole().and_then(|b| a.pow_int(&b)),
                    _ => {
                        return Err(ComptimeError::NotComptime(
                            "unsupported exact compile-time float operator".to_string(),
                        ));
                    }
                };
                value
                    .map(CtValue::FloatLiteral)
                    .ok_or_else(|| bad("invalid exact compile-time arithmetic"))
            }
            (CtValue::Int(a), CtValue::IntLiteral(b)) => {
                self.eval_infix_values(op, CtValue::IntLiteral(a.into()), CtValue::IntLiteral(b))
            }
            (CtValue::IntLiteral(a), CtValue::Int(b)) => {
                self.eval_infix_values(op, CtValue::IntLiteral(a), CtValue::IntLiteral(b.into()))
            }
            (CtValue::IntLiteral(a), CtValue::FloatLiteral(b)) => self.eval_infix_values(
                op,
                CtValue::FloatLiteral(mojito_common::literal::FloatLiteral::from_int(&a)),
                CtValue::FloatLiteral(b),
            ),
            (CtValue::FloatLiteral(a), CtValue::IntLiteral(b)) => self.eval_infix_values(
                op,
                CtValue::FloatLiteral(a),
                CtValue::FloatLiteral(mojito_common::literal::FloatLiteral::from_int(&b)),
            ),
            _ => Err(ComptimeError::NotComptime(
                "unsupported compile-time operands".to_string(),
            )),
        }
    }

    pub(super) fn eval_infix_values(
        &self,
        op: InfixOp,
        left: CtValue,
        right: CtValue,
    ) -> Result<CtValue, ComptimeError> {
        let scope = HashMap::from([("__left".to_string(), left), ("__right".to_string(), right)]);
        let expression = |name: &str| Expr {
            kind: ExprKind::Identifier(name.to_string()),
            span: Span::default(),
            source: None,
            syntax_id: mojito_common::token::SyntaxId::fresh(),
        };
        self.eval_infix(op, &expression("__left"), &expression("__right"), &scope)
    }

    /// Evaluate a `comptime for` / CTFE `for` iterable to the sequence of loop
    /// values: a `range(...)` of `Int`s, or any compile-time tuple/list.
    pub(super) fn eval_iter(
        &self,
        iter: &Expr,
        scope: &HashMap<String, CtValue>,
    ) -> Result<Vec<CtValue>, ComptimeError> {
        if let ExprKind::Call { name, args, .. } = &iter.kind
            && name == "range"
        {
            let vals: Vec<i64> = args
                .iter()
                .map(|a| self.eval(a, scope)?.as_int("range argument"))
                .collect::<Result<_, _>>()?;
            let (start, stop, step) = match vals.as_slice() {
                [stop] => (0, *stop, 1),
                [start, stop] => (*start, *stop, 1),
                [start, stop, step] => (*start, *stop, *step),
                _ => {
                    return Err(ComptimeError::BadRange(
                        "range takes 1-3 arguments".to_string(),
                    ));
                }
            };
            let mut out = Vec::new();
            let mut i = start;
            while (step > 0 && i < stop) || (step < 0 && i > stop) {
                out.push(CtValue::Int(i));
                i += step;
            }
            return Ok(out);
        }
        self.eval(iter, scope)?
            .as_sequence("a range(...), tuple, or list")
    }
}

/// Wrap element types as the compile-time `TypeList` value: a marker struct
/// holding the `values` tuple, so member access and predicates dispatch on
/// the TypeList identity rather than on every plain comptime tuple.
fn make_typelist(types: Vec<CtValue>) -> CtValue {
    CtValue::Struct {
        name: "TypeList".to_string(),
        fields: vec![("values".to_string(), CtValue::Tuple(types))],
    }
}
