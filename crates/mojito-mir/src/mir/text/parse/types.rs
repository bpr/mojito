//! Type decoding: `Ty`, callable types, parameter declarations,
//! constraints, and comptime expressions/values.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

impl Decoder {
    pub(super) fn ty(&mut self, value: &Value) -> Option<Ty> {
        match &value.kind {
            ValueKind::Atom(tag) => Some(match tag.as_str() {
                "Int" => Ty::Int,
                "UInt" => Ty::UInt,
                "Bool" => Ty::Bool,
                "StringLiteral" => Ty::StringLiteral,
                "Float64" => Ty::Float64,
                "None" => Ty::None,
                "Never" => Ty::Never,
                "IntLiteral" => Ty::IntLiteral,
                "FloatLiteral" => Ty::FloatLiteral,
                "Infer" => Ty::Infer,
                "DType" => Ty::Dtype,
                "Self" => Ty::SelfType,
                "Error" => Ty::Error,
                other => {
                    self.error(value.span, format!("unknown type `{other}`"));
                    return None;
                }
            }),
            ValueKind::Positional(tag, inner) => Some(match tag.as_str() {
                "overload" => Ty::Overload(self.types(inner)),
                "comptime_list" => Ty::ComptimeList(Box::new(self.ty(inner)?)),
                "tuple" => Ty::Tuple(self.types(inner)),
                "runtime_pack" => Ty::RuntimePack(self.types(inner)),
                "variadic_pack" => Ty::VariadicPack(Box::new(self.ty(inner)?)),
                "variant" => Ty::Variant(self.types(inner)),
                // A type-valued parameter expression; it folds back to the
                // type it denotes when the text spelled a closed one. A list
                // of types computed from packs is the spread a struct type's
                // one argument holds.
                "dependent_parameter" => {
                    let expr = self.param_expr(inner)?;
                    if *expr.meta() == MetaTy::type_list() {
                        return Some(Ty::Dependent(DependentType::Parameter(expr)));
                    }
                    if *expr.meta() != MetaTy::Type {
                        self.error(
                            value.span,
                            format!(
                                "a dependent type needs a Type expression, not `{}`",
                                expr.meta()
                            ),
                        );
                        return None;
                    }
                    DependentType::resolve(expr)
                }
                other => {
                    self.error(value.span, format!("unknown type `{other}`"));
                    return None;
                }
            }),
            ValueKind::Record(tag, fields) => Some(match tag.as_str() {
                "func" => self.callable_ty(value, fields, false)?,
                "generic_func" => self.callable_ty(value, fields, true)?,
                "param" => {
                    let name = self.req(value, fields, "name", Self::symbol)?;
                    let id = self.binder_id(value, fields, &name)?;
                    let bounds = self.req(value, fields, "bounds", |d, v| Some(d.strings(v)))?;
                    let callable_bound = self
                        .req(value, fields, "callable_bound", |d, v| {
                            Some(d.option_ty(Some(v)))
                        })?
                        .map(Box::new);
                    self.unknown(
                        fields,
                        &["owner", "slot", "name", "bounds", "callable_bound"],
                    );
                    Ty::Param {
                        binder: mojito_types::param_expr::ParamRef {
                            id,
                            name: name.into(),
                        },
                        bounds,
                        callable_bound,
                    }
                }
                "assoc" => {
                    let base = Box::new(self.req(value, fields, "base", Self::ty)?);
                    let name = self.req(value, fields, "member", Self::symbol)?;
                    let args = self.req(value, fields, "arguments", |d, v| Some(d.ty_args(v)))?;
                    self.unknown(fields, &["base", "member", "arguments"]);
                    Ty::Assoc { base, name, args }
                }
                // Schema 1.0's finite selection; 1.1 spells it as the
                // `dependent_parameter` of a `param_select`.
                "dependent_index" => {
                    self.require_legacy(value, tag)?;
                    let elements = self.req(value, fields, "elements", |d, v| Some(d.types(v)))?;
                    let index = self.req(value, fields, "index", Self::param_expr)?;
                    self.unknown(fields, &["elements", "index"]);
                    let selected = self.context.select(elements, &index);
                    DependentType::resolve(self.built(value, selected)?)
                }
                "struct_type" => {
                    let name = self.req(value, fields, "name", Self::symbol)?;
                    let args = self.req(value, fields, "arguments", |d, v| Some(d.ty_args(v)))?;
                    self.unknown(fields, &["name", "arguments"]);
                    Ty::Struct(name, args.into())
                }
                // Either slot may be a `ct_expr(...)` while symbolic; the
                // rebuild folds a closed one back to its constant.
                "simd" => {
                    let dtype = self.req(value, fields, "dtype", Self::simd_dtype_slot)?;
                    let width = self.req(value, fields, "width", Self::simd_width_slot)?;
                    self.unknown(fields, &["dtype", "width"]);
                    mojito_types::types::simd_ty_from_slots(dtype, width)
                        .map_err(|error| self.error(value.span, error.to_string()))
                        .ok()?
                }
                "pointer" => {
                    let element = Box::new(self.req(value, fields, "element", Self::ty)?);
                    let origin = self.req(value, fields, "origin", Self::pointer_origin)?;
                    self.unknown(fields, &["element", "origin"]);
                    Ty::Pointer { element, origin }
                }
                "ref" => Ty::Ref(self.ref_ty(value)?),
                other => {
                    self.error(value.span, format!("unknown type `{other}`"));
                    return None;
                }
            }),
            _ => {
                self.error(value.span, "expected type");
                None
            }
        }
    }

