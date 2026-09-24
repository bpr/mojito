//! Reflection under the checker: a `reflect[T]` query is answered from the
//! struct table when its subject is a registered struct and as a
//! `ParamKind::Reflect` node when the subject is still a parameter, so a body
//! reading a reflection handle validates with its parameters symbolic. The
//! elaborator evaluates the same queries per instance (`comptime/eval.rs`).
//! See `docs/symbol-map.md`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_ast::ast::ParamArg;
use mojito_types::param_expr::ReflectQuery;
use mojito_types::types::DependentType;

impl Checker {
    /// The type of a reflection read in a value position: a count, an index,
    /// or an `is_struct()` answer, an element of `field_names()`, the length
    /// of either list, or a construction of a field type (`FT()`,
    /// `types[i]()`). `None` when `expr` reads no reflection handle. A bare
    /// handle or list is no runtime value, so its `comptime` binding is kept
    /// for inlining at its compile-time uses.
    pub(super) fn infer_reflection(&self, expr: &Expr) -> Result<Option<Ty>, TypeError> {
        match &expr.kind {
            ExprKind::Identifier(name) => match self.local_comptime_value(name) {
                Some(bound) => self.infer_reflection(bound),
                None => Ok(None),
            },
            ExprKind::MethodCall { .. } | ExprKind::Invoke { .. } => {
                match self.reflection_query_of(expr)? {
                    Some((subject, query)) => self.reflection_value_ty(&subject, &query).map(Some),
                    None => Ok(None),
                }
            }
            ExprKind::Index { object, index } => match self.reflection_list(object)? {
                Some((ReflectQuery::FieldNames, _)) => {
                    self.reflection_index(index)?;
                    Ok(Some(Ty::StringLiteral))
                }
                Some(_) => Err(TypeError::NotComptime(
                    "a reflected field type is not a runtime value; construct it, or use it in \
                     a type position"
                        .to_string(),
                )),
                None => Ok(None),
            },
            ExprKind::Member { object, field } if field == "length" => {
                Ok(self.reflection_list(object)?.map(|_| Ty::Int))
            }
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } if name == "len" && param_args.is_empty() && args.len() == 1 && kwargs.is_empty() => {
                Ok(self.reflection_list(&args[0])?.map(|_| Ty::Int))
            }
            // `types[i]()`: a construction of the field type at `i`.
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } if args.is_empty()
                && kwargs.is_empty()
                && let [ParamArg::Value(index)] = param_args.as_slice()
                && let Some(bound) = self.local_comptime_value(name)
                && let Some((ReflectQuery::FieldTypes, list)) = self.reflection_list(bound)? =>
            {
                let element = self.reflection_list_element(&list, index)?;
                self.construct_dependent(element).map(Some)
            }
            // `FT()`: a construction of a field type bound as a local alias.
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } if param_args.is_empty()
                && args.is_empty()
                && kwargs.is_empty()
                && let Some(alias @ Ty::Dependent(_)) = self.lookup_tparam(name) =>
            {
                self.construct_dependent(alias).map(Some)
            }
            _ => Ok(None),
        }
    }

    /// The compile-time value of a reflection query, for the constant and
    /// dependent-expression evaluators. `None` when `expr` is no query.
    pub(super) fn eval_reflection_expr(&self, expr: &Expr) -> Result<Option<CtValue>, TypeError> {
        match self.reflection_query_of(expr)? {
            Some((subject, query)) => self.eval_reflection(&subject, query).map(Some),
            None => Ok(None),
        }
    }

    /// The type a reflection handle chain denotes in a compile-time position
    /// spelled as an expression: `types[i]`, `r.field_at[i].T`,
    /// `r.field["x"].T`, or a bound name of one.
    pub(super) fn reflected_type_operand(&self, expr: &Expr) -> Result<Option<Ty>, TypeError> {
        match &expr.kind {
            ExprKind::Identifier(name) => match self.local_comptime_value(name) {
                Some(bound) => self.reflected_type_operand(bound),
                None => Ok(None),
            },
            ExprKind::Member { object, field } if field == "T" => self.reflection_handle(object),
            ExprKind::Index { object, index } => match self.reflection_list(object)? {
                Some((ReflectQuery::FieldTypes, list)) => {
                    self.reflection_list_element(&list, index).map(Some)
                }
                _ => Ok(None),
            },
            _ => Ok(None),
        }
    }

    /// The same from a type annotation: `types[i]` (a `Named` with one
    /// argument over a bound list), `r.field_at[i].T`, `r.field["x"].T`, and
    /// `f.T` over a bound handle. A handle is always reached through a
    /// `comptime` binding here, as the elaborator requires too.
    pub(super) fn reflected_type_annotation(
        &self,
        source: &SourceType,
    ) -> Result<Option<Ty>, TypeError> {
        if self.local_comptime_values.iter().all(HashMap::is_empty) {
            return Ok(None);
        }
        match source {
            SourceType::Named(name, args) => {
                let [ParamArg::Value(index)] = args.as_slice() else {
                    return Ok(None);
                };
                let Some(bound) = self.local_comptime_value(name) else {
                    return Ok(None);
                };
                match self.reflection_list(bound)? {
                    Some((ReflectQuery::FieldTypes, list)) => {
                        self.reflection_list_element(&list, index).map(Some)
                    }
                    _ => Ok(None),
                }
            }
            SourceType::Assoc { base, name, args } if name == "T" && args.is_empty() => {
                self.handle_from_annotation(base)
            }
            _ => Ok(None),
        }
    }

    /// Answer a query: from the struct table for a registered struct (its
    /// arguments, symbolic or not, bind the field types), `False` for
    /// `is_struct()` of any other closed type, and a `ParamKind::Reflect`
    /// node for a subject that is still a parameter.
    pub(super) fn eval_reflection(
        &self,
        subject: &Ty,
        query: ReflectQuery,
    ) -> Result<CtValue, TypeError> {
        let (name, arguments, info) = match subject {
            Ty::Struct(name, arguments) if let Some(info) = self.structs.get(name) => {
                (name, arguments, info)
            }
            Ty::Param { .. } | Ty::Dependent(_) | Ty::SelfType => {
                let shape = self.param_context.type_shape(subject.clone());
                return Ok(CtValue::Expr(
                    self.param_context.reflect_query(&shape, query),
                ));
            }
            _ if mojito_types::types::has_free_parameters(subject) => {
                let shape = self.param_context.type_shape(subject.clone());
                return Ok(CtValue::Expr(
                    self.param_context.reflect_query(&shape, query),
                ));
            }
            _ => {
                return match query {
                    ReflectQuery::IsStruct => Ok(CtValue::Bool(false)),
                    query => Err(TypeError::NotComptime(format!(
                        "reflect[{subject}].{query} requires a struct type"
                    ))),
                };
            }
        };
        let field_ty =
            |(_, ty): &(String, Ty)| CtValue::Type(Box::new(substitute_at(ty, info, arguments)));
        let position = |field: &str| {
            info.fields
                .iter()
                .position(|(declared, _)| declared == field)
                .ok_or_else(|| {
                    TypeError::NotComptime(format!("struct '{name}' has no field named '{field}'"))
                })
        };
        Ok(match query {
            ReflectQuery::IsStruct => CtValue::Bool(true),
            ReflectQuery::FieldCount => CtValue::Int(info.fields.len() as i64),
            ReflectQuery::FieldNames => CtValue::Tuple(
                info.fields
                    .iter()
                    .map(|(declared, _)| CtValue::Str(declared.clone()))
                    .collect(),
            ),
            ReflectQuery::FieldTypes => CtValue::Tuple(info.fields.iter().map(field_ty).collect()),
            ReflectQuery::FieldIndex(field) => CtValue::Int(position(&field)? as i64),
            ReflectQuery::FieldNamed(field) => field_ty(&info.fields[position(&field)?]),
        })
    }

    /// The handle and query an expression reads: `r.field_count()`,
    /// `r.is_struct()`, `r.field_names()`, `r.field_types()`,
    /// `r.field_index["x"]()`, with `r` a handle expression or a bound name.
    fn reflection_query_of(&self, expr: &Expr) -> Result<Option<(Ty, ReflectQuery)>, TypeError> {
        match &expr.kind {
            ExprKind::Identifier(name) => match self.local_comptime_value(name) {
                Some(bound) => self.reflection_query_of(bound),
                None => Ok(None),
            },
            ExprKind::MethodCall {
                object,
                method,
                args,
                kwargs,
            } if args.is_empty() && kwargs.is_empty() => {
                let Some(subject) = self.reflection_handle(object)? else {
                    return Ok(None);
                };
                let query = match method.as_str() {
                    "is_struct" => ReflectQuery::IsStruct,
                    "field_count" => ReflectQuery::FieldCount,
                    "field_names" => ReflectQuery::FieldNames,
                    "field_types" => ReflectQuery::FieldTypes,
                    _ => return Err(no_such_reflection_method(&subject, method)),
                };
                Ok(Some((subject, query)))
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
                let Some(subject) = self.reflection_handle(object)? else {
                    return Ok(None);
                };
                match field.as_str() {
                    "field_index" => Ok(Some((
                        subject,
                        ReflectQuery::FieldIndex(reflection_field_name(param_args)?),
                    ))),
                    "field_type" => Err(removed_field_type()),
                    _ => Err(no_such_reflection_method(&subject, field)),
                }
            }
            _ => Ok(None),
        }
    }

    /// The subject type of a reflection handle expression: `reflect[X]`, a
    /// bound handle name, or a `field_at[i]` / `field[name]` selection on one.
    fn reflection_handle(&self, expr: &Expr) -> Result<Option<Ty>, TypeError> {
        match &expr.kind {
            ExprKind::TypeApply { name, args } if name == "reflect" => {
                self.reflection_subject(args).map(Some)
            }
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } if name == "reflect" && args.is_empty() && kwargs.is_empty() => {
                self.reflection_subject(param_args).map(Some)
            }
            ExprKind::Identifier(name) => match self.local_comptime_value(name) {
                Some(bound) => self.reflection_handle(bound),
                None => Ok(None),
            },
            ExprKind::Index { object, index } => {
                let ExprKind::Member {
                    object: inner,
                    field,
                } = &object.kind
                else {
                    return Ok(None);
                };
                if !matches!(field.as_str(), "field" | "field_at" | "field_type") {
                    return Ok(None);
                }
                let Some(subject) = self.reflection_handle(inner)? else {
                    return Ok(None);
                };
                self.reflection_selector(&subject, field, index).map(Some)
            }
            _ => Ok(None),
        }
    }

    /// A handle spelled in an annotation: a bound name, or a
    /// `field_at[i]` / `field[name]` selection on one.
    fn handle_from_annotation(&self, source: &SourceType) -> Result<Option<Ty>, TypeError> {
        match source {
            SourceType::Named(name, args) if args.is_empty() => {
                match self.local_comptime_value(name) {
                    Some(bound) => self.reflection_handle(bound),
                    None => Ok(None),
                }
            }
            SourceType::IndexedProjection { base, index } => {
                let SourceType::Assoc {
                    base: inner,
                    name,
                    args,
                } = base.as_ref()
                else {
                    return Ok(None);
                };
                if !args.is_empty() || !matches!(name.as_str(), "field" | "field_at" | "field_type")
                {
                    return Ok(None);
                }
                let Some(subject) = self.handle_from_annotation(inner)? else {
                    return Ok(None);
                };
                self.reflection_selector(&subject, name, index).map(Some)
            }
            _ => Ok(None),
        }
    }

    /// `field_at[index]` / `field[name]` over a subject: the selected field's
    /// type, dependent while the subject is a parameter.
    fn reflection_selector(
        &self,
        subject: &Ty,
        selector: &str,
        index: &Expr,
    ) -> Result<Ty, TypeError> {
        match selector {
            "field_at" => {
                let list = self.eval_reflection(subject, ReflectQuery::FieldTypes)?;
                self.reflection_list_element(&list, index)
            }
            "field" => {
                let ExprKind::Str(name) = &index.kind else {
                    return Err(TypeError::NotComptime(
                        "reflection field name must be a string literal".to_string(),
                    ));
                };
                match self.eval_reflection(subject, ReflectQuery::FieldNamed(name.clone()))? {
                    CtValue::Type(ty) => Ok(*ty),
                    CtValue::Expr(expr) => Ok(DependentType::resolve(expr)),
                    other => Err(TypeError::InvariantViolation(format!(
                        "reflected field '{name}' answered '{other}' rather than a type"
                    ))),
                }
            }
            "field_type" => Err(removed_field_type()),
            _ => Err(no_such_reflection_method(subject, selector)),
        }
    }

    /// The type `reflect[...]` applies to. `Self` is the enclosing struct at
    /// its own parameters, or the abstract `Self` of a trait body.
    fn reflection_subject(&self, args: &[ParamArg]) -> Result<Ty, TypeError> {
        let [argument] = args else {
            return Err(TypeError::WrongTypeArgCount {
                name: "reflect".to_string(),
                expected: 1,
                got: args.len(),
            });
        };
        if let ParamArg::Value(Expr {
            kind: ExprKind::Identifier(name),
            ..
        }) = argument
            && name == "Self"
        {
            return Ok(self.self_ty.clone().unwrap_or(Ty::SelfType));
        }
        self.type_param_argument(argument, "reflect")
    }

    /// A `field_names()` or `field_types()` read, with its value.
    fn reflection_list(&self, expr: &Expr) -> Result<Option<(ReflectQuery, CtValue)>, TypeError> {
        match self.reflection_query_of(expr)? {
            Some((subject, query @ (ReflectQuery::FieldNames | ReflectQuery::FieldTypes))) => {
                let value = self.eval_reflection(&subject, query.clone())?;
                Ok(Some((query, value)))
            }
            _ => Ok(None),
        }
    }

    /// The runtime type of a query's answer: an `Int`, a `Bool`; a list is
    /// a compile-time value only.
    fn reflection_value_ty(&self, subject: &Ty, query: &ReflectQuery) -> Result<Ty, TypeError> {
        let value = self.eval_reflection(subject, query.clone())?;
        match &value {
            CtValue::Int(_) | CtValue::IntLiteral(_) => Ok(Ty::Int),
            CtValue::Bool(_) => Ok(Ty::Bool),
            CtValue::Expr(expr) if let Some(ty) = expr.meta().as_value() => Ok(ty.clone()),
            _ => Err(TypeError::NotComptime(format!(
                "'reflect[{subject}].{query}' is a compile-time list; bind it with 'comptime' \
                 and index it"
            ))),
        }
    }

    /// Element `index` of a reflected field-type list: a closed list selects
    /// (a constant index folds to the field's type), a symbolic one is the
    /// dependent element, as a pack element is.
    fn reflection_list_element(&self, list: &CtValue, index: &Expr) -> Result<Ty, TypeError> {
        let list = match list {
            CtValue::Expr(expr) => expr.clone(),
            constant => self
                .param_context
                .constant(constant.clone())
                .map_err(param_error)?,
        };
        let index = self.reflection_index(index)?;
        self.param_context
            .list_get(&list, &index)
            .map(DependentType::resolve)
            .map_err(param_error)
    }

    /// A compile-time `Int` index over the parameters and `comptime for`
    /// variables in scope.
    fn reflection_index(&self, index: &Expr) -> Result<ParamExpr, TypeError> {
        self.compile_dependent_ct_expr(index)
            .map_err(|_| TypeError::TypeMismatch {
                expected: "a compile-time Int index".to_string(),
                found: "a runtime value".to_string(),
                context: "reflected field index".to_string(),
            })
    }

    /// `FT()` over a reflected field type: the type must be proved
    /// `Defaultable`, as the pin requires.
    fn construct_dependent(&self, ty: Ty) -> Result<Ty, TypeError> {
        let view = self.opaque_element(&ty).unwrap_or_else(|| ty.clone());
        if !self.conforms_to(&view, "Defaultable") {
            return Err(TypeError::BadCall {
                func: "__init__".to_string(),
                reason: format!(
                    "type '{ty}' does not conform to trait 'Defaultable'; either prove the \
                     conformance with 'conforms_to', or add conformance"
                ),
            });
        }
        Ok(ty)
    }

    /// The defining expression of a function-local `comptime` binding kept
    /// for inlining, innermost scope first.
    fn local_comptime_value(&self, name: &str) -> Option<&Expr> {
        self.local_comptime_values
            .iter()
            .rev()
            .find_map(|scope| scope.get(name))
    }
}

/// The one string parameter of `field_index[name]()`, positional or named.
fn reflection_field_name(param_args: &[ParamArg]) -> Result<String, TypeError> {
    let literal = |argument: &ParamArg| match argument {
        ParamArg::Value(Expr {
            kind: ExprKind::Str(name),
            ..
        }) => Some(name.clone()),
        _ => None,
    };
    match param_args {
        [argument @ ParamArg::Value(_)] => literal(argument),
        [ParamArg::Named { name, value }] if name == "name" => literal(value),
        _ => None,
    }
    .ok_or_else(|| {
        TypeError::NotComptime(
            "reflect[T].field_index[name]() takes one String parameter".to_string(),
        )
    })
}

fn no_such_reflection_method(subject: &Ty, method: &str) -> TypeError {
    TypeError::NoSuchMethod {
        object_type: format!("reflect[{subject}]"),
        method: method.to_string(),
    }
}

fn removed_field_type() -> TypeError {
    TypeError::NotComptime(
        "Reflected.field_type was removed; use Reflected.field[name]".to_string(),
    )
}
