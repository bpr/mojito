//! Source-annotation conversion into resolved checked types and origins.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
pub use mojito_types::types::splats_to;

impl Checker {
    /// The declaration that owns method `m`'s own binders on the struct
    /// `owner` ([`binder_owner`]): `Struct.method`, or for an overloaded
    /// method the symbol its template lowers to, so two overloads own
    /// distinct binders and every clone of one shares its template's.
    pub(super) fn method_binder_owner(&self, owner: &str, m: &Method) -> String {
        let template = binder_owner(owner);
        m.body
            .first()
            .and_then(|first| {
                self.overloaded_method_owners
                    .get(&(template.clone(), first.span))
            })
            .cloned()
            .unwrap_or_else(|| format!("{template}.{}", binder_owner(&m.name)))
    }

    /// [`Self::method_binder_owner`] of `m` on the struct `self_ty` names.
    pub(super) fn method_owner(&self, self_ty: &Ty, m: &Method) -> String {
        match self_ty {
            Ty::Struct(owner, _) => self.method_binder_owner(owner, m),
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
            && let Some(dtype) = self.dtype_named(name)
        {
            return Ok(dtype);
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

    /// The dtype a bare name denotes: a `comptime` binding of one, or a `[dt:
    /// DType]` parameter in scope — symbolic while its declaration is still a
    /// template. The binding table and the parameter scope agree here, so a
    /// `comptime d = dt` binding names the same lane its parameter does.
    pub(super) fn dtype_named(&self, name: &str) -> Option<SimdDtype> {
        if let Some(dtype) = self.comptime_dtypes.get(name) {
            return Some(dtype.clone());
        }
        self.value_parameter_in_scope(name)
            .filter(|expr| expr.meta().as_value() == Some(&Ty::Dtype))
            .map(SimdDtype::Expr)
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
        .filter_map(|decl| type_parameter(decl).map(|ty| (decl.name().to_string(), ty)))
        .collect()
}

/// The `Ty::Param` a type binder's uses carry, `None` for a value binder.
pub(super) fn type_parameter(decl: &ParamDecl) -> Option<Ty> {
    match decl {
        ParamDecl::Type {
            bounds,
            callable_bound,
            ..
        } => Some(Ty::Param {
            binder: decl.binder(),
            bounds: bounds.clone(),
            callable_bound: callable_bound.clone(),
        }),
        ParamDecl::Value { .. } => None,
    }
}

/// A struct's own parameters, as the `TyArg`s they contribute to the struct's
/// `Self` type while its body is checked: a type parameter as `Ty::Param`, a
/// value parameter as a reference to its declaration.
pub(super) fn params_as_args(decls: &[ParamDecl]) -> Vec<TyArg> {
    decls
        .iter()
        .map(|decl| match decl {
            ParamDecl::Type { .. } => {
                TyArg::Ty(type_parameter(decl).expect("a type binder has a type parameter"))
            }
            ParamDecl::Value { ty, .. } => TyArg::Val(value_parameter(decl, ty)),
        })
        .collect()
}

/// The reference to the value binder `decl`, typed `ty`.
pub(super) fn value_parameter(decl: &ParamDecl, ty: &Ty) -> CtValue {
    CtValue::Expr(value_parameter_expr(decl, ty))
}

pub(super) fn value_parameter_expr(decl: &ParamDecl, ty: &Ty) -> ParamExpr {
    value_binder_expr(decl.id().clone(), decl.name(), ty)
}

/// The binder `slot` of a parameterized associated member or generic alias
/// `owner` declares: its own `[params]` are numbered by source position.
pub(super) fn member_binder(owner: &str, slot: usize, name: &str) -> ParamRef {
    ParamRef {
        id: ParamId::new(&binder_owner(owner), slot),
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
                Ty::Param {
                    binder: member_binder(owner, slot, &p.name),
                    bounds: p.bounds.clone(),
                    callable_bound: None,
                },
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

/// Whether a binder is an existential `Some[Trait]` parameter's
/// [`synthetic_binder`]: declared by the callee that spells it and never
/// solved per call.
pub(super) fn existential_binder(binder: &ParamRef) -> bool {
    binder
        .id
        .owner
        .strip_prefix(SYNTHETIC_BINDER_PREFIX)
        .is_some_and(|name| name.starts_with("Some["))
}

/// The lane-shaped view of the wildcard vector binder inside its method's
/// body: `SIMD[$simd.dtype, $simd.size]`, its dtype and width two hidden
/// value binders derived from the binder's identity
/// ([`simd_binder_slots`]). The signature keeps the bare `Ty::Param`, which
/// call-site inference, the `Hasher` conformance check, and the per-call
/// clone names read; only the body sees the vector, whose lane reads
/// (`to_bits[dt]()`, `.length`, `v[i]`) check symbolically, and a clone's
/// derivation folds both slots from its baked argument
/// (`instance_substitution`). `None` for any other type.
pub(super) fn simd_binder_view(ty: &Ty) -> Option<Ty> {
    let Ty::Param { binder, .. } = ty else {
        return None;
    };
    if !simd_wildcard_param(ty) {
        return None;
    }
    let (dtype, size) = simd_binder_slots(binder);
    let context = ParamContext::detached();
    Some(Ty::Simd {
        dtype: SimdDtype::Expr(context.decl_ref(dtype.id, &dtype.name, MetaTy::value(Ty::Dtype))),
        width: SimdWidth::Expr(context.decl_ref(size.id, &size.name, MetaTy::int())),
    })
}

/// The hidden dtype and width value binders of a wildcard vector binder,
/// spelled `$simd.dtype` and `$simd.size` and owned beside the binder
/// itself, so no declaration's own parameter shares either identity.
pub(super) fn simd_binder_slots(binder: &ParamRef) -> (ParamRef, ParamRef) {
    let slot = |suffix: &str, index: usize| ParamRef {
        id: ParamId::new(&format!("{}$lane", binder.id.owner), index),
        name: format!("{}.{suffix}", binder.name).into(),
    };
    (slot("dtype", 0), slot("size", 1))
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

/// The binder owner of every overloaded struct method in `program`: the
/// symbol its template lowers to, keyed by the template struct and the byte
/// range of the method's first body statement, as
/// [`Checker::method_binder_owner`] looks it up. A clone keeps its template's
/// body spans but not its signature, so the owner is computed once, from the
/// template.
pub(super) fn overloaded_method_owners(
    program: &[Stmt],
    sets: &mojito_symbol::symbol::OverloadSets,
) -> HashMap<(String, mojito_common::token::Span), String> {
    program
        .iter()
        .filter_map(|statement| match &statement.kind {
            StmtKind::Struct {
                name,
                type_params,
                methods,
                ..
            } if binder_owner(name) == *name => Some((name, type_params, methods)),
            _ => None,
        })
        .flat_map(|(name, type_params, methods)| {
            methods
                .iter()
                .filter(|m| m.self_ty.is_none() && binder_owner(&m.name) == m.name)
                .filter_map(move |m| {
                    let source =
                        format!("{name}.{}", mojito_symbol::symbol::lifecycle_method_name(m));
                    let lowered = mojito_symbol::symbol::lowered_method_name(
                        &source,
                        type_params,
                        &m.params,
                        m.keyword_only,
                        m.has_self,
                        m.self_convention,
                        sets,
                    );
                    let first = m.body.first()?;
                    (lowered != source).then(|| ((name.clone(), first.span), lowered))
                })
        })
        .collect()
}

/// The checker's diagnostic for a parameter-expression error.
pub(super) fn param_error(error: mojito_types::param_expr::ParamError) -> TypeError {
    use mojito_types::param_expr::ParamError;
    TypeError::NotComptime(match error {
        ParamError::Arithmetic(message) | ParamError::Unsupported(message) => message,
        other => other.to_string(),
    })
}

/// The variadic type packs a declaration's own binders open, as the
/// parameter-list references an element of the pack indexes.
pub(super) fn pack_scope(decls: &[ParamDecl]) -> HashMap<String, ParamExpr> {
    decls
        .iter()
        .filter_map(|decl| match decl {
            ParamDecl::Type {
                name,
                variadic: true,
                ..
            } => Some((
                name.trim_start_matches('*').to_string(),
                ParamContext::detached().decl_ref(
                    decl.id().clone(),
                    name.trim_start_matches('*'),
                    mojito_types::param_expr::MetaTy::type_list(),
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
            } if !matches!(ty.as_ref(), Ty::Func { .. } | Ty::GenericFunc { .. }) => Some((
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