    pub(super) fn callable_ty(
        &mut self,
        value: &Value,
        fields: &[Field],
        generic: bool,
    ) -> Option<Ty> {
        let environment = self.req(value, fields, "environment", Self::environment)?;
        let decls = self.req(value, fields, "param_decls", |d, v| Some(d.param_decls(v)))?;
        let params = self.req(value, fields, "params", |d, v| Some(d.types(v)))?;
        let names = self.req(value, fields, "names", |d, v| Some(d.strings(v)))?;
        let ret = Box::new(self.req(value, fields, "return_type", Self::ty)?);
        let required = self.req(value, fields, "required", |d, v| Some(d.bools(v)))?;
        let variadic = self
            .req(value, fields, "variadic", |d, v| Some(d.option_ty(Some(v))))?
            .map(Box::new);
        let kw_variadic = self
            .req(value, fields, "kw_variadic", |d, v| {
                Some(d.option_ty(Some(v)))
            })?
            .map(Box::new);
        let positional_only = self.req(value, fields, "positional_only", |d, v| {
            Some(d.option_uint(v))
        })?;
        let keyword_only =
            self.req(value, fields, "keyword_only", |d, v| Some(d.option_uint(v)))?;
        let raises = self.req(value, fields, "raises", Self::boolean)?;
        let error = self
            .req(value, fields, "error_type", |d, v| {
                Some(d.option_ty(Some(v)))
            })?
            .map(Box::new);
        let conventions = self.req(value, fields, "conventions", |d, v| Some(d.conventions(v)))?;
        let ref_params =
            Box::new(self.req(value, fields, "ref_params", |d, v| Some(d.ref_sigs(v)))?);
        let ref_return = {
            let field = self.required(value, fields, "ref_return")?;
            self.option_value(Some(field))
                .and_then(|v| self.ref_sig(v))
                .map(Box::new)
        };
        let transfers = self.req(value, fields, "transfers", |d, v| Some(d.transfer_set(v)))?;
        self.unknown(
            fields,
            &[
                "environment",
                "param_decls",
                "params",
                "names",
                "return_type",
                "required",
                "variadic",
                "kw_variadic",
                "positional_only",
                "keyword_only",
                "raises",
                "error_type",
                "conventions",
                "ref_params",
                "ref_return",
                "transfers",
            ],
        );
        if generic {
            return Some(Ty::GenericFunc {
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
            });
        }
        if !decls.is_empty() {
            self.error(value.span, "`func` types take no param_decls");
            return None;
        }
        Some(Ty::Func {
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
        })
    }

    pub(super) fn transfer_set(&mut self, value: &Value) -> TransferSet {
        TransferSet(
            self.list(value)
                .map(|values| values.iter().filter_map(|v| self.transfer(v)).collect())
                .unwrap_or_default(),
        )
    }

    pub(super) fn transfer(&mut self, value: &Value) -> Option<TransferEffect> {
        let fields = self.record(value, "transfer").ok()?;
        let dest = self.req(value, fields, "dest", Self::sig_origin)?;
        let src = self.req(value, fields, "src", Self::sig_origin)?;
        let src_is_place = self.req(value, fields, "src_is_place", Self::boolean)?;
        let mutable = self.req(value, fields, "mutable", Self::boolean)?;
        self.unknown(fields, &["dest", "src", "src_is_place", "mutable"]);
        Some(TransferEffect {
            dest,
            src,
            src_is_place,
            mutable,
        })
    }

    pub(super) fn ty_args(&mut self, value: &Value) -> Vec<TyArg> {
        self.list(value)
            .map(|values| values.iter().filter_map(|v| self.ty_arg(v)).collect())
            .unwrap_or_default()
    }

    pub(super) fn ty_arg(&mut self, value: &Value) -> Option<TyArg> {
        let (tag, inner) = self.positional_value(value)?;
        match tag {
            "type_arg" => self.ty(inner).map(TyArg::Ty),
            "value_arg" => self.ct_value(inner).map(TyArg::Val),
            "origin_arg" => self.origin(inner).map(TyArg::Origin),
            other => {
                self.error(value.span, format!("unknown type argument `{other}`"));
                None
            }
        }
    }

    /// A parameter list. A `where` operand or a callable default older than
    /// schema 1.4 names its binder by spelling, which is the binder of that
    /// spelling in this list.
    pub(super) fn param_decls(&mut self, value: &Value) -> Vec<ParamDecl> {
        let decls: Vec<ParamDecl> = self
            .list(value)
            .map(|values| values.iter().filter_map(|v| self.param_decl(v)).collect())
            .unwrap_or_default();
        decls
            .iter()
            .map(|decl| decl.bind_references(&[&decls]))
            .collect()
    }

    pub(super) fn param_decl(&mut self, value: &Value) -> Option<ParamDecl> {
        let (tag, fields) = self.any_record(value)?;
        match tag {
            "type_param" => {
                let name = self.req(value, fields, "name", Self::symbol)?;
                let bounds = self.req(value, fields, "bounds", |d, v| Some(d.strings(v)))?;
                let callable_bound = self
                    .req(value, fields, "callable_bound", |d, v| {
                        Some(d.option_ty(Some(v)))
                    })?
                    .map(Box::new);
                let default = self
                    .req(value, fields, "default", |d, v| Some(d.option_ty(Some(v))))?
                    .map(Box::new);
                let infer_only = self.req(value, fields, "infer_only", Self::boolean)?;
                let variadic = self.req(value, fields, "variadic", Self::boolean)?;
                let constraints =
                    self.req(value, fields, "constraints", |d, v| Some(d.constraints(v)))?;
                self.unknown(
                    fields,
                    &[
                        "owner",
                        "slot",
                        "name",
                        "bounds",
                        "callable_bound",
                        "default",
                        "infer_only",
                        "variadic",
                        "constraints",
                    ],
                );
                Some(ParamDecl::Type {
                    id: self.binder_id(value, fields, &name)?,
                    name,
                    bounds,
                    callable_bound,
                    default,
                    infer_only,
                    variadic,
                    constraints,
                })
            }
            "value_param" => {
                let name = self.req(value, fields, "name", Self::symbol)?;
                let ty = Box::new(self.req(value, fields, "type", Self::ty)?);
                let default = {
                    let field = self.required(value, fields, "default")?;
                    self.option_value(Some(field))
                        .and_then(|v| self.param_expr(v))
                };
                let callable_default = {
                    let field = self.required(value, fields, "callable_default")?;
                    self.option_value(Some(field))
                        .and_then(|v| self.callable_default(v))
                };
                let infer_only = self.req(value, fields, "infer_only", Self::boolean)?;
                let variadic = self.req(value, fields, "variadic", Self::boolean)?;
                let constraints =
                    self.req(value, fields, "constraints", |d, v| Some(d.constraints(v)))?;
                self.unknown(
                    fields,
                    &[
                        "owner",
                        "slot",
                        "name",
                        "type",
                        "default",
                        "callable_default",
                        "infer_only",
                        "variadic",
                        "constraints",
                    ],
                );
                Some(ParamDecl::Value {
                    id: self.binder_id(value, fields, &name)?,
                    name,
                    ty,
                    default,
                    callable_default,
                    infer_only,
                    variadic,
                    constraints,
                })
            }
            other => {
                self.error(
                    value.span,
                    format!("unknown parameter declaration `{other}`"),
                );
                None
            }
        }
    }

