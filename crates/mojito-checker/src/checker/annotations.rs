//! Source-annotation conversion into resolved checked types and origins.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::TyKind;
pub use mojito_types::types::splats_to;

impl Checker {
    /// The declaration that owns method `m`'s own binders on the struct
    /// `owner` ([`mojito_symbol::symbol::MethodBinderOwners`]).
    pub(super) fn method_binder_owner(&self, owner: &str, m: &Method) -> String {
        self.method_binder_owners.owner(owner, m)
    }

    /// [`Self::method_binder_owner`] of `m` on the struct `self_ty` names.
    pub(super) fn method_owner(&self, self_ty: &Ty, m: &Method) -> String {
        match self_ty.kind() {
            TyKind::Struct(owner, _) => self.method_binder_owner(owner, m),
            _ => self.method_binder_owner("Self", m),
        }
    }

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
            // A leading-dot member (`.int32`) is `DType`'s, the slot's type.
            if let ExprKind::Identifier(ns) = &object.kind
                && (ns == "DType" || ns == mojito_ast::ast::CONTEXTUAL_SENTINEL)
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
            && let Some(dtype) = self.dtype_named(name)
        {
            return Ok(dtype);
        }
        if let mojito_ast::ast::ParamArg::Type(SourceType::SelfParam(param)) = arg {
            match self.self_param_value(param) {
                Some(CtValue::Dtype(dtype)) => return Ok(SimdDtype::Known(dtype)),
                Some(CtValue::Expr(expr)) if expr.meta().as_value() == Some(&Ty::dtype()) => {
                    return Ok(SimdDtype::Expr(expr));
                }
                _ => {}
            }
        }
        // Any other compile-time expression of type `DType`
        // (`_uint_type_of_width[w]()`) is the value it folds to, or that
        // expression while its operands are open.
        if let mojito_ast::ast::ParamArg::Value(expr) = arg
            && matches!(
                expr.kind,
                ExprKind::Call { .. } | ExprKind::MethodCall { .. }
            )
        {
            let _lifting = self.lifting_position();
            let evaluated = self.eval_associated_ct(expr);
            match evaluated {
                Ok(CtValue::Dtype(dtype)) => return Ok(SimdDtype::Known(dtype)),
                Ok(CtValue::Expr(expr)) if expr.meta().as_value() == Some(&Ty::dtype()) => {
                    return Ok(SimdDtype::Expr(expr));
                }
                _ => {
                    if let Some(application) = self
                        .lifted_application(expr)
                        .filter(|application| application.meta().as_value() == Some(&Ty::dtype()))
                    {
                        return Ok(SimdDtype::Expr(application));
                    }
                }
            }
        }
        Err(TypeError::BadDtype(match arg {
            mojito_ast::ast::ParamArg::Value(Expr {
                kind: ExprKind::Member { field, .. },
                ..
            }) => {
                format!("DType.{field}")
            }
            mojito_ast::ast::ParamArg::Value(Expr {
                kind: ExprKind::Identifier(name),
                ..
            }) => format!("'{name}' does not name a DType in scope"),
            _ => "a non-DType argument".to_string(),
        }))
    }

    /// The dtype a bare name denotes: a `comptime` binding of one, a module
    /// constant the declarations pass folded to one ahead of its statement,
    /// or a `[dt: DType]` parameter in scope — symbolic while its
    /// declaration is still a template. The binding table and the parameter
    /// scope agree here, so a `comptime d = dt` binding names the same lane
    /// its parameter does.
    pub(super) fn dtype_named(&self, name: &str) -> Option<SimdDtype> {
        if let Some(dtype) = self.comptime_dtype(name) {
            return Some(dtype.clone());
        }
        if self.lookup_tparam(name).is_none()
            && let Some(CtValue::Dtype(dtype)) = self.module_values.get(name)
        {
            return Some(SimdDtype::Known(*dtype));
        }
        self.value_parameter_in_scope(name)
            .filter(|expr| expr.meta().as_value() == Some(&Ty::dtype()))
            .map(SimdDtype::Expr)
    }

    /// The dtype a local or module `comptime` binding named `name` holds,
    /// innermost scope first.
    pub(super) fn comptime_dtype(&self, name: &str) -> Option<&SimdDtype> {
        self.comptime_dtypes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name))
            .or_else(|| self.installed_comptime_dtype(name))
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
/// `conforms_to`-backed clause (`check_simd_value`). A symbolic lane takes
/// any integer or float source: which conversions the instantiation admits
/// is its own check.
pub(super) fn converts_to_lane(ty: &Ty, dtype: &SimdDtype) -> bool {
    if splats_to(ty, dtype) {
        return true;
    }
    let integer_source = matches!(ty.kind(), TyKind::INT | TyKind::UINT)
        || matches!(scalar_simd_dtype(ty), Some(d) if !d.is_float() && d != Dtype::Bool);
    let float_source = matches!(ty.kind(), TyKind::FLOAT64)
        || matches!(scalar_simd_dtype(ty), Some(d) if d.is_float());
    match dtype.known() {
        // Upstream's `IntLiteral` initializer converts a literal into a
        // mask lane by `value != 0`.
        Some(Dtype::Bool) => *ty.kind() == TyKind::IntLiteral,
        Some(dtype) if dtype.is_float() => float_source || integer_source,
        Some(_) => integer_source,
        None => float_source || integer_source,
    }
}

