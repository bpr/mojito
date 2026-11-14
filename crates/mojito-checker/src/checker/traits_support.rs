//! Trait support: method-requirement satisfaction, associated-requirement
//! merging.

use super::overload_support::explicit_parameter_names;
#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::param_expr::fold::comparison_holds;
use mojito_types::types::{FuncTy, TransferSet};
use mojito_types::types::{GeneratorTy, TyKind};

pub(super) use mojito_types::types::same_generator_signature;

/// Compose inherited associated-member requirements, as upstream merges
/// two traits' declarations of one member. Unparameterized type-valued
/// members denote one associated type, so refinement accumulates their
/// bounds instead of treating stronger composition as an ambiguity. Every
/// other requirement has a type of its own (a value's type, or a
/// parameterized member's `__generator_type`) that the other declaration
/// must restate exactly, parameter names included; anything else is
/// upstream's "invalid redefinition".
pub(super) fn merge_associated_requirement(
    existing: &mut CtMemberReq,
    incoming: &CtMemberReq,
    member: &str,
) -> Result<(), TypeError> {
    let agrees = match (&mut *existing, incoming) {
        (
            CtMemberReq::Type { bounds, params },
            CtMemberReq::Type {
                bounds: more,
                params: more_params,
            },
        ) if params.is_empty() && more_params.is_empty() => {
            for bound in more {
                if !bounds.contains(bound) {
                    bounds.push(bound.clone());
                }
            }
            true
        }
        (CtMemberReq::Value(ty), CtMemberReq::Value(more)) => {
            match (ty.as_generator(), more.as_generator()) {
                (Some(generator), Some(more)) => same_generator_type(generator, more),
                _ => ty == more,
            }
        }
        (CtMemberReq::Type { .. }, CtMemberReq::Type { .. }) => *existing == *incoming,
        _ => false,
    };
    if agrees {
        return Ok(());
    }
    Err(TypeError::InvalidRedefinition {
        name: member.to_string(),
        found: requirement_type(existing),
        expected: requirement_type(incoming),
    })
}

/// `constraint` over the binders `from_owner` owns restated over `to`'s,
/// slot by slot, as a conformer's member restates a requirement's clause
/// over its own parameters, names aside.
pub(super) fn rebind_slots(
    constraint: &GenericConstraint,
    from_owner: &str,
    to: &[ParamDecl],
) -> GenericConstraint {
    let context = ParamContext::detached();
    let mut renamed = mojito_types::param_expr::ParamBindings::new();
    for target in to {
        renamed.bind(
            ParamId::new(from_owner, target.id().slot),
            context.decl_ref(target.id().clone(), target.name(), target.binder_meta()),
        );
    }
    let binder = |reference: &ParamRef| match to
        .iter()
        .find(|target| target.id().slot == reference.id.slot)
    {
        Some(target) if *reference.id.owner == *from_owner => ParamRef {
            id: target.id().clone(),
            name: target.name().into(),
        },
        _ => reference.clone(),
    };
    let operand = |operand: &ConstraintOperand| match operand {
        ConstraintOperand::Expr(expr) => ConstraintOperand::Expr(
            context
                .replace(expr, &renamed)
                .unwrap_or_else(|_| expr.clone()),
        ),
        other => other.clone(),
    };
    constraint.map(&binder, &operand)
}

/// A generator's result type `result`, stated over the binders `from`,
/// restated over `to` slot by slot, as two generator types compare with
/// their binder lists aligned by position (`comptime get[n: Int]:
/// SIMD[DType.int32, n]` in a trait and in its conformer). A result that
/// does not restate keeps its own binders.
pub(super) fn rebind_generator_result(result: &Ty, from: &[ParamDecl], to: &[ParamDecl]) -> Ty {
    let context = ParamContext::detached();
    let mut renamed = mojito_types::param_expr::ParamBindings::new();
    for (source, target) in from.iter().zip(to) {
        renamed.bind(
            source.id().clone(),
            context.decl_ref(target.id().clone(), target.name(), target.binder_meta()),
        );
    }
    mojito_types::types::replace_parameters(&context, result, &renamed, 0)
        .unwrap_or_else(|_| result.clone())
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
        && parameter_packs(a).eq(parameter_packs(b))
}