    pub(super) fn constraints(&mut self, value: &Value) -> Vec<GenericConstraint> {
        self.list(value)
            .map(|values| values.iter().filter_map(|v| self.constraint(v)).collect())
            .unwrap_or_default()
    }

    pub(super) fn constraint(&mut self, value: &Value) -> Option<GenericConstraint> {
        match &value.kind {
            ValueKind::Positional(tag, inner) => match tag.as_str() {
                "not" => Some(GenericConstraint::Not(Box::new(self.constraint(inner)?))),
                "constraint_bool" => self.boolean(inner).map(GenericConstraint::Bool),
                other => {
                    self.error(value.span, format!("unknown constraint `{other}`"));
                    None
                }
            },
            ValueKind::Record(tag, fields) => match tag.as_str() {
                "with_message" => {
                    let condition =
                        Box::new(self.req(value, fields, "condition", Self::constraint)?);
                    let message = self.req(value, fields, "message", Self::string)?;
                    self.unknown(fields, &["condition", "message"]);
                    Some(GenericConstraint::WithMessage(condition, message))
                }
                "conforms" | "conforms_pack" => {
                    let param = self.req(value, fields, "param", Self::constraint_binder)?;
                    let trait_name = self.req(value, fields, "trait", Self::symbol)?;
                    self.unknown(fields, &["param", "trait"]);
                    Some(if tag == "conforms" {
                        GenericConstraint::Conforms { param, trait_name }
                    } else {
                        GenericConstraint::ConformsPack { param, trait_name }
                    })
                }
                "pack_predicate" => {
                    let param = self.req(value, fields, "param", Self::constraint_binder)?;
                    let predicate = self.req(value, fields, "predicate", Self::pack_predicate)?;
                    let all = self.req(value, fields, "all", Self::boolean)?;
                    self.unknown(fields, &["param", "predicate", "all"]);
                    Some(GenericConstraint::PackPredicate {
                        param,
                        predicate,
                        all,
                    })
                }
                "pack_contains" => {
                    let param = self.req(value, fields, "param", Self::constraint_binder)?;
                    let element = self.req(value, fields, "element", Self::constraint_operand)?;
                    self.unknown(fields, &["param", "element"]);
                    Some(GenericConstraint::PackContains { param, element })
                }
                "trivial" => {
                    let kind = self.req(value, fields, "lifecycle", Self::lifecycle)?;
                    let operand = self.req(value, fields, "operand", Self::constraint_operand)?;
                    self.unknown(fields, &["lifecycle", "operand"]);
                    Some(GenericConstraint::Trivial(kind, operand))
                }
                "eq" | "ne" | "lt" | "le" | "gt" | "ge" => {
                    let left = self.req(value, fields, "left", Self::constraint_operand)?;
                    let right = self.req(value, fields, "right", Self::constraint_operand)?;
                    self.unknown(fields, &["left", "right"]);
                    Some(match tag.as_str() {
                        "eq" => GenericConstraint::Eq(left, right),
                        "ne" => GenericConstraint::Ne(left, right),
                        "lt" => GenericConstraint::Lt(left, right),
                        "le" => GenericConstraint::Le(left, right),
                        "gt" => GenericConstraint::Gt(left, right),
                        _ => GenericConstraint::Ge(left, right),
                    })
                }
                "and" | "or" => {
                    let left = Box::new(self.req(value, fields, "left", Self::constraint)?);
                    let right = Box::new(self.req(value, fields, "right", Self::constraint)?);
                    self.unknown(fields, &["left", "right"]);
                    Some(if tag == "and" {
                        GenericConstraint::And(left, right)
                    } else {
                        GenericConstraint::Or(left, right)
                    })
                }
                other => {
                    self.error(value.span, format!("unknown constraint `{other}`"));
                    None
                }
            },
            _ => {
                self.error(value.span, "expected constraint");
                None
            }
        }
    }

    /// The binder a `where` operand or a callable default names: a `binder`
    /// record, or the bare spelling of an artifact older than schema 1.4.
    fn constraint_binder(&mut self, value: &Value) -> Option<ParamRef> {
        if matches!(&value.kind, ValueKind::Record(tag, _) if tag == "binder") {
            return self.binder_ref(value);
        }
        self.symbol(value).map(|name| ParamRef::unbound(&name))
    }

    pub(super) fn constraint_operand(&mut self, value: &Value) -> Option<ConstraintOperand> {
        let (tag, inner) = self.positional_value(value)?;
        match tag {
            "operand_param" => self.constraint_binder(inner).map(ConstraintOperand::Param),
            "operand_value" => self.ct_value(inner).map(ConstraintOperand::Value),
            "operand_type" => self.ty(inner).map(ConstraintOperand::Type),
            "operand_pack_length" => self
                .constraint_binder(inner)
                .map(ConstraintOperand::PackLength),
            "operand_expr" => self.param_expr(inner).map(ConstraintOperand::Expr),
            other => {
                self.error(value.span, format!("unknown constraint operand `{other}`"));
                None
            }
        }
    }