pub(super) const fn int_literal_materializes_to_dtype(dtype: Dtype) -> bool {
    match dtype {
        Dtype::Int
        | Dtype::UInt
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
/// is the native `TyKind::FLOAT64` (Mojo unifies `Float64` with `SIMD[DType.float64,
/// 1]`); everything else is a `TyKind::Simd`.
pub(super) fn simd_ty(dtype: Dtype, width: i64) -> Ty {
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
        "Int" => Some(Ty::int()),
        // A `DType` value, and the type of a `[dtype: DType]` value parameter.
        "DType" => Some(Ty::dtype()),
        "UInt" => Some(Ty::uint()),
        "Bool" => Some(Ty::bool()),
        "String" => Some(Ty::string_literal()),
        "StringLiteral" => Some(Ty::string_literal()),
        "Float64" => Some(Ty::float64()),
        // The prelude rewrite qualifies `String` bounds like any other name;
        // a `[text: String]` value parameter keeps the compile-time string
        // type regardless of the nominal stdlib struct.
        _ if mojito_symbol::symbol::is_stdlib_string_struct(name) => Some(Ty::string_literal()),
        _ => None,
    }
}

/// The type-parameter scope of a parameter list, for resolving a bare `T`
/// annotation. Retaining the complete checked `TyKind::Param` is important for
/// callable bounds: a name-only/bounds-only scope would discard the signature
/// needed to type `f(...)` inside `[F: def(...) -> ...]`.
pub(super) fn type_scope(decls: &[ParamDecl]) -> HashMap<String, Ty> {
    decls
        .iter()
        .filter_map(|decl| type_parameter(decl).map(|ty| (decl.name().to_string(), ty)))
        .collect()
}

/// The `TyKind::Param` a type binder's uses carry, `None` for a value binder.
pub(super) fn type_parameter(decl: &ParamDecl) -> Option<Ty> {
    match decl {
        ParamDecl::Type {
            bounds,
            callable_bound,
            ..
        } => Some(
            TyKind::Param {
                binder: decl.binder(),
                bounds: bounds.clone(),
                callable_bound: callable_bound.clone(),
            }
            .into(),
        ),
        ParamDecl::Value { .. } => None,
    }
}

/// A struct's own parameters, as the `TyArg`s they contribute to the struct's
/// `Self` type while its body is checked: a type parameter as `TyKind::Param`, a
/// value parameter as a reference to its declaration.
pub(super) fn params_as_args(decls: &[ParamDecl]) -> Vec<TyArg> {
    decls.iter().map(ParamDecl::own_argument).collect()
}

/// The reference to the value binder `decl`, typed `ty`.
pub(super) fn value_parameter(decl: &ParamDecl, ty: &Ty) -> CtValue {
    CtValue::Expr(value_parameter_expr(decl, ty))
}

/// The reference to the value binder `decl` typed `ty`, at the meta-type its
/// binder declares: a pack (`*values: Int`) or a raw list is a parameter list.
pub(super) fn value_parameter_expr(decl: &ParamDecl, ty: &Ty) -> ParamExpr {
    ParamContext::detached().decl_ref(
        decl.id().clone(),
        decl.name().trim_start_matches('*'),
        decl.with_value_type(ty.clone()).binder_meta(),
    )
}

/// The binder `slot` of a parameterized associated member or generic alias
/// `owner` declares: its own `[params]` are numbered by source position.
pub(super) fn member_binder(owner: &str, slot: usize, name: &str) -> ParamRef {
    ParamRef {
        id: ParamId::new(&binder_owner(owner), slot),
        name: name.into(),
    }
}

/// The binder of argument `slot` of the `@always_inline("builtin")`
/// callable `symbol`, over which its folded body is stated
/// ([`super::Checker::fold_builtin_bodies`]): one owner per callable, apart
/// from its declared binders' own.
pub(super) fn builtin_body_binder(symbol: &str, slot: usize, name: &str) -> ParamRef {
    ParamRef {
        id: ParamId::new(&format!("{BUILTIN_BODY_PREFIX}{symbol}"), slot),
        name: name.into(),
    }
}

/// The type-parameter scope a parameterized associated member's own
/// `[params]` open while its body is lowered ([`member_binder`] numbering).
pub(super) fn member_type_scope(
    owner: &str,
    params: &[mojito_ast::ast::TypeParam],
) -> HashMap<String, Ty> {
    params
        .iter()
        .enumerate()
        .filter(|(_, p)| {
            matches!(
                super::constraints::assoc_param_kind(p),
                super::constraints::AssocParamKind::Type
            )
        })
        .map(|(slot, p)| {
            (
                p.name.clone(),
                TyKind::Param {
                    binder: member_binder(owner, slot, &p.name),
                    bounds: p.bounds.clone(),
                    callable_bound: None,
                }
                .into(),
            )
        })
        .collect()
}

/// A checker-made type parameter no declaration owns: an existential
/// `Some[Trait]`, a builtin intrinsic's signature, a probe. Its spelling is
/// its whole identity.
pub(super) fn synthetic_binder(name: &str) -> ParamRef {
    ParamRef {
        id: ParamId::new(&format!("{SYNTHETIC_BINDER_PREFIX}{name}"), 0),
        name: name.into(),
    }
}

/// Whether `binder` is an existential `Some[Trait]`'s [`synthetic_binder`].
pub(super) fn is_existential_binder(binder: &ParamRef) -> bool {
    binder.id.owner.starts_with(SYNTHETIC_BINDER_PREFIX) && binder.name.starts_with("Some[")
}

/// The reference to the value binder `id` spelled `name`, typed `ty`.
pub(super) fn value_binder_expr(id: ParamId, name: &str, ty: &Ty) -> ParamExpr {
    ParamContext::detached().decl_ref(
        id,
        name.trim_start_matches('*'),
        mojito_types::param_expr::MetaTy::value(ty.clone()),
    )
}

/// The declaration that owns a name's parameter binders: the template, so a
/// `$` clone shares its template's parameters rather than minting fresh ones.
pub(super) fn binder_owner(name: &str) -> String {
    mojito_symbol::symbol::specialization_template(name)
        .unwrap_or(name)
        .to_string()
}

/// The checker's diagnostic for a parameter-expression error.
pub(super) fn param_error(error: mojito_types::param_expr::ParamError) -> TypeError {
    use mojito_types::param_expr::ParamError;
    TypeError::NotComptime(match error {
        ParamError::Arithmetic(message) | ParamError::Unsupported(message) => message,
        other => other.to_string(),
    })
}

/// The variadic packs a declaration's own binders open, type and value
/// packs alike, as the parameter-list references an element of the pack
/// indexes: what upstream types a `TypeList` or a `ParameterList`.
pub(super) fn pack_scope(decls: &[ParamDecl]) -> HashMap<String, ParamExpr> {
    decls
        .iter()
        .filter_map(|decl| match decl {
            ParamDecl::Type {
                name,
                variadic: true,
                ..
            }
            | ParamDecl::Value {
                name,
                variadic: true,
                ..
            } => Some((
                name.trim_start_matches('*').to_string(),
                ParamContext::detached().decl_ref(
                    decl.id().clone(),
                    name.trim_start_matches('*'),
                    decl.binder_meta(),
                ),
            )),
            _ => None,
        })
        .collect()
}

/// The value-parameter scope a declaration's own binders open.
pub(super) fn value_scope(decls: &[ParamDecl]) -> HashMap<String, ParamExpr> {
    decls
        .iter()
        .filter_map(|decl| match decl {
            ParamDecl::Value {
                name,
                ty,
                callable_default: None,
                ..
            } if !decl.binds_callables() => Some((
                name.trim_start_matches('*').to_string(),
                value_parameter_expr(decl, ty),
            )),
            _ => None,
        })
        .collect()
}

/// The substitution mapping a struct's type binders to a value's type
/// arguments (`[T] @ [Int]` ⟹ `{T: Int}`). Value parameters/arguments are
/// skipped (they never appear in a type). Empty for a non-generic struct.
pub(super) fn struct_subst(decls: &[ParamDecl], targs: &[TyArg]) -> TySubst {
    mojito_types::types::struct_argument_substitution(decls, targs)
}

/// The owner spelling every [`synthetic_binder`] starts with.
const SYNTHETIC_BINDER_PREFIX: &str = "$synthetic:";

/// The owner spelling every [`builtin_body_binder`] starts with.
const BUILTIN_BODY_PREFIX: &str = "$builtin:";
