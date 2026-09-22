//! Source-annotation conversion into resolved checked types and origins.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
pub use mojito_types::types::splats_to;

impl Checker {
    /// The dtype a SIMD element-type argument names: a `DType.<name>` member,
    /// a `SIMD` type's `dtype` (`Float64.dtype`), a `comptime` binding of
    /// one, or — symbolic while its declaration is a template — a `[dt:
    /// DType]` parameter in scope, bare or as `Self.dt`.
    pub(super) fn dtype_from_arg(
        &self,
        arg: &mojito_ast::ast::ParamArg,
    ) -> Result<SimdDtype, TypeError> {
        if let mojito_ast::ast::ParamArg::Value(Expr {
            kind: ExprKind::Member { object, field },
            ..
        }) = arg
        {
            if let ExprKind::Identifier(ns) = &object.kind
                && ns == "DType"
                && let Some(dtype) = Dtype::from_name(field)
            {
                return Ok(SimdDtype::Known(dtype));
            }
            if field == "dtype"
                && let Some(dtype) = self.dtype_constant(object, field)
            {
                return dtype;
            }
        }
        // A bracketed `Float64.dtype` parses as an associated-member type.
        if let mojito_ast::ast::ParamArg::Type(SourceType::Assoc { base, name, args }) = arg
            && name == "dtype"
            && args.is_empty()
            && let Some(dtype) = self
                .ty_from_anno(base)
                .ok()
                .as_ref()
                .and_then(super::indexing::simd_dtype)
        {
            return dtype;
        }
        if let mojito_ast::ast::ParamArg::Value(Expr {
            kind: ExprKind::Identifier(name),
            ..
        }) = arg
        {
            if let Some(dtype) = self.comptime_dtypes.get(name) {
                return Ok(SimdDtype::Known(*dtype));
            }
            if let Some(expr) = self.value_parameter_in_scope(name)
                && expr.meta().as_value() == Some(&Ty::Dtype)
            {
                return Ok(SimdDtype::Expr(expr));
            }
        }
        if let mojito_ast::ast::ParamArg::Type(SourceType::SelfParam(param)) = arg {
            match self.self_param_value(param) {
                Some(CtValue::Dtype(dtype)) => return Ok(SimdDtype::Known(dtype)),
                Some(CtValue::Expr(expr)) if expr.meta().as_value() == Some(&Ty::Dtype) => {
                    return Ok(SimdDtype::Expr(expr));
                }
                _ => {}
            }
        }
        Err(TypeError::BadDtype(match arg {
            mojito_ast::ast::ParamArg::Value(Expr {
                kind: ExprKind::Member { field, .. },
                ..
            }) => {
                format!("DType.{field}")
            }
            _ => "a non-DType argument".to_string(),
        }))
    }
}

/// Whether `ty` may be an **explicit construction** argument for a `dtype`
/// lane — `SIMD[...](...)`, a scalar alias like `Byte(...)`, or
/// `Scalar[DType.x](...)`. Explicit construction converts where the implicit
/// contexts `splats_to` governs (operator splats, coercions) stay
/// literal-exact: runtime integers wrap to any integer width and convert to
/// float lanes, and runtime floats adjust precision across float widths. A
/// float source never converts to an integer lane — spell the truncation
/// (`Int(x)`) first. `Intable` params/structs are the caller's separate,
/// `conforms_to`-backed clause (`check_simd_args`). A symbolic lane takes
/// any integer or float source: which conversions the instantiation admits
/// is its own check.
pub(super) fn converts_to_lane(ty: &Ty, dtype: &SimdDtype) -> bool {
    if splats_to(ty, dtype) {
        return true;
    }
    let integer_source = matches!(ty, Ty::Int | Ty::UInt)
        || matches!(scalar_simd_dtype(ty), Some(d) if !d.is_float() && d != Dtype::Bool);
    let float_source =
        matches!(ty, Ty::Float64) || matches!(scalar_simd_dtype(ty), Some(d) if d.is_float());
    match dtype.known() {
        Some(Dtype::Bool) => false,
        Some(dtype) if dtype.is_float() => float_source || integer_source,
        Some(_) => integer_source,
        None => float_source || integer_source,
    }
}