    pub(super) fn pack_predicate(&mut self, value: &Value) -> Option<PackPredicateRef> {
        let (tag, inner) = self.positional_value(value)?;
        match tag {
            "predicate_trivial" => self.lifecycle(inner).map(PackPredicateRef::Trivial),
            "predicate_alias" => self.symbol(inner).map(PackPredicateRef::Alias),
            other => {
                self.error(value.span, format!("unknown pack predicate `{other}`"));
                None
            }
        }
    }

    pub(super) fn lifecycle(&mut self, value: &Value) -> Option<TrivialLifecycle> {
        match self.atom(value)? {
            "movable" => Some(TrivialLifecycle::Movable),
            "copyable" => Some(TrivialLifecycle::Copyable),
            "deinitable" => Some(TrivialLifecycle::Deinitable),
            other => {
                self.error(value.span, format!("unknown lifecycle `{other}`"));
                None
            }
        }
    }

    pub(super) fn callable_default(&mut self, value: &Value) -> Option<CallableDefault> {
        match &value.kind {
            ValueKind::Positional(tag, inner) => match tag.as_str() {
                "default_symbol" => self.symbol(inner).map(CallableDefault::Symbol),
                "default_parameter" => self
                    .constraint_binder(inner)
                    .map(CallableDefault::Parameter),
                other => {
                    self.error(value.span, format!("unknown callable default `{other}`"));
                    None
                }
            },
            ValueKind::Record(tag, fields) if tag == "default_if" => {
                let condition = self.req(value, fields, "condition", Self::param_expr)?;
                let then_value =
                    Box::new(self.req(value, fields, "then_value", Self::callable_default)?);
                let else_value =
                    Box::new(self.req(value, fields, "else_value", Self::callable_default)?);
                self.unknown(fields, &["condition", "then_value", "else_value"]);
                Some(CallableDefault::If {
                    condition,
                    then_value,
                    else_value,
                })
            }
            _ => {
                self.error(value.span, "expected callable default");
                None
            }
        }
    }

    /// A parameter expression. Every form re-enters the canonicalizing
    /// constructors, so a parsed expression is canonical whatever order the
    /// text spelled it in; an operator re-enters through `rebuild`, which
    /// keeps an opaque atom over constants (`4 // 2` in an instance's type)
    /// unfolded, as replacement left it. Schema 1.0's name-only tree is accepted in a 1.0
    /// artifact and translated through the artifact's declared binders.
    /// A vector's lane dtype slot: a dtype name, or `ct_expr(...)` while it
    /// is a parameter expression.
    pub(super) fn simd_dtype_slot(
        &mut self,
        value: &Value,
    ) -> Option<mojito_types::types::SimdDtype> {
        match &value.kind {
            ValueKind::Positional(tag, inner) if tag == "ct_expr" => self
                .param_expr(inner)
                .map(mojito_types::types::SimdDtype::Expr),
            _ => self.dtype(value).map(mojito_types::types::SimdDtype::Known),
        }
    }

    /// A vector's width slot: a lane count, or `ct_expr(...)` while symbolic.
    pub(super) fn simd_width_slot(
        &mut self,
        value: &Value,
    ) -> Option<mojito_types::types::SimdWidth> {
        match &value.kind {
            ValueKind::Positional(tag, inner) if tag == "ct_expr" => self
                .param_expr(inner)
                .map(mojito_types::types::SimdWidth::Expr),
            _ => self.int64(value).map(mojito_types::types::SimdWidth::Known),
        }
    }

    /// A `Variant` alternative: its position, or a template's `ct_expr`.
    pub(super) fn variant_index(
        &mut self,
        value: &Value,
    ) -> Option<mojito_types::types::VariantIndex> {
        match &value.kind {
            ValueKind::Positional(tag, inner) if tag == "ct_expr" => self
                .param_expr(inner)
                .map(mojito_types::types::VariantIndex::Expr),
            _ => self
                .uint(value)
                .map(mojito_types::types::VariantIndex::Known),
        }
    }

    /// A lane gather's mask: a known index list, or a template's
    /// `lane_shuffle`/`lane_slice`/`lane_join` form.
    pub(super) fn lane_mask(&mut self, value: &Value) -> Option<mojito_types::types::LaneMask> {
        use mojito_types::types::LaneMask;
        match &value.kind {
            ValueKind::Record(tag, fields) => match tag.as_str() {
                "lane_shuffle" => self
                    .req(value, fields, "lanes", |d, v| {
                        d.list(v)
                            .ok()?
                            .iter()
                            .map(|lane| d.param_expr(lane))
                            .collect::<Option<Vec<_>>>()
                    })
                    .map(LaneMask::Shuffle),
                "lane_slice" => Some(LaneMask::Slice {
                    start: self.req(value, fields, "offset", Self::param_expr)?,
                    count: self.req(value, fields, "width", Self::simd_width_slot)?,
                }),
                "lane_join" => Some(LaneMask::Join),
                other => {
                    self.error(value.span, format!("unknown lane mask `{other}`"));
                    None
                }
            },
            _ => self
                .list(value)
                .ok()?
                .iter()
                .map(|lane| self.uint(lane))
                .collect::<Option<Vec<_>>>()
                .map(LaneMask::Known),
        }
    }

