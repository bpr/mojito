//! Reflection under the checker: a `reflect[T]` query is answered from the
//! struct table when its subject is a registered struct and as a
//! `ParamKind::Reflect` node when the subject is still a parameter, so a body
//! reading a reflection handle validates with its parameters symbolic. The
//! elaborator evaluates the same queries per instance: on a clone in
//! `comptime/eval.rs`, and in a template-served body through the parameter
//! constant `record_reflection_value` records, which `native::mono` answers.
//! See `docs/symbol-map.md`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_ast::ast::ParamArg;
use mojito_types::param_expr::{ParamKind, ReflectQuery};
use mojito_types::types::DependentType;

impl Checker {
    /// The type of a reflection read in a value position: a count, an index,
    /// or an `is_struct()` answer, the `field_names()` list called in place
    /// (the `Array` the pin's query returns) or an element of it, the length
    /// of either list, or a construction of a field type (`FT()`,
    /// `types[i]()`). `None` when `expr` reads no reflection handle. A bare
    /// handle or field-type list is no runtime value, so its `comptime`
    /// binding is kept for inlining at its compile-time uses.
    pub(super) fn infer_reflection(&self, expr: &Expr) -> Result<Option<Ty>, TypeError> {
        match &expr.kind {
            ExprKind::Identifier(name) => match self.local_comptime_value(name) {
                Some(bound) => self.infer_reflection(bound),
                None => Ok(None),
            },
            ExprKind::MethodCall { .. } | ExprKind::Invoke { .. } => {
                match self.reflection_query_of(expr)? {
                    Some((subject, query)) => {
                        let value = self.eval_reflection(&subject, query.clone())?;
                        let names = (query == ReflectQuery::FieldNames)
                            .then(|| self.reflected_names_array_ty(value.clone()))
                            .flatten();
                        let ty = match names {
                            Some(ty) => ty,
                            None => reflection_value_ty(&subject, &query, &value)?,
                        };
                        self.record_reflection_value(expr, value);
                        Ok(Some(ty))
                    }
                    None => Ok(None),
                }
            }
            ExprKind::Index { object, index } => match self.reflection_list(object)? {
                Some((ReflectQuery::FieldNames, list)) => {
                    self.reject_bound_list_crossing(object, &list)?;
                    let index = self.reflection_index(index)?;
                    if let Ok(list) = self.param_context.constant(list)
                        && let Ok(element) = self.param_context.list_get(&list, &index)
                    {
                        self.record_reflection_value(expr, CtValue::Expr(element));
                    }
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
                Ok(self.reflection_list_length(expr, object)?)
            }
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } if name == "len" && param_args.is_empty() && args.len() == 1 && kwargs.is_empty() => {
                Ok(self.reflection_list_length(expr, &args[0])?)
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
                self.construct_dependent(expr, element).map(Some)
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
                self.construct_dependent(expr, alias).map(Some)
            }
            _ => Ok(None),
        }
    }

    /// `materialize[names]()` of a field-name list in the executable check:
    /// the list is over a type parameter (the crossing pass folds a closed
    /// one), so the call is the parameter value MIR carries and the
    /// elaborator constructs per instance as the `Array` it types as. A
    /// field-type list is no runtime value here. `None` when `operand` is
    /// no reflected list.
    pub(super) fn materialized_reflection_list(
        &self,
        span: &SourceSpan,
        operand: &Expr,
    ) -> Result<Option<Ty>, TypeError> {
        match self.reflection_list(operand)? {
            Some((ReflectQuery::FieldNames, list)) => {
                let ty = self.reflected_names_array_ty(list.clone()).ok_or_else(|| {
                    TypeError::NotComptime(
                        "materialize[...]() of a field-name list whose length is unknown"
                            .to_string(),
                    )
                })?;
                self.record_param_value(span.clone(), list);
                Ok(Some(ty))
            }
            Some(_) => Err(TypeError::NotComptime(
                "type-valued or symbolic comptime values cannot materialize at runtime".to_string(),
            )),
            None => Ok(None),
        }
    }

