//! Trait support: method-requirement satisfaction, associated-requirement
//! merging.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::TyKind;
use mojito_types::types::{FuncTy, TransferSet};

/// Compose inherited associated-member requirements. Type-valued members with
/// the same name denote one associated type, so refinement accumulates their
/// bounds instead of treating stronger composition as an ambiguity. Value
/// members must retain one exact type; mixing value and type requirements is a
/// real conflict.
pub(super) fn merge_associated_requirement(
    existing: &mut CtMemberReq,
    incoming: &CtMemberReq,
    member: &str,
) -> Result<(), TypeError> {
    match (existing, incoming) {
        (
            CtMemberReq::Type { bounds, params },
            CtMemberReq::Type {
                bounds: more,
                params: more_params,
            },
        ) => {
            // A refined associated type must keep the same parameterization.
            if !params.is_empty() && !more_params.is_empty() && params != more_params {
                return Err(TypeError::Unsupported(format!(
                    "refined associated type '{member}' changes its parameter list"
                )));
            }
            if params.is_empty() {
                params.clone_from(more_params);
            }
            for bound in more {
                if !bounds.contains(bound) {
                    bounds.push(bound.clone());
                }
            }
            Ok(())
        }
        (CtMemberReq::Value(left), CtMemberReq::Value(right)) if left == right => Ok(()),
        (
            CtMemberReq::Generator { params, result },
            CtMemberReq::Generator {
                params: more_params,
                result: more_result,
            },
        ) if result == more_result && same_generator_signature(params, more_params) => Ok(()),
        _ => Err(TypeError::Unsupported(format!(
            "conflicting inherited associated member '{member}'"
        ))),
    }
}

/// Whether two generators' parameter lists declare the same signature:
/// position by position the same kind, declared type or bounds,
/// infer-only and variadic markers, as upstream compares generator types.
/// Names are the declarations' own and do not take part.
pub(super) fn same_generator_signature(left: &[ParamDecl], right: &[ParamDecl]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|pair| match pair {
            (
                ParamDecl::Type {
                    bounds,
                    infer_only,
                    variadic,
                    ..
                },
                ParamDecl::Type {
                    bounds: more_bounds,
                    infer_only: more_infer_only,
                    variadic: more_variadic,
                    ..
                },
            ) => {
                bounds.len() == more_bounds.len()
                    && bounds.iter().all(|bound| more_bounds.contains(bound))
                    && infer_only == more_infer_only
                    && variadic == more_variadic
            }
            (
                ParamDecl::Value {
                    ty,
                    infer_only,
                    variadic,
                    ..
                },
                ParamDecl::Value {
                    ty: more_ty,
                    infer_only: more_infer_only,
                    variadic: more_variadic,
                    ..
                },
            ) => ty == more_ty && infer_only == more_infer_only && variadic == more_variadic,
            _ => false,
        })
}

pub(super) fn compare_ct_integers(op: InfixOp, left: &CtValue, right: &CtValue) -> Option<bool> {
    let (left, right) = (ct_integer(left)?, ct_integer(right)?);
    Some(match op {
        InfixOp::Eq => left == right,
        InfixOp::Ne => left != right,
        InfixOp::Lt => left < right,
        InfixOp::Le => left <= right,
        InfixOp::Gt => left > right,
        InfixOp::Ge => left >= right,
        _ => return None,
    })
}

pub(super) fn ty_args_equal(left: &TyArg, right: &TyArg) -> bool {
    match (left, right) {
        (TyArg::Val(left), TyArg::Val(right)) => ct_values_equal(left, right),
        _ => left == right,
    }
}

pub(super) fn same_method_shape(a: &MethodSig, b: &MethodSig) -> bool {
    // Keyword-only parameter NAMES are part of overload identity: two
    // signatures with identical types may still be distinct overloads when
    // their keyword-only selectors differ (`s[byte=i]` vs `s[codepoint=i]`).
    let keyword_names = |sig: &MethodSig| match sig.keyword_only {
        Some(index) => sig.names[index..].to_vec(),
        None => Vec::new(),
    };
    method_arity_range(a) == method_arity_range(b)
        && symbol_equivalent_params(&a.params, &b.params)
        && a.variadic == b.variadic
        && a.kw_variadic == b.kw_variadic
        && keyword_names(a) == keyword_names(b)
}

/// Current Mojo rejects a `__setitem__` pair whose assignment value is the
/// final positional parameter in one overload and a keyword-only parameter in
/// the other over the same index types: selection would otherwise depend on
/// the assignment's right-hand side.
pub(super) fn competing_setitem_value_shapes(a: &MethodSig, b: &MethodSig) -> bool {
    pub(super) fn positional_value_indices(sig: &MethodSig) -> Option<&[Ty]> {
        (sig.keyword_only.is_none()
            && sig.variadic.is_none()
            && sig.kw_variadic.is_none()
            && !sig.params.is_empty())
        .then(|| &sig.params[..sig.params.len() - 1])
    }
    pub(super) fn keyword_value_indices(sig: &MethodSig) -> Option<&[Ty]> {
        let keyword_only = sig.keyword_only?;
        (sig.variadic.is_none() && sig.kw_variadic.is_none() && sig.names.len() == keyword_only + 1)
            .then(|| &sig.params[..keyword_only])
    }
    pub(super) fn competes(positional: &MethodSig, keyword: &MethodSig) -> bool {
        matches!(
            (
                positional_value_indices(positional),
                keyword_value_indices(keyword),
            ),
            (Some(left), Some(right)) if symbol_equivalent_params(left, right)
        )
    }
    competes(a, b) || competes(b, a)
}