    pub(super) fn param_expr(&mut self, value: &Value) -> Option<ParamExpr> {
        match &value.kind {
            ValueKind::Positional(tag, inner) => match tag.as_str() {
                "param_constant" => {
                    let constant = self.ct_value(inner)?;
                    let built = self.context.constant(constant);
                    self.built(value, built)
                }
                "param_type_shape" => self.ty(inner).map(|ty| self.context.type_shape(ty)),
                "ct_value" => {
                    self.require_legacy(value, tag)?;
                    let constant = self.ct_value(inner)?;
                    let built = self.context.constant(constant);
                    self.built(value, built)
                }
                "ct_param" => {
                    self.require_legacy(value, tag)?;
                    let name = self.symbol(inner)?;
                    match self.legacy_reference(value, &name)? {
                        LegacyBinder::Typed(reference) => Some(reference),
                        LegacyBinder::Undeclared => {
                            self.error(
                                value.span,
                                format!("compile-time parameter `{name}` has no declaration"),
                            );
                            None
                        }
                    }
                }
                "ct_neg" => {
                    self.require_legacy(value, tag)?;
                    let operand = self.param_expr(inner)?;
                    let built = self.context.neg(&operand);
                    self.built(value, built)
                }
                other => {
                    self.error(
                        value.span,
                        format!("unknown compile-time expression `{other}`"),
                    );
                    None
                }
            },
            ValueKind::Record(tag, fields) => match tag.as_str() {
                "param_decl_ref" => {
                    let owner = self.req(value, fields, "owner", Self::string)?;
                    let slot = self.req(value, fields, "slot", Self::uint)?;
                    let name = self.req(value, fields, "name", Self::symbol)?;
                    let meta = self.req(value, fields, "type", Self::meta_ty)?;
                    self.unknown(fields, &["owner", "slot", "name", "type"]);
                    Some(
                        self.context
                            .decl_ref(ParamId::new(&owner, slot), &name, meta),
                    )
                }
                "param_index_ref" => {
                    let depth = self.req(value, fields, "depth", Self::uint32)?;
                    let index = self.req(value, fields, "index", Self::uint32)?;
                    let meta = self.req(value, fields, "type", Self::meta_ty)?;
                    self.unknown(fields, &["depth", "index", "type"]);
                    Some(self.context.index_ref(depth, index, meta))
                }
                "param_expr" => {
                    let op_value = self.required(value, fields, "op")?;
                    let op_name = self.atom(op_value)?.to_string();
                    let Some(op) = ParamOp::from_name(&op_name) else {
                        self.error(
                            op_value.span,
                            format!("unknown parameter operator `{op_name}`"),
                        );
                        return None;
                    };
                    let meta = self.req(value, fields, "type", Self::meta_ty)?;
                    let operands_value = self.required(value, fields, "operands")?;
                    let operands: Vec<ParamExpr> = self
                        .list(operands_value)
                        .ok()?
                        .iter()
                        .map(|operand| self.param_expr(operand))
                        .collect::<Option<_>>()?;
                    self.unknown(fields, &["op", "type", "operands"]);
                    let built = self.context.rebuild(op, &operands);
                    let built = self.built(value, built)?;
                    if *built.meta() != meta {
                        self.error(
                            value.span,
                            format!(
                                "parameter expression has type `{}`, not the recorded `{meta}`",
                                built.meta()
                            ),
                        );
                        return None;
                    }
                    Some(built)
                }
                "param_identical" => {
                    let left = self.req(value, fields, "left", Self::param_expr)?;
                    let right = self.req(value, fields, "right", Self::param_expr)?;
                    self.unknown(fields, &["left", "right"]);
                    Some(self.context.identical(&left, &right))
                }
                "param_conforms" => {
                    let subject = self.req(value, fields, "subject", Self::param_expr)?;
                    let trait_name = self.req(value, fields, "trait", Self::symbol)?;
                    self.unknown(fields, &["subject", "trait"]);
                    let built = self.context.conforms(&subject, &trait_name);
                    self.built(value, built)
                }
                "param_trivial" => {
                    let lifecycle = self.req(value, fields, "lifecycle", Self::lifecycle)?;
                    let subject = self.req(value, fields, "subject", Self::param_expr)?;
                    self.unknown(fields, &["lifecycle", "subject"]);
                    let built = self.context.trivial(lifecycle, &subject);
                    self.built(value, built)
                }
                "param_select" => {
                    let elements = self.req(value, fields, "elements", |d, v| Some(d.types(v)))?;
                    let index = self.req(value, fields, "index", Self::param_expr)?;
                    self.unknown(fields, &["elements", "index"]);
                    let built = self.context.select(elements, &index);
                    self.built(value, built)
                }
                "param_pack_query" => {
                    let pack = self.req(value, fields, "pack", Self::pack_binder)?;
                    let query = self.req(value, fields, "query", Self::pack_query)?;
                    self.unknown(fields, &["pack", "query"]);
                    Some(self.context.pack_query(&pack, query))
                }
                "param_list_get" => {
                    let list = self.req(value, fields, "list", Self::param_expr)?;
                    let index = self.req(value, fields, "index", Self::param_expr)?;
                    self.unknown(fields, &["list", "index"]);
                    let built = self.context.list_get(&list, &index);
                    self.built(value, built)
                }
                "param_list_tabulate" => {
                    let count = self.req(value, fields, "count", Self::param_expr)?;
                    let element = self.req(value, fields, "element", Self::param_expr)?;
                    self.unknown(fields, &["count", "element"]);
                    let built = self.context.list_tabulate(&count, &element);
                    self.built(value, built)
                }
                "param_list_concat" => {
                    let lists_value = self.required(value, fields, "lists")?;
                    let lists: Vec<ParamExpr> = self
                        .list(lists_value)
                        .ok()?
                        .iter()
                        .map(|list| self.param_expr(list))
                        .collect::<Option<_>>()?;
                    self.unknown(fields, &["lists"]);
                    let built = self.context.list_concat(&lists);
                    self.built(value, built)
                }
                "param_field" => {
                    let base = self.req(value, fields, "base", Self::param_expr)?;
                    let name = self.req(value, fields, "name", Self::symbol)?;
                    let meta = self.req(value, fields, "type", Self::meta_ty)?;
                    self.unknown(fields, &["base", "name", "type"]);
                    let built = self.context.field(&base, &name, meta);
                    self.built(value, built)
                }
                "param_reflect" => {
                    let subject = self.req(value, fields, "subject", Self::param_expr)?;
                    let query_value = self.required(value, fields, "query")?;
                    let spelling = self.symbol(query_value)?;
                    let Some(query) =
                        mojito_types::param_expr::ReflectQuery::from_spelling(&spelling)
                    else {
                        self.error(
                            query_value.span,
                            format!("unknown reflection query `{spelling}`"),
                        );
                        return None;
                    };
                    self.unknown(fields, &["subject", "query"]);
                    Some(self.context.reflect_query(&subject, query))
                }
                "param_type_member" => {
                    let subject = self.req(value, fields, "subject", Self::param_expr)?;
                    let name = self.req(value, fields, "name", Self::symbol)?;
                    let meta = self.req(value, fields, "type", Self::meta_ty)?;
                    self.unknown(fields, &["subject", "name", "type"]);
                    Some(self.context.type_member(&subject, &name, meta))
                }
                "param_apply" => {
                    let function = self.req(value, fields, "function", Self::string)?;
                    let meta = self.req(value, fields, "type", Self::meta_ty)?;
                    let args_value = self.required(value, fields, "args")?;
                    let args: Vec<ParamExpr> = self
                        .list(args_value)
                        .ok()?
                        .iter()
                        .map(|arg| self.param_expr(arg))
                        .collect::<Option<_>>()?;
                    // Schema 1.37 and earlier wrote the value a compile-time
                    // route had established for the application; the
                    // application's identity never carried it.
                    if let Ok(evaluated) = self.field(fields, "evaluated")
                        && let Some(inner) = self.option_value(Some(evaluated))
                    {
                        self.ct_value(inner)?;
                    }
                    self.unknown(fields, &["function", "type", "args", "evaluated"]);
                    Some(self.context.apply(&function, &args, meta))
                }
                // A hole is a boundary error at MIR, never a parsed form.
                "param_hole" => {
                    self.error(
                        value.span,
                        "an unknown or unbound parameter cannot cross MIR",
                    );
                    None
                }
                legacy
                @ ("ct_add" | "ct_sub" | "ct_mul" | "ct_floor_div" | "ct_mod" | "ct_pow") => {
                    self.require_legacy(value, legacy)?;
                    let op = match legacy {
                        "ct_add" => InfixOp::Add,
                        "ct_sub" => InfixOp::Sub,
                        "ct_mul" => InfixOp::Mul,
                        "ct_floor_div" => InfixOp::FloorDiv,
                        "ct_mod" => InfixOp::Mod,
                        _ => InfixOp::Pow,
                    };
                    let left = self.req(value, fields, "left", Self::param_expr)?;
                    let right = self.req(value, fields, "right", Self::param_expr)?;
                    self.unknown(fields, &["left", "right"]);
                    let built = self.context.infix(op, &left, &right);
                    self.built(value, built)
                }
                other => {
                    self.error(
                        value.span,
                        format!("unknown compile-time expression `{other}`"),
                    );
                    None
                }
            },
            _ => {
                self.error(value.span, "expected compile-time expression");
                None
            }
        }
    }

