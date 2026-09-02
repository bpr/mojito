//! Trait support: default-method expansion, method-requirement
//! satisfaction, associated-requirement merging, and the built-in
//! trait table.

use super::*;

/// Materialize trait default methods into each conforming struct before semantic
/// checking. This keeps default dispatch static: downstream MIR sees an ordinary
/// struct method and needs no trait-object runtime machinery.
pub(super) fn expand_trait_defaults(stmts: &[Stmt]) -> Result<Vec<Stmt>, TypeError> {
    #[derive(Clone)]
    struct TraitDefaults {
        refines: Vec<String>,
        methods: Vec<crate::ast::TraitMethod>,
    }

    pub(super) fn defaults_for(
        name: &str,
        traits: &HashMap<String, TraitDefaults>,
        visiting: &mut HashSet<String>,
    ) -> Result<HashMap<String, Method>, TypeError> {
        if !visiting.insert(name.to_string()) {
            return Err(TypeError::Unsupported(format!(
                "cyclic trait refinement involving '{name}'"
            )));
        }
        let Some(info) = traits.get(name) else {
            visiting.remove(name);
            return Ok(HashMap::new());
        };
        let mut defaults = HashMap::new();
        for parent in &info.refines {
            for (method, implementation) in defaults_for(parent, traits, visiting)? {
                if defaults.insert(method.clone(), implementation).is_some() {
                    return Err(TypeError::Unsupported(format!(
                        "ambiguous inherited default method '{method}'"
                    )));
                }
            }
        }
        for method in &info.methods {
            let Some(body) = &method.default_body else {
                continue;
            };
            defaults.insert(
                method.name.clone(),
                Method {
                    name: method.name.clone(),
                    type_params: method.type_params.clone(),
                    has_self: true,
                    self_convention: method.self_convention,
                    self_origin: method.self_origin.clone(),
                    decorators: Vec::new(),
                    params: method.params.clone(),
                    positional_only: method.positional_only,
                    keyword_only: method.keyword_only,
                    raises: method.raises,
                    raises_type: method.raises_type.clone(),
                    ret: method.ret.clone(),
                    body: body.clone(),
                    where_clauses: method.where_clauses.clone(),
                },
            );
        }
        visiting.remove(name);
        Ok(defaults)
    }

    let traits: HashMap<_, _> = stmts
        .iter()
        .filter_map(|stmt| match &stmt.kind {
            StmtKind::Trait {
                name,
                refines,
                methods,
                ..
            } => Some((
                name.clone(),
                TraitDefaults {
                    refines: refines.clone(),
                    methods: methods.clone(),
                },
            )),
            _ => None,
        })
        .collect();
    let mut expanded = stmts.to_vec();
    for stmt in &mut expanded {
        let StmtKind::Struct {
            conforms, methods, ..
        } = &mut stmt.kind
        else {
            continue;
        };
        let explicit: HashSet<_> = methods.iter().map(|method| method.name.clone()).collect();
        let mut inherited = HashMap::<String, Method>::new();
        for trait_name in conforms.iter() {
            for (name, implementation) in defaults_for(trait_name, &traits, &mut HashSet::new())? {
                if explicit.contains(&name) {
                    continue;
                }
                if inherited.insert(name.clone(), implementation).is_some() {
                    return Err(TypeError::Unsupported(format!(
                        "ambiguous default method '{name}'; provide an explicit override"
                    )));
                }
            }
        }
        methods.extend(inherited.into_values());
    }
    Ok(expanded)
}

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
                *params = more_params.clone();
            }
            for bound in more {
                if !bounds.contains(bound) {
                    bounds.push(bound.clone());
                }
            }
            Ok(())
        }
        (CtMemberReq::Value(left), CtMemberReq::Value(right)) if left == right => Ok(()),
        _ => Err(TypeError::Unsupported(format!(
            "conflicting inherited associated member '{member}'"
        ))),
    }
}

pub(super) fn conformance_operand(
    expression: &Expr,
    arguments: &HashMap<&str, &TyArg>,
) -> Option<CtValue> {
    match &expression.kind {
        ExprKind::Int(value) => Some(CtValue::IntLiteral(value.clone())),
        ExprKind::Bool(value) => Some(CtValue::Bool(*value)),
        ExprKind::Str(value) => Some(CtValue::Str(value.clone())),
        ExprKind::Identifier(name) => match arguments.get(name.as_str())? {
            TyArg::Val(value) => Some((*value).clone()),
            TyArg::Ty(_) | TyArg::Origin(_) => None,
        },
        _ => None,
    }
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
pub(super) fn method_satisfies_requirement(got: &MethodSig, required: &MethodSig) -> bool {
    let mut got_shape = got.clone();
    got_shape.raises = false;
    got_shape.error = None;
    let mut required_shape = required.clone();
    required_shape.raises = false;
    required_shape.error = None;
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

pub(super) fn method_callable_ty(method: &MethodSig) -> Ty {
    Ty::Func {
        environment: crate::origin::CallableEnvironment::Default,
        params: method.params.clone(),
        names: method.names.clone(),
        ret: Box::new(method.ret.clone()),
        required: method.required.clone(),
        variadic: method.variadic.clone(),
        kw_variadic: method.kw_variadic.clone(),
        positional_only: method.positional_only,
        keyword_only: method.keyword_only,
        raises: method.raises,
        error: method.error.clone(),
        conventions: method.conventions.clone(),
        ref_params: Box::new(method.ref_params.clone()),
        ref_return: method.ref_return.clone().map(Box::new),
        transfers: Default::default(),
    }
}

/// Mojo's built-in traits that mojito recognizes in a type-parameter bound.
/// User-defined traits (and conformance checking) are a later phase, so a bound
/// must name one of these. `AnyType` is the least restrictive.
pub(super) const BUILTIN_TRAITS: &[&str] = &[
    "AnyType",
    "Deinitable",
    "Movable",
    "Copyable",
    "ImplicitlyCopyable",
    "RegisterPassable",
    "TrivialRegisterPassable",
    "Defaultable",
    "Representable",
    "Writable",
    "Writer",
    "Boolable",
    "Intable",
    "Floatable",
    "Indexer",
    "Equatable",
    "Comparable",
    "Hashable",
    "Hasher",
    "Identifiable",
    "Sized",
    "SizedRaising",
    "Iterable",
    "IterableOwned",
    "Iterator",
    "Absable",
    "Powable",
    "Roundable",
    "Ceilable",
    "Floorable",
    "Truncable",
    "CeilDivable",
    "CeilDivableRaising",
    "DivModable",
    "Addable",
    "Subtractable",
    "Multipliable",
    "Divisible",
    "FloorDivisible",
    "Modable",
    "ShiftLeftable",
    "ShiftRightable",
    "Andable",
    "Orable",
    "Xorable",
    "Negatable",
];

/// The linker qualifies `from std.utils import Variant` declarations.  Keep the
/// intrinsic recognition narrow so an unrelated user type ending in `Variant`
/// does not silently acquire built-in semantics.
pub(super) fn is_variant_name(name: &str) -> bool {
    matches!(
        name,
        "Variant" | "__module$std$utilsVariant" | "__module$std$utils$Variant"
    )
}