    /// The compile-time value of a reflection query, or of the length of a
    /// reflected list (`len(names)`, `names.length`), for the constant and
    /// dependent-expression evaluators. `None` when `expr` is neither.
    pub(super) fn eval_reflection_expr(&self, expr: &Expr) -> Result<Option<CtValue>, TypeError> {
        if let Some((subject, query)) = self.reflection_query_of(expr)? {
            return self.eval_reflection(&subject, query).map(Some);
        }
        let measured = match &expr.kind {
            ExprKind::Member { object, field } if field == "length" => object,
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } if name == "len" && param_args.is_empty() && kwargs.is_empty() => {
                match args.as_slice() {
                    [list] => list,
                    _ => return Ok(None),
                }
            }
            _ => return Ok(None),
        };
        Ok(self
            .reflection_list(measured)?
            .and_then(|(_, list)| self.reflection_list_count(list)))
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
    /// arguments, symbolic or not, bind the field types), `True` for
    /// `is_struct()` of any other closed type (only an MLIR primitive, which
    /// Mojito never spells, answers `False` at the pin), and a `ParamKind::Reflect`
    /// node for a subject that is still a parameter.
    pub(super) fn eval_reflection(
        &self,
        subject: &Ty,
        query: ReflectQuery,
    ) -> Result<CtValue, TypeError> {
        let fields = match subject {
            Ty::Struct(name, arguments) if let Some(info) = self.structs.get(name) => Some(
                info.fields
                    .iter()
                    .map(|(field, ty)| (field.clone(), substitute_at(ty, info, arguments)))
                    .collect::<Vec<_>>(),
            ),
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
            _ => None,
        };
        query
            .answer(subject, fields.as_deref())
            .map_err(|error| TypeError::NotComptime(error.to_string()))
    }