    /// The pack a pack query names: a `binder` record, or the bare spelling
    /// of an artifact older than schema 1.6, which reads as an unbound
    /// reference of that spelling.
    fn pack_binder(&mut self, value: &Value) -> Option<ParamRef> {
        if matches!(&value.kind, ValueKind::Record(tag, _) if tag == "binder") {
            return self.binder_ref(value);
        }
        self.symbol(value).map(|name| ParamRef::unbound(&name))
    }

    fn pack_query(&mut self, value: &Value) -> Option<PackQuery> {
        match &value.kind {
            ValueKind::Atom(atom) if atom == "pack_length" => Some(PackQuery::Length),
            ValueKind::Positional(tag, inner) => match tag.as_str() {
                "pack_conforms" => self.symbol(inner).map(PackQuery::Conforms),
                "pack_contains" => self.param_expr(inner).map(PackQuery::Contains),
                "pack_index_of" => self.param_expr(inner).map(PackQuery::IndexOf),
                other => {
                    self.error(value.span, format!("unknown pack query `{other}`"));
                    None
                }
            },
            ValueKind::Record(tag, fields) if tag == "pack_predicate" => {
                let predicate = self.req(value, fields, "predicate", Self::pack_predicate)?;
                let all = self.req(value, fields, "all", Self::boolean)?;
                self.unknown(fields, &["predicate", "all"]);
                Some(PackQuery::Predicate { predicate, all })
            }
            _ => {
                self.error(value.span, "expected pack query");
                None
            }
        }
    }

    fn meta_ty(&mut self, value: &Value) -> Option<MetaTy> {
        match &value.kind {
            ValueKind::Atom(atom) if atom == "meta_type" => Some(MetaTy::Type),
            ValueKind::Atom(atom) if atom == "meta_reflected" => Some(MetaTy::ReflectedType),
            ValueKind::Positional(tag, inner) => match tag.as_str() {
                "meta_value" => self.ty(inner).map(MetaTy::value),
                "meta_tuple" => self.meta_tys(inner).map(MetaTy::Tuple),
                "meta_list" => self.meta_tys(inner).map(MetaTy::List),
                "meta_set" => self.meta_tys(inner).map(MetaTy::Set),
                "meta_param_list" => self
                    .meta_ty(inner)
                    .map(|element| MetaTy::ParamList(Box::new(element))),
                "meta_dict" => self
                    .list(inner)
                    .ok()?
                    .iter()
                    .map(|entry| {
                        let fields = self.record(entry, "meta_entry").ok()?;
                        let key = self.req(entry, fields, "key", Self::meta_ty)?;
                        let value = self.req(entry, fields, "value", Self::meta_ty)?;
                        self.unknown(fields, &["key", "value"]);
                        Some((key, value))
                    })
                    .collect::<Option<_>>()
                    .map(MetaTy::Dict),
                other => {
                    self.error(value.span, format!("unknown meta-type `{other}`"));
                    None
                }
            },
            _ => {
                self.error(value.span, "expected meta-type");
                None
            }
        }
    }

    fn meta_tys(&mut self, value: &Value) -> Option<Vec<MetaTy>> {
        self.list(value)
            .ok()?
            .iter()
            .map(|meta| self.meta_ty(meta))
            .collect()
    }

    /// Report a constructor's rejection (bad arity, operand domain, budget)
    /// at the text that spelled the expression.
    fn built(
        &mut self,
        value: &Value,
        built: Result<ParamExpr, mojito_types::param_expr::ParamError>,
    ) -> Option<ParamExpr> {
        built
            .map_err(|error| self.error(value.span, error.to_string()))
            .ok()
    }