/// Which explicit compile-time parameters of `sig` are packs, part of its
/// symbol (`SignatureKey`'s `packs`): `shuffle[*mask: SIMDLength]` overloads
/// `shuffle[mask: IndexList[…]]`.
fn parameter_packs(sig: &MethodSig) -> impl Iterator<Item = bool> + '_ {
    explicit_parameter_names(&sig.decls).map(|name| name.starts_with('*'))
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
    TyKind::Func(Box::new(method_func_ty(
        method,
        mojito_types::origin::CallableEnvironment::Default,
    )))
    .into()
}

/// Whether `method`, a read-receiver `__call__`, answers a struct's
/// `def(...)` conformance `contract`: a generic contract's binders must be
/// its own, up to spelling, over a mutually coercible signature.
pub(super) fn call_method_matches_contract(method: &MethodSig, contract: &Ty) -> bool {
    if matches!(contract.kind(), TyKind::GenericFunc(_)) {
        return generic_callable_shapes_match(&method_generic_callable_ty(method), contract);
    }
    let actual = method_callable_ty(method);
    coerces(&actual, contract) && coerces(contract, &actual)
}

/// The function type of `method`'s call surface, its receiver excluded,
/// in `environment`.
pub(super) fn method_func_ty(
    method: &MethodSig,
    environment: mojito_types::origin::CallableEnvironment,
) -> FuncTy {
    FuncTy {
        environment,
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
    }
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
        let binder = existential_slot_binder(binder, slot);
        let id = binder.id.clone();
        let bounds = bounds.clone();
        *param = TyKind::Param {
            binder,
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

/// A requirement's type as upstream spells it in a redefinition error.
fn requirement_type(requirement: &CtMemberReq) -> String {
    match requirement {
        CtMemberReq::Value(ty) => ty.to_string(),
        CtMemberReq::Type { bounds, params } => {
            let bounds = if bounds.is_empty() {
                "AnyType".to_string()
            } else {
                bounds.join(" & ")
            };
            if params.is_empty() {
                return bounds;
            }
            let params = params
                .iter()
                .map(|param| match param.bounds.as_slice() {
                    [] => param.name.clone(),
                    bounds => format!("{}: {}", param.name, bounds.join(" & ")),
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("__generator_type[{params}] {bounds}")
        }
    }
}

/// `method`'s call surface as a callable contract with its own binders:
/// the generic function type a `def[...]` contract's `__call__` must match.
fn method_generic_callable_ty(method: &MethodSig) -> Ty {
    let func = method_func_ty(method, mojito_types::origin::CallableEnvironment::Default);
    if method.decls.is_empty() {
        return TyKind::Func(Box::new(func)).into();
    }
    TyKind::GenericFunc(Box::new(GenericFuncTy {
        decls: method.decls.clone(),
        func,
    }))
    .into()
}

/// Whether two generic callables are one contract: the same binders up to
/// spelling, each with the same constraints, over mutually coercible
/// signatures.
fn generic_callable_shapes_match(actual: &Ty, expected: &Ty) -> bool {
    use mojito_types::types::erase_generic_callable_binders;
    erase_generic_callable_binders(actual)
        .zip(erase_generic_callable_binders(expected))
        .is_some_and(|((actual_decls, actual), (expected_decls, expected))| {
            actual_decls == expected_decls
                && coerces(&actual, &expected)
                && coerces(&expected, &actual)
        })
}

/// Whether two declarations of one parameterized value requirement state
/// the same generator type: the same signature position by position, the
/// same binder names (the type spells them, so a refinement may not rename
/// one), and one result over the binders both own. Their `where` clauses
/// compare as every inherited member's constraints do, by implication.
fn same_generator_type(left: &GeneratorTy, right: &GeneratorTy) -> bool {
    left.result == right.result
        && same_generator_signature(&left.params, &right.params)
        && left
            .params
            .iter()
            .zip(&right.params)
            .all(|(left, right)| left.name() == right.name())
}