pub(super) const fn int_literal_materializes_to_dtype(dtype: Dtype) -> bool {
    match dtype {
        Dtype::Int
        | Dtype::Int8
        | Dtype::Int16
        | Dtype::Int32
        | Dtype::Int64
        | Dtype::UInt8
        | Dtype::UInt16
        | Dtype::UInt32
        | Dtype::UInt64 => true,
        // Integer and floating literals round during floating materialization;
        // overflow is the corresponding IEEE infinity.
        Dtype::Float16 | Dtype::Float32 | Dtype::Float64 => true,
        Dtype::Bool => false,
    }
}

/// The (canonicalized) `Ty` for a SIMD of `dtype`/`width`: a **width-1 `float64`**
/// is the native `Ty::Float64` (Mojo unifies `Float64` with `SIMD[DType.float64,
/// 1]`); everything else is a `Ty::Simd`.
pub(super) const fn simd_ty(dtype: Dtype, width: i64) -> Ty {
    mojito_types::types::canonical_simd_ty(dtype, width)
}

/// [`simd_ty`] over slots that may still be symbolic.
pub(super) fn simd_of(dtype: SimdDtype, width: SimdWidth) -> Result<Ty, TypeError> {
    mojito_types::types::simd_ty_from_slots(dtype, width).map_err(param_error)
}

/// The scalar `Ty` a value-parameter type name denotes, or `None` if the name is
/// not a scalar type (so it is a trait, i.e. a type parameter). Used to classify
/// `[name: X]` as a value vs. type parameter.
pub(super) fn scalar_type_name(name: &str) -> Option<Ty> {
    match name {
        "Int" => Some(Ty::Int),
        // A `DType` value, and the type of a `[dtype: DType]` value parameter.
        "DType" => Some(Ty::Dtype),
        // The removed `SIMDSize` spelling rejects (upstream removed it 2026-08).
        "SIMDLength" => Some(Ty::Int),
        "UInt" => Some(Ty::UInt),
        "Bool" => Some(Ty::Bool),
        "String" => Some(Ty::StringLiteral),
        "StringLiteral" => Some(Ty::StringLiteral),
        "Float64" => Some(Ty::Float64),
        // The prelude rewrite qualifies `String` bounds like any other name;
        // a `[text: String]` value parameter keeps the compile-time string
        // type regardless of the nominal stdlib struct.
        _ if mojito_symbol::symbol::is_stdlib_string_struct(name) => Some(Ty::StringLiteral),
        _ => None,
    }
}

/// The type-parameter scope of a parameter list, for resolving a bare `T`
/// annotation. Retaining the complete checked `Ty::Param` is important for
/// callable bounds: a name-only/bounds-only scope would discard the signature
/// needed to type `f(...)` inside `[F: def(...) -> ...]`.
pub(super) fn type_scope(decls: &[ParamDecl]) -> HashMap<String, Ty> {
    decls
        .iter()
        .filter_map(|d| match d {
            ParamDecl::Type {
                name,
                bounds,
                callable_bound,
                ..
            } => Some((
                name.clone(),
                Ty::Param {
                    name: name.clone(),
                    bounds: bounds.clone(),
                    callable_bound: callable_bound.clone(),
                },
            )),
            ParamDecl::Value { .. } => None,
        })
        .collect()
}

/// A struct's own parameters, as the `TyArg`s they contribute to the struct's
/// `Self` type while its body is checked: a type parameter as `Ty::Param`, a
/// value parameter as a reference to its declaration.
pub(super) fn params_as_args(owner: &str, decls: &[ParamDecl]) -> Vec<TyArg> {
    decls
        .iter()
        .enumerate()
        .map(|(slot, decl)| match decl {
            ParamDecl::Type {
                name,
                bounds,
                callable_bound,
                ..
            } => TyArg::Ty(Ty::Param {
                name: name.clone(),
                bounds: bounds.clone(),
                callable_bound: callable_bound.clone(),
            }),
            ParamDecl::Value { name, ty, .. } => TyArg::Val(value_parameter(owner, slot, name, ty)),
        })
        .collect()
}