    /// A schema 1.0 form is an error in a 1.1 artifact.
    fn require_legacy(&mut self, value: &Value, tag: &str) -> Option<()> {
        if self.legacy_binders.is_some() {
            return Some(());
        }
        self.error(
            value.span,
            format!("`{tag}` is schema 1.0 syntax; schema 1.1 spells a typed `param_*` form"),
        );
        None
    }

    /// What a 1.0 name denotes, or `None` (with a diagnostic) when the
    /// artifact declares several value parameters of that name with different
    /// types, since a 1.0 reference carries nothing that chooses between them.
    fn legacy_reference(&mut self, value: &Value, name: &str) -> Option<LegacyBinder> {
        let metas = self
            .legacy_binders
            .as_ref()
            .and_then(|binders| binders.get(name.trim_start_matches('*')))
            .cloned()
            .unwrap_or_default();
        match metas.as_slice() {
            [] => Some(LegacyBinder::Undeclared),
            [meta] => Some(LegacyBinder::Typed(self.context.decl_ref(
                ParamId::new(&format!("$mir-1.0:{name}"), 0),
                name,
                meta.clone(),
            ))),
            _ => {
                self.error(
                    value.span,
                    format!(
                        "schema 1.0 parameter `{name}` is declared with more than one type;                          re-emit the artifact as schema 1.1"
                    ),
                );
                None
            }
        }
    }

    /// Every value parameter an artifact declares, by name, decoded on a
    /// scratch decoder so the real pass reports each diagnostic once.
    pub(super) fn legacy_value_binders(artifact: &Value) -> HashMap<String, Vec<MetaTy>> {
        fn walk(value: &Value, scratch: &mut Decoder, out: &mut HashMap<String, Vec<MetaTy>>) {
            match &value.kind {
                ValueKind::Record(tag, fields) => {
                    if tag == "value_param"
                        && let (Ok(name), Ok(ty)) =
                            (scratch.field(fields, "name"), scratch.field(fields, "type"))
                        && let (Some(name), Some(ty)) = (scratch.symbol(name), scratch.ty(ty))
                        && !matches!(ty, Ty::Func { .. } | Ty::GenericFunc { .. })
                    {
                        let metas = out
                            .entry(name.trim_start_matches('*').to_string())
                            .or_default();
                        let meta = MetaTy::value(ty);
                        if !metas.contains(&meta) {
                            metas.push(meta);
                        }
                    }
                    for field in fields {
                        walk(&field.value, scratch, out);
                    }
                }
                ValueKind::List(values) => {
                    for value in values {
                        walk(value, scratch, out);
                    }
                }
                ValueKind::Positional(_, inner) => walk(inner, scratch, out),
                ValueKind::Atom(_) | ValueKind::String(_) => {}
            }
        }
        let mut out = HashMap::new();
        // The scratch decoder reads no 1.0 reference itself: a parameter's
        // declared type never names another parameter by a bare `ct_param`.
        let mut scratch = Self::new(false);
        walk(artifact, &mut scratch, &mut out);
        out
    }

    pub(super) fn ct_values(&mut self, value: &Value) -> Vec<CtValue> {
        self.list(value)
            .map(|values| values.iter().filter_map(|v| self.ct_value(v)).collect())
            .unwrap_or_default()
    }

    /// A deferred slot: the `binder` record of the parameter whose slot it
    /// fills. An artifact older than schema 1.5 spells it as a bare symbol,
    /// which reads as an unbound reference of that spelling, or as the
    /// elaborator marker the symbol spells.
    fn deferred_slot(&mut self, value: &Value) -> Option<CtValue> {
        if matches!(&value.kind, ValueKind::Record(tag, _) if tag == "binder") {
            return self.binder_ref(value).map(CtValue::Deferred);
        }
        let name = self.symbol(value)?;
        Some(legacy_marker(&name).map_or_else(
            || CtValue::Deferred(ParamRef::unbound(&name)),
            CtValue::Marker,
        ))
    }

    fn ct_marker(&mut self, value: &Value) -> Option<CtMarker> {
        match &value.kind {
            ValueKind::Atom(tag) if tag == "marker_local" => Some(CtMarker::RuntimeLocal),
            ValueKind::Atom(tag) if tag == "marker_type" => Some(CtMarker::TypeName),
            ValueKind::Record(tag, fields) if tag == "marker_tuple_origin" => {
                let id = self.req(value, fields, "id", Self::uint32)?;
                let mutability = self.req(value, fields, "mutability", Self::mutability)?;
                self.unknown(fields, &["id", "mutability"]);
                Some(CtMarker::TupleOrigin {
                    id: OriginParamId(id),
                    mutability,
                })
            }
            ValueKind::Atom(tag) if tag == "marker_applied" => Some(CtMarker::Applied),
            // Schema 1.37 and earlier carried the folded value of an applied
            // constant, and spelled a layout constant apart.
            ValueKind::Positional(tag, inner) if tag == "marker_applied" => {
                self.int64(inner).map(|_| CtMarker::Applied)
            }
            ValueKind::Positional(tag, inner) if tag == "marker_layout" => {
                self.ty(inner).map(|_| CtMarker::Applied)
            }
            _ => {
                self.error(value.span, "expected compile-time marker");
                None
            }
        }
    }

