//! Comptime-parameter classification, specialization retention,
//! origin markers, and instance mangling.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::TyKind;

/// Whether a source parameter is semantic metadata/runtime callable input rather
/// than a value the compile-time evaluator may inspect.  These parameters stay
/// on every generated specialization, and their call arguments stay at the
/// rewritten call site.
///
/// An unqualified `F: def(...)` is a callable *type constraint* and therefore is
/// still an ordinary type parameter.  Mojo's explicit `thin`/`capturing[...]`
/// forms declare a compile-time callable value; evaluating that value as a
/// [`CtValue`] would incorrectly require the compile-time universe to own VM
/// closures and captured storage.
pub(super) fn retained_specialization_param(tp: &TypeParam, siblings: &[TypeParam]) -> bool {
    if tp.is_erased_origin_param(siblings) {
        return true;
    }
    matches!(
        tp.callable_bound.as_ref(),
        Some(
            Type::Func { thin: true, .. }
                | Type::Func {
                    capturing: Some(_),
                    ..
                }
        )
    )
}

/// Classify one of `owner`'s source parameters that participates in
/// compile-time evaluation; `None` means specialization retains it
/// symbolically (see [`retained_specialization_param`]).
pub(super) fn classify_ct_param(
    tp: &TypeParam,
    siblings: &[TypeParam],
    owner: &str,
) -> Option<ParamDecl> {
    if retained_specialization_param(tp, siblings) {
        return None;
    }
    let value = |ty: Ty| ParamDecl::Value {
        id: elaborated_binder(tp, siblings, owner),
        name: tp.name.clone(),
        ty: ParamDecl::value_list_ty(ty, tp.name.starts_with('*')),
        default: tp
            .default
            .as_ref()
            .and_then(|default| ct_expr_from_ast(default, siblings, owner)),
        callable_default: None,
        infer_only: tp.infer_only,
        variadic: tp.name.starts_with('*'),
        constraints: Vec::new(),
    };
    if let Some(source_type) = &tp.value_type
        && let Some(ty) = ct_param_source_type(source_type)
    {
        return Some(value(ty));
    }
    if let Some(element) = tp
        .value_type
        .as_ref()
        .and_then(|source| param_list_element(tp, source, siblings, owner))
    {
        return Some(value(TyKind::ParamList(element).into()));
    }
    if let [only] = tp.bounds.as_slice()
        && let Some(ty) =
            ct_value_param_type(only).or_else(|| sibling_type_binder(tp, only, siblings, owner))
    {
        return Some(value(ty));
    }
    let trait_param = tp
        .bounds
        .iter()
        .find_map(|bound| sibling_trait_binder(tp, bound, siblings, owner));
    let bounds = match &trait_param {
        Some((binder, declared)) => declared
            .iter()
            .chain(tp.bounds.iter().filter(|bound| **bound != *binder.name))
            .fold(Vec::new(), |mut bounds, bound| {
                if !bounds.contains(bound) {
                    bounds.push(bound.clone());
                }
                bounds
            }),
        None => tp.bounds.clone(),
    };
    Some(ParamDecl::Type {
        id: elaborated_binder(tp, siblings, owner),
        name: tp.name.clone(),
        bounds,
        trait_param: trait_param.map(|(binder, _)| binder),
        callable_bound: None,
        default: tp.default.as_ref().and_then(|value| match &value.kind {
            ExprKind::Identifier(name) => scalar_type_name(name),
            ExprKind::TypeValue(ty) => ct_param_source_type(ty),
            _ => None,
        }),
        infer_only: tp.infer_only,
        variadic: tp.name.starts_with('*'),
        constraints: Vec::new(),
    })
}

/// The identity of an elaborator-classified binder, as the checker numbers
/// it: the declaration `owner` names — its template, so a `$` clone shares
/// its template's binders; a method's is
/// [`mojito_symbol::symbol::MethodBinderOwners::owner`] — and the binder's
/// slot among its siblings, erased origin parameters skipped.
pub(super) fn elaborated_binder(
    tp: &TypeParam,
    siblings: &[TypeParam],
    owner: &str,
) -> mojito_types::param_expr::ParamId {
    mojito_types::param_expr::ParamId::new(
        mojito_symbol::symbol::specialization_template(owner).unwrap_or(owner),
        tp.binder_slot(siblings),
    )
}

pub(super) fn ct_value_param_type(name: &str) -> Option<Ty> {
    Some(match name {
        "Int" => Ty::int(),
        // A `DType` value, and the type of a `[dtype: DType]` value parameter.
        "DType" => Ty::dtype(),
        // A SIMD width parameter is a compile-time Int value parameter (the
        // removed `SIMDSize` spelling rejects).
        "SIMDLength" => Ty::int(),
        "Bool" => Ty::bool(),
        "String" => Ty::string_literal(),
        "StringLiteral" => Ty::string_literal(),
        "UInt" => Ty::uint(),
        "Float64" => Ty::float64(),
        // The prelude rewrite qualifies `String` bounds like any other name;
        // a `[text: String]` value parameter keeps the compile-time string
        // type regardless of the nominal stdlib struct.
        _ if mojito_symbol::symbol::is_stdlib_string_struct(name) => Ty::string_literal(),
        _ => return None,
    })
}

