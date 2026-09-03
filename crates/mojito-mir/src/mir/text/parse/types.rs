//! Type decoding: `Ty`, callable types, parameter declarations,
//! constraints, and comptime expressions/values.

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
                    let bounds = self.req(value, fields, "bounds", |d, v| Some(d.strings(v)))?;
                    let callable_bound = self
                        .req(value, fields, "callable_bound", |d, v| {
                            Some(d.option_ty(Some(v)))
                        })?
                        .map(Box::new);
                    self.unknown(fields, &["name", "bounds", "callable_bound"]);
                    Ty::Param {
                        name,
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
                "dependent_index" => {
                    let elements = self.req(value, fields, "elements", |d, v| Some(d.types(v)))?;
                    let index = self.req(value, fields, "index", Self::ct_expr)?;
                    self.unknown(fields, &["elements", "index"]);
                    Ty::Dependent(DependentType::Indexed { elements, index })
                }
                "struct_type" => {
                    let name = self.req(value, fields, "name", Self::symbol)?;
                    let args = self.req(value, fields, "arguments", |d, v| Some(d.ty_args(v)))?;
                    self.unknown(fields, &["name", "arguments"]);
                    Ty::Struct(name, args)
                }
                "simd" => {
                    let dtype = self.req(value, fields, "dtype", Self::dtype)?;
                    let width = self.req(value, fields, "width", Self::int64)?;
                    self.unknown(fields, &["dtype", "width"]);
                    Ty::Simd { dtype, width }
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

    pub(super) fn param_decls(&mut self, value: &Value) -> Vec<ParamDecl> {
        self.list(value)
            .map(|values| values.iter().filter_map(|v| self.param_decl(v)).collect())
            .unwrap_or_default()
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
                    self.option_value(Some(field)).and_then(|v| self.ct_expr(v))
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
                    let param = self.req(value, fields, "param", Self::symbol)?;
                    let trait_name = self.req(value, fields, "trait", Self::symbol)?;
                    self.unknown(fields, &["param", "trait"]);
                    Some(if tag == "conforms" {
                        GenericConstraint::Conforms { param, trait_name }
                    } else {
                        GenericConstraint::ConformsPack { param, trait_name }
                    })
                }
                "pack_predicate" => {
                    let param = self.req(value, fields, "param", Self::symbol)?;
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
                    let param = self.req(value, fields, "param", Self::symbol)?;
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

    pub(super) fn constraint_operand(&mut self, value: &Value) -> Option<ConstraintOperand> {
        let (tag, inner) = self.positional_value(value)?;
        match tag {
            "operand_param" => self.symbol(inner).map(ConstraintOperand::Param),
            "operand_value" => self.ct_value(inner).map(ConstraintOperand::Value),
            "operand_type" => self.ty(inner).map(ConstraintOperand::Type),
            "operand_pack_length" => self.symbol(inner).map(ConstraintOperand::PackLength),
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
                "default_parameter" => self.symbol(inner).map(CallableDefault::Parameter),
                other => {
                    self.error(value.span, format!("unknown callable default `{other}`"));
                    None
                }
            },
            ValueKind::Record(tag, fields) if tag == "default_if" => {
                let condition = self.req(value, fields, "condition", Self::ct_expr)?;
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

    pub(super) fn ct_expr(&mut self, value: &Value) -> Option<CtExpr> {
        match &value.kind {
            ValueKind::Positional(tag, inner) => match tag.as_str() {
                "ct_value" => self.ct_value(inner).map(CtExpr::Value),
                "ct_param" => self.symbol(inner).map(CtExpr::Param),
                "ct_neg" => Some(CtExpr::Neg(Box::new(self.ct_expr(inner)?))),
                other => {
                    self.error(
                        value.span,
                        format!("unknown compile-time expression `{other}`"),
                    );
                    None
                }
            },
            ValueKind::Record(tag, fields) => {
                let make = match tag.as_str() {
                    "ct_add" => CtExpr::Add,
                    "ct_sub" => CtExpr::Sub,
                    "ct_mul" => CtExpr::Mul,
                    "ct_floor_div" => CtExpr::FloorDiv,
                    "ct_mod" => CtExpr::Mod,
                    "ct_pow" => CtExpr::Pow,
                    other => {
                        self.error(
                            value.span,
                            format!("unknown compile-time expression `{other}`"),
                        );
                        return None;
                    }
                };
                let left = Box::new(self.req(value, fields, "left", Self::ct_expr)?);
                let right = Box::new(self.req(value, fields, "right", Self::ct_expr)?);
                self.unknown(fields, &["left", "right"]);
                Some(make(left, right))
            }
            _ => {
                self.error(value.span, "expected compile-time expression");
                None
            }
        }
    }

    pub(super) fn ct_values(&mut self, value: &Value) -> Vec<CtValue> {
        self.list(value)
            .map(|values| values.iter().filter_map(|v| self.ct_value(v)).collect())
            .unwrap_or_default()
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
                "ct_param" => self.symbol(inner).map(CtValue::Param),
                other => {
                    self.error(value.span, format!("unknown compile-time value `{other}`"));
                    None
                }
            },
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
            _ => {
                self.error(value.span, "expected compile-time value");
                None
            }
        }
    }
}