    pub(super) fn ct_value(&mut self, value: &Value) -> Option<CtValue> {
        match &value.kind {
            ValueKind::Positional(tag, inner) => match tag.as_str() {
                "ct_int" => self.int64(inner).map(CtValue::Int),
                "ct_uint" => self.uint64(inner).map(CtValue::UInt),
                "ct_float_bits" => self.float_bits(inner).map(CtValue::Float),
                "ct_int_literal" => self.int_literal(inner).map(CtValue::IntLiteral),
                "ct_float_literal" => self.float_literal(inner).map(CtValue::FloatLiteral),
                "ct_bool" => self.boolean(inner).map(CtValue::Bool),
                "ct_string" => self.string(inner).map(CtValue::Str),
                "ct_tuple" => Some(CtValue::Tuple(self.ct_values(inner))),
                "ct_list" => Some(CtValue::List(self.ct_values(inner))),
                "ct_dtype" => self.dtype(inner).map(CtValue::Dtype),
                "ct_type" => self.ty(inner).map(|ty| CtValue::Type(Box::new(ty))),
                "ct_reflected" => self.ty(inner).map(|ty| CtValue::Reflected(Box::new(ty))),
                "ct_expr" => self.param_expr(inner).map(ParamExpr::into_value),
                "ct_deferred" => self.deferred_slot(inner),
                "ct_marker" => self.ct_marker(inner).map(CtValue::Marker),
                // Schema 1.0 spelled both a parameter reference and a
                // deferred callable-value slot this way; a declared scalar
                // value parameter is the reference.
                "ct_param" => {
                    self.require_legacy(value, tag)?;
                    let name = self.symbol(inner)?;
                    self.legacy_reference(value, &name)
                        .map(|binder| match binder {
                            LegacyBinder::Typed(reference) => reference.into_value(),
                            LegacyBinder::Undeclared => CtValue::Deferred(ParamRef::unbound(&name)),
                        })
                }
                other => {
                    self.error(value.span, format!("unknown compile-time value `{other}`"));
                    None
                }
            },
            ValueKind::Record(tag, fields) if tag == "ct_simd" => {
                let dtype_value = self.required(value, fields, "dtype")?;
                let dtype = self.dtype(dtype_value)?;
                let lanes_value = self.required(value, fields, "lanes")?;
                let mut lanes = Vec::new();
                if let Ok(values) = self.list(lanes_value) {
                    for lane in values {
                        let decoded = match &lane.kind {
                            ValueKind::Positional(tag, inner) => match tag.as_str() {
                                "lane_int" => self
                                    .atom(inner)
                                    .and_then(|text| text.parse::<i128>().ok())
                                    .map(mojito_types::ct::CtLane::Int),
                                "lane_float_bits" => {
                                    self.float_bits(inner).map(mojito_types::ct::CtLane::Float)
                                }
                                "lane_bool" => {
                                    self.boolean(inner).map(mojito_types::ct::CtLane::Bool)
                                }
                                _ => None,
                            },
                            _ => None,
                        };
                        match decoded {
                            Some(decoded) => lanes.push(decoded),
                            None => self.error(lane.span, "expected a SIMD lane value"),
                        }
                    }
                }
                self.unknown(fields, &["dtype", "lanes"]);
                Some(CtValue::Simd { dtype, lanes })
            }
            ValueKind::Record(tag, fields) if tag == "ct_struct" => {
                let name = self.req(value, fields, "name", Self::symbol)?;
                let fields_value = self.required(value, fields, "fields")?;
                let mut entries = Vec::new();
                if let Ok(values) = self.list(fields_value) {
                    for entry in values {
                        let Ok(entry_fields) = self.record(entry, "ct_field") else {
                            continue;
                        };
                        let field_name = self
                            .required(entry, entry_fields, "name")
                            .and_then(|v| self.symbol(v));
                        let field_value = self
                            .required(entry, entry_fields, "value")
                            .and_then(|v| self.ct_value(v));
                        self.unknown(entry_fields, &["name", "value"]);
                        if let (Some(field_name), Some(field_value)) = (field_name, field_value) {
                            entries.push((field_name, field_value));
                        }
                    }
                }
                self.unknown(fields, &["name", "fields"]);
                Some(CtValue::Struct {
                    name,
                    fields: entries,
                })
            }
            ValueKind::Record(tag, fields) if tag == "ct_dict" => {
                let spelling = match self.field(fields, "spelling") {
                    Ok(spelling) => Some(self.ty(spelling)?),
                    Err(()) => None,
                };
                let entries_value = self.required(value, fields, "entries")?;
                let mut entries = Vec::new();
                if let Ok(values) = self.list(entries_value) {
                    for entry in values {
                        let Ok(entry_fields) = self.record(entry, "ct_entry") else {
                            continue;
                        };
                        let key = self
                            .required(entry, entry_fields, "key")
                            .and_then(|v| self.ct_value(v));
                        let element = self
                            .required(entry, entry_fields, "value")
                            .and_then(|v| self.ct_value(v));
                        self.unknown(entry_fields, &["key", "value"]);
                        if let (Some(key), Some(element)) = (key, element) {
                            entries.push((key, element));
                        }
                    }
                }
                self.unknown(fields, &["spelling", "entries"]);
                Some(CtValue::Dict {
                    spelling: spelling.map(Box::new),
                    entries,
                })
            }
            ValueKind::Record(tag, fields) if tag == "ct_set" => {
                let spelling = match self.field(fields, "spelling") {
                    Ok(spelling) => Some(self.ty(spelling)?),
                    Err(()) => None,
                };
                let elements_value = self.required(value, fields, "elements")?;
                let elements = self.ct_values(elements_value);
                self.unknown(fields, &["spelling", "elements"]);
                Some(CtValue::Set {
                    spelling: spelling.map(Box::new),
                    elements,
                })
            }
            _ => {
                self.error(value.span, "expected compile-time value");
                None
            }
        }
    }
}

/// What a schema 1.0 parameter name resolves to.
enum LegacyBinder {
    /// The one value parameter the artifact declares under that name.
    Typed(ParamExpr),
    /// No value parameter of that name: a deferred slot in value position,
    /// an error in expression position.
    Undeclared,
}

/// The elaborator marker a deferred slot older than schema 1.5 spells.
fn legacy_marker(name: &str) -> Option<CtMarker> {
    match name {
        "$local" => return Some(CtMarker::RuntimeLocal),
        "$type" => return Some(CtMarker::TypeName),
        _ => {}
    }
    let (index, permission) = name.strip_prefix("$tuple-origin:")?.split_once(':')?;
    let id = OriginParamId(index.parse().ok()?);
    let mutability = match permission {
        "imm" => Mutability::Immutable,
        "mut" => Mutability::Mutable,
        "param" => Mutability::Param(id),
        _ => return None,
    };
    Some(CtMarker::TupleOrigin { id, mutability })
}