/// The reference to value parameter `slot` of the declaration `owner`, which
/// [`binder_owner`] or [`method_binder_owner`] names.
pub(super) fn value_parameter(owner: &str, slot: usize, name: &str, ty: &Ty) -> CtValue {
    CtValue::Expr(value_parameter_expr(owner, slot, name, ty))
}

pub(super) fn value_parameter_expr(owner: &str, slot: usize, name: &str, ty: &Ty) -> ParamExpr {
    ParamContext::detached().decl_ref(
        mojito_types::param_expr::ParamId::new(&binder_owner(owner), slot),
        name.trim_start_matches('*'),
        mojito_types::param_expr::MetaTy::value(ty.clone()),
    )
}

/// The declaration that owns a name's parameter binders: the template, so a
/// `$` clone shares its template's parameters rather than minting fresh ones.
pub(super) fn binder_owner(name: &str) -> String {
    mojito_symbol::symbol::demangle_specialization(name)
        .map_or(name, |(template, _)| template)
        .to_string()
}

/// [`binder_owner`] for a method's own parameters.
pub(super) fn method_binder_owner(owner: &str, method: &str) -> String {
    format!("{}.{}", binder_owner(owner), binder_owner(method))
}

/// The checker's diagnostic for a parameter-expression error.
pub(super) fn param_error(error: mojito_types::param_expr::ParamError) -> TypeError {
    use mojito_types::param_expr::ParamError;
    TypeError::NotComptime(match error {
        ParamError::Arithmetic(message) | ParamError::Unsupported(message) => message,
        other => other.to_string(),
    })
}

/// The binder owner of `method` on the struct `self_ty` names.
pub(super) fn method_owner(self_ty: &Ty, method: &str) -> String {
    match self_ty {
        Ty::Struct(owner, _) => method_binder_owner(owner, method),
        _ => method_binder_owner("Self", method),
    }
}

/// The variadic type packs a declaration's own binders open, as the
/// parameter-list references an element of the pack indexes.
pub(super) fn pack_scope(owner: &str, decls: &[ParamDecl]) -> HashMap<String, ParamExpr> {
    decls
        .iter()
        .enumerate()
        .filter_map(|(slot, decl)| match decl {
            ParamDecl::Type {
                name,
                variadic: true,
                ..
            } => Some((
                name.trim_start_matches('*').to_string(),
                ParamContext::detached().decl_ref(
                    mojito_types::param_expr::ParamId::new(&binder_owner(owner), slot),
                    name.trim_start_matches('*'),
                    mojito_types::param_expr::MetaTy::type_list(),
                ),
            )),
            _ => None,
        })
        .collect()
}

/// The value-parameter scope a declaration's own binders open.
pub(super) fn value_scope(owner: &str, decls: &[ParamDecl]) -> HashMap<String, ParamExpr> {
    decls
        .iter()
        .enumerate()
        .filter_map(|(slot, decl)| match decl {
            ParamDecl::Value {
                name,
                ty,
                callable_default: None,
                ..
            } if !matches!(ty.as_ref(), Ty::Func { .. } | Ty::GenericFunc { .. }) => Some((
                name.trim_start_matches('*').to_string(),
                value_parameter_expr(owner, slot, name, ty),
            )),
            _ => None,
        })
        .collect()
}

/// The substitution mapping a struct's type-parameter names to a value's type
/// arguments (`[T] @ [Int]` ⟹ `{T: Int}`). Value parameters/arguments are
/// skipped (they never appear in a type). Empty for a non-generic struct.
pub(super) fn struct_subst(decls: &[ParamDecl], targs: &[TyArg]) -> HashMap<String, Ty> {
    mojito_types::types::struct_argument_substitution(decls, targs)
}
