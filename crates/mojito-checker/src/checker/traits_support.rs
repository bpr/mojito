//! Trait support: method-requirement satisfaction, associated-requirement
//! merging.

use super::overload_support::explicit_parameter_names;
#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::param_expr::fold::comparison_holds;
use mojito_types::types::TyKind;
use mojito_types::types::{FuncTy, TransferSet};

pub(super) use mojito_types::types::same_generator_signature;

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

/// Whether two compile-time arguments are equal: values as the one folder
/// compares them, and a value it does not compare (a tuple, a frozen struct)
/// structurally.
pub(super) fn ty_args_equal(left: &TyArg, right: &TyArg) -> bool {
    match (left, right) {
        (TyArg::Val(left), TyArg::Val(right)) => {
            comparison_holds(InfixOp::Eq, left, right).unwrap_or(left == right)
        }
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
        && (!a.params.is_empty()
            || a.variadic.is_some()
            || a.kw_variadic.is_some()
            || explicit_parameter_names(&a.decls).eq(explicit_parameter_names(&b.decls)))
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
/// position, as [`canonical_method_shape`] names them in its signature: a
/// binder an operand's expression reads (`d.is_floating_point()`) as well
/// as a bare one.
pub(super) fn canonical_availability(method: &MethodSig) -> Vec<GenericConstraint> {
    let canonical_id = |index| ParamId::new(mojito_types::types::CONTRACT_BINDER_OWNER, index);
    let canonical = |reference: &ParamRef| {
        method
            .decls
            .iter()
            .position(|decl| decl.id() == &reference.id)
            .map_or_else(
                || reference.clone(),
                |index| ParamRef {
                    id: canonical_id(index),
                    name: format!("${index}").into(),
                },
            )
    };
    let context = ParamContext::detached();
    let mut renamed = mojito_types::param_expr::ParamBindings::new();
    for (index, decl) in method.decls.iter().enumerate() {
        renamed.bind(
            decl.id().clone(),
            context.decl_ref(
                canonical_id(index),
                &format!("${index}"),
                decl.binder_meta(),
            ),
        );
    }
    let operand = |operand: &ConstraintOperand| match operand {
        ConstraintOperand::Expr(expr) => ConstraintOperand::Expr(
            context
                .replace(expr, &renamed)
                .unwrap_or_else(|_| expr.clone()),
        ),
        other => other.clone(),
    };
    method
        .availability
        .iter()
        .map(|constraint| constraint.map(&canonical, &operand))
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
    let method = &with_existential_binders(method);
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

/// `method` with each existential parameter (`hasher: Some[Hasher]`) bound
/// by the anonymous type binder upstream desugars it to, after the declared
/// ones, so `[H: Hasher](hasher: H)` is the same contract.
fn with_existential_binders(method: &MethodSig) -> MethodSig {
    let mut lifted = method.clone();
    for (slot, param) in lifted.params.iter_mut().enumerate() {
        let TyKind::Param {
            binder,
            bounds,
            callable_bound: None,
        } = param.kind()
        else {
            continue;
        };
        if !is_existential_binder(binder) {
            continue;
        }
        let id = ParamId::new(&binder.id.owner, slot + 1);
        let bounds = bounds.clone();
        *param = TyKind::Param {
            binder: ParamRef {
                id: id.clone(),
                name: binder.name.clone(),
            },
            bounds: bounds.clone(),
            callable_bound: None,
        }
        .into();
        lifted.decls.push(ParamDecl::Type {
            id,
            name: String::new(),
            bounds,
            trait_param: None,
            callable_bound: None,
            default: None,
            infer_only: false,
            variadic: false,
            constraints: Vec::new(),
        });
    }
    lifted
}