    /// The defining expression of a function-local `comptime` binding kept
    /// for inlining, innermost scope first.
    pub(super) fn local_comptime_value(&self, name: &str) -> Option<&Expr> {
        self.local_comptime_values
            .iter()
            .rev()
            .find_map(|scope| scope.get(name))
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
    pub(super) fn reflection_handle(&self, expr: &Expr) -> Result<Option<Ty>, TypeError> {
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
    pub(super) fn reflection_list(
        &self,
        expr: &Expr,
    ) -> Result<Option<(ReflectQuery, CtValue)>, TypeError> {
        match self.reflection_query_of(expr)? {
            Some((subject, query @ (ReflectQuery::FieldNames | ReflectQuery::FieldTypes))) => {
                let value = self.eval_reflection(&subject, query.clone())?;
                Ok(Some((query, value)))
            }
            _ => Ok(None),
        }
    }

    /// The length of a `field_names()` or `field_types()` list read at
    /// `expr`: an `Int`, which over a symbolic subject is its field count,
    /// as upstream sizes both lists by `_field_types_of[T]().length`.
    fn reflection_list_length(&self, expr: &Expr, list: &Expr) -> Result<Option<Ty>, TypeError> {
        let Some((query, value)) = self.reflection_list(list)? else {
            return Ok(None);
        };
        if query == ReflectQuery::FieldNames {
            self.reject_bound_list_crossing(list, &value)?;
        }
        if let Some(count) = self.reflection_list_count(value) {
            self.record_reflection_value(expr, count);
        }
        Ok(Some(Ty::Int))
    }

    /// A runtime read through a `comptime` binding of a field-name list
    /// (`names[i]`, `len(names)`) would materialize the whole list, which is
    /// not implicitly copyable, as the pin rejects any compile-time `Array`
    /// read at runtime. A query spelled in place is a runtime call, and a
    /// compile-time position reads the binding where it stands.
    fn reject_bound_list_crossing(&self, list: &Expr, value: &CtValue) -> Result<(), TypeError> {
        if !matches!(list.kind, ExprKind::Identifier(_)) || !self.crosses_to_runtime() {
            return Ok(());
        }
        let length = self
            .reflection_list_count(value.clone())
            .map_or_else(|| "_".to_string(), |count| count.to_string());
        Err(TypeError::ComptimeCrossing(format!(
            "Array[String, Int({length})]"
        )))
    }

    /// The element count of a reflected list value: a closed list's length,
    /// a symbolic one's field count query.
    fn reflection_list_count(&self, list: CtValue) -> Option<CtValue> {
        match list {
            CtValue::Expr(value) => match value.kind() {
                ParamKind::Reflect { subject, .. } => Some(CtValue::Expr(
                    self.param_context
                        .reflect_query(subject, ReflectQuery::FieldCount),
                )),
                _ => None,
            },
            CtValue::Tuple(elements) => Some(CtValue::Int(elements.len() as i64)),
            _ => None,
        }
    }

    /// Record the answer to a query read as a runtime value at `expr`, as the
    /// constant MIR carries: the elaborator answers one over a subject that
    /// is still a parameter per instance. A closed answer reaches here where
    /// its subject was not (`reflect[Self].field_count()` in a generic
    /// struct's method), so the crossing pass could not fold it.
    fn record_reflection_value(&self, expr: &Expr, value: CtValue) {
        self.record_param_value(expr.source_span(), value);
    }

    /// The runtime `Array` a field-name list materializes as: `String`
    /// elements, sized by the list's count.
    fn reflected_names_array_ty(&self, list: CtValue) -> Option<Ty> {
        self.reflection_list_count(list).map(|count| {
            mojito_types::types::array_type_of(
                mojito_types::types::nominal_type(
                    mojito_symbol::symbol::STDLIB_STRING_STRUCT,
                    Vec::new(),
                ),
                count,
            )
        })
    }

    /// Record `value` as the parameter constant read at `span`. A read
    /// re-inferred keeps the temporary a borrow of it already materialized
    /// (`materialize[names]()[0]`).
    fn record_param_value(&self, span: SourceSpan, value: CtValue) {
        let Ok(value) = self.param_context.constant(value) else {
            return;
        };
        let mut adjustments = self.operation_adjustments.borrow_mut();
        let materialized = match adjustments.get(&span) {
            Some(mojito_checked::checked::SemanticAdjustment::ParamValue {
                materialized, ..
            }) => *materialized,
            _ => None,
        };
        adjustments.insert(
            span,
            mojito_checked::checked::SemanticAdjustment::ParamValue {
                value,
                materialized,
            },
        );
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
    /// `Defaultable`, as the pin requires. The construction is recorded for
    /// MIR, which constructs the type the expression denotes.
    fn construct_dependent(&self, expr: &Expr, ty: Ty) -> Result<Ty, TypeError> {
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
        self.operation_adjustments.borrow_mut().insert(
            expr.source_span(),
            mojito_checked::checked::SemanticAdjustment::ConstructType { ty: ty.clone() },
        );
        Ok(ty)
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

/// The runtime type of a scalar query's answer: an `Int`, a `Bool`; a
/// field-type list is a compile-time value only.
fn reflection_value_ty(
    subject: &Ty,
    query: &ReflectQuery,
    value: &CtValue,
) -> Result<Ty, TypeError> {
    match value {
        CtValue::Int(_) | CtValue::IntLiteral(_) => Ok(Ty::Int),
        CtValue::Bool(_) => Ok(Ty::Bool),
        CtValue::Expr(expr) if let Some(ty) = expr.meta().as_value() => Ok(ty.clone()),
        _ => Err(TypeError::NotComptime(format!(
            "'reflect[{subject}].{query}' is a compile-time list; bind it with 'comptime' and \
             index it"
        ))),
    }
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