/// The `(dtype, width)` of a source `SIMD[DType.d, w]` argument list, or
/// `None` when either argument is not a literal dtype member / width.
pub(super) fn simd_source_dims(args: &[ParamArg]) -> Option<(mojito_ast::ast::Dtype, i64)> {
    let [dtype, width] = args else {
        return None;
    };
    let ParamArg::Value(Expr {
        kind: ExprKind::Member { object, field },
        ..
    }) = dtype
    else {
        return None;
    };
    if !matches!(&object.kind, ExprKind::Identifier(name) if name == "DType") {
        return None;
    }
    let dtype = mojito_ast::ast::Dtype::from_name(field)?;
    let ParamArg::Value(Expr {
        kind: ExprKind::Int(width),
        ..
    }) = width
    else {
        return None;
    };
    let width = width.wrapping_signed(64)?;
    (width >= 1 && (width & (width - 1)) == 0).then_some((dtype, width))
}

/// The trait-valued binder `name` (`Tr: type_of(Copyable)`) declared before
/// `tp`, with its declared trait set: a bound `T: Tr` is that binder's
/// symbolic bound, as the checker classifies it.
fn sibling_trait_binder(
    tp: &TypeParam,
    name: &str,
    siblings: &[TypeParam],
    owner: &str,
) -> Option<(mojito_types::param_expr::ParamRef, Vec<String>)> {
    let position = siblings
        .iter()
        .position(|sibling| sibling.name == tp.name)?;
    let sibling = siblings[..position]
        .iter()
        .find(|sibling| sibling.name == name)?;
    let ParamDecl::Value { id, ty, .. } = classify_ct_param(sibling, siblings, owner)? else {
        return None;
    };
    let TyKind::AnyTrait(bound) = ty.kind() else {
        return None;
    };
    Some((
        mojito_types::param_expr::ParamRef {
            id,
            name: std::sync::Arc::from(name),
        },
        bound.clone(),
    ))
}

/// The element type of upstream's raw parameter list annotation,
/// `_MLIR.KGENParamListType[T]`, as the checker resolves it: a trait-valued
/// sibling binder (`TypeList`'s `Trait`) as the dependent type it names, or a
/// type binder or scalar type.
fn param_list_element(
    tp: &TypeParam,
    source: &Type,
    siblings: &[TypeParam],
    owner: &str,
) -> Option<Ty> {
    let Type::IndexedProjection { base, index } = source else {
        return None;
    };
    if !matches!(&**base, Type::Assoc { name, .. } if name == "KGENParamListType") {
        return None;
    }
    let ExprKind::Identifier(element) = &index.kind else {
        return None;
    };
    let position = siblings
        .iter()
        .position(|sibling| sibling.name == tp.name)?;
    let trait_binder = siblings[..position]
        .iter()
        .find(|sibling| sibling.name == *element)
        .and_then(|sibling| classify_ct_param(sibling, siblings, owner));
    if let Some(ParamDecl::Value { id, ty, .. }) = trait_binder
        && matches!(ty.kind(), TyKind::AnyTrait(_))
    {
        let reference = mojito_types::param_expr::ParamContext::detached().decl_ref(
            id,
            element,
            mojito_types::param_expr::MetaTy::Value(ty),
        );
        return Some(mojito_types::types::DependentType::resolve(reference));
    }
    scalar_type_name(element).or_else(|| sibling_type_binder(tp, element, siblings, owner))
}

/// The `TyKind::Param` of the type binder `name` when it is a non-variadic
/// sibling declared before `tp`: a value parameter of that type
/// (`[T: AnyType, //, v: T]`), as the checker classifies it.
fn sibling_type_binder(
    tp: &TypeParam,
    name: &str,
    siblings: &[TypeParam],
    owner: &str,
) -> Option<Ty> {
    let position = siblings
        .iter()
        .position(|sibling| sibling.name == tp.name)?;
    let sibling = siblings[..position]
        .iter()
        .find(|sibling| sibling.name == name)?;
    let ParamDecl::Type {
        id,
        bounds,
        variadic: false,
        ..
    } = classify_ct_param(sibling, siblings, owner)?
    else {
        return None;
    };
    Some(
        TyKind::Param {
            binder: mojito_types::param_expr::ParamRef {
                id,
                name: std::sync::Arc::from(name),
            },
            bounds,
            callable_bound: None,
        }
        .into(),
    )
}
