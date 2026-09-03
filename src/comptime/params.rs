//! Comptime-parameter classification, specialization retention,
//! origin markers, and instance mangling.

use super::*;

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
    if matches!(tp.bounds.as_slice(), [only] if only == "Origin" || only == "OriginSet") {
        return true;
    }
    if tp.is_origin_mutability_binder(siblings) {
        return true;
    }
    matches!(
        tp.callable_bound.as_ref(),
        Some(Type::Func { thin: true, .. })
            | Some(Type::Func {
                capturing: Some(_),
                ..
            })
    )
}

/// Classify one source parameter that participates in compile-time evaluation.
/// `None` means the parameter is retained symbolically by specialization.
/// The concrete source type to substitute for a specialization type parameter
/// that is dropped from the clone's signature and calls, or `None` when the
/// parameter must remain symbolic: type packs, callable-value bindings,
/// constrained parameters, and types that do not round-trip to source syntax
/// (such as origin-carrying references). The resolver (`resolve_spec_args_for`)
/// and the clone generator (`generate_def_spec`) must agree on this decision,
/// so both consult this one predicate.
pub(super) fn spec_type_param_substitution(decl: &ParamDecl, value: &CtValue) -> Option<Type> {
    let ParamDecl::Type {
        variadic: false,
        callable_bound: None,
        constraints,
        ..
    } = decl
    else {
        return None;
    };
    if !constraints.is_empty() {
        return None;
    }
    let CtValue::Type(ty) = value else {
        return None;
    };
    source_type_from_ty(ty)
}

/// The registry-aware form of [`classify_ct_param`]: a single bound naming a
/// struct classifies as a struct-typed **value** parameter.
pub(super) fn classify_ct_param_with(
    tp: &TypeParam,
    siblings: &[TypeParam],
    is_value_struct: &dyn Fn(&str) -> bool,
) -> Option<ParamDecl> {
    if let [only] = tp.bounds.as_slice()
        && !retained_specialization_param(tp, siblings)
        && tp.value_type.is_none()
        && ct_value_param_type(only).is_none()
        && is_value_struct(only)
    {
        return Some(ParamDecl::Value {
            name: tp.name.clone(),
            ty: Box::new(Ty::Struct(only.clone(), Vec::new())),
            default: tp.default.as_ref().and_then(ct_expr_from_ast),
            callable_default: None,
            infer_only: tp.infer_only,
            variadic: tp.name.starts_with('*'),
            constraints: Vec::new(),
        });
    }
    classify_ct_param(tp, siblings)
}

pub(super) fn classify_ct_param(tp: &TypeParam, siblings: &[TypeParam]) -> Option<ParamDecl> {
    if retained_specialization_param(tp, siblings) {
        return None;
    }
    if let Some(source_type) = &tp.value_type
        && let Some(ty) = ct_param_source_type(source_type)
    {
        return Some(ParamDecl::Value {
            name: tp.name.clone(),
            ty: Box::new(ty),
            default: tp.default.as_ref().and_then(ct_expr_from_ast),
            callable_default: None,
            infer_only: tp.infer_only,
            variadic: tp.name.starts_with('*'),
            constraints: Vec::new(),
        });
    }
    if let [only] = tp.bounds.as_slice()
        && let Some(ty) = ct_value_param_type(only)
    {
        return Some(ParamDecl::Value {
            name: tp.name.clone(),
            ty: Box::new(ty),
            default: tp.default.as_ref().and_then(ct_expr_from_ast),
            callable_default: None,
            infer_only: tp.infer_only,
            variadic: tp.name.starts_with('*'),
            constraints: Vec::new(),
        });
    }
    Some(ParamDecl::Type {
        name: tp.name.clone(),
        bounds: tp.bounds.clone(),
        callable_bound: None,
        default: tp.default.as_ref().and_then(|value| match &value.kind {
            ExprKind::Identifier(name) => scalar_type_name(name).map(Box::new),
            ExprKind::TypeValue(ty) => ct_param_source_type(ty).map(Box::new),
            _ => None,
        }),
        infer_only: tp.infer_only,
        variadic: tp.name.starts_with('*'),
        constraints: Vec::new(),
    })
}

pub(super) fn decode_ct_origin_marker(value: &CtValue) -> Option<crate::origin::RefTy> {
    let CtValue::Param(marker) = value else {
        return None;
    };
    let marker = marker.strip_prefix("$tuple-origin:")?;
    let (index, permission) = marker.split_once(':')?;
    let id = crate::origin::OriginParamId(index.parse().ok()?);
    let mutability = match permission {
        "imm" => crate::origin::Mutability::Immutable,
        "mut" => crate::origin::Mutability::Mutable,
        "param" => crate::origin::Mutability::Param(id),
        _ => return None,
    };
    Some(crate::origin::RefTy {
        // Filled by `type_from_anno` after the marker establishes provenance.
        referent: Box::new(Ty::None),
        origin: crate::origin::Origin::Param(id),
        mutability,
    })
}

pub(super) fn ct_value_param_type(name: &str) -> Option<Ty> {
    Some(match name {
        "Int" => Ty::Int,
        // A `[dtype: DType]` value parameter; compile-time-only.
        "DType" => Ty::Dtype,
        // A SIMD width parameter is a compile-time Int value parameter (the
        // removed `SIMDSize` spelling rejects).
        "SIMDLength" => Ty::Int,
        "Bool" => Ty::Bool,
        "String" => Ty::StringLiteral,
        "StringLiteral" => Ty::StringLiteral,
        "UInt" => Ty::UInt,
        "Float64" => Ty::Float64,
        // The prelude rewrite qualifies `String` bounds like any other name;
        // a `[text: String]` value parameter keeps the compile-time string
        // type regardless of the nominal stdlib struct.
        _ if crate::symbol::is_stdlib_string_struct(name) => Ty::StringLiteral,
        _ => return None,
    })
}

/// CTFE does not evaluate an Origin as a runtime value, but nested type
/// annotations still need its stable declaration-order identity while the
/// monomorphizer resolves a variadic Tuple element pack. Encode that semantic
/// fact in the existing non-materializable `Param` carrier for the duration of
/// the enclosing struct walk.
pub(super) fn ct_origin_marker(index: usize, mutability: crate::origin::Mutability) -> CtValue {
    let permission = match mutability {
        crate::origin::Mutability::Immutable => "imm",
        crate::origin::Mutability::Mutable => "mut",
        crate::origin::Mutability::Param(_) => "param",
    };
    CtValue::Param(format!("$tuple-origin:{index}:{permission}"))
}

pub(super) fn ct_value_has_type(value: &CtValue, ty: &Ty) -> bool {
    materialize_ct_value(value.clone(), ty).is_some()
}