/// A conforming method may promise no error where its trait requirement raises,
/// but a raising implementation must preserve the exact declared error family.
/// Bare `raises` denotes `Error`; it is not a wildcard for a distinct typed
/// error. `raises Never` is already normalized to a non-raising signature when
/// `MethodSig` is built.
///
/// Defaults are the witness's own: a call through the bound binds each slot
/// it leaves out from the requirement's default (`bound_default_arguments`),
/// so a witness may default differently, or not at all. So is its
/// positional-only marker: the pin lets `__lt__(self, rhs: Self, /)`
/// witness `Comparable.__lt__(self, rhs: Self)`, and the reverse.
pub(super) fn method_satisfies_requirement(got: &MethodSig, required: &MethodSig) -> bool {
    let mut got_shape = canonical_method_shape(got);
    got_shape.raises = false;
    got_shape.error = None;
    got_shape.overload = None;
    got_shape.required.clone_from(&required.required);
    got_shape.defaults.clone_from(&required.defaults);
    got_shape.positional_only = required.positional_only;
    got_shape.synthesized_default = required.synthesized_default;
    // A collector is never bound by name, so a witness may rename it, and a
    // call through the bound binds a positional parameter by the
    // requirement's name, so the witness may rename that too.
    got_shape.variadic_name.clone_from(&required.variadic_name);
    let positional = required.keyword_only.unwrap_or(required.names.len());
    if got_shape.names.len() == required.names.len() {
        got_shape.names[..positional].clone_from_slice(&required.names[..positional]);
    }
    let mut required_shape = canonical_method_shape(required);
    required_shape.raises = false;
    required_shape.error = None;
    required_shape.overload = None;
    if got_shape != required_shape {
        return false;
    }
    if !got.raises {
        return true;
    }
    if !required.raises {
        return false;
    }
    got.error == required.error
}

/// A method's availability clauses with its own binders named by their
/// position, as [`canonical_method_shape`] names them in its signature.
pub(super) fn canonical_availability(method: &MethodSig) -> Vec<GenericConstraint> {
    let canonical = |reference: &ParamRef| {
        method
            .decls
            .iter()
            .position(|decl| decl.id() == &reference.id)
            .map_or_else(
                || reference.clone(),
                |index| ParamRef {
                    id: ParamId::new(mojito_types::types::CONTRACT_BINDER_OWNER, index),
                    name: format!("${index}").into(),
                },
            )
    };
    method
        .availability
        .iter()
        .map(|constraint| constraint.map(&canonical, &ConstraintOperand::clone))
        .collect()
}

pub(super) fn method_callable_ty(method: &MethodSig) -> Ty {
    TyKind::Func(Box::new(FuncTy {
        environment: mojito_types::origin::CallableEnvironment::Default,
        params: method.params.clone(),
        names: method.names.clone(),
        ret: method.ret.clone(),
        required: method.required.clone(),
        variadic: method.variadic.clone(),
        kw_variadic: method.kw_variadic.clone(),
        positional_only: method.positional_only,
        keyword_only: method.keyword_only,
        raises: method.raises,
        error: method.error.clone(),
        conventions: method.conventions.clone(),
        ref_params: method.ref_params.clone(),
        ref_return: method.ref_return.clone().map(Box::new),
        transfers: TransferSet::default(),
    }))
    .into()
}

/// A method's contract with its own binders canonicalized to signature slots
/// (`canonical_generic_signature`): a witness spelled `def push[X: Hasher]`
/// satisfies a requirement spelled `def push[H: Hasher]`, since a binder's
/// identity is its declaration's and its spelling is not part of the shape.
fn canonical_method_shape(method: &MethodSig) -> MethodSig {
    if method.decls.is_empty() {
        return method.clone();
    }
    let mut types = method.params.clone();
    types.push(method.ret.clone());
    types.extend(method.variadic.clone());
    types.extend(method.kw_variadic.clone());
    types.extend(method.error.clone());
    let (decls, mut types) =
        mojito_types::types::canonical_generic_signature(&method.decls, &types);
    let mut shape = method.clone();
    shape.decls = decls;
    shape.error = method
        .error
        .is_some()
        .then(|| types.pop().expect("error type"));
    shape.kw_variadic = method
        .kw_variadic
        .is_some()
        .then(|| types.pop().expect("keyword collector type"));
    shape.variadic = method
        .variadic
        .is_some()
        .then(|| types.pop().expect("collector type"));
    shape.ret = types.pop().expect("return type");
    shape.params = types;
    shape
}
