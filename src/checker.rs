//! Static semantic checker: the authoritative handoff between elaborated AST and
//! compiler lowering. It resolves annotations, calls, traits, and conventions
//! into [`CheckedProgram`](crate::checked::CheckedProgram). It is a *sound*
//! approximation: if [`check`] succeeds, compiled execution will not raise
//! `UndefinedVariable`, `TypeError`, `NotCallable`, `ArityMismatch`, or
//! `ClosureEscape`. It is deliberately not *complete* — see the forward-reference
//! note below — so a few valid Mojo programs are rejected.
//!
//! ## Scoping
//! A stack of scopes (`Vec<HashMap<String, Ty>>`) models lexical name lookup.
//! Names are bound *sequentially* in source order, and a nested `def` body is checked at its definition
//! site with the enclosing scopes still on the stack (so capture is lexical).
//! One consequence: a function body may not forward-reference a sibling `def`
//! declared later in the same block (mutual recursion). Choosing soundness over completeness here keeps
//! the checker simple; hoisting `def` signatures per block is future work.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::ast::{
    ArgConvention, Dtype, Expr, ExprKind, FnParam, InfixOp, Method, PrefixOp, Stmt, StmtKind,
    StructComptime, SubscriptArg, TStringPart, TraitComptime, Type as SourceType,
};
use crate::call::{
    ArgSlot, CallVariadics, MatchError, effective_keyword_only_index, match_call_slots,
    regular_marker_index,
};
use crate::ct::{CtExpr, CtValue};
use crate::error::TypeError;
use crate::token::SourceSpan;
use crate::types::{
    CallableDefault, ConstraintOperand, DependentType, GenericConstraint, ParamDecl, SliceKind, Ty,
    TyArg, contains_infer, dict_elements, dict_type, list_element, list_type, range_type,
    set_element, set_type, tuple_elements, tuple_type as nominal_tuple_type,
};

type SubscriptDescriptorPlan = (Vec<Option<SliceKind>>, bool);

/// The raw-slot operations on `UnsafePointer` are an implementation privilege,
/// not source-language API. Linked expressions retain their exact source path;
/// only files physically shipped in the compiler's collection library receive
/// the checked adjustment that can lower these operations.
fn is_bundled_collection_source(source: Option<&str>) -> bool {
    let (Some(manifest), Some(source)) = (option_env!("CARGO_MANIFEST_DIR"), source) else {
        return false;
    };
    let stdlib = Path::new(manifest).join("stdlib");
    let source = Path::new(source);
    source == stdlib.join("std/collections/list.mojo")
        || source == stdlib.join("list.mojo")
        || source == stdlib.join("std/collections/dict.mojo")
        || source == stdlib.join("dict.mojo")
}

/// The checked signature of a struct, kept in the checker's registry.
struct StructInfo {
    /// Compile-time parameters (type and value); empty for a non-generic struct.
    decls: Vec<ParamDecl>,
    /// Concrete semantic arguments retained by an erased compiler-generated
    /// specialization. Public `Tuple[*Ts]` is emitted as a parameter-free
    /// implementation struct, but its checked identity must still carry the
    /// element types selected for `Ts`.
    fixed_arguments: Option<Vec<TyArg>>,
    /// Traits this struct declares conformance to (verified at definition).
    conforms: Vec<String>,
    callable_conformance: Option<Ty>,
    /// Exact lowered `__call__` method selected by the declared callable
    /// conformance. This is distinct from the callable contract: overloaded
    /// methods with the same arity require the signature-qualified target to
    /// survive into indirect-call MIR.
    callable_target: Option<String>,
    conformance_conditions: HashMap<String, Expr>,
    /// Declared fields, in order (drives the fieldwise constructor).
    fields: Vec<(String, Ty)>,
    /// Associated compile-time facts declared by `comptime NAME = ...` in the
    /// struct body. These live on the type, not on runtime instances.
    associated: HashMap<String, CtValue>,
    methods: HashMap<String, Vec<MethodSig>>,
    fieldwise_init: bool,
    explicit_destroy_message: Option<String>,
    explicit_destructors: HashMap<String, bool>,
}

#[derive(Clone, Copy)]
struct DependentIndexAccessorFamily {
    place: &'static str,
    value: &'static str,
}

/// Select the current Mojo parameter-index hook first, while retaining the
/// earlier spelling as an intentional source-compatibility fallback.
fn dependent_index_accessor_family(info: &StructInfo) -> Option<DependentIndexAccessorFamily> {
    if info.methods.contains_key("__getitem_param__$0") {
        Some(DependentIndexAccessorFamily {
            place: "__getitem_param__",
            value: "__getitem_param_value__",
        })
    } else if info.methods.contains_key("__getitem__$0") {
        Some(DependentIndexAccessorFamily {
            place: "__getitem__",
            value: "__getitem_value__",
        })
    } else {
        None
    }
}

/// The source-level pieces of a struct declaration passed through checking.
struct StructDeclaration<'a> {
    module: &'a Option<String>,
    name: &'a str,
    type_params: &'a [crate::ast::TypeParam],
    conforms: &'a [String],
    callable_conformance: &'a Option<SourceType>,
    conformance_conditions: &'a [(String, Expr)],
    fields: &'a [crate::ast::Param],
    associated: &'a [StructComptime],
    methods: &'a [Method],
    fieldwise_init: bool,
    decorators: &'a [crate::ast::Decorator],
}

/// The checked signature of a trait: required methods plus associated
/// compile-time facts. A method requirement's signature may mention
/// `Ty::SelfType` (the conforming type).
struct TraitInfo {
    refines: Vec<String>,
    methods: HashMap<String, Vec<MethodSig>>,
    comptime_members: HashMap<String, CtMemberReq>,
}

/// The required kind/type of a trait `comptime NAME: Annotation` member.
#[derive(Clone, PartialEq)]
enum CtMemberReq {
    /// A compile-time value whose value type must match this type.
    Value(Box<Ty>),
    /// A compile-time type value whose type must conform to these trait bounds.
    Type { bounds: Vec<String> },
}

/// Compose inherited associated-member requirements. Type-valued members with
/// the same name denote one associated type, so refinement accumulates their
/// bounds instead of treating stronger composition as an ambiguity. Value
/// members must retain one exact type; mixing value and type requirements is a
/// real conflict.
fn merge_associated_requirement(
    existing: &mut CtMemberReq,
    incoming: &CtMemberReq,
    member: &str,
) -> Result<(), TypeError> {
    match (existing, incoming) {
        (CtMemberReq::Type { bounds }, CtMemberReq::Type { bounds: more }) => {
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

fn conformance_operand(expression: &Expr, arguments: &HashMap<&str, &TyArg>) -> Option<CtValue> {
    match &expression.kind {
        ExprKind::Int(value) => Some(CtValue::IntLiteral(value.clone())),
        ExprKind::Bool(value) => Some(CtValue::Bool(*value)),
        ExprKind::Str(value) => Some(CtValue::Str(value.clone())),
        ExprKind::Identifier(name) => match arguments.get(name.as_str())? {
            TyArg::Val(value) => Some((*value).clone()),
            TyArg::Ty(_) => None,
        },
        _ => None,
    }
}

fn ct_integer(value: &CtValue) -> Option<crate::literal::IntLiteral> {
    match value {
        CtValue::Int(value) => Some((*value).into()),
        CtValue::UInt(value) => Some((*value).into()),
        CtValue::IntLiteral(value) => Some(value.clone()),
        _ => None,
    }
}

fn ct_values_equal(left: &CtValue, right: &CtValue) -> bool {
    match (ct_integer(left), ct_integer(right)) {
        (Some(left), Some(right)) => left == right,
        _ => left == right,
    }
}

fn compare_ct_integers(op: InfixOp, left: &CtValue, right: &CtValue) -> Option<bool> {
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

fn ty_args_equal(left: &TyArg, right: &TyArg) -> bool {
    match (left, right) {
        (TyArg::Val(left), TyArg::Val(right)) => ct_values_equal(left, right),
        _ => left == right,
    }
}

#[derive(Clone, PartialEq)]
struct MethodSig {
    decls: Vec<ParamDecl>,
    availability: Vec<GenericConstraint>,
    has_self: bool,
    /// Regular parameters only; variadic element type is stored separately.
    params: Vec<Ty>,
    names: Vec<String>,
    required: Vec<bool>,
    variadic: Option<Box<Ty>>,
    variadic_index: Option<usize>,
    kw_variadic: Option<Box<Ty>>,
    kw_variadic_index: Option<usize>,
    positional_only: Option<usize>,
    keyword_only: Option<usize>,
    conventions: Vec<Option<ArgConvention>>,
    ret: Ty,
    raises: bool,
    error: Option<Box<Ty>>,
    /// Receiver convention. `None` means plain read-only `self`; explicit
    /// conventions (`mut`, `var`, `ref`, ...) are preserved so trait
    /// requirements can compare them exactly. Today only `mut self` changes call
    /// checking behavior.
    self_convention: Option<crate::ast::ArgConvention>,
    ref_params: Vec<Option<crate::origin::RefSig>>,
    ref_return: Option<crate::origin::RefSig>,
    implicit: bool,
}

type MethodInstantiation = (
    Vec<Ty>,
    Option<Ty>,
    Option<Ty>,
    HashMap<String, Ty>,
    HashMap<String, TyArg>,
);

impl MethodSig {
    fn intrinsic(params: Vec<Ty>, ret: Ty) -> MethodSig {
        let len = params.len();
        MethodSig {
            decls: Vec::new(),
            availability: Vec::new(),
            has_self: true,
            params,
            names: (0..len).map(|i| format!("arg{i}")).collect(),
            required: vec![true; len],
            variadic: None,
            variadic_index: None,
            kw_variadic: None,
            kw_variadic_index: None,
            positional_only: None,
            keyword_only: None,
            conventions: vec![None; len],
            ret,
            raises: false,
            error: None,
            self_convention: None,
            ref_params: vec![None; len],
            ref_return: None,
            implicit: false,
        }
    }
}

fn callable_parameter_count(ty: &Ty) -> Option<usize> {
    match ty {
        Ty::Func { params, .. } => Some(params.len()),
        Ty::GenericFunc { params, .. } => Some(params.len()),
        _ => None,
    }
}

fn place_root_name(expr: &Expr) -> Option<&str> {
    match &expr.kind {
        ExprKind::Identifier(name) => Some(name),
        ExprKind::Member { object, .. }
        | ExprKind::Index { object, .. }
        | ExprKind::Slice { object, .. }
        | ExprKind::MultiIndex { object, .. } => place_root_name(object),
        ExprKind::TypeApply { name, .. } => Some(name),
        _ => None,
    }
}

fn place_has_index(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::Index { .. } | ExprKind::Slice { .. } | ExprKind::MultiIndex { .. } => true,
        ExprKind::Member { object, .. } => place_has_index(object),
        _ => false,
    }
}

fn method_arity_range(sig: &MethodSig) -> (usize, usize) {
    (sig.params.len(), sig.params.len())
}

fn same_method_shape(a: &MethodSig, b: &MethodSig) -> bool {
    method_arity_range(a) == method_arity_range(b)
        && a.params == b.params
        && a.variadic == b.variadic
        && a.kw_variadic == b.kw_variadic
}

/// A conforming method may promise no error where its trait requirement raises,
/// but a raising implementation must preserve the exact declared error family.
/// Bare `raises` denotes `Error`; it is not a wildcard for a distinct typed
/// error. `raises Never` is already normalized to a non-raising signature when
/// `MethodSig` is built.
fn method_satisfies_requirement(got: &MethodSig, required: &MethodSig) -> bool {
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

/// A deliberately small implication relation for declaration availability.
/// It proves only facts which are syntactically present in a positive
/// conjunction (plus exact predicates).  In particular it does not turn a
/// failed symbolic evaluation, a negation, or either arm of a disjunction into
/// an assumption.
fn generic_constraint_implies(
    premise: &GenericConstraint,
    consequence: &GenericConstraint,
) -> bool {
    if premise == consequence || matches!(consequence, GenericConstraint::Bool(true)) {
        return true;
    }
    match (premise, consequence) {
        (_, GenericConstraint::And(left, right)) => {
            generic_constraint_implies(premise, left) && generic_constraint_implies(premise, right)
        }
        (GenericConstraint::And(left, right), _) => {
            generic_constraint_implies(left, consequence)
                || generic_constraint_implies(right, consequence)
        }
        _ => false,
    }
}

fn guaranteed_conformance_atoms(
    constraint: &GenericConstraint,
    output: &mut Vec<(String, String)>,
) {
    match constraint {
        GenericConstraint::Conforms { param, trait_name } => {
            let atom = (param.clone(), trait_name.clone());
            if !output.contains(&atom) {
                output.push(atom);
            }
        }
        GenericConstraint::And(left, right) => {
            guaranteed_conformance_atoms(left, output);
            guaranteed_conformance_atoms(right, output);
        }
        // A disjunction, negation, comparison, or symbolic pack predicate does
        // not unconditionally refine one ordinary type parameter.
        _ => {}
    }
}

/// The checker's value-coercion predicate, shared with MIR verification so the
/// verifier never re-derives conversion rules.
pub(crate) fn value_coerces(from: &Ty, to: &Ty) -> bool {
    coerces(from, to)
}

fn same_callable_signature(a: &Ty, b: &Ty) -> bool {
    match (a, b) {
        (
            Ty::Func {
                params: ap,
                variadic: av,
                kw_variadic: akw,
                ..
            },
            Ty::Func {
                params: bp,
                variadic: bv,
                kw_variadic: bkw,
                ..
            },
        ) => ap == bp && av == bv && akw == bkw,
        (
            Ty::GenericFunc {
                decls: ad,
                params: ap,
                variadic: av,
                kw_variadic: akw,
                ..
            },
            Ty::GenericFunc {
                decls: bd,
                params: bp,
                variadic: bv,
                kw_variadic: bkw,
                ..
            },
        ) => {
            let aparams: Vec<_> = ap
                .iter()
                .chain(av.iter().map(Box::as_ref))
                .chain(akw.iter().map(Box::as_ref))
                .cloned()
                .collect();
            let bparams: Vec<_> = bp
                .iter()
                .chain(bv.iter().map(Box::as_ref))
                .chain(bkw.iter().map(Box::as_ref))
                .cloned()
                .collect();
            canonical_generic_signature(ad, &aparams) == canonical_generic_signature(bd, &bparams)
        }
        _ => false,
    }
}

fn canonical_generic_signature(decls: &[ParamDecl], params: &[Ty]) -> (Vec<ParamDecl>, Vec<Ty>) {
    let mut subst = HashMap::new();
    let mut value_names = HashMap::new();
    let canonical_decls = decls
        .iter()
        .enumerate()
        .map(|(index, decl)| match decl {
            ParamDecl::Type {
                name,
                bounds,
                callable_bound,
                default: _,
                infer_only: _,
                variadic,
                constraints,
            } => {
                let canonical_name = format!("${index}");
                let canonical_callable_bound = callable_bound.as_ref().map(|bound| {
                    Box::new(rename_dependent_parameters(
                        &substitute(bound, &subst),
                        &value_names,
                    ))
                });
                subst.insert(
                    name.clone(),
                    Ty::Param {
                        name: canonical_name.clone(),
                        bounds: bounds.clone(),
                        callable_bound: canonical_callable_bound.clone(),
                    },
                );
                ParamDecl::Type {
                    name: canonical_name,
                    bounds: bounds.clone(),
                    callable_bound: canonical_callable_bound,
                    // Binder defaults and the `//` inference marker govern a
                    // call through the contract; current Mojo does not make
                    // either part of generic callable conformance identity.
                    default: None,
                    infer_only: false,
                    variadic: *variadic,
                    constraints: constraints.clone(),
                }
            }
            ParamDecl::Value {
                name,
                ty,
                default: _,
                callable_default: _,
                infer_only: _,
                variadic,
                constraints,
                ..
            } => {
                let canonical_name = format!("${index}");
                let canonical_ty =
                    rename_dependent_parameters(&substitute(ty, &subst), &value_names);
                value_names.insert(
                    name.trim_start_matches('*').to_string(),
                    canonical_name.clone(),
                );
                ParamDecl::Value {
                    name: canonical_name,
                    ty: Box::new(canonical_ty),
                    default: None,
                    callable_default: None,
                    infer_only: false,
                    variadic: *variadic,
                    constraints: constraints.clone(),
                }
            }
        })
        .collect();
    let canonical_params = params
        .iter()
        .map(|ty| rename_dependent_parameters(&substitute(ty, &subst), &value_names))
        .collect();
    (canonical_decls, canonical_params)
}

/// The lowered symbol the checker records as the resolved callee of an
/// overloaded free-function call — formatted by the canonical symbol module so
/// it names exactly the `MirFunction` the MIR emits for that definition.
fn callable_lowered_name(name: &str, ty: &Ty) -> Option<String> {
    let (params, variadic, kw_variadic) = match ty {
        Ty::Func {
            params,
            variadic,
            kw_variadic,
            ..
        }
        | Ty::GenericFunc {
            params,
            variadic,
            kw_variadic,
            ..
        } => (params, variadic, kw_variadic),
        _ => return None,
    };
    let signature_types: Vec<_> = params
        .iter()
        .chain(variadic.iter().map(Box::as_ref))
        .chain(kw_variadic.iter().map(Box::as_ref))
        .collect();
    Some(crate::symbol::function_symbol(
        name,
        &crate::symbol::SignatureKey::from_tys(signature_types),
    ))
}

/// The lowered symbol of an overloaded method/constructor resolution, likewise
/// canonical (`sig.params` are the declared parameter types, unsubstituted —
/// matching the MIR definition side, which mangles the declared annotations).
fn method_lowered_name(type_name: &str, method: &str, sig: &MethodSig) -> String {
    let signature_types = sig
        .params
        .iter()
        .chain(sig.variadic.iter().map(Box::as_ref))
        .chain(sig.kw_variadic.iter().map(Box::as_ref));
    let signature = crate::symbol::SignatureKey::from_tys(signature_types);
    if method == "__iter__" {
        crate::symbol::iterator_method_symbol(type_name, sig.self_convention, &signature)
    } else {
        crate::symbol::method_symbol(type_name, method, &signature)
    }
}

/// A signature-qualified abstract `__call__` symbol for an indirect callable
/// contract. The VM retargets this symbol to a nominal receiver's runtime type;
/// ordinary function and closure values ignore it. Keeping the signature here
/// avoids falling back to arity when a callable struct overloads `__call__` on
/// parameter type.
pub(crate) fn callable_contract_target(ty: &Ty) -> Option<String> {
    let contract = callable_contract_ty(ty)?;
    let (params, variadic, kw_variadic) = match contract {
        Ty::Func {
            params,
            variadic,
            kw_variadic,
            ..
        } => (params, variadic, kw_variadic),
        _ => return None,
    };
    let signature_types = params
        .iter()
        .chain(variadic.iter().map(Box::as_ref))
        .chain(kw_variadic.iter().map(Box::as_ref));
    Some(crate::symbol::method_symbol(
        "__trait_dispatch",
        "__call__",
        &crate::symbol::SignatureKey::from_tys(signature_types),
    ))
}

/// Recover the monomorphic or generic callable contract carried either directly
/// by a function type or indirectly by a callable-bounded type parameter.
pub(crate) fn callable_contract_ty(ty: &Ty) -> Option<&Ty> {
    match ty {
        Ty::Func { .. } | Ty::GenericFunc { .. } => Some(ty),
        Ty::Param {
            callable_bound: Some(bound),
            ..
        } => callable_contract_ty(bound),
        _ => None,
    }
}

fn callable_convention_accepts(
    actual: Option<ArgConvention>,
    contract: Option<ArgConvention>,
) -> bool {
    let actual = actual.unwrap_or(ArgConvention::Read);
    let contract = contract.unwrap_or(ArgConvention::Read);
    match (actual, contract) {
        // A read-only callee demands less access than a mutable callable
        // contract promises to supply, so it is a valid implementation.
        (ArgConvention::Read, ArgConvention::Read | ArgConvention::Mut) => true,
        (ArgConvention::Mut, ArgConvention::Mut) => true,
        // Ownership-changing and parametric-reference conventions retain their
        // exact ABI until their full subtyping rules are modeled.
        (actual, contract) => actual == contract,
    }
}

/// Whether a concrete monomorphic callable implementation fulfills an
/// anonymous `def(...)` trait contract. This is intentionally directional:
/// non-raising/read-only implementations may fulfill raising/mutable contracts,
/// but not vice versa.
pub(crate) fn callable_bound_accepts(actual: &Ty, contract: &Ty) -> bool {
    if matches!(actual, Ty::GenericFunc { .. }) || matches!(contract, Ty::GenericFunc { .. }) {
        let (Some((actual_decls, actual)), Some((contract_decls, contract))) = (
            erase_generic_callable_binders(actual),
            erase_generic_callable_binders(contract),
        ) else {
            return false;
        };
        return actual_decls == contract_decls && callable_bound_accepts(&actual, &contract);
    }

    let (
        Ty::Func {
            environment: actual_environment,
            params: actual_params,
            ret: actual_ret,
            required: actual_required,
            variadic: actual_variadic,
            kw_variadic: actual_kw_variadic,
            positional_only: actual_positional_only,
            keyword_only: actual_keyword_only,
            raises: actual_raises,
            error: actual_error,
            conventions: actual_conventions,
            ref_params: actual_ref_params,
            ref_return: actual_ref_return,
            ..
        },
        Ty::Func {
            environment: contract_environment,
            params: contract_params,
            ret: contract_ret,
            required: contract_required,
            variadic: contract_variadic,
            kw_variadic: contract_kw_variadic,
            positional_only: contract_positional_only,
            keyword_only: contract_keyword_only,
            raises: contract_raises,
            error: contract_error,
            conventions: contract_conventions,
            ref_params: contract_ref_params,
            ref_return: contract_ref_return,
            ..
        },
    ) = (actual, contract)
    else {
        return false;
    };

    callable_environment_coerces(actual_environment, contract_environment)
        && actual_params.len() == contract_params.len()
        && actual_params
            .iter()
            .zip(contract_params)
            .all(|(actual, contract)| actual == contract)
        && coerces(actual_ret, contract_ret)
        && actual_required.len() == contract_required.len()
        && actual_required
            .iter()
            .zip(contract_required)
            .all(|(actual, contract)| !*actual || *contract)
        && actual_variadic.is_none()
        && contract_variadic.is_none()
        && actual_kw_variadic.is_none()
        && contract_kw_variadic.is_none()
        && actual_positional_only == contract_positional_only
        && actual_keyword_only == contract_keyword_only
        && actual_conventions.len() == contract_conventions.len()
        && actual_conventions
            .iter()
            .zip(contract_conventions)
            .all(|(actual, contract)| callable_convention_accepts(*actual, *contract))
        && actual_ref_params == contract_ref_params
        && actual_ref_return == contract_ref_return
        && (!*actual_raises || *contract_raises)
        && match (actual_error.as_deref(), contract_error.as_deref()) {
            (None, _) | (Some(Ty::Never), _) => true,
            (Some(_), None) => false,
            (Some(actual), Some(Ty::Error)) => actual != &Ty::Never,
            (Some(actual), Some(contract)) => actual == contract,
        }
}

/// Alpha-normalize a generic anonymous callable into its declaration list and
/// a monomorphic callable shape whose parameter occurrences use canonical
/// `$N` names.  Generic callable compatibility can then reuse the ordinary
/// directional callable-contract rules without making source binder spelling
/// part of the type identity.
fn erase_generic_callable_binders(callable: &Ty) -> Option<(Vec<ParamDecl>, Ty)> {
    let Ty::GenericFunc {
        environment,
        decls,
        params,
        names,
        ret,
        required,
        variadic,
        kw_variadic,
        positional_only,
        keyword_only,
        raises,
        error,
        conventions,
        ref_params,
        ref_return,
    } = callable
    else {
        return None;
    };

    let mut signature = params.clone();
    let variadic_index = variadic.as_ref().map(|parameter| {
        let index = signature.len();
        signature.push((**parameter).clone());
        index
    });
    let kw_variadic_index = kw_variadic.as_ref().map(|parameter| {
        let index = signature.len();
        signature.push((**parameter).clone());
        index
    });
    let return_index = signature.len();
    signature.push((**ret).clone());
    let error_index = error.as_ref().map(|error| {
        let index = signature.len();
        signature.push((**error).clone());
        index
    });
    let (decls, signature) = canonical_generic_signature(decls, &signature);

    Some((
        decls,
        Ty::Func {
            environment: environment.clone(),
            params: signature[..params.len()].to_vec(),
            names: names.clone(),
            ret: Box::new(signature[return_index].clone()),
            required: required.clone(),
            variadic: variadic_index.map(|index| Box::new(signature[index].clone())),
            kw_variadic: kw_variadic_index.map(|index| Box::new(signature[index].clone())),
            positional_only: *positional_only,
            keyword_only: *keyword_only,
            raises: *raises,
            error: error_index.map(|index| Box::new(signature[index].clone())),
            conventions: conventions.clone(),
            ref_params: ref_params.clone(),
            ref_return: ref_return.clone(),
        },
    ))
}

fn method_callable_ty(method: &MethodSig) -> Ty {
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
    }
}

fn with_callable_environment(
    mut callable: Ty,
    environment: crate::origin::CallableEnvironment,
) -> Ty {
    match &mut callable {
        Ty::Func {
            environment: current,
            ..
        }
        | Ty::GenericFunc {
            environment: current,
            ..
        } => *current = environment,
        _ => {}
    }
    callable
}

enum OverloadSelect {
    NoMatch,
    Ambiguous,
}

const CONVERSION_RANK: usize = 1 << 24;
const VARIADIC_RANK: usize = 1 << 16;
const SIGNATURE_LENGTH_RANK: usize = 1 << 8;

fn overload_rank(conversions: usize, variadic: bool, signature_len: usize, generic: bool) -> usize {
    conversions * CONVERSION_RANK
        + usize::from(variadic) * VARIADIC_RANK
        + signature_len * SIGNATURE_LENGTH_RANK
        + usize::from(generic)
}

fn conversion_count(actual: &Ty, expected: &Ty) -> usize {
    if actual == expected
        || matches!(actual, Ty::IntLiteral) && matches!(expected, Ty::Int)
        || matches!(actual, Ty::FloatLiteral) && matches!(expected, Ty::Float64)
    {
        0
    } else {
        1
    }
}

/// A concrete method candidate after receiver-type substitution and argument
/// scoring. Named fields keep overload resolution readable as it evolves.
struct MethodCallResolution {
    conversion_score: usize,
    slots: Vec<ArgSlot>,
    positional_overflow: Vec<usize>,
    keyword_overflow: Vec<usize>,
    variadic_element: Option<Ty>,
    keyword_element: Option<Ty>,
    conventions: Vec<Option<ArgConvention>>,
    self_convention: Option<ArgConvention>,
    return_type: Ty,
    raises: bool,
    error: Option<Box<Ty>>,
    mutates_receiver: bool,
    consumes_receiver: bool,
    lowered_name: Option<String>,
    ref_params: Vec<Option<crate::origin::RefSig>>,
    ref_return: Option<crate::origin::RefSig>,
    param_types: Vec<Ty>,
    param_decls: Vec<ParamDecl>,
}

struct MethodCallScore {
    rank: usize,
    slots: Vec<ArgSlot>,
    positional_overflow: Vec<usize>,
    keyword_overflow: Vec<usize>,
}

/// Source-level arguments attached to a method invocation. Keeping the runtime
/// and compile-time argument lists together prevents the two method-resolution
/// paths from slowly acquiring different call-shape parameters.
#[derive(Clone, Copy)]
struct MethodCallArguments<'a> {
    param_args: &'a [crate::ast::ParamArg],
    args: &'a [Expr],
    kwargs: &'a [crate::ast::KwArg],
    parameterized_syntax: bool,
    /// The caller separately records a more precise projected write, so a
    /// `mut self` call must not also invalidate every receiver interior.
    preserves_receiver_interiors: bool,
}

/// Interior-generation state immediately before applying one selected method
/// contract. Candidate scoring has already inferred the argument expressions at
/// this point, so subtracting this snapshot isolates callee effects from effects
/// which belong to evaluation of the source expression itself.
struct CallBoundarySnapshot {
    invalidations: HashMap<SourceSpan, Vec<crate::checked::InteriorInvalidation>>,
}

#[derive(Clone)]
struct ValueAdjustmentSnapshot {
    source: SourceSpan,
    overload_target: Option<String>,
    implicit_conversion: Option<String>,
    operation: Option<crate::checked::SemanticAdjustment>,
}

impl<'a> MethodCallArguments<'a> {
    fn ordinary(args: &'a [Expr], kwargs: &'a [crate::ast::KwArg]) -> Self {
        Self {
            param_args: &[],
            args,
            kwargs,
            parameterized_syntax: false,
            preserves_receiver_interiors: false,
        }
    }

    fn interior_preserving(args: &'a [Expr], kwargs: &'a [crate::ast::KwArg]) -> Self {
        Self {
            preserves_receiver_interiors: true,
            ..Self::ordinary(args, kwargs)
        }
    }

    fn parameterized(
        param_args: &'a [crate::ast::ParamArg],
        args: &'a [Expr],
        kwargs: &'a [crate::ast::KwArg],
    ) -> Self {
        Self {
            param_args,
            args,
            kwargs,
            parameterized_syntax: true,
            preserves_receiver_interiors: false,
        }
    }
}

struct SubscriptResolution {
    return_type: Ty,
    lowered_name: Option<String>,
    value_keyword: bool,
}

type ReturnRefContract = (
    crate::origin::RefSig,
    Vec<crate::origin::OwnerId>,
    Option<crate::origin::OriginPlace>,
);

fn select_callable_overload(
    matches: Vec<(Ty, usize, String, Option<Ty>)>,
) -> Result<(Ty, String, Option<Ty>), OverloadSelect> {
    let best = matches
        .iter()
        .map(|(_, score, _, _)| *score)
        .min()
        .ok_or(OverloadSelect::NoMatch)?;
    let mut best_matches = matches
        .into_iter()
        .filter(|(_, score, _, _)| *score == best)
        .collect::<Vec<_>>();
    if best_matches.len() != 1 {
        return Err(OverloadSelect::Ambiguous);
    }
    let (ret, _, target, error) = best_matches.remove(0);
    Ok((ret, target, error))
}

fn select_method_overload(
    _method: &str,
    matches: Vec<MethodCallResolution>,
) -> Result<MethodCallResolution, OverloadSelect> {
    let best = matches
        .iter()
        .map(|candidate| candidate.conversion_score)
        .min()
        .ok_or(OverloadSelect::NoMatch)?;
    let mut best_matches = matches
        .into_iter()
        .filter(|candidate| candidate.conversion_score == best)
        .collect::<Vec<_>>();
    if best_matches.len() == 1 {
        Ok(best_matches.remove(0))
    } else {
        Err(OverloadSelect::Ambiguous)
    }
}

fn overload_candidates(existing: &Ty, new_ty: &Ty) -> Option<Vec<Ty>> {
    callable_parameter_count(new_ty)?;
    match existing {
        Ty::Func { .. } | Ty::GenericFunc { .. }
            if callable_parameter_count(existing).is_some() =>
        {
            Some(vec![existing.clone()])
        }
        Ty::Overload(candidates) => Some(candidates.clone()),
        _ => None,
    }
}

/// Type-check a whole program. Convenience wrapper over [`Checker`].
pub fn check(stmts: &[Stmt]) -> Result<(), TypeError> {
    check_program(stmts).map(|_| ())
}

/// Type-check and retain the semantic facts consumed by lowering/backends.
pub fn check_program(stmts: &[Stmt]) -> Result<crate::checked::CheckedProgram, TypeError> {
    check_program_with_materialized_callables(stmts, HashMap::new())
}

/// Check compiler-generated Tuple declarations with the exact callable types
/// referenced by their opaque, parser-unconstructible annotation ids.
pub(crate) fn check_program_with_materialized_callables(
    stmts: &[Stmt],
    materialized_callables: HashMap<String, Ty>,
) -> Result<crate::checked::CheckedProgram, TypeError> {
    let mut expanded = expand_trait_defaults(stmts)?;
    // Source locations survive elaboration clones and therefore cannot identify
    // semantic occurrences. Re-key the final checked tree after the last
    // checker-side cloning transform, before any fact table is populated.
    crate::ast::rekey_syntax(&mut expanded);
    let mut checker = Checker::new_with_materialized_callables(materialized_callables);
    checker.check_program(&expanded)?;
    checker.check_reference_result_reads()?;
    let explicit_destroy_types = checker
        .structs
        .iter()
        .filter_map(|(name, info)| {
            let self_ty = Ty::Struct(name.clone(), info.decls.iter().map(param_as_arg).collect());
            (!checker.is_implicitly_deletable(&self_ty)).then(|| {
                (
                    name.clone(),
                    crate::checked::ExplicitDestroyInfo {
                        message: info.explicit_destroy_message.clone().unwrap_or_else(|| {
                            "value is not implicitly deletable and must be explicitly destroyed"
                                .to_string()
                        }),
                        destructors: info.explicit_destructors.clone(),
                        fields: info
                            .fields
                            .iter()
                            .filter_map(|(field, ty)| match ty {
                                Ty::Struct(field_ty, _) if !checker.is_implicitly_deletable(ty) => {
                                    Some((field.clone(), field_ty.clone()))
                                }
                                _ => None,
                            })
                            .collect(),
                    },
                )
            })
        })
        .collect();
    {
        let binding_types = checker.binding_types.borrow();
        let comprehension_bindings = checker.comprehension_bindings.borrow();
        let deletability = checker.explicit_destroy_deletability.borrow();
        crate::explicit_destroy::check(
            &expanded,
            &binding_types,
            &comprehension_bindings,
            &deletability,
            &explicit_destroy_types,
        )?;
    }
    Ok(crate::checked::CheckedProgram::new(
        expanded,
        checker.overload_targets.into_inner(),
        checker.implicit_conversions.into_inner(),
        checker.declaration_types.into_inner(),
        checker.generic_parameters.into_inner(),
        checker.expression_types.into_inner(),
        checker.expression_bindings.into_inner(),
        checker.statement_bindings.into_inner(),
        checker.declaration_captures.into_inner(),
        checker.comprehension_bindings.into_inner(),
        checker.expression_place_types.into_inner(),
        checker.binding_types.into_inner(),
        checker.expression_effects.into_inner(),
        checker.selected_calls.into_inner(),
        checker.subscript_descriptors.into_inner(),
        checker.iteration_protocols.into_inner(),
        checker.simd_constructions.into_inner(),
        checker.operation_adjustments.into_inner(),
        checker.tuple_unpack_plans.into_inner(),
        checker.interior_references.into_inner(),
        checker.interior_invalidations.into_inner(),
        explicit_destroy_types,
        checker.explicit_destroy_calls.into_inner(),
        checker.reference_value_uses.into_inner(),
        checker.copy_place_value_uses.into_inner(),
        checker.call_place_uses.into_inner(),
        checker.implicitly_copied_consuming_receivers.into_inner(),
        checker.declaration_effects.into_inner(),
    ))
}

/// A declaration-only view of the checker's conformance registry for phases
/// that necessarily run before whole-program type checking.  Compile-time
/// specialization uses this to validate an inferred heterogeneous type pack at
/// its call site; it must not grow a second, subtly different implementation of
/// trait conformance.
///
/// The oracle records trait refinement, nominal struct conformances,
/// conformance conditions, field types, and lifecycle method presence.  Method
/// bodies and full requirement signatures remain the ordinary checker's job
/// after elaboration.
pub(crate) struct ConformanceOracle {
    checker: Checker,
}

/// Evidence retained when a pre-check conformance query fails.
pub(crate) struct ConformanceFailure {
    pub(crate) reason: Option<String>,
}

impl ConformanceOracle {
    pub(crate) fn from_program(stmts: &[Stmt]) -> Result<Self, TypeError> {
        let mut checker = Checker::new();

        // Refinement is the only trait fact needed by `conforms_to`. Register
        // every name first so the oracle is independent of body checking and
        // can answer nominal queries while specialization is still rewriting
        // the program.
        for statement in stmts {
            let StmtKind::Trait { name, refines, .. } = &statement.kind else {
                continue;
            };
            checker.traits.insert(
                name.clone(),
                TraitInfo {
                    refines: refines.clone(),
                    methods: HashMap::new(),
                    comptime_members: HashMap::new(),
                },
            );
        }

        // Struct facts are likewise signature-only. Full conformance
        // verification still runs after elaboration, so accepting a declaration
        // into this registry never bypasses method or associated-member checks.
        for statement in stmts {
            let StmtKind::Struct {
                name,
                type_params,
                conforms,
                conformance_conditions,
                fields,
                methods,
                fieldwise_init,
                ..
            } = &statement.kind
            else {
                continue;
            };

            let decls = checker.classify_params(type_params)?;
            let self_ty = Ty::Struct(name.clone(), decls.iter().map(param_as_arg).collect());
            let saved_self_decls = std::mem::replace(&mut checker.self_decls, decls.clone());
            let saved_type_params =
                std::mem::replace(&mut checker.enclosing_type_params, type_params.clone());
            let saved_self_ty = checker.self_ty.replace(self_ty);
            let field_types = if decls.iter().any(|decl| {
                matches!(
                    decl,
                    ParamDecl::Type { variadic: true, .. }
                        | ParamDecl::Value { variadic: true, .. }
                )
            }) {
                // Pack-dependent fields are expanded into ordinary concrete
                // fields/types by specialization. The template itself cannot be
                // resolved as a single erased type.
                Ok(Vec::new())
            } else {
                fields
                    .iter()
                    .map(|field| {
                        checker
                            .ty_from_anno(&field.ty)
                            .map(|ty| (field.name.clone(), ty))
                    })
                    .collect::<Result<Vec<_>, _>>()
            };
            checker.self_decls = saved_self_decls;
            checker.enclosing_type_params = saved_type_params;
            checker.self_ty = saved_self_ty;

            let mut method_names: HashMap<String, Vec<MethodSig>> = HashMap::new();
            for method in methods {
                method_names
                    .entry(lifecycle_method_name(method).to_string())
                    .or_default();
            }
            checker.structs.insert(
                name.clone(),
                StructInfo {
                    decls,
                    fixed_arguments: None,
                    conforms: conforms.clone(),
                    callable_conformance: None,
                    callable_target: None,
                    conformance_conditions: conformance_conditions.iter().cloned().collect(),
                    fields: field_types?,
                    associated: HashMap::new(),
                    methods: method_names,
                    fieldwise_init: *fieldwise_init,
                    explicit_destroy_message: None,
                    explicit_destructors: HashMap::new(),
                },
            );
        }

        Ok(Self { checker })
    }

    pub(crate) fn require(&self, ty: &Ty, trait_name: &str) -> Result<(), ConformanceFailure> {
        if self.checker.conforms_to(ty, trait_name) {
            Ok(())
        } else {
            Err(ConformanceFailure {
                reason: self.checker.trait_failure_reason(ty, trait_name),
            })
        }
    }
}

/// Materialize trait default methods into each conforming struct before semantic
/// checking. This keeps default dispatch static: downstream MIR sees an ordinary
/// struct method and needs no trait-object runtime machinery.
fn expand_trait_defaults(stmts: &[Stmt]) -> Result<Vec<Stmt>, TypeError> {
    #[derive(Clone)]
    struct TraitDefaults {
        refines: Vec<String>,
        methods: Vec<crate::ast::TraitMethod>,
    }

    fn defaults_for(
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
                    where_clause: method.where_clause.clone(),
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

/// Type-check a program and return the concrete lowered callee chosen for every
/// overloaded call site. MIR lowering uses this side table so source calls like
/// `f(x)` can lower to a signature-specific function even when overloads share
/// the same arity.
pub fn resolve_overload_targets(stmts: &[Stmt]) -> Result<HashMap<SourceSpan, String>, TypeError> {
    Ok(check_program(stmts)?.overload_targets().clone())
}

#[derive(Clone)]
struct CapturePolicy {
    /// Scope index at which the nested function's own locals begin.
    base: usize,
    function_name: String,
    declaration: SourceSpan,
    entries: HashMap<String, crate::ast::CaptureKind>,
    default: Option<crate::ast::CaptureKind>,
}

/// How one source-level `Origin` parameter is represented by the callable's
/// slot-relative reference contracts. Origin parameters are erased from
/// `Ty::Func`'s ordinary compile-time parameter list, so this checker-owned fact
/// is what lets a value expression such as `borrow[origin_of(value)]` bind that
/// parameter without reconstructing it from source later in MIR lowering.
#[derive(Clone)]
struct CallableOriginParam {
    name: String,
    slots: Vec<usize>,
}

/// One source compile-time parameter in a callable declaration. `Origin`
/// parameters are erased from `Ty::GenericFunc::decls`, so retaining this
/// ordered layout is necessary to split a mixed specialization such as
/// `borrow[Int, origin_of(value)]` without shifting the ordinary type argument.
#[derive(Clone)]
struct CallableSourceParam {
    name: String,
    infer_only: bool,
    origin: Option<usize>,
    ordinary: bool,
}

/// Origin-specialization metadata for one declaration in an overload set.
/// Entries are registered in the same order as `Ty::Overload` candidates.
#[derive(Clone)]
struct CallableOriginSignature {
    origins: Vec<CallableOriginParam>,
    source: Vec<CallableSourceParam>,
}

type SplitCallableSpecialization = (
    Vec<crate::ast::ParamArg>,
    Vec<(Vec<usize>, crate::origin::Origin)>,
);

/// A single-pass static type checker over the parsed AST.
pub struct Checker {
    /// Lexical scope chain, innermost last. Starts with the global scope.
    scopes: Vec<HashMap<String, Ty>>,
    /// Binding mutability, parallel to `scopes`. `var` locals are writable;
    /// ordinary function parameters are not.
    mutable_scopes: Vec<HashMap<String, bool>>,
    /// Stable identities for value bindings, parallel to `scopes`. Origin and
    /// loan facts use these identities so a shadowing declaration cannot be
    /// confused with the binding of the same source name in an outer scope.
    owner_scopes: Vec<HashMap<String, crate::origin::OwnerId>>,
    /// Origins retained inside reference-bearing aggregate bindings, parallel
    /// to the lexical value scopes.  Unlike `Ty::Struct`, this preserves the
    /// use-site owner identity needed for escape checking.
    aggregate_origin_scopes: Vec<HashMap<String, Vec<crate::origin::Origin>>>,
    /// Field-specific projection of `aggregate_origin_scopes`. Keeping direct
    /// reference fields separate prevents a write through one stored handle
    /// from invalidating interiors reached through an unrelated field.
    aggregate_field_origin_scopes:
        Vec<HashMap<String, HashMap<String, Vec<crate::origin::Origin>>>>,
    /// Reference-parameter handle types. Parameter expression typing still
    /// reads through to the declared referent, while storage contexts can ask
    /// for the handle explicitly.
    reference_parameter_scopes: Vec<HashMap<String, crate::origin::RefTy>>,
    /// Origin-parameter declarations for callable values, parallel to the
    /// lexical value scopes. The outer vector stored per name has one entry per
    /// overload declaration. Each entry also retains the original compile-time
    /// parameter order so erased Origin arguments can participate in overload
    /// and generic candidate selection.
    callable_origin_scopes: Vec<HashMap<String, Vec<CallableOriginSignature>>>,
    next_owner: u32,
    /// Index of the local scope for each function currently being checked.
    function_bases: Vec<usize>,
    /// Function/method scope base and caller-owned inputs which may legally
    /// appear inside a returned reference-bearing aggregate.
    aggregate_escape_contexts: Vec<(usize, HashSet<crate::origin::OwnerId>)>,
    /// Explicit capture policy for each nested function body being checked.
    capture_contexts: RefCell<Vec<CapturePolicy>>,
    /// Defined structs, by name (a separate namespace from value bindings).
    structs: HashMap<String, StructInfo>,
    /// Top-level struct symbols in the checked program, collected before body
    /// checking. Concrete Tuple types can therefore select their generated
    /// implementation independent of declaration order.
    declared_structs: HashSet<String>,
    /// Fixed semantic arguments for every compiler-generated public Tuple in
    /// the final program. This signature-only predeclaration is populated as a
    /// closed set before sequential member checking, so reciprocal transforms
    /// can name each other's complete nominal type without exposing ordinary
    /// source forward references.
    predeclared_generated_tuple_arguments: HashMap<String, Vec<TyArg>>,
    /// Exact semantic callable contracts named by compiler-only opaque type
    /// ids in generated Tuple declarations. Parsed source cannot populate this
    /// namespace.
    materialized_callables: HashMap<String, Ty>,
    /// Generated Tuple implementations are emitted at the stdlib template
    /// position, which may precede user element declarations. While checking
    /// that compiler-owned specialization only, permit nominal forward type
    /// identities; ordinary source declarations retain sequential visibility.
    allow_generated_tuple_forward_types: bool,
    /// Defined traits, by name (their method requirements).
    traits: HashMap<String, TraitInfo>,
    /// Stack of a generic `def`'s checked type parameters, innermost last. A
    /// bare `T` annotation resolves to the complete `Ty::Param`, including any
    /// anonymous callable-trait contract.
    /// (A `def`'s *value* parameters are ordinary `Int` locals, not here.)
    tparams: Vec<HashMap<String, Ty>>,
    /// The enclosing struct's parameters while checking its fields and methods,
    /// so `Self.T` resolves to `Ty::Param` and `Self.n` to a value parameter.
    /// Saved/restored around a (possibly nested) struct definition.
    self_decls: Vec<ParamDecl>,
    /// Positive conformance facts guaranteed by the active method's `where`
    /// clause. These are deliberately kept separate from `self_decls`: adding
    /// a temporary bound to the declaration changes the identity of `Ty::Param`
    /// and can make an otherwise identical return type fail to match. The
    /// facts refine capability queries only while that method body is checked.
    assumed_conformances: Vec<HashSet<(String, String)>>,
    enclosing_type_params: Vec<crate::ast::TypeParam>,
    /// The `Ty` denoted by a bare `Self` while checking a struct's members (the
    /// struct type) or a trait's requirements (`Ty::SelfType`). `None` elsewhere.
    self_ty: Option<Ty>,
    /// Trait-associated comptime requirements in scope while checking a trait's
    /// own method requirement signatures, so `Self.Element` can resolve.
    trait_self_comptime: Vec<HashMap<String, CtMemberReq>>,
    /// Exact integer constants declared by `comptime NAME = value`.
    comptimes: HashMap<String, crate::literal::IntLiteral>,
    /// Whether `self` is writable in the method body being checked — set while
    /// checking a `mut self` method's body (so `self.x = e` is allowed there).
    self_mutable: bool,
    /// An `out self` lifecycle initializer is establishing field storage.  For
    /// a reference-valued field, assigning a reference here stores its handle;
    /// later assignments write through the established handle instead.
    self_initializing: bool,
    /// Source-span to lowered callee for calls whose source name denotes an
    /// overload set. Interior mutability keeps expression inference usable from
    /// read-only helper methods while still recording resolution facts.
    overload_targets: RefCell<HashMap<SourceSpan, String>>,
    implicit_conversions: RefCell<HashMap<SourceSpan, String>>,
    simd_constructions: RefCell<HashMap<SourceSpan, (Dtype, i64)>>,
    /// Checked operation decisions — `Variant` construction/tag/projection/
    /// update and origin-bearing pointer construction — keyed by the source
    /// expression.  These cross the typed boundary so MIR never reinterprets
    /// syntax.
    operation_adjustments: RefCell<HashMap<SourceSpan, crate::checked::SemanticAdjustment>>,
    /// Synthetic tuple element reads introduced by unpacking have no source
    /// expression nodes. Retain their checked types and exact generated
    /// accessors on the RHS expression for HIR/MIR lowering.
    tuple_unpack_plans:
        RefCell<HashMap<SourceSpan, Vec<crate::checked::CheckedTupleUnpackElement>>>,
    /// Place expressions that define a fresh interior-reference generation.
    /// Kept separate from operation adjustments because a Variant projection,
    /// for example, carries both facts at the same checked node.
    interior_references: RefCell<HashMap<SourceSpan, crate::origin::OriginPlace>>,
    /// Mutations which invalidate interior generations below checked bases.
    interior_invalidations: RefCell<HashMap<SourceSpan, Vec<crate::checked::InteriorInvalidation>>>,
    declaration_types: RefCell<HashMap<crate::checked::AnnotationSite, Ty>>,
    generic_parameters: RefCell<HashMap<crate::checked::GenericSite, Vec<crate::types::ParamDecl>>>,
    /// Checked raising contract and reference-return fact per callable
    /// declaration; lowering never re-reads source `raises`/return syntax.
    declaration_effects:
        RefCell<HashMap<crate::checked::AnnotationSite, crate::checked::DeclarationEffect>>,
    expression_types: RefCell<HashMap<SourceSpan, Ty>>,
    expression_bindings: RefCell<HashMap<SourceSpan, crate::origin::OwnerId>>,
    /// Stable identities assigned by declarations and other binding statements.
    /// HIR uses these facts to map checked owners to runtime slots without
    /// recovering a binding from its source spelling.
    statement_bindings: RefCell<HashMap<SourceSpan, crate::origin::OwnerId>>,
    /// Explicit capture entries resolved at the nested declaration site. Keeping
    /// unused entries is essential: a move capture still transfers at declaration.
    declaration_captures: RefCell<HashMap<SourceSpan, Vec<crate::checked::CheckedCapture>>>,
    /// Stable identities/types for the lexical binders introduced by each
    /// comprehension, retained for checked HIR and explicit-destroy analysis.
    comprehension_bindings:
        RefCell<HashMap<SourceSpan, Vec<crate::checked::CheckedComprehensionBinding>>>,
    expression_place_types: RefCell<HashMap<SourceSpan, Ty>>,
    binding_types: RefCell<HashMap<SourceSpan, Ty>>,
    /// Positive site-sensitive drop facts retained for the later explicit-
    /// destroy CFG pass. Conditional conformances are meaningful only in the
    /// constraint environment in which a binding was checked.
    explicit_destroy_deletability: RefCell<crate::explicit_destroy::CheckedDeletability>,
    /// Selected call effects keyed by the checked call expression. This records
    /// the contract chosen during overload/bounded dispatch so later phases do
    /// not have to rediscover it from source syntax.
    expression_effects: RefCell<HashMap<SourceSpan, crate::checked::EffectFacts>>,
    /// Complete overload/origin/effect contract for a selected method-like
    /// call.  Nominal subscripts and ordinary method syntax share this fact.
    selected_calls: RefCell<HashMap<SourceSpan, crate::checked::CheckedCallContract>>,
    /// Subscript descriptor construction is orthogonal to call selection and
    /// may coexist with a reference-result adjustment at the same expression.
    subscript_descriptors: RefCell<HashMap<SourceSpan, SubscriptDescriptorPlan>>,
    /// Exact iterator protocol selected for each loop/comprehension iterable.
    /// Lowering consumes this fact instead of re-selecting `__iter__` by name.
    iteration_protocols: RefCell<HashMap<SourceSpan, crate::checked::IterationProtocol>>,
    explicit_destroy_calls: RefCell<std::collections::HashSet<SourceSpan>>,
    /// Expressions whose reference handle, rather than referent value, is
    /// required by a reference binding or origin-bearing aggregate operation.
    /// The bool records whether the resulting capability is writable.
    reference_value_uses: RefCell<HashMap<SourceSpan, bool>>,
    /// Reference-result reads proven copyable in their site-sensitive generic
    /// constraint environment. Final validation runs after those method scopes
    /// have been popped, so it must retain rather than recompute this fact.
    copyable_reference_result_reads: RefCell<HashSet<SourceSpan>>,
    /// Place expressions selected for an independent value copy at a consuming
    /// boundary. This stays checker-owned because conditional Copyable
    /// conformance can depend on the active generic constraint environment.
    copy_place_value_uses: RefCell<HashSet<SourceSpan>>,
    /// Actual arguments whose caller place must remain live through a selected
    /// `mut`/`ref` call. This checker-owned fact keeps MIR lowering from
    /// retaining ordinary copied arguments merely because they are syntactic
    /// places.
    call_place_uses: RefCell<HashSet<SourceSpan>>,
    /// Consuming method calls whose place receiver is implicitly copied. Kept
    /// separate from the single operation-adjustment slot so parameterized
    /// method metadata can coexist at the same expression.
    implicitly_copied_consuming_receivers: RefCell<HashSet<SourceSpan>>,
    return_ref_contracts: Vec<Option<ReturnRefContract>>,
    named_result_context: Vec<bool>,
    raising_context: Vec<Option<Ty>>,
    handled_raise_depth: usize,
    handled_raise_types: RefCell<Vec<Vec<Ty>>>,
    uninitialized: RefCell<HashSet<crate::origin::OwnerId>>,
}

impl Checker {
    pub fn new() -> Self {
        Self::new_with_materialized_callables(HashMap::new())
    }

    fn new_with_materialized_callables(materialized_callables: HashMap<String, Ty>) -> Self {
        Self {
            scopes: vec![HashMap::new()],
            mutable_scopes: vec![HashMap::new()],
            owner_scopes: vec![HashMap::new()],
            aggregate_origin_scopes: vec![HashMap::new()],
            aggregate_field_origin_scopes: vec![HashMap::new()],
            reference_parameter_scopes: vec![HashMap::new()],
            callable_origin_scopes: vec![HashMap::new()],
            next_owner: 0,
            function_bases: Vec::new(),
            aggregate_escape_contexts: Vec::new(),
            capture_contexts: RefCell::new(Vec::new()),
            structs: HashMap::new(),
            declared_structs: HashSet::new(),
            predeclared_generated_tuple_arguments: HashMap::new(),
            materialized_callables,
            allow_generated_tuple_forward_types: false,
            traits: HashMap::new(),
            tparams: Vec::new(),
            self_decls: Vec::new(),
            assumed_conformances: Vec::new(),
            enclosing_type_params: Vec::new(),
            self_ty: None,
            trait_self_comptime: Vec::new(),
            comptimes: HashMap::new(),
            self_mutable: false,
            self_initializing: false,
            overload_targets: RefCell::new(HashMap::new()),
            implicit_conversions: RefCell::new(HashMap::new()),
            simd_constructions: RefCell::new(HashMap::new()),
            operation_adjustments: RefCell::new(HashMap::new()),
            tuple_unpack_plans: RefCell::new(HashMap::new()),
            interior_references: RefCell::new(HashMap::new()),
            interior_invalidations: RefCell::new(HashMap::new()),
            declaration_types: RefCell::new(HashMap::new()),
            generic_parameters: RefCell::new(HashMap::new()),
            declaration_effects: RefCell::new(HashMap::new()),
            expression_types: RefCell::new(HashMap::new()),
            expression_bindings: RefCell::new(HashMap::new()),
            statement_bindings: RefCell::new(HashMap::new()),
            declaration_captures: RefCell::new(HashMap::new()),
            comprehension_bindings: RefCell::new(HashMap::new()),
            expression_place_types: RefCell::new(HashMap::new()),
            binding_types: RefCell::new(HashMap::new()),
            explicit_destroy_deletability: RefCell::new(
                crate::explicit_destroy::CheckedDeletability::default(),
            ),
            expression_effects: RefCell::new(HashMap::new()),
            selected_calls: RefCell::new(HashMap::new()),
            subscript_descriptors: RefCell::new(HashMap::new()),
            iteration_protocols: RefCell::new(HashMap::new()),
            explicit_destroy_calls: RefCell::new(std::collections::HashSet::new()),
            reference_value_uses: RefCell::new(HashMap::new()),
            copyable_reference_result_reads: RefCell::new(HashSet::new()),
            copy_place_value_uses: RefCell::new(HashSet::new()),
            call_place_uses: RefCell::new(HashSet::new()),
            implicitly_copied_consuming_receivers: RefCell::new(HashSet::new()),
            return_ref_contracts: Vec::new(),
            named_result_context: Vec::new(),
            raising_context: Vec::new(),
            handled_raise_depth: 0,
            handled_raise_types: RefCell::new(Vec::new()),
            uninitialized: RefCell::new(HashSet::new()),
        }
    }

    fn raising_allowed(&self) -> bool {
        self.handled_raise_depth > 0
            || self
                .raising_context
                .last()
                .is_some_and(|error| error.as_ref().is_some_and(|ty| *ty != Ty::Never))
    }

    fn require_error(&self, operation: impl Into<String>, error: Ty) -> Result<(), TypeError> {
        if self.handled_raise_depth > 0 {
            if let Some(types) = self.handled_raise_types.borrow_mut().last_mut() {
                types.push(error);
            }
            return Ok(());
        }
        if let Some(Some(expected)) = self.raising_context.last() {
            if *expected == error {
                return Ok(());
            }
            return Err(TypeError::RaiseTypeMismatch {
                expected: expected.to_string(),
                found: error.to_string(),
            });
        }
        if self.raising_allowed() {
            Ok(())
        } else {
            Err(TypeError::UnhandledRaise(operation.into()))
        }
    }

    fn record_call_effect(&self, span: SourceSpan, error: Ty) {
        self.expression_effects.borrow_mut().insert(
            span,
            crate::checked::EffectFacts {
                raises: Some(error),
                may_suspend: false,
                diverges: false,
            },
        );
    }

    fn concrete_callable_captures(&self, ty: &Ty) -> Vec<crate::origin::CaptureOrigin> {
        use crate::origin::{CallableEnvironment, CaptureOriginSet};
        let callable = match ty {
            Ty::Struct(name, _) => self
                .structs
                .get(name)
                .and_then(|info| info.callable_conformance.as_ref()),
            other => Some(other),
        };
        match callable {
            Some(Ty::Func {
                environment: CallableEnvironment::Capturing(CaptureOriginSet::Concrete(captures)),
                ..
            })
            | Some(Ty::GenericFunc {
                environment: CallableEnvironment::Capturing(CaptureOriginSet::Concrete(captures)),
                ..
            }) => captures.clone(),
            _ => Vec::new(),
        }
    }

    /// Retain every concrete environment access that may happen while a call is
    /// active. Callable arguments are included because Mojito supports only
    /// non-escaping downward funargs: their environments may be invoked by the
    /// callee before it returns.
    fn call_capture_effects<'a>(
        &self,
        types: impl IntoIterator<Item = &'a Ty>,
    ) -> Vec<crate::origin::CaptureOrigin> {
        use crate::origin::CaptureOriginSet;
        let captures = types
            .into_iter()
            .flat_map(|ty| self.concrete_callable_captures(ty))
            .collect::<Vec<_>>();
        let CaptureOriginSet::Concrete(captures) = CaptureOriginSet::concrete(captures) else {
            unreachable!("concrete capture canonicalization stays concrete")
        };
        captures
    }

    fn record_call_capture_effects<'a>(
        &self,
        span: SourceSpan,
        types: impl IntoIterator<Item = &'a Ty>,
    ) -> Vec<crate::origin::CaptureOrigin> {
        let captures = self.call_capture_effects(types);
        if captures.is_empty() {
            return captures;
        }
        self.operation_adjustments.borrow_mut().insert(
            span,
            crate::checked::SemanticAdjustment::CallableCaptureAccesses(captures.clone()),
        );
        captures
    }

    fn record_call_environment_effects(
        &self,
        span: SourceSpan,
        callable: &Ty,
        param_args: &[crate::ast::ParamArg],
        args: &[Expr],
        kwargs: &[crate::ast::KwArg],
    ) -> Result<(), TypeError> {
        // Call checking may have supplied essential context to an argument
        // before capture-effect collection runs (notably an empty collection
        // display, an exact literal, or an overloaded callable value). Reuse
        // that checked type instead of independently re-inferring the syntax
        // without its selected parameter contract.
        let checked_argument_type = |expression: &Expr| {
            let checked = {
                self.expression_types
                    .borrow()
                    .get(&expression.source_span())
                    .cloned()
            };
            match checked {
                Some(ty) => Ok(ty),
                None => self.infer(expression),
            }
        };
        let mut types = Vec::with_capacity(1 + param_args.len() + args.len() + kwargs.len());
        types.push(callable.clone());
        for argument in param_args {
            let expression = match argument {
                crate::ast::ParamArg::Value(expression) => Some(expression),
                crate::ast::ParamArg::Named { value, .. } => match &**value {
                    crate::ast::ParamArg::Value(expression) => Some(expression),
                    crate::ast::ParamArg::Type(_) | crate::ast::ParamArg::Named { .. } => None,
                },
                crate::ast::ParamArg::Type(_) => None,
            };
            if let Some(expression) = expression {
                types.push(checked_argument_type(expression)?);
            }
        }
        for argument in args {
            types.push(checked_argument_type(argument)?);
        }
        for argument in kwargs {
            types.push(checked_argument_type(&argument.value)?);
        }
        self.record_call_capture_effects(span, &types);
        Ok(())
    }

    fn declared_error(
        &self,
        raises: bool,
        raises_type: Option<&SourceType>,
    ) -> Result<Option<Ty>, TypeError> {
        if !raises {
            return Ok(None);
        }
        Ok(Some(match raises_type {
            Some(error) => self.ty_from_anno(error)?,
            None => Ty::Error,
        }))
    }

    fn lower_callable_environment(
        &self,
        type_params: &[crate::ast::TypeParam],
        thin: bool,
        capturing: Option<&crate::ast::OriginSpec>,
    ) -> Result<crate::origin::CallableEnvironment, TypeError> {
        use crate::origin::{CallableEnvironment, CaptureOriginSet, CaptureSetParamId};
        if thin && capturing.is_some() {
            return Err(TypeError::Unsupported(
                "a callable type cannot be both 'thin' and 'capturing'".to_string(),
            ));
        }
        if thin {
            return Ok(CallableEnvironment::Thin);
        }
        let Some(origins) = capturing else {
            return Ok(CallableEnvironment::Default);
        };
        match origins.as_slice() {
            [] => Ok(CallableEnvironment::Capturing(CaptureOriginSet::empty())),
            [
                Expr {
                    kind: ExprKind::Identifier(name),
                    ..
                },
            ] if name == "_" => Ok(CallableEnvironment::Capturing(CaptureOriginSet::Infer)),
            [
                Expr {
                    kind: ExprKind::Identifier(name),
                    ..
                },
            ] => {
                let index = type_params
                    .iter()
                    .position(|parameter| {
                        parameter.name == *name && parameter.bounds.as_slice() == ["OriginSet"]
                    })
                    .ok_or_else(|| TypeError::UndefinedVariable(name.clone()))?;
                Ok(CallableEnvironment::Capturing(CaptureOriginSet::Param(
                    CaptureSetParamId(index as u32),
                )))
            }
            _ => Err(TypeError::Unsupported(
                "capturing[...] requires exactly one OriginSet parameter or '_'".to_string(),
            )),
        }
    }

    /// Lower a callable annotation's origin contract. Declaration contracts
    /// normally refer to their own parameters and use `lower_ref_sig`; a
    /// contextual function-value type may additionally bind `origin_of(local)`
    /// directly to the checked local owner.
    fn lower_callable_ref_sig(
        &self,
        spec: &crate::ast::OriginSpec,
        type_params: &[crate::ast::TypeParam],
        params: &[&FnParam],
    ) -> Result<crate::origin::RefSig, TypeError> {
        use crate::origin::{Mutability, RefSig, SigMutability, SigOrigin};

        if let Ok(signature) = lower_ref_sig(spec, type_params, params) {
            return Ok(signature);
        }
        let mut members = Vec::new();
        let mut bound_mutability = Vec::new();
        for expression in spec {
            match &expression.kind {
                ExprKind::Call {
                    name,
                    args,
                    kwargs,
                    param_args,
                } if name == "origin_of" && kwargs.is_empty() && param_args.is_empty() => {
                    if args.is_empty() {
                        return Err(TypeError::Unsupported(
                            "origin_of requires at least one place".to_string(),
                        ));
                    }
                    for argument in args {
                        let (root, path) = place_path(argument).ok_or_else(|| {
                            TypeError::Unsupported("origin_of requires places".to_string())
                        })?;
                        if root == "self" {
                            members.push(project_sig_origin(SigOrigin::Self_, &path));
                        } else if let Some(index) =
                            params.iter().position(|parameter| parameter.name == root)
                        {
                            members.push(project_sig_origin(SigOrigin::Param(index), &path));
                        } else {
                            let actual = self.reference_actual(argument)?;
                            bound_mutability.push(actual.mutability);
                            members.push(SigOrigin::Bound(actual.origin));
                        }
                    }
                }
                _ => members.push(lower_sig_origin_expression(
                    expression,
                    type_params,
                    params,
                )?),
            }
        }
        let origin = match members.as_slice() {
            [] => SigOrigin::Infer,
            [single] => single.clone(),
            _ => SigOrigin::union(members),
        };
        let mutability = if bound_mutability.is_empty() {
            SigMutability::Infer
        } else if bound_mutability
            .iter()
            .all(|mutability| *mutability == Mutability::Immutable)
        {
            SigMutability::Immutable
        } else {
            SigMutability::Mutable
        };
        Ok(RefSig { origin, mutability })
    }

    fn lower_callable_ref_param_sigs(
        &self,
        type_params: &[crate::ast::TypeParam],
        params: &[&FnParam],
    ) -> Result<Vec<Option<crate::origin::RefSig>>, TypeError> {
        params
            .iter()
            .map(|parameter| {
                if parameter.convention != Some(ArgConvention::Ref) {
                    return Ok(None);
                }
                match &parameter.origin {
                    Some(spec) => self
                        .lower_callable_ref_sig(spec, type_params, params)
                        .map(Some),
                    None => Ok(Some(crate::origin::RefSig {
                        origin: crate::origin::SigOrigin::Infer,
                        mutability: crate::origin::SigMutability::Infer,
                    })),
                }
            })
            .collect()
    }

    /// The type denoted by a source annotation; resolves type parameters and
    /// validates struct names and type-argument counts.
    fn ty_from_anno(&self, ty: &SourceType) -> Result<Ty, TypeError> {
        self.resolve_ty_from_anno(ty)
    }

    /// Contextually instantiate a generic function value when a monomorphic
    /// callable type supplies all of its type information. Runtime execution is
    /// still type-erased; this produces the checked callable view used by the
    /// binding or argument site.
    fn value_coerces(&self, from: &Ty, to: &Ty) -> bool {
        if coerces(from, to) {
            return true;
        }
        if matches!((from, to), (Ty::GenericFunc { .. }, Ty::GenericFunc { .. }))
            && callable_bound_accepts(from, to)
        {
            return true;
        }
        if matches!((from, to), (Ty::GenericFunc { .. }, Ty::GenericFunc { .. })) {
            return callable_bound_accepts(from, to);
        }
        if let Ty::Struct(name, _) = from
            && let Some(callable) = self
                .structs
                .get(name)
                .and_then(|info| info.callable_conformance.as_ref())
        {
            return coerces(callable, to);
        }
        let (
            Ty::GenericFunc {
                environment,
                decls,
                params,
                ret,
                required,
                variadic,
                kw_variadic,
                positional_only,
                keyword_only,
                raises,
                error,
                conventions,
                ref_params,
                ref_return,
                ..
            },
            Ty::Func {
                params: expected_params,
                ret: expected_ret,
                ..
            },
        ) = (from, to)
        else {
            return false;
        };
        let mut patterns = params.clone();
        patterns.push((**ret).clone());
        let mut actuals = expected_params.clone();
        actuals.push((**expected_ret).clone());
        let Ok((subst, _)) =
            self.resolve_use_params("<generic callable>", decls, &[], &patterns, &actuals)
        else {
            return false;
        };
        let instantiated = Ty::Func {
            environment: environment.clone(),
            params: params.iter().map(|ty| substitute(ty, &subst)).collect(),
            names: (0..params.len())
                .map(|index| format!("arg{index}"))
                .collect(),
            ret: Box::new(substitute(ret, &subst)),
            required: required.clone(),
            variadic: variadic.as_ref().map(|ty| Box::new(substitute(ty, &subst))),
            kw_variadic: kw_variadic
                .as_ref()
                .map(|ty| Box::new(substitute(ty, &subst))),
            positional_only: *positional_only,
            keyword_only: *keyword_only,
            raises: *raises,
            error: error
                .as_ref()
                .map(|error| Box::new(substitute(error, &subst))),
            conventions: conventions.clone(),
            ref_params: ref_params.clone(),
            ref_return: ref_return.clone(),
        };
        coerces(&instantiated, to)
    }

    /// Checker-selected nominal target when the concrete callable type is
    /// known, otherwise the signature-qualified abstract dispatch target for a
    /// `def(...)` value. Runtime retargeting uses the latter only when the value
    /// is a callable struct.
    fn indirect_callable_target(&self, callable: &Ty) -> Option<String> {
        if let Ty::Struct(name, _) = callable {
            return self
                .structs
                .get(name)
                .and_then(|info| info.callable_target.clone());
        }
        callable_contract_target(callable)
    }

    /// Resolve the callable contract an actual type is *declared* to provide.
    /// Structs participate only through nominal `struct S(def(...))`
    /// conformance; merely defining a shape-compatible `__call__` is not enough.
    fn declared_callable_contract(&self, actual: &Ty) -> Option<Ty> {
        match actual {
            Ty::Func { .. } => Some(actual.clone()),
            Ty::Param {
                callable_bound: Some(bound),
                ..
            } => Some((**bound).clone()),
            Ty::Struct(name, arguments) => {
                let info = self.structs.get(name)?;
                let contract = info.callable_conformance.as_ref()?;
                Some(substitute(contract, &struct_subst(&info.decls, arguments)))
            }
            // A generic function has a callable identity only for a matching
            // generic anonymous contract. `callable_bound_accepts` rejects it
            // when the required contract is monomorphic.
            Ty::GenericFunc { .. } => Some(actual.clone()),
            // An overload set has no unique callable identity.
            Ty::Overload(_) => None,
            _ => None,
        }
    }

    fn validate_callable_parameter_bounds(
        &self,
        generic_name: &str,
        decls: &[ParamDecl],
        arguments: &[TyArg],
    ) -> Result<(), TypeError> {
        let subst = struct_subst(decls, arguments);
        for (decl, argument) in decls.iter().zip(arguments) {
            let ParamDecl::Type {
                name,
                callable_bound: Some(bound),
                ..
            } = decl
            else {
                continue;
            };
            let TyArg::Ty(actual) = argument else {
                return Err(TypeError::Unsupported(format!(
                    "variadic callable type parameter '{}' in '{generic_name}'",
                    name.trim_start_matches('*')
                )));
            };
            let contract = substitute(bound, &subst);
            let Some(actual_contract) = self.declared_callable_contract(actual) else {
                return Err(TypeError::TraitNotSatisfied {
                    param: name.clone(),
                    ty: actual.to_string(),
                    trait_name: contract.to_string(),
                    reason: Some(
                        "requires one monomorphic callable type or nominal callable conformance"
                            .to_string(),
                    ),
                });
            };
            if !callable_bound_accepts(&actual_contract, &contract) {
                return Err(TypeError::TraitNotSatisfied {
                    param: name.clone(),
                    ty: actual.to_string(),
                    trait_name: contract.to_string(),
                    reason: Some(format!(
                        "declared callable contract '{}' is incompatible",
                        actual_contract
                    )),
                });
            }
        }
        Ok(())
    }

    fn implicit_conversion_target(&self, from: &Ty, to: &Ty) -> Result<Option<String>, TypeError> {
        let Ty::Struct(name, args) = to else {
            return Ok(None);
        };
        let Some(info) = self.structs.get(name) else {
            return Ok(None);
        };
        if info.decls.len() != args.len() {
            return Ok(None);
        }
        let subst = struct_subst(&info.decls, args);
        let Some(constructors) = info.methods.get("__init__") else {
            return Ok(None);
        };
        let matches = constructors
            .iter()
            .filter(|sig| {
                sig.implicit
                    && sig.params.len() == 1
                    && coerces(from, &substitute(&sig.params[0], &subst))
            })
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [] => Ok(None),
            [sig] => Ok(Some(if constructors.len() == 1 {
                name.clone()
            } else {
                method_lowered_name(name, "__init__", sig)
            })),
            _ => Err(TypeError::BadCall {
                func: name.clone(),
                reason: format!("ambiguous implicit conversion from '{from}' to '{to}'"),
            }),
        }
    }

    fn record_implicit_conversion(
        &self,
        expression: &Expr,
        from: &Ty,
        to: &Ty,
    ) -> Result<bool, TypeError> {
        if let Ty::Overload(candidates) = from {
            let matches: Vec<_> = candidates
                .iter()
                .filter(|candidate| self.value_coerces(candidate, to))
                .filter_map(|candidate| {
                    let ExprKind::Identifier(name) = &expression.kind else {
                        return None;
                    };
                    callable_lowered_name(name, candidate).map(|target| (candidate, target))
                })
                .collect();
            return match matches.as_slice() {
                [(_, target)] => {
                    self.overload_targets
                        .borrow_mut()
                        .insert(expression.source_span(), target.clone());
                    Ok(true)
                }
                [] => Ok(false),
                _ => Err(TypeError::BadCall {
                    func: "overloaded callable value".to_string(),
                    reason: format!("multiple overloads fit expected type '{to}'"),
                }),
            };
        }
        if let Ty::Simd { dtype, width: 1 } = to
            && splats_to(from, *dtype)
        {
            self.record_literal_materializations(expression, from, to)?;
            return Ok(true);
        }
        if self.value_coerces(from, to) {
            self.record_literal_materializations(expression, from, to)?;
            return Ok(true);
        }
        let Some(target) = self.implicit_conversion_target(from, to)? else {
            return Ok(false);
        };
        self.implicit_conversions
            .borrow_mut()
            .insert(expression.source_span(), target);
        Ok(true)
    }

    /// Retain every exact-literal boundary selected by contextual typing.  The
    /// recursion matters for aggregates: `(1, 2.0)` materialized as
    /// `Tuple[Int, Float64]` has two scalar boundaries, not one tuple cast.
    fn record_literal_materializations(
        &self,
        expression: &Expr,
        from: &Ty,
        to: &Ty,
    ) -> Result<(), TypeError> {
        let scalar_boundary = matches!(from, Ty::IntLiteral | Ty::FloatLiteral)
            && matches!(
                to,
                Ty::Int | Ty::UInt | Ty::Float64 | Ty::Simd { width: 1, .. }
            );
        if scalar_boundary {
            if let Some(value) = self.exact_literal_value(expression)
                && !self.literal_value_fits_target(&value, to)
            {
                return Err(TypeError::TypeMismatch {
                    expected: to.to_string(),
                    found: from.to_string(),
                    context: format!("numeric literal materialization of '{value}'"),
                });
            }
            self.operation_adjustments.borrow_mut().insert(
                expression.source_span(),
                crate::checked::SemanticAdjustment::MaterializeLiteral(to.clone()),
            );
            return Ok(());
        }

        // A public Tuple literal is discovered under the canonical nominal
        // name and checked again under its generated specialization name. The
        // names are implementation identities; literal materialization follows
        // the retained element types across that handoff.
        if let (Some(actual), Some(expected)) = (tuple_elements(from), tuple_elements(to))
            && actual.len() == expected.len()
        {
            let values = match &expression.kind {
                ExprKind::TupleLit(values) => Some(values.as_slice()),
                ExprKind::Call { name, args, .. } if name == "Tuple" => Some(args.as_slice()),
                _ => None,
            };
            if let Some(values) = values {
                for ((value, actual), expected) in values.iter().zip(actual).zip(expected) {
                    self.record_literal_materializations(value, actual, expected)?;
                }
            }
            return Ok(());
        }

        match (from, to) {
            (Ty::Tuple(actual), Ty::Tuple(expected)) if actual.len() == expected.len() => {
                let values = match &expression.kind {
                    ExprKind::TupleLit(values) => Some(values.as_slice()),
                    ExprKind::Call { name, args, .. } if name == "Tuple" => Some(args.as_slice()),
                    _ => None,
                };
                if let Some(values) = values {
                    for ((value, actual), expected) in values.iter().zip(actual).zip(expected) {
                        self.record_literal_materializations(value, actual, expected)?;
                    }
                }
            }
            (Ty::ComptimeList(actual), Ty::ComptimeList(expected)) => {
                let values = match &expression.kind {
                    ExprKind::ListLit(values) => Some(values.as_slice()),
                    ExprKind::Call { name, args, .. } if name == "List" => Some(args.as_slice()),
                    _ => None,
                };
                if let Some(values) = values {
                    for value in values {
                        self.record_literal_materializations(value, actual, expected)?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn exact_literal_value(&self, expression: &Expr) -> Option<CtValue> {
        match self.eval_associated_ct(expression, &HashMap::new()).ok()? {
            value @ (CtValue::IntLiteral(_) | CtValue::FloatLiteral(_)) => Some(value),
            _ => None,
        }
    }

    fn literal_value_fits_target(&self, value: &CtValue, target: &Ty) -> bool {
        match (value, target) {
            (CtValue::IntLiteral(_), Ty::Simd { dtype, width: 1 }) => {
                int_literal_materializes_to_dtype(*dtype)
            }
            (CtValue::FloatLiteral(_), Ty::Simd { dtype, width: 1 }) => dtype.is_float(),
            (value, Ty::Int | Ty::UInt | Ty::Float64) => {
                value.clone().materialize_as(target).is_some()
            }
            _ => false,
        }
    }

    fn resolve_ty_from_anno(&self, ty: &SourceType) -> Result<Ty, TypeError> {
        Ok(match ty {
            SourceType::Int => Ty::Int,
            SourceType::UInt => Ty::UInt,
            SourceType::Bool => Ty::Bool,
            SourceType::String => Ty::String,
            SourceType::Float64 => Ty::Float64,
            SourceType::None => Ty::None,
            SourceType::Func {
                type_params,
                params,
                ret,
                thin,
                capturing,
                raises,
                raises_type,
            } => {
                let environment =
                    self.lower_callable_environment(type_params, *thin, capturing.as_ref())?;
                let function_params: Vec<FnParam> = params
                    .iter()
                    .enumerate()
                    .map(|(index, parameter)| FnParam {
                        name: parameter
                            .name
                            .clone()
                            .unwrap_or_else(|| format!("arg{index}")),
                        ty: parameter.ty.clone(),
                        default: None,
                        kind: crate::ast::ParamKind::Regular,
                        convention: parameter.convention,
                        origin: parameter.origin.clone(),
                    })
                    .collect();
                let regular: Vec<&FnParam> = function_params.iter().collect();
                let parameter_types = function_params
                    .iter()
                    .map(|parameter| self.resolve_ty_from_anno(&parameter.ty))
                    .collect::<Result<Vec<_>, _>>()?;
                let (return_type, ref_return) = match &**ret {
                    SourceType::Ref { referent, origin } => (
                        self.resolve_ty_from_anno(referent)?,
                        Some(Box::new(self.lower_callable_ref_sig(
                            origin.as_ref().ok_or_else(|| {
                                TypeError::Unsupported(
                                    "reference return requires an origin".to_string(),
                                )
                            })?,
                            type_params,
                            &regular,
                        )?)),
                    ),
                    return_type => (self.resolve_ty_from_anno(return_type)?, None),
                };
                Ty::Func {
                    environment,
                    params: parameter_types,
                    names: function_params
                        .iter()
                        .map(|parameter| parameter.name.clone())
                        .collect(),
                    ret: Box::new(return_type),
                    required: vec![true; params.len()],
                    variadic: None,
                    kw_variadic: None,
                    positional_only: None,
                    keyword_only: None,
                    raises: *raises,
                    error: if *raises {
                        Some(Box::new(match raises_type {
                            Some(error) => self.resolve_ty_from_anno(error)?,
                            None => Ty::Error,
                        }))
                    } else {
                        None
                    },
                    conventions: function_params
                        .iter()
                        .map(|parameter| parameter.convention)
                        .collect(),
                    ref_params: Box::new(
                        self.lower_callable_ref_param_sigs(type_params, &regular)?,
                    ),
                    ref_return,
                }
            }
            SourceType::MaterializedCallable(key) => {
                let callable = self.materialized_callables.get(key).ok_or_else(|| {
                    TypeError::InvariantViolation(format!(
                        "compiler-generated callable annotation has unknown id '{key}'"
                    ))
                })?;
                if !matches!(callable, Ty::Func { .. } | Ty::GenericFunc { .. }) {
                    return Err(TypeError::InvariantViolation(
                        "compiler-generated callable annotation contains a non-callable type"
                            .to_string(),
                    ));
                }
                callable.clone()
            }
            SourceType::Ref { referent, origin } => {
                let spec = origin.as_ref().ok_or_else(|| {
                    TypeError::Unsupported(
                        "reference-valued fields require an explicit origin".to_string(),
                    )
                })?;
                let [origin_expr] = spec.as_slice() else {
                    return Err(TypeError::Unsupported(
                        "reference-valued fields currently require one origin parameter"
                            .to_string(),
                    ));
                };
                let ExprKind::Identifier(origin_name) = &origin_expr.kind else {
                    return Err(TypeError::Unsupported(
                        "reference-valued fields require a named origin parameter".to_string(),
                    ));
                };
                if origin_name == "UnsafeAnyOrigin" {
                    return Err(TypeError::Unsupported(
                        "UnsafeAnyOrigin cannot be hidden in a stored reference field".to_string(),
                    ));
                }
                if origin_name == "UntrackedOrigin" {
                    return Ok(Ty::Ref(crate::origin::RefTy {
                        referent: Box::new(self.resolve_ty_from_anno(referent)?),
                        origin: crate::origin::Origin::Untracked { mutable: false },
                        mutability: crate::origin::Mutability::Immutable,
                    }));
                }
                let (index, parameter) = self
                    .enclosing_type_params
                    .iter()
                    .enumerate()
                    .find(|(_, parameter)| {
                        parameter.name == *origin_name && parameter.bounds.as_slice() == ["Origin"]
                    })
                    .ok_or_else(|| TypeError::UndefinedVariable(origin_name.clone()))?;
                let mutability = match parameter.origin_mutability.as_ref().map(|e| &e.kind) {
                    Some(ExprKind::Bool(true)) => crate::origin::Mutability::Mutable,
                    Some(ExprKind::Bool(false)) => crate::origin::Mutability::Immutable,
                    _ => {
                        crate::origin::Mutability::Param(crate::origin::OriginParamId(index as u32))
                    }
                };
                Ty::Ref(crate::origin::RefTy {
                    referent: Box::new(self.resolve_ty_from_anno(referent)?),
                    origin: crate::origin::Origin::Param(crate::origin::OriginParamId(
                        index as u32,
                    )),
                    mutability,
                })
            }
            // A bare name may be an in-scope type parameter (a generic `def`'s
            // `T`) or a struct type, optionally applied to parameter arguments.
            SourceType::Named(name, args) => {
                let existential_trait = args.first().and_then(|argument| match argument {
                    crate::ast::ParamArg::Type(SourceType::Named(trait_name, trait_args))
                        if trait_args.is_empty() =>
                    {
                        Some(trait_name)
                    }
                    crate::ast::ParamArg::Value(Expr {
                        kind: ExprKind::Identifier(trait_name),
                        ..
                    }) => Some(trait_name),
                    _ => None,
                });
                if name == "Some"
                    && args.len() == 1
                    && let Some(trait_name) = existential_trait
                    && (BUILTIN_TRAITS.contains(&trait_name.as_str())
                        || self.traits.contains_key(trait_name))
                {
                    return Ok(Ty::Param {
                        name: format!("Some[{trait_name}]"),
                        bounds: vec![trait_name.clone()],
                        callable_bound: None,
                    });
                }
                if name == "Never" && args.is_empty() {
                    return Ok(Ty::Never);
                }
                if name == "NoneType" && args.is_empty() {
                    return Ok(Ty::None);
                }
                if matches!(name.as_str(), "Slice" | "ContiguousSlice" | "StridedSlice")
                    && args.is_empty()
                {
                    return Ok(Ty::Struct(name.clone(), Vec::new()));
                }
                // Mojo exposes the compile-time `StringLiteral` type. Mojito
                // materializes string literals directly as runtime strings, so
                // it is represented by the existing string type.
                if name == "StringLiteral" && args.is_empty() {
                    return Ok(Ty::String);
                }
                if args.is_empty()
                    && let Some(parameter) = self.lookup_tparam(name)
                {
                    return Ok(parameter);
                }
                // SIMD vector types and their fixed-width scalar aliases.
                if let Some(dtype) = Dtype::from_scalar_alias(name) {
                    if !args.is_empty() {
                        return Err(TypeError::WrongTypeArgCount {
                            name: name.clone(),
                            expected: 0,
                            got: args.len(),
                        });
                    }
                    return Ok(Ty::Simd { dtype, width: 1 });
                }
                if name == "SIMD" {
                    return self.simd_type(args);
                }
                if name == "Scalar" {
                    if args.len() != 1 {
                        return Err(TypeError::WrongTypeArgCount {
                            name: name.clone(),
                            expected: 1,
                            got: args.len(),
                        });
                    }
                    return Ok(simd_ty(dtype_from_arg(&args[0])?, 1));
                }
                if name == "$pack" {
                    return self.tuple_element_types(args).map(Ty::RuntimePack);
                }
                if name == "__RuntimeTuple" {
                    return self.tuple_element_types(args).map(Ty::Tuple);
                }
                if name == "_" && args.is_empty() {
                    return Ok(Ty::Infer);
                }
                if name == "Error" && args.is_empty() {
                    return Ok(Ty::Error);
                }
                // `Variant` is a compiler-provided tagged union even when its
                // stdlib declaration has been module-qualified by the linker.
                if is_variant_name(name) && (name != "Variant" || self.structs.contains_key(name)) {
                    return self.variant_type(args);
                }
                // Literal families are lang items: direct `_` holes are solved
                // from an initializer before ordinary generic-bound checking.
                if name == "List" {
                    return self.list_type(args);
                }
                if name == "Set" {
                    return self.set_type(args);
                }
                if name == "Dict" {
                    return self.dict_type(args);
                }
                if name == "Tuple" {
                    return self.tuple_type(args);
                }
                if let Some(info) = self.structs.get(name) {
                    let decls = info.decls.clone();
                    let (_, tyargs) = self.resolve_use_params(name, &decls, args, &[], &[])?;
                    return Ok(self.struct_instance_type(name, tyargs));
                }
                if self.allow_generated_tuple_forward_types && self.declared_structs.contains(name)
                {
                    return self.generated_tuple_forward_type(name, args);
                }
                if matches!(name.as_str(), "UnsafePointer" | "Pointer") {
                    return self.pointer_type(args);
                }
                return Err(TypeError::UnknownType(name.clone()));
            }
            // `Self.T` — one of the enclosing struct's *type* parameters (a value
            // parameter is not a type, so `Self.n` in type position is an error).
            SourceType::SelfParam(name) => {
                match self.self_decls.iter().find(|d| d.name() == name) {
                    Some(ParamDecl::Type {
                        bounds,
                        callable_bound,
                        ..
                    }) => Ty::Param {
                        name: name.clone(),
                        bounds: bounds.clone(),
                        callable_bound: callable_bound.clone(),
                    },
                    _ => return self.associated_type_for_self(name),
                }
            }
            // Bare `Self` — the enclosing struct type or a trait's abstract Self.
            // Not usable as a type in a value-parameterized struct (a value
            // parameter can't appear in a type).
            SourceType::SelfType => match &self.self_ty {
                Some(Ty::Struct(_, args)) if args.iter().any(|a| matches!(a, TyArg::Val(_))) => {
                    return Err(TypeError::UnknownSelfParam("Self".to_string()));
                }
                Some(ty) => ty.clone(),
                None => return Err(TypeError::UnknownSelfParam("Self".to_string())),
            },
            SourceType::Assoc { base, name } => {
                let base_ty = self.ty_from_anno(base)?;
                self.associated_type_from_base(&base_ty, name)?
            }
            SourceType::IndexedProjection { base, index } => {
                let elements = self.dependent_type_sequence(base)?;
                let index = self.compile_dependent_ct_expr(index)?;
                self.resolve_dependent_index(elements, index, &HashMap::new())?
            }
        })
    }

    /// Resolve only the nominal identity embedded in a compiler-generated
    /// Tuple's concrete metadata. Full parameter arity/bound validation still
    /// occurs at the user's original type use during discovery; this path exists
    /// solely because the generated implementation may be emitted before that
    /// already-checked user struct declaration.
    fn generated_tuple_forward_type(
        &self,
        name: &str,
        arguments: &[crate::ast::ParamArg],
    ) -> Result<Ty, TypeError> {
        fn argument(checker: &Checker, value: &crate::ast::ParamArg) -> Result<TyArg, TypeError> {
            match value {
                crate::ast::ParamArg::Type(ty) => checker.ty_from_anno(ty).map(TyArg::Ty),
                crate::ast::ParamArg::Value(value) => checker
                    .eval_associated_ct(value, &HashMap::new())
                    .map(TyArg::Val),
                crate::ast::ParamArg::Named { value, .. } => argument(checker, value),
            }
        }

        let arguments = if arguments.is_empty() {
            self.predeclared_generated_tuple_arguments
                .get(name)
                .cloned()
                .unwrap_or_default()
        } else {
            arguments
                .iter()
                .map(|value| argument(self, value))
                .collect::<Result<Vec<_>, _>>()?
        };
        Ok(Ty::Struct(name.to_string(), arguments))
    }

    /// Resolve the type-valued sequence at the base of an indexed type
    /// projection. A source value may expose such a sequence through an
    /// associated compile-time member; its runtime value is never inspected.
    fn dependent_type_sequence(&self, projection: &SourceType) -> Result<Vec<Ty>, TypeError> {
        let SourceType::Assoc { base, name } = projection else {
            return Err(TypeError::Unsupported(
                "dependent type indexing requires a type-valued associated member".to_string(),
            ));
        };
        let base_ty = match base.as_ref() {
            SourceType::Named(binding, arguments) if arguments.is_empty() => self
                .lookup(binding)
                .cloned()
                .map(Ok)
                .unwrap_or_else(|| self.ty_from_anno(base))?,
            _ => self.ty_from_anno(base)?,
        };

        // Public Tuple exposes its concrete element pack as `element_types`.
        // Use the checked nominal arguments, never its generated symbol text.
        if name == "element_types"
            && let Some(elements) = tuple_elements(&base_ty)
        {
            return Ok(elements.into_iter().cloned().collect());
        }

        let Ty::Struct(struct_name, arguments) = &base_ty else {
            return Err(TypeError::NoSuchAssociatedType {
                object_type: base_ty.to_string(),
                member: name.clone(),
            });
        };
        let info = self
            .structs
            .get(struct_name)
            .ok_or_else(|| TypeError::UnknownType(struct_name.clone()))?;
        let value = info
            .associated
            .get(name)
            .ok_or_else(|| TypeError::NoSuchAssociatedType {
                object_type: base_ty.to_string(),
                member: name.clone(),
            })?;
        let values = match value {
            CtValue::Tuple(values) | CtValue::List(values) => values,
            _ => {
                return Err(TypeError::NoSuchAssociatedType {
                    object_type: base_ty.to_string(),
                    member: name.clone(),
                });
            }
        };
        let substitution = struct_subst(&info.decls, arguments);
        values
            .iter()
            .map(|value| match value {
                CtValue::Type(ty) => Ok(self.resolve_assoc_ty(&substitute(ty, &substitution))),
                _ => Err(TypeError::NotComptime(format!(
                    "{}.{} contains a non-type value",
                    base_ty, name
                ))),
            })
            .collect()
    }

    /// Collapse an indexed dependent type when its compile-time environment is
    /// concrete; otherwise retain the structural expression in generic
    /// metadata for later specialization.
    fn resolve_dependent_index(
        &self,
        elements: Vec<Ty>,
        index: CtExpr,
        parameters: &HashMap<String, CtValue>,
    ) -> Result<Ty, TypeError> {
        let Some(value) = index.evaluate(parameters) else {
            return Ok(Ty::Dependent(DependentType::Indexed { elements, index }));
        };
        let index_value = match value {
            CtValue::Int(value) => Some(value),
            CtValue::UInt(value) => i64::try_from(value).ok(),
            CtValue::IntLiteral(value) => value.to_i64(),
            _ => None,
        }
        .ok_or_else(|| TypeError::NotComptime("dependent type index must be an Int".to_string()))?;
        let position = usize::try_from(index_value).map_err(|_| {
            TypeError::NotComptime(format!("dependent type index {index_value} is negative"))
        })?;
        elements.get(position).cloned().ok_or_else(|| {
            TypeError::NotComptime(format!(
                "dependent type index {index_value} is out of range for {} element(s)",
                elements.len()
            ))
        })
    }

    /// Resolve dependent leaves after a generic use has supplied its value
    /// parameters. This is a typed walk: the candidate type sequence and the
    /// retained [`CtExpr`] remain structural until the environment is concrete.
    fn resolve_dependent_ty(
        &self,
        ty: &Ty,
        parameters: &HashMap<String, CtValue>,
    ) -> Result<Ty, TypeError> {
        Ok(match ty {
            Ty::Dependent(DependentType::Indexed { elements, index }) => {
                let elements = elements
                    .iter()
                    .map(|element| self.resolve_dependent_ty(element, parameters))
                    .collect::<Result<Vec<_>, _>>()?;
                self.resolve_dependent_index(elements, index.clone(), parameters)?
            }
            Ty::Struct(name, arguments) => Ty::Struct(
                name.clone(),
                arguments
                    .iter()
                    .map(|argument| match argument {
                        TyArg::Ty(ty) => self.resolve_dependent_ty(ty, parameters).map(TyArg::Ty),
                        TyArg::Val(value) => Ok(TyArg::Val(value.clone())),
                    })
                    .collect::<Result<Vec<_>, TypeError>>()?,
            ),
            Ty::ComptimeList(element) => {
                Ty::ComptimeList(Box::new(self.resolve_dependent_ty(element, parameters)?))
            }
            Ty::Tuple(elements) => Ty::Tuple(
                elements
                    .iter()
                    .map(|element| self.resolve_dependent_ty(element, parameters))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            Ty::RuntimePack(elements) => Ty::RuntimePack(
                elements
                    .iter()
                    .map(|element| self.resolve_dependent_ty(element, parameters))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            Ty::VariadicPack(element) => {
                Ty::VariadicPack(Box::new(self.resolve_dependent_ty(element, parameters)?))
            }
            Ty::Variant(alternatives) => Ty::Variant(
                alternatives
                    .iter()
                    .map(|alternative| self.resolve_dependent_ty(alternative, parameters))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            Ty::Pointer { element, origin } => Ty::Pointer {
                element: Box::new(self.resolve_dependent_ty(element, parameters)?),
                origin: origin.clone(),
            },
            Ty::Ref(reference) => Ty::Ref(crate::origin::RefTy {
                referent: Box::new(self.resolve_dependent_ty(&reference.referent, parameters)?),
                origin: reference.origin.clone(),
                mutability: reference.mutability,
            }),
            // A nested generic callable owns its own value-binder scope. Leave
            // that scope structural here; its own invocation resolves it.
            Ty::GenericFunc { .. } => ty.clone(),
            Ty::Func {
                environment,
                params,
                names,
                ret,
                required,
                variadic,
                kw_variadic,
                positional_only,
                keyword_only,
                raises,
                error,
                conventions,
                ref_params,
                ref_return,
            } => Ty::Func {
                environment: environment.clone(),
                params: params
                    .iter()
                    .map(|parameter| self.resolve_dependent_ty(parameter, parameters))
                    .collect::<Result<Vec<_>, _>>()?,
                names: names.clone(),
                ret: Box::new(self.resolve_dependent_ty(ret, parameters)?),
                required: required.clone(),
                variadic: variadic
                    .as_ref()
                    .map(|parameter| {
                        self.resolve_dependent_ty(parameter, parameters)
                            .map(Box::new)
                    })
                    .transpose()?,
                kw_variadic: kw_variadic
                    .as_ref()
                    .map(|parameter| {
                        self.resolve_dependent_ty(parameter, parameters)
                            .map(Box::new)
                    })
                    .transpose()?,
                positional_only: *positional_only,
                keyword_only: *keyword_only,
                raises: *raises,
                error: error
                    .as_ref()
                    .map(|error| self.resolve_dependent_ty(error, parameters).map(Box::new))
                    .transpose()?,
                conventions: conventions.clone(),
                ref_params: ref_params.clone(),
                ref_return: ref_return.clone(),
            },
            Ty::Overload(candidates) => Ty::Overload(
                candidates
                    .iter()
                    .map(|candidate| self.resolve_dependent_ty(candidate, parameters))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            _ => ty.clone(),
        })
    }

    fn value_argument_environment(
        decls: &[ParamDecl],
        arguments: &[TyArg],
    ) -> HashMap<String, CtValue> {
        decls
            .iter()
            .zip(arguments)
            .filter_map(|(declaration, argument)| match argument {
                TyArg::Val(value) => Some((
                    declaration.name().trim_start_matches('*').to_string(),
                    value.clone(),
                )),
                TyArg::Ty(_) => None,
            })
            .collect()
    }

    fn associated_type_for_self(&self, name: &str) -> Result<Ty, TypeError> {
        if let Some(reqs) = self.trait_self_comptime.last()
            && let Some(req) = reqs.get(name)
        {
            return match req {
                CtMemberReq::Type { .. } => Ok(Ty::Assoc {
                    base: Box::new(Ty::SelfType),
                    name: name.to_string(),
                }),
                CtMemberReq::Value(_) => Err(TypeError::NoSuchAssociatedType {
                    object_type: "Self".to_string(),
                    member: name.to_string(),
                }),
            };
        }
        let Some(self_ty) = &self.self_ty else {
            return Err(TypeError::UnknownSelfParam(name.to_string()));
        };
        if let Ty::Struct(sname, _) = self_ty
            && !self.structs.contains_key(sname)
        {
            return Err(TypeError::UnknownSelfParam(name.to_string()));
        }
        self.associated_type_from_base(self_ty, name)
    }

    fn associated_type_from_base(&self, base: &Ty, name: &str) -> Result<Ty, TypeError> {
        match base {
            Ty::Struct(sname, targs) => {
                let info = self
                    .structs
                    .get(sname)
                    .ok_or_else(|| TypeError::UnknownType(sname.clone()))?;
                let value =
                    info.associated
                        .get(name)
                        .ok_or_else(|| TypeError::NoSuchAssociatedType {
                            object_type: base.to_string(),
                            member: name.to_string(),
                        })?;
                let CtValue::Type(ty) = value else {
                    return Err(TypeError::NoSuchAssociatedType {
                        object_type: base.to_string(),
                        member: name.to_string(),
                    });
                };
                let subst = struct_subst(&info.decls, targs);
                Ok(self.resolve_assoc_ty(&substitute(ty, &subst)))
            }
            Ty::Param { bounds, .. } => {
                if self.lookup_trait_assoc_type(bounds, name).is_some() {
                    Ok(Ty::Assoc {
                        base: Box::new(base.clone()),
                        name: name.to_string(),
                    })
                } else {
                    Err(TypeError::NoSuchAssociatedType {
                        object_type: base.to_string(),
                        member: name.to_string(),
                    })
                }
            }
            Ty::Assoc { .. } => Ok(Ty::Assoc {
                base: Box::new(base.clone()),
                name: name.to_string(),
            }),
            _ => Err(TypeError::NoSuchAssociatedType {
                object_type: base.to_string(),
                member: name.to_string(),
            }),
        }
    }

    fn resolve_assoc_ty(&self, ty: &Ty) -> Ty {
        match ty {
            Ty::Param {
                name,
                bounds,
                callable_bound,
            } => Ty::Param {
                name: name.clone(),
                bounds: bounds.clone(),
                callable_bound: callable_bound
                    .as_ref()
                    .map(|bound| Box::new(self.resolve_assoc_ty(bound))),
            },
            Ty::Assoc { base, name } => {
                let base = self.resolve_assoc_ty(base);
                self.associated_type_from_base(&base, name)
                    .unwrap_or_else(|_| Ty::Assoc {
                        base: Box::new(base),
                        name: name.clone(),
                    })
            }
            Ty::Struct(name, args) => {
                Ty::Struct(name.clone(), map_tyargs(args, |t| self.resolve_assoc_ty(t)))
            }
            Ty::ComptimeList(elem) => Ty::ComptimeList(Box::new(self.resolve_assoc_ty(elem))),
            Ty::Tuple(elems) => Ty::Tuple(elems.iter().map(|t| self.resolve_assoc_ty(t)).collect()),
            Ty::RuntimePack(elems) => {
                Ty::RuntimePack(elems.iter().map(|t| self.resolve_assoc_ty(t)).collect())
            }
            Ty::Variant(alternatives) => Ty::Variant(
                alternatives
                    .iter()
                    .map(|ty| self.resolve_assoc_ty(ty))
                    .collect(),
            ),
            Ty::Pointer { element, origin } => Ty::Pointer {
                element: Box::new(self.resolve_assoc_ty(element)),
                origin: origin.clone(),
            },
            Ty::Func {
                environment,
                params,
                names,
                ret,
                required,
                variadic,
                kw_variadic,
                positional_only,
                keyword_only,
                raises,
                error,
                conventions,
                ref_params,
                ref_return,
            } => Ty::Func {
                environment: environment.clone(),
                params: params.iter().map(|p| self.resolve_assoc_ty(p)).collect(),
                names: names.clone(),
                ret: Box::new(self.resolve_assoc_ty(ret)),
                required: required.clone(),
                variadic: variadic
                    .as_ref()
                    .map(|v| Box::new(self.resolve_assoc_ty(v))),
                kw_variadic: kw_variadic
                    .as_ref()
                    .map(|v| Box::new(self.resolve_assoc_ty(v))),
                positional_only: *positional_only,
                keyword_only: *keyword_only,
                raises: *raises,
                error: error
                    .as_ref()
                    .map(|error| Box::new(self.resolve_assoc_ty(error))),
                conventions: conventions.clone(),
                ref_params: ref_params.clone(),
                ref_return: ref_return.clone(),
            },
            Ty::GenericFunc {
                environment,
                decls,
                params,
                names,
                ret,
                required,
                variadic,
                kw_variadic,
                positional_only,
                keyword_only,
                raises,
                error,
                conventions,
                ref_params,
                ref_return,
            } => Ty::GenericFunc {
                environment: environment.clone(),
                decls: decls.clone(),
                params: params.iter().map(|p| self.resolve_assoc_ty(p)).collect(),
                names: names.clone(),
                ret: Box::new(self.resolve_assoc_ty(ret)),
                required: required.clone(),
                variadic: variadic
                    .as_ref()
                    .map(|v| Box::new(self.resolve_assoc_ty(v))),
                kw_variadic: kw_variadic
                    .as_ref()
                    .map(|v| Box::new(self.resolve_assoc_ty(v))),
                positional_only: *positional_only,
                keyword_only: *keyword_only,
                raises: *raises,
                error: error
                    .as_ref()
                    .map(|error| Box::new(self.resolve_assoc_ty(error))),
                conventions: conventions.clone(),
                ref_params: ref_params.clone(),
                ref_return: ref_return.clone(),
            },
            Ty::Overload(candidates) => Ty::Overload(
                candidates
                    .iter()
                    .map(|candidate| self.resolve_assoc_ty(candidate))
                    .collect(),
            ),
            _ => ty.clone(),
        }
    }

    /// Resolve one supplied parameter argument against its declared parameter: a
    /// type parameter takes a type (bound-checked); a value parameter takes a
    /// comptime `Int`. A lone-identifier value argument is reinterpreted as a
    /// type when the parameter is a type parameter.
    fn resolve_param_arg(
        &self,
        decl: &ParamDecl,
        arg: &crate::ast::ParamArg,
    ) -> Result<TyArg, TypeError> {
        use crate::ast::ParamArg;
        match decl {
            ParamDecl::Type { name, bounds, .. } => {
                let ty = match arg {
                    ParamArg::Type(t) => self.ty_from_anno(t)?,
                    ParamArg::Value(Expr {
                        kind: ExprKind::Identifier(id),
                        ..
                    }) => self.ty_from_anno(&SourceType::Named(id.clone(), vec![]))?,
                    ParamArg::Value(_) => {
                        return Err(TypeError::TypeMismatch {
                            expected: "a type".to_string(),
                            found: "a value".to_string(),
                            context: format!("type parameter '{}'", name),
                        });
                    }
                    ParamArg::Named { value, .. } => {
                        return self.resolve_param_arg(decl, value);
                    }
                };
                for bound in bounds {
                    if !self.conforms_to(&ty, bound) {
                        return Err(TypeError::TraitNotSatisfied {
                            param: name.clone(),
                            ty: ty.to_string(),
                            trait_name: bound.clone(),
                            reason: self.trait_failure_reason(&ty, bound),
                        });
                    }
                }
                Ok(TyArg::Ty(ty))
            }
            ParamDecl::Value { name, ty, .. } => match arg {
                ParamArg::Value(expr) => {
                    // Function values are compile-time parameters in source, but
                    // deliberately remain runtime values in the VM ABI: MIR
                    // evaluates the parameter argument into a register and the
                    // call frame reifies it under `name`. `CtValue::Param` is only
                    // the erased generic-identity marker used by this resolver.
                    if matches!(ty.as_ref(), Ty::Func { .. } | Ty::GenericFunc { .. }) {
                        let actual = self.infer(expr)?;
                        if !self.value_coerces(&actual, ty) {
                            return Err(TypeError::TypeMismatch {
                                expected: ty.to_string(),
                                found: actual.to_string(),
                                context: format!("callable-value parameter '{}'", name),
                            });
                        }
                        return Ok(TyArg::Val(CtValue::Param(name.clone())));
                    }
                    let value = self.eval_associated_ct(expr, &HashMap::new())?;
                    let actual =
                        self.ct_value_ty(&value, ty)
                            .ok_or_else(|| TypeError::TypeMismatch {
                                expected: ty.to_string(),
                                found: "a non-materializable compile-time value".to_string(),
                                context: format!("value parameter '{}'", name),
                            })?;
                    if !coerces(&actual, ty) {
                        return Err(TypeError::TypeMismatch {
                            expected: ty.to_string(),
                            found: actual.to_string(),
                            context: format!("value parameter '{}'", name),
                        });
                    }
                    self.record_literal_materializations(expr, &actual, ty)?;
                    let rendered = value.to_string();
                    let value = value.clone().materialize_as(ty).ok_or_else(|| {
                        TypeError::TypeMismatch {
                            expected: ty.to_string(),
                            found: rendered,
                            context: format!("value parameter '{}'", name),
                        }
                    })?;
                    Ok(TyArg::Val(value))
                }
                ParamArg::Type(_) => Err(TypeError::TypeMismatch {
                    expected: "a value".to_string(),
                    found: "a type".to_string(),
                    context: format!("value parameter '{}'", name),
                }),
                ParamArg::Named { value, .. } => self.resolve_param_arg(decl, value),
            },
        }
    }

    /// Resolve `List[T]` from its single type argument.
    fn list_type(&self, args: &[crate::ast::ParamArg]) -> Result<Ty, TypeError> {
        if args.is_empty() {
            return Ok(list_type(Ty::Infer));
        }
        if args.len() != 1 {
            return Err(TypeError::WrongTypeArgCount {
                name: "List".to_string(),
                expected: 1,
                got: args.len(),
            });
        }
        match &args[0] {
            crate::ast::ParamArg::Type(t) => Ok(list_type(self.ty_from_anno(t)?)),
            // A bare-identifier arg is reinterpreted as a type (as elsewhere).
            crate::ast::ParamArg::Value(Expr {
                kind: ExprKind::Identifier(id),
                ..
            }) => Ok(list_type(
                self.ty_from_anno(&SourceType::Named(id.clone(), vec![]))?,
            )),
            crate::ast::ParamArg::Value(_) => Err(TypeError::TypeMismatch {
                expected: "a type".to_string(),
                found: "a value".to_string(),
                context: "List element type".to_string(),
            }),
            crate::ast::ParamArg::Named { .. } => Err(TypeError::TypeMismatch {
                expected: "a positional type argument".to_string(),
                found: "a named argument".to_string(),
                context: "List element type".to_string(),
            }),
        }
    }

    fn collection_type_argument(
        &self,
        collection: &str,
        argument: &crate::ast::ParamArg,
    ) -> Result<Ty, TypeError> {
        match argument {
            crate::ast::ParamArg::Type(ty) => self.ty_from_anno(ty),
            crate::ast::ParamArg::Value(Expr {
                kind: ExprKind::Identifier(name),
                ..
            }) => self.ty_from_anno(&SourceType::Named(name.clone(), Vec::new())),
            crate::ast::ParamArg::Value(_) => Err(TypeError::TypeMismatch {
                expected: "a type".to_string(),
                found: "a value".to_string(),
                context: format!("{collection} type argument"),
            }),
            crate::ast::ParamArg::Named { .. } => Err(TypeError::TypeMismatch {
                expected: "a positional type argument".to_string(),
                found: "a named argument".to_string(),
                context: format!("{collection} type argument"),
            }),
        }
    }

    fn set_type(&self, args: &[crate::ast::ParamArg]) -> Result<Ty, TypeError> {
        if args.is_empty() {
            return Ok(set_type(Ty::Infer));
        }
        if args.len() != 1 {
            return Err(TypeError::WrongTypeArgCount {
                name: "Set".to_string(),
                expected: 1,
                got: args.len(),
            });
        }
        Ok(set_type(self.collection_type_argument("Set", &args[0])?))
    }

    fn dict_type(&self, args: &[crate::ast::ParamArg]) -> Result<Ty, TypeError> {
        if args.is_empty() {
            return Ok(dict_type(Ty::Infer, Ty::Infer));
        }
        if args.len() != 2 {
            return Err(TypeError::WrongTypeArgCount {
                name: "Dict".to_string(),
                expected: 2,
                got: args.len(),
            });
        }
        Ok(dict_type(
            self.collection_type_argument("Dict", &args[0])?,
            self.collection_type_argument("Dict", &args[1])?,
        ))
    }

    /// Resolve Mojito's legacy `UnsafePointer[T]` spelling or current Mojo's
    /// origin-bearing `UnsafePointer[T, origin]`.  The inferred mutability
    /// parameter is intentionally absent from the user-facing argument list.
    fn pointer_type(&self, args: &[crate::ast::ParamArg]) -> Result<Ty, TypeError> {
        if !matches!(args.len(), 1 | 2) {
            return Err(TypeError::WrongTypeArgCount {
                name: "UnsafePointer".to_string(),
                expected: 2,
                got: args.len(),
            });
        }
        let elem = match &args[0] {
            crate::ast::ParamArg::Type(t) => self.ty_from_anno(t)?,
            crate::ast::ParamArg::Value(Expr {
                kind: ExprKind::Identifier(id),
                ..
            }) => self.ty_from_anno(&SourceType::Named(id.clone(), vec![]))?,
            crate::ast::ParamArg::Value(_) => {
                return Err(TypeError::TypeMismatch {
                    expected: "a type".to_string(),
                    found: "a value".to_string(),
                    context: "UnsafePointer element type".to_string(),
                });
            }
            crate::ast::ParamArg::Named { .. } => {
                return Err(TypeError::Unsupported(
                    "named Tuple element arguments".to_string(),
                ));
            }
        };
        let origin = if args.len() == 1 {
            crate::origin::PointerOrigin::Legacy
        } else {
            self.pointer_origin_arg(&args[1])?
        };
        Ok(Ty::Pointer {
            element: Box::new(elem),
            origin,
        })
    }

    fn pointer_origin_arg(
        &self,
        argument: &crate::ast::ParamArg,
    ) -> Result<crate::origin::PointerOrigin, TypeError> {
        use crate::origin::{Mutability, OriginParamId, PointerOrigin};

        let constant = match argument {
            crate::ast::ParamArg::Type(SourceType::SelfParam(name)) => {
                let (index, parameter) = self
                    .enclosing_type_params
                    .iter()
                    .enumerate()
                    .find(|(_, parameter)| {
                        parameter.name == *name && parameter.bounds.as_slice() == ["Origin"]
                    })
                    .ok_or_else(|| TypeError::UnknownSelfParam(name.clone()))?;
                let id = OriginParamId(index as u32);
                let mutability = match parameter.origin_mutability.as_ref().map(|e| &e.kind) {
                    Some(ExprKind::Bool(true)) => Mutability::Mutable,
                    Some(ExprKind::Bool(false)) => Mutability::Immutable,
                    _ => Mutability::Param(id),
                };
                return Ok(PointerOrigin::Param { id, mutability });
            }
            crate::ast::ParamArg::Value(Expr {
                kind: ExprKind::Identifier(name),
                ..
            }) => name.as_str(),
            crate::ast::ParamArg::Type(SourceType::Named(name, arguments))
                if arguments.is_empty() =>
            {
                name.as_str()
            }
            _ => {
                return Err(TypeError::TypeMismatch {
                    expected: "Self.origin or a concrete Origin value".to_string(),
                    found: "a non-origin parameter argument".to_string(),
                    context: "UnsafePointer origin".to_string(),
                });
            }
        };
        match constant {
            "MutUntrackedOrigin" => Ok(PointerOrigin::Untracked { mutable: true }),
            "ImmutUntrackedOrigin" => Ok(PointerOrigin::Untracked { mutable: false }),
            "MutUnsafeAnyOrigin" => Ok(PointerOrigin::UnsafeAny { mutable: true }),
            "ImmutUnsafeAnyOrigin" => Ok(PointerOrigin::UnsafeAny { mutable: false }),
            "StaticConstantOrigin" => Ok(PointerOrigin::Static),
            name => Err(TypeError::UndefinedVariable(name.to_string())),
        }
    }

    /// Resolve `Tuple[T1, …, Tn]` from its type arguments (each a type).
    fn tuple_element_types(&self, args: &[crate::ast::ParamArg]) -> Result<Vec<Ty>, TypeError> {
        let mut elems = Vec::with_capacity(args.len());
        for arg in args {
            elems.push(match arg {
                crate::ast::ParamArg::Type(t) => self.ty_from_anno(t)?,
                // A bare-identifier arg is reinterpreted as a type (as elsewhere).
                crate::ast::ParamArg::Value(Expr {
                    kind: ExprKind::Identifier(id),
                    ..
                }) => self.ty_from_anno(&SourceType::Named(id.clone(), vec![]))?,
                crate::ast::ParamArg::Value(_) => {
                    return Err(TypeError::TypeMismatch {
                        expected: "a type".to_string(),
                        found: "a value".to_string(),
                        context: "Tuple element type".to_string(),
                    });
                }
                crate::ast::ParamArg::Named { .. } => {
                    return Err(TypeError::Unsupported(
                        "named Tuple element arguments".to_string(),
                    ));
                }
            });
        }
        Ok(elems)
    }

    fn tuple_type(&self, args: &[crate::ast::ParamArg]) -> Result<Ty, TypeError> {
        self.tuple_element_types(args)
            .map(|elements| self.public_tuple_type(elements))
    }

    /// Recover the concrete public-Tuple arguments deliberately materialized by
    /// variadic-struct specialization. A user declaration cannot forge the
    /// compiler-generated symbol because `$` is not a source identifier, and
    /// the canonical symbol is recomputed from the semantic element types rather
    /// than decoded from text.
    fn generated_tuple_arguments(
        &self,
        name: &str,
        associated: &[StructComptime],
    ) -> Result<Option<Vec<TyArg>>, TypeError> {
        let Some(element_types) = associated
            .iter()
            .find(|member| member.name == "element_types")
        else {
            return Ok(None);
        };
        let ExprKind::TupleLit(elements) = &element_types.value.kind else {
            return Ok(None);
        };
        let semantic = elements
            .iter()
            .map(|element| match &element.kind {
                ExprKind::TypeValue(ty) => self.ty_from_anno(ty),
                _ => Err(TypeError::NotComptime(
                    "Tuple.element_types must contain only types".to_string(),
                )),
            })
            .collect::<Result<Vec<_>, _>>()?;
        if crate::comptime::tuple_specialization_symbol(&semantic) != name {
            return Ok(None);
        }
        Ok(Some(semantic.into_iter().map(TyArg::Ty).collect()))
    }

    /// Construct the checked identity of an ordinary struct or of a concrete
    /// erased specialization whose source parameters have become fixed facts.
    fn struct_instance_type(&self, name: &str, arguments: Vec<TyArg>) -> Ty {
        let arguments = self
            .structs
            .get(name)
            .and_then(|info| info.fixed_arguments.clone())
            .unwrap_or(arguments);
        Ty::Struct(name.to_string(), arguments)
    }

    /// Resolve the alternatives of `Variant[T1, ..., Tn]`.  Alternative order
    /// is significant because it becomes the runtime tag; duplicate types would
    /// make `isa[T]` and `value[T]` ambiguous and are rejected here.
    fn variant_type(&self, args: &[crate::ast::ParamArg]) -> Result<Ty, TypeError> {
        if args.is_empty() {
            return Err(TypeError::WrongTypeArgCount {
                name: "Variant".to_string(),
                expected: 1,
                got: 0,
            });
        }
        let mut alternatives = Vec::with_capacity(args.len());
        for arg in args {
            let alternative = self.type_param_argument(arg, "Variant alternative")?;
            if alternatives.contains(&alternative) {
                return Err(TypeError::Unsupported(format!(
                    "Variant contains duplicate alternative '{alternative}'"
                )));
            }
            alternatives.push(alternative);
        }
        Ok(Ty::Variant(alternatives))
    }

    fn type_param_argument(
        &self,
        arg: &crate::ast::ParamArg,
        context: &str,
    ) -> Result<Ty, TypeError> {
        match arg {
            crate::ast::ParamArg::Type(ty) => self.ty_from_anno(ty),
            crate::ast::ParamArg::Value(Expr {
                kind: ExprKind::Identifier(name),
                ..
            }) => self.ty_from_anno(&SourceType::Named(name.clone(), Vec::new())),
            crate::ast::ParamArg::Value(_) => Err(TypeError::TypeMismatch {
                expected: "a type".to_string(),
                found: "a value".to_string(),
                context: context.to_string(),
            }),
            crate::ast::ParamArg::Named { .. } => Err(TypeError::Unsupported(format!(
                "named arguments are not supported in {context}"
            ))),
        }
    }

    /// Resolve `SIMD[DType.<dt>, width]` from its two parameter arguments to its
    /// `(dtype, width)` (raw — not canonicalized).
    fn simd_dims(&self, args: &[crate::ast::ParamArg]) -> Result<(Dtype, i64), TypeError> {
        if args.len() != 2 {
            return Err(TypeError::WrongTypeArgCount {
                name: "SIMD".to_string(),
                expected: 2,
                got: args.len(),
            });
        }
        let dtype = dtype_from_arg(&args[0])?;
        let width = if matches!(
            &args[1],
            crate::ast::ParamArg::Value(Expr { kind: ExprKind::Identifier(name), .. }) if name == "_"
        ) {
            -1
        } else {
            self.simd_width(&args[1])?
        };
        Ok((dtype, width))
    }

    /// The (canonicalized) `Ty` for `SIMD[DType.<dt>, width]` — a width-1 `float64`
    /// resolves to `Ty::Float64` (the unification).
    fn simd_type(&self, args: &[crate::ast::ParamArg]) -> Result<Ty, TypeError> {
        let (dtype, width) = self.simd_dims(args)?;
        Ok(simd_ty(dtype, width))
    }

    /// Evaluate a SIMD width argument: a comptime `Int` that is a power of two.
    fn simd_width(&self, arg: &crate::ast::ParamArg) -> Result<i64, TypeError> {
        let w = match arg {
            crate::ast::ParamArg::Value(expr) => {
                let value = self.eval_ct(expr)?;
                value
                    .to_i64()
                    .ok_or_else(|| TypeError::BadSimdWidth(value.to_string()))?
            }
            crate::ast::ParamArg::Type(_) => {
                return Err(TypeError::BadSimdWidth("a type".to_string()));
            }
            crate::ast::ParamArg::Named { .. } => {
                return Err(TypeError::BadSimdWidth("a named argument".to_string()));
            }
        };
        if w >= 1 && (w & (w - 1)) == 0 {
            Ok(w)
        } else {
            Err(TypeError::BadSimdWidth(w.to_string()))
        }
    }

    /// If `name` is a generic type parameter currently in scope, return its
    /// complete checked type-parameter fact.
    fn lookup_tparam(&self, name: &str) -> Option<Ty> {
        self.tparams
            .iter()
            .rev()
            .find_map(|scope| scope.get(name))
            .cloned()
    }

    /// Evaluate a compile-time `Int` expression: literals, `comptime` constants,
    /// and `+ - * // % **` / unary `-`. Rejects anything non-comptime (a value
    /// parameter, a call, a non-`Int` operation).
    fn eval_ct(&self, expr: &Expr) -> Result<crate::literal::IntLiteral, TypeError> {
        match &expr.kind {
            ExprKind::Int(n) => Ok(n.clone()),
            ExprKind::Identifier(name) => self
                .comptimes
                .get(name)
                .cloned()
                .ok_or_else(|| TypeError::NotComptime(name.clone())),
            ExprKind::Prefix(PrefixOp::Neg, e) => Ok(self.eval_ct(e)?.neg()),
            ExprKind::Infix(op, l, r) => {
                let (a, b) = (self.eval_ct(l)?, self.eval_ct(r)?);
                match op {
                    InfixOp::Add => Ok(a.add(&b)),
                    InfixOp::Sub => Ok(a.sub(&b)),
                    InfixOp::Mul => Ok(a.mul(&b)),
                    InfixOp::FloorDiv => a.floor_div(&b).ok_or_else(|| {
                        TypeError::NotComptime("compile-time division by zero".to_string())
                    }),
                    InfixOp::Mod => a.floor_mod(&b).ok_or_else(|| {
                        TypeError::NotComptime("compile-time modulo by zero".to_string())
                    }),
                    InfixOp::Pow => a.pow(&b).ok_or_else(|| {
                        TypeError::NotComptime(
                            "invalid or resource-limited compile-time power".to_string(),
                        )
                    }),
                    InfixOp::Shl => a.shl(&b).ok_or_else(|| {
                        TypeError::NotComptime(
                            "invalid or resource-limited compile-time shift".to_string(),
                        )
                    }),
                    InfixOp::Shr => a.shr(&b).ok_or_else(|| {
                        TypeError::NotComptime(
                            "invalid or resource-limited compile-time shift".to_string(),
                        )
                    }),
                    InfixOp::BitAnd => Ok(a.bitand(&b)),
                    InfixOp::BitOr => Ok(a.bitor(&b)),
                    InfixOp::BitXor => Ok(a.bitxor(&b)),
                    _ => Err(TypeError::NotComptime(
                        "unsupported comptime operation".to_string(),
                    )),
                }
            }
            _ => Err(TypeError::NotComptime(
                "not a comptime Int expression".to_string(),
            )),
        }
    }

    /// Classify a trait comptime-member annotation. In Mojo terms,
    /// `comptime count: Int` requires an integer compile-time value, while
    /// `comptime Element: AnyType` requires a type-valued member whose type
    /// conforms to `AnyType`.
    fn ct_member_req_from_anno(&self, ty: &SourceType) -> Result<CtMemberReq, TypeError> {
        if let SourceType::Named(name, args) = ty
            && name == "$trait_composition"
        {
            let mut bounds = Vec::with_capacity(args.len());
            for argument in args {
                let crate::ast::ParamArg::Type(SourceType::Named(bound, bound_args)) = argument
                else {
                    return Err(TypeError::Unsupported(
                        "associated type bounds must be trait names".to_string(),
                    ));
                };
                if !bound_args.is_empty() {
                    return Err(TypeError::Unsupported(
                        "associated type bounds cannot take arguments".to_string(),
                    ));
                }
                self.check_trait_name(bound)?;
                if !bounds.contains(bound) {
                    bounds.push(bound.clone());
                }
            }
            return Ok(CtMemberReq::Type { bounds });
        }
        if let SourceType::Named(name, args) = ty
            && args.is_empty()
            && (BUILTIN_TRAITS.contains(&name.as_str()) || self.traits.contains_key(name))
        {
            self.check_trait_name(name)?;
            return Ok(CtMemberReq::Type {
                bounds: vec![name.clone()],
            });
        }
        Ok(CtMemberReq::Value(Box::new(self.ty_from_anno(ty)?)))
    }

    fn check_struct_associated(
        &self,
        associated: &[StructComptime],
    ) -> Result<HashMap<String, CtValue>, TypeError> {
        let mut out = HashMap::new();
        for member in associated {
            if out.contains_key(&member.name) {
                return Err(TypeError::Redeclaration(member.name.clone()));
            }
            let value = self.eval_associated_ct(&member.value, &out)?;
            out.insert(member.name.clone(), value);
        }
        Ok(out)
    }

    /// Evaluate a struct-level associated comptime value. This intentionally
    /// accepts type-valued expressions in addition to runtime-materializable
    /// constants because associated facts are type metadata, not executable code.
    fn eval_associated_ct(
        &self,
        expr: &Expr,
        associated: &HashMap<String, CtValue>,
    ) -> Result<CtValue, TypeError> {
        match &expr.kind {
            ExprKind::Int(n) => Ok(CtValue::IntLiteral(n.clone())),
            ExprKind::Float(value) => Ok(CtValue::FloatLiteral(value.clone())),
            ExprKind::Bool(b) => Ok(CtValue::Bool(*b)),
            ExprKind::Str(s) => Ok(CtValue::Str(s.clone())),
            ExprKind::TypeValue(ty) => self.ty_from_anno(ty).map(Box::new).map(CtValue::Type),
            ExprKind::Identifier(name) => {
                if let Some(n) = self.comptimes.get(name) {
                    return Ok(CtValue::IntLiteral(n.clone()));
                }
                self.ty_value_from_name(name, &[])
                    .ok_or_else(|| TypeError::NotComptime(name.clone()))
            }
            ExprKind::TypeApply { name, args } => self
                .ty_value_from_name(name, args)
                .ok_or_else(|| TypeError::NotComptime(name.clone())),
            ExprKind::Member { object, field } => {
                if let ExprKind::Identifier(s) = &object.kind
                    && s == "Self"
                {
                    if let Some(value) = self.self_param_ct_value(field) {
                        return Ok(value);
                    }
                    if let Some(value) = associated.get(field) {
                        return Ok(value.clone());
                    }
                    return Err(TypeError::UnknownSelfParam(field.clone()));
                }
                Err(TypeError::NotComptime(
                    "unsupported associated comptime member access".to_string(),
                ))
            }
            ExprKind::Prefix(PrefixOp::Neg, e) => match self.eval_associated_ct(e, associated)? {
                CtValue::Int(n) => n.checked_neg().map(CtValue::Int).ok_or_else(|| {
                    TypeError::NotComptime("compile-time integer overflow".to_string())
                }),
                CtValue::IntLiteral(n) => Ok(CtValue::IntLiteral(n.neg())),
                CtValue::FloatLiteral(value) => Ok(CtValue::FloatLiteral(value.neg())),
                _ => Err(TypeError::NotComptime(
                    "unary '-' expects a comptime numeric value".to_string(),
                )),
            },
            ExprKind::Infix(op, l, r) => self.eval_associated_ct_infix(
                *op,
                self.eval_associated_ct(l, associated)?,
                self.eval_associated_ct(r, associated)?,
            ),
            ExprKind::TupleLit(elems) => elems
                .iter()
                .map(|e| self.eval_associated_ct(e, associated))
                .collect::<Result<Vec<_>, _>>()
                .map(CtValue::Tuple),
            ExprKind::ListLit(elems) => elems
                .iter()
                .map(|e| self.eval_associated_ct(e, associated))
                .collect::<Result<Vec<_>, _>>()
                .map(CtValue::List),
            _ => Err(TypeError::NotComptime(
                "not an associated comptime expression".to_string(),
            )),
        }
    }

    fn eval_associated_ct_infix(
        &self,
        op: InfixOp,
        left: CtValue,
        right: CtValue,
    ) -> Result<CtValue, TypeError> {
        let unsupported =
            || TypeError::NotComptime("unsupported associated comptime operation".to_string());
        match (left, right) {
            (CtValue::Int(left), CtValue::Int(right)) => {
                let value = match op {
                    InfixOp::Add => left.checked_add(right),
                    InfixOp::Sub => left.checked_sub(right),
                    InfixOp::Mul => left.checked_mul(right),
                    InfixOp::FloorDiv if right != 0 => left.checked_div_euclid(right),
                    InfixOp::Mod if right != 0 => left.checked_rem_euclid(right),
                    InfixOp::Pow if right >= 0 => u32::try_from(right)
                        .ok()
                        .and_then(|exponent| left.checked_pow(exponent)),
                    _ => return Err(unsupported()),
                };
                value
                    .map(CtValue::Int)
                    .ok_or_else(|| TypeError::NotComptime("compile-time integer overflow".into()))
            }
            (CtValue::IntLiteral(left), CtValue::IntLiteral(right)) => {
                let value = match op {
                    InfixOp::Add => Some(CtValue::IntLiteral(left.add(&right))),
                    InfixOp::Sub => Some(CtValue::IntLiteral(left.sub(&right))),
                    InfixOp::Mul => Some(CtValue::IntLiteral(left.mul(&right))),
                    InfixOp::Div => crate::literal::FloatLiteral::from_int(&left)
                        .div(&crate::literal::FloatLiteral::from_int(&right))
                        .map(CtValue::FloatLiteral),
                    InfixOp::FloorDiv => left.floor_div(&right).map(CtValue::IntLiteral),
                    InfixOp::Mod => left.floor_mod(&right).map(CtValue::IntLiteral),
                    InfixOp::Pow => left.pow(&right).map(CtValue::IntLiteral),
                    _ => return Err(unsupported()),
                };
                value.ok_or_else(|| {
                    TypeError::NotComptime("invalid exact compile-time arithmetic".into())
                })
            }
            (CtValue::FloatLiteral(left), CtValue::FloatLiteral(right)) => {
                let value = match op {
                    InfixOp::Add => Some(left.add(&right)),
                    InfixOp::Sub => Some(left.sub(&right)),
                    InfixOp::Mul => Some(left.mul(&right)),
                    InfixOp::Div => left.div(&right),
                    InfixOp::FloorDiv => left.floor_div(&right),
                    InfixOp::Mod => left.floor_mod(&right),
                    InfixOp::Pow => right
                        .to_int_if_whole()
                        .and_then(|exponent| left.pow_int(&exponent)),
                    _ => return Err(unsupported()),
                };
                value.map(CtValue::FloatLiteral).ok_or_else(|| {
                    TypeError::NotComptime("invalid exact compile-time arithmetic".into())
                })
            }
            (CtValue::Int(value), CtValue::IntLiteral(literal)) => self.eval_associated_ct_infix(
                op,
                CtValue::IntLiteral(value.into()),
                CtValue::IntLiteral(literal),
            ),
            (CtValue::IntLiteral(literal), CtValue::Int(value)) => self.eval_associated_ct_infix(
                op,
                CtValue::IntLiteral(literal),
                CtValue::IntLiteral(value.into()),
            ),
            (CtValue::IntLiteral(integer), CtValue::FloatLiteral(float)) => self
                .eval_associated_ct_infix(
                    op,
                    CtValue::FloatLiteral(crate::literal::FloatLiteral::from_int(&integer)),
                    CtValue::FloatLiteral(float),
                ),
            (CtValue::FloatLiteral(float), CtValue::IntLiteral(integer)) => self
                .eval_associated_ct_infix(
                    op,
                    CtValue::FloatLiteral(float),
                    CtValue::FloatLiteral(crate::literal::FloatLiteral::from_int(&integer)),
                ),
            _ => Err(unsupported()),
        }
    }

    fn self_param_ct_value(&self, name: &str) -> Option<CtValue> {
        self.self_decls.iter().find_map(|decl| match decl {
            ParamDecl::Type {
                name: n,
                bounds,
                callable_bound,
                ..
            } if n == name => Some(CtValue::Type(Box::new(Ty::Param {
                name: n.clone(),
                bounds: bounds.clone(),
                callable_bound: callable_bound.clone(),
            }))),
            ParamDecl::Value { name: n, .. } if n == name => Some(CtValue::Param(n.clone())),
            _ => None,
        })
    }

    fn ty_value_from_name(&self, name: &str, args: &[crate::ast::ParamArg]) -> Option<CtValue> {
        if args.is_empty() {
            if let Some(ty) = scalar_type_name(name) {
                return Some(CtValue::Type(Box::new(ty)));
            }
            if name == "None" {
                return Some(CtValue::Type(Box::new(Ty::None)));
            }
        }
        self.ty_from_anno(&SourceType::Named(name.to_string(), args.to_vec()))
            .ok()
            .map(|ty| CtValue::Type(Box::new(ty)))
    }

    pub fn check_program(&mut self, stmts: &[Stmt]) -> Result<(), TypeError> {
        self.declared_structs
            .extend(stmts.iter().filter_map(|statement| match &statement.kind {
                StmtKind::Struct { name, .. } => Some(name.clone()),
                _ => None,
            }));
        // Phase one for generated public Tuples: recover every concrete pack
        // identity from its materialized `element_types` member before any
        // declaration body is checked. Reverse transforms can be requested in
        // both directions, so no sequential declaration order can make both
        // result types complete. The forward-type gate remains compiler-owned;
        // user declarations are still checked in source order below.
        let saved_forward_types =
            std::mem::replace(&mut self.allow_generated_tuple_forward_types, true);
        for statement in stmts {
            let StmtKind::Struct {
                name,
                type_params,
                associated,
                ..
            } = &statement.kind
            else {
                continue;
            };
            if !(name.starts_with("Tuple$") || name.contains("$Tuple$")) {
                continue;
            }
            let saved_type_params =
                std::mem::replace(&mut self.enclosing_type_params, type_params.clone());
            let arguments = self.generated_tuple_arguments(name, associated);
            self.enclosing_type_params = saved_type_params;
            if let Some(arguments) = arguments? {
                self.predeclared_generated_tuple_arguments
                    .insert(name.clone(), arguments);
            }
        }
        self.allow_generated_tuple_forward_types = saved_forward_types;
        // `ret = None` marks "not inside a function", so a top-level `return`
        // is rejected; `in_loop = false` likewise rejects a top-level `break`.
        self.check_block(stmts, None, false)
    }

    /// Check the statements of a block in the current scope. `ret` is the
    /// enclosing function's declared return type (or `None` at module level);
    /// `in_loop` is true inside a `while`/`for` body (gating `break`/`continue`).
    fn check_block(
        &mut self,
        stmts: &[Stmt],
        ret: Option<&Ty>,
        in_loop: bool,
    ) -> Result<(), TypeError> {
        for stmt in stmts {
            self.check_stmt(stmt, ret, in_loop)?;
        }
        Ok(())
    }

    /// Check a block in a fresh nested scope (the body of an `if`/`elif`/`else`
    /// or loop). The new scope is popped before returning.
    fn check_scoped_block(
        &mut self,
        stmts: &[Stmt],
        ret: Option<&Ty>,
        in_loop: bool,
    ) -> Result<(), TypeError> {
        self.push_scope();
        let result = self.check_block(stmts, ret, in_loop);
        self.pop_scope();
        result
    }

    fn check_stmt(
        &mut self,
        stmt: &Stmt,
        ret: Option<&Ty>,
        in_loop: bool,
    ) -> Result<(), TypeError> {
        match &stmt.kind {
            StmtKind::RefDecl { name, value } => {
                let reference = match self.infer(value)? {
                    Ty::Ref(reference) => reference,
                    _ => {
                        // Ordinary expression inference reads through a
                        // reference-returning method to its referent.  The
                        // method call nevertheless left an exact checked
                        // `ReferenceResult` adjustment, which a `ref` binding
                        // must retain instead of trying to reinterpret the call
                        // expression as a syntactic place. `reference_actual`
                        // consumes that checked result and remains the shared
                        // fallback for an ordinary place binding.
                        self.reference_actual(value)?
                    }
                };
                let mutable = reference.mutability == crate::origin::Mutability::Mutable;
                // A reference to an ordinary projection below a named owned
                // interior (for example `dict[key].field`) carries the full
                // projected generation, even though only the nested index
                // expression originally introduced the interior fact. Retain
                // that canonical origin on the binding expression so MIR can
                // establish the projected generation rather than degrading it
                // to an untracked ordinary place loan.
                if let crate::origin::Origin::Place(origin) = &reference.origin
                    && origin
                        .path
                        .iter()
                        .any(|segment| matches!(segment, crate::origin::OriginSeg::Interior(_)))
                {
                    self.interior_references
                        .borrow_mut()
                        .insert(value.source_span(), origin.clone());
                }
                self.reference_value_uses
                    .borrow_mut()
                    .insert(value.source_span(), mutable);
                let binding_ty = Ty::Ref(reference);
                self.binding_types
                    .borrow_mut()
                    .insert(stmt.source_span(), binding_ty.clone());
                self.declare_with_mutability(name, binding_ty, mutable)?;
                self.record_statement_binding(stmt, name);
                Ok(())
            }
            StmtKind::VarDecl { name, ty, value } => {
                if matches!(value.kind, ExprKind::Uninitialized) {
                    let Some(annotation) = ty else {
                        return Err(TypeError::Unsupported(
                            "an uninitialized variable requires a type annotation".to_string(),
                        ));
                    };
                    let declared = self.ty_from_anno(annotation)?;
                    self.declare(name, declared)?;
                    self.record_statement_binding(stmt, name);
                    if let Some(owner) = self.lookup_owner(name) {
                        self.uninitialized.borrow_mut().insert(owner);
                    }
                    return Ok(());
                }
                self.register_named_bindings(value)?;
                let contextual = ty
                    .as_ref()
                    .map(|annotation| self.ty_from_anno(annotation))
                    .transpose()?;
                let found = match contextual.as_ref() {
                    Some(expected) => self.infer_with_expected(value, expected, true)?,
                    None => self.infer(value)?,
                };
                self.check_consuming(value, &found, &format!("variable '{name}'"))?;
                let declared = match ty {
                    // Annotated: the value must coerce to the annotation.
                    Some(anno) => {
                        let expected = contextual.clone().unwrap_or(self.ty_from_anno(anno)?);
                        if contains_infer(&expected) {
                            if contains_infer(&found) {
                                return Err(TypeError::CannotInferTypeParam {
                                    name: expected.to_string(),
                                    param: "_".to_string(),
                                });
                            }
                            found.clone()
                        } else {
                            if !self.record_implicit_conversion(value, &found, &expected)? {
                                return Err(TypeError::TypeMismatch {
                                    expected: expected.to_string(),
                                    found: found.to_string(),
                                    context: format!("variable '{}'", name),
                                });
                            }
                            expected
                        }
                    }
                    // Inferred `var x = e`: declare the value's materialized type.
                    None if matches!(value.kind, ExprKind::TypeApply { .. })
                        && self
                            .overload_targets
                            .borrow()
                            .contains_key(&value.source_span()) =>
                    {
                        // Explicitly specializing an Origin parameter produces
                        // a first-class, non-capturing function value whose
                        // checked `Ty::Func` contains the bound origin. Plain
                        // inferred function/closure values remain non-escaping.
                        found.clone()
                    }
                    None => self.inferred_binding_ty(&found, name)?,
                };
                if ty.is_none() {
                    self.record_literal_materializations(value, &found, &declared)?;
                }
                self.binding_types
                    .borrow_mut()
                    .insert(value.source_span(), declared.clone());
                if self.is_implicitly_deletable(&declared) {
                    self.explicit_destroy_deletability
                        .borrow_mut()
                        .bindings
                        .insert(value.source_span());
                }
                let (aggregate_origins, aggregate_field_origins) =
                    if !matches!(declared, Ty::Ref(_)) && self.type_carries_loans(&declared) {
                        (
                            self.aggregate_origins(value),
                            self.aggregate_field_origins(value),
                        )
                    } else {
                        (Vec::new(), HashMap::new())
                    };
                self.declare(name, declared)?;
                self.record_statement_binding(stmt, name);
                self.set_aggregate_origins(name, aggregate_origins);
                self.set_aggregate_field_origins(name, aggregate_field_origins);
                Ok(())
            }

            StmtKind::Assign { name, value } => {
                self.register_named_bindings(value)?;
                self.check_capture_access(name, true)?;
                let found = self.infer(value)?;
                self.check_consuming(value, &found, &format!("assignment to '{name}'"))?;
                // Mojo treats a bare assignment in a function as a local
                // introduction unless that name is already local to this
                // function. Its initializer may still read an outer binding.
                let target = if let Some(&base) = self.function_bases.last() {
                    self.scopes[base..]
                        .iter()
                        .rev()
                        .find_map(|s| s.get(name))
                        .cloned()
                        .or_else(|| {
                            // A mutable captured variable is updated by reference;
                            // an immutable capture (notably `comptime`) is instead
                            // shadowed by a new function-local binding.
                            let mutable = self.mutable_scopes[..base]
                                .iter()
                                .rev()
                                .find_map(|s| s.get(name))
                                .copied()
                                .unwrap_or(false);
                            if mutable {
                                self.scopes[..base]
                                    .iter()
                                    .rev()
                                    .find_map(|s| s.get(name))
                                    .cloned()
                            } else {
                                None
                            }
                        })
                } else {
                    self.lookup(name).cloned()
                };
                match target {
                    // Re-assignment: the value must keep the variable's type.
                    Some(target) => {
                        if !self.is_binding_mutable(name) {
                            return Err(TypeError::ImmutableBinding(name.clone()));
                        }
                        match &target {
                            // Assignment to a reference binding writes through
                            // the handle; it does not rebind the handle.  Use
                            // the reference's checked origin so replacing a
                            // whole container also expires references into its
                            // owned interior regions.  The handle itself stays
                            // valid across that write.
                            Ty::Ref(reference) => {
                                if let crate::origin::Origin::Place(base) = &reference.origin {
                                    self.record_origin_invalidation(
                                        value.source_span(),
                                        base.clone(),
                                        self.lookup_owner(name),
                                    );
                                }
                            }
                            _ => {
                                if let Some(owner) = self.lookup_owner(name) {
                                    self.record_owner_invalidation(
                                        value.source_span(),
                                        owner,
                                        Vec::new(),
                                    );
                                }
                            }
                        }
                        let (aggregate_origins, aggregate_field_origins) =
                            if !matches!(target, Ty::Ref(_)) && self.type_carries_loans(&target) {
                                (
                                    self.aggregate_origins(value),
                                    self.aggregate_field_origins(value),
                                )
                            } else {
                                (Vec::new(), HashMap::new())
                            };
                        let target = match target {
                            Ty::Ref(reference) => *reference.referent,
                            other => other,
                        };
                        // Assigning a closure could move it to an outer binding.
                        if matches!(
                            found,
                            Ty::Func { .. } | Ty::GenericFunc { .. } | Ty::Overload(_)
                        ) {
                            return Err(TypeError::ClosureEscape);
                        }
                        if !self.record_implicit_conversion(value, &found, &target)? {
                            return Err(TypeError::TypeMismatch {
                                expected: target.to_string(),
                                found: found.to_string(),
                                context: format!("assignment to '{}'", name),
                            });
                        }
                        if let Some(owner) = self.lookup_owner(name) {
                            self.uninitialized.borrow_mut().remove(&owner);
                        }
                        self.set_aggregate_origins(name, aggregate_origins);
                        self.set_aggregate_field_origins(name, aggregate_field_origins);
                        Ok(())
                    }
                    // `x = e` on an undeclared name is a **var-less introduction**
                    // (implicit declaration). Mojo allows it; mojito parses and
                    // type-checks it by binding the materialized type. Later
                    // lowering retains the explicit unsupported boundary.
                    None => {
                        let declared = self.inferred_binding_ty(&found, name)?;
                        self.record_literal_materializations(value, &found, &declared)?;
                        let (aggregate_origins, aggregate_field_origins) = if !matches!(
                            declared,
                            Ty::Ref(_)
                        ) && self
                            .type_carries_loans(&declared)
                        {
                            (
                                self.aggregate_origins(value),
                                self.aggregate_field_origins(value),
                            )
                        } else {
                            (Vec::new(), HashMap::new())
                        };
                        self.declare_function_implicit(name, declared)?;
                        self.record_statement_binding(stmt, name);
                        self.set_aggregate_origins(name, aggregate_origins);
                        self.set_aggregate_field_origins(name, aggregate_field_origins);
                        if let Some(owner) = self.lookup_owner(name) {
                            self.uninitialized.borrow_mut().remove(&owner);
                        }
                        Ok(())
                    }
                }
            }

            StmtKind::AugAssign { place, op, value } => {
                if let Some(root) = place_root_name(place) {
                    self.check_capture_access(root, true)?;
                }
                let nominal_subscript = match &place.kind {
                    ExprKind::Index { object, .. }
                    | ExprKind::Slice { object, .. }
                    | ExprKind::MultiIndex { object, .. } => {
                        matches!(self.infer(object)?, Ty::Struct(name, _) if self.structs.contains_key(&name))
                    }
                    _ => false,
                };
                // `target OP= value` means `target = target OP value`: the place
                // must be writable, and the result of the operator must keep the
                // place's type. A nominal subscript is two selected calls, not a
                // raw writable projection: infer its getter first, then select
                // the setter against the computed operator result.
                let shared_argument_sources = if nominal_subscript {
                    match &place.kind {
                        ExprKind::Index { index, .. } => vec![index.source_span()],
                        ExprKind::MultiIndex { args, .. } => args
                            .iter()
                            .filter_map(|argument| match argument {
                                SubscriptArg::Index(index) => Some(index.source_span()),
                                SubscriptArg::Slice { .. } => None,
                            })
                            .collect(),
                        ExprKind::Slice { .. } => Vec::new(),
                        _ => unreachable!("nominal subscript classification"),
                    }
                } else {
                    Vec::new()
                };
                let shared_adjustments = self.snapshot_value_adjustments(&shared_argument_sources);
                let target = if nominal_subscript {
                    self.infer(place)?
                } else {
                    self.check_place(place)?
                };
                let getter = if nominal_subscript {
                    let contract = self
                        .selected_calls
                        .borrow()
                        .get(&place.source_span())
                        .cloned()
                        .ok_or_else(|| {
                            TypeError::InvariantViolation(
                                "augmented nominal subscript lost its getter contract".to_string(),
                            )
                        })?;
                    if let Some(reference) = &contract.reference_result
                        && reference.mutability != crate::origin::Mutability::Mutable
                    {
                        return Err(TypeError::ImmutableBinding(
                            "immutable reference returned by '__getitem__'".to_string(),
                        ));
                    }
                    Some(contract)
                } else {
                    None
                };
                if !nominal_subscript {
                    self.record_place_write_invalidation(place.source_span(), place);
                }
                if let Some(Ty::Ref(reference)) = self.place_storage_ty(place)
                    && reference.mutability != crate::origin::Mutability::Mutable
                {
                    return Err(TypeError::ImmutableBinding(
                        "immutable reference field".to_string(),
                    ));
                }
                let result = self.infer_infix(None, *op, place, value)?;
                if !coerces(&result, &target) {
                    return Err(TypeError::TypeMismatch {
                        expected: target.to_string(),
                        found: result.to_string(),
                        context: "augmented assignment".to_string(),
                    });
                }
                if nominal_subscript {
                    let site = place.source_span();
                    let getter = getter.expect("nominal getter was captured above");
                    if getter.reference_result.is_some() {
                        // A value-returning augmented subscript reaches the
                        // setter checker below, which also records descriptor
                        // shape. A mutable-reference getter has no setter, so a
                        // plain index must retain that metadata here for MIR.
                        if matches!(place.kind, ExprKind::Index { .. }) {
                            self.subscript_descriptors
                                .borrow_mut()
                                .entry(site.clone())
                                .or_insert((vec![None], false));
                        }
                        self.expression_types
                            .borrow_mut()
                            .insert(site.clone(), target.clone());
                        self.operation_adjustments.borrow_mut().insert(
                            site,
                            crate::checked::SemanticAdjustment::AugmentedSubscript(Box::new(
                                crate::checked::CheckedAugmentedSubscript {
                                    getter,
                                    setter: None,
                                    operand_ty: target,
                                    result_ty: result,
                                    value_source: None,
                                },
                            )),
                        );
                        return Ok(());
                    }
                    // The getter and setter share syntax, not parameter
                    // adaptation or call effects. Freeze the getter above, then
                    // return those source-keyed compatibility tables to their
                    // pre-getter state before selecting the setter.
                    self.restore_value_adjustments(&shared_adjustments);
                    self.remove_call_boundary_invalidations(&site, &getter.boundary);

                    let mut computed = Expr::new(ExprKind::None, crate::token::DUMMY_SPAN);
                    computed.source = place.source.clone();
                    let value_source = computed.source_span();
                    self.expression_types
                        .borrow_mut()
                        .insert(value_source.clone(), result.clone());
                    self.check_nominal_subscript_assignment(place, &computed)?
                        .ok_or_else(|| {
                            TypeError::InvariantViolation(
                                "augmented nominal subscript lost its setter selection".to_string(),
                            )
                        })?;
                    let mut setter = self
                        .selected_calls
                        .borrow()
                        .get(&site)
                        .cloned()
                        .ok_or_else(|| {
                            TypeError::InvariantViolation(
                                "augmented nominal subscript lost its setter contract".to_string(),
                            )
                        })?;
                    self.implicit_conversions.borrow_mut().remove(&value_source);
                    self.expression_types.borrow_mut().remove(&value_source);
                    if matches!(
                        self.operation_adjustments.borrow().get(&value_source),
                        Some(crate::checked::SemanticAdjustment::MaterializeLiteral(_))
                    ) {
                        self.operation_adjustments
                            .borrow_mut()
                            .remove(&value_source);
                    }
                    let before_write = self
                        .interior_invalidations
                        .borrow()
                        .get(&site)
                        .cloned()
                        .unwrap_or_default();
                    self.record_place_write_invalidation(site.clone(), place);
                    if let Some(after_write) = self.interior_invalidations.borrow().get(&site) {
                        let existing = setter.boundary.invalidations.clone();
                        let additional = after_write
                            .iter()
                            .filter(|fact| !before_write.contains(fact))
                            .filter(|fact| !existing.contains(fact))
                            .cloned()
                            .collect::<Vec<_>>();
                        setter.boundary.invalidations.extend(additional);
                    }
                    self.restore_value_adjustments(&shared_adjustments);
                    self.remove_call_boundary_invalidations(&site, &setter.boundary);
                    self.selected_calls
                        .borrow_mut()
                        .insert(site.clone(), setter.clone());
                    self.expression_types
                        .borrow_mut()
                        .insert(site.clone(), target.clone());
                    self.operation_adjustments.borrow_mut().insert(
                        site,
                        crate::checked::SemanticAdjustment::AugmentedSubscript(Box::new(
                            crate::checked::CheckedAugmentedSubscript {
                                getter,
                                setter: Some(setter),
                                operand_ty: target,
                                result_ty: result,
                                value_source: Some(value_source),
                            },
                        )),
                    );
                }
                Ok(())
            }

            // Tuple unpacking `a, b = t`: `t` must be a tuple of matching arity; each
            // target (a NAME or a place) receives the corresponding element type. A
            // NAME follows the assignment rules (re-assign if in scope, else a
            // var-less introduction).
            StmtKind::Unpack { targets, value } => {
                let vt = self.infer(value)?;
                let Some(elems) = tuple_elements(&vt) else {
                    return Err(TypeError::TypeMismatch {
                        expected: "a tuple".to_string(),
                        found: vt.to_string(),
                        context: "tuple unpacking".to_string(),
                    });
                };
                if elems.len() != targets.len() {
                    return Err(TypeError::TypeMismatch {
                        expected: format!("a {}-element tuple", targets.len()),
                        found: vt.to_string(),
                        context: "tuple unpacking".to_string(),
                    });
                }
                let elems = elems.into_iter().cloned().collect::<Vec<_>>();
                let mut unpack_plan = elems
                    .iter()
                    .cloned()
                    .map(|ty| crate::checked::CheckedTupleUnpackElement {
                        ty,
                        accessor: None,
                        reference: None,
                    })
                    .collect::<Vec<_>>();
                if let Ty::Struct(name, _) = &vt
                    && let Some(info) = self.structs.get(name)
                    && let Some(family) = dependent_index_accessor_family(info)
                {
                    let value_receiver = !is_place_expr(value);
                    let self_reference = if value_receiver {
                        None
                    } else {
                        Some(self.reference_actual(value)?)
                    };
                    for (index, element) in unpack_plan.iter_mut().enumerate() {
                        let method = if value_receiver {
                            format!("{}${index}", family.value)
                        } else {
                            format!("{}${index}", family.place)
                        };
                        let signature = info
                            .methods
                            .get(&method)
                            .and_then(|overloads| overloads.first())
                            .ok_or_else(|| {
                                if value_receiver {
                                    TypeError::NonCopyable {
                                        ty: vt.to_string(),
                                        context: "unpacking an rvalue Tuple requires implicitly copyable elements"
                                            .to_string(),
                                    }
                                } else {
                                    TypeError::InvariantViolation(format!(
                                        "generated Tuple '{name}' is missing accessor '{method}'"
                                    ))
                                }
                            })?;
                        element.ty = signature.ret.clone();
                        element.accessor = Some(format!("{name}.{method}"));
                        if let Some(reference_return) = &signature.ref_return {
                            let self_reference = self_reference.as_ref().ok_or_else(|| {
                                TypeError::InvariantViolation(format!(
                                    "generated Tuple value accessor '{method}' returns a reference"
                                ))
                            })?;
                            let origin = substitute_sig_origin_with_self(
                                &reference_return.origin,
                                &[],
                                Some(self_reference.origin.clone()),
                            );
                            let mutability = match reference_return.mutability {
                                crate::origin::SigMutability::Immutable => {
                                    crate::origin::Mutability::Immutable
                                }
                                crate::origin::SigMutability::Mutable => {
                                    crate::origin::Mutability::Mutable
                                }
                                _ if self_reference.mutability
                                    == crate::origin::Mutability::Mutable =>
                                {
                                    crate::origin::Mutability::Mutable
                                }
                                _ => crate::origin::Mutability::Immutable,
                            };
                            element.reference = Some(crate::origin::RefTy {
                                referent: Box::new(signature.ret.clone()),
                                origin,
                                mutability,
                            });
                        }
                    }
                }
                self.tuple_unpack_plans
                    .borrow_mut()
                    .insert(value.source_span(), unpack_plan);
                for (target, elem) in targets.iter().zip(&elems) {
                    match &target.kind {
                        ExprKind::Identifier(name) => match self.lookup(name).cloned() {
                            Some(existing) => {
                                self.check_capture_access(name, true)?;
                                if !self.is_binding_mutable(name) {
                                    return Err(TypeError::ImmutableBinding(name.clone()));
                                }
                                if !coerces(elem, &existing) {
                                    return Err(TypeError::TypeMismatch {
                                        expected: existing.to_string(),
                                        found: elem.to_string(),
                                        context: format!("unpacking into '{name}'"),
                                    });
                                }
                                if matches!(existing, Ty::Ref(_)) {
                                    self.record_place_write_invalidation(
                                        target.source_span(),
                                        target,
                                    );
                                } else if let Some(owner) = self.lookup_owner(name) {
                                    self.record_owner_invalidation(
                                        target.source_span(),
                                        owner,
                                        Vec::new(),
                                    );
                                }
                            }
                            None => {
                                let declared = self.inferred_binding_ty(elem, name)?;
                                self.declare(name, declared)?;
                            }
                        },
                        _ => {
                            if let Some(root) = place_root_name(target) {
                                self.check_capture_access(root, true)?;
                            }
                            let target_ty = self.check_place(target)?;
                            self.record_place_write_invalidation(target.source_span(), target);
                            if !coerces(elem, &target_ty) {
                                return Err(TypeError::TypeMismatch {
                                    expected: target_ty.to_string(),
                                    found: elem.to_string(),
                                    context: "unpacking into a place".to_string(),
                                });
                            }
                        }
                    }
                    if let ExprKind::Identifier(name) = &target.kind
                        && let Some(owner) = self.lookup_owner(name)
                    {
                        self.expression_bindings
                            .borrow_mut()
                            .insert(target.source_span(), owner);
                        // Unpack targets are binder/place syntax, not ordinary
                        // inferred expressions, so retain their complete slot
                        // metadata explicitly for checked MIR verification and
                        // for a nested closure that captures the introduction.
                        if let Some(storage_ty) = self.lookup(name).cloned() {
                            let value_ty = match &storage_ty {
                                Ty::Ref(reference) => (*reference.referent).clone(),
                                value => value.clone(),
                            };
                            self.expression_types
                                .borrow_mut()
                                .insert(target.source_span(), value_ty);
                            self.expression_place_types
                                .borrow_mut()
                                .insert(target.source_span(), storage_ty.clone());
                            self.binding_types
                                .borrow_mut()
                                .insert(target.source_span(), storage_ty);
                        }
                    }
                }
                Ok(())
            }

            // `with` blocks parse, but the context-manager (`__enter__`/`__exit__`)
            // protocol is deferred — flagged, like the other parse-only constructs.
            StmtKind::With { .. } => Err(TypeError::Unsupported("with statement".to_string())),

            StmtKind::SetPlace { place, value } => {
                if let Some(root) = place_root_name(place) {
                    self.check_capture_access(root, true)?;
                }
                // Nominal subscript assignment is a method call, not a raw
                // storage projection. Resolve it with the RHS present so
                // overloads, conversions, ownership conventions, origins,
                // aliases, captures, and raising behavior all use the same
                // machinery as explicit `.__setitem__(...)` syntax.
                if self
                    .check_nominal_subscript_assignment(place, value)?
                    .is_some()
                {
                    self.record_place_write_invalidation(place.source_span(), place);
                    return Ok(());
                }
                // The place must be a writable location (a field/index chain
                // rooted at a mutable variable or `mut self`); the value must
                // keep the place's type. A width-1 SIMD target (a lane write, or
                // a scalar-alias field) additionally accepts a splatting literal.
                let target = self.check_place(place)?;
                self.record_place_write_invalidation(place.source_span(), place);
                let storage = self.place_storage_ty(place);
                let found = self.infer(value)?;
                if let Some(Ty::Ref(expected_reference)) = &storage {
                    let initializes_reference =
                        self.self_initializing && place_root_name(place) == Some("self");
                    if initializes_reference {
                        let actual = self.infer_reference_value(value).ok_or_else(|| {
                            TypeError::TypeMismatch {
                                expected: format!("ref {}", expected_reference.referent),
                                found: found.to_string(),
                                context: "reference field initialization".to_string(),
                            }
                        })?;
                        if !coerces(&actual.referent, &expected_reference.referent)
                            || (expected_reference.mutability == crate::origin::Mutability::Mutable
                                && actual.mutability != crate::origin::Mutability::Mutable)
                        {
                            return Err(TypeError::TypeMismatch {
                                expected: format!("ref {}", expected_reference.referent),
                                found: format!("ref {}", actual.referent),
                                context: "reference field initialization".to_string(),
                            });
                        }
                        self.reference_value_uses.borrow_mut().insert(
                            value.source_span(),
                            expected_reference.mutability == crate::origin::Mutability::Mutable,
                        );
                    } else if expected_reference.mutability != crate::origin::Mutability::Mutable {
                        return Err(TypeError::ImmutableBinding(
                            "immutable reference field".to_string(),
                        ));
                    }
                }
                let ok = self.record_implicit_conversion(value, &found, &target)?;
                if !ok {
                    return Err(TypeError::TypeMismatch {
                        expected: target.to_string(),
                        found: found.to_string(),
                        context: "assignment target".to_string(),
                    });
                }
                // Existing out-self initialization permits a non-Copyable
                // generic parameter to be installed without spelling a second
                // source-level transfer. Preserve that constructor convention;
                // this marker is needed only for an actual Copyable place read.
                if !matches!(storage, Some(Ty::Ref(_))) && self.is_copyable(&found) {
                    self.check_consuming(value, &found, "assignment target")?;
                }
                Ok(())
            }

            // `raises` is parsed but its effect is not analyzed (deferred).
            StmtKind::Def {
                name,
                type_params,
                params,
                positional_only,
                keyword_only,
                captures,
                ret: ret_anno,
                body,
                raises,
                raises_type,
                decorators,
                where_clause,
            } => {
                if self.structs.contains_key(name) {
                    return Err(TypeError::Redeclaration(name.clone()));
                }
                // Free functions, including generic functions, share one binder
                // for regular, `*args`, and homogeneous `**kwargs` parameters.
                if let Some(feature) = Self::advanced_param_feature(
                    params,
                    *positional_only,
                    *keyword_only,
                    false,
                    false,
                    false,
                ) {
                    return Err(TypeError::Unsupported(feature.to_string()));
                }
                // A `*args` variadic is supported on non-generic functions; any
                // regular parameters after it are keyword-only.
                let variadic_idx = params
                    .iter()
                    .position(|p| p.kind == crate::ast::ParamKind::Variadic);
                let kw_variadic_idx = params
                    .iter()
                    .position(|p| p.kind == crate::ast::ParamKind::KwVariadic);
                // Regular (non-variadic) parameters, over which arity is computed.
                let regular: Vec<&crate::ast::FnParam> = params
                    .iter()
                    .filter(|p| p.kind == crate::ast::ParamKind::Regular)
                    .collect();
                let out_params: Vec<_> = regular
                    .iter()
                    .copied()
                    .filter(|p| matches!(p.convention, Some(crate::ast::ArgConvention::Out)))
                    .collect();
                if out_params.len() > 1 {
                    return Err(TypeError::Unsupported(
                        "multiple named 'out' results".to_string(),
                    ));
                }
                let named_result = out_params.first().copied();
                if named_result.is_some() && ret_anno.is_some() {
                    return Err(TypeError::Unsupported(
                        "a function cannot declare both a named result and '->' return type"
                            .to_string(),
                    ));
                }
                let caller_regular: Vec<_> = regular
                    .iter()
                    .copied()
                    .filter(|p| !matches!(p.convention, Some(crate::ast::ArgConvention::Out)))
                    .collect();
                let pos_only = regular_marker_index(params, *positional_only);
                let kw_only = effective_keyword_only_index(params, *keyword_only, variadic_idx);
                let required = required_mask(&caller_regular, kw_only)?;
                self.validate_origin_signature(type_params, params, None)?;
                let mut decls = self.classify_params(type_params)?;
                let mut function_assumptions = HashSet::new();
                if let Some(condition) = where_clause {
                    let constraint = self.compile_generic_constraint(condition)?;
                    let mut facts = Vec::new();
                    guaranteed_conformance_atoms(&constraint, &mut facts);
                    function_assumptions.extend(facts.into_iter().map(
                        |(parameter, trait_name)| {
                            (parameter.trim_start_matches('*').to_string(), trait_name)
                        },
                    ));
                    let Some(last) = decls.last_mut() else {
                        return Err(TypeError::Unsupported(
                            "a where clause requires compile-time parameters".to_string(),
                        ));
                    };
                    match last {
                        ParamDecl::Type { constraints, .. }
                        | ParamDecl::Value { constraints, .. } => constraints.push(constraint),
                    }
                }
                self.generic_parameters.borrow_mut().insert(
                    crate::checked::GenericSite::Function {
                        module: stmt.module.clone(),
                        declaration: stmt.span,
                        syntax: stmt.syntax_id,
                    },
                    decls.clone(),
                );
                // Type parameters are in scope while resolving the signature and
                // checking the body (as bare `T`).
                self.tparams.push(type_scope(&decls));

                let signature = (|| {
                    let param_tys = self.param_tys(params)?;
                    let ret_ty = match (ret_anno, named_result) {
                        (Some(SourceType::Ref { referent, .. }), _) => {
                            self.ty_from_anno(referent)?
                        }
                        (Some(t), _) => self.ty_from_anno(t)?,
                        (None, Some(result)) => self.ty_from_anno(&result.ty)?,
                        (None, None) => Ty::None,
                    };
                    Ok::<_, TypeError>((param_tys, ret_ty))
                })();
                let (param_tys, ret_ty) = match signature {
                    Ok(sig) => sig,
                    Err(e) => {
                        self.tparams.pop();
                        return Err(e);
                    }
                };
                let ref_params = lower_ref_param_sigs(type_params, &caller_regular)?;
                let ref_return = match ret_anno {
                    Some(SourceType::Ref { origin, .. }) => Some(lower_ref_sig(
                        origin.as_ref().ok_or_else(|| {
                            TypeError::Unsupported(
                                "reference return requires an origin".to_string(),
                            )
                        })?,
                        type_params,
                        &regular,
                    )?),
                    _ => None,
                };
                for (param, ty) in param_tys.iter().enumerate() {
                    self.declaration_types.borrow_mut().insert(
                        crate::checked::AnnotationSite::FunctionParam {
                            module: stmt.module.clone(),
                            declaration: stmt.span,
                            syntax: stmt.syntax_id,
                            param,
                        },
                        ty.clone(),
                    );
                }
                self.declaration_types.borrow_mut().insert(
                    crate::checked::AnnotationSite::FunctionReturn {
                        module: stmt.module.clone(),
                        declaration: stmt.span,
                        syntax: stmt.syntax_id,
                    },
                    ret_ty.clone(),
                );
                // A default value must fit its parameter's type.
                for (p, pty) in params.iter().zip(&param_tys) {
                    if let Some(d) = &p.default {
                        let dty = match self.infer(d) {
                            Ok(t) => t,
                            Err(e) => {
                                self.tparams.pop();
                                return Err(e);
                            }
                        };
                        if !coerces(&dty, pty) {
                            self.tparams.pop();
                            return Err(TypeError::TypeMismatch {
                                expected: pty.to_string(),
                                found: dty.to_string(),
                                context: format!("default value of '{}'", p.name),
                            });
                        }
                    }
                }

                // Bind the function in the enclosing scope before checking its
                // body, so it can call itself (recursion). A generic `def`
                // becomes a `GenericFunc` (its call sites infer/supply parameters).
                let declared_error = self.declared_error(*raises, raises_type.as_ref())?;
                let effect_raises = declared_error.as_ref().is_some_and(|ty| *ty != Ty::Never);
                let parameter_closure = decorators
                    .iter()
                    .any(|decorator| decorator.path.len() == 1 && decorator.path[0] == "parameter");
                let initial_environment = if parameter_closure {
                    crate::origin::CallableEnvironment::Capturing(
                        crate::origin::CaptureOriginSet::Infer,
                    )
                } else if self.function_bases.is_empty() {
                    crate::origin::CallableEnvironment::Thin
                } else {
                    crate::origin::CallableEnvironment::Default
                };
                self.declaration_effects.borrow_mut().insert(
                    crate::checked::AnnotationSite::FunctionReturn {
                        module: stmt.module.clone(),
                        declaration: stmt.span,
                        syntax: stmt.syntax_id,
                    },
                    crate::checked::DeclarationEffect {
                        raises: effect_raises,
                        error: effect_raises.then(|| declared_error.clone()).flatten(),
                        returns_reference: ref_return.is_some(),
                    },
                );
                let fn_ty = if decls.is_empty() {
                    let regular_tys: Vec<Ty> = params
                        .iter()
                        .zip(&param_tys)
                        .filter(|(p, _)| {
                            p.kind == crate::ast::ParamKind::Regular
                                && !matches!(p.convention, Some(crate::ast::ArgConvention::Out))
                        })
                        .map(|(_, ty)| ty.clone())
                        .collect();
                    Ty::Func {
                        environment: initial_environment.clone(),
                        params: regular_tys,
                        names: caller_regular.iter().map(|p| p.name.clone()).collect(),
                        ret: Box::new(ret_ty.clone()),
                        required,
                        variadic: variadic_idx.map(|vi| Box::new(param_tys[vi].clone())),
                        kw_variadic: kw_variadic_idx
                            .map(|index| Box::new(param_tys[index].clone())),
                        positional_only: pos_only,
                        keyword_only: kw_only,
                        raises: effect_raises,
                        error: declared_error.clone().map(Box::new),
                        conventions: caller_regular.iter().map(|p| p.convention).collect(),
                        ref_params: Box::new(ref_params.clone()),
                        ref_return: ref_return.clone().map(Box::new),
                    }
                } else {
                    let regular_tys: Vec<Ty> = params
                        .iter()
                        .zip(&param_tys)
                        .filter(|(p, _)| {
                            p.kind == crate::ast::ParamKind::Regular
                                && !matches!(p.convention, Some(crate::ast::ArgConvention::Out))
                        })
                        .map(|(_, ty)| ty.clone())
                        .collect();
                    Ty::GenericFunc {
                        environment: initial_environment,
                        decls: decls.clone(),
                        params: regular_tys,
                        names: caller_regular.iter().map(|p| p.name.clone()).collect(),
                        ret: Box::new(ret_ty.clone()),
                        required,
                        variadic: variadic_idx.map(|vi| Box::new(param_tys[vi].clone())),
                        kw_variadic: kw_variadic_idx
                            .map(|index| Box::new(param_tys[index].clone())),
                        positional_only: pos_only,
                        keyword_only: kw_only,
                        raises: effect_raises,
                        error: declared_error.clone().map(Box::new),
                        conventions: caller_regular.iter().map(|p| p.convention).collect(),
                        ref_params: Box::new(ref_params.clone()),
                        ref_return: ref_return.clone().map(Box::new),
                    }
                };
                self.declaration_types.borrow_mut().insert(
                    crate::checked::AnnotationSite::FunctionType {
                        module: stmt.module.clone(),
                        declaration: stmt.span,
                        syntax: stmt.syntax_id,
                    },
                    fn_ty.clone(),
                );
                if let Err(e) = self.declare(name, fn_ty.clone()) {
                    self.tparams.pop();
                    return Err(e);
                }
                self.record_statement_binding(stmt, name);
                self.register_callable_origins(
                    name,
                    callable_origin_signature(type_params, &caller_regular),
                );
                let capture_policy = if self.function_bases.is_empty() {
                    if captures.is_some() {
                        self.tparams.pop();
                        return Err(TypeError::Unsupported(
                            "unified capture lists are valid only on nested functions".to_string(),
                        ));
                    }
                    None
                } else {
                    let mut entries = HashMap::new();
                    let mut checked_captures = Vec::new();
                    if let Some(captures) = captures {
                        for capture in &captures.entries {
                            if let Err(error) = self.check_capture_access(&capture.name, false) {
                                self.tparams.pop();
                                return Err(error);
                            }
                            let Some(scope) = self.binding_scope(&capture.name) else {
                                self.tparams.pop();
                                return Err(TypeError::UndefinedVariable(capture.name.clone()));
                            };
                            if scope == 0 {
                                self.tparams.pop();
                                return Err(TypeError::Unsupported(format!(
                                    "module binding '{}' is not a closure capture",
                                    capture.name
                                )));
                            }
                            if capture.kind == crate::ast::CaptureKind::Mut
                                && !self.is_binding_mutable(&capture.name)
                            {
                                self.tparams.pop();
                                return Err(TypeError::ImmutableBinding(capture.name.clone()));
                            }
                            if let Err(error) =
                                self.check_capture_capability(&capture.name, capture.kind)
                            {
                                self.tparams.pop();
                                return Err(error);
                            }
                            let binding = self.lookup_owner(&capture.name).ok_or_else(|| {
                                TypeError::InvariantViolation(format!(
                                    "capture '{}' lost its checked binding",
                                    capture.name
                                ))
                            })?;
                            let ty = self.lookup(&capture.name).cloned().ok_or_else(|| {
                                TypeError::InvariantViolation(format!(
                                    "capture '{}' lost its checked storage type",
                                    capture.name
                                ))
                            })?;
                            checked_captures.push(self.checked_capture(
                                &capture.name,
                                binding,
                                ty,
                                capture.kind,
                            ));
                            entries.insert(capture.name.clone(), capture.kind);
                        }
                    }
                    self.declaration_captures
                        .borrow_mut()
                        .insert(stmt.source_span(), checked_captures);
                    Some(CapturePolicy {
                        base: self.scopes.len(),
                        function_name: name.clone(),
                        declaration: stmt.source_span(),
                        entries,
                        default: captures
                            .as_ref()
                            .and_then(|list| list.default)
                            .or_else(|| parameter_closure.then_some(crate::ast::CaptureKind::Read)),
                    })
                };
                self.assumed_conformances.push(function_assumptions);
                for (param, ty) in param_tys.iter().enumerate() {
                    if self.is_implicitly_deletable(ty) {
                        self.explicit_destroy_deletability
                            .borrow_mut()
                            .declarations
                            .insert(crate::checked::AnnotationSite::FunctionParam {
                                module: stmt.module.clone(),
                                declaration: stmt.span,
                                syntax: stmt.syntax_id,
                                param,
                            });
                    }
                }
                self.push_scope();
                self.function_bases.push(self.scopes.len() - 1);
                if let Some(policy) = capture_policy {
                    self.capture_contexts.borrow_mut().push(policy);
                }
                self.raising_context.push(declared_error);
                let mut result = Ok(());
                // Value parameters are ordinary `Int` locals in the body.
                for d in &decls {
                    if let ParamDecl::Value { name, ty, .. } = d {
                        result = self.declare_immutable(
                            name.trim_start_matches('*'),
                            if matches!(d, ParamDecl::Value { variadic: true, .. }) {
                                Ty::VariadicPack(ty.clone())
                            } else {
                                (**ty).clone()
                            },
                        );
                        if result.is_err() {
                            break;
                        }
                    }
                }
                if result.is_ok() {
                    for (param, ty) in params.iter().zip(&param_tys) {
                        // A `*args` parameter is compiler pack storage inside the
                        // body; it must not impersonate the nominal stdlib List.
                        let bind_ty = match param.kind {
                            crate::ast::ParamKind::Variadic => match ty {
                                Ty::RuntimePack(elements) => Ty::Tuple(elements.clone()),
                                _ => Ty::VariadicPack(Box::new(ty.clone())),
                            },
                            crate::ast::ParamKind::KwVariadic => self.kwargs_collector_ty(
                                ty.clone(),
                                &format!("keyword collector '{}'", param.name),
                            )?,
                            crate::ast::ParamKind::Regular => ty.clone(),
                        };
                        // Duplicate parameter names are a redeclaration.
                        result = self.declare_with_mutability(
                            &param.name,
                            bind_ty.clone(),
                            param.kind == crate::ast::ParamKind::KwVariadic
                                || matches!(param.convention, Some(crate::ast::ArgConvention::Out))
                                || ref_parameter_is_writable(param, type_params),
                        );
                        if result.is_ok()
                            && matches!(param.convention, Some(crate::ast::ArgConvention::Ref))
                        {
                            self.register_reference_parameter(
                                &param.name,
                                bind_ty.clone(),
                                ref_parameter_is_writable(param, type_params),
                            );
                        }
                        if result.is_ok()
                            && !matches!(bind_ty, Ty::Ref(_))
                            && self.type_carries_loans(&bind_ty)
                            && let Some(owner) = self.lookup_owner(&param.name)
                        {
                            self.set_aggregate_origins(
                                &param.name,
                                vec![crate::origin::Origin::Place(crate::origin::OriginPlace {
                                    root: owner,
                                    path: Vec::new(),
                                })],
                            );
                        }
                        if result.is_err() {
                            break;
                        }
                    }
                }
                // A function body is a fresh loop context: `break`/`continue`
                // do not cross into a nested `def`.
                if result.is_ok() {
                    let owners: Vec<_> = caller_regular
                        .iter()
                        .map(|param| {
                            self.lookup_owner(&param.name)
                                .expect("bound function parameter")
                        })
                        .collect();
                    let base = *self
                        .function_bases
                        .last()
                        .expect("function scope is active");
                    self.aggregate_escape_contexts
                        .push((base, owners.iter().copied().collect()));
                    self.return_ref_contracts.push(
                        ref_return
                            .clone()
                            .map(|signature| (signature, owners, None)),
                    );
                    self.named_result_context.push(named_result.is_some());
                    result = self.check_block(body, Some(&ret_ty), false);
                    self.named_result_context.pop();
                    self.return_ref_contracts.pop();
                    self.aggregate_escape_contexts.pop();
                }
                // A function with a non-`None` return type must return on every
                // path (falling off the end would yield `None`).
                if result.is_ok()
                    && named_result.is_none()
                    && ret_ty != Ty::None
                    && !definitely_returns(body)
                {
                    result = Err(TypeError::MissingReturn(name.clone()));
                }
                if result.is_ok()
                    && let Some(named_result) = named_result
                    && !definitely_initializes_named_result(body, &named_result.name)
                {
                    result = Err(TypeError::MissingReturn(name.clone()));
                }
                if result.is_ok() {
                    let captures = self
                        .declaration_captures
                        .borrow()
                        .get(&stmt.source_span())
                        .cloned()
                        .unwrap_or_default();
                    let concrete = crate::origin::CaptureOriginSet::concrete(
                        captures
                            .iter()
                            .flat_map(|capture| capture.origins.iter().cloned()),
                    );
                    let environment = if parameter_closure || !captures.is_empty() {
                        crate::origin::CallableEnvironment::Capturing(concrete)
                    } else {
                        crate::origin::CallableEnvironment::Thin
                    };
                    let finalized = with_callable_environment(fn_ty.clone(), environment);
                    let function_scope = *self
                        .function_bases
                        .last()
                        .expect("function scope remains active through finalization");
                    if let Some(existing) = function_scope
                        .checked_sub(1)
                        .and_then(|scope| self.scopes.get_mut(scope))
                        .and_then(|scope| scope.get_mut(name))
                        && !matches!(existing, Ty::Overload(_))
                    {
                        *existing = finalized.clone();
                    }
                    self.declaration_types.borrow_mut().insert(
                        crate::checked::AnnotationSite::FunctionType {
                            module: stmt.module.clone(),
                            declaration: stmt.span,
                            syntax: stmt.syntax_id,
                        },
                        finalized,
                    );
                }
                self.pop_scope();
                self.function_bases.pop();
                if !self.function_bases.is_empty() {
                    self.capture_contexts.borrow_mut().pop();
                }
                self.raising_context.pop();
                self.assumed_conformances.pop();
                self.tparams.pop();
                result
            }

            StmtKind::Struct {
                name,
                type_params,
                conforms,
                callable_conformance,
                conformance_conditions,
                fields,
                associated,
                methods,
                fieldwise_init,
                decorators,
            } => {
                if self.lookup(name).is_some() {
                    return Err(TypeError::Redeclaration(name.clone()));
                }
                self.check_struct(&StructDeclaration {
                    module: &stmt.module,
                    name,
                    type_params,
                    conforms,
                    callable_conformance,
                    conformance_conditions,
                    fields,
                    associated,
                    methods,
                    fieldwise_init: *fieldwise_init,
                    decorators,
                })
            }

            StmtKind::Trait {
                name,
                refines,
                methods,
                comptime_members,
            } => self.check_trait(name, refines, methods, comptime_members),

            StmtKind::Comptime { name, value } => {
                // A comptime `Int` is recorded (for value-parameter use) and bound as
                // `Int`. A richer comptime value (tuple/list/string) the `Int` folder
                // can't evaluate is still an ordinary binding — the elaborator has
                // already consumed it for any `comptime for`/`comptime if`.
                match self.eval_ct(value) {
                    Ok(v) => {
                        self.comptimes.insert(name.clone(), v);
                        self.declare_immutable(name, Ty::IntLiteral)?;
                    }
                    Err(_) => {
                        let ty = self.infer(value)?;
                        let declared = self.inferred_binding_ty(&ty, name)?;
                        self.declare_immutable(name, declared)?;
                    }
                }
                self.record_statement_binding(stmt, name);
                Ok(())
            }

            // `comptime if` / `comptime for` parse and are grammar-documented, but
            // compile-time branch selection / loop unrolling is deferred — flagged
            // here, like the other syntax-first parse-only constructs.
            StmtKind::ComptimeIf { .. } => Err(TypeError::Unsupported("comptime if".to_string())),
            StmtKind::ComptimeFor { .. } => Err(TypeError::Unsupported("comptime for".to_string())),

            StmtKind::If { branches, orelse } => {
                for (_, body) in branches {
                    self.predeclare_implicit_assignments(body)?;
                }
                if let Some(body) = orelse {
                    self.predeclare_implicit_assignments(body)?;
                }
                let before = self.uninitialized.borrow().clone();
                let mut exits = Vec::new();
                // Definite initialization follows only reachable exits when a
                // condition is a compile-time Bool literal. We still check every
                // source branch for type errors, but `if True: x = ...` establishes
                // a function-scoped implicit binding just as an unconditional
                // assignment does. Unknown conditions retain both the taken and
                // fallthrough possibilities.
                let mut fallthrough_reachable = true;
                for (cond, body) in branches {
                    *self.uninitialized.borrow_mut() = before.clone();
                    self.register_named_bindings(cond)?;
                    self.expect_bool(cond, "if condition")?;
                    self.check_scoped_block(body, ret, in_loop)?;
                    let condition = match &cond.kind {
                        ExprKind::Bool(value) => Some(*value),
                        _ => None,
                    };
                    if fallthrough_reachable && condition != Some(false) {
                        exits.push(self.uninitialized.borrow().clone());
                    }
                    if condition == Some(true) {
                        fallthrough_reachable = false;
                    }
                }
                if let Some(body) = orelse {
                    *self.uninitialized.borrow_mut() = before.clone();
                    self.check_scoped_block(body, ret, in_loop)?;
                    if fallthrough_reachable {
                        exits.push(self.uninitialized.borrow().clone());
                    }
                } else if fallthrough_reachable {
                    exits.push(before);
                }
                *self.uninitialized.borrow_mut() =
                    exits.into_iter().flatten().collect::<HashSet<_>>();
                Ok(())
            }

            StmtKind::While { cond, body, orelse } => {
                self.predeclare_implicit_assignments(body)?;
                let before = self.uninitialized.borrow().clone();
                self.register_named_bindings(cond)?;
                self.expect_bool(cond, "while condition")?;
                self.check_scoped_block(body, ret, true)?;
                *self.uninitialized.borrow_mut() = before;
                if let Some(body) = orelse {
                    self.check_scoped_block(body, ret, in_loop)?;
                }
                Ok(())
            }

            // `raise` — its operand must be an `Error` (or a `String`, the
            // shorthand). The raises *effect* (that this must be in a `raises`
            // function or a `try`) is deliberately not analyzed.
            StmtKind::Raise(expr) => {
                self.register_named_bindings(expr)?;
                let ty = self.infer(expr)?;
                let error = if ty == Ty::String { Ty::Error } else { ty };
                self.require_error("'raise'", error)?;
                Ok(())
            }

            // Imports are parsed but not resolved (no module system yet), so
            // they are a checker no-op — imported names are not made available.
            StmtKind::Import { .. } | StmtKind::FromImport { .. } => Ok(()),

            StmtKind::Try {
                body,
                except,
                orelse,
                finalbody,
            } => {
                if except.is_some() {
                    self.handled_raise_depth += 1;
                    self.handled_raise_types.borrow_mut().push(Vec::new());
                }
                let body_result = self.check_scoped_block(body, ret, in_loop);
                let handled_types = except.as_ref().map(|_| {
                    self.handled_raise_types
                        .borrow_mut()
                        .pop()
                        .unwrap_or_default()
                });
                if except.is_some() {
                    self.handled_raise_depth -= 1;
                }
                body_result?;
                if let Some((name, ex_body)) = except {
                    self.push_scope();
                    let error = handled_types
                        .as_ref()
                        .and_then(|types| types.first().cloned())
                        .unwrap_or(Ty::Error);
                    if handled_types
                        .as_ref()
                        .is_some_and(|types| types.iter().any(|candidate| *candidate != error))
                    {
                        self.pop_scope();
                        return Err(TypeError::RaiseTypeMismatch {
                            expected: error.to_string(),
                            found: "multiple error types in one try block".to_string(),
                        });
                    }
                    let result = match name {
                        Some(n) => self.declare(n, error.clone()).and_then(|()| {
                            // The exception target is a real lexical binding. Its
                            // owner is attached to the containing `try` syntax
                            // because the AST stores the target as a name rather
                            // than as an expression node.
                            self.record_statement_binding(stmt, n);
                            self.binding_types
                                .borrow_mut()
                                .insert(stmt.source_span(), error.clone());
                            self.check_block(ex_body, ret, in_loop)
                        }),
                        None => self.check_block(ex_body, ret, in_loop),
                    };
                    self.pop_scope();
                    result?;
                }
                if let Some(body) = orelse {
                    self.check_scoped_block(body, ret, in_loop)?;
                }
                if let Some(body) = finalbody {
                    self.check_scoped_block(body, ret, in_loop)?;
                }
                Ok(())
            }

            StmtKind::For {
                var,
                reference,
                owned,
                iter,
                body,
                orelse,
            } => {
                self.register_named_bindings(iter)?;
                // The loop variable's type comes from the iterable: `Int` for a
                // `range`, the element type for a `List`, or — for a user struct —
                // the element type of its `__iter__()` iterator (`__next__`'s return).
                let iter_ty = self.infer(iter)?;
                let (elem_ty, mut protocol) = self.iteration_protocol(&iter_ty, *owned)?;
                if !*owned
                    && (list_element(&iter_ty).is_some()
                        || set_element(&iter_ty).is_some()
                        || dict_elements(&iter_ty).is_some())
                    && let Ok(mut origin) = self.origin_place(iter)
                {
                    origin
                        .path
                        .push(crate::origin::OriginSeg::Interior("element".to_string()));
                    protocol.borrowed_origin = Some(origin);
                }
                if *owned && !matches!(iter.kind, ExprKind::Transfer(_)) {
                    return Err(TypeError::Unsupported(
                        "owned iteration requires a transferred iterable (`for var item in collection^`)"
                            .to_string(),
                    ));
                }
                if !*owned && matches!(iter.kind, ExprKind::Transfer(_)) {
                    return Err(TypeError::Unsupported(
                        "a transferred iterable requires an explicit `var` loop binding"
                            .to_string(),
                    ));
                }
                if *reference && *owned {
                    return Err(TypeError::Unsupported(
                        "a loop binding cannot be both `ref` and `var`".to_string(),
                    ));
                }
                if !*owned && !*reference && !self.is_copyable(&elem_ty) {
                    return Err(TypeError::NonCopyable {
                        ty: elem_ty.to_string(),
                        context: "immutable iteration; use `for var item in collection^`"
                            .to_string(),
                    });
                }
                if *owned
                    && !self.is_implicitly_deletable(&elem_ty)
                    && block_can_escape_owned_iteration(body, 0)
                {
                    return Err(TypeError::Unsupported(format!(
                        "owned iteration over non-ImplicitlyDeletable '{}' cannot exit early; its residual elements would require explicit destruction",
                        elem_ty
                    )));
                }
                let binding_ty = if *reference {
                    if list_element(&iter_ty).is_none() {
                        return Err(TypeError::Unsupported(
                            "reference iteration currently requires a List place".to_string(),
                        ));
                    }
                    let reference_protocol = self.reference_iteration_protocol(iter)?;
                    let reference = reference_protocol
                        .getitem
                        .reference_result
                        .clone()
                        .ok_or_else(|| {
                            TypeError::InvariantViolation(
                                "checked List reference iteration selected a value-returning __getitem__"
                                    .to_string(),
                            )
                        })?;
                    protocol.reference = Some(Box::new(reference_protocol));
                    Ty::Ref(reference)
                } else {
                    elem_ty
                };
                self.iteration_protocols
                    .borrow_mut()
                    .insert(iter.source_span(), protocol);
                self.push_scope();
                self.binding_types
                    .borrow_mut()
                    .insert(stmt.source_span(), binding_ty.clone());
                if self.is_implicitly_deletable(&binding_ty) {
                    self.explicit_destroy_deletability
                        .borrow_mut()
                        .bindings
                        .insert(stmt.source_span());
                }
                let mutable = *owned
                    || !*reference
                    || matches!(&binding_ty, Ty::Ref(reference) if reference.mutability == crate::origin::Mutability::Mutable);
                let result = match self.declare_with_mutability(var, binding_ty, mutable) {
                    Ok(()) => {
                        self.record_statement_binding(stmt, var);
                        self.check_block(body, ret, true)
                    }
                    Err(e) => Err(e),
                };
                self.pop_scope();
                result?;
                if let Some(body) = orelse {
                    self.check_scoped_block(body, ret, in_loop)?;
                }
                Ok(())
            }

            StmtKind::Break => {
                if in_loop {
                    Ok(())
                } else {
                    Err(TypeError::BreakOutsideLoop)
                }
            }

            StmtKind::Continue => {
                if in_loop {
                    Ok(())
                } else {
                    Err(TypeError::ContinueOutsideLoop)
                }
            }

            StmtKind::Return(expr) => {
                if let Some(expression) = expr {
                    self.register_named_bindings(expression)?;
                }
                let expected = match ret {
                    Some(ty) => ty,
                    None => return Err(TypeError::ReturnOutsideFunction),
                };
                let found = match expr {
                    Some(e) => self.infer_with_expected(e, expected, true)?,
                    None if self.named_result_context.last() == Some(&true) => expected.clone(),
                    None => Ty::None,
                };
                if let Some(expression) = expr
                    && !matches!(expected, Ty::Ref(_))
                    && (self.type_carries_loans(expected) || self.type_carries_loans(&found))
                    && self
                        .aggregate_origins(expression)
                        .iter()
                        .any(|origin| self.aggregate_origin_escapes(origin))
                {
                    // A bare pointer return gets the pointer-specific
                    // diagnostic; an aggregate may mix reference and pointer
                    // loans and keeps the reference wording.
                    if matches!(found, Ty::Pointer { .. }) {
                        return Err(TypeError::PointerEscapesOrigin);
                    }
                    return Err(TypeError::ReturnsReferenceToLocal);
                }
                if let (Some(e), Some(Some((signature, parameter_owners, self_owner)))) =
                    (expr, self.return_ref_contracts.last())
                {
                    let actual = self.reference_actual(e)?.origin;
                    let parameter_origins: Vec<_> = parameter_owners
                        .iter()
                        .map(|owner| {
                            Some(crate::origin::Origin::Place(crate::origin::OriginPlace {
                                root: *owner,
                                path: Vec::new(),
                            }))
                        })
                        .collect();
                    let allowed = substitute_sig_origin_with_self(
                        &signature.origin,
                        &parameter_origins,
                        self_owner.clone().map(crate::origin::Origin::Place),
                    );
                    // Bundled collections intentionally abstract their private
                    // backing storage behind a public owned-interior region:
                    // List bridges its raw pointer slot to `element`, while Dict
                    // bridges its entries List to the replace-on-lookup `value`
                    // generation. Callers inherit the public region, never the
                    // implementation storage origin. Keep this privilege at the
                    // return boundary alongside the private List take/destroy
                    // gate, and restrict it to compiler-shipped source paths.
                    let bundled_collection_interior_bridge =
                        self.self_ty.as_ref().is_some_and(|ty| {
                            list_element(ty).is_some() || dict_elements(ty).is_some()
                        }) && is_bundled_collection_source(e.source.as_deref());
                    if !origin_is_within(&actual, &allowed) && !bundled_collection_interior_bridge {
                        return Err(TypeError::ReturnsReferenceToLocal);
                    }
                }
                // `expected` is the referent type for a reference-returning
                // declaration; the reference contract is retained separately.
                // Returning a place through that checked contract borrows it —
                // it does not copy/consume the referent.
                let returning_reference = self
                    .return_ref_contracts
                    .last()
                    .is_some_and(Option::is_some);
                if let Some(e) = expr
                    && !returning_reference
                {
                    self.check_consuming(e, &found, "return value")?;
                }
                // Returning a callable *value* is an escape; returning a
                // checked reference to callable storage is not. The latter is
                // how a nominal Tuple's indexed accessor exposes a function
                // element while the Tuple owner remains live.
                if !returning_reference
                    && matches!(
                        found,
                        Ty::Func { .. } | Ty::GenericFunc { .. } | Ty::Overload(_)
                    )
                {
                    return Err(TypeError::ClosureEscape);
                }
                let compatible = match expr {
                    Some(expression) => {
                        self.record_implicit_conversion(expression, &found, expected)?
                    }
                    None => self.value_coerces(&found, expected),
                };
                if !compatible {
                    return Err(TypeError::TypeMismatch {
                        expected: expected.to_string(),
                        found: found.to_string(),
                        context: "return".to_string(),
                    });
                }
                Ok(())
            }

            StmtKind::Pass => Ok(()),

            StmtKind::Expr(expr) => {
                self.register_named_bindings(expr)?;
                self.infer(expr)?;
                Ok(())
            }
        }
    }

    /// Resolve a parameter/field list to its types.
    fn param_tys(&self, params: &[crate::ast::FnParam]) -> Result<Vec<Ty>, TypeError> {
        params
            .iter()
            .map(|parameter| {
                if matches!(
                    &parameter.ty,
                    SourceType::Func { type_params, .. } if !type_params.is_empty()
                ) {
                    return Err(TypeError::Unsupported(
                        "a runtime parameter cannot use a parametric function type; declare it as a compile-time callable parameter"
                            .to_string(),
                    ));
                }
                self.ty_from_anno(&parameter.ty)
            })
            .collect()
    }

    fn method_sig(
        &self,
        method: &Method,
        decls: Vec<ParamDecl>,
        all_types: &[Ty],
    ) -> Result<MethodSig, TypeError> {
        let error = self.declared_error(method.raises, method.raises_type.as_ref())?;
        let variadic_idx = method
            .params
            .iter()
            .position(|p| p.kind == crate::ast::ParamKind::Variadic);
        let kw_variadic_idx = method
            .params
            .iter()
            .position(|p| p.kind == crate::ast::ParamKind::KwVariadic);
        let regular: Vec<_> = method
            .params
            .iter()
            .enumerate()
            .filter(|(_, p)| p.kind == crate::ast::ParamKind::Regular)
            .collect();
        let keyword_only =
            effective_keyword_only_index(&method.params, method.keyword_only, variadic_idx);
        let regular_params: Vec<&FnParam> = regular.iter().map(|(_, param)| *param).collect();
        Ok(MethodSig {
            decls,
            availability: method
                .where_clause
                .as_ref()
                .map(|condition| self.compile_generic_constraint(condition))
                .transpose()?
                .into_iter()
                .collect(),
            has_self: method.has_self,
            params: regular
                .iter()
                .map(|(index, _)| all_types[*index].clone())
                .collect(),
            names: regular.iter().map(|(_, p)| p.name.clone()).collect(),
            required: required_mask(
                &regular.iter().map(|(_, p)| *p).collect::<Vec<_>>(),
                keyword_only,
            )?,
            variadic: variadic_idx.map(|index| Box::new(all_types[index].clone())),
            variadic_index: regular_marker_index(&method.params, variadic_idx),
            kw_variadic: kw_variadic_idx.map(|index| Box::new(all_types[index].clone())),
            kw_variadic_index: kw_variadic_idx,
            positional_only: regular_marker_index(&method.params, method.positional_only),
            keyword_only,
            conventions: regular.iter().map(|(_, p)| p.convention).collect(),
            ret: match &method.ret {
                Some(SourceType::Ref { referent, .. }) => self.ty_from_anno(referent)?,
                Some(ret) => self.ty_from_anno(ret)?,
                None => Ty::None,
            },
            raises: error.as_ref().is_some_and(|ty| *ty != Ty::Never),
            error: error.map(Box::new),
            self_convention: method.self_convention,
            ref_params: lower_ref_param_sigs(&self.enclosing_type_params, &regular_params)?,
            ref_return: match &method.ret {
                Some(SourceType::Ref { origin, .. }) => Some(lower_ref_sig(
                    origin.as_ref().ok_or_else(|| {
                        TypeError::Unsupported("reference return requires an origin".to_string())
                    })?,
                    &self.enclosing_type_params,
                    &regular_params,
                )?),
                _ => None,
            },
            implicit: method
                .decorators
                .iter()
                .any(|decorator| decorator.path.len() == 1 && decorator.path[0] == "implicit"),
        })
    }

    /// The name of the first advanced parameter feature used by a signature (a
    /// default value, a `*args`/`**kwargs` variadic, or an argument convention, or
    /// `None` if the signature is supported by this checking path. `/` and bare
    /// `*` markers are modeled by call matching and are not advanced anymore.
    fn advanced_param_feature(
        params: &[crate::ast::FnParam],
        _positional_only: Option<usize>,
        _keyword_only: Option<usize>,
        flag_defaults: bool,
        flag_variadic: bool,
        flag_kw_variadic: bool,
    ) -> Option<&'static str> {
        use crate::ast::ParamKind;
        if flag_defaults && params.iter().any(|p| p.default.is_some()) {
            return Some("default argument values");
        }
        if flag_variadic && params.iter().any(|p| p.kind == ParamKind::Variadic) {
            return Some("variadic '*args' parameters");
        }
        if flag_kw_variadic && params.iter().any(|p| p.kind == ParamKind::KwVariadic) {
            return Some("variadic '**kwargs' parameters");
        }
        None
    }

    /// Classify a `[...]` parameter list into type and value parameters, and
    /// validate them: names must be distinct; a single bound naming a concrete
    /// type is a **value** parameter (must be `Int`); otherwise the bounds must
    /// all name traits (built-in or user), giving a **type** parameter. The
    /// parser guarantees each parameter carries at least one `: bound` (Mojo has
    /// no unconstrained parameters).
    fn classify_params(
        &mut self,
        tps: &[crate::ast::TypeParam],
    ) -> Result<Vec<ParamDecl>, TypeError> {
        let mut decls = Vec::new();
        let mut seen = HashSet::new();
        for tp in tps {
            if !seen.insert(tp.name.clone()) {
                return Err(TypeError::Redeclaration(tp.name.clone()));
            }
            // Origin and OriginSet parameters are semantic-only and erased before
            // runtime generic argument binding. `Origin` participates in ref
            // signatures; `OriginSet` names a capturing callable's environment.
            // Both are inferred from places/callable values rather than occupying
            // a source-visible value-parameter slot.
            if matches!(tp.bounds.as_slice(), [only] if only == "Origin" || only == "OriginSet") {
                continue;
            }
            if let Some(value_type) = &tp.value_type {
                let ty = self.ty_from_anno(value_type)?;
                let default = tp
                    .default
                    .as_ref()
                    .map(|expr| self.compile_dependent_ct_expr(expr))
                    .transpose()?;
                decls.push(ParamDecl::Value {
                    name: tp.name.clone(),
                    ty: Box::new(ty),
                    default,
                    callable_default: None,
                    infer_only: tp.infer_only,
                    variadic: tp.name.starts_with('*'),
                    constraints: tp
                        .constraints
                        .iter()
                        .map(|condition| self.compile_generic_constraint(condition))
                        .collect::<Result<_, _>>()?,
                });
                continue;
            }
            // A lone bound that names a scalar type marks a value parameter.
            if let [only] = tp.bounds.as_slice()
                && let Some(vty) = scalar_type_name(only)
            {
                if !matches!(
                    vty,
                    Ty::Int | Ty::UInt | Ty::Bool | Ty::String | Ty::Float64
                ) {
                    return Err(TypeError::BadValueParamType {
                        name: tp.name.clone(),
                        ty: only.clone(),
                    });
                }
                decls.push(ParamDecl::Value {
                    name: tp.name.clone(),
                    ty: Box::new(vty),
                    default: tp
                        .default
                        .as_ref()
                        .map(|expr| self.compile_dependent_ct_expr(expr))
                        .transpose()?,
                    callable_default: None,
                    infer_only: tp.infer_only,
                    variadic: tp.name.starts_with('*'),
                    constraints: tp
                        .constraints
                        .iter()
                        .map(|condition| self.compile_generic_constraint(condition))
                        .collect::<Result<_, _>>()?,
                });
                continue;
            }
            let trait_bounds = tp
                .bounds
                .iter()
                .filter(|bound| bound.as_str() != "<function type>")
                .cloned()
                .collect::<Vec<_>>();
            for bound in &trait_bounds {
                self.check_trait_name(bound)?;
            }
            decls.push(ParamDecl::Type {
                name: tp.name.clone(),
                bounds: trait_bounds,
                callable_bound: None,
                // A callable RHS is initially represented by this temporary
                // type declaration, but its default is a function value rather
                // than a type. Compile it only after the callable contract has
                // been lowered below.
                default: if tp.callable_bound.is_some() {
                    None
                } else {
                    tp.default
                        .as_ref()
                        .map(|value| self.type_default_from_expr(value))
                        .transpose()?
                        .map(Box::new)
                },
                infer_only: tp.infer_only,
                variadic: tp.name.starts_with('*'),
                constraints: tp
                    .constraints
                    .iter()
                    .map(|condition| self.compile_generic_constraint(condition))
                    .collect::<Result<_, _>>()?,
            });
        }

        // Callable constraints may depend on any type parameter in this list
        // (`F: def(T) -> T`), so lower them only after the complete preliminary
        // parameter scope exists. An explicit `thin`/`capturing[...]` spelling is
        // instead a compile-time callable-value parameter in current Mojo.
        self.tparams.push(type_scope(&decls));
        let result = (|| {
            for source in tps {
                let Some(callable) = &source.callable_bound else {
                    continue;
                };
                let SourceType::Func {
                    thin, capturing, ..
                } = callable
                else {
                    return Err(TypeError::InvariantViolation(
                        "retained callable parameter bound is not a function type".to_string(),
                    ));
                };
                let checked = self.lower_anonymous_callable_type(callable, tps)?;
                let Some(index) = decls.iter().position(|decl| decl.name() == source.name) else {
                    return Err(TypeError::InvariantViolation(format!(
                        "callable constraint parameter '{}' was not classified",
                        source.name
                    )));
                };
                let ParamDecl::Type {
                    constraints,
                    infer_only,
                    variadic,
                    ..
                } = &decls[index]
                else {
                    return Err(TypeError::InvariantViolation(
                        "callable constraint was classified as a value parameter".to_string(),
                    ));
                };
                let constraints = constraints.clone();
                let infer_only = *infer_only;
                let variadic = *variadic;
                if *thin || capturing.is_some() {
                    let callable_default = source
                        .default
                        .as_ref()
                        .map(|default| {
                            self.compile_callable_default(default, &checked, &decls[..index])
                        })
                        .transpose()?;
                    decls[index] = ParamDecl::Value {
                        name: source.name.clone(),
                        ty: Box::new(checked),
                        default: None,
                        callable_default,
                        infer_only,
                        variadic,
                        constraints,
                    };
                } else {
                    let ParamDecl::Type { callable_bound, .. } = &mut decls[index] else {
                        unreachable!("callable type parameter changed classification")
                    };
                    *callable_bound = Some(Box::new(checked));
                }
            }
            Ok(decls)
        })();
        self.tparams.pop();
        result
    }

    /// Lower a callable contract with its own `def[...]` binders. The
    /// anonymous scope is nested inside the surrounding declaration's scope,
    /// so alpha-renamed binders retain their own identity while a signature may
    /// still depend on an outer type. Origin declarations stay in the source
    /// context used by reference/capture lowering, but are erased from the
    /// ordinary `GenericFunc::decls` just like named defs.
    fn lower_anonymous_callable_type(
        &mut self,
        callable: &SourceType,
        outer_type_params: &[crate::ast::TypeParam],
    ) -> Result<Ty, TypeError> {
        let SourceType::Func { type_params, .. } = callable else {
            return Err(TypeError::InvariantViolation(
                "anonymous callable lowering received a non-function type".to_string(),
            ));
        };
        let decls = self.classify_params(type_params)?;
        self.tparams.push(type_scope(&decls));

        let mut contextual_callable = callable.clone();
        let SourceType::Func {
            type_params: callable_context,
            ..
        } = &mut contextual_callable
        else {
            unreachable!("callable source was matched above")
        };
        // Own declarations remain first so any own Origin/OriginSet indexes
        // have the same source-relative positions they do on a named generic
        // def. The appended outer declarations are only a lookup context.
        callable_context.extend_from_slice(outer_type_params);
        let checked = self.ty_from_anno(&contextual_callable);
        self.tparams.pop();
        let checked = checked?;

        if decls.is_empty() {
            return Ok(checked);
        }
        let Ty::Func {
            environment,
            params,
            names,
            ret,
            required,
            variadic,
            kw_variadic,
            positional_only,
            keyword_only,
            raises,
            error,
            conventions,
            ref_params,
            ref_return,
        } = checked
        else {
            return Err(TypeError::InvariantViolation(
                "anonymous callable signature did not lower to a function type".to_string(),
            ));
        };
        Ok(Ty::GenericFunc {
            environment,
            decls,
            params,
            names,
            ret,
            required,
            variadic,
            kw_variadic,
            positional_only,
            keyword_only,
            raises,
            error,
            conventions,
            ref_params,
            ref_return,
        })
    }

    fn type_default_from_expr(&self, value: &Expr) -> Result<Ty, TypeError> {
        match &value.kind {
            ExprKind::Identifier(name) => {
                if let Some(ty) = scalar_type_name(name) {
                    Ok(ty)
                } else {
                    self.ty_from_anno(&SourceType::Named(name.clone(), Vec::new()))
                }
            }
            ExprKind::TypeApply { name, args } => {
                self.ty_from_anno(&SourceType::Named(name.clone(), args.clone()))
            }
            ExprKind::TypeValue(ty) => self.ty_from_anno(ty),
            _ => Err(TypeError::TypeMismatch {
                expected: "a type".to_string(),
                found: "a value".to_string(),
                context: "type parameter default".to_string(),
            }),
        }
    }

    fn compile_callable_default(
        &self,
        expression: &Expr,
        expected: &Ty,
        earlier: &[ParamDecl],
    ) -> Result<CallableDefault, TypeError> {
        if let ExprKind::IfExpr {
            cond,
            then_branch,
            else_branch,
        } = &expression.kind
        {
            let condition = self.compile_dependent_ct_expr(cond)?;
            if let ExprKind::Identifier(name) = &cond.kind {
                let is_bool_parameter = earlier.iter().any(|declaration| {
                    matches!(declaration,
                        ParamDecl::Value { name: parameter, ty, .. }
                            if parameter == name && ty.as_ref() == &Ty::Bool)
                });
                if !is_bool_parameter && !self.comptimes.contains_key(name) {
                    return Err(TypeError::TypeMismatch {
                        expected: Ty::Bool.to_string(),
                        found: format!("compile-time parameter '{name}'"),
                        context: "callable default condition".to_string(),
                    });
                }
            }
            return Ok(CallableDefault::If {
                condition,
                then_value: Box::new(self.compile_callable_default(
                    then_branch,
                    expected,
                    earlier,
                )?),
                else_value: Box::new(self.compile_callable_default(
                    else_branch,
                    expected,
                    earlier,
                )?),
            });
        }

        if let ExprKind::Identifier(name) = &expression.kind
            && let Some(ParamDecl::Value { ty, .. }) = earlier
                .iter()
                .find(|declaration| declaration.name() == name)
        {
            if !self.value_coerces(ty, expected) {
                return Err(TypeError::TypeMismatch {
                    expected: expected.to_string(),
                    found: ty.to_string(),
                    context: format!("default for callable parameter '{name}'"),
                });
            }
            return Ok(CallableDefault::Parameter(name.clone()));
        }

        let (name, arguments) = match &expression.kind {
            ExprKind::Identifier(name) => (name.as_str(), &[][..]),
            ExprKind::TypeApply { name, args } => (name.as_str(), args.as_slice()),
            _ => {
                return Err(TypeError::Unsupported(
                    "a callable default must be a function, an earlier callable parameter, or a conditional of those values"
                        .to_string(),
                ));
            }
        };
        let actual = self
            .infer_specialized_callable_value(
                expression.source_span(),
                name,
                arguments,
                Some(expected),
                true,
            )?
            .ok_or_else(|| TypeError::NotCallable {
                name: name.to_string(),
                ty: self
                    .lookup(name)
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "undefined".to_string()),
            })?;
        if !self.value_coerces(&actual, expected) {
            return Err(TypeError::TypeMismatch {
                expected: expected.to_string(),
                found: actual.to_string(),
                context: "callable parameter default".to_string(),
            });
        }
        let symbol = self
            .overload_targets
            .borrow()
            .get(&expression.source_span())
            .cloned()
            .unwrap_or_else(|| name.to_string());
        Ok(CallableDefault::Symbol(symbol))
    }

    fn compile_dependent_ct_expr(&self, expr: &Expr) -> Result<CtExpr, TypeError> {
        let pair = |left: &Expr, right: &Expr| {
            Ok((
                Box::new(self.compile_dependent_ct_expr(left)?),
                Box::new(self.compile_dependent_ct_expr(right)?),
            ))
        };
        Ok(match &expr.kind {
            ExprKind::Int(value) => CtExpr::Value(CtValue::IntLiteral(value.clone())),
            ExprKind::Float(value) => CtExpr::Value(CtValue::FloatLiteral(value.clone())),
            ExprKind::Bool(value) => CtExpr::Value(CtValue::Bool(*value)),
            ExprKind::Str(value) => CtExpr::Value(CtValue::Str(value.clone())),
            ExprKind::Identifier(name) => {
                if let Some(value) = self.comptimes.get(name) {
                    CtExpr::Value(CtValue::IntLiteral(value.clone()))
                } else {
                    CtExpr::Param(name.clone())
                }
            }
            ExprKind::TupleLit(values) => CtExpr::Value(CtValue::Tuple(
                values
                    .iter()
                    .map(|value| self.eval_associated_ct(value, &HashMap::new()))
                    .collect::<Result<_, _>>()?,
            )),
            ExprKind::ListLit(values) => CtExpr::Value(CtValue::List(
                values
                    .iter()
                    .map(|value| self.eval_associated_ct(value, &HashMap::new()))
                    .collect::<Result<_, _>>()?,
            )),
            ExprKind::Prefix(PrefixOp::Neg, value) => {
                CtExpr::Neg(Box::new(self.compile_dependent_ct_expr(value)?))
            }
            ExprKind::Infix(op, left, right) => {
                let (left, right) = pair(left, right)?;
                match op {
                    InfixOp::Add => CtExpr::Add(left, right),
                    InfixOp::Sub => CtExpr::Sub(left, right),
                    InfixOp::Mul => CtExpr::Mul(left, right),
                    InfixOp::FloorDiv => CtExpr::FloorDiv(left, right),
                    InfixOp::Mod => CtExpr::Mod(left, right),
                    InfixOp::Pow => CtExpr::Pow(left, right),
                    _ => {
                        return Err(TypeError::Unsupported(
                            "unsupported dependent parameter expression".to_string(),
                        ));
                    }
                }
            }
            _ => {
                return Err(TypeError::Unsupported(
                    "unsupported dependent parameter expression".to_string(),
                ));
            }
        })
    }

    fn compile_generic_constraint(&self, expr: &Expr) -> Result<GenericConstraint, TypeError> {
        let binary = |left: &Expr, right: &Expr| {
            Ok((
                self.constraint_operand(left)?,
                self.constraint_operand(right)?,
            ))
        };
        Ok(match &expr.kind {
            ExprKind::Bool(value) => GenericConstraint::Bool(*value),
            ExprKind::Prefix(PrefixOp::Not, value) => {
                GenericConstraint::Not(Box::new(self.compile_generic_constraint(value)?))
            }
            ExprKind::Infix(InfixOp::And, left, right) => GenericConstraint::And(
                Box::new(self.compile_generic_constraint(left)?),
                Box::new(self.compile_generic_constraint(right)?),
            ),
            ExprKind::Infix(InfixOp::Or, left, right) => GenericConstraint::Or(
                Box::new(self.compile_generic_constraint(left)?),
                Box::new(self.compile_generic_constraint(right)?),
            ),
            ExprKind::Infix(op, left, right) => {
                let (left, right) = binary(left, right)?;
                match op {
                    InfixOp::Eq => GenericConstraint::Eq(left, right),
                    InfixOp::Ne => GenericConstraint::Ne(left, right),
                    InfixOp::Lt => GenericConstraint::Lt(left, right),
                    InfixOp::Le => GenericConstraint::Le(left, right),
                    InfixOp::Gt => GenericConstraint::Gt(left, right),
                    InfixOp::Ge => GenericConstraint::Ge(left, right),
                    _ => {
                        return Err(TypeError::Unsupported(
                            "unsupported generic where proposition".to_string(),
                        ));
                    }
                }
            }
            ExprKind::Call {
                name, args, kwargs, ..
            } if name == "conforms_to" && kwargs.is_empty() && args.len() == 2 => {
                let (param, pack) = match &args[0].kind {
                    ExprKind::Identifier(param) => (param.clone(), false),
                    ExprKind::Member { object, field } if matches!(&object.kind, ExprKind::Identifier(name) if name == "Self") => {
                        (field.clone(), false)
                    }
                    ExprKind::Member { object, field }
                        if field == "values" && matches!(&object.kind, ExprKind::Identifier(_)) =>
                    {
                        let ExprKind::Identifier(param) = &object.kind else {
                            unreachable!()
                        };
                        (param.clone(), true)
                    }
                    _ => {
                        return Err(TypeError::Unsupported(
                            "conforms_to requires a parameter name".to_string(),
                        ));
                    }
                };
                let ExprKind::Identifier(trait_name) = &args[1].kind else {
                    return Err(TypeError::Unsupported(
                        "conforms_to requires a trait name".to_string(),
                    ));
                };
                self.check_trait_name(trait_name)?;
                if pack {
                    GenericConstraint::ConformsPack {
                        param,
                        trait_name: trait_name.clone(),
                    }
                } else {
                    GenericConstraint::Conforms {
                        param,
                        trait_name: trait_name.clone(),
                    }
                }
            }
            _ => {
                return Err(TypeError::Unsupported(
                    "unsupported generic where proposition".to_string(),
                ));
            }
        })
    }

    fn constraint_operand(&self, expr: &Expr) -> Result<ConstraintOperand, TypeError> {
        Ok(match &expr.kind {
            ExprKind::Identifier(name) => scalar_type_name(name)
                .map(ConstraintOperand::Type)
                .unwrap_or_else(|| ConstraintOperand::Param(name.clone())),
            ExprKind::Member { object, field } if matches!(&object.kind, ExprKind::Identifier(name) if name == "Self") => {
                ConstraintOperand::Param(field.clone())
            }
            ExprKind::Int(value) => ConstraintOperand::Value(CtValue::IntLiteral(value.clone())),
            ExprKind::Bool(value) => ConstraintOperand::Value(CtValue::Bool(*value)),
            ExprKind::Str(value) => ConstraintOperand::Value(CtValue::Str(value.clone())),
            ExprKind::TypeValue(ty) => ConstraintOperand::Type(self.ty_from_anno(ty)?),
            ExprKind::TypeApply { name, args } => ConstraintOperand::Type(
                self.ty_from_anno(&SourceType::Named(name.clone(), args.clone()))?,
            ),
            _ => {
                return Err(TypeError::Unsupported(
                    "unsupported generic constraint operand".to_string(),
                ));
            }
        })
    }

    fn validate_origin_signature(
        &self,
        type_params: &[crate::ast::TypeParam],
        params: &[crate::ast::FnParam],
        self_origin: Option<&crate::ast::OriginSpec>,
    ) -> Result<(), TypeError> {
        let origin_params: HashSet<&str> = type_params
            .iter()
            .filter(|param| param.bounds.as_slice() == ["Origin"])
            .map(|param| param.name.as_str())
            .collect();
        let value_params: HashSet<&str> = params.iter().map(|param| param.name.as_str()).collect();
        let bool_params: HashSet<&str> = type_params
            .iter()
            .filter(|param| param.bounds.as_slice() == ["Bool"])
            .map(|param| param.name.as_str())
            .collect();

        for origin in type_params
            .iter()
            .filter(|param| param.bounds.as_slice() == ["Origin"])
        {
            if let Some(expr) = &origin.origin_mutability
                && !matches!(expr.kind, ExprKind::Bool(_))
                && !matches!(&expr.kind, ExprKind::Identifier(name) if bool_params.contains(name.as_str()))
            {
                return Err(TypeError::Unsupported(format!(
                    "origin mutability for '{}' must be Bool or a Bool parameter",
                    origin.name
                )));
            }
        }

        let validate = |spec: &crate::ast::OriginSpec| {
            for expr in spec {
                validate_origin_expr(expr, &origin_params, &value_params)?;
            }
            Ok::<(), TypeError>(())
        };
        if let Some(spec) = self_origin {
            validate(spec)?;
        }
        for param in params {
            if param.convention != Some(ArgConvention::Ref) && param.origin.is_some() {
                return Err(TypeError::Unsupported(format!(
                    "origin clause on non-ref parameter '{}'",
                    param.name
                )));
            }
            if let Some(spec) = &param.origin {
                validate(spec)?;
            }
        }
        Ok(())
    }

    /// A trait name is valid if it is a built-in or a user trait defined so far.
    fn check_trait_name(&self, name: &str) -> Result<(), TypeError> {
        if BUILTIN_TRAITS.contains(&name) || self.traits.contains_key(name) {
            Ok(())
        } else {
            Err(TypeError::UnknownTrait(name.to_string()))
        }
    }

    /// Register and check a `trait`: its method requirements (each typed with
    /// `Self` as the abstract conforming type, `Ty::SelfType`).
    fn check_trait(
        &mut self,
        name: &str,
        refines: &[String],
        methods: &[crate::ast::TraitMethod],
        comptime_members: &[TraitComptime],
    ) -> Result<(), TypeError> {
        if self.traits.contains_key(name) || self.structs.contains_key(name) {
            return Err(TypeError::Redeclaration(name.to_string()));
        }
        for parent in refines {
            self.check_trait_name(parent)?;
            if BUILTIN_TRAITS.contains(&parent.as_str()) {
                return Err(TypeError::Unsupported(format!(
                    "user trait '{name}' cannot refine builtin trait '{parent}' yet"
                )));
            }
        }
        let mut ct_members = HashMap::new();
        for parent in refines {
            let inherited = self.traits.get(parent).ok_or_else(|| {
                TypeError::InvariantViolation(format!("trait '{parent}' was not registered"))
            })?;
            for (member, requirement) in &inherited.comptime_members {
                if let Some(existing) = ct_members.get_mut(member) {
                    merge_associated_requirement(existing, requirement, member)?;
                } else {
                    ct_members.insert(member.clone(), requirement.clone());
                }
            }
        }
        for member in comptime_members {
            let requirement = self.ct_member_req_from_anno(&member.ty)?;
            if let Some(existing) = ct_members.get_mut(&member.name) {
                merge_associated_requirement(existing, &requirement, &member.name)?;
            } else {
                ct_members.insert(member.name.clone(), requirement);
            }
        }
        // Requirement signatures resolve `Self` to the abstract `Ty::SelfType`.
        let saved_self_ty = self.self_ty.replace(Ty::SelfType);
        let saved_self_decls = std::mem::take(&mut self.self_decls);
        self.trait_self_comptime.push(ct_members.clone());
        let result = (|| {
            let mut sigs: HashMap<String, Vec<MethodSig>> = HashMap::new();
            for parent in refines {
                let inherited = &self.traits[parent].methods;
                for (method, parent_sigs) in inherited {
                    let overloads = sigs.entry(method.clone()).or_default();
                    for sig in parent_sigs {
                        if !overloads.contains(sig) {
                            overloads.push(sig.clone());
                        }
                    }
                }
            }
            for m in methods {
                self.validate_origin_signature(&[], &m.params, m.self_origin.as_ref())?;
                if ct_members.contains_key(&m.name) {
                    return Err(TypeError::Redeclaration(m.name.clone()));
                }
                if let Some(feature) = Self::advanced_param_feature(
                    &m.params,
                    m.positional_only,
                    m.keyword_only,
                    true,
                    true,
                    false,
                ) {
                    return Err(TypeError::Unsupported(feature.to_string()));
                }
                if m.positional_only.is_some() || m.keyword_only.is_some() {
                    return Err(TypeError::Unsupported(
                        "positional-only/keyword-only markers on trait methods".to_string(),
                    ));
                }
                let mut decls = self.classify_params(&m.type_params)?;
                if let Some(condition) = &m.where_clause {
                    let constraint = self.compile_generic_constraint(condition)?;
                    let Some(last) = decls.last_mut() else {
                        return Err(TypeError::Unsupported(
                            "a where clause requires compile-time parameters".to_string(),
                        ));
                    };
                    match last {
                        ParamDecl::Type { constraints, .. }
                        | ParamDecl::Value { constraints, .. } => constraints.push(constraint),
                    }
                }
                self.tparams.push(type_scope(&decls));
                let signature = (|| {
                    Ok::<_, TypeError>((
                        self.param_tys(&m.params)?,
                        match &m.ret {
                            Some(SourceType::Ref { referent, .. }) => {
                                self.ty_from_anno(referent)?
                            }
                            Some(t) => self.ty_from_anno(t)?,
                            None => Ty::None,
                        },
                        self.declared_error(m.raises, m.raises_type.as_ref())?,
                    ))
                })();
                self.tparams.pop();
                let (all_types, ret, error) = signature?;
                let kw_variadic_idx = m
                    .params
                    .iter()
                    .position(|param| param.kind == crate::ast::ParamKind::KwVariadic);
                if let Some(index) = kw_variadic_idx {
                    self.kwargs_collector_ty(
                        all_types[index].clone(),
                        &format!("trait method '{}.{}' keyword collector", name, m.name),
                    )?;
                }
                let regular: Vec<_> = m
                    .params
                    .iter()
                    .enumerate()
                    .filter(|(_, param)| param.kind == crate::ast::ParamKind::Regular)
                    .collect();
                let regular_params: Vec<_> = regular.iter().map(|(_, param)| *param).collect();
                let sig = MethodSig {
                    decls,
                    availability: Vec::new(),
                    has_self: true,
                    params: regular
                        .iter()
                        .map(|(index, _)| all_types[*index].clone())
                        .collect(),
                    names: regular
                        .iter()
                        .map(|(_, param)| param.name.clone())
                        .collect(),
                    required: vec![true; regular.len()],
                    variadic: None,
                    variadic_index: None,
                    kw_variadic: kw_variadic_idx.map(|index| Box::new(all_types[index].clone())),
                    kw_variadic_index: kw_variadic_idx,
                    positional_only: m.positional_only,
                    keyword_only: m.keyword_only,
                    conventions: regular.iter().map(|(_, param)| param.convention).collect(),
                    ret,
                    raises: error.as_ref().is_some_and(|ty| *ty != Ty::Never),
                    error: error.map(Box::new),
                    self_convention: m.self_convention,
                    ref_params: lower_ref_param_sigs(&m.type_params, &regular_params)?,
                    ref_return: None,
                    implicit: false,
                };
                let overloads = sigs.entry(m.name.clone()).or_default();
                if overloads.iter().any(|existing| {
                    same_method_shape(existing, &sig)
                        && (m.name != "__iter__" || existing.self_convention == sig.self_convention)
                }) {
                    return Err(TypeError::Redeclaration(m.name.clone()));
                }
                overloads.push(sig);
            }
            Ok(sigs)
        })();
        self.trait_self_comptime.pop();
        self.self_ty = saved_self_ty;
        self.self_decls = saved_self_decls;
        let methods = result?;
        self.traits.insert(
            name.to_string(),
            TraitInfo {
                refines: refines.to_vec(),
                methods,
                comptime_members: ct_members,
            },
        );
        Ok(())
    }

    /// Register a struct and check its method bodies. A generic struct's type
    /// parameters are validated and kept in scope (as `Self.T`) for its fields
    /// and methods; field/method types referring to them become `Ty::Param`.
    /// Declared trait conformances are verified once the members are known.
    fn check_struct(&mut self, declaration: &StructDeclaration<'_>) -> Result<(), TypeError> {
        let name = declaration.name;
        let type_params = declaration.type_params;
        let conforms = declaration.conforms;
        if self.structs.contains_key(name) || self.traits.contains_key(name) {
            return Err(TypeError::Redeclaration(name.to_string()));
        }
        let decls = self.classify_params(type_params)?;
        self.generic_parameters.borrow_mut().insert(
            crate::checked::GenericSite::Struct {
                module: declaration.module.clone(),
                declaration: name.to_string(),
            },
            decls.clone(),
        );
        // A variadic struct template is compiled by compile-time specialization
        // (each instantiation is a concrete struct); the unspecialized template
        // has pack-dependent members and cannot be checked erased.
        if decls.iter().any(|decl| {
            matches!(
                decl,
                ParamDecl::Type { variadic: true, .. } | ParamDecl::Value { variadic: true, .. }
            )
        }) {
            return Err(TypeError::Unsupported(format!(
                "variadic struct '{name}' is compiled by compile-time specialization; instantiate it with explicit compile-time arguments (e.g. `{name}[Int, Bool](...)`) instead of checking the template"
            )));
        }
        for tr in conforms {
            self.check_trait_name(tr)?;
        }

        // A generated public-Tuple implementation has erased its source pack
        // declaration, but its materialized `element_types` member retains the
        // concrete pack. Recover that checked identity before resolving `Self`
        // in fields and method signatures. The reserved specialization symbol
        // verifies provenance without decoding a mangled name.
        let generated_tuple = name.starts_with("Tuple$") || name.contains("$Tuple$");
        let saved_forward_types = std::mem::replace(
            &mut self.allow_generated_tuple_forward_types,
            generated_tuple,
        );
        let saved_type_params =
            std::mem::replace(&mut self.enclosing_type_params, type_params.to_vec());
        let fixed_arguments = match self.generated_tuple_arguments(name, declaration.associated) {
            Ok(arguments) => arguments,
            Err(error) => {
                self.enclosing_type_params = saved_type_params;
                self.allow_generated_tuple_forward_types = saved_forward_types;
                return Err(error);
            }
        };

        // The struct's parameters are in scope as `Self.T` / `Self.n`, and bare
        // `Self` is the struct type, while checking its members. Type parameters
        // appear as `Ty::Param`, value parameters as symbolic `CtValue::Param`.
        let self_ty = Ty::Struct(
            name.to_string(),
            fixed_arguments
                .clone()
                .unwrap_or_else(|| decls.iter().map(param_as_arg).collect()),
        );
        let saved_self_decls = std::mem::replace(&mut self.self_decls, decls.clone());
        let saved_self_ty = self.self_ty.replace(self_ty.clone());
        let result = self.check_struct_members(declaration, decls, fixed_arguments, &self_ty);
        self.self_decls = saved_self_decls;
        self.enclosing_type_params = saved_type_params;
        self.self_ty = saved_self_ty;
        self.allow_generated_tuple_forward_types = saved_forward_types;
        result
    }

    fn check_struct_members(
        &mut self,
        declaration: &StructDeclaration<'_>,
        decls: Vec<ParamDecl>,
        fixed_arguments: Option<Vec<TyArg>>,
        self_ty: &Ty,
    ) -> Result<(), TypeError> {
        let name = declaration.name;
        let conforms = declaration.conforms;
        let fields = declaration.fields;
        let associated = declaration.associated;
        let methods = declaration.methods;
        let fieldwise_init = declaration.fieldwise_init;
        let explicit_destroy_message = declaration
            .decorators
            .iter()
            .find(|decorator| decorator.path.len() == 1 && decorator.path[0] == "explicit_destroy")
            .map(|decorator| {
                if !decorator.kwargs.is_empty() || decorator.args.len() != 1 {
                    return Err(TypeError::Unsupported(
                        "@explicit_destroy requires exactly one positional string message"
                            .to_string(),
                    ));
                }
                match decorator.args.first().map(|arg| &arg.kind) {
                    Some(ExprKind::Str(message)) => Ok(message.clone()),
                    Some(_) => Err(TypeError::Unsupported(
                        "@explicit_destroy message must be a string literal".to_string(),
                    )),
                    None => unreachable!("decorator arity was checked above"),
                }
            })
            .transpose()?;
        let explicit_destructors = methods
            .iter()
            .filter(|method| {
                method.name != "__del__" && method.self_convention == Some(ArgConvention::Deinit)
            })
            .map(|method| (method.name.clone(), method.raises))
            .collect::<HashMap<_, _>>();
        // Field types are resolved against structs defined *so far* (so a struct
        // can't contain itself); duplicate field names are a redeclaration.
        let mut field_tys: Vec<(String, Ty)> = Vec::new();
        for (field_index, f) in fields.iter().enumerate() {
            if field_tys.iter().any(|(n, _)| n == &f.name) {
                return Err(TypeError::Redeclaration(f.name.clone()));
            }
            let ty = self.ty_from_anno(&f.ty)?;
            if Self::type_contains_unsafe_any_pointer(&ty) {
                return Err(TypeError::Unsupported(format!(
                    "field '{}' cannot hide a MutUnsafeAnyOrigin or ImmutUnsafeAnyOrigin pointer",
                    f.name
                )));
            }
            self.declaration_types.borrow_mut().insert(
                crate::checked::AnnotationSite::StructField {
                    module: declaration.module.clone(),
                    declaration: name.to_string(),
                    field: field_index,
                },
                ty.clone(),
            );
            field_tys.push((f.name.clone(), ty));
        }
        let associated_values = self.check_struct_associated(associated)?;
        let callable_conformance = declaration
            .callable_conformance
            .as_ref()
            .map(|annotation| self.ty_from_anno(annotation))
            .transpose()?;
        if callable_conformance
            .as_ref()
            .is_some_and(|ty| !matches!(ty, Ty::Func { .. }))
        {
            return Err(TypeError::Unsupported(
                "callable conformance must be a def(...) function type".to_string(),
            ));
        }
        // Register the (method-less) struct first, so methods may reference the
        // struct's own type (even parameterized, `Pair[Self.T]`) in signatures.
        self.structs.insert(
            name.to_string(),
            StructInfo {
                decls,
                fixed_arguments,
                conforms: conforms.to_vec(),
                callable_conformance,
                callable_target: None,
                conformance_conditions: declaration
                    .conformance_conditions
                    .iter()
                    .cloned()
                    .collect(),
                fields: field_tys,
                associated: associated_values,
                methods: HashMap::new(),
                fieldwise_init,
                explicit_destroy_message,
                explicit_destructors,
            },
        );
        // Method signatures.
        for (method_index, m) in methods.iter().enumerate() {
            let method_name = lifecycle_method_name(m);
            let method_decls = self.classify_params(&m.type_params)?;
            self.generic_parameters.borrow_mut().insert(
                crate::checked::GenericSite::Method {
                    module: declaration.module.clone(),
                    declaration: name.to_string(),
                    method: method_index,
                },
                method_decls.clone(),
            );
            self.tparams.push(type_scope(&method_decls));
            let saved_method_type_params = self.enclosing_type_params.clone();
            self.enclosing_type_params.extend(m.type_params.clone());
            let signature = (|| {
                let all_types = self.param_tys(&m.params)?;
                let sig = self.method_sig(m, method_decls, &all_types)?;
                Ok::<_, TypeError>((all_types, sig))
            })();
            self.enclosing_type_params = saved_method_type_params;
            self.tparams.pop();
            let (all_types, sig) = signature?;
            for (param, ty) in all_types.iter().enumerate() {
                self.declaration_types.borrow_mut().insert(
                    crate::checked::AnnotationSite::MethodParam {
                        module: declaration.module.clone(),
                        declaration: name.to_string(),
                        method: method_index,
                        param,
                    },
                    ty.clone(),
                );
            }
            self.declaration_types.borrow_mut().insert(
                crate::checked::AnnotationSite::MethodReturn {
                    module: declaration.module.clone(),
                    declaration: name.to_string(),
                    method: method_index,
                },
                sig.ret.clone(),
            );
            self.declaration_effects.borrow_mut().insert(
                crate::checked::AnnotationSite::MethodReturn {
                    module: declaration.module.clone(),
                    declaration: name.to_string(),
                    method: method_index,
                },
                crate::checked::DeclarationEffect {
                    raises: sig.raises,
                    error: sig.raises.then(|| sig.error.as_deref().cloned()).flatten(),
                    returns_reference: sig.ref_return.is_some(),
                },
            );
            let info = self.structs.get_mut(name).ok_or_else(|| {
                TypeError::InvariantViolation(format!("struct '{name}' was not registered"))
            })?;
            let overloads = info.methods.entry(method_name.to_string()).or_default();
            if overloads.iter().any(|existing| {
                same_method_shape(existing, &sig)
                    && (method_name != "__iter__"
                        || existing.self_convention == sig.self_convention)
            }) {
                return Err(TypeError::Redeclaration(method_name.to_string()));
            }
            overloads.push(sig);
        }
        // `@fieldwise_init` and a hand-written `__init__` both define a constructor;
        // having both is a conflict (the decorator *generates* `__init__`).
        if fieldwise_init
            && self
                .structs
                .get(name)
                .is_some_and(|i| i.methods.contains_key("__init__"))
        {
            return Err(TypeError::ConflictingConstructor(name.to_string()));
        }
        // Verify each declared conformance now that the method signatures exist.
        for tr in conforms {
            self.verify_conformance(name, tr, self_ty)?;
        }
        if let Some(expected) = self
            .structs
            .get(name)
            .and_then(|info| info.callable_conformance.clone())
        {
            let Some(call_methods) = self
                .structs
                .get(name)
                .and_then(|info| info.methods.get("__call__"))
            else {
                return Err(TypeError::MissingTraitMethod {
                    struct_name: name.to_string(),
                    trait_name: expected.to_string(),
                    method: "__call__".to_string(),
                });
            };
            let matching = call_methods
                .iter()
                .filter(|method| {
                    let actual = method_callable_ty(method);
                    coerces(&actual, &expected) && coerces(&expected, &actual)
                })
                .collect::<Vec<_>>();
            let [selected] = matching.as_slice() else {
                return Err(TypeError::TraitMethodMismatch {
                    struct_name: name.to_string(),
                    trait_name: expected.to_string(),
                    method: "__call__".to_string(),
                });
            };
            let target = if call_methods.len() == 1 {
                format!("{name}.__call__")
            } else {
                method_lowered_name(name, "__call__", selected)
            };
            self.structs
                .get_mut(name)
                .expect("callable struct remains registered")
                .callable_target = Some(target);
        }
        // Method bodies, each with `self` bound to this struct at its own type
        // parameters (so `self.field : Ty::Param` inside a generic struct).
        for (method_index, m) in methods.iter().enumerate() {
            self.check_method(self_ty, m, declaration.module.clone(), name, method_index)?;
        }
        Ok(())
    }

    /// Verify that struct `name` (whose `Self` type is `self_ty`) implements
    /// every method required by trait `tr`, with a matching signature. A few
    /// built-in marker traits have real lifecycle semantics; other built-ins
    /// remain shallow recognized bounds until their corresponding feature grows.
    fn verify_conformance(&self, name: &str, tr: &str, self_ty: &Ty) -> Result<(), TypeError> {
        if BUILTIN_TRAITS.contains(&tr) {
            return self.verify_builtin_conformance(name, tr, self_ty);
        }
        let trait_info = match self.traits.get(tr) {
            Some(info) => info,
            None => return Ok(()),
        };
        let struct_info = self.structs.get(name).ok_or_else(|| {
            TypeError::InvariantViolation(format!(
                "struct '{name}' was not registered before conformance checking"
            ))
        })?;
        let conformance_assumption = struct_info
            .conformance_conditions
            .get(tr)
            .map(|condition| self.compile_generic_constraint(condition))
            .transpose()?;
        for (mname, req_sigs) in &trait_info.methods {
            let got_sigs =
                struct_info
                    .methods
                    .get(mname)
                    .ok_or_else(|| TypeError::MissingTraitMethod {
                        struct_name: name.to_string(),
                        trait_name: tr.to_string(),
                        method: mname.clone(),
                    })?;
            // The requirement's `Self` becomes this struct's type. Receiver
            // conventions are part of the trait method contract.
            for req_sig in req_sigs {
                let want =
                    MethodSig {
                        decls: req_sig.decls.clone(),
                        availability: req_sig.availability.clone(),
                        has_self: true,
                        params: req_sig
                            .params
                            .iter()
                            .map(|t| self.resolve_assoc_ty(&substitute_self(t, self_ty)))
                            .collect(),
                        names: req_sig.names.clone(),
                        required: req_sig.required.clone(),
                        variadic: req_sig.variadic.as_ref().map(|ty| {
                            Box::new(self.resolve_assoc_ty(&substitute_self(ty, self_ty)))
                        }),
                        variadic_index: req_sig.variadic_index,
                        kw_variadic: req_sig.kw_variadic.as_ref().map(|ty| {
                            Box::new(self.resolve_assoc_ty(&substitute_self(ty, self_ty)))
                        }),
                        kw_variadic_index: req_sig.kw_variadic_index,
                        positional_only: req_sig.positional_only,
                        keyword_only: req_sig.keyword_only,
                        conventions: req_sig.conventions.clone(),
                        ret: self.resolve_assoc_ty(&substitute_self(&req_sig.ret, self_ty)),
                        raises: req_sig.raises,
                        error: req_sig.error.as_ref().map(|error| {
                            Box::new(self.resolve_assoc_ty(&substitute_self(error, self_ty)))
                        }),
                        self_convention: req_sig.self_convention,
                        ref_params: req_sig.ref_params.clone(),
                        ref_return: req_sig.ref_return.clone(),
                        implicit: req_sig.implicit,
                    };
                if !got_sigs.iter().any(|got| {
                    self.method_satisfies_requirement_under(
                        got,
                        &want,
                        conformance_assumption.as_ref(),
                    )
                }) {
                    return Err(TypeError::TraitMethodMismatch {
                        struct_name: name.to_string(),
                        trait_name: tr.to_string(),
                        method: mname.clone(),
                    });
                }
            }
        }
        for (member, req) in &trait_info.comptime_members {
            let got = struct_info.associated.get(member).ok_or_else(|| {
                TypeError::MissingTraitComptimeMember {
                    struct_name: name.to_string(),
                    trait_name: tr.to_string(),
                    member: member.clone(),
                }
            })?;
            if !self.ct_member_satisfies(got, req, self_ty, conformance_assumption.as_ref()) {
                return Err(TypeError::TraitComptimeMemberMismatch {
                    struct_name: name.to_string(),
                    trait_name: tr.to_string(),
                    member: member.clone(),
                });
            }
        }
        Ok(())
    }

    fn method_satisfies_requirement_under(
        &self,
        got: &MethodSig,
        required: &MethodSig,
        conformance_assumption: Option<&GenericConstraint>,
    ) -> bool {
        let availability_is_covered = got.availability.iter().all(|constraint| {
            required
                .availability
                .iter()
                .any(|premise| generic_constraint_implies(premise, constraint))
                || conformance_assumption
                    .is_some_and(|premise| generic_constraint_implies(premise, constraint))
        });
        if !availability_is_covered {
            return false;
        }
        // Availability was proved above. Normalize it to the requirement before
        // comparing the remainder of the callable contract.
        let mut normalized = got.clone();
        normalized.availability = required.availability.clone();
        method_satisfies_requirement(&normalized, required)
    }

    fn verify_builtin_conformance(
        &self,
        name: &str,
        tr: &str,
        self_ty: &Ty,
    ) -> Result<(), TypeError> {
        let ok = match tr {
            "Copyable" => self.struct_copyable_conformance_ok(name),
            "ImplicitlyCopyable" => self.struct_implicitly_copyable_conformance_ok(name),
            "Movable" => self.is_movable(self_ty),
            "ImplicitlyDeletable" => true,
            "Indexer" => self.structs.get(name).is_some_and(|info| {
                info.methods.get("__mlir_index__").is_some_and(|methods| {
                    methods.iter().any(|method| {
                        method.has_self && method.params.is_empty() && method.ret == Ty::Int
                    })
                })
            }),
            "Writer" => self.structs.get(name).is_some_and(|info| {
                info.methods.get("write_string").is_some_and(|methods| {
                    methods.iter().any(|method| {
                        method.has_self
                            && method.self_convention == Some(ArgConvention::Mut)
                            && method.params == [Ty::String]
                            && method.ret == Ty::None
                    })
                })
            }),
            "Hasher" => self.structs.get(name).is_some_and(|info| {
                let initializes = info.methods.get("__init__").is_some_and(|methods| {
                    methods.iter().any(|method| method.params.is_empty())
                });
                let updates = info.methods.get("update").is_some_and(|methods| {
                    methods.iter().any(|method| {
                        method.self_convention == Some(ArgConvention::Mut)
                            && method.params.len() == 1
                            && method.ret == Ty::None
                    })
                });
                let finishes = info.methods.get("finish").is_some_and(|methods| {
                    methods.iter().any(|method| {
                        method.params.is_empty() && method.ret == Ty::UInt
                    })
                });
                initializes && updates && finishes
            }),
            "Writable" => self.structs.get(name).is_some_and(|info| {
                ["write_to", "write_repr_to"].into_iter().all(|name| {
                    info.methods.get(name).is_none_or(|methods| {
                        methods.iter().any(|method| {
                            method.params.len() == 1
                                && method.conventions[0] == Some(ArgConvention::Mut)
                                && matches!(&method.params[0], Ty::Param { bounds, .. } if bounds.iter().any(|bound| bound == "Writer"))
                                && method.ret == Ty::None
                        })
                    })
                })
            }),
            // An operation trait with a known dunder signature requires the
            // struct to define that dunder (`Addable` needs `__add__`, etc.).
            // Layout/backend markers without a dunder remain accepted-but-shallow.
            _ => match builtin_trait_operation(tr) {
                Some(signature) => {
                    let dunder = signature.split('(').next().unwrap_or(signature);
                    self.structs.get(name).is_some_and(|info| {
                        info.methods
                            .get(dunder)
                            .is_some_and(|methods| methods.iter().any(|method| method.has_self))
                    })
                }
                None => true,
            },
        };
        if ok {
            Ok(())
        } else {
            Err(TypeError::TraitNotSatisfied {
                param: "Self".to_string(),
                ty: self_ty.to_string(),
                trait_name: tr.to_string(),
                reason: self.trait_failure_reason(self_ty, tr),
            })
        }
    }

    fn ct_member_satisfies(
        &self,
        value: &CtValue,
        req: &CtMemberReq,
        self_ty: &Ty,
        conformance_assumption: Option<&GenericConstraint>,
    ) -> bool {
        match req {
            CtMemberReq::Value(expected) => self
                .ct_value_ty(value, self_ty)
                .is_some_and(|actual| coerces(&actual, expected)),
            CtMemberReq::Type { bounds } => {
                let CtValue::Type(ty) = value else {
                    return false;
                };
                bounds.iter().all(|bound| {
                    self.conforms_to_under_assumption(ty, bound, conformance_assumption)
                })
            }
        }
    }

    fn conforms_to_under_assumption(
        &self,
        ty: &Ty,
        required: &str,
        assumption: Option<&GenericConstraint>,
    ) -> bool {
        self.conforms_to_under_assumption_inner(ty, required, assumption, &mut HashSet::new())
    }

    fn conforms_to_under_assumption_inner(
        &self,
        ty: &Ty,
        required: &str,
        assumption: Option<&GenericConstraint>,
        visiting: &mut HashSet<(String, String)>,
    ) -> bool {
        if self.conforms_to(ty, required) {
            return true;
        }
        if let Ty::Param { name, .. } = ty {
            let needed = GenericConstraint::Conforms {
                param: name.clone(),
                trait_name: required.to_string(),
            };
            return assumption.is_some_and(|known| generic_constraint_implies(known, &needed));
        }
        let Ty::Struct(name, args) = ty else {
            return false;
        };
        let key = (name.clone(), required.to_string());
        if !visiting.insert(key.clone()) {
            return false;
        }
        let result = self.structs.get(name).is_some_and(|info| {
            info.conforms.iter().any(|declared| {
                if declared != required && !self.trait_refines(declared, required) {
                    return false;
                }
                let Some(condition) = info.conformance_conditions.get(declared) else {
                    return true;
                };
                let Ok(condition) = self.compile_generic_constraint(condition) else {
                    return false;
                };
                let environment: HashMap<&str, &TyArg> = info
                    .decls
                    .iter()
                    .zip(args)
                    .map(|(decl, argument)| (decl.name().trim_start_matches('*'), argument))
                    .collect();
                self.eval_constraint_under_assumption(
                    &condition,
                    &environment,
                    assumption,
                    visiting,
                )
            })
        });
        visiting.remove(&key);
        result
    }

    fn eval_constraint_under_assumption(
        &self,
        constraint: &GenericConstraint,
        environment: &HashMap<&str, &TyArg>,
        assumption: Option<&GenericConstraint>,
        visiting: &mut HashSet<(String, String)>,
    ) -> bool {
        use GenericConstraint::*;
        match constraint {
            Conforms { param, trait_name } => environment
                .get(param.as_str())
                .is_some_and(|argument| match argument {
                    TyArg::Ty(ty) => self.conforms_to_under_assumption_inner(
                        ty,
                        trait_name,
                        assumption,
                        visiting,
                    ),
                    TyArg::Val(_) => false,
                }),
            ConformsPack { param, trait_name } => environment
                .get(param.as_str())
                .is_some_and(|argument| match argument {
                    TyArg::Val(CtValue::Tuple(values)) => values.iter().all(|value| {
                        matches!(value, CtValue::Type(ty) if self.conforms_to_under_assumption_inner(
                            ty,
                            trait_name,
                            assumption,
                            visiting,
                        ))
                    }),
                    _ => false,
                }),
            And(left, right) => {
                self.eval_constraint_under_assumption(left, environment, assumption, visiting)
                    && self.eval_constraint_under_assumption(
                        right,
                        environment,
                        assumption,
                        visiting,
                    )
            }
            Or(left, right) => {
                self.eval_constraint_under_assumption(left, environment, assumption, visiting)
                    || self.eval_constraint_under_assumption(
                        right,
                        environment,
                        assumption,
                        visiting,
                    )
            }
            // Do not derive a negative proposition from an unknown symbolic
            // fact. Exact non-conformance constraints continue through the
            // ordinary evaluator when their arguments are concrete.
            Not(_) => self.eval_generic_constraint(constraint, environment),
            _ => self.eval_generic_constraint(constraint, environment),
        }
    }

    fn ct_value_ty(&self, value: &CtValue, self_ty: &Ty) -> Option<Ty> {
        match value {
            CtValue::Int(_) | CtValue::Param(_) => Some(Ty::Int),
            CtValue::UInt(_) => Some(Ty::UInt),
            CtValue::Float(_) => Some(Ty::Float64),
            CtValue::IntLiteral(_) => Some(Ty::IntLiteral),
            CtValue::FloatLiteral(_) => Some(Ty::FloatLiteral),
            CtValue::Bool(_) => Some(Ty::Bool),
            CtValue::Str(_) => Some(Ty::String),
            CtValue::Tuple(values) => values
                .iter()
                .map(|v| self.ct_value_ty(v, self_ty))
                .collect::<Option<Vec<_>>>()
                .map(|elements| {
                    if matches!(self_ty, Ty::Tuple(_) | Ty::RuntimePack(_)) {
                        Ty::Tuple(elements)
                    } else {
                        nominal_tuple_type(elements)
                    }
                }),
            CtValue::List(values) => {
                let first = values.first()?;
                let elem = self.ct_value_ty(first, self_ty)?;
                if values.iter().skip(1).all(|v| {
                    self.ct_value_ty(v, self_ty)
                        .is_some_and(|ty| coerces(&ty, &elem))
                }) {
                    Some(list_type(elem))
                } else {
                    None
                }
            }
            CtValue::Type(_) | CtValue::Reflected(_) => {
                let _ = self_ty;
                None
            }
        }
    }

    fn check_method(
        &mut self,
        self_ty: &Ty,
        m: &Method,
        module: Option<String>,
        declaration: &str,
        method_index: usize,
    ) -> Result<(), TypeError> {
        let decls = self.classify_params(&m.type_params)?;
        self.tparams.push(type_scope(&decls));
        let saved = self.enclosing_type_params.clone();
        self.enclosing_type_params.extend(m.type_params.clone());
        let assumptions = (|| {
            let Some(condition) = &m.where_clause else {
                return Ok(HashSet::new());
            };
            let constraint = self.compile_generic_constraint(condition)?;
            let mut facts = Vec::new();
            guaranteed_conformance_atoms(&constraint, &mut facts);
            Ok(facts
                .into_iter()
                .map(|(parameter, trait_name)| {
                    (parameter.trim_start_matches('*').to_string(), trait_name)
                })
                .collect())
        })();
        let result = match assumptions {
            Ok(assumptions) => {
                self.assumed_conformances.push(assumptions);
                let result = (|| {
                    for param in 0..m.params.len() {
                        let site = crate::checked::AnnotationSite::MethodParam {
                            module: module.clone(),
                            declaration: declaration.to_string(),
                            method: method_index,
                            param,
                        };
                        let ty = self
                            .declaration_types
                            .borrow()
                            .get(&site)
                            .cloned()
                            .ok_or_else(|| {
                                TypeError::InvariantViolation(format!(
                                    "method parameter {} for '{}.{}' has no checked type",
                                    param, declaration, m.name
                                ))
                            })?;
                        if self.is_implicitly_deletable(&ty) {
                            self.explicit_destroy_deletability
                                .borrow_mut()
                                .declarations
                                .insert(site);
                        }
                    }
                    self.check_method_inner(self_ty, m)
                })();
                self.assumed_conformances.pop();
                result
            }
            Err(error) => Err(error),
        };
        self.enclosing_type_params = saved;
        self.tparams.pop();
        result
    }

    fn check_method_inner(&mut self, self_ty: &Ty, m: &Method) -> Result<(), TypeError> {
        let is_implicit = m
            .decorators
            .iter()
            .any(|decorator| decorator.path.len() == 1 && decorator.path[0] == "implicit");
        if is_implicit
            && (m.name != "__init__"
                || !m.has_self
                || m.self_convention != Some(ArgConvention::Out)
                || m.params.len() != 1
                || m.params[0].kind != crate::ast::ParamKind::Regular
                || m.params[0].default.is_some()
                || m.params[0].convention.is_some()
                || m.ret.is_some()
                || m.raises)
        {
            return Err(TypeError::Unsupported(
                "@implicit requires a non-raising single-argument '__init__(out self, value: T)'"
                    .to_string(),
            ));
        }
        self.validate_origin_signature(
            &self.enclosing_type_params,
            &m.params,
            m.self_origin.as_ref(),
        )?;
        if !is_mojo_copy_constructor(m)
            && !is_mojo_move_constructor(m)
            && let Some(feature) = Self::advanced_param_feature(
                &m.params,
                m.positional_only,
                m.keyword_only,
                false,
                false,
                false,
            )
        {
            return Err(TypeError::Unsupported(feature.to_string()));
        }
        // `out self` initializes the receiver: it is allowed on the **`__init__`**
        // lifecycle method (a hand-written constructor), where `self`'s fields are
        // assigned in the body. `ref self` (parametric-mutability references), and
        // `out self` on any other method, still need semantics we don't model, so
        // they stay flagged. A plain `self`, `read self`, `mut self`, or `var
        // self` consuming method is fine.
        // `out self` initializes the receiver — allowed on the lifecycle methods
        // `__init__` (constructor), `__copyinit__` (copy), and `__moveinit__` (move),
        // whose bodies assign `self`'s fields. `ref self`, and `out self` elsewhere,
        // stay flagged.
        let is_lifecycle_init = matches!(
            m.name.as_str(),
            "__init__" | "__copyinit__" | "__moveinit__"
        );
        let out_init =
            matches!(m.self_convention, Some(crate::ast::ArgConvention::Out)) && is_lifecycle_init;
        if matches!(m.self_convention, Some(crate::ast::ArgConvention::Out)) && !out_init {
            return Err(TypeError::Unsupported(
                "'out self' receiver outside a lifecycle initializer".to_string(),
            ));
        }
        let ret_ty = match &m.ret {
            Some(SourceType::Ref { referent, .. }) => self.ty_from_anno(referent)?,
            Some(t) => self.ty_from_anno(t)?,
            None => Ty::None,
        };
        let regular: Vec<&FnParam> = m
            .params
            .iter()
            .filter(|param| param.kind == crate::ast::ParamKind::Regular)
            .collect();
        let ref_return = match &m.ret {
            Some(SourceType::Ref { origin, .. }) => Some(lower_ref_sig(
                origin.as_ref().ok_or_else(|| {
                    TypeError::Unsupported("reference return requires an origin".to_string())
                })?,
                &self.enclosing_type_params,
                &regular,
            )?),
            _ => None,
        };
        for param in &m.params {
            if let Some(default) = &param.default {
                let expected = self.ty_from_anno(&param.ty)?;
                let found = self.infer(default)?;
                if !coerces(&found, &expected) {
                    return Err(TypeError::TypeMismatch {
                        expected: expected.to_string(),
                        found: found.to_string(),
                        context: format!("default value of method parameter '{}'", param.name),
                    });
                }
            }
        }
        self.push_scope();
        self.raising_context
            .push(self.declared_error(m.raises, m.raises_type.as_ref())?);
        let mut result = self.bind_and_check_method(self_ty, m, &ret_ty, ref_return);
        // Definite initialization (conservative, flow-insensitive first pass): an
        // `__init__` must assign every declared field somewhere in its body, so a
        // constructed value has no unset fields. Path-sensitive DI (assign exactly
        // once, before any read, on every path) is left for a later refinement.
        if result.is_ok()
            && out_init
            && let Ty::Struct(sname, _) = self_ty
        {
            result = self.check_definite_init(sname, &m.name, &m.body);
        }
        self.raising_context.pop();
        self.pop_scope();
        result
    }

    /// Verify an `out self` lifecycle method (`method`) assigns every declared field
    /// of `sname` (flow-insensitive: assigned *somewhere*). Reports the first missing
    /// field.
    fn check_definite_init(
        &self,
        sname: &str,
        method: &str,
        body: &[Stmt],
    ) -> Result<(), TypeError> {
        let info = self.structs.get(sname).ok_or_else(|| {
            TypeError::InvariantViolation(format!("struct '{sname}' was not registered"))
        })?;
        for (field, _) in &info.fields {
            if !definitely_initializes_self_field(body, field) {
                return Err(TypeError::UninitializedField {
                    struct_name: sname.to_string(),
                    method: method.to_string(),
                    field: field.clone(),
                });
            }
        }
        Ok(())
    }

    fn bind_and_check_method(
        &mut self,
        self_ty: &Ty,
        m: &Method,
        ret_ty: &Ty,
        ref_return: Option<crate::origin::RefSig>,
    ) -> Result<(), TypeError> {
        // Compile-time callable/scalar value parameters occupy named runtime
        // slots in a method body, just as they do in a generic free function.
        // Type parameters remain type-only and are available through `tparams`.
        let method_decls = self.classify_params(&m.type_params)?;
        for declaration in &method_decls {
            if let ParamDecl::Value {
                name, ty, variadic, ..
            } = declaration
            {
                self.declare_immutable(
                    name.trim_start_matches('*'),
                    if *variadic {
                        Ty::VariadicPack(ty.clone())
                    } else {
                        (**ty).clone()
                    },
                )?;
            }
        }
        let mut reference_type_params = self.enclosing_type_params.clone();
        reference_type_params.extend(m.type_params.iter().cloned());
        let self_writable = ref_binding_is_writable(
            m.self_convention,
            m.self_origin.as_deref(),
            &reference_type_params,
        );
        if m.has_self {
            self.declare_with_mutability("self", self_ty.clone(), self_writable)?;
            if self.type_carries_loans(self_ty)
                && let Some(owner) = self.lookup_owner("self")
            {
                self.set_aggregate_origins(
                    "self",
                    vec![crate::origin::Origin::Place(crate::origin::OriginPlace {
                        root: owner,
                        path: Vec::new(),
                    })],
                );
            }
        }
        for p in &m.params {
            let mut pty = self.ty_from_anno(&p.ty)?;
            pty = match p.kind {
                // A specialized heterogeneous pack (`$pack` → RuntimePack)
                // binds as the tuple itself; an ordinary variadic collects into
                // source-inexpressible homogeneous pack storage.
                crate::ast::ParamKind::Variadic => match pty {
                    Ty::RuntimePack(elements) => Ty::Tuple(elements),
                    _ => Ty::VariadicPack(Box::new(pty)),
                },
                crate::ast::ParamKind::KwVariadic => {
                    self.kwargs_collector_ty(pty, &format!("keyword collector '{}'", p.name))?
                }
                crate::ast::ParamKind::Regular => pty,
            };
            self.declare_with_mutability(
                &p.name,
                pty.clone(),
                p.kind == crate::ast::ParamKind::KwVariadic
                    || ref_parameter_is_writable(p, &reference_type_params),
            )?;
            if matches!(p.convention, Some(crate::ast::ArgConvention::Ref)) {
                self.register_reference_parameter(
                    &p.name,
                    pty.clone(),
                    ref_parameter_is_writable(p, &reference_type_params),
                );
            }
            if !matches!(pty, Ty::Ref(_))
                && self.type_carries_loans(&pty)
                && let Some(owner) = self.lookup_owner(&p.name)
            {
                self.set_aggregate_origins(
                    &p.name,
                    vec![crate::origin::Origin::Place(crate::origin::OriginPlace {
                        root: owner,
                        path: Vec::new(),
                    })],
                );
            }
        }
        // `self` is writable in a `mut self` method, or an `out self` `__init__`
        // (which assigns its fields). Restored after the body.
        let saved = std::mem::replace(&mut self.self_mutable, self_writable);
        let initializing = matches!(m.self_convention, Some(crate::ast::ArgConvention::Out))
            && matches!(
                lifecycle_method_name(m),
                "__init__" | "__copyinit__" | "__moveinit__"
            );
        let saved_initializing = std::mem::replace(&mut self.self_initializing, initializing);
        let owners: Vec<_> = m
            .params
            .iter()
            .filter(|param| param.kind == crate::ast::ParamKind::Regular)
            .map(|param| {
                self.lookup_owner(&param.name)
                    .expect("bound method parameter")
            })
            .collect();
        let self_owner = self.lookup_owner("self");
        let mut allowed: HashSet<_> = owners.iter().copied().collect();
        allowed.extend(self_owner);
        self.aggregate_escape_contexts
            .push((self.scopes.len().saturating_sub(1), allowed));
        self.return_ref_contracts.push(ref_return.map(|signature| {
            (
                signature,
                owners,
                self_owner.map(|root| crate::origin::OriginPlace {
                    root,
                    path: Vec::new(),
                }),
            )
        }));
        // A method body is a function scope for nested closures just as a
        // top-level `def` body is. In particular, an explicit capture list on a
        // method-local function may name `self`, parameters, and method locals.
        self.function_bases.push(self.scopes.len() - 1);
        let result = self.check_block(&m.body, Some(ret_ty), false);
        self.function_bases.pop();
        self.return_ref_contracts.pop();
        self.aggregate_escape_contexts.pop();
        self.self_mutable = saved;
        self.self_initializing = saved_initializing;
        result?;
        if *ret_ty != Ty::None && !definitely_returns(&m.body) {
            return Err(TypeError::MissingReturn(m.name.clone()));
        }
        Ok(())
    }

    /// Type a struct construction `Name[param_args](args)` (the fieldwise
    /// constructor). Type parameters are supplied explicitly or inferred from the
    /// field arguments; value parameters must be supplied explicitly.
    fn infer_construction(
        &self,
        span: SourceSpan,
        name: &str,
        param_args: &[crate::ast::ParamArg],
        args: &[Expr],
        kwargs: &[crate::ast::KwArg],
    ) -> Result<Ty, TypeError> {
        let info = self.structs.get(name).ok_or_else(|| {
            TypeError::InvariantViolation(format!("constructor target '{name}' is not registered"))
        })?;
        if !kwargs.is_empty() && args.is_empty() && kwargs.len() == 1 && kwargs[0].name == "copy" {
            let Some(sig) = info
                .methods
                .get("__copyinit__")
                .and_then(|sigs| sigs.iter().find(|sig| sig.params.len() == 1))
            else {
                return Err(TypeError::BadCall {
                    func: name.to_string(),
                    reason: "no matching copy constructor".to_string(),
                });
            };
            let params = sig.params.clone();
            let decls = info.decls.clone();
            let arg_ty = self.infer(&kwargs[0].value)?;
            let (subst, tyargs) = self.resolve_use_params(
                name,
                &decls,
                param_args,
                &params,
                std::slice::from_ref(&arg_ty),
            )?;
            let expected = substitute(&params[0], &subst);
            if !coerces(&arg_ty, &expected) {
                return Err(TypeError::TypeMismatch {
                    expected: expected.to_string(),
                    found: arg_ty.to_string(),
                    context: format!("argument 'copy' to '{}.__init__'", name),
                });
            }
            return Ok(self.struct_instance_type(name, tyargs));
        }
        // A hand-written `def __init__(out self, …)` is the constructor: check the
        // call arguments against its parameters (the `self` receiver is implicit).
        // Takes precedence over `@fieldwise_init`. On a **generic** struct, the type
        // parameters are solved by unifying `__init__`'s parameter types against the
        // argument types — exactly as the fieldwise path unifies field types.
        if let Some(sigs) = info.methods.get("__init__") {
            if info.decls.is_empty() {
                let mut matches = Vec::new();
                for sig in sigs {
                    if let Ok(scored) = self.score_method_call(
                        sig,
                        &sig.params,
                        sig.variadic.as_deref(),
                        sig.kw_variadic.as_deref(),
                        args,
                        kwargs,
                    ) {
                        matches.push(MethodCallResolution {
                            conversion_score: scored.rank,
                            slots: scored.slots,
                            positional_overflow: scored.positional_overflow,
                            keyword_overflow: scored.keyword_overflow,
                            variadic_element: sig.variadic.as_deref().cloned(),
                            keyword_element: sig.kw_variadic.as_deref().cloned(),
                            conventions: sig.conventions.clone(),
                            self_convention: sig.self_convention,
                            return_type: self.struct_instance_type(name, Vec::new()),
                            raises: sig.raises,
                            error: sig.error.clone(),
                            mutates_receiver: false,
                            consumes_receiver: false,
                            lowered_name: (sigs.len() > 1)
                                .then(|| method_lowered_name(name, "__init__", sig)),
                            ref_params: sig.ref_params.clone(),
                            ref_return: None,
                            param_types: sig.params.clone(),
                            param_decls: sig.decls.clone(),
                        });
                    }
                }
                let selected = select_method_overload("__init__", matches).map_err(|kind| {
                    TypeError::BadCall {
                        func: name.to_string(),
                        reason: match kind {
                            OverloadSelect::NoMatch => {
                                "no constructor overload matches the supplied arguments"
                            }
                            OverloadSelect::Ambiguous => "ambiguous overloaded constructor call",
                        }
                        .to_string(),
                    }
                })?;
                if let Some(target) = &selected.lowered_name {
                    self.overload_targets
                        .borrow_mut()
                        .insert(span, target.clone());
                }
                self.record_selected_method_conversions("__init__", &selected, args, kwargs)?;
                // Constructor calls use the same reference-parameter handles as
                // ordinary calls. Record their retained caller places after
                // overload selection so MIR does not have to inspect the
                // constructor declaration (and rejected candidates cannot leak
                // facts into the selected call).
                self.solve_call_origins(
                    &selected.slots,
                    &selected.conventions,
                    &selected.ref_params,
                    None,
                    args,
                    kwargs,
                )?;
                for (index, slot) in selected.slots.iter().enumerate() {
                    if !matches!(
                        selected.conventions.get(index),
                        Some(Some(ArgConvention::Var | ArgConvention::Deinit))
                    ) {
                        continue;
                    }
                    let argument = match slot {
                        ArgSlot::Positional(position) => &args[*position],
                        ArgSlot::Keyword(position) => &kwargs[*position].value,
                        ArgSlot::Default => continue,
                    };
                    let ty = self.infer(argument)?;
                    self.check_consuming(
                        argument,
                        &ty,
                        &format!("argument {} to '{name}'", index + 1),
                    )?;
                }
                return Ok(self.struct_instance_type(name, Vec::new()));
            }
            if sigs.len() == 1 {
                let sig = &sigs[0];
                let params = sig.params.clone();
                let decls = info.decls.clone();
                let arg_tys = args
                    .iter()
                    .map(|a| self.infer(a))
                    .collect::<Result<Vec<_>, _>>()?;
                let (subst, tyargs) =
                    self.resolve_use_params(name, &decls, param_args, &params, &arg_tys)?;
                for (i, (aty, pty)) in arg_tys.iter().zip(&params).enumerate() {
                    let expected = substitute(pty, &subst);
                    if !coerces(aty, &expected) {
                        return Err(TypeError::TypeMismatch {
                            expected: expected.to_string(),
                            found: aty.to_string(),
                            context: format!("argument {} to '{}.__init__'", i + 1, name),
                        });
                    }
                    if matches!(
                        sig.conventions.get(i),
                        Some(Some(ArgConvention::Var | ArgConvention::Deinit))
                    ) {
                        self.check_consuming(
                            &args[i],
                            aty,
                            &format!("argument {} to '{}'", i + 1, name),
                        )?;
                    }
                }
                let slots = (0..args.len()).map(ArgSlot::Positional).collect::<Vec<_>>();
                self.solve_call_origins(
                    &slots,
                    &sig.conventions,
                    &sig.ref_params,
                    None,
                    args,
                    kwargs,
                )?;
                return Ok(self.struct_instance_type(name, tyargs));
            }
            let decls = info.decls.clone();
            let arg_tys = args
                .iter()
                .map(|a| self.infer(a))
                .collect::<Result<Vec<_>, _>>()?;
            let overloaded = sigs.len() > 1;
            let mut matches = Vec::new();
            for sig in sigs {
                let params = sig.params.clone();
                if params.len() != arg_tys.len() {
                    continue;
                }
                if let Ok((subst, tyargs)) =
                    self.resolve_use_params(name, &decls, param_args, &params, &arg_tys)
                {
                    let mut score = 0;
                    let mut ok = true;
                    for (aty, pty) in arg_tys.iter().zip(&params) {
                        let expected = substitute(pty, &subst);
                        if !coerces(aty, &expected) {
                            ok = false;
                            break;
                        }
                        if *aty != expected {
                            score += 1;
                        }
                    }
                    if ok {
                        matches.push((score, sig.clone(), tyargs));
                    }
                }
            }
            let best = matches.iter().map(|(score, ..)| *score).min();
            if let Some(best) = best {
                let mut best_matches = matches
                    .into_iter()
                    .filter(|(score, ..)| *score == best)
                    .collect::<Vec<_>>();
                if best_matches.len() != 1 {
                    return Err(TypeError::BadCall {
                        func: name.to_string(),
                        reason: "ambiguous overloaded constructor call".to_string(),
                    });
                }
                let (_, sig, tyargs) = best_matches.remove(0);
                for (i, aty) in arg_tys.iter().enumerate() {
                    if matches!(
                        sig.conventions.get(i),
                        Some(Some(ArgConvention::Var | ArgConvention::Deinit))
                    ) {
                        self.check_consuming(
                            &args[i],
                            aty,
                            &format!("argument {} to '{}'", i + 1, name),
                        )?;
                    }
                }
                if overloaded {
                    self.overload_targets
                        .borrow_mut()
                        .insert(span, method_lowered_name(name, "__init__", &sig));
                }
                let slots = (0..args.len()).map(ArgSlot::Positional).collect::<Vec<_>>();
                self.solve_call_origins(
                    &slots,
                    &sig.conventions,
                    &sig.ref_params,
                    None,
                    args,
                    kwargs,
                )?;
                return Ok(self.struct_instance_type(name, tyargs));
            }
            return Err(TypeError::BadCall {
                func: name.to_string(),
                reason: "no constructor overload matches the supplied arguments".to_string(),
            });
        }
        if info.methods.contains_key("__init__") {
            return Err(TypeError::ArityMismatch {
                name: name.to_string(),
                expected: info
                    .methods
                    .get("__init__")
                    .and_then(|sigs| sigs.first())
                    .map(|sig| sig.params.len())
                    .unwrap_or(0),
                got: args.len(),
            });
        }
        if !info.fieldwise_init {
            return Err(TypeError::NoConstructor(name.to_string()));
        }
        let decls = info.decls.clone();
        let field_tys: Vec<Ty> = info.fields.iter().map(|(_, t)| t.clone()).collect();
        if field_tys.len() != args.len() {
            return Err(TypeError::ArityMismatch {
                name: name.to_string(),
                expected: field_tys.len(),
                got: args.len(),
            });
        }
        let arg_tys = args
            .iter()
            .zip(&field_tys)
            .map(|(argument, field)| {
                if self.type_contains_reference(field) {
                    self.infer_storage_value(argument, field)
                } else {
                    self.infer(argument)
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        let (subst, tyargs) =
            self.resolve_use_params(name, &decls, param_args, &field_tys, &arg_tys)?;
        for (i, (aty, fty)) in arg_tys.iter().zip(&field_tys).enumerate() {
            let expected = substitute(fty, &subst);
            if !Self::storage_value_coerces(aty, &expected) {
                return Err(TypeError::TypeMismatch {
                    expected: expected.to_string(),
                    found: aty.to_string(),
                    context: format!("field {} of '{}'", i + 1, name),
                });
            }
            if self.type_contains_reference(&expected) {
                self.mark_reference_storage_uses(&args[i], &expected);
            }
            if matches!(expected, Ty::Ref(_)) {
                continue;
            }
            // A constructor stores each argument in a field by value — a consuming
            // position.
            self.check_consuming(&args[i], aty, &format!("field {} of '{}'", i + 1, name))?;
        }
        Ok(self.struct_instance_type(name, tyargs))
    }

    /// Resolve a generic use site's parameters, returning a type-parameter
    /// substitution and the full argument list (types + values) for the struct's
    /// identity. When `param_args` is non-empty the parameters are supplied
    /// explicitly (positionally); otherwise the type parameters are inferred from
    /// `patterns`/`actuals` (a value parameter cannot be inferred).
    fn resolve_use_params(
        &self,
        name: &str,
        decls: &[ParamDecl],
        param_args: &[crate::ast::ParamArg],
        patterns: &[Ty],
        actuals: &[Ty],
    ) -> Result<(HashMap<String, Ty>, Vec<TyArg>), TypeError> {
        let mut subst: HashMap<String, Ty> = HashMap::new();
        if decls.is_empty() {
            if !param_args.is_empty() {
                return Err(TypeError::WrongTypeArgCount {
                    name: name.to_string(),
                    expected: 0,
                    got: param_args.len(),
                });
            }
            return Ok((subst, Vec::new()));
        }
        if !param_args.is_empty() {
            let mut bound: Vec<Vec<&crate::ast::ParamArg>> = vec![Vec::new(); decls.len()];
            let mut positional = 0;
            let mut saw_keyword = false;
            for argument in param_args {
                match argument {
                    crate::ast::ParamArg::Named {
                        name: keyword,
                        value,
                    } => {
                        saw_keyword = true;
                        let Some(index) = decls
                            .iter()
                            .position(|decl| decl.name().trim_start_matches('*') == keyword)
                        else {
                            return Err(TypeError::Unsupported(format!(
                                "generic '{name}' has no parameter named '{keyword}'"
                            )));
                        };
                        if !bound[index].is_empty() {
                            return Err(TypeError::Redeclaration(keyword.clone()));
                        }
                        bound[index].push(value);
                    }
                    positional_argument => {
                        if saw_keyword {
                            return Err(TypeError::Unsupported(
                                "positional compile-time argument follows a keyword argument"
                                    .to_string(),
                            ));
                        }
                        while positional < decls.len()
                            && !bound[positional].is_empty()
                            && !matches!(
                                decls[positional],
                                ParamDecl::Type { variadic: true, .. }
                                    | ParamDecl::Value { variadic: true, .. }
                            )
                        {
                            positional += 1;
                        }
                        if positional >= decls.len() {
                            return Err(TypeError::WrongTypeArgCount {
                                name: name.to_string(),
                                expected: decls.len(),
                                got: param_args.len(),
                            });
                        }
                        bound[positional].push(positional_argument);
                        if !matches!(
                            decls[positional],
                            ParamDecl::Type { variadic: true, .. }
                                | ParamDecl::Value { variadic: true, .. }
                        ) {
                            positional += 1;
                        }
                    }
                }
            }
            let mut tyargs = Vec::with_capacity(decls.len());
            let mut value_environment = HashMap::new();
            for (decl, arguments) in decls.iter().zip(bound) {
                let infer_only = matches!(
                    decl,
                    ParamDecl::Type {
                        infer_only: true,
                        ..
                    } | ParamDecl::Value {
                        infer_only: true,
                        ..
                    }
                );
                if infer_only && !arguments.is_empty() {
                    return Err(TypeError::Unsupported(format!(
                        "infer-only parameter '{}' cannot be supplied explicitly",
                        decl.name().trim_start_matches('*')
                    )));
                }
                let variadic = matches!(
                    decl,
                    ParamDecl::Type { variadic: true, .. }
                        | ParamDecl::Value { variadic: true, .. }
                );
                if variadic {
                    let values = arguments
                        .into_iter()
                        .map(|argument| self.resolve_param_arg(decl, argument))
                        .map(|result| match result? {
                            TyArg::Ty(ty) => Ok(CtValue::Type(Box::new(ty))),
                            TyArg::Val(value) => Ok(value),
                        })
                        .collect::<Result<Vec<_>, TypeError>>()?;
                    let value = CtValue::Tuple(values);
                    value_environment.insert(
                        decl.name().trim_start_matches('*').to_string(),
                        value.clone(),
                    );
                    tyargs.push(TyArg::Val(value));
                    continue;
                }
                let tyarg = if let Some(argument) = arguments.first() {
                    self.resolve_param_arg(decl, argument)?
                } else if let ParamDecl::Value {
                    callable_default: Some(_),
                    name,
                    ..
                } = decl
                {
                    // The VM evaluates the symbolic default after reifying all
                    // preceding scalar/callable parameters.  Generic identity
                    // records only that this runtime value occupies the slot.
                    TyArg::Val(CtValue::Param(name.clone()))
                } else if let ParamDecl::Value {
                    default: Some(value),
                    ty,
                    ..
                } = decl
                {
                    let value = value.evaluate(&value_environment).ok_or_else(|| {
                        TypeError::NotComptime(format!("default for parameter '{}'", decl.name()))
                    })?;
                    let rendered = value.to_string();
                    TyArg::Val(
                        value
                            .materialize_as(ty)
                            .ok_or_else(|| TypeError::TypeMismatch {
                                expected: ty.to_string(),
                                found: rendered,
                                context: format!("default for parameter '{}'", decl.name()),
                            })?,
                    )
                } else if let ParamDecl::Type {
                    default: Some(ty), ..
                } = decl
                {
                    TyArg::Ty((**ty).clone())
                } else {
                    return Err(TypeError::CannotInferTypeParam {
                        name: name.to_string(),
                        param: decl.name().to_string(),
                    });
                };
                if let (ParamDecl::Type { name, .. }, TyArg::Ty(t)) = (decl, &tyarg) {
                    subst.insert(name.clone(), t.clone());
                }
                tyargs.push(tyarg);
                if let Some(TyArg::Val(value)) = tyargs.last() {
                    value_environment.insert(
                        decl.name().trim_start_matches('*').to_string(),
                        value.clone(),
                    );
                }
            }
            self.validate_callable_parameter_bounds(name, decls, &tyargs)?;
            self.validate_generic_constraints(name, decls, &tyargs)?;
            return Ok((subst, tyargs));
        }
        // Inference: only type parameters, solved from the argument types.
        for (pat, act) in patterns.iter().zip(actuals) {
            if let Ty::Param { name, bounds, .. } = pat
                && name.starts_with('*')
            {
                for bound in bounds {
                    if !self.conforms_to(act, bound) {
                        return Err(TypeError::TraitNotSatisfied {
                            param: name.clone(),
                            ty: act.to_string(),
                            trait_name: bound.clone(),
                            reason: self.trait_failure_reason(act, bound),
                        });
                    }
                }
                subst.entry(name.clone()).or_insert_with(|| pat.clone());
            } else {
                unify(pat, act, &mut subst)?;
            }
        }
        let inferred_packs: HashMap<String, Vec<CtValue>> = patterns
            .iter()
            .zip(actuals)
            .filter_map(|(pattern, actual)| match pattern {
                Ty::Param { name, .. } if name.starts_with('*') => {
                    Some((name.trim_start_matches('*').to_string(), actual.clone()))
                }
                _ => None,
            })
            .fold(HashMap::new(), |mut packs, (name, ty)| {
                packs
                    .entry(name)
                    .or_insert_with(Vec::new)
                    .push(CtValue::Type(Box::new(ty)));
                packs
            });
        let mut tyargs = Vec::with_capacity(decls.len());
        let mut value_environment = HashMap::new();
        for decl in decls {
            match decl {
                ParamDecl::Value {
                    name: pname,
                    default,
                    callable_default,
                    ty,
                    ..
                } => {
                    if let Some(value) = default {
                        let value = value.evaluate(&value_environment).ok_or_else(|| {
                            TypeError::NotComptime(format!("default for parameter '{}'", pname))
                        })?;
                        let rendered = value.to_string();
                        let value =
                            value
                                .materialize_as(ty)
                                .ok_or_else(|| TypeError::TypeMismatch {
                                    expected: ty.to_string(),
                                    found: rendered,
                                    context: format!("default for parameter '{}'", pname),
                                })?;
                        value_environment
                            .insert(pname.trim_start_matches('*').to_string(), value.clone());
                        tyargs.push(TyArg::Val(value));
                    } else if callable_default.is_some() {
                        tyargs.push(TyArg::Val(CtValue::Param(pname.clone())));
                    } else {
                        return Err(TypeError::CannotInferTypeParam {
                            name: name.to_string(),
                            param: pname.clone(),
                        });
                    }
                }
                ParamDecl::Type {
                    name: pname,
                    bounds,
                    default,
                    variadic,
                    ..
                } => {
                    if *variadic {
                        tyargs.push(TyArg::Val(CtValue::Tuple(
                            inferred_packs
                                .get(pname.trim_start_matches('*'))
                                .cloned()
                                .unwrap_or_default(),
                        )));
                        continue;
                    }
                    let solved = subst
                        .get(pname)
                        .cloned()
                        .or_else(|| default.as_ref().map(|default| (**default).clone()))
                        .ok_or_else(|| TypeError::CannotInferTypeParam {
                            name: name.to_string(),
                            param: pname.clone(),
                        })?;
                    subst.insert(pname.clone(), solved.clone());
                    for bound in bounds {
                        if !self.conforms_to(&solved, bound) {
                            return Err(TypeError::TraitNotSatisfied {
                                param: pname.clone(),
                                ty: solved.to_string(),
                                trait_name: bound.clone(),
                                reason: self.trait_failure_reason(&solved, bound),
                            });
                        }
                    }
                    tyargs.push(TyArg::Ty(solved));
                }
            }
        }
        self.validate_callable_parameter_bounds(name, decls, &tyargs)?;
        self.validate_generic_constraints(name, decls, &tyargs)?;
        Ok((subst, tyargs))
    }

    fn validate_generic_constraints(
        &self,
        name: &str,
        decls: &[ParamDecl],
        arguments: &[TyArg],
    ) -> Result<(), TypeError> {
        let environment: HashMap<&str, &TyArg> = decls
            .iter()
            .zip(arguments)
            .map(|(decl, argument)| (decl.name().trim_start_matches('*'), argument))
            .collect();
        for constraint in decls.iter().flat_map(|decl| match decl {
            ParamDecl::Type { constraints, .. } | ParamDecl::Value { constraints, .. } => {
                constraints.as_slice()
            }
        }) {
            if !self.eval_generic_constraint(constraint, &environment) {
                return Err(TypeError::BadCall {
                    func: name.to_string(),
                    reason: format!("generic constraint is not satisfied: {constraint:?}"),
                });
            }
        }
        Ok(())
    }

    fn eval_generic_constraint(
        &self,
        constraint: &GenericConstraint,
        environment: &HashMap<&str, &TyArg>,
    ) -> bool {
        use GenericConstraint::*;
        match constraint {
            Bool(value) => *value,
            Not(value) => !self.eval_generic_constraint(value, environment),
            And(left, right) => {
                self.eval_generic_constraint(left, environment)
                    && self.eval_generic_constraint(right, environment)
            }
            Or(left, right) => {
                self.eval_generic_constraint(left, environment)
                    || self.eval_generic_constraint(right, environment)
            }
            Conforms { param, trait_name } => environment
                .get(param.as_str())
                .and_then(|argument| match argument {
                    TyArg::Ty(ty) => Some(self.conforms_to(ty, trait_name)),
                    TyArg::Val(_) => None,
                })
                .unwrap_or(false),
            ConformsPack { param, trait_name } => environment
                .get(param.as_str())
                .and_then(|argument| {
                    match argument {
                    TyArg::Val(CtValue::Tuple(values)) => Some(values.iter().all(|value| {
                        matches!(value, CtValue::Type(ty) if self.conforms_to(ty, trait_name))
                    })),
                    _ => None,
                }
                })
                .unwrap_or(false),
            Eq(left, right) => {
                match (
                    self.constraint_value(left, environment),
                    self.constraint_value(right, environment),
                ) {
                    (Some(left), Some(right)) => ty_args_equal(&left, &right),
                    _ => false,
                }
            }
            Ne(left, right) => {
                match (
                    self.constraint_value(left, environment),
                    self.constraint_value(right, environment),
                ) {
                    (Some(left), Some(right)) => !ty_args_equal(&left, &right),
                    _ => false,
                }
            }
            Lt(left, right) | Le(left, right) | Gt(left, right) | Ge(left, right) => {
                let (Some(TyArg::Val(left)), Some(TyArg::Val(right))) = (
                    self.constraint_value(left, environment),
                    self.constraint_value(right, environment),
                ) else {
                    return false;
                };
                let op = match constraint {
                    Lt(_, _) => InfixOp::Lt,
                    Le(_, _) => InfixOp::Le,
                    Gt(_, _) => InfixOp::Gt,
                    Ge(_, _) => InfixOp::Ge,
                    _ => unreachable!(),
                };
                compare_ct_integers(op, &left, &right).unwrap_or(false)
            }
        }
    }

    fn constraint_value<'b>(
        &self,
        operand: &'b ConstraintOperand,
        environment: &HashMap<&str, &'b TyArg>,
    ) -> Option<TyArg> {
        match operand {
            ConstraintOperand::Param(name) => {
                environment.get(name.as_str()).map(|value| (*value).clone())
            }
            ConstraintOperand::Value(value) => Some(TyArg::Val(value.clone())),
            ConstraintOperand::Type(ty) => Some(TyArg::Ty(ty.clone())),
        }
    }

    /// Whether `ty` conforms to trait `tr`. Lifecycle marker built-ins are tied
    /// to observable ownership behavior; other built-ins remain recognized but
    /// shallow unless their feature has a dedicated checker path. A user trait is
    /// satisfied nominally: a struct must *declare* conformance, and a type
    /// parameter must carry `tr` among its bounds (so a bounded `T` can be
    /// forwarded to another `[U: tr]` parameter).
    fn conforms_to(&self, ty: &Ty, tr: &str) -> bool {
        if self.has_assumed_conformance(ty, tr) {
            return true;
        }
        if let Ty::Param { bounds, .. } = ty
            && bounds.iter().any(|bound| bound == tr)
        {
            return true;
        }
        if BUILTIN_TRAITS.contains(&tr) {
            return match tr {
                "AnyType" => true,
                "Copyable" => self.is_copyable(ty),
                "ImplicitlyCopyable" => self.is_implicitly_copyable(ty),
                "Movable" => self.is_movable(ty),
                "ImplicitlyDeletable" => self.is_implicitly_deletable(ty),
                "Hashable" => self.is_hashable(ty),
                "Writable" => match ty {
                    Ty::Struct(name, args) => self.struct_conformance_applies(name, args, tr),
                    Ty::Variant(alternatives) => alternatives
                        .iter()
                        .all(|alternative| self.conforms_to(alternative, tr)),
                    Ty::Param { bounds, .. } => bounds.iter().any(|bound| bound == tr),
                    Ty::Func { .. } | Ty::GenericFunc { .. } | Ty::Overload(_) => false,
                    _ => true,
                },
                "Writer" | "Hasher" => match ty {
                    Ty::Struct(name, args) => self.struct_conformance_applies(name, args, tr),
                    Ty::Param { bounds, .. } => bounds.iter().any(|bound| bound == tr),
                    _ => false,
                },
                "Indexer" => match ty {
                    Ty::Int | Ty::IntLiteral => true,
                    Ty::Struct(name, args) => self.struct_conformance_applies(name, args, tr),
                    Ty::Param { bounds, .. } => bounds.iter().any(|bound| bound == tr),
                    _ => false,
                },
                "Equatable" => has_equality_bound_or_concrete(self, ty),
                "Comparable" => self.is_comparable(ty),
                "Absable" | "Roundable" | "Powable" | "Addable" | "Subtractable"
                | "Multipliable" | "Divisible" | "FloorDivisible" | "Modable" => {
                    is_numeric_like(ty)
                }
                "ShiftLeftable" | "ShiftRightable" | "Andable" | "Orable" | "Xorable" => {
                    is_integer_like(ty)
                }
                "Negatable" => is_signed_numeric_like(ty),
                "Intable" => is_numeric_like(ty) || *ty == Ty::Bool,
                "Floatable" => is_numeric_like(ty),
                // Layout/backend markers and future operation traits stay shallow.
                _ => true,
            };
        }
        match ty {
            Ty::Struct(name, args) => self.struct_conformance_applies(name, args, tr),
            Ty::Param { bounds, .. } => bounds
                .iter()
                .any(|bound| bound == tr || self.trait_refines(bound, tr)),
            _ => false,
        }
    }

    /// A positive `conforms_to(T, Trait)` atom from the active method's
    /// availability clause refines only that opaque parameter while its body is
    /// checked. No negative or disjunctive fact reaches this table (see
    /// `guaranteed_conformance_atoms`).
    fn has_assumed_conformance(&self, ty: &Ty, required: &str) -> bool {
        let Ty::Param { name, .. } = ty else {
            return false;
        };
        let name = name.trim_start_matches('*');
        self.assumed_conformances.iter().rev().any(|scope| {
            scope.iter().any(|(parameter, available)| {
                parameter.trim_start_matches('*') == name
                    && (available == required
                        || self.trait_refines(available, required)
                        || matches!(
                            (available.as_str(), required),
                            ("ImplicitlyCopyable", "Copyable")
                        ))
            })
        })
    }

    fn struct_conformance_applies(&self, name: &str, args: &[TyArg], required: &str) -> bool {
        let Some(info) = self.structs.get(name) else {
            return false;
        };
        info.conforms.iter().any(|declared| {
            (declared == required || self.trait_refines(declared, required))
                && info
                    .conformance_conditions
                    .get(declared)
                    .is_none_or(|condition| self.eval_conformance_condition(info, args, condition))
        })
    }

    fn eval_conformance_condition(&self, info: &StructInfo, args: &[TyArg], expr: &Expr) -> bool {
        let arguments: HashMap<&str, &TyArg> = info
            .decls
            .iter()
            .zip(args)
            .map(|(decl, arg)| {
                let name = match decl {
                    ParamDecl::Type { name, .. } | ParamDecl::Value { name, .. } => name.as_str(),
                };
                (name, arg)
            })
            .collect();
        self.eval_conformance_predicate(expr, &arguments)
    }

    fn eval_conformance_predicate(&self, expr: &Expr, args: &HashMap<&str, &TyArg>) -> bool {
        match &expr.kind {
            ExprKind::Bool(value) => *value,
            ExprKind::Prefix(PrefixOp::Not, value) => !self.eval_conformance_predicate(value, args),
            ExprKind::Infix(InfixOp::And, left, right) => {
                self.eval_conformance_predicate(left, args)
                    && self.eval_conformance_predicate(right, args)
            }
            ExprKind::Infix(InfixOp::Or, left, right) => {
                self.eval_conformance_predicate(left, args)
                    || self.eval_conformance_predicate(right, args)
            }
            ExprKind::Infix(op, left, right)
                if matches!(
                    op,
                    InfixOp::Eq
                        | InfixOp::Ne
                        | InfixOp::Lt
                        | InfixOp::Le
                        | InfixOp::Gt
                        | InfixOp::Ge
                ) =>
            {
                let Some(left) = conformance_operand(left, args) else {
                    return false;
                };
                let Some(right) = conformance_operand(right, args) else {
                    return false;
                };
                compare_ct_integers(*op, &left, &right).unwrap_or_else(|| match op {
                    InfixOp::Eq => ct_values_equal(&left, &right),
                    InfixOp::Ne => !ct_values_equal(&left, &right),
                    _ => false,
                })
            }
            ExprKind::Call {
                name,
                args: operands,
                kwargs,
                ..
            } if name == "conforms_to" && kwargs.is_empty() && operands.len() == 2 => {
                let ExprKind::Identifier(type_name) = &operands[0].kind else {
                    return false;
                };
                let ExprKind::Identifier(trait_name) = &operands[1].kind else {
                    return false;
                };
                matches!(args.get(type_name.as_str()), Some(TyArg::Ty(ty)) if self.conforms_to(ty, trait_name))
            }
            _ => false,
        }
    }

    fn trait_refines(&self, candidate: &str, required: &str) -> bool {
        self.trait_refines_inner(candidate, required, &mut HashSet::new())
    }

    fn trait_refines_inner(
        &self,
        candidate: &str,
        required: &str,
        visiting: &mut HashSet<String>,
    ) -> bool {
        if !visiting.insert(candidate.to_string()) {
            return false;
        }
        self.traits.get(candidate).is_some_and(|info| {
            info.refines.iter().any(|parent| {
                parent == required || self.trait_refines_inner(parent, required, visiting)
            })
        })
    }

    /// Explain the first actionable reason a built-in bound failed. This is
    /// intentionally evidence-oriented: marker traits name the field that
    /// prevents fieldwise synthesis, while operation traits name the operation
    /// promised by the bound.
    fn trait_failure_reason(&self, ty: &Ty, tr: &str) -> Option<String> {
        let Ty::Struct(name, _) = ty else {
            return builtin_trait_operation(tr)
                .map(|operation| format!("missing required operation '{operation}'"));
        };
        let info = self.structs.get(name)?;
        let field_failure = |predicate: &dyn Fn(&Ty) -> bool| {
            info.fields
                .iter()
                .find(|(_, field_ty)| !predicate(field_ty))
                .map(|(field, field_ty)| {
                    format!("field '{field}' has type '{field_ty}', which is not {tr}")
                })
        };
        match tr {
            "Copyable" => field_failure(&|field_ty| self.is_copyable(field_ty)),
            "ImplicitlyCopyable" => {
                if info.methods.contains_key("__copyinit__") {
                    Some(
                        "defines '__copyinit__'; implicit copying requires fieldwise synthesis"
                            .to_string(),
                    )
                } else {
                    field_failure(&|field_ty| self.is_implicitly_copyable(field_ty))
                }
            }
            "ImplicitlyDeletable" => {
                field_failure(&|field_ty| self.is_implicitly_deletable(field_ty))
            }
            _ => builtin_trait_operation(tr)
                .map(|operation| format!("missing required operation '{operation}'")),
        }
    }

    /// Whether a value of this type may be **copied** (implicitly duplicated).
    /// Mojo is move-only by default: scalars and the built-in value types are
    /// Copyable, but a `struct` is Copyable only if it declares Copyable/
    /// ImplicitlyCopyable conformance **or defines `__copyinit__`**, and a type
    /// parameter only if bounded by Copyable/ImplicitlyCopyable.
    fn is_copyable(&self, ty: &Ty) -> bool {
        if self.has_assumed_conformance(ty, "Copyable")
            || self.has_assumed_conformance(ty, "ImplicitlyCopyable")
        {
            return true;
        }
        match ty {
            Ty::ComptimeList(element) => self.is_copyable(element),
            Ty::Tuple(elements) | Ty::RuntimePack(elements) => {
                elements.iter().all(|element| self.is_copyable(element))
            }
            Ty::Variant(alternatives) => alternatives
                .iter()
                .all(|alternative| self.is_copyable(alternative)),
            Ty::Struct(name, _) => self
                .structs
                .get(name)
                .map(|s| {
                    s.conforms
                        .iter()
                        .any(|c| matches!(c.as_str(), "Copyable" | "ImplicitlyCopyable"))
                        || s.methods.contains_key("__copyinit__")
                })
                .unwrap_or(true),
            Ty::Param { bounds, .. } => bounds
                .iter()
                .any(|b| matches!(b.as_str(), "Copyable" | "ImplicitlyCopyable")),
            // Scalars, `String`, `List`/`Tuple`/`Simd`/`Range`, `Error`, closures,
            // and `Self` are treated as copyable (element-wise copyability of
            // aggregates is not modeled).
            _ => true,
        }
    }

    /// An ordinary value use of a reference-returning expression owns an
    /// independent value: MIR lowers it to `ReadRef` followed by `CopyValue`.
    /// Reference-bearing contexts are recorded in `reference_value_uses` and
    /// retain the handle instead. Validate the remaining reads after the whole
    /// tree has been checked so a later enclosing context can mark a nested
    /// result as a retained handle before this distinction is enforced.
    fn check_reference_result_reads(&self) -> Result<(), TypeError> {
        let operations = self.operation_adjustments.borrow();
        let retained_handles = self.reference_value_uses.borrow();
        let copyable_reads = self.copyable_reference_result_reads.borrow();
        for (span, adjustment) in operations.iter() {
            let crate::checked::SemanticAdjustment::ReferenceResult { reference } = adjustment
            else {
                continue;
            };
            if retained_handles.contains_key(span)
                || copyable_reads.contains(span)
                || self.is_copyable(&reference.referent)
            {
                continue;
            }
            return Err(TypeError::NonCopyable {
                ty: reference.referent.to_string(),
                context: "ordinary value read through a reference result".to_string(),
            });
        }
        Ok(())
    }

    /// Compiler-generated keyword collectors use the bundled self-hosted
    /// `StringDict`. Its current `List`/`DictEntry` implementation is copy-based,
    /// so accept only element types that it can store without duplicating a
    /// linear value. Reference-bearing values also wait for collector origin
    /// metadata instead of silently losing their loans at the call boundary.
    fn kwargs_collector_ty(&self, element: Ty, context: &str) -> Result<Ty, TypeError> {
        if !self.is_copyable(&element) {
            return Err(TypeError::TraitNotSatisfied {
                param: "V".to_string(),
                ty: element.to_string(),
                trait_name: "Copyable".to_string(),
                reason: Some(format!(
                    "{context} is materialized as StringDict[V], whose current storage is copy-based"
                )),
            });
        }
        if self.type_contains_reference(&element) {
            return Err(TypeError::Unsupported(format!(
                "{context} cannot contain references until keyword collectors carry origin metadata"
            )));
        }
        Ok(Ty::Struct(
            "StringDict".to_string(),
            vec![TyArg::Ty(element)],
        ))
    }

    /// `ImplicitlyCopyable` is stronger than `Copyable`: it means the type can be
    /// copied by the ordinary implicit copy path, not only by an explicit custom
    /// copy constructor. Structs opt in by declaring the marker, and fieldwise
    /// conformance requires all fields to be implicitly copyable.
    fn is_implicitly_copyable(&self, ty: &Ty) -> bool {
        if self.has_assumed_conformance(ty, "ImplicitlyCopyable") {
            return true;
        }
        match ty {
            Ty::ComptimeList(element) => self.is_implicitly_copyable(element),
            Ty::Tuple(elements) | Ty::RuntimePack(elements) => elements
                .iter()
                .all(|element| self.is_implicitly_copyable(element)),
            Ty::Variant(alternatives) => alternatives
                .iter()
                .all(|alternative| self.is_implicitly_copyable(alternative)),
            Ty::Struct(name, _) => self.structs.get(name).is_some_and(|s| {
                s.conforms.iter().any(|c| c == "ImplicitlyCopyable")
                    && self.struct_implicitly_copyable_conformance_ok(name)
            }),
            Ty::Param { bounds, .. } => bounds.iter().any(|b| b == "ImplicitlyCopyable"),
            _ => true,
        }
    }

    fn is_movable(&self, _ty: &Ty) -> bool {
        // The current ownership model supports moving every initialized value.
        true
    }

    fn is_implicitly_deletable(&self, ty: &Ty) -> bool {
        if self.has_assumed_conformance(ty, "ImplicitlyDeletable") {
            return true;
        }
        match ty {
            Ty::ComptimeList(element) => self.is_implicitly_deletable(element),
            Ty::Tuple(elements) | Ty::RuntimePack(elements) => elements
                .iter()
                .all(|element| self.is_implicitly_deletable(element)),
            Ty::Variant(alternatives) => alternatives
                .iter()
                .all(|alternative| self.is_implicitly_deletable(alternative)),
            Ty::Struct(name, args) => self.structs.get(name).is_none_or(|info| {
                if info.conforms.iter().any(|tr| tr == "ImplicitlyDeletable") {
                    self.struct_conformance_applies(name, args, "ImplicitlyDeletable")
                } else {
                    true
                }
            }),
            Ty::Param { bounds, .. } => bounds.iter().any(|b| b == "ImplicitlyDeletable"),
            _ => true,
        }
    }

    fn is_hashable(&self, ty: &Ty) -> bool {
        if self.has_assumed_conformance(ty, "Hashable") {
            return true;
        }
        match ty {
            Ty::Variant(alternatives) => alternatives
                .iter()
                .all(|alternative| self.is_hashable(alternative)),
            Ty::Struct(name, _) => self.structs.get(name).is_some_and(|s| {
                s.conforms.iter().any(|c| c == "Hashable") || s.methods.contains_key("__hash__")
            }),
            Ty::Param { bounds, .. } => bounds.iter().any(|b| b == "Hashable"),
            _ => builtin_hashable_ty(ty),
        }
    }

    fn is_comparable(&self, ty: &Ty) -> bool {
        if self.has_assumed_conformance(ty, "Comparable") {
            return true;
        }
        // The discovery check runs before variadic public-Tuple templates have
        // been replaced by concrete generated declarations. Preserve the
        // template's conditional Comparable contract structurally across that
        // staging seam; the final specialization carries the same conformance
        // as an ordinary nominal declaration.
        if let Some(elements) = tuple_elements(ty) {
            return elements
                .into_iter()
                .all(|element| self.is_comparable(element));
        }
        match ty {
            Ty::Struct(name, args) => self.struct_conformance_applies(name, args, "Comparable"),
            Ty::Param { bounds, .. } => bounds.iter().any(|b| b == "Comparable"),
            _ => is_numeric_like(ty),
        }
    }

    fn struct_copyable_conformance_ok(&self, name: &str) -> bool {
        let Some(info) = self.structs.get(name) else {
            return false;
        };
        info.methods.contains_key("__copyinit__")
            || info.fields.iter().all(|(_, ty)| self.is_copyable(ty))
    }

    fn struct_implicitly_copyable_conformance_ok(&self, name: &str) -> bool {
        let Some(info) = self.structs.get(name) else {
            return false;
        };
        !info.methods.contains_key("__copyinit__")
            && info
                .fields
                .iter()
                .all(|(_, ty)| self.is_implicitly_copyable(ty))
    }

    /// At a **consuming** position (binding a value to a new place, passing it by
    /// value, returning it, …): a non-Copyable value that is a *place* (names an
    /// existing binding) is being copied — reject it unless it was transferred with
    /// `^` (which is a move, not a place). `context` names the site for the error.
    fn check_consuming(&self, expr: &Expr, ty: &Ty, context: &str) -> Result<(), TypeError> {
        // A `^` transfer is `Expr::Transfer`, not a place, so it is naturally
        // exempt. A fresh temporary (a call result, a literal, an operator) is not a
        // place either — moving it is free.
        if is_place_expr(expr) {
            if !self.is_copyable(ty) {
                return Err(TypeError::NonCopyable {
                    ty: ty.to_string(),
                    context: context.to_string(),
                });
            }
            self.copy_place_value_uses
                .borrow_mut()
                .insert(expr.source_span());
        }
        Ok(())
    }

    /// Bind `name` in the innermost scope. Repeated function declarations form an
    /// overload set when their call shapes differ; other same-scope repeats remain
    /// redeclarations.
    fn declare(&mut self, name: &str, ty: Ty) -> Result<(), TypeError> {
        self.declare_with_mutability(name, ty, true)
    }

    fn declare_immutable(&mut self, name: &str, ty: Ty) -> Result<(), TypeError> {
        self.declare_with_mutability(name, ty, false)
    }

    fn declare_with_mutability(
        &mut self,
        name: &str,
        ty: Ty,
        mutable: bool,
    ) -> Result<(), TypeError> {
        let nested_scope = self.scopes.len() > 1;
        let scope = self.scopes.last_mut().ok_or_else(|| {
            TypeError::InvariantViolation("checker scope stack is empty".to_string())
        })?;
        if let Some(existing) = scope.get_mut(name) {
            if let Some(mut candidates) = overload_candidates(existing, &ty) {
                if nested_scope {
                    return Err(TypeError::Unsupported("overloaded nested def".to_string()));
                }
                if candidates
                    .iter()
                    .any(|candidate| same_callable_signature(candidate, &ty))
                {
                    return Err(TypeError::Redeclaration(name.to_string()));
                }
                candidates.push(ty);
                *existing = Ty::Overload(candidates);
                return Ok(());
            }
            return Err(TypeError::Redeclaration(name.to_string()));
        }
        scope.insert(name.to_string(), ty);
        self.mutable_scopes
            .last_mut()
            .ok_or_else(|| {
                TypeError::InvariantViolation("checker mutability scope stack is empty".to_string())
            })?
            .insert(name.to_string(), mutable);
        let owner = crate::origin::OwnerId(self.next_owner);
        self.next_owner = self.next_owner.checked_add(1).ok_or_else(|| {
            TypeError::InvariantViolation("checker exhausted binding identities".to_string())
        })?;
        self.owner_scopes
            .last_mut()
            .ok_or_else(|| {
                TypeError::InvariantViolation("checker owner scope stack is empty".to_string())
            })?
            .insert(name.to_string(), owner);
        Ok(())
    }

    fn declare_function_implicit(&mut self, name: &str, ty: Ty) -> Result<(), TypeError> {
        let scope_index = self
            .function_bases
            .last()
            .copied()
            .unwrap_or(self.scopes.len().saturating_sub(1));
        if self.scopes[scope_index].contains_key(name) {
            return Err(TypeError::Redeclaration(name.to_string()));
        }
        self.scopes[scope_index].insert(name.to_string(), ty);
        self.mutable_scopes[scope_index].insert(name.to_string(), true);
        let owner = crate::origin::OwnerId(self.next_owner);
        self.next_owner = self.next_owner.checked_add(1).ok_or_else(|| {
            TypeError::InvariantViolation("checker exhausted binding identities".to_string())
        })?;
        self.owner_scopes[scope_index].insert(name.to_string(), owner);
        Ok(())
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
        self.mutable_scopes.push(HashMap::new());
        self.owner_scopes.push(HashMap::new());
        self.aggregate_origin_scopes.push(HashMap::new());
        self.aggregate_field_origin_scopes.push(HashMap::new());
        self.reference_parameter_scopes.push(HashMap::new());
        self.callable_origin_scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        if let Some(owners) = self.owner_scopes.last() {
            let ids: HashSet<_> = owners.values().copied().collect();
            self.uninitialized
                .borrow_mut()
                .retain(|owner| !ids.contains(owner));
        }
        self.scopes.pop();
        self.mutable_scopes.pop();
        self.owner_scopes.pop();
        self.aggregate_origin_scopes.pop();
        self.aggregate_field_origin_scopes.pop();
        self.reference_parameter_scopes.pop();
        self.callable_origin_scopes.pop();
    }

    fn is_binding_mutable(&self, name: &str) -> bool {
        self.mutable_scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
            .unwrap_or(true)
    }

    /// The type to declare for an **inferred** binding — `var x = e` (no
    /// annotation) or a var-less `x = e`. A numeric literal materializes to its
    /// default kind (`default_literal`); a value that cannot live in a named
    /// binding is rejected: a closure (`ClosureEscape`, matching `return`/reassign)
    /// or another value outside the source language's first-class surface.
    fn inferred_binding_ty(&self, value_ty: &Ty, _name: &str) -> Result<Ty, TypeError> {
        match value_ty {
            Ty::Func { .. } | Ty::GenericFunc { .. } | Ty::Overload(_) => {
                Err(TypeError::ClosureEscape)
            }
            other => Ok(default_literal(other)),
        }
    }

    /// Look up `name`, walking outward through the scope chain (lexical lookup).
    fn lookup(&self, name: &str) -> Option<&Ty> {
        self.scopes.iter().rev().find_map(|scope| scope.get(name))
    }

    fn register_callable_origins(&mut self, name: &str, signature: CallableOriginSignature) {
        self.callable_origin_scopes
            .last_mut()
            .expect("checker origin-callable scope stack is not empty")
            .entry(name.to_string())
            .or_default()
            .push(signature);
    }

    fn lookup_callable_origins(&self, name: &str) -> Option<Vec<CallableOriginSignature>> {
        self.callable_origin_scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
    }

    fn binding_scope(&self, name: &str) -> Option<usize> {
        self.scopes
            .iter()
            .rposition(|scope| scope.contains_key(name))
    }

    fn check_capture_access(&self, name: &str, writing: bool) -> Result<(), TypeError> {
        let contexts = self.capture_contexts.borrow();
        if contexts.is_empty() {
            return Ok(());
        }
        let Some(scope) = self.binding_scope(name) else {
            return Ok(());
        };
        // Module globals are not captures. For a value crossing more than one
        // nested-function boundary, every intervening environment must forward
        // it: an inner `{value}` cannot tunnel through a middle `{}`.
        if scope == 0 {
            return Ok(());
        }
        for policy in contexts
            .iter()
            .filter(|policy| scope < policy.base && name != policy.function_name)
        {
            let kind = policy.entries.get(name).copied().or(policy.default);
            if let Some(kind) = kind {
                self.check_capture_capability(name, kind)?;
                let binding = self.lookup_owner(name).ok_or_else(|| {
                    TypeError::InvariantViolation(format!(
                        "capture '{name}' lost its checked binding"
                    ))
                })?;
                let ty = self.lookup(name).cloned().ok_or_else(|| {
                    TypeError::InvariantViolation(format!(
                        "capture '{name}' lost its checked storage type"
                    ))
                })?;
                let checked = self.checked_capture(name, binding, ty, kind);
                let mut declarations = self.declaration_captures.borrow_mut();
                let captures = declarations.entry(policy.declaration.clone()).or_default();
                if !captures.iter().any(|capture| capture.binding == binding) {
                    captures.push(checked);
                }
            }
            match (kind, writing) {
                (Some(crate::ast::CaptureKind::Mut), _) if self.is_binding_mutable(name) => {}
                (Some(crate::ast::CaptureKind::Ref), false) => {}
                (Some(crate::ast::CaptureKind::Ref), true) if self.is_binding_mutable(name) => {}
                // A transferred capture is owned by the closure and retains
                // mutable state across calls. A copied capture is an immutable
                // snapshot in current Mojo.
                (Some(crate::ast::CaptureKind::Move), _) => {}
                (Some(crate::ast::CaptureKind::Copy | crate::ast::CaptureKind::Read), false) => {}
                (Some(_), true) | (Some(crate::ast::CaptureKind::Mut), false) => {
                    return Err(TypeError::ImmutableBinding(name.to_string()));
                }
                (None, _) => {
                    return Err(TypeError::Unsupported(format!(
                        "nested function must explicitly capture '{name}' with {{...}}"
                    )));
                }
            }
        }
        Ok(())
    }

    /// Record a name-based call as a capture when an enclosing policy permits
    /// it, without changing the historical rule that synthesized sibling calls
    /// may be reconstructed when no capture policy names them.
    fn record_permitted_call_capture(&self, name: &str) {
        let contexts = self.capture_contexts.borrow();
        let Some(scope) = self.binding_scope(name) else {
            return;
        };
        if scope == 0 {
            return;
        }
        let Some(binding) = self.lookup_owner(name) else {
            return;
        };
        let Some(ty) = self.lookup(name).cloned() else {
            return;
        };
        for policy in contexts
            .iter()
            .filter(|policy| scope < policy.base && name != policy.function_name)
        {
            let Some(kind) = policy.entries.get(name).copied().or(policy.default) else {
                continue;
            };
            let checked = self.checked_capture(name, binding, ty.clone(), kind);
            let mut declarations = self.declaration_captures.borrow_mut();
            let captures = declarations.entry(policy.declaration.clone()).or_default();
            if !captures.iter().any(|capture| capture.binding == binding) {
                captures.push(checked);
            }
        }
    }

    fn check_capture_capability(
        &self,
        name: &str,
        kind: crate::ast::CaptureKind,
    ) -> Result<(), TypeError> {
        let Some(ty) = self.lookup(name) else {
            return Ok(());
        };
        let missing = match kind {
            // `{var value}` performs an implicit copy at the nested-function
            // declaration. A merely explicitly Copyable value is therefore not
            // enough; current Mojo requires the stronger marker here.
            crate::ast::CaptureKind::Copy if !self.is_implicitly_copyable(ty) => {
                Some("ImplicitlyCopyable")
            }
            crate::ast::CaptureKind::Move if !self.is_movable(ty) => Some("Movable"),
            _ => None,
        };
        if let Some(trait_name) = missing {
            return Err(TypeError::TraitNotSatisfied {
                param: name.to_string(),
                ty: ty.to_string(),
                trait_name: trait_name.to_string(),
                reason: Some(format!(
                    "capture convention for '{name}' requires {trait_name}"
                )),
            });
        }
        Ok(())
    }

    fn lookup_owner(&self, name: &str) -> Option<crate::origin::OwnerId> {
        self.owner_scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
    }

    fn record_statement_binding(&self, statement: &Stmt, name: &str) {
        if let Some(owner) = self.lookup_owner(name) {
            self.statement_bindings
                .borrow_mut()
                .insert(statement.source_span(), owner);
        }
    }

    fn owner_is_mutable(&self, owner: crate::origin::OwnerId) -> bool {
        self.owner_scopes
            .iter()
            .zip(&self.mutable_scopes)
            .rev()
            .find_map(|(owners, mutability)| {
                owners
                    .iter()
                    .find(|(_, id)| **id == owner)
                    .and_then(|(name, _)| mutability.get(name).copied())
            })
            .unwrap_or(false)
    }

    /// Convert a source place into the stable, projection-sensitive identity
    /// used by checked origins. Index values are intentionally abstracted: the
    /// loan checker must conservatively treat arbitrary indices as overlapping.
    fn origin_place(&self, expr: &Expr) -> Result<crate::origin::OriginPlace, TypeError> {
        use crate::origin::{OriginPlace, OriginSeg};
        if let Some(interior) = self
            .interior_references
            .borrow()
            .get(&expr.source_span())
            .cloned()
        {
            return Ok(interior);
        }
        match &expr.kind {
            ExprKind::Identifier(name) => {
                if let Some(Ty::Ref(reference)) = self.lookup(name)
                    && let crate::origin::Origin::Place(place) = &reference.origin
                {
                    return Ok(place.clone());
                }
                let root = self
                    .lookup_owner(name)
                    .ok_or_else(|| TypeError::UndefinedVariable(name.clone()))?;
                Ok(OriginPlace {
                    root,
                    path: Vec::new(),
                })
            }
            ExprKind::Member { object, field } => {
                if matches!(self.place_storage_ty(expr), Some(Ty::Ref(_))) {
                    fn collect_places(
                        origin: crate::origin::Origin,
                        places: &mut Vec<OriginPlace>,
                    ) {
                        match origin {
                            crate::origin::Origin::Place(place) => places.push(place),
                            crate::origin::Origin::Union(members) => {
                                for member in members {
                                    collect_places(member, places);
                                }
                            }
                            crate::origin::Origin::Param(_)
                            | crate::origin::Origin::Static
                            | crate::origin::Origin::Untracked { .. } => {}
                        }
                    }

                    let mut referents = Vec::new();
                    for origin in self.aggregate_origins(expr) {
                        collect_places(origin, &mut referents);
                    }
                    referents.sort();
                    referents.dedup();
                    if let [referent] = referents.as_slice() {
                        return Ok(referent.clone());
                    }
                }
                let mut place = self.origin_place(object)?;
                place.path.push(OriginSeg::Field(field.clone()));
                Ok(place)
            }
            ExprKind::Index { object, .. } => {
                let mut place = self.origin_place(object)?;
                place.path.push(OriginSeg::AnyIndex);
                Ok(place)
            }
            ExprKind::TypeApply { name, .. }
                if self
                    .operation_adjustments
                    .borrow()
                    .get(&expr.source_span())
                    .is_some_and(|operation| {
                        matches!(
                            operation,
                            crate::checked::SemanticAdjustment::VariantProject { .. }
                        )
                    }) =>
            {
                let root = self
                    .lookup_owner(name)
                    .ok_or_else(|| TypeError::UndefinedVariable(name.clone()))?;
                // `record_interior_reference` adds the payload's named
                // `Interior("value")` segment after this base is resolved.
                Ok(OriginPlace {
                    root,
                    path: Vec::new(),
                })
            }
            _ => Err(TypeError::Unsupported(
                "reference binding to a non-place expression".to_string(),
            )),
        }
    }

    /// Resolve the reference capability actually supplied by an expression.
    /// A reference-valued identifier/field can carry a union of concrete
    /// referents; collapsing it to the handle slot would lose both escape and
    /// interior-generation facts when the handle is forwarded through another
    /// call. Plain places synthesize the corresponding place reference.
    fn reference_actual(&self, expr: &Expr) -> Result<crate::origin::RefTy, TypeError> {
        use crate::origin::{Mutability, Origin, RefTy};

        if let Some(mut reference) = self.infer_reference_value(expr) {
            let retained = self.aggregate_origins(expr);
            if !retained.is_empty() {
                reference.origin = Origin::union(retained);
            }
            return Ok(reference);
        }

        let place = self.origin_place(expr)?;
        let mutability = if self.owner_is_mutable(place.root) {
            Mutability::Mutable
        } else {
            Mutability::Immutable
        };
        Ok(RefTy {
            referent: Box::new(self.infer(expr)?),
            origin: Origin::Place(place),
            mutability,
        })
    }

    /// Resolve the compile-time value accepted by an `Origin` parameter at a
    /// function-value specialization site. `origin_of` observes checked places
    /// (including reference-valued places) and never evaluates at runtime.
    fn explicit_origin_argument(
        &self,
        argument: &crate::ast::ParamArg,
    ) -> Result<crate::origin::Origin, TypeError> {
        use crate::ast::ParamArg;
        use crate::origin::Origin;

        let expression = match argument {
            ParamArg::Value(expression) => expression,
            ParamArg::Named { value, .. } => return self.explicit_origin_argument(value),
            ParamArg::Type(_) => {
                return Err(TypeError::TypeMismatch {
                    expected: "an Origin value".to_string(),
                    found: "a type".to_string(),
                    context: "explicit callable origin specialization".to_string(),
                });
            }
        };
        match &expression.kind {
            ExprKind::Call {
                name,
                args,
                kwargs,
                param_args,
            } if name == "origin_of" && kwargs.is_empty() && param_args.is_empty() => {
                if args.is_empty() {
                    return Err(TypeError::Unsupported(
                        "origin_of requires at least one place".to_string(),
                    ));
                }
                args.iter()
                    .map(|place| {
                        self.reference_actual(place)
                            .map(|reference| reference.origin)
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map(Origin::union)
            }
            ExprKind::Identifier(name) if name == "StaticOrigin" => Ok(Origin::Static),
            ExprKind::Identifier(name) if name == "UntrackedOrigin" => {
                Ok(Origin::Untracked { mutable: false })
            }
            ExprKind::Identifier(name) if name == "UnsafeAnyOrigin" => {
                Ok(Origin::Untracked { mutable: true })
            }
            _ => Err(TypeError::TypeMismatch {
                expected: "origin_of(place) or a builtin Origin value".to_string(),
                found: "a runtime value".to_string(),
                context: "explicit callable origin specialization".to_string(),
            }),
        }
    }

    /// Split the source parameter list at an explicit specialization site.
    /// Ordinary arguments are rewritten as named arguments before being handed
    /// to the generic binder; this preserves their source slot even when an
    /// erased or infer-only semantic parameter precedes them.
    fn split_callable_specialization(
        &self,
        name: &str,
        arguments: &[crate::ast::ParamArg],
        signature: &CallableOriginSignature,
    ) -> Result<SplitCallableSpecialization, TypeError> {
        use crate::ast::ParamArg;

        if signature.origins.is_empty() {
            return Ok((arguments.to_vec(), Vec::new()));
        }
        let mut supplied = vec![false; signature.source.len()];
        let mut origins = vec![None; signature.origins.len()];
        let mut ordinary = Vec::new();
        let mut next_positional = 0;
        for argument in arguments {
            let (index, value) = match argument {
                ParamArg::Named {
                    name: argument_name,
                    value,
                } => {
                    let index = signature
                        .source
                        .iter()
                        .position(|parameter| parameter.name == *argument_name)
                        .ok_or_else(|| TypeError::BadCall {
                            func: name.to_string(),
                            reason: format!("unknown compile-time parameter '{argument_name}'"),
                        })?;
                    (index, (**value).clone())
                }
                other => {
                    while next_positional < signature.source.len()
                        && (signature.source[next_positional].infer_only
                            || supplied[next_positional])
                    {
                        next_positional += 1;
                    }
                    if next_positional == signature.source.len() {
                        return Err(TypeError::WrongTypeArgCount {
                            name: name.to_string(),
                            expected: signature
                                .source
                                .iter()
                                .filter(|parameter| !parameter.infer_only)
                                .count(),
                            got: arguments.len(),
                        });
                    }
                    let index = next_positional;
                    next_positional += 1;
                    (index, other.clone())
                }
            };
            let parameter = &signature.source[index];
            if parameter.infer_only {
                return Err(TypeError::Unsupported(format!(
                    "infer-only parameter '{}' cannot be supplied explicitly",
                    parameter.name
                )));
            }
            if supplied[index] {
                return Err(TypeError::BadCall {
                    func: name.to_string(),
                    reason: format!("parameter '{}' was supplied twice", parameter.name),
                });
            }
            supplied[index] = true;
            if let Some(origin_index) = parameter.origin {
                if let ParamArg::Value(expression) = &value {
                    self.operation_adjustments.borrow_mut().insert(
                        expression.source_span(),
                        crate::checked::SemanticAdjustment::EraseCompileTimeArgument,
                    );
                }
                origins[origin_index] = Some(self.explicit_origin_argument(&value)?);
            } else if parameter.ordinary {
                ordinary.push(ParamArg::Named {
                    name: parameter.name.trim_start_matches('*').to_string(),
                    value: Box::new(value),
                });
            } else {
                return Err(TypeError::Unsupported(format!(
                    "semantic parameter '{}' is inferred and cannot be supplied explicitly",
                    parameter.name
                )));
            }
        }

        let bindings = signature
            .origins
            .iter()
            .zip(origins)
            .filter_map(|(parameter, origin)| {
                origin.map(|origin| (parameter.slots.clone(), origin))
            })
            .collect::<Vec<_>>();
        Ok((ordinary, bindings))
    }

    fn bind_callable_origins(
        &self,
        mut callable: Ty,
        bindings: &[(Vec<usize>, crate::origin::Origin)],
    ) -> Ty {
        let (ref_params, ref_return) = match &mut callable {
            Ty::Func {
                ref_params,
                ref_return,
                ..
            }
            | Ty::GenericFunc {
                ref_params,
                ref_return,
                ..
            } => (ref_params, ref_return),
            _ => return callable,
        };
        for signature in ref_params.iter_mut().flatten() {
            signature.origin = bind_sig_origin(&signature.origin, bindings);
        }
        if let Some(signature) = ref_return {
            signature.origin = bind_sig_origin(&signature.origin, bindings);
        }
        callable
    }

    fn prepare_callable_specialization(
        &self,
        name: &str,
        arguments: &[crate::ast::ParamArg],
        callable: Ty,
        signature: Option<&CallableOriginSignature>,
    ) -> Result<(Ty, Vec<crate::ast::ParamArg>), TypeError> {
        let Some(signature) = signature else {
            return Ok((callable, arguments.to_vec()));
        };
        let (ordinary, bindings) =
            self.split_callable_specialization(name, arguments, signature)?;
        Ok((self.bind_callable_origins(callable, &bindings), ordinary))
    }

    /// Materialize the monomorphic checked view of an explicitly specialized
    /// generic function value. Generic execution remains type-erased; only its
    /// callable contract is instantiated here.
    fn instantiate_generic_callable_value(
        &self,
        name: &str,
        callable: Ty,
        arguments: &[crate::ast::ParamArg],
    ) -> Result<(Ty, Vec<TyArg>), TypeError> {
        let Ty::GenericFunc {
            environment,
            decls,
            params,
            names,
            ret,
            required,
            variadic,
            kw_variadic,
            positional_only,
            keyword_only,
            raises,
            error,
            conventions,
            ref_params,
            ref_return,
        } = callable
        else {
            return Ok((callable, Vec::new()));
        };
        let (subst, tyargs) = self.resolve_use_params(name, &decls, arguments, &[], &[])?;
        let values = Self::value_argument_environment(&decls, &tyargs);
        let resolve = |ty: &Ty| {
            let substituted = self.resolve_assoc_ty(&substitute(ty, &subst));
            self.resolve_dependent_ty(&substituted, &values)
        };
        let contract = Ty::Func {
            environment,
            params: params.iter().map(resolve).collect::<Result<Vec<_>, _>>()?,
            names,
            ret: Box::new(resolve(&ret)?),
            required,
            variadic: variadic
                .as_ref()
                .map(|parameter| resolve(parameter).map(Box::new))
                .transpose()?,
            kw_variadic: kw_variadic
                .as_ref()
                .map(|parameter| resolve(parameter).map(Box::new))
                .transpose()?,
            positional_only,
            keyword_only,
            raises,
            error: error
                .as_ref()
                .map(|error| resolve(error).map(Box::new))
                .transpose()?,
            conventions,
            ref_params,
            ref_return,
        };
        Ok((contract, tyargs))
    }

    fn specialize_callable_value_candidate(
        &self,
        name: &str,
        arguments: &[crate::ast::ParamArg],
        callable: Ty,
        signature: Option<&CallableOriginSignature>,
    ) -> Result<Ty, TypeError> {
        let (callable, ordinary) =
            self.prepare_callable_specialization(name, arguments, callable, signature)?;
        match callable {
            callable @ Ty::GenericFunc { .. } => self
                .instantiate_generic_callable_value(name, callable, &ordinary)
                .map(|(contract, _)| contract),
            callable @ Ty::Func { .. } if ordinary.is_empty() => Ok(callable),
            Ty::Func { .. } => Err(TypeError::WrongTypeArgCount {
                name: name.to_string(),
                expected: 0,
                got: ordinary.len(),
            }),
            other => Err(TypeError::NotCallable {
                name: name.to_string(),
                ty: other.to_string(),
            }),
        }
    }

    fn infer_specialized_callable_value(
        &self,
        span: SourceSpan,
        name: &str,
        arguments: &[crate::ast::ParamArg],
        expected: Option<&Ty>,
        record: bool,
    ) -> Result<Option<Ty>, TypeError> {
        let Some(callable) = self.lookup(name).cloned() else {
            return Ok(None);
        };
        if !matches!(
            callable,
            Ty::Func { .. } | Ty::GenericFunc { .. } | Ty::Overload(_)
        ) {
            return Ok(None);
        }
        self.check_capture_access(name, false)?;
        if record && let Some(owner) = self.lookup_owner(name) {
            self.expression_bindings
                .borrow_mut()
                .insert(span.clone(), owner);
        }
        let signatures = self.lookup_callable_origins(name).unwrap_or_default();
        let (selected, target) = match callable {
            Ty::Overload(candidates) => {
                let expected = expected.ok_or_else(|| TypeError::BadCall {
                    func: name.to_string(),
                    reason: "an overloaded function value requires a contextual callable type"
                        .to_string(),
                })?;
                let mut matches = candidates
                    .iter()
                    .enumerate()
                    .filter_map(|(index, candidate)| {
                        let specialized = self
                            .specialize_callable_value_candidate(
                                name,
                                arguments,
                                candidate.clone(),
                                signatures.get(index),
                            )
                            .ok()?;
                        self.value_coerces(&specialized, expected)
                            .then(|| {
                                callable_lowered_name(name, candidate)
                                    .map(|target| (specialized, target))
                            })
                            .flatten()
                    })
                    .collect::<Vec<_>>();
                match matches.len() {
                    0 => {
                        return Err(TypeError::TypeMismatch {
                            expected: expected.to_string(),
                            found: format!("specialization of overload({name})"),
                            context: "overloaded callable value".to_string(),
                        });
                    }
                    1 => matches.pop().expect("one callable-value candidate"),
                    _ => {
                        return Err(TypeError::BadCall {
                            func: name.to_string(),
                            reason: format!(
                                "multiple specialized overloads fit expected type '{expected}'"
                            ),
                        });
                    }
                }
            }
            candidate => {
                let specialized = self.specialize_callable_value_candidate(
                    name,
                    arguments,
                    candidate,
                    signatures.first(),
                )?;
                (specialized, name.to_string())
            }
        };
        if record {
            self.overload_targets
                .borrow_mut()
                .insert(span.clone(), target);
            self.expression_types
                .borrow_mut()
                .insert(span.clone(), selected.clone());
        }
        Ok(Some(selected))
    }

    /// Record that evaluating `expression` as a reference produces a fresh
    /// generation in the named interior region owned by `base`.
    fn record_interior_reference(&self, site: SourceSpan, base: &Expr, name: &str) {
        if self
            .interior_references
            .borrow()
            .get(&site)
            .is_some_and(|origin| {
                matches!(origin.path.last(), Some(crate::origin::OriginSeg::Interior(tag)) if tag == name)
            })
        {
            // Inference is intentionally repeatable. Do not project the fact
            // through itself when the same checked expression is revisited.
            return;
        }
        if let Ok(mut origin) = self.origin_place(base) {
            origin
                .path
                .push(crate::origin::OriginSeg::Interior(name.to_string()));
            self.interior_references.borrow_mut().insert(site, origin);
        }
    }

    /// Record Mojo's owned-interior generation refresh for a named region.
    /// Defining a new `base._get_owned_interior["name"]` origin invalidates an
    /// older generation of that same region, but not sibling regions below the
    /// owner. Dict lookup uses this for `"value"`, so a new lookup stales an
    /// earlier value reference without invalidating the `"element"` generation
    /// retained by key iteration.
    fn record_replacing_interior_reference(&self, site: SourceSpan, base: &Expr, name: &str) {
        if let Ok(mut origin) = self.origin_place(base) {
            origin
                .path
                .push(crate::origin::OriginSeg::Interior(name.to_string()));
            self.record_origin_invalidation_kind(site.clone(), origin, None, true);
        }
        self.record_interior_reference(site, base, name);
    }

    /// Record a mutation of `base`. Existing generations rooted below this
    /// path become stale. If `base` is itself a local reference, mutations
    /// through that handle preserve its own generation while still invalidating
    /// interiors nested underneath it.
    fn record_interior_invalidation(&self, site: SourceSpan, base: &Expr) {
        let Ok(origin) = self.origin_place(base) else {
            return;
        };
        let except = match &base.kind {
            ExprKind::Identifier(name) if matches!(self.lookup(name), Some(Ty::Ref(_))) => {
                self.lookup_owner(name)
            }
            _ => None,
        };
        self.record_origin_invalidation(site, origin, except);
    }

    /// Record the storage generation replaced by a checked place write. Index
    /// and Variant-payload targets are places too: replacing one preserves a
    /// reference to that exact generation, but invalidates references into
    /// interiors nested below it. Two handle-bearing place forms need their
    /// semantic referent rather than their syntactic storage path:
    ///
    /// * `pointer[0]` replaces the origin-bearing pointer's proven source place;
    /// * assigning through a reference-valued aggregate field replaces the
    ///   place(s) whose handles the aggregate retains.
    fn record_place_write_invalidation(&self, site: SourceSpan, place: &Expr) {
        if let ExprKind::Index { object, .. } = &place.kind
            && let Ok(Ty::Pointer {
                origin: crate::origin::PointerOrigin::Place { place: origin, .. },
                ..
            }) = self.infer(object)
        {
            self.record_origin_invalidation(site, origin, None);
            return;
        }

        if matches!(self.place_storage_ty(place), Some(Ty::Ref(_))) {
            // An `out self` initializer stores the incoming reference handle;
            // subsequent assignments write through that established handle.
            if self.self_initializing && place_root_name(place) == Some("self") {
                return;
            }

            let mut origins = self.aggregate_origins(place).into_iter().peekable();
            if origins.peek().is_some() {
                for origin in origins {
                    self.record_aggregate_origin_invalidation(site.clone(), origin);
                }
                return;
            }
        }

        self.record_interior_invalidation(site, place);
    }

    fn record_aggregate_origin_invalidation(
        &self,
        site: SourceSpan,
        origin: crate::origin::Origin,
    ) {
        self.record_aggregate_origin_invalidation_except(site, origin, None);
    }

    fn record_aggregate_origin_invalidation_except(
        &self,
        site: SourceSpan,
        origin: crate::origin::Origin,
        except: Option<crate::origin::OwnerId>,
    ) {
        match origin {
            crate::origin::Origin::Place(place) => {
                self.record_origin_invalidation(site, place, except);
            }
            crate::origin::Origin::Union(members) => {
                for member in members {
                    self.record_aggregate_origin_invalidation_except(site.clone(), member, except);
                }
            }
            crate::origin::Origin::Param(_)
            | crate::origin::Origin::Static
            | crate::origin::Origin::Untracked { .. } => {}
        }
    }

    fn record_origin_invalidation(
        &self,
        site: SourceSpan,
        base: crate::origin::OriginPlace,
        except: Option<crate::origin::OwnerId>,
    ) {
        self.record_origin_invalidation_kind(site, base, except, false);
    }

    fn record_origin_invalidation_kind(
        &self,
        site: SourceSpan,
        base: crate::origin::OriginPlace,
        except: Option<crate::origin::OwnerId>,
        include_base_generation: bool,
    ) {
        let fact = crate::checked::InteriorInvalidation {
            base,
            except,
            include_base_generation,
        };
        let mut invalidations = self.interior_invalidations.borrow_mut();
        let values = invalidations.entry(site).or_default();
        if !values.contains(&fact) {
            values.push(fact);
        }
    }

    fn record_owner_invalidation(
        &self,
        site: SourceSpan,
        owner: crate::origin::OwnerId,
        path: Vec<crate::origin::OriginSeg>,
    ) {
        let fact = crate::checked::InteriorInvalidation {
            base: crate::origin::OriginPlace { root: owner, path },
            except: None,
            include_base_generation: false,
        };
        let mut invalidations = self.interior_invalidations.borrow_mut();
        let values = invalidations.entry(site).or_default();
        if !values.contains(&fact) {
            values.push(fact);
        }
    }

    fn lookup_aggregate_origins(&self, name: &str) -> Vec<crate::origin::Origin> {
        self.aggregate_origin_scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
            .unwrap_or_default()
    }

    fn set_aggregate_origins(&mut self, name: &str, origins: Vec<crate::origin::Origin>) {
        let Some(scope) = self.binding_scope(name) else {
            return;
        };
        if origins.is_empty() {
            self.aggregate_origin_scopes[scope].remove(name);
        } else {
            self.aggregate_origin_scopes[scope].insert(name.to_string(), origins);
        }
    }

    fn lookup_aggregate_field_origins(
        &self,
        name: &str,
    ) -> HashMap<String, Vec<crate::origin::Origin>> {
        self.aggregate_field_origin_scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
            .unwrap_or_default()
    }

    fn set_aggregate_field_origins(
        &mut self,
        name: &str,
        fields: HashMap<String, Vec<crate::origin::Origin>>,
    ) {
        let Some(scope) = self.binding_scope(name) else {
            return;
        };
        if fields.is_empty() {
            self.aggregate_field_origin_scopes[scope].remove(name);
        } else {
            self.aggregate_field_origin_scopes[scope].insert(name.to_string(), fields);
        }
    }

    /// Origins retained by each direct field of an aggregate value. The flat
    /// aggregate origin set remains useful for lifetime extension, but it
    /// cannot identify the referent of `pair.right` when `pair` also retains a
    /// distinct `left` origin.
    fn aggregate_field_origins(
        &self,
        expression: &Expr,
    ) -> HashMap<String, Vec<crate::origin::Origin>> {
        fn append_unique(
            into: &mut Vec<crate::origin::Origin>,
            values: impl IntoIterator<Item = crate::origin::Origin>,
        ) {
            for value in values {
                if !into.contains(&value) {
                    into.push(value);
                }
            }
        }

        match &expression.kind {
            ExprKind::Identifier(name) => self.lookup_aggregate_field_origins(name),
            ExprKind::Transfer(inner) | ExprKind::Named { value: inner, .. } => {
                self.aggregate_field_origins(inner)
            }
            ExprKind::IfExpr {
                then_branch,
                else_branch,
                ..
            } => {
                let mut result = self.aggregate_field_origins(then_branch);
                for (field, origins) in self.aggregate_field_origins(else_branch) {
                    append_unique(result.entry(field).or_default(), origins);
                }
                result
            }
            ExprKind::Call {
                name, args, kwargs, ..
            } => {
                let Some(info) = self.structs.get(name) else {
                    return HashMap::new();
                };
                let fields = info.fields.clone();
                let mut result = HashMap::new();
                if info.fieldwise_init {
                    for ((field_name, field_ty), argument) in fields.into_iter().zip(args) {
                        let origins = if matches!(field_ty, Ty::Ref(_)) {
                            self.infer_reference_value(argument)
                                .map(|reference| vec![reference.origin])
                                .unwrap_or_default()
                        } else if self.type_carries_loans(&field_ty) {
                            self.aggregate_origins(argument)
                        } else {
                            Vec::new()
                        };
                        if !origins.is_empty() {
                            result.insert(field_name, origins);
                        }
                    }
                    return result;
                }

                // A conventional handwritten initializer commonly forwards a
                // same-named ref parameter into each reference field. Preserve
                // that field identity at the call site too; arbitrary computed
                // initializer data flow remains represented by the flat,
                // conservative aggregate origin set.
                let Some(signature) = info.methods.get("__init__").and_then(|signatures| {
                    signatures
                        .iter()
                        .find(|signature| signature.params.len() >= args.len())
                }) else {
                    return result;
                };
                for (field_name, field_ty) in fields {
                    if !matches!(field_ty, Ty::Ref(_)) {
                        continue;
                    }
                    let Some(index) = signature
                        .names
                        .iter()
                        .position(|parameter| parameter == &field_name)
                    else {
                        continue;
                    };
                    let argument = args.get(index).or_else(|| {
                        kwargs
                            .iter()
                            .find(|argument| argument.name == field_name)
                            .map(|argument| &argument.value)
                    });
                    if let Some(argument) = argument {
                        let origins = self
                            .reference_actual(argument)
                            .ok()
                            .map(|reference| vec![reference.origin])
                            .or_else(|| {
                                signature
                                    .ref_params
                                    .get(index)
                                    .is_some_and(Option::is_some)
                                    .then(|| {
                                        self.origin_place(argument)
                                            .ok()
                                            .map(crate::origin::Origin::Place)
                                            .into_iter()
                                            .collect::<Vec<_>>()
                                    })
                            })
                            .unwrap_or_default();
                        if !origins.is_empty() {
                            result.insert(field_name, origins);
                        }
                    }
                }
                result
            }
            _ => HashMap::new(),
        }
    }

    fn register_reference_parameter(&mut self, name: &str, referent: Ty, mutable: bool) {
        let Some(scope) = self.binding_scope(name) else {
            return;
        };
        let Some(owner) = self.lookup_owner(name) else {
            return;
        };
        self.reference_parameter_scopes[scope].insert(
            name.to_string(),
            crate::origin::RefTy {
                referent: Box::new(referent),
                origin: crate::origin::Origin::Place(crate::origin::OriginPlace {
                    root: owner,
                    path: Vec::new(),
                }),
                mutability: if mutable {
                    crate::origin::Mutability::Mutable
                } else {
                    crate::origin::Mutability::Immutable
                },
            },
        );
    }

    fn lookup_reference_parameter(&self, name: &str) -> Option<crate::origin::RefTy> {
        self.reference_parameter_scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
    }

    fn type_contains_reference(&self, ty: &Ty) -> bool {
        self.type_storage_contains(ty, false)
    }

    /// Whether storage of this type carries owner loans: reference-valued
    /// leaves, or pointers whose provenance designates checked storage.
    fn type_carries_loans(&self, ty: &Ty) -> bool {
        self.type_storage_contains(ty, true)
    }

    fn capture_origins_in_type(&self, ty: &Ty) -> Vec<crate::origin::CaptureOrigin> {
        use crate::origin::{CallableEnvironment, CaptureOrigin, CaptureOriginSet};

        fn collect(checker: &Checker, ty: &Ty, out: &mut Vec<CaptureOrigin>) {
            if let Some(element) = list_element(ty).or_else(|| set_element(ty)) {
                collect(checker, element, out);
                return;
            }
            if let Some((key, value)) = dict_elements(ty) {
                collect(checker, key, out);
                collect(checker, value, out);
                return;
            }
            if let Some(elements) = tuple_elements(ty) {
                for element in elements {
                    collect(checker, element, out);
                }
                return;
            }
            match ty {
                Ty::Ref(reference) => out.push(CaptureOrigin::read(reference.origin.clone())),
                Ty::Pointer { element, origin } => {
                    if let Some(origin) = origin.as_origin() {
                        out.push(CaptureOrigin::read(origin));
                    }
                    collect(checker, element, out);
                }
                Ty::ComptimeList(element) => collect(checker, element, out),
                Ty::Tuple(elements) | Ty::RuntimePack(elements) | Ty::Variant(elements) => {
                    for element in elements {
                        collect(checker, element, out);
                    }
                }
                Ty::Struct(name, arguments) => {
                    let Some(info) = checker.structs.get(name) else {
                        return;
                    };
                    let subst = struct_subst(&info.decls, arguments);
                    for (_, field) in &info.fields {
                        collect(checker, &substitute(field, &subst), out);
                    }
                }
                Ty::Func {
                    environment:
                        CallableEnvironment::Capturing(CaptureOriginSet::Concrete(captures)),
                    ..
                }
                | Ty::GenericFunc {
                    environment:
                        CallableEnvironment::Capturing(CaptureOriginSet::Concrete(captures)),
                    ..
                } => out.extend(captures.iter().cloned()),
                _ => {}
            }
        }

        let mut origins = Vec::new();
        collect(self, ty, &mut origins);
        let CaptureOriginSet::Concrete(origins) = CaptureOriginSet::concrete(origins) else {
            unreachable!("concrete capture canonicalization stays concrete")
        };
        origins
    }

    fn checked_capture(
        &self,
        name: &str,
        binding: crate::origin::OwnerId,
        ty: Ty,
        kind: crate::ast::CaptureKind,
    ) -> crate::checked::CheckedCapture {
        use crate::origin::{CaptureAccess, CaptureOrigin, Origin, OriginPlace};
        let mut origins = self.capture_origins_in_type(&ty);
        match kind {
            crate::ast::CaptureKind::Read => origins.push(CaptureOrigin {
                origin: Origin::Place(OriginPlace {
                    root: binding,
                    path: Vec::new(),
                }),
                access: CaptureAccess::Read,
            }),
            crate::ast::CaptureKind::Mut | crate::ast::CaptureKind::Ref => {
                origins.push(CaptureOrigin {
                    origin: Origin::Place(OriginPlace {
                        root: binding,
                        path: Vec::new(),
                    }),
                    access: CaptureAccess::Write,
                })
            }
            crate::ast::CaptureKind::Copy | crate::ast::CaptureKind::Move => {}
        }
        let crate::origin::CaptureOriginSet::Concrete(origins) =
            crate::origin::CaptureOriginSet::concrete(origins)
        else {
            unreachable!("concrete capture canonicalization stays concrete")
        };
        crate::checked::CheckedCapture {
            name: name.to_string(),
            binding,
            ty,
            kind,
            origins,
        }
    }

    fn type_storage_contains(&self, ty: &Ty, pointer_loans: bool) -> bool {
        fn contains(
            checker: &Checker,
            ty: &Ty,
            pointer_loans: bool,
            seen: &mut HashSet<String>,
        ) -> bool {
            if let Some(element) = list_element(ty).or_else(|| set_element(ty)) {
                return contains(checker, element, pointer_loans, seen);
            }
            if let Some((key, value)) = dict_elements(ty) {
                return contains(checker, key, pointer_loans, seen)
                    || contains(checker, value, pointer_loans, seen);
            }
            if let Some(elements) = tuple_elements(ty) {
                return elements
                    .into_iter()
                    .any(|element| contains(checker, element, pointer_loans, seen));
            }
            match ty {
                Ty::Ref(_) => true,
                Ty::Pointer { element, origin } => {
                    (pointer_loans && origin.as_origin().is_some())
                        || contains(checker, element, pointer_loans, seen)
                }
                Ty::ComptimeList(element) => contains(checker, element, pointer_loans, seen),
                Ty::Tuple(elements) | Ty::RuntimePack(elements) => elements
                    .iter()
                    .any(|element| contains(checker, element, pointer_loans, seen)),
                Ty::Variant(alternatives) => alternatives
                    .iter()
                    .any(|alternative| contains(checker, alternative, pointer_loans, seen)),
                Ty::Struct(name, args) => {
                    let key = ty.to_string();
                    if !seen.insert(key.clone()) {
                        return false;
                    }
                    let result = checker.structs.get(name).is_some_and(|info| {
                        let subst = struct_subst(&info.decls, args);
                        info.fields
                            .iter()
                            .map(|(_, field)| substitute(field, &subst))
                            .any(|field| contains(checker, &field, pointer_loans, seen))
                    });
                    seen.remove(&key);
                    result
                }
                Ty::Func {
                    environment:
                        crate::origin::CallableEnvironment::Capturing(
                            crate::origin::CaptureOriginSet::Concrete(captures),
                        ),
                    ..
                }
                | Ty::GenericFunc {
                    environment:
                        crate::origin::CallableEnvironment::Capturing(
                            crate::origin::CaptureOriginSet::Concrete(captures),
                        ),
                    ..
                } => captures.iter().any(|capture| {
                    matches!(
                        capture.origin,
                        crate::origin::Origin::Place(_) | crate::origin::Origin::Param(_)
                    )
                }),
                _ => false,
            }
        }
        contains(self, ty, pointer_loans, &mut HashSet::new())
    }

    fn type_contains_unsafe_any_pointer(ty: &Ty) -> bool {
        if let Some(element) = list_element(ty).or_else(|| set_element(ty)) {
            return Self::type_contains_unsafe_any_pointer(element);
        }
        if let Some((key, value)) = dict_elements(ty) {
            return Self::type_contains_unsafe_any_pointer(key)
                || Self::type_contains_unsafe_any_pointer(value);
        }
        if let Some(elements) = tuple_elements(ty) {
            return elements
                .into_iter()
                .any(Self::type_contains_unsafe_any_pointer);
        }
        match ty {
            Ty::Pointer {
                origin: crate::origin::PointerOrigin::UnsafeAny { .. },
                ..
            } => true,
            Ty::Pointer { element, .. } | Ty::ComptimeList(element) => {
                Self::type_contains_unsafe_any_pointer(element)
            }
            Ty::Tuple(elements) | Ty::RuntimePack(elements) | Ty::Variant(elements) => {
                elements.iter().any(Self::type_contains_unsafe_any_pointer)
            }
            _ => false,
        }
    }

    /// Origins retained by a value expression. This follows only value flow;
    /// ordinary arithmetic and reads cannot invent a stored reference handle.
    fn aggregate_origins(&self, expression: &Expr) -> Vec<crate::origin::Origin> {
        use crate::origin::Origin;

        fn append_unique(into: &mut Vec<Origin>, values: impl IntoIterator<Item = Origin>) {
            for value in values {
                if !into.contains(&value) {
                    into.push(value);
                }
            }
        }

        match &expression.kind {
            ExprKind::Identifier(name) => {
                let aggregate = self.lookup_aggregate_origins(name);
                if !aggregate.is_empty() {
                    return aggregate;
                }
                match self.lookup(name) {
                    Some(Ty::Ref(reference)) => vec![reference.origin.clone()],
                    Some(Ty::Pointer { origin, .. }) => origin
                        .as_origin()
                        .map(|origin| vec![origin])
                        .unwrap_or_default(),
                    Some(ty @ (Ty::Func { .. } | Ty::GenericFunc { .. })) => self
                        .capture_origins_in_type(ty)
                        .into_iter()
                        .map(|capture| capture.origin)
                        .collect(),
                    _ => self
                        .lookup_reference_parameter(name)
                        .map(|reference| vec![reference.origin])
                        .unwrap_or_default(),
                }
            }
            ExprKind::Member { object, field } => {
                if let Some(origins) = self.aggregate_field_origins(object).get(field) {
                    return origins.clone();
                }
                let aggregate = self.aggregate_origins(object);
                if !aggregate.is_empty() {
                    aggregate
                } else {
                    self.infer_reference_value(expression)
                        .map(|reference| vec![reference.origin])
                        .unwrap_or_default()
                }
            }
            ExprKind::Transfer(inner) | ExprKind::Named { value: inner, .. } => {
                self.aggregate_origins(inner)
            }
            ExprKind::ListLit(values) | ExprKind::TupleLit(values) => {
                let mut result = Vec::new();
                for value in values {
                    append_unique(&mut result, self.aggregate_origins(value));
                }
                result
            }
            ExprKind::IfExpr {
                then_branch,
                else_branch,
                ..
            } => {
                let mut result = self.aggregate_origins(then_branch);
                append_unique(&mut result, self.aggregate_origins(else_branch));
                result
            }
            ExprKind::Call {
                name, args, kwargs, ..
            } => {
                // A checked pointer construction retains exactly its source
                // place; the checker recorded the decision when it typed the
                // call, so shadowed `UnsafePointer` names cannot match here.
                if self
                    .operation_adjustments
                    .borrow()
                    .get(&expression.source_span())
                    .is_some_and(|operation| {
                        matches!(
                            operation,
                            crate::checked::SemanticAdjustment::PointerToPlace { .. }
                        )
                    })
                    && let Some(argument) = kwargs.first()
                    && let Ok(place) = self.origin_place(&argument.value)
                {
                    return vec![Origin::Place(place)];
                }
                let mut result = Vec::new();
                if let Some(info) = self.structs.get(name) {
                    if info.fieldwise_init {
                        let fields: Vec<Ty> =
                            info.fields.iter().map(|(_, ty)| ty.clone()).collect();
                        for (field, argument) in fields.iter().zip(args) {
                            if matches!(field, Ty::Ref(_)) {
                                if let Ok(reference) = self.reference_actual(argument) {
                                    append_unique(&mut result, [reference.origin]);
                                }
                            } else {
                                append_unique(&mut result, self.aggregate_origins(argument));
                            }
                        }
                    } else if let Some(signature) =
                        info.methods.get("__init__").and_then(|signatures| {
                            signatures.iter().find(|sig| sig.params.len() == args.len())
                        })
                    {
                        let refs = signature.ref_params.clone();
                        for (index, argument) in args.iter().enumerate() {
                            if refs.get(index).is_some_and(Option::is_some) {
                                if let Ok(reference) = self.reference_actual(argument) {
                                    append_unique(&mut result, [reference.origin]);
                                }
                            } else {
                                append_unique(&mut result, self.aggregate_origins(argument));
                            }
                        }
                    }
                }
                if result.is_empty() {
                    for argument in args {
                        append_unique(&mut result, self.aggregate_origins(argument));
                    }
                    for argument in kwargs {
                        append_unique(&mut result, self.aggregate_origins(&argument.value));
                    }
                }
                result
            }
            ExprKind::Invoke { args, kwargs, .. } | ExprKind::MethodCall { args, kwargs, .. } => {
                let mut result = Vec::new();
                for argument in args {
                    append_unique(&mut result, self.aggregate_origins(argument));
                }
                for argument in kwargs {
                    append_unique(&mut result, self.aggregate_origins(&argument.value));
                }
                result
            }
            _ => Vec::new(),
        }
    }

    fn aggregate_origin_escapes(&self, origin: &crate::origin::Origin) -> bool {
        use crate::origin::Origin;
        let Some((base, allowed)) = self.aggregate_escape_contexts.last() else {
            return false;
        };
        match origin {
            Origin::Place(place) => {
                let scope = self
                    .owner_scopes
                    .iter()
                    .position(|owners| owners.values().any(|candidate| *candidate == place.root));
                scope.is_some_and(|scope| scope >= *base && !allowed.contains(&place.root))
            }
            Origin::Union(origins) => origins
                .iter()
                .any(|origin| self.aggregate_origin_escapes(origin)),
            Origin::Param(_) | Origin::Static | Origin::Untracked { .. } => false,
        }
    }

    /// Require `expr` to have type `Bool` (used for `if`/`while` conditions).
    fn expect_bool(&self, expr: &Expr, context: &str) -> Result<(), TypeError> {
        let ty = self.infer(expr)?;
        if ty == Ty::Bool {
            Ok(())
        } else {
            Err(TypeError::TypeMismatch {
                expected: "Bool".to_string(),
                found: ty.to_string(),
                context: context.to_string(),
            })
        }
    }

    /// Check a comprehension in its own lexical scope and cache its result type
    /// for ordinary read-only expression inference. Clauses are visited in
    /// source order, so a later iterable/filter sees earlier bindings while the
    /// produced key/value sees all generator bindings.
    fn check_comprehension(&mut self, expression: &Expr) -> Result<(), TypeError> {
        let ExprKind::Comprehension {
            kind,
            key,
            value,
            clauses,
        } = &expression.kind
        else {
            return Ok(());
        };

        let scope_base = self.scopes.len();
        let mut bindings = Vec::new();
        let result = (|| {
            for clause in clauses {
                match clause {
                    crate::ast::ComprehensionClause::For {
                        var,
                        reference,
                        owned,
                        iter,
                    } => {
                        self.register_named_bindings(iter)?;
                        let iter_ty = self.infer(iter)?;
                        let (elem_ty, mut protocol) = self.iteration_protocol(&iter_ty, *owned)?;
                        if !*owned
                            && (list_element(&iter_ty).is_some()
                                || set_element(&iter_ty).is_some()
                                || dict_elements(&iter_ty).is_some())
                            && let Ok(mut origin) = self.origin_place(iter)
                        {
                            origin
                                .path
                                .push(crate::origin::OriginSeg::Interior("element".to_string()));
                            protocol.borrowed_origin = Some(origin);
                        }
                        self.iteration_protocols
                            .borrow_mut()
                            .insert(iter.source_span(), protocol);
                        if *reference {
                            return Err(TypeError::Unsupported(
                                "reference bindings in collection comprehensions are not implemented; use an explicit `for ref` loop"
                                    .to_string(),
                            ));
                        }
                        if *owned && !matches!(iter.kind, ExprKind::Transfer(_)) {
                            return Err(TypeError::Unsupported(
                                "an owned comprehension binding requires a transferred iterable (`for var x in values^`)"
                                    .to_string(),
                            ));
                        }
                        if !*owned && matches!(iter.kind, ExprKind::Transfer(_)) {
                            return Err(TypeError::Unsupported(
                                "a transferred comprehension iterable requires an explicit `var` binding"
                                    .to_string(),
                            ));
                        }
                        if !*owned && !*reference && !self.is_copyable(&elem_ty) {
                            return Err(TypeError::NonCopyable {
                                ty: elem_ty.to_string(),
                                context:
                                    "immutable comprehension iteration; use `for var ... in ...^`"
                                        .to_string(),
                            });
                        }
                        let binding_ty = elem_ty;
                        let implicitly_deletable = self.is_implicitly_deletable(&binding_ty);
                        // A generator binder scopes everything to its right, but
                        // not its own iterable. Giving every generator a lexical
                        // scope also permits a later generator to shadow the same
                        // spelling without changing an outer local.
                        self.push_scope();
                        self.declare_with_mutability(var, binding_ty, *owned)?;
                        let owner = self.lookup_owner(var).ok_or_else(|| {
                            TypeError::InvariantViolation(format!(
                                "comprehension binder '{var}' has no stable owner"
                            ))
                        })?;
                        bindings.push(crate::checked::CheckedComprehensionBinding {
                            name: var.clone(),
                            owner,
                            ty: self.lookup(var).cloned().ok_or_else(|| {
                                TypeError::InvariantViolation(format!(
                                    "comprehension binder '{var}' has no checked type"
                                ))
                            })?,
                            mutable: *owned,
                            implicitly_deletable,
                        });
                    }
                    crate::ast::ComprehensionClause::If(condition) => {
                        self.register_named_bindings(condition)?;
                        self.expect_bool(condition, "comprehension filter")?;
                    }
                }
            }

            if let Some(key) = key {
                self.register_named_bindings(key)?;
            }
            self.register_named_bindings(value)?;
            let value_ty = default_literal(&self.infer(value)?);
            self.check_consuming(value, &value_ty, "collection comprehension element")?;
            let result_ty = match kind {
                crate::ast::CollectionKind::List => list_type(value_ty),
                crate::ast::CollectionKind::Set => {
                    if !self.is_hashable(&value_ty) {
                        return Err(TypeError::TraitNotSatisfied {
                            param: "T".to_string(),
                            ty: value_ty.to_string(),
                            trait_name: "Hashable".to_string(),
                            reason: self.trait_failure_reason(&value_ty, "Hashable"),
                        });
                    }
                    set_type(value_ty)
                }
                crate::ast::CollectionKind::Dict => {
                    let key = key.as_ref().expect("dictionary comprehension has a key");
                    let key_ty = default_literal(&self.infer(key)?);
                    self.check_consuming(key, &key_ty, "dictionary comprehension key")?;
                    if !self.is_hashable(&key_ty) {
                        return Err(TypeError::TraitNotSatisfied {
                            param: "K".to_string(),
                            ty: key_ty.to_string(),
                            trait_name: "Hashable".to_string(),
                            reason: self.trait_failure_reason(&key_ty, "Hashable"),
                        });
                    }
                    dict_type(key_ty, value_ty)
                }
            };
            self.record_collection_construction(expression.source_span(), &result_ty);
            self.expression_types
                .borrow_mut()
                .insert(expression.source_span(), result_ty);
            self.comprehension_bindings
                .borrow_mut()
                .insert(expression.source_span(), bindings.clone());
            Ok(())
        })();
        while self.scopes.len() > scope_base {
            self.pop_scope();
        }
        result
    }

    fn register_named_bindings(&mut self, expression: &Expr) -> Result<(), TypeError> {
        if matches!(expression.kind, ExprKind::Comprehension { .. }) {
            return self.check_comprehension(expression);
        }
        if let ExprKind::Named { name, value } = &expression.kind {
            self.register_named_bindings(value)?;
            let found = self.infer(value)?;
            let base = self
                .function_bases
                .last()
                .copied()
                .unwrap_or(self.scopes.len().saturating_sub(1));
            let existing = self.scopes[base..]
                .iter()
                .rev()
                .find_map(|scope| scope.get(name))
                .cloned();
            if let Some(existing) = existing {
                if !self.value_coerces(&found, &existing) {
                    return Err(TypeError::TypeMismatch {
                        expected: existing.to_string(),
                        found: found.to_string(),
                        context: format!("walrus assignment to '{name}'"),
                    });
                }
            } else {
                let declared = self.inferred_binding_ty(&found, name)?;
                self.declare_function_implicit(name, declared)?;
            }
            return Ok(());
        }
        match &expression.kind {
            ExprKind::Prefix(_, value) | ExprKind::Transfer(value) => {
                self.register_named_bindings(value)?
            }
            ExprKind::Infix(_, left, right)
            | ExprKind::Index {
                object: left,
                index: right,
            } => {
                self.register_named_bindings(left)?;
                self.register_named_bindings(right)?;
            }
            ExprKind::Call { args, kwargs, .. } => {
                for argument in args {
                    self.register_named_bindings(argument)?;
                }
                for argument in kwargs {
                    self.register_named_bindings(&argument.value)?;
                }
            }
            ExprKind::Invoke {
                callee,
                args,
                kwargs,
                ..
            } => {
                self.register_named_bindings(callee)?;
                for argument in args {
                    self.register_named_bindings(argument)?;
                }
                for argument in kwargs {
                    self.register_named_bindings(&argument.value)?;
                }
            }
            ExprKind::Member { object, .. } => self.register_named_bindings(object)?,
            ExprKind::MethodCall {
                object,
                args,
                kwargs,
                ..
            } => {
                self.register_named_bindings(object)?;
                for argument in args {
                    self.register_named_bindings(argument)?;
                }
                for argument in kwargs {
                    self.register_named_bindings(&argument.value)?;
                }
            }
            ExprKind::Slice {
                object,
                lower,
                upper,
                step,
                ..
            } => {
                self.register_named_bindings(object)?;
                for bound in [lower, upper, step].into_iter().flatten() {
                    self.register_named_bindings(bound)?;
                }
            }
            ExprKind::MultiIndex { object, args } => {
                self.register_named_bindings(object)?;
                for argument in args {
                    match argument {
                        crate::ast::SubscriptArg::Index(value) => {
                            self.register_named_bindings(value)?
                        }
                        crate::ast::SubscriptArg::Slice {
                            lower, upper, step, ..
                        } => {
                            for bound in [lower, upper, step].into_iter().flatten() {
                                self.register_named_bindings(bound)?;
                            }
                        }
                    }
                }
            }
            ExprKind::ListLit(values) | ExprKind::TupleLit(values) => {
                for value in values {
                    self.register_named_bindings(value)?;
                }
            }
            ExprKind::BraceLit(entries) => {
                for (key, value) in entries {
                    self.register_named_bindings(key)?;
                    if let Some(value) = value {
                        self.register_named_bindings(value)?;
                    }
                }
            }
            ExprKind::IfExpr {
                cond,
                then_branch,
                else_branch,
            } => {
                self.register_named_bindings(cond)?;
                self.register_named_bindings(then_branch)?;
                self.register_named_bindings(else_branch)?;
            }
            ExprKind::Compare { first, rest } => {
                self.register_named_bindings(first)?;
                for (_, value) in rest {
                    self.register_named_bindings(value)?;
                }
            }
            ExprKind::TString { parts, .. } => {
                for part in parts {
                    if let TStringPart::Expr(value) = part {
                        self.register_named_bindings(value)?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn predeclare_implicit_assignments(&mut self, statements: &[Stmt]) -> Result<(), TypeError> {
        for statement in statements {
            match &statement.kind {
                StmtKind::Assign { name, value } if self.lookup(name).is_none() => {
                    let found = self.infer(value)?;
                    let declared = self.inferred_binding_ty(&found, name)?;
                    self.declare_function_implicit(name, declared)?;
                    if let Some(owner) = self.lookup_owner(name) {
                        self.uninitialized.borrow_mut().insert(owner);
                    }
                }
                StmtKind::If { branches, orelse } => {
                    for (_, body) in branches {
                        self.predeclare_implicit_assignments(body)?;
                    }
                    if let Some(body) = orelse {
                        self.predeclare_implicit_assignments(body)?;
                    }
                }
                StmtKind::While { body, .. } | StmtKind::For { body, .. } => {
                    self.predeclare_implicit_assignments(body)?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn infer(&self, expr: &Expr) -> Result<Ty, TypeError> {
        let result = self.infer_impl(expr);
        if let Ok(ty) = &result {
            if matches!(
                ty,
                Ty::Int | Ty::UInt | Ty::Float64 | Ty::Simd { width: 1, .. }
            ) && let Some(value) = self.exact_literal_value(expr)
            {
                let literal_ty = match value {
                    CtValue::IntLiteral(_) => Ty::IntLiteral,
                    CtValue::FloatLiteral(_) => Ty::FloatLiteral,
                    _ => unreachable!("exact_literal_value only returns literal values"),
                };
                self.record_literal_materializations(expr, &literal_ty, ty)?;
            }
            self.expression_types
                .borrow_mut()
                .insert(expr.source_span(), ty.clone());
            if let Some(crate::checked::SemanticAdjustment::ReferenceResult { reference }) =
                self.operation_adjustments.borrow().get(&expr.source_span())
                && self.is_copyable(&reference.referent)
            {
                self.copyable_reference_result_reads
                    .borrow_mut()
                    .insert(expr.source_span());
            }
            if let ExprKind::Call {
                name,
                param_args,
                args,
                ..
            } = &expr.kind
            {
                let dimensions = if name == "SIMD" {
                    self.simd_dims(param_args).ok().map(|(dtype, width)| {
                        let width = if width == -1 {
                            i64::try_from(args.len()).unwrap_or(0)
                        } else {
                            width
                        };
                        (dtype, width)
                    })
                } else {
                    Dtype::from_scalar_alias(name).map(|dtype| (dtype, 1))
                };
                if let Some(dimensions) = dimensions {
                    self.simd_constructions
                        .borrow_mut()
                        .insert(expr.source_span(), dimensions);
                }
            }
            let place_ty = match &expr.kind {
                ExprKind::Identifier(name) => self.lookup(name).cloned(),
                ExprKind::Member { object, field } => self.infer(object).ok().and_then(|base| {
                    let Ty::Struct(name, arguments) = base else {
                        return None;
                    };
                    let info = self.structs.get(&name)?;
                    let (_, field_ty) = info
                        .fields
                        .iter()
                        .find(|(candidate, _)| candidate == field)?;
                    Some(substitute(field_ty, &struct_subst(&info.decls, &arguments)))
                }),
                ExprKind::Index { object, index } => self
                    .index_storage_ty(object, index)
                    .or_else(|| Some(ty.clone())),
                ExprKind::TypeApply { .. }
                    if self
                        .operation_adjustments
                        .borrow()
                        .get(&expr.source_span())
                        .is_some_and(|operation| {
                            matches!(
                                operation,
                                crate::checked::SemanticAdjustment::VariantProject { .. }
                            )
                        }) =>
                {
                    Some(ty.clone())
                }
                _ => None,
            };
            if let Some(place_ty) = place_ty {
                self.expression_place_types
                    .borrow_mut()
                    .insert(expr.source_span(), place_ty);
            }
        }
        result
    }

    /// Type of a reference *handle* in a context that stores or forwards one.
    /// Ordinary expression inference intentionally reads through references.
    fn infer_reference_value(&self, expr: &Expr) -> Option<crate::origin::RefTy> {
        if let Some(crate::checked::SemanticAdjustment::ReferenceResult { reference }) =
            self.operation_adjustments.borrow().get(&expr.source_span())
        {
            return Some(reference.clone());
        }
        match &expr.kind {
            // `^` changes ownership of the reference *value*, not what that
            // value refers to.  Named arguments are likewise transparent to
            // contextual reference-value inference.  Ordinary inference still
            // reads through both forms unless the surrounding storage/call
            // context explicitly asks to retain the handle.
            ExprKind::Transfer(inner) | ExprKind::Named { value: inner, .. } => {
                self.infer_reference_value(inner)
            }
            ExprKind::Identifier(name) => match self.lookup(name) {
                Some(Ty::Ref(reference)) => Some(reference.clone()),
                _ => self.lookup_reference_parameter(name),
            },
            ExprKind::Member { object, field } => {
                let object_ty = self.infer(object).ok()?;
                let Ty::Struct(name, _) = object_ty else {
                    return None;
                };
                self.structs
                    .get(&name)?
                    .fields
                    .iter()
                    .find_map(|(candidate, ty)| {
                        (candidate == field).then_some(ty).and_then(|ty| match ty {
                            Ty::Ref(reference) => Some(reference.clone()),
                            _ => None,
                        })
                    })
            }
            ExprKind::Index { object, index } => match self.index_storage_ty(object, index)? {
                Ty::Ref(reference) => Some(reference),
                referent => self
                    .interior_references
                    .borrow()
                    .get(&expr.source_span())
                    .cloned()
                    .map(|origin| crate::origin::RefTy {
                        referent: Box::new(referent),
                        mutability: if self.owner_is_mutable(origin.root) {
                            crate::origin::Mutability::Mutable
                        } else {
                            crate::origin::Mutability::Immutable
                        },
                        origin: crate::origin::Origin::Place(origin),
                    }),
            },
            ExprKind::TypeApply { .. } => self
                .interior_references
                .borrow()
                .get(&expr.source_span())
                .cloned()
                .and_then(|origin| {
                    self.infer(expr).ok().map(|referent| crate::origin::RefTy {
                        referent: Box::new(referent),
                        mutability: if self.owner_is_mutable(origin.root) {
                            crate::origin::Mutability::Mutable
                        } else {
                            crate::origin::Mutability::Immutable
                        },
                        origin: crate::origin::Origin::Place(origin),
                    })
                }),
            _ => None,
        }
    }

    /// Infer the type stored by `expr` when the surrounding context expects a
    /// reference-bearing value.  Normal expression inference intentionally reads
    /// through a `ref`; aggregate construction instead needs to preserve the
    /// handle.  Keeping this contextual and recursive avoids changing ordinary
    /// expression semantics for tuples/lists which merely contain references.
    fn infer_storage_value(&self, expr: &Expr, expected: &Ty) -> Result<Ty, TypeError> {
        match expected {
            Ty::Ref(_) => self
                .infer_reference_value(expr)
                .map(Ty::Ref)
                .ok_or_else(|| TypeError::TypeMismatch {
                    expected: expected.to_string(),
                    found: self
                        .infer(expr)
                        .map_or_else(|_| "<error>".to_string(), |ty| ty.to_string()),
                    context: "reference-valued aggregate element".to_string(),
                }),
            expected if tuple_elements(expected).is_some() => {
                let expected_elements = tuple_elements(expected).expect("tuple elements");
                let values = match &expr.kind {
                    ExprKind::TupleLit(values) => Some(values.as_slice()),
                    ExprKind::Call { name, args, .. } if name == "Tuple" => Some(args.as_slice()),
                    _ => None,
                };
                if let Some(values) = values {
                    if values.len() != expected_elements.len() {
                        return Err(TypeError::ArityMismatch {
                            name: "Tuple".to_string(),
                            expected: expected_elements.len(),
                            got: values.len(),
                        });
                    }
                    let actual = values
                        .iter()
                        .zip(expected_elements)
                        .map(|(value, expected)| self.infer_storage_value(value, expected))
                        .collect::<Result<Vec<_>, _>>()
                        .map(nominal_tuple_type)?;
                    self.record_collection_construction(expr.source_span(), expected);
                    self.expression_types
                        .borrow_mut()
                        .insert(expr.source_span(), expected.clone());
                    return Ok(actual);
                }
                self.infer(expr)
            }
            expected if list_element(expected).is_some() => {
                let expected_element = list_element(expected).expect("list element");
                let values = match &expr.kind {
                    ExprKind::ListLit(values) => Some(values.as_slice()),
                    ExprKind::Call { name, args, .. } if name == "List" => Some(args.as_slice()),
                    _ => None,
                };
                if let Some(values) = values {
                    for value in values {
                        let actual = self.infer_storage_value(value, expected_element)?;
                        if !Self::storage_value_coerces(&actual, expected_element) {
                            return Err(TypeError::TypeMismatch {
                                expected: expected_element.to_string(),
                                found: actual.to_string(),
                                context: "reference-valued list element".to_string(),
                            });
                        }
                    }
                    self.record_collection_construction(expr.source_span(), expected);
                    self.expression_types
                        .borrow_mut()
                        .insert(expr.source_span(), expected.clone());
                    return Ok(list_type(expected_element.clone()));
                }
                self.infer(expr)
            }
            _ => self.infer(expr),
        }
    }

    /// Storage compatibility is ordinary coercion plus recursive reference
    /// compatibility.  A tracked origin parameter accepts a tracked place; an
    /// explicitly untracked field must only receive the same untracked kind.
    fn storage_value_coerces(from: &Ty, to: &Ty) -> bool {
        match (from, to) {
            (Ty::Ref(actual), Ty::Ref(expected)) => {
                coerces(&actual.referent, &expected.referent)
                    && (expected.mutability != crate::origin::Mutability::Mutable
                        || actual.mutability == crate::origin::Mutability::Mutable)
                    && match &expected.origin {
                        crate::origin::Origin::Untracked { mutable } => matches!(
                            &actual.origin,
                            crate::origin::Origin::Untracked {
                                mutable: actual_mutability
                            } if actual_mutability == mutable
                        ),
                        _ => !matches!(actual.origin, crate::origin::Origin::Untracked { .. }),
                    }
            }
            (
                Ty::Pointer {
                    element: actual_element,
                    origin: actual_origin,
                },
                Ty::Pointer {
                    element: expected_element,
                    origin: expected_origin,
                },
            ) => {
                coerces(actual_element, expected_element)
                    && match (actual_origin, expected_origin) {
                        // A concrete place provenance may bind a declared origin
                        // parameter, but storage must not invent mutable
                        // capability an immutable place never had.
                        (
                            crate::origin::PointerOrigin::Place { mutable, .. },
                            crate::origin::PointerOrigin::Param { mutability, .. },
                        ) => *mutable || *mutability == crate::origin::Mutability::Immutable,
                        _ => actual_origin == expected_origin,
                    }
            }
            (actual, expected)
                if tuple_elements(actual).is_some() && tuple_elements(expected).is_some() =>
            {
                let actual = tuple_elements(actual).expect("tuple elements");
                let expected = tuple_elements(expected).expect("tuple elements");
                actual.len() == expected.len()
                    && actual
                        .iter()
                        .zip(expected)
                        .all(|(actual, expected)| Self::storage_value_coerces(actual, expected))
            }
            (actual, expected)
                if list_element(actual).is_some() && list_element(expected).is_some() =>
            {
                Self::storage_value_coerces(
                    list_element(actual).expect("list element"),
                    list_element(expected).expect("list element"),
                )
            }
            _ => coerces(from, to),
        }
    }

    /// Mark every syntax leaf that must lower to a reference handle rather than
    /// an ordinary read-through value.  MIR consumes these checked adjustments;
    /// it never has to rediscover aggregate reference semantics from source AST.
    fn mark_reference_storage_uses(&self, expr: &Expr, expected: &Ty) {
        match expected {
            Ty::Ref(reference) => {
                // The outer `Transfer` retains its Move adjustment.  Mark the
                // transferred place itself so MIR loads the stored reference
                // handle instead of reading through it to the referent.
                let expr = match &expr.kind {
                    ExprKind::Transfer(inner) | ExprKind::Named { value: inner, .. } => inner,
                    _ => expr,
                };
                self.reference_value_uses.borrow_mut().insert(
                    expr.source_span(),
                    reference.mutability == crate::origin::Mutability::Mutable,
                );
            }
            expected if tuple_elements(expected).is_some() => {
                let expected_elements = tuple_elements(expected).expect("tuple elements");
                let values = match &expr.kind {
                    ExprKind::TupleLit(values) => Some(values.as_slice()),
                    ExprKind::Call { name, args, .. } if name == "Tuple" => Some(args.as_slice()),
                    _ => None,
                };
                if let Some(values) = values {
                    for (value, expected) in values.iter().zip(expected_elements) {
                        self.mark_reference_storage_uses(value, expected);
                    }
                }
            }
            expected if list_element(expected).is_some() => {
                let expected_element = list_element(expected).expect("list element");
                let values = match &expr.kind {
                    ExprKind::ListLit(values) => Some(values.as_slice()),
                    ExprKind::Call { name, args, .. } if name == "List" => Some(args.as_slice()),
                    _ => None,
                };
                if let Some(values) = values {
                    for value in values {
                        self.mark_reference_storage_uses(value, expected_element);
                    }
                }
            }
            _ => {}
        }
    }

    fn infer_impl(&self, expr: &Expr) -> Result<Ty, TypeError> {
        match &expr.kind {
            ExprKind::Int(_) => Ok(Ty::IntLiteral),
            ExprKind::Float(_) => Ok(Ty::FloatLiteral),
            ExprKind::Bool(_) => Ok(Ty::Bool),
            ExprKind::Str(_) => Ok(Ty::String),
            ExprKind::None => Ok(Ty::None),
            ExprKind::Uninitialized => Err(TypeError::InvariantViolation(
                "uninitialized marker reached expression inference".to_string(),
            )),
            ExprKind::TypeValue(_) => Err(TypeError::Unsupported(
                "function types as compile-time values".to_string(),
            )),
            ExprKind::Spread(_) => Err(TypeError::Unsupported(
                "call spread outside a specialized type pack".to_string(),
            )),
            ExprKind::Invoke {
                callee,
                param_args,
                args,
                kwargs,
            } => {
                if let Some(result) =
                    self.infer_variant_invoke(expr.source_span(), callee, param_args, args, kwargs)
                {
                    return result;
                }
                // Parameterized method syntax is parsed as `Invoke(Member)` so
                // that ordinary indexing remains unambiguous. Keep it a direct
                // method call semantically: bound methods do not become
                // first-class/escaping values merely because their compile-time
                // arguments are explicit.
                if let ExprKind::Member { object, field } = &callee.kind {
                    match self.infer_method_call(
                        expr.source_span(),
                        object,
                        field,
                        MethodCallArguments::parameterized(param_args, args, kwargs),
                    ) {
                        Ok(result) => return Ok(result),
                        Err(TypeError::NoSuchMethod { .. }) => {}
                        Err(error) => return Err(error),
                    }
                }
                let callable = self.infer(callee)?;
                let target = self.indirect_callable_target(&callable);
                let (ret, _, error) = self.infer_callable_ty(
                    "<callable>",
                    callable.clone(),
                    param_args,
                    args,
                    kwargs,
                )?;
                self.record_call_environment_effects(
                    expr.source_span(),
                    &callable,
                    param_args,
                    args,
                    kwargs,
                )?;
                if let Some(target) = target {
                    self.overload_targets
                        .borrow_mut()
                        .insert(expr.source_span(), target);
                }
                if let Some(error) = error.filter(|ty| *ty != Ty::Never) {
                    self.record_call_effect(expr.source_span(), error.clone());
                    self.require_error("call through a raising callable", error)?;
                }
                Ok(ret)
            }
            ExprKind::BraceLit(entries) => {
                if entries.is_empty() {
                    return Err(TypeError::Unsupported(
                        "an empty '{}' display needs a Dict[K, V] type annotation".to_string(),
                    ));
                }
                let dictionary = entries[0].1.is_some();
                if entries
                    .iter()
                    .any(|(_, value)| value.is_some() != dictionary)
                {
                    return Err(TypeError::Unsupported(
                        "set elements and dictionary key/value pairs cannot be mixed".to_string(),
                    ));
                }
                let keys = entries
                    .iter()
                    .map(|(key, _)| key.clone())
                    .collect::<Vec<_>>();
                let key_ty = self.infer_list_elem(&keys)?;
                for key in &keys {
                    self.check_consuming(key, &key_ty, "collection display element")?;
                }
                if !self.is_hashable(&key_ty) {
                    return Err(TypeError::TraitNotSatisfied {
                        param: "K".to_string(),
                        ty: key_ty.to_string(),
                        trait_name: "Hashable".to_string(),
                        reason: self.trait_failure_reason(&key_ty, "Hashable"),
                    });
                }
                if !dictionary {
                    let result = set_type(key_ty);
                    self.record_collection_construction(expr.source_span(), &result);
                    return Ok(result);
                }
                let values = entries
                    .iter()
                    .filter_map(|(_, value)| value.clone())
                    .collect::<Vec<_>>();
                let value_ty = self.infer_list_elem(&values)?;
                for value in &values {
                    self.check_consuming(value, &value_ty, "dictionary display value")?;
                }
                let result = dict_type(key_ty, value_ty);
                self.record_collection_construction(expr.source_span(), &result);
                Ok(result)
            }
            ExprKind::Comprehension { .. } => self
                .expression_types
                .borrow()
                .get(&expr.source_span())
                .cloned()
                .ok_or_else(|| {
                    TypeError::InvariantViolation(
                        "comprehension reached inference before scoped checking".to_string(),
                    )
                }),
            ExprKind::Identifier(name) => {
                self.check_capture_access(name, false)?;
                if let Some(owner) = self.lookup_owner(name) {
                    self.expression_bindings
                        .borrow_mut()
                        .insert(expr.source_span(), owner);
                }
                if self
                    .lookup_owner(name)
                    .is_some_and(|owner| self.uninitialized.borrow().contains(&owner))
                {
                    return Err(TypeError::Unsupported(format!(
                        "variable '{name}' may be uninitialized"
                    )));
                }
                self.lookup(name)
                    .map(|ty| match ty {
                        Ty::Ref(reference) => (*reference.referent).clone(),
                        other => other.clone(),
                    })
                    .ok_or_else(|| TypeError::UndefinedVariable(name.clone()))
            }
            ExprKind::Prefix(op, operand) => self.infer_prefix(*op, operand),
            ExprKind::Infix(op, left, right) => {
                self.infer_infix(Some(expr.source_span()), *op, left, right)
            }
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } => self.infer_call(expr.source_span(), name, param_args, args, kwargs),
            ExprKind::Member { object, field } => self.infer_member(object, field),
            ExprKind::MethodCall {
                object,
                method,
                args,
                kwargs,
            } => self.infer_method_call(
                expr.source_span(),
                object,
                method,
                MethodCallArguments::ordinary(args, kwargs),
            ),
            ExprKind::Index { object, index } => {
                self.infer_index(expr.source_span(), object, index)
            }
            // A dynamic/indexed expression does not designate independently
            // movable storage. Current Mojo permits `^` there only when the
            // result is implicitly copyable (the transfer is effectively a
            // copy); accepting a linear element would duplicate its destructor.
            // Compiler-private Tuple storage is the exception: each indexed
            // element is a tracked owned slot, which is how whole heterogeneous
            // packs and public Tuple's private backing field transfer linear
            // elements exactly once.
            ExprKind::Transfer(inner) => {
                let ty = self.infer(inner)?;
                let private_tuple_element = match &inner.kind {
                    ExprKind::Index { object, .. } => {
                        matches!(self.infer(object), Ok(Ty::Tuple(_)))
                    }
                    _ => false,
                };
                if place_has_index(inner)
                    && !self.is_implicitly_copyable(&ty)
                    && !private_tuple_element
                {
                    return Err(TypeError::Unsupported(
                        "cannot transfer a non-implicitly-copyable indexed value; the expression does not designate independently movable storage"
                            .to_string(),
                    ));
                }
                Ok(ty)
            }
            // An empty list literal needs a contextual element type; the
            // context-aware paths never reach this uncontextualized inference.
            ExprKind::ListLit(elems) if elems.is_empty() => Err(TypeError::CannotInferTypeParam {
                name: "List".to_string(),
                param: "T".to_string(),
            }),
            ExprKind::ListLit(elems) => {
                let result = list_type(self.infer_list_elem(elems)?);
                self.record_collection_construction(expr.source_span(), &result);
                Ok(result)
            }
            // A tuple literal keeps each element's own type (heterogeneous).
            ExprKind::TupleLit(elems) => {
                let tys = elems
                    .iter()
                    .map(|e| self.infer(e))
                    .collect::<Result<Vec<_>, _>>()?;
                let result = self.public_tuple_type(tys);
                self.record_collection_construction(expr.source_span(), &result);
                Ok(result)
            }
            // Walrus `name := value` types as `value`; MIR marks execution as
            // unsupported. The name is not bound here — `infer` is read-only — so
            // a program that *uses* the walrus-bound name later won't type-check.
            ExprKind::Named { value, .. } => self.infer(value),
            // Ternary `a if cond else b`: `cond` must be `Bool`; the branches must
            // have a common type (the result type).
            ExprKind::IfExpr {
                cond,
                then_branch,
                else_branch,
            } => {
                let ct = self.infer(cond)?;
                if ct != Ty::Bool {
                    return Err(TypeError::TypeMismatch {
                        expected: "Bool".to_string(),
                        found: ct.to_string(),
                        context: "conditional-expression condition".to_string(),
                    });
                }
                let tt = self.infer(then_branch)?;
                let et = self.infer(else_branch)?;
                common_branch_ty(&tt, &et).ok_or_else(|| TypeError::TypeMismatch {
                    expected: tt.to_string(),
                    found: et.to_string(),
                    context: "conditional-expression branches".to_string(),
                })
            }
            // Chained comparison `a < b < c`: each adjacent pair must compare to a
            // `Bool` (same rules as a single comparison); the result is `Bool`.
            ExprKind::Compare { first, rest } => {
                let mut left: &Expr = first;
                for (op, right) in rest {
                    if self.infer_infix(None, *op, left, right)? != Ty::Bool {
                        return Err(TypeError::BadOperator {
                            op: infix_symbol(*op).to_string(),
                            operands: "a chained comparison must compare to Bool".to_string(),
                        });
                    }
                    left = right;
                }
                Ok(Ty::Bool)
            }
            // Slice `object[lower:upper:step]` on a `List`/`String`: each present
            // bound must be `Int`; the result is the same sequence type.
            ExprKind::Slice {
                object,
                lower,
                upper,
                step,
                explicit_step,
            } => self.infer_slice_subscript(
                expr.source_span(),
                object,
                lower.as_deref(),
                upper.as_deref(),
                step.as_deref(),
                *explicit_step,
            ),
            ExprKind::MultiIndex { object, args } => {
                self.infer_multi_subscript(expr.source_span(), object, args)
            }
            ExprKind::TString { parts, .. } => {
                for part in parts {
                    if let TStringPart::Expr(value) = part {
                        let ty = self.infer(value)?;
                        if !self.conforms_to(&ty, "Writable") {
                            return Err(TypeError::TraitNotSatisfied {
                                param: "interpolation".to_string(),
                                ty: ty.to_string(),
                                trait_name: "Writable".to_string(),
                                reason: self.trait_failure_reason(&ty, "Writable"),
                            });
                        }
                    }
                }
                Ok(Ty::String)
            }
            // A parameterized type is not a runtime value; it is only valid as a
            // static-method receiver (`UnsafePointer[T].alloc(…)`), typed in
            // `infer_method_call`.
            ExprKind::TypeApply { name, args } => {
                if let Some(specialized) = self.infer_specialized_callable_value(
                    expr.source_span(),
                    name,
                    args,
                    None,
                    true,
                )? {
                    Ok(specialized)
                } else if let Some(Ty::Variant(alternatives)) = self.lookup(name).cloned() {
                    self.check_capture_access(name, false)?;
                    let (index, result) = self.variant_alternative(&alternatives, args)?;
                    if let Some(owner) = self.lookup_owner(name) {
                        self.expression_bindings
                            .borrow_mut()
                            .insert(expr.source_span(), owner);
                    }
                    self.operation_adjustments.borrow_mut().insert(
                        expr.source_span(),
                        crate::checked::SemanticAdjustment::VariantProject {
                            alternatives,
                            index,
                        },
                    );
                    self.record_interior_reference(expr.source_span(), expr, "value");
                    Ok(result)
                } else {
                    Err(TypeError::TypeMismatch {
                        expected: "a value".to_string(),
                        found: format!("the type '{name}[…]'"),
                        context: "a parameterized type is not a value".to_string(),
                    })
                }
            }
        }
    }

    /// Recognize compiler-known parameterized `Variant` operations. The parser
    /// preserves their type arguments on the invoke; checked metadata records
    /// every selected tag and whether the runtime operation is checked or unsafe.
    fn infer_variant_invoke(
        &self,
        span: SourceSpan,
        callee: &Expr,
        param_args: &[crate::ast::ParamArg],
        args: &[Expr],
        kwargs: &[crate::ast::KwArg],
    ) -> Option<Result<Ty, TypeError>> {
        let ExprKind::Member { object, field } = &callee.kind else {
            return None;
        };
        let object_ty = match self.infer(object) {
            Ok(ty) => ty,
            Err(error) => return Some(Err(error)),
        };
        let Ty::Variant(alternatives) = object_ty else {
            return None;
        };
        if !matches!(
            field.as_str(),
            "isa"
                | "is_type_supported"
                | "set"
                | "take"
                | "unsafe_take"
                | "replace"
                | "unsafe_replace"
        ) {
            return None;
        }
        Some((|| {
            if !kwargs.is_empty() {
                return Err(TypeError::BadCall {
                    func: format!("Variant.{field}"),
                    reason: "keyword arguments are not supported".to_string(),
                });
            }
            match field.as_str() {
                "isa" => {
                    let (index, _) = self.variant_alternative(&alternatives, param_args)?;
                    if !args.is_empty() {
                        return Err(TypeError::ArityMismatch {
                            name: "Variant.isa".to_string(),
                            expected: 0,
                            got: args.len(),
                        });
                    }
                    self.operation_adjustments.borrow_mut().insert(
                        span,
                        crate::checked::SemanticAdjustment::VariantIs {
                            alternatives,
                            index,
                        },
                    );
                    Ok(Ty::Bool)
                }
                "is_type_supported" => {
                    if param_args.len() != 1 {
                        return Err(TypeError::WrongTypeArgCount {
                            name: "Variant.is_type_supported".to_string(),
                            expected: 1,
                            got: param_args.len(),
                        });
                    }
                    if !args.is_empty() {
                        return Err(TypeError::ArityMismatch {
                            name: "Variant.is_type_supported".to_string(),
                            expected: 0,
                            got: args.len(),
                        });
                    }
                    let requested =
                        self.type_param_argument(&param_args[0], "Variant.is_type_supported")?;
                    self.operation_adjustments.borrow_mut().insert(
                        span,
                        crate::checked::SemanticAdjustment::VariantTypeSupported {
                            supported: alternatives.contains(&requested),
                        },
                    );
                    Ok(Ty::Bool)
                }
                "set" => {
                    let (index, alternative) =
                        self.variant_alternative(&alternatives, param_args)?;
                    if args.len() != 1 {
                        return Err(TypeError::ArityMismatch {
                            name: "Variant.set".to_string(),
                            expected: 1,
                            got: args.len(),
                        });
                    }
                    self.check_place(object)?;
                    let actual = self.infer(&args[0])?;
                    if !self.record_implicit_conversion(&args[0], &actual, &alternative)? {
                        return Err(TypeError::TypeMismatch {
                            expected: alternative.to_string(),
                            found: actual.to_string(),
                            context: "argument to 'Variant.set'".to_string(),
                        });
                    }
                    self.check_consuming(&args[0], &actual, "argument to 'Variant.set'")?;
                    self.operation_adjustments.borrow_mut().insert(
                        span.clone(),
                        crate::checked::SemanticAdjustment::VariantSet {
                            alternatives,
                            index,
                        },
                    );
                    self.record_interior_invalidation(span, object);
                    Ok(Ty::None)
                }
                "take" | "unsafe_take" => {
                    let (index, alternative) =
                        self.variant_alternative(&alternatives, param_args)?;
                    if !args.is_empty() {
                        return Err(TypeError::ArityMismatch {
                            name: format!("Variant.{field}"),
                            expected: 0,
                            got: args.len(),
                        });
                    }
                    if !is_place_expr(object) {
                        return Err(TypeError::BadCall {
                            func: format!("Variant.{field}"),
                            reason: "consuming receiver must be an owned place".to_string(),
                        });
                    }
                    self.operation_adjustments.borrow_mut().insert(
                        span.clone(),
                        crate::checked::SemanticAdjustment::VariantTake {
                            alternatives,
                            index,
                            checked: field == "take",
                        },
                    );
                    self.record_interior_invalidation(span, object);
                    Ok(alternative)
                }
                "replace" | "unsafe_replace" => {
                    if param_args.len() != 2 {
                        return Err(TypeError::WrongTypeArgCount {
                            name: format!("Variant.{field}"),
                            expected: 2,
                            got: param_args.len(),
                        });
                    }
                    if args.len() != 1 {
                        return Err(TypeError::ArityMismatch {
                            name: format!("Variant.{field}"),
                            expected: 1,
                            got: args.len(),
                        });
                    }
                    let input = self.type_param_argument(&param_args[0], "Variant.replace")?;
                    let output = self.type_param_argument(&param_args[1], "Variant.replace")?;
                    let input_index = alternatives
                        .iter()
                        .position(|alternative| alternative == &input)
                        .ok_or_else(|| TypeError::TypeMismatch {
                            expected: format!("one of {}", Ty::Variant(alternatives.clone())),
                            found: input.to_string(),
                            context: "Variant replacement input type".to_string(),
                        })?;
                    let output_index = alternatives
                        .iter()
                        .position(|alternative| alternative == &output)
                        .ok_or_else(|| TypeError::TypeMismatch {
                            expected: format!("one of {}", Ty::Variant(alternatives.clone())),
                            found: output.to_string(),
                            context: "Variant replacement output type".to_string(),
                        })?;
                    self.check_place(object)?;
                    if field == "replace" && !self.is_implicitly_deletable(&input) {
                        return Err(TypeError::TraitNotSatisfied {
                            param: "Tin".to_string(),
                            ty: input.to_string(),
                            trait_name: "ImplicitlyDeletable".to_string(),
                            reason: Some(
                                "checked replacement must be able to delete the incoming value if the active tag mismatches"
                                    .to_string(),
                            ),
                        });
                    }
                    let actual = self.infer(&args[0])?;
                    if !self.record_implicit_conversion(&args[0], &actual, &input)? {
                        return Err(TypeError::TypeMismatch {
                            expected: input.to_string(),
                            found: actual.to_string(),
                            context: format!("argument to 'Variant.{field}'"),
                        });
                    }
                    self.check_consuming(
                        &args[0],
                        &actual,
                        &format!("argument to 'Variant.{field}'"),
                    )?;
                    self.operation_adjustments.borrow_mut().insert(
                        span.clone(),
                        crate::checked::SemanticAdjustment::VariantReplace {
                            alternatives,
                            input_index,
                            output_index,
                            checked: field == "replace",
                        },
                    );
                    self.record_interior_invalidation(span, object);
                    Ok(output)
                }
                _ => unreachable!("checked Variant operation"),
            }
        })())
    }

    fn variant_alternative(
        &self,
        alternatives: &[Ty],
        args: &[crate::ast::ParamArg],
    ) -> Result<(usize, Ty), TypeError> {
        if args.len() != 1 {
            return Err(TypeError::WrongTypeArgCount {
                name: "Variant operation".to_string(),
                expected: 1,
                got: args.len(),
            });
        }
        let requested = self.type_param_argument(&args[0], "Variant operation")?;
        alternatives
            .iter()
            .position(|alternative| alternative == &requested)
            .map(|index| (index, requested.clone()))
            .ok_or_else(|| TypeError::TypeMismatch {
                expected: format!("one of {}", Ty::Variant(alternatives.to_vec())),
                found: requested.to_string(),
                context: "Variant operation type".to_string(),
            })
    }

    /// Infer a collection display against an expected collection type. Empty
    /// displays need this context to choose their family, and non-empty numeric
    /// displays need it to materialize elements (for example `{1, 2}` as
    /// `Set[Float64]`) instead of merely coercing the aggregate shell.
    ///
    /// Candidate scoring uses `record = false`; after overload selection the
    /// checked path repeats this with `record = true`, retaining the chosen root
    /// type and any element conversions for HIR/MIR.
    fn infer_with_expected(
        &self,
        expression: &Expr,
        expected: &Ty,
        record: bool,
    ) -> Result<Ty, TypeError> {
        // Compiler-synthesized slice descriptors have no standalone source
        // expression. Their exact protocol type is installed by
        // `synthetic_slice_descriptor`; consume that checked fact directly.
        if matches!(expression.kind, ExprKind::None)
            && let Some(ty) = self
                .expression_types
                .borrow()
                .get(&expression.source_span())
                .cloned()
        {
            return Ok(ty);
        }
        if let ExprKind::TypeApply { name, args } = &expression.kind
            && matches!(expected, Ty::Func { .. })
            && let Some(specialized) = self.infer_specialized_callable_value(
                expression.source_span(),
                name,
                args,
                Some(expected),
                record,
            )?
        {
            return Ok(specialized);
        }
        // Reference-typed values normally read through to their referents in
        // expression position.  A reference-typed parameter is a storage
        // context, just like a reference field or aggregate element: infer and
        // forward the handle itself.  This matters for generated Tuple
        // `consume_elements`, whose element type may itself be `ref[...] T`.
        if matches!(expected, Ty::Ref(_)) {
            let actual = self.infer_storage_value(expression, expected)?;
            if !Self::storage_value_coerces(&actual, expected) {
                return Err(TypeError::TypeMismatch {
                    expected: expected.to_string(),
                    found: actual.to_string(),
                    context: "reference-valued argument".to_string(),
                });
            }
            if record {
                self.mark_reference_storage_uses(expression, expected);
            }
            return Ok(actual);
        }
        // Current Mojo solves direct collection-annotation holes from the
        // literal initializer (`List[_]`, bare `List`, `Dict[String, _]`). The
        // solve is intentionally shallow: nested holes remain non-concrete.
        let solved_expected = match (&expression.kind, expected) {
            (ExprKind::ListLit(values), expected)
                if list_element(expected).is_some_and(|element| *element == Ty::Infer) =>
            {
                if values.is_empty() {
                    return Err(TypeError::CannotInferTypeParam {
                        name: "List".to_string(),
                        param: "T".to_string(),
                    });
                }
                Some(list_type(self.infer_list_elem(values)?))
            }
            (ExprKind::BraceLit(entries), expected)
                if set_element(expected).is_some_and(|element| *element == Ty::Infer)
                    && entries.iter().all(|(_, value)| value.is_none()) =>
            {
                if entries.is_empty() {
                    return Err(TypeError::CannotInferTypeParam {
                        name: "Set".to_string(),
                        param: "T".to_string(),
                    });
                }
                let keys = entries
                    .iter()
                    .map(|(key, _)| key.clone())
                    .collect::<Vec<_>>();
                Some(set_type(self.infer_list_elem(&keys)?))
            }
            (ExprKind::BraceLit(entries), expected)
                if dict_elements(expected).is_some()
                    && (entries.is_empty() || entries.iter().all(|(_, value)| value.is_some())) =>
            {
                let (expected_key, expected_value) =
                    dict_elements(expected).expect("dictionary arguments");
                if (expected_key == &Ty::Infer || expected_value == &Ty::Infer)
                    && entries.is_empty()
                {
                    return Err(TypeError::CannotInferTypeParam {
                        name: "Dict".to_string(),
                        param: "K or V".to_string(),
                    });
                }
                let actual_keys = entries
                    .iter()
                    .map(|(key, _)| key.clone())
                    .collect::<Vec<_>>();
                let actual_values = entries
                    .iter()
                    .filter_map(|(_, value)| value.clone())
                    .collect::<Vec<_>>();
                let key = if expected_key == &Ty::Infer {
                    self.infer_list_elem(&actual_keys)?
                } else {
                    expected_key.clone()
                };
                let value = if expected_value == &Ty::Infer {
                    self.infer_list_elem(&actual_values)?
                } else {
                    expected_value.clone()
                };
                (key != *expected_key || value != *expected_value).then(|| dict_type(key, value))
            }
            _ => None,
        };
        let expected = solved_expected.as_ref().unwrap_or(expected);

        let elements: Option<Vec<(&Expr, &Ty, &'static str)>> =
            if let (ExprKind::ListLit(values), Some(element)) =
                (&expression.kind, list_element(expected))
            {
                Some(
                    values
                        .iter()
                        .map(|value| (value, element, "collection display element"))
                        .collect(),
                )
            } else if let (ExprKind::BraceLit(entries), Some(element)) =
                (&expression.kind, set_element(expected))
            {
                entries.iter().all(|(_, value)| value.is_none()).then(|| {
                    entries
                        .iter()
                        .map(|(value, _)| (value, element, "collection display element"))
                        .collect()
                })
            } else if let (ExprKind::BraceLit(entries), Some((key, value))) =
                (&expression.kind, dict_elements(expected))
            {
                (entries.is_empty() || entries.iter().all(|(_, value)| value.is_some())).then(
                    || {
                        entries
                            .iter()
                            .flat_map(|(actual_key, actual_value)| {
                                [
                                    (actual_key, key, "dictionary display key"),
                                    (
                                        actual_value
                                            .as_ref()
                                            .expect("contextual dictionary entry has a value"),
                                        value,
                                        "dictionary display value",
                                    ),
                                ]
                            })
                            .collect()
                    },
                )
            } else {
                None
            };

        let Some(elements) = elements else {
            return self.infer(expression);
        };
        if let Some(element) = set_element(expected)
            && !self.is_hashable(element)
        {
            return Err(TypeError::TraitNotSatisfied {
                param: "T".to_string(),
                ty: element.to_string(),
                trait_name: "Hashable".to_string(),
                reason: self.trait_failure_reason(element, "Hashable"),
            });
        }
        if let Some((key, _)) = dict_elements(expected)
            && !self.is_hashable(key)
        {
            return Err(TypeError::TraitNotSatisfied {
                param: "K".to_string(),
                ty: key.to_string(),
                trait_name: "Hashable".to_string(),
                reason: self.trait_failure_reason(key, "Hashable"),
            });
        }

        for (value, element, context) in elements {
            let actual = self.infer_with_expected(value, element, record)?;
            let compatible = if record {
                self.record_implicit_conversion(value, &actual, element)?
            } else {
                self.value_coerces(&actual, element)
                    || self.implicit_conversion_target(&actual, element)?.is_some()
            };
            if !compatible {
                return Err(TypeError::TypeMismatch {
                    expected: element.to_string(),
                    found: actual.to_string(),
                    context: context.to_string(),
                });
            }
            self.check_consuming(value, &actual, context)?;
        }
        if record {
            self.record_collection_construction(expression.source_span(), expected);
            self.expression_types
                .borrow_mut()
                .insert(expression.source_span(), expected.clone());
        }
        Ok(expected.clone())
    }

    /// Infer the common element type of a non-empty list of expressions: numeric
    /// elements unify (widening literals), non-numeric elements must match; the
    /// result is materialized (a literal → its concrete default).
    fn infer_list_elem(&self, elems: &[Expr]) -> Result<Ty, TypeError> {
        let mut acc: Option<Ty> = None;
        for e in elems {
            let ty = self.infer(e)?;
            acc = Some(match acc {
                None => ty,
                Some(cur) => common_elem(&cur, &ty).ok_or_else(|| TypeError::TypeMismatch {
                    expected: cur.to_string(),
                    found: ty.to_string(),
                    context: "list element".to_string(),
                })?,
            });
        }
        // A non-empty literal always sets `acc`; empty is handled by the caller.
        let element = acc.ok_or_else(|| {
            TypeError::InvariantViolation("empty list reached non-empty inference".to_string())
        })?;
        let materialized = default_literal(&element);
        for value in elems {
            let actual = self.infer(value)?;
            self.record_literal_materializations(value, &actual, &materialized)?;
        }
        Ok(materialized)
    }

    fn record_collection_construction(&self, span: SourceSpan, target: &Ty) {
        let Ty::Struct(name, _) = target else {
            return;
        };
        let insert = if list_element(target).is_some() {
            Some(format!("{name}.append"))
        } else if set_element(target).is_some() {
            Some(format!("{name}.add"))
        } else if dict_elements(target).is_some() {
            Some(format!("{name}.__setitem__"))
        } else {
            None
        };
        self.operation_adjustments.borrow_mut().insert(
            span,
            crate::checked::SemanticAdjustment::ConstructCollection {
                target: target.clone(),
                insert,
            },
        );
    }

    /// Type a `List` construction: `List[T](args)` (explicit element type) or
    /// `List(args)` (element type inferred from the arguments — non-empty).
    fn infer_list_construction(
        &self,
        param_args: &[crate::ast::ParamArg],
        args: &[Expr],
    ) -> Result<Ty, TypeError> {
        if !param_args.is_empty() {
            let collection = self.list_type(param_args)?;
            let elem = list_element(&collection)
                .expect("List type construction has one type argument")
                .clone();
            for (i, arg) in args.iter().enumerate() {
                let aty = if self.type_contains_reference(&elem) {
                    self.infer_storage_value(arg, &elem)?
                } else {
                    self.infer(arg)?
                };
                if !Self::storage_value_coerces(&aty, &elem) {
                    return Err(TypeError::TypeMismatch {
                        expected: elem.to_string(),
                        found: aty.to_string(),
                        context: format!("element {} of List", i + 1),
                    });
                }
                self.record_literal_materializations(arg, &aty, &elem)?;
                self.mark_reference_storage_uses(arg, &elem);
            }
            return Ok(list_type(elem));
        }
        if args.is_empty() {
            return Err(TypeError::CannotInferTypeParam {
                name: "List".to_string(),
                param: "T".to_string(),
            });
        }
        Ok(list_type(self.infer_list_elem(args)?))
    }

    /// Type `Tuple(args...)` (element types inferred) and
    /// `Tuple[T1, ..., Tn](args...)` (fixed, element-wise checked).
    fn infer_tuple_construction(
        &self,
        param_args: &[crate::ast::ParamArg],
        args: &[Expr],
    ) -> Result<Ty, TypeError> {
        if param_args.is_empty() {
            return args
                .iter()
                .map(|arg| self.infer(arg))
                .collect::<Result<Vec<_>, _>>()
                .map(|elements| self.public_tuple_type(elements));
        }
        let tuple = self.tuple_type(param_args)?;
        let elements = tuple_elements(&tuple)
            .expect("Tuple type construction has type arguments")
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        if elements.len() != args.len() {
            return Err(TypeError::ArityMismatch {
                name: "Tuple".to_string(),
                expected: elements.len(),
                got: args.len(),
            });
        }
        for (index, (argument, expected)) in args.iter().zip(&elements).enumerate() {
            let actual = if self.type_contains_reference(expected) {
                self.infer_storage_value(argument, expected)?
            } else {
                self.infer(argument)?
            };
            if !Self::storage_value_coerces(&actual, expected) {
                return Err(TypeError::TypeMismatch {
                    expected: expected.to_string(),
                    found: actual.to_string(),
                    context: format!("element {} of Tuple", index + 1),
                });
            }
            self.mark_reference_storage_uses(argument, expected);
        }
        Ok(self.public_tuple_type(elements))
    }

    /// Select the concrete variadic-struct specialization after the compiler's
    /// discovery pass has materialized it. During discovery no such declaration
    /// exists yet, so the canonical `Tuple[T0, ...]` type remains available for
    /// collecting the exact specialization request.
    fn public_tuple_type(&self, elements: Vec<Ty>) -> Ty {
        let specialized = crate::comptime::tuple_specialization_symbol(&elements);
        let arguments = elements.iter().cloned().map(TyArg::Ty).collect::<Vec<_>>();
        match self.structs.get(&specialized) {
            Some(info) if info.fixed_arguments.as_ref() == Some(&arguments) => {
                Ty::Struct(specialized, arguments)
            }
            _ if self.declared_structs.contains(&specialized) => Ty::Struct(specialized, arguments),
            _ => nominal_tuple_type(elements),
        }
    }

    /// Replace every closed public Tuple nested in an instantiated generic
    /// result with the concrete declaration materialized by the compiler's
    /// discovery pass. Generic declarations themselves retain canonical
    /// `Tuple[T, ...]`; only a substituted use can select an executable nominal
    /// implementation.
    fn canonicalize_public_tuple_types(&self, ty: Ty) -> Ty {
        if let Some(elements) = tuple_elements(&ty) {
            let elements = elements
                .into_iter()
                .cloned()
                .map(|element| self.canonicalize_public_tuple_types(element))
                .collect();
            return self.public_tuple_type(elements);
        }
        match ty {
            Ty::Struct(name, arguments) => Ty::Struct(
                name,
                arguments
                    .into_iter()
                    .map(|argument| match argument {
                        TyArg::Ty(ty) => TyArg::Ty(self.canonicalize_public_tuple_types(ty)),
                        value => value,
                    })
                    .collect(),
            ),
            Ty::ComptimeList(element) => {
                Ty::ComptimeList(Box::new(self.canonicalize_public_tuple_types(*element)))
            }
            Ty::Tuple(elements) => Ty::Tuple(
                elements
                    .into_iter()
                    .map(|element| self.canonicalize_public_tuple_types(element))
                    .collect(),
            ),
            Ty::RuntimePack(elements) => Ty::RuntimePack(
                elements
                    .into_iter()
                    .map(|element| self.canonicalize_public_tuple_types(element))
                    .collect(),
            ),
            Ty::VariadicPack(element) => {
                Ty::VariadicPack(Box::new(self.canonicalize_public_tuple_types(*element)))
            }
            Ty::Variant(alternatives) => Ty::Variant(
                alternatives
                    .into_iter()
                    .map(|alternative| self.canonicalize_public_tuple_types(alternative))
                    .collect(),
            ),
            Ty::Pointer { element, origin } => Ty::Pointer {
                element: Box::new(self.canonicalize_public_tuple_types(*element)),
                origin,
            },
            Ty::Ref(mut reference) => {
                reference.referent =
                    Box::new(self.canonicalize_public_tuple_types(*reference.referent));
                Ty::Ref(reference)
            }
            other => other,
        }
    }

    fn infer_variant_construction(
        &self,
        span: SourceSpan,
        param_args: &[crate::ast::ParamArg],
        args: &[Expr],
        kwargs: &[crate::ast::KwArg],
    ) -> Result<Ty, TypeError> {
        if !kwargs.is_empty() {
            return Err(TypeError::BadCall {
                func: "Variant".to_string(),
                reason: "keyword arguments are not supported".to_string(),
            });
        }
        if args.len() != 1 {
            return Err(TypeError::ArityMismatch {
                name: "Variant".to_string(),
                expected: 1,
                got: args.len(),
            });
        }
        let Ty::Variant(alternatives) = self.variant_type(param_args)? else {
            return Err(TypeError::InvariantViolation(
                "Variant type construction did not produce a variant".to_string(),
            ));
        };
        let actual = self.infer(&args[0])?;
        let exact: Vec<_> = alternatives
            .iter()
            .enumerate()
            .filter(|(_, alternative)| **alternative == actual)
            .collect();
        // A bare literal first materializes to its ordinary scalar type. Current
        // Mojo therefore chooses `Int` for `Variant[Int, UInt](1)` instead of
        // treating both numeric conversions as equally good.
        let materialized = default_literal(&actual);
        let materialized_exact: Vec<_> = alternatives
            .iter()
            .enumerate()
            .filter(|(_, alternative)| **alternative == materialized)
            .collect();
        let candidates: Vec<_> = if !exact.is_empty() {
            exact
        } else if !materialized_exact.is_empty() {
            materialized_exact
        } else {
            alternatives
                .iter()
                .enumerate()
                .filter(|(_, alternative)| self.value_coerces(&actual, alternative))
                .collect()
        };
        let [(index, selected)] = candidates.as_slice() else {
            return Err(TypeError::BadCall {
                func: "Variant".to_string(),
                reason: if candidates.is_empty() {
                    format!("'{actual}' is not one of its declared alternatives")
                } else {
                    format!("'{actual}' matches more than one declared alternative")
                },
            });
        };
        if !self.record_implicit_conversion(&args[0], &actual, selected)? {
            return Err(TypeError::TypeMismatch {
                expected: selected.to_string(),
                found: actual.to_string(),
                context: "Variant payload".to_string(),
            });
        }
        self.operation_adjustments.borrow_mut().insert(
            span,
            crate::checked::SemanticAdjustment::ConstructVariant {
                alternatives: alternatives.clone(),
                index: *index,
            },
        );
        Ok(Ty::Variant(alternatives))
    }

    /// Validate an assignment **place** and return the type stored there. A place
    /// is a chain of field (`.x`) and index (`[i]`) accesses over a root that
    /// must be a mutable location: any variable, or `self` in a `mut self`
    /// method. Recursing on the object of each step verifies the whole chain is
    /// rooted at a mutable place (so `foo().x = e` or `self.x` in a read-only
    /// method are rejected). SIMD lane writes are not supported yet.
    fn check_place(&self, place: &Expr) -> Result<Ty, TypeError> {
        let result = self.check_place_impl(place);
        if let Ok(ty) = &result {
            self.expression_types
                .borrow_mut()
                .insert(place.source_span(), ty.clone());
            if let ExprKind::Identifier(name) = &place.kind
                && let Some(owner) = self.lookup_owner(name)
            {
                self.expression_bindings
                    .borrow_mut()
                    .insert(place.source_span(), owner);
            }
            let storage_ty = self.place_storage_ty(place).or_else(|| Some(ty.clone()));
            if let Some(storage_ty) = storage_ty {
                self.expression_place_types
                    .borrow_mut()
                    .insert(place.source_span(), storage_ty);
            }
        }
        result
    }

    fn place_storage_ty(&self, place: &Expr) -> Option<Ty> {
        match &place.kind {
            ExprKind::Identifier(name) => self.lookup(name).cloned(),
            ExprKind::Member { object, field } => self.infer(object).ok().and_then(|base| {
                let Ty::Struct(name, arguments) = base else {
                    return None;
                };
                let info = self.structs.get(&name)?;
                let (_, field_ty) = info
                    .fields
                    .iter()
                    .find(|(candidate, _)| candidate == field)?;
                Some(substitute(field_ty, &struct_subst(&info.decls, &arguments)))
            }),
            ExprKind::Index { object, index } => self.index_storage_ty(object, index),
            _ => None,
        }
    }

    /// Type physically stored at an index place, before the usual read-through
    /// rule for a reference element.  Tuple indices are compile-time constants,
    /// while homogeneous list/pointer storage has one element type.
    fn index_storage_ty(&self, object: &Expr, index: &Expr) -> Option<Ty> {
        let object_ty = self.infer(object).ok()?;
        if let Some(elements) = tuple_elements(&object_ty) {
            let index = usize::try_from(self.eval_ct(index).ok()?.to_i64()?).ok()?;
            return elements.get(index).map(|element| (*element).clone());
        }
        if let Some(element) = list_element(&object_ty) {
            return Some(element.clone());
        }
        if let Some((_, value)) = dict_elements(&object_ty) {
            return Some(value.clone());
        }
        match object_ty {
            Ty::Tuple(elements) => {
                let index = usize::try_from(self.eval_ct(index).ok()?.to_i64()?).ok()?;
                elements.get(index).cloned()
            }
            Ty::Pointer { element, .. } => Some(*element),
            Ty::Simd { dtype, .. } => Some(simd_ty(dtype, 1)),
            _ => None,
        }
    }

    fn check_place_impl(&self, place: &Expr) -> Result<Ty, TypeError> {
        match &place.kind {
            ExprKind::Identifier(name) => {
                if name == "self" && !self.self_mutable {
                    return Err(TypeError::ImmutableSelf);
                }
                if !self.is_binding_mutable(name) {
                    return Err(TypeError::ImmutableBinding(name.clone()));
                }
                self.lookup(name)
                    .map(|ty| match ty {
                        Ty::Ref(reference) => (*reference.referent).clone(),
                        other => other.clone(),
                    })
                    .ok_or_else(|| TypeError::UndefinedVariable(name.clone()))
            }
            ExprKind::Member { object, field } => {
                // The object must itself be a writable place (a struct value).
                self.check_place(object)?;
                // Reuse the field-typing logic (validates the field exists).
                self.infer_member(object, field)
            }
            ExprKind::Index { object, index } => {
                let obj_ty = self.check_place(object)?;
                self.prepare_index_argument(&obj_ty, index, "__setitem__", 0)?;
                if let Ty::Struct(name, _) = &obj_ty
                    && !self.structs.contains_key(name)
                {
                    if let Some(element) = list_element(&obj_ty) {
                        let idx_ty = self.infer(index)?;
                        if !self.is_index_type(&idx_ty) {
                            return Err(TypeError::TypeMismatch {
                                expected: "Indexer".to_string(),
                                found: idx_ty.to_string(),
                                context: "index".to_string(),
                            });
                        }
                        return Ok(match element.clone() {
                            Ty::Ref(reference) => *reference.referent,
                            element => element,
                        });
                    }
                    if let Some((key, value)) = dict_elements(&obj_ty) {
                        let idx_ty = self.infer(index)?;
                        if !coerces(&idx_ty, key) {
                            return Err(TypeError::TypeMismatch {
                                expected: key.to_string(),
                                found: idx_ty.to_string(),
                                context: "dictionary key".to_string(),
                            });
                        }
                        return Ok(match value.clone() {
                            Ty::Ref(reference) => *reference.referent,
                            value => value,
                        });
                    }
                    if tuple_elements(&obj_ty).is_some() {
                        return Err(TypeError::InvalidAssignTarget(
                            "Tuple elements are immutable".to_string(),
                        ));
                    }
                }
                // A user struct with `__setitem__(mut self, i, v)` is index-assignable:
                // `c[i] = e` → `c.__setitem__(i, e)`. The index must coerce to the
                // first parameter; the *target* type (what `e` must be) is the second.
                if let Ty::Struct(_, _) = &obj_ty {
                    let mut idx_ty = self.infer(index)?;
                    if self.has_index_normalization(index, &Ty::Int) {
                        idx_ty = Ty::Int;
                    }
                    let resolution = self.resolve_struct_setitem(&obj_ty, &[idx_ty], None)?;
                    if let Some(target) = resolution.lowered_name {
                        self.overload_targets
                            .borrow_mut()
                            .insert(place.source_span(), target);
                    }
                    self.subscript_descriptors
                        .borrow_mut()
                        .insert(place.source_span(), (vec![None], resolution.value_keyword));
                    return Ok(match resolution.return_type {
                        // Indexing a reference-valued container reads and writes
                        // through the stored handle. The physical storage type
                        // remains available through `place_storage_ty`; the
                        // assignment target is the referent.
                        Ty::Ref(reference) => *reference.referent,
                        value => value,
                    });
                }
                let elem = match &obj_ty {
                    // A pointer store `ptr[i] = e`: the target is the pointee type.
                    // An origin-bearing pointer designates one value and its
                    // provenance must carry mutable capability.
                    Ty::Pointer { element, origin } => {
                        self.check_pointer_offset(origin, index)?;
                        self.check_pointer_write(origin)?;
                        (**element).clone()
                    }
                    // A SIMD lane write `v[i] = e`: the target is the width-1 scalar.
                    Ty::Simd { dtype, .. } => simd_ty(*dtype, 1),
                    _ => return Err(TypeError::NotIndexable(obj_ty.to_string())),
                };
                let idx_ty = self.infer(index)?;
                if !self.is_index_type(&idx_ty) {
                    return Err(TypeError::TypeMismatch {
                        expected: "Indexer".to_string(),
                        found: idx_ty.to_string(),
                        context: "index".to_string(),
                    });
                }
                Ok(match elem {
                    Ty::Ref(reference) => *reference.referent,
                    other => other,
                })
            }
            ExprKind::Slice {
                object,
                lower,
                upper,
                step,
                explicit_step,
            } => {
                let object_type = self.check_place(object)?;
                self.check_slice_bounds(lower.as_deref(), upper.as_deref(), step.as_deref())?;
                let kind = if *explicit_step {
                    SliceKind::StridedSlice
                } else {
                    SliceKind::ContiguousSlice
                };
                let descriptor = Ty::Struct(kind.type_name().to_string(), Vec::new());
                let resolution = self.resolve_struct_setitem(&object_type, &[descriptor], None)?;
                if let Some(target) = resolution.lowered_name {
                    self.overload_targets
                        .borrow_mut()
                        .insert(place.source_span(), target);
                }
                self.subscript_descriptors.borrow_mut().insert(
                    place.source_span(),
                    (vec![Some(kind)], resolution.value_keyword),
                );
                Ok(resolution.return_type)
            }
            ExprKind::MultiIndex { object, args } => {
                let object_type = self.check_place(object)?;
                let mut argument_types = Vec::with_capacity(args.len());
                let mut descriptors = Vec::with_capacity(args.len());
                for (position, argument) in args.iter().enumerate() {
                    match argument {
                        SubscriptArg::Index(value) => {
                            self.prepare_index_argument(
                                &object_type,
                                value,
                                "__setitem__",
                                position,
                            )?;
                            let mut argument_type = self.infer(value)?;
                            if self.has_index_normalization(value, &Ty::Int) {
                                argument_type = Ty::Int;
                            }
                            argument_types.push(argument_type);
                            descriptors.push(None);
                        }
                        SubscriptArg::Slice {
                            lower,
                            upper,
                            step,
                            explicit_step,
                        } => {
                            self.check_slice_bounds(
                                lower.as_deref(),
                                upper.as_deref(),
                                step.as_deref(),
                            )?;
                            let kind = if *explicit_step {
                                SliceKind::StridedSlice
                            } else {
                                SliceKind::ContiguousSlice
                            };
                            argument_types
                                .push(Ty::Struct(kind.type_name().to_string(), Vec::new()));
                            descriptors.push(Some(kind));
                        }
                    }
                }
                let resolution =
                    self.resolve_struct_setitem(&object_type, &argument_types, None)?;
                if let Some(target) = resolution.lowered_name {
                    self.overload_targets
                        .borrow_mut()
                        .insert(place.source_span(), target);
                }
                self.subscript_descriptors
                    .borrow_mut()
                    .insert(place.source_span(), (descriptors, resolution.value_keyword));
                Ok(resolution.return_type)
            }
            ExprKind::TypeApply { name, args } => {
                self.check_capture_access(name, true)?;
                if !self.is_binding_mutable(name) {
                    return Err(TypeError::ImmutableBinding(name.clone()));
                }
                let Ty::Variant(alternatives) = self
                    .lookup(name)
                    .cloned()
                    .ok_or_else(|| TypeError::UndefinedVariable(name.clone()))?
                else {
                    return Err(TypeError::InvalidAssignTarget(name.clone()));
                };
                let (index, alternative) = self.variant_alternative(&alternatives, args)?;
                self.operation_adjustments.borrow_mut().insert(
                    place.source_span(),
                    crate::checked::SemanticAdjustment::VariantProject {
                        alternatives,
                        index,
                    },
                );
                if let Some(owner) = self.lookup_owner(name) {
                    self.expression_bindings
                        .borrow_mut()
                        .insert(place.source_span(), owner);
                }
                Ok(alternative)
            }
            other => Err(TypeError::InvalidAssignTarget(format!("{:?}", other))),
        }
    }

    fn check_slice_bounds(
        &self,
        lower: Option<&Expr>,
        upper: Option<&Expr>,
        step: Option<&Expr>,
    ) -> Result<(), TypeError> {
        for bound in [lower, upper, step].into_iter().flatten() {
            let found = self.infer(bound)?;
            if !coerces(&found, &Ty::Int) {
                return Err(TypeError::TypeMismatch {
                    expected: "Int".to_string(),
                    found: found.to_string(),
                    context: "slice bound".to_string(),
                });
            }
        }
        Ok(())
    }

    /// A slice descriptor is a real runtime argument to `__getitem__`/
    /// `__setitem__`, but its source syntax is spread across optional bounds.
    /// Give overload/origin/effect checking one typed synthetic argument while
    /// retaining descriptor construction separately for MIR.
    fn synthetic_slice_descriptor(&self, span: &SourceSpan, kind: SliceKind) -> Expr {
        let mut expression = Expr::new(ExprKind::None, span.span);
        expression.source = span.source.clone();
        self.expression_types.borrow_mut().insert(
            expression.source_span(),
            Ty::Struct(kind.type_name().to_string(), Vec::new()),
        );
        expression
    }

    /// Whether `expression` is the checker-only value standing in for source
    /// slice syntax while selecting `__getitem__`/`__setitem__`. Current Mojo
    /// permits descriptor-family widening (for example `ContiguousSlice` to
    /// `Slice`), but does not feed a slice literal through an arbitrary user
    /// `@implicit` constructor. Keeping that boundary here also prevents a
    /// conversion for a nonexistent expression from being attached to the
    /// enclosing subscript's source span.
    fn is_synthetic_slice_descriptor(&self, expression: &Expr) -> bool {
        matches!(expression.kind, ExprKind::None)
            && matches!(
                self.expression_types
                    .borrow()
                    .get(&expression.source_span()),
                Some(Ty::Struct(name, args))
                    if matches!(
                        name.as_str(),
                        "Slice" | "ContiguousSlice" | "StridedSlice"
                    ) && args.is_empty()
            )
    }

    /// Check one nominal `object[...] = value` as the exact selected
    /// `__setitem__` invocation. `Ok(None)` leaves primitive pointer/SIMD and
    /// compiler-private storage assignments on their intrinsic place path.
    fn check_nominal_subscript_assignment(
        &self,
        target: &Expr,
        value: &Expr,
    ) -> Result<Option<Ty>, TypeError> {
        let (object, mut arguments, descriptors) = match &target.kind {
            ExprKind::Index { object, index } => {
                let object_ty = self.infer(object)?;
                if !matches!(&object_ty, Ty::Struct(name, _) if self.structs.contains_key(name)) {
                    return Ok(None);
                }
                self.prepare_index_argument(&object_ty, index, "__setitem__", 0)?;
                self.infer(index)?;
                (object.as_ref(), vec![index.as_ref().clone()], vec![None])
            }
            ExprKind::Slice {
                object,
                lower,
                upper,
                step,
                explicit_step,
            } => {
                let object_ty = self.infer(object)?;
                if !matches!(&object_ty, Ty::Struct(name, _) if self.structs.contains_key(name)) {
                    return Ok(None);
                }
                self.check_slice_bounds(lower.as_deref(), upper.as_deref(), step.as_deref())?;
                let kind = if *explicit_step {
                    SliceKind::StridedSlice
                } else {
                    SliceKind::ContiguousSlice
                };
                let descriptor = self.synthetic_slice_descriptor(&target.source_span(), kind);
                (object.as_ref(), vec![descriptor], vec![Some(kind)])
            }
            ExprKind::MultiIndex {
                object,
                args: source,
            } => {
                let object_ty = self.infer(object)?;
                if !matches!(&object_ty, Ty::Struct(name, _) if self.structs.contains_key(name)) {
                    return Ok(None);
                }
                let mut arguments = Vec::with_capacity(source.len());
                let mut descriptors = Vec::with_capacity(source.len());
                for (position, argument) in source.iter().enumerate() {
                    match argument {
                        SubscriptArg::Index(index) => {
                            self.prepare_index_argument(
                                &object_ty,
                                index,
                                "__setitem__",
                                position,
                            )?;
                            self.infer(index)?;
                            arguments.push(index.clone());
                            descriptors.push(None);
                        }
                        SubscriptArg::Slice {
                            lower,
                            upper,
                            step,
                            explicit_step,
                        } => {
                            self.check_slice_bounds(
                                lower.as_deref(),
                                upper.as_deref(),
                                step.as_deref(),
                            )?;
                            let kind = if *explicit_step {
                                SliceKind::StridedSlice
                            } else {
                                SliceKind::ContiguousSlice
                            };
                            arguments
                                .push(self.synthetic_slice_descriptor(&target.source_span(), kind));
                            descriptors.push(Some(kind));
                        }
                    }
                }
                (object.as_ref(), arguments, descriptors)
            }
            _ => return Ok(None),
        };

        let object_ty = self.infer(object)?;
        let index_argument_count = arguments.len();
        let value_keyword = self.select_subscript_set_call_shape(&object_ty, &arguments, value)?;
        let kwargs = if value_keyword {
            vec![crate::ast::KwArg {
                name: "value".to_string(),
                value: value.clone(),
            }]
        } else {
            arguments.push(value.clone());
            Vec::new()
        };
        // List element replacement is allocation-stable.  Its projected write
        // is recorded after this check as `list[AnyIndex]`, preserving the
        // List's `element` generation while still expiring interiors nested in
        // the replaced element. Slice assignment and arbitrary user setters
        // retain the conservative whole-receiver mutation effect.
        let preserves_receiver_interiors =
            matches!(&target.kind, ExprKind::Index { .. }) && list_element(&object_ty).is_some();
        let call = if preserves_receiver_interiors {
            MethodCallArguments::interior_preserving(&arguments, &kwargs)
        } else {
            MethodCallArguments::ordinary(&arguments, &kwargs)
        };
        self.infer_method_call(target.source_span(), object, "__setitem__", call)?;
        let contract = self
            .selected_calls
            .borrow()
            .get(&target.source_span())
            .cloned()
            .ok_or_else(|| {
                TypeError::InvariantViolation(
                    "nominal subscript assignment lost its selected call contract".to_string(),
                )
            })?;
        if contract.receiver_convention != Some(ArgConvention::Mut) {
            return Err(TypeError::TypeMismatch {
                expected: "a 'mut self' __setitem__".to_string(),
                found: "read-only self".to_string(),
                context: format!("index assignment on '{object_ty}'"),
            });
        }
        let value_source = if value_keyword {
            crate::checked::CheckedCallArgumentSource::Keyword(0)
        } else {
            crate::checked::CheckedCallArgumentSource::Positional(index_argument_count)
        };
        let target_ty = contract
            .arguments
            .iter()
            .find(|argument| argument.source == value_source)
            .map(|argument| argument.parameter_ty.clone())
            .ok_or_else(|| {
                TypeError::InvariantViolation(
                    "selected __setitem__ contract has no assignment-value slot".to_string(),
                )
            })?;
        self.subscript_descriptors
            .borrow_mut()
            .insert(target.source_span(), (descriptors, value_keyword));
        let target_ty = match target_ty {
            Ty::Ref(reference) => *reference.referent,
            ty => ty,
        };
        self.expression_types
            .borrow_mut()
            .insert(target.source_span(), target_ty.clone());
        self.expression_place_types
            .borrow_mut()
            .insert(target.source_span(), target_ty.clone());
        Ok(Some(target_ty))
    }

    /// Select how assignment syntax supplies the implicit RHS to
    /// `__setitem__`. Both the final-positional and `value=` shapes participate
    /// in ordinary overload scoring, including RHS-driven generic inference.
    /// A signature which accepts both shapes is considered only once and uses
    /// the conventional positional ABI; distinct equally-ranked signatures are
    /// still an ambiguous overload.
    fn select_subscript_set_call_shape(
        &self,
        receiver: &Ty,
        index_arguments: &[Expr],
        value: &Expr,
    ) -> Result<bool, TypeError> {
        let Ty::Struct(name, type_arguments) = receiver else {
            return Err(TypeError::NotIndexable(receiver.to_string()));
        };
        let info = self
            .structs
            .get(name)
            .ok_or_else(|| TypeError::NotIndexable(receiver.to_string()))?;
        let signatures = info
            .methods
            .get("__setitem__")
            .ok_or_else(|| TypeError::NotIndexable(receiver.to_string()))?;
        let receiver_substitution = struct_subst(&info.decls, type_arguments);
        let mut positional_arguments = index_arguments.to_vec();
        positional_arguments.push(value.clone());
        let keyword_arguments = vec![crate::ast::KwArg {
            name: "value".to_string(),
            value: value.clone(),
        }];
        let no_keywords: &[crate::ast::KwArg] = &[];
        let shapes = [
            (false, positional_arguments.as_slice(), no_keywords),
            (true, index_arguments, keyword_arguments.as_slice()),
        ];

        // Keep only the best ABI for each declaration. A normal fixed setter
        // accepts its final parameter both positionally and by name; counting
        // those as two overloads would make every such assignment ambiguous.
        let mut best_by_signature: Vec<Option<(usize, bool)>> = vec![None; signatures.len()];
        for (signature_index, signature) in signatures.iter().enumerate() {
            if !signature.has_self {
                continue;
            }
            let receiver_params = signature
                .params
                .iter()
                .map(|parameter| substitute(parameter, &receiver_substitution))
                .collect::<Vec<_>>();
            let receiver_variadic = signature
                .variadic
                .as_deref()
                .map(|parameter| substitute(parameter, &receiver_substitution));
            let receiver_kw_variadic = signature
                .kw_variadic
                .as_deref()
                .map(|parameter| substitute(parameter, &receiver_substitution));
            for (value_keyword, arguments, keywords) in shapes {
                let Ok((params, variadic, kw_variadic, _, mut method_arguments)) = self
                    .instantiate_method_generics(
                        &format!("{name}.__setitem__"),
                        signature,
                        &receiver_params,
                        receiver_variadic.as_ref(),
                        receiver_kw_variadic.as_ref(),
                        &[],
                        arguments,
                        keywords,
                    )
                else {
                    continue;
                };
                for (declaration, argument) in info.decls.iter().zip(type_arguments) {
                    method_arguments.insert(
                        declaration.name().trim_start_matches('*').to_string(),
                        argument.clone(),
                    );
                }
                if !self.method_constraints_apply(signature, &method_arguments) {
                    continue;
                }
                let Ok(scored) = self.score_method_call(
                    signature,
                    &params,
                    variadic.as_ref(),
                    kw_variadic.as_ref(),
                    arguments,
                    keywords,
                ) else {
                    continue;
                };
                let candidate = (scored.rank, value_keyword);
                let replace =
                    best_by_signature[signature_index].is_none_or(|current| candidate < current);
                if replace {
                    best_by_signature[signature_index] = Some(candidate);
                }
            }
        }

        let candidates = best_by_signature.into_iter().flatten().collect::<Vec<_>>();
        let Some(best_rank) = candidates.iter().map(|(rank, _)| *rank).min() else {
            return Err(TypeError::BadCall {
                func: format!("{name}.__setitem__"),
                reason: "no overload matches the supplied indices and assignment value".to_string(),
            });
        };
        let mut selected = candidates
            .into_iter()
            .filter(|(rank, _)| *rank == best_rank);
        let (_, value_keyword) = selected.next().expect("at least one best setter shape");
        if selected.next().is_some() {
            return Err(TypeError::BadCall {
                func: format!("{name}.__setitem__"),
                reason: "ambiguous subscript-assignment overload".to_string(),
            });
        }
        Ok(value_keyword)
    }

    fn infer_slice_subscript(
        &self,
        span: SourceSpan,
        object: &Expr,
        lower: Option<&Expr>,
        upper: Option<&Expr>,
        step: Option<&Expr>,
        explicit_step: bool,
    ) -> Result<Ty, TypeError> {
        self.check_slice_bounds(lower, upper, step)?;
        let kind = if explicit_step {
            SliceKind::StridedSlice
        } else {
            SliceKind::ContiguousSlice
        };
        let object_type = self.infer(object)?;
        let result = match &object_type {
            Ty::String => Ty::String,
            Ty::Struct(name, _)
                if !self.structs.contains_key(name) && list_element(&object_type).is_some() =>
            {
                object_type.clone()
            }
            Ty::Struct(..) | Ty::Param { .. } => {
                let descriptor = self.synthetic_slice_descriptor(&span, kind);
                self.infer_method_call(
                    span.clone(),
                    object,
                    "__getitem__",
                    MethodCallArguments::ordinary(std::slice::from_ref(&descriptor), &[]),
                )?
            }
            _ => return Err(TypeError::NotIndexable(object_type.to_string())),
        };
        self.subscript_descriptors
            .borrow_mut()
            .insert(span, (vec![Some(kind)], false));
        Ok(result)
    }

    fn infer_multi_subscript(
        &self,
        span: SourceSpan,
        object: &Expr,
        arguments: &[SubscriptArg],
    ) -> Result<Ty, TypeError> {
        let object_type = self.infer(object)?;
        if !matches!(object_type, Ty::Struct(..) | Ty::Param { .. }) {
            return Err(TypeError::NotIndexable(object_type.to_string()));
        }
        let mut actual_arguments = Vec::with_capacity(arguments.len());
        let mut descriptors = Vec::with_capacity(arguments.len());
        for (position, argument) in arguments.iter().enumerate() {
            match argument {
                SubscriptArg::Index(value) => {
                    self.prepare_index_argument(&object_type, value, "__getitem__", position)?;
                    self.infer(value)?;
                    actual_arguments.push(value.clone());
                    descriptors.push(None);
                }
                SubscriptArg::Slice {
                    lower,
                    upper,
                    step,
                    explicit_step,
                } => {
                    self.check_slice_bounds(lower.as_deref(), upper.as_deref(), step.as_deref())?;
                    let kind = if *explicit_step {
                        SliceKind::StridedSlice
                    } else {
                        SliceKind::ContiguousSlice
                    };
                    actual_arguments.push(self.synthetic_slice_descriptor(&span, kind));
                    descriptors.push(Some(kind));
                }
            }
        }
        let result = self.infer_method_call(
            span.clone(),
            object,
            "__getitem__",
            MethodCallArguments::ordinary(&actual_arguments, &[]),
        )?;
        self.subscript_descriptors
            .borrow_mut()
            .insert(span, (descriptors, false));
        Ok(result)
    }

    /// Resolve `receiver[indices...] = value`. The assignment value is the final
    /// regular parameter for a fixed-arity `__setitem__`. A variadic setitem uses
    /// Mojo's `*indices, *, value: T` shape, so lowering must pass the value through
    /// the keyword-only slot while the source indices fill the variadic pack.
    fn resolve_struct_setitem(
        &self,
        receiver: &Ty,
        arguments: &[Ty],
        value: Option<&Ty>,
    ) -> Result<SubscriptResolution, TypeError> {
        let Ty::Struct(name, type_arguments) = receiver else {
            return Err(TypeError::NotIndexable(receiver.to_string()));
        };
        let info = self
            .structs
            .get(name)
            .ok_or_else(|| TypeError::NotIndexable(receiver.to_string()))?;
        let signatures = info
            .methods
            .get("__setitem__")
            .ok_or_else(|| TypeError::NotIndexable(receiver.to_string()))?;
        let substitution = struct_subst(&info.decls, type_arguments);
        let mut matches = Vec::new();
        let mut saw_read_only = false;
        let mut value_mismatches = Vec::new();

        for signature in signatures
            .iter()
            .filter(|signature| signature.has_self && signature.decls.is_empty())
        {
            if !matches!(
                signature.self_convention,
                Some(crate::ast::ArgConvention::Mut)
            ) {
                saw_read_only = true;
                continue;
            }
            let parameters: Vec<_> = signature
                .params
                .iter()
                .map(|parameter| substitute(parameter, &substitution))
                .collect();
            if parameters.is_empty() {
                continue;
            }
            let variadic = signature
                .variadic
                .as_deref()
                .map(|parameter| substitute(parameter, &substitution));

            let (value_index, value_keyword, fixed_index_count) = if variadic.is_some() {
                let Some(value_index) = signature.names.iter().position(|name| name == "value")
                else {
                    continue;
                };
                let fixed_index_count = signature.variadic_index.unwrap_or(0);
                // The currently published variadic operator shape has only a
                // keyword-only `value` parameter after `*indices`.
                if value_index < fixed_index_count || parameters.len() != fixed_index_count + 1 {
                    continue;
                }
                (value_index, true, fixed_index_count)
            } else {
                (parameters.len() - 1, false, parameters.len() - 1)
            };
            if arguments.len() < fixed_index_count
                || (variadic.is_none() && arguments.len() != fixed_index_count)
            {
                continue;
            }

            let mut score = 0;
            let mut compatible = true;
            for (actual, expected) in arguments
                .iter()
                .take(fixed_index_count)
                .zip(parameters.iter().take(fixed_index_count))
            {
                if !coerces(actual, expected) {
                    compatible = false;
                    break;
                }
                score += conversion_count(actual, expected);
            }
            if compatible && let Some(element) = &variadic {
                for actual in arguments.iter().skip(fixed_index_count) {
                    if !coerces(actual, element) {
                        compatible = false;
                        break;
                    }
                    score += conversion_count(actual, element);
                }
            }
            if compatible && let Some(actual) = value {
                let expected = &parameters[value_index];
                if !self.value_coerces(actual, expected)
                    && self.implicit_conversion_target(actual, expected)?.is_none()
                {
                    value_mismatches.push(expected.clone());
                    compatible = false;
                } else {
                    score += conversion_count(actual, expected);
                }
            }
            if compatible {
                matches.push((
                    overload_rank(score, variadic.is_some(), parameters.len(), false),
                    signature,
                    parameters[value_index].clone(),
                    value_keyword,
                ));
            }
        }

        matches.sort_by_key(|(score, _, _, _)| *score);
        let Some((best, signature, value_type, value_keyword)) = matches.first() else {
            if let (Some(found), Some(expected)) = (value, value_mismatches.first()) {
                return Err(TypeError::TypeMismatch {
                    expected: expected.to_string(),
                    found: found.to_string(),
                    context: format!("assignment value for '{name}.__setitem__'"),
                });
            }
            if saw_read_only {
                return Err(TypeError::TypeMismatch {
                    expected: "a 'mut self' __setitem__".to_string(),
                    found: "read-only self".to_string(),
                    context: format!("index assignment on '{name}'"),
                });
            }
            return Err(TypeError::NotIndexable(receiver.to_string()));
        };
        if matches.get(1).is_some_and(|(score, _, _, _)| score == best) {
            return Err(TypeError::BadCall {
                func: format!("{name}.__setitem__"),
                reason: "ambiguous subscript-assignment overload".to_string(),
            });
        }
        Ok(SubscriptResolution {
            return_type: value_type.clone(),
            lowered_name: (signatures.len() > 1)
                .then(|| method_lowered_name(name, "__setitem__", signature)),
            value_keyword: *value_keyword,
        })
    }

    /// Type a subscript over tuples, SIMD, lists, pointers, or a user-defined
    /// `__getitem__` implementation.
    fn infer_index(&self, span: SourceSpan, object: &Expr, index: &Expr) -> Result<Ty, TypeError> {
        let obj_ty = self.infer(object)?;
        self.prepare_index_argument(&obj_ty, index, "__getitem__", 0)?;
        // A generated Tuple declaration may occur later than the generic body
        // currently being checked (the bundled List slice overload is one such
        // body). Phase one has already validated and retained that Tuple's
        // concrete element identity, so select its conventional generated
        // accessor now instead of falling through to the metadata-only Tuple
        // shortcut and losing executable dispatch.
        if let Ty::Struct(name, _) = &obj_ty
            && !self.structs.contains_key(name)
            && self
                .predeclared_generated_tuple_arguments
                .contains_key(name)
        {
            let elements = tuple_elements(&obj_ty).ok_or_else(|| {
                TypeError::InvariantViolation(format!(
                    "predeclared generated Tuple '{name}' lost its element types"
                ))
            })?;
            let exact = self.eval_ct(index).map_err(|_| TypeError::TypeMismatch {
                expected: "a compile-time Int index".to_string(),
                found: "a runtime value".to_string(),
                context: format!("variadic struct '{name}' subscript"),
            })?;
            let k = exact.to_i64().ok_or_else(|| TypeError::TypeMismatch {
                expected: format!("a pack index in 0..{}", elements.len()),
                found: exact.to_string(),
                context: format!("variadic struct '{name}' subscript"),
            })?;
            if k < 0 || k as usize >= elements.len() {
                return Err(TypeError::TypeMismatch {
                    expected: format!("a pack index in 0..{}", elements.len()),
                    found: k.to_string(),
                    context: format!("variadic struct '{name}' subscript"),
                });
            }
            let element = elements[k as usize].clone();
            let value_receiver = !is_place_expr(object);
            let method = if value_receiver {
                if !self.is_implicitly_copyable(&element) {
                    return Err(TypeError::NonCopyable {
                        ty: obj_ty.to_string(),
                        context: "indexing an rvalue Tuple requires an implicitly copyable element"
                            .to_string(),
                    });
                }
                format!("__getitem_param_value__${k}")
            } else {
                format!("__getitem_param__${k}")
            };
            let target = format!("{name}.{method}");
            self.overload_targets
                .borrow_mut()
                .insert(span.clone(), target.clone());
            if value_receiver {
                let result_ty = match &element {
                    Ty::Ref(reference) => reference.referent.as_ref().clone(),
                    value => value.clone(),
                };
                self.selected_calls.borrow_mut().insert(
                    span,
                    crate::checked::CheckedCallContract {
                        target,
                        raises: None,
                        result_ty: result_ty.clone(),
                        receiver_requires_place: false,
                        receiver_convention: None,
                        arguments: Vec::new(),
                        captures: Vec::new(),
                        reference_result: None,
                        parameter_arguments: Vec::new(),
                        param_decls: Vec::new(),
                        boundary: crate::checked::CheckedCallBoundary::default(),
                    },
                );
                return Ok(result_ty);
            }
            let reference = match element {
                // The generated accessor for a reference-valued element
                // forwards that element's original handle rather than creating
                // an outer reference to the Tuple's private storage slot.
                Ty::Ref(reference) => reference,
                referent => {
                    let receiver = self.reference_actual(object)?;
                    crate::origin::RefTy {
                        referent: Box::new(referent),
                        origin: receiver.origin,
                        mutability: receiver.mutability,
                    }
                }
            };
            let result = (*reference.referent).clone();
            self.selected_calls.borrow_mut().insert(
                span.clone(),
                crate::checked::CheckedCallContract {
                    target,
                    raises: None,
                    result_ty: Ty::Ref(reference.clone()),
                    receiver_requires_place: true,
                    receiver_convention: Some(ArgConvention::Ref),
                    arguments: Vec::new(),
                    captures: Vec::new(),
                    reference_result: Some(reference.clone()),
                    parameter_arguments: Vec::new(),
                    param_decls: Vec::new(),
                    boundary: crate::checked::CheckedCallBoundary::default(),
                },
            );
            self.operation_adjustments.borrow_mut().insert(
                span,
                crate::checked::SemanticAdjustment::ReferenceResult { reference },
            );
            return Ok(result);
        }
        // Current Mojo permits a general compile-time parameter subscript hook,
        // not only the dependent accessor family synthesized for variadic
        // structs. Pass the source index as a checked value parameter and no
        // runtime argument; ordinary generic-method specialization then leaves
        // an exact callable target for HIR/MIR.
        if let Ty::Struct(name, _) = &obj_ty
            && self
                .structs
                .get(name)
                .is_some_and(|info| info.methods.contains_key("__getitem_param__"))
        {
            let parameter_arguments = [crate::ast::ParamArg::Value(index.clone())];
            let result = self.infer_method_call(
                span,
                object,
                "__getitem_param__",
                MethodCallArguments::parameterized(&parameter_arguments, &[], &[]),
            )?;
            return Ok(match result {
                Ty::Ref(reference) => *reference.referent,
                value => value,
            });
        }
        // A specialized variadic struct exposes one concrete dependent
        // accessor per pack element. Resolve it before the generic Tuple
        // element shortcut below: generated public Tuples retain nominal
        // element metadata too, but executable indexing must dispatch through
        // the checked ordinary method rather than a VM tuple intrinsic.
        if let Ty::Struct(name, _) = &obj_ty
            && let Some(info) = self.structs.get(name)
            && let Some(family) = dependent_index_accessor_family(info)
        {
            let count = (0..)
                .take_while(|k| info.methods.contains_key(&format!("{}${k}", family.place)))
                .count();
            let exact = self.eval_ct(index).map_err(|_| TypeError::TypeMismatch {
                expected: "a compile-time Int index".to_string(),
                found: "a runtime value".to_string(),
                context: format!("variadic struct '{name}' subscript"),
            })?;
            let k = exact.to_i64().ok_or_else(|| TypeError::TypeMismatch {
                expected: format!("a pack index in 0..{count}"),
                found: exact.to_string(),
                context: format!("variadic struct '{name}' subscript"),
            })?;
            if k < 0 || k as usize >= count {
                return Err(TypeError::TypeMismatch {
                    expected: format!("a pack index in 0..{count}"),
                    found: k.to_string(),
                    context: format!("variadic struct '{name}' subscript"),
                });
            }
            let value_receiver = !is_place_expr(object);
            let value_method = format!("{}${k}", family.value);
            let method = if value_receiver && info.methods.contains_key(&value_method) {
                value_method
            } else {
                format!("{}${k}", family.place)
            };
            let ret = self.infer_method_call(
                span.clone(),
                object,
                &method,
                MethodCallArguments::ordinary(&[], &[]),
            )?;
            self.overload_targets
                .borrow_mut()
                .insert(span, format!("{name}.{method}"));
            return Ok(ret);
        }
        // A tuple is heterogeneous, so its index must be a **compile-time** `Int`
        // constant — the result type is that element's type.
        let tuple_elements = tuple_elements(&obj_ty).or_else(|| match &obj_ty {
            // Compiler-private heterogeneous pack storage uses the same static
            // per-index typing rule as public Tuple, but never method dispatch.
            Ty::Tuple(elements) => Some(elements.iter().collect()),
            _ => None,
        });
        if let Some(elems) = tuple_elements {
            let exact = self.eval_ct(index).map_err(|_| TypeError::TypeMismatch {
                expected: "a compile-time Int index".to_string(),
                found: "a runtime value".to_string(),
                context: "tuple index".to_string(),
            })?;
            let i = exact.to_i64().ok_or_else(|| TypeError::TypeMismatch {
                expected: format!("a tuple index in 0..{}", elems.len()),
                found: exact.to_string(),
                context: "tuple index".to_string(),
            })?;
            if i < 0 || i as usize >= elems.len() {
                return Err(TypeError::TypeMismatch {
                    expected: format!("a tuple index in 0..{}", elems.len()),
                    found: i.to_string(),
                    context: "tuple index".to_string(),
                });
            }
            return Ok(match elems[i as usize] {
                Ty::Ref(reference) => (*reference.referent).clone(),
                element => element.clone(),
            });
        }
        // A homogeneous runtime variadic collector is compiler-private Tuple
        // storage with one repeated element type. Unlike a heterogeneous pack,
        // its index may vary at runtime without changing the result type.
        if let Ty::VariadicPack(element) = &obj_ty {
            let idx_ty = self.infer(index)?;
            if !self.is_index_type(&idx_ty) {
                return Err(TypeError::TypeMismatch {
                    expected: "Indexer".to_string(),
                    found: idx_ty.to_string(),
                    context: "variadic-pack index".to_string(),
                });
            }
            return Ok(match &**element {
                Ty::Ref(reference) => (*reference.referent).clone(),
                element => element.clone(),
            });
        }
        if let Ty::Struct(name, _) = &obj_ty
            && !self.structs.contains_key(name)
        {
            if let Some(element) = list_element(&obj_ty) {
                let idx_ty = self.infer(index)?;
                if !self.is_index_type(&idx_ty) {
                    return Err(TypeError::TypeMismatch {
                        expected: "Indexer".to_string(),
                        found: idx_ty.to_string(),
                        context: "index".to_string(),
                    });
                }
                self.record_interior_reference(span, object, "element");
                return Ok(match element.clone() {
                    Ty::Ref(reference) => *reference.referent,
                    element => element,
                });
            }
            if let Some((key, value)) = dict_elements(&obj_ty) {
                let idx_ty = self.infer(index)?;
                if !coerces(&idx_ty, key) {
                    return Err(TypeError::TypeMismatch {
                        expected: key.to_string(),
                        found: idx_ty.to_string(),
                        context: "dictionary key".to_string(),
                    });
                }
                self.record_replacing_interior_reference(span, object, "value");
                return Ok(match value.clone() {
                    Ty::Ref(reference) => *reference.referent,
                    value => value,
                });
            }
        }
        // A user struct with `__getitem__` is subscriptable: `c[i]` →
        // `c.__getitem__(i)`, typed by the method (the index need not be `Int`).
        if matches!(obj_ty, Ty::Struct(..)) {
            if let Ty::Struct(name, _) = &obj_ty
                && self
                    .structs
                    .get(name)
                    .is_some_and(|info| info.methods.contains_key("__getitem__"))
            {
                let result = self.infer_method_call(
                    span.clone(),
                    object,
                    "__getitem__",
                    MethodCallArguments::ordinary(std::slice::from_ref(index), &[]),
                )?;
                if list_element(&obj_ty).is_some() {
                    self.record_interior_reference(span, object, "element");
                } else if dict_elements(&obj_ty).is_some() {
                    self.record_replacing_interior_reference(span, object, "value");
                }
                // A source `ref[...] T` subscript result is a reference handle
                // only in a reference-valued context. Ordinary expression
                // inference reads through it to `T`; `RefDecl` and aggregate
                // storage recover the checked handle from the interior-origin
                // and place-type facts recorded above.
                return Ok(match result {
                    Ty::Ref(reference) => *reference.referent,
                    value => value,
                });
            }
            return Err(TypeError::NotIndexable(obj_ty.to_string()));
        }
        // Bounded type parameters use the same method-selection path as
        // concrete nominal receivers. This retains the requirement's error,
        // convention, capture, and reference-origin contract instead of
        // projecting only its parameter/result types.
        if matches!(obj_ty, Ty::Param { .. }) {
            return self.infer_method_call(
                span,
                object,
                "__getitem__",
                MethodCallArguments::ordinary(std::slice::from_ref(index), &[]),
            );
        }
        // The result of indexing: a SIMD lane, a List element, or a pointer pointee.
        let result = match &obj_ty {
            Ty::Simd { dtype, .. } => simd_ty(*dtype, 1),
            Ty::Pointer { element, origin } => {
                self.check_pointer_offset(origin, index)?;
                (**element).clone()
            }
            _ => return Err(TypeError::NotIndexable(obj_ty.to_string())),
        };
        let idx_ty = self.infer(index)?;
        if !self.is_index_type(&idx_ty) {
            return Err(TypeError::TypeMismatch {
                expected: "Indexer".to_string(),
                found: idx_ty.to_string(),
                context: "index".to_string(),
            });
        }
        Ok(match result {
            Ty::Ref(reference) => *reference.referent,
            other => other,
        })
    }

    /// An origin-bearing pointer designates exactly one checked value, so only
    /// offset 0 can be dereferenced; any other offset would be out-of-provenance
    /// access that Mojo leaves undefined.
    fn check_pointer_offset(
        &self,
        origin: &crate::origin::PointerOrigin,
        index: &Expr,
    ) -> Result<(), TypeError> {
        if origin.as_origin().is_none() {
            return Ok(());
        }
        match self.eval_ct(index) {
            Ok(value) if value.is_zero() => Ok(()),
            _ => Err(TypeError::Unsupported(
                "an origin-bearing UnsafePointer designates a single value; only \
                 offset 0 can be dereferenced"
                    .to_string(),
            )),
        }
    }

    /// Reject a write through an origin-bearing pointer whose provenance does
    /// not carry mutable capability. A symbolic parameter mutability is
    /// writable: storage coercion only admits mutable places into fields whose
    /// declared mutability is not explicitly immutable.
    fn check_pointer_write(&self, origin: &crate::origin::PointerOrigin) -> Result<(), TypeError> {
        let writable = match origin {
            crate::origin::PointerOrigin::Place { mutable, .. } => *mutable,
            crate::origin::PointerOrigin::Param { mutability, .. } => {
                *mutability != crate::origin::Mutability::Immutable
            }
            _ => return Ok(()),
        };
        if writable {
            Ok(())
        } else {
            Err(TypeError::Unsupported(
                "cannot write through an UnsafePointer with an immutable origin".to_string(),
            ))
        }
    }

    /// Whether a value can be normalized to the VM's index representation.
    /// Numeric literals/Int use the identity path; an opaque `Indexer` or a
    /// concrete conformer supplies `__mlir_index__() -> Int`.
    fn is_index_type(&self, ty: &Ty) -> bool {
        coerces(ty, &Ty::Int)
            || matches!(ty, Ty::Param { bounds, .. } if bounds.iter().any(|bound| bound == "Indexer"))
            || matches!(ty, Ty::Struct(..))
                && self.struct_dunder(ty, "__mlir_index__", &[]) == Some(Ok(Ty::Int))
    }

    /// Resolve the exact normalization method for a non-Int `Indexer`. The
    /// abstract trait symbol is retained for a bounded generic and retargeted by
    /// ordinary checked method dispatch once its runtime receiver is known.
    fn index_normalization_target(&self, ty: &Ty) -> Option<String> {
        match ty {
            Ty::Struct(name, arguments) => {
                let info = self.structs.get(name)?;
                let substitution = struct_subst(&info.decls, arguments);
                let methods = info.methods.get("__mlir_index__")?;
                let selected = methods.iter().find(|method| {
                    method.has_self
                        && method.params.is_empty()
                        && substitute(&method.ret, &substitution) == Ty::Int
                })?;
                Some(if methods.len() == 1 {
                    format!("{name}.__mlir_index__")
                } else {
                    method_lowered_name(name, "__mlir_index__", selected)
                })
            }
            Ty::Param { bounds, .. } => {
                let methods = self.lookup_trait_methods(bounds, "__mlir_index__", 0);
                let selected = methods
                    .iter()
                    .find(|method| method.params.is_empty() && method.ret == Ty::Int)?;
                Some(method_lowered_name(
                    "__trait_dispatch",
                    "__mlir_index__",
                    selected,
                ))
            }
            _ => None,
        }
    }

    fn has_index_normalization(&self, expression: &Expr, expected: &Ty) -> bool {
        *expected == Ty::Int
            && self
                .implicit_conversions
                .borrow()
                .get(&expression.source_span())
                .is_some_and(|target| crate::symbol::is_index_normalization_symbol(target))
    }

    /// Prepare a subscript argument for checked lowering. A nominal receiver
    /// normalizes an Indexer only when no `__getitem__` or `__setitem__`
    /// candidate accepts its source type at this argument position and an Int
    /// candidate exists there, preserving direct user overloads. Positions in a
    /// `*indices` tail use the variadic element contract. Primitive pointer/SIMD
    /// and homogeneous private-pack indexing has a fixed Int contract and
    /// therefore always records the checked normalization fallback.
    fn prepare_index_argument(
        &self,
        receiver: &Ty,
        expression: &Expr,
        method: &str,
        position: usize,
    ) -> Result<(), TypeError> {
        let actual = self.infer(expression)?;
        if coerces(&actual, &Ty::Int) {
            return Ok(());
        }
        let Some(target) = self.index_normalization_target(&actual) else {
            return Ok(());
        };
        let Ty::Struct(name, arguments) = receiver else {
            if matches!(
                receiver,
                Ty::Pointer { .. } | Ty::Simd { .. } | Ty::VariadicPack(_)
            ) {
                self.implicit_conversions
                    .borrow_mut()
                    .insert(expression.source_span(), target);
            }
            return Ok(());
        };
        let Some(info) = self.structs.get(name) else {
            return Ok(());
        };
        let Some(signatures) = info.methods.get(method) else {
            return Ok(());
        };
        let substitution = struct_subst(&info.decls, arguments);
        let mut accepts_source = false;
        let mut accepts_int = false;
        for signature in signatures.iter().filter(|signature| signature.has_self) {
            let parameter = match signature.variadic_index {
                Some(variadic_index) if position >= variadic_index => signature.variadic.as_deref(),
                _ => signature.params.get(position),
            };
            let Some(parameter) = parameter else {
                continue;
            };
            let expected = substitute(parameter, &substitution);
            accepts_source |= self.value_coerces(&actual, &expected)
                || self
                    .implicit_conversion_target(&actual, &expected)?
                    .is_some();
            accepts_int |= expected == Ty::Int;
        }
        if !accepts_source && accepts_int {
            self.implicit_conversions
                .borrow_mut()
                .insert(expression.source_span(), target);
        }
        Ok(())
    }

    /// Type a field access `object.field`. On a generic struct value the field
    /// type has the struct's type arguments substituted in (`Pair[Int].left :
    /// Int`).
    fn infer_member(&self, object: &Expr, field: &str) -> Result<Ty, TypeError> {
        // `Self.n` reads the enclosing struct's value parameter (an `Int`).
        if let ExprKind::Identifier(s) = &object.kind
            && s == "Self"
        {
            if let Some(self_ty) = &self.self_ty {
                self.expression_types
                    .borrow_mut()
                    .insert(object.source_span(), self_ty.clone());
            }
            return match self.self_decls.iter().find(|d| d.name() == field) {
                Some(ParamDecl::Value { .. }) => Ok(Ty::Int),
                _ => Err(TypeError::UnknownSelfParam(field.to_string())),
            };
        }
        // `T.size` where `T` is a generic type parameter and a bound trait
        // requires `comptime size: Int`: expression-level access to an associated
        // compile-time value. Type-valued associated members remain type-position
        // only (`T.Element` in annotations).
        if let ExprKind::Identifier(name) = &object.kind
            && let Some(parameter) = self.lookup_tparam(name)
            && let Ty::Param { bounds, .. } = &parameter
            && let Some(ty) = self.lookup_trait_assoc_value_ty(bounds, field)
        {
            self.expression_types
                .borrow_mut()
                .insert(object.source_span(), parameter);
            return Ok(ty);
        }
        let obj_ty = self.infer(object)?;
        // Projecting a field through a reference-returning expression borrows
        // that referent for the projection; it does not first create an owned
        // copy of the whole value. Retain the handle exactly as method and
        // chained-subscript receivers do, including for linear referents.
        if let Some(reference) = self.infer_reference_value(object) {
            self.reference_value_uses.borrow_mut().insert(
                object.source_span(),
                reference.mutability == crate::origin::Mutability::Mutable,
            );
        }
        if matches!(&obj_ty, Ty::Struct(name, args) if matches!(name.as_str(), "Slice" | "ContiguousSlice" | "StridedSlice") && args.is_empty())
            && matches!(field, "start" | "end" | "step")
        {
            return Ok(Ty::Struct("Optional".to_string(), vec![TyArg::Ty(Ty::Int)]));
        }
        if let Ty::Struct(sname, targs) = &obj_ty {
            let info = self.structs.get(sname).ok_or_else(|| {
                TypeError::InvariantViolation(format!("struct '{sname}' was not registered"))
            })?;
            if let Some((_, fty)) = info.fields.iter().find(|(n, _)| n == field) {
                let subst = struct_subst(&info.decls, targs);
                return Ok(match substitute(fty, &subst) {
                    Ty::Ref(reference) => *reference.referent,
                    value => value,
                });
            }
        }
        Err(TypeError::NoSuchField {
            object_type: obj_ty.to_string(),
            field: field.to_string(),
        })
    }

    /// Type a method call `object.method(args)`. On a generic struct value the
    /// method's parameter and return types are substituted at the receiver's
    /// type arguments; on a bounded type parameter (`x: T` with `T: SomeTrait`)
    /// the method is resolved from the bound trait's requirement, with `Self`
    /// substituted to `T`.
    fn infer_method_call(
        &self,
        span: SourceSpan,
        object: &Expr,
        method: &str,
        call: MethodCallArguments<'_>,
    ) -> Result<Ty, TypeError> {
        let MethodCallArguments {
            param_args,
            args,
            kwargs,
            parameterized_syntax,
            preserves_receiver_interiors,
        } = call;
        // A **static** method on a parameterized built-in type — the receiver is a
        // type, not a value (`UnsafePointer[T].alloc(n)`). Handled before inferring
        // the object (which would reject a bare `TypeApply`).
        if let ExprKind::TypeApply { name, args: targs } = &object.kind {
            reject_kwargs(kwargs)?;
            return self.infer_static_method(name, targs, method, args);
        }
        if let ExprKind::Identifier(sname) = &object.kind
            && let Some(info) = self.structs.get(sname)
            && let Some(signatures) = info.methods.get(method)
        {
            let mut matches = Vec::new();
            for sig in signatures.iter().filter(|sig| !sig.has_self) {
                let (params, variadic, kw_variadic, method_subst, method_arguments) = match self
                    .instantiate_method_generics(
                        &format!("{sname}.{method}"),
                        sig,
                        &sig.params,
                        sig.variadic.as_deref(),
                        sig.kw_variadic.as_deref(),
                        param_args,
                        args,
                        kwargs,
                    ) {
                    Ok(instantiated) => instantiated,
                    Err(_) => continue,
                };
                if !self.method_constraints_apply(sig, &method_arguments) {
                    continue;
                }
                if let Ok(scored) = self.score_method_call(
                    sig,
                    &params,
                    variadic.as_ref(),
                    kw_variadic.as_ref(),
                    args,
                    kwargs,
                ) {
                    matches.push(MethodCallResolution {
                        conversion_score: scored.rank,
                        slots: scored.slots,
                        positional_overflow: scored.positional_overflow,
                        keyword_overflow: scored.keyword_overflow,
                        variadic_element: variadic.clone(),
                        keyword_element: kw_variadic.clone(),
                        conventions: sig.conventions.clone(),
                        self_convention: sig.self_convention,
                        return_type: substitute(&sig.ret, &method_subst),
                        raises: sig.raises,
                        error: sig
                            .error
                            .as_ref()
                            .map(|error| Box::new(substitute(error, &method_subst))),
                        mutates_receiver: false,
                        consumes_receiver: false,
                        lowered_name: if signatures.len() > 1 {
                            Some(method_lowered_name(sname, method, sig))
                        } else if parameterized_syntax {
                            Some(format!("{sname}.{method}"))
                        } else {
                            None
                        },
                        ref_params: sig.ref_params.clone(),
                        ref_return: sig.ref_return.clone(),
                        param_types: params,
                        param_decls: sig.decls.clone(),
                    });
                }
            }
            if !matches.is_empty() {
                let selected =
                    select_method_overload(method, matches).map_err(|kind| TypeError::BadCall {
                        func: format!("{sname}.{method}"),
                        reason: match kind {
                            OverloadSelect::NoMatch => "no overload matches the supplied arguments",
                            OverloadSelect::Ambiguous => "ambiguous overloaded call",
                        }
                        .to_string(),
                    })?;
                self.record_selected_method_conversions(method, &selected, args, kwargs)?;
                if let Some(target) = selected.lowered_name {
                    self.overload_targets
                        .borrow_mut()
                        .insert(span.clone(), target);
                }
                if selected.raises {
                    let error = selected.error.as_deref().cloned().unwrap_or(Ty::Error);
                    self.record_call_effect(span.clone(), error.clone());
                    self.require_error(
                        format!("call to raising method '{sname}.{method}'"),
                        error,
                    )?;
                }
                return Ok(selected.return_type);
            }
        }
        let obj_ty = self.infer(object)?;
        if let Ty::Struct(name, _) = &obj_ty
            && !self.structs.contains_key(name)
        {
            if let Some(element) = list_element(&obj_ty) {
                reject_kwargs(kwargs)?;
                let result = self.infer_list_method(object, method, element, args)?;
                if matches!(
                    method,
                    "append" | "insert" | "remove" | "pop" | "clear" | "reverse" | "extend"
                ) {
                    self.record_interior_invalidation(span.clone(), object);
                }
                return Ok(result);
            }
            if let Some(element) = set_element(&obj_ty) {
                reject_kwargs(kwargs)?;
                return match method {
                    "add" => {
                        self.check_place(object)?;
                        let values = self.builtin_args("Set.add", 1, args)?;
                        if !coerces(&values[0], element) {
                            return Err(TypeError::TypeMismatch {
                                expected: element.to_string(),
                                found: values[0].to_string(),
                                context: "Set.add value".to_string(),
                            });
                        }
                        self.check_consuming(&args[0], &values[0], "Set.add value")?;
                        Ok(Ty::None)
                    }
                    _ => Err(TypeError::NoSuchMethod {
                        object_type: obj_ty.to_string(),
                        method: method.to_string(),
                    }),
                };
            }
            if let Some(elements) = tuple_elements(&obj_ty) {
                reject_kwargs(kwargs)?;
                let elements = elements.into_iter().cloned().collect::<Vec<_>>();
                return self.infer_tuple_method(&span, object, method, &elements, call);
            }
        }
        if matches!(&obj_ty, Ty::Struct(name, args) if matches!(name.as_str(), "Slice" | "ContiguousSlice" | "StridedSlice") && args.is_empty())
        {
            reject_kwargs(kwargs)?;
            if method != "indices" {
                return Err(TypeError::NoSuchMethod {
                    object_type: obj_ty.to_string(),
                    method: method.to_string(),
                });
            }
            let types = self.builtin_args("Slice.indices", 1, args)?;
            if !coerces(&types[0], &Ty::Int) {
                return Err(TypeError::TypeMismatch {
                    expected: "Int".to_string(),
                    found: types[0].to_string(),
                    context: "Slice.indices length".to_string(),
                });
            }
            return Ok(self.public_tuple_type(vec![Ty::Int, Ty::Int, Ty::Int]));
        }
        if matches!(&obj_ty, Ty::Struct(name, args) if name == "Optional" && matches!(args.as_slice(), [TyArg::Ty(Ty::Int)]))
        {
            reject_kwargs(kwargs)?;
            return match method {
                "is_some" if args.is_empty() => Ok(Ty::Bool),
                "or_else" => {
                    let types = self.builtin_args("Optional.or_else", 1, args)?;
                    if coerces(&types[0], &Ty::Int) {
                        Ok(Ty::Int)
                    } else {
                        Err(TypeError::TypeMismatch {
                            expected: "Int".to_string(),
                            found: types[0].to_string(),
                            context: "Optional.or_else default".to_string(),
                        })
                    }
                }
                _ => Err(TypeError::NoSuchMethod {
                    object_type: obj_ty.to_string(),
                    method: method.to_string(),
                }),
            };
        }
        if self.conforms_to(&obj_ty, "Writer") && method == "write" {
            reject_kwargs(kwargs)?;
            self.check_place(object)?;
            self.infer_print(args)?;
            return Ok(Ty::None);
        }
        if matches!(&obj_ty, Ty::Param { bounds, .. } if bounds.iter().any(|bound| bound == "Hasher"))
            && method == "update"
        {
            reject_kwargs(kwargs)?;
            self.check_place(object)?;
            let tys = self.builtin_args("Hasher.update", 1, args)?;
            if !self.conforms_to(&tys[0], "Hashable") {
                return Err(TypeError::TraitNotSatisfied {
                    param: "T".to_string(),
                    ty: tys[0].to_string(),
                    trait_name: "Hashable".to_string(),
                    reason: self.trait_failure_reason(&tys[0], "Hashable"),
                });
            }
            return Ok(Ty::None);
        }
        if obj_ty == Ty::String && method == "format" {
            reject_kwargs(kwargs)?;
            self.infer_print(args)?;
            return Ok(Ty::String);
        }
        if let Ty::Tuple(elements) = &obj_ty {
            reject_kwargs(kwargs)?;
            return self.infer_tuple_method(&span, object, method, elements, call);
        }
        // Built-in `UnsafePointer` methods. Raw storage take/destroy are
        // checker-gated compiler-private operations; ordinary user code only
        // sees the public pointer surface.
        if let Ty::Pointer {
            element: elem,
            origin,
        } = &obj_ty
        {
            reject_kwargs(kwargs)?;
            return self.infer_pointer_method(&span, object, method, elem, origin, args);
        }
        // Resolve the method to a concrete signature (params + return + whether
        // it mutates `self`) for this receiver, substituting the receiver's type
        // arguments (struct) or `Self` (a bounded type parameter's trait method).
        let resolved: Result<Option<MethodCallResolution>, OverloadSelect> = match &obj_ty {
            Ty::Struct(sname, targs) => {
                let info = self.structs.get(sname).ok_or_else(|| {
                    TypeError::InvariantViolation(format!("struct '{sname}' was not registered"))
                })?;
                match info.methods.get(method) {
                    Some(sigs) => {
                        let overloaded = sigs.len() > 1;
                        let subst = struct_subst(&info.decls, targs);
                        let mut matches = Vec::new();
                        for sig in sigs {
                            let receiver_params: Vec<Ty> =
                                sig.params.iter().map(|t| substitute(t, &subst)).collect();
                            let receiver_variadic =
                                sig.variadic.as_ref().map(|ty| substitute(ty, &subst));
                            let receiver_kw_variadic =
                                sig.kw_variadic.as_ref().map(|ty| substitute(ty, &subst));
                            let Ok((
                                params,
                                variadic,
                                kw_variadic,
                                method_subst,
                                mut method_arguments,
                            )) = self.instantiate_method_generics(
                                &format!("{sname}.{method}"),
                                sig,
                                &receiver_params,
                                receiver_variadic.as_ref(),
                                receiver_kw_variadic.as_ref(),
                                param_args,
                                args,
                                kwargs,
                            )
                            else {
                                continue;
                            };
                            for (decl, argument) in info.decls.iter().zip(targs) {
                                method_arguments.insert(
                                    decl.name().trim_start_matches('*').to_string(),
                                    argument.clone(),
                                );
                            }
                            if !self.method_constraints_apply(sig, &method_arguments) {
                                continue;
                            }
                            if let Ok(scored) = self.score_method_call(
                                sig,
                                &params,
                                variadic.as_ref(),
                                kw_variadic.as_ref(),
                                args,
                                kwargs,
                            ) {
                                matches.push(MethodCallResolution {
                                    conversion_score: scored.rank,
                                    slots: scored.slots,
                                    positional_overflow: scored.positional_overflow,
                                    keyword_overflow: scored.keyword_overflow,
                                    variadic_element: variadic.clone(),
                                    keyword_element: kw_variadic.clone(),
                                    conventions: sig.conventions.clone(),
                                    self_convention: sig.self_convention,
                                    return_type: substitute(
                                        &substitute(&sig.ret, &subst),
                                        &method_subst,
                                    ),
                                    raises: sig.raises,
                                    error: sig.error.as_ref().map(|error| {
                                        Box::new(substitute(
                                            &substitute(error, &subst),
                                            &method_subst,
                                        ))
                                    }),
                                    mutates_receiver: matches!(
                                        sig.self_convention,
                                        Some(crate::ast::ArgConvention::Mut)
                                    ),
                                    consumes_receiver: matches!(
                                        sig.self_convention,
                                        Some(
                                            crate::ast::ArgConvention::Var
                                                | crate::ast::ArgConvention::Deinit
                                        )
                                    ),
                                    lowered_name: if overloaded {
                                        Some(method_lowered_name(sname, method, sig))
                                    } else if parameterized_syntax {
                                        Some(format!("{sname}.{method}"))
                                    } else {
                                        None
                                    },
                                    ref_params: sig.ref_params.clone(),
                                    ref_return: sig.ref_return.clone(),
                                    param_types: params,
                                    param_decls: sig.decls.clone(),
                                });
                            }
                        }
                        select_method_overload(method, matches).map(Some)
                    }
                    None => Ok(None),
                }
            }
            Ty::Param { bounds, .. } => {
                let signatures = self.lookup_trait_methods(bounds, method, args.len());
                if signatures.is_empty() {
                    return Err(TypeError::NoSuchMethod {
                        object_type: obj_ty.to_string(),
                        method: method.to_string(),
                    });
                }
                let mut matches = Vec::new();
                for sig in signatures {
                    let receiver_params: Vec<_> = sig
                        .params
                        .iter()
                        .map(|ty| substitute_self(ty, &obj_ty))
                        .collect();
                    let receiver_variadic = sig
                        .variadic
                        .as_deref()
                        .map(|ty| substitute_self(ty, &obj_ty));
                    let receiver_kw_variadic = sig
                        .kw_variadic
                        .as_deref()
                        .map(|ty| substitute_self(ty, &obj_ty));
                    let Ok((params, variadic, kw_variadic, method_subst, method_arguments)) = self
                        .instantiate_method_generics(
                            &format!("{obj_ty}.{method}"),
                            &sig,
                            &receiver_params,
                            receiver_variadic.as_ref(),
                            receiver_kw_variadic.as_ref(),
                            param_args,
                            args,
                            kwargs,
                        )
                    else {
                        continue;
                    };
                    if !self.method_constraints_apply(&sig, &method_arguments) {
                        continue;
                    }
                    let Ok(scored) = self.score_method_call(
                        &sig,
                        &params,
                        variadic.as_ref(),
                        kw_variadic.as_ref(),
                        args,
                        kwargs,
                    ) else {
                        continue;
                    };
                    matches.push(MethodCallResolution {
                        conversion_score: scored.rank,
                        slots: scored.slots,
                        positional_overflow: scored.positional_overflow,
                        keyword_overflow: scored.keyword_overflow,
                        variadic_element: variadic.clone(),
                        keyword_element: kw_variadic.clone(),
                        conventions: sig.conventions.clone(),
                        self_convention: sig.self_convention,
                        return_type: self.resolve_assoc_ty(&substitute(
                            &substitute_self(&sig.ret, &obj_ty),
                            &method_subst,
                        )),
                        raises: sig.raises,
                        error: sig.error.as_ref().map(|error| {
                            Box::new(self.resolve_assoc_ty(&substitute(
                                &substitute_self(error, &obj_ty),
                                &method_subst,
                            )))
                        }),
                        mutates_receiver: matches!(
                            sig.self_convention,
                            Some(crate::ast::ArgConvention::Mut)
                        ),
                        consumes_receiver: matches!(
                            sig.self_convention,
                            Some(
                                crate::ast::ArgConvention::Var | crate::ast::ArgConvention::Deinit
                            )
                        ),
                        lowered_name: Some(method_lowered_name("__trait_dispatch", method, &sig)),
                        ref_params: sig.ref_params.clone(),
                        ref_return: sig.ref_return.clone(),
                        param_types: params,
                        param_decls: sig.decls.clone(),
                    });
                }
                select_method_overload(method, matches).map(Some)
            }
            // `x.__hash__()` on a concrete built-in hashable type (`Int`, `String`,
            // …) is an intrinsic returning `UInt` — lets a key struct combine
            // `self.field.__hash__()` values (roadmap milestone 6).
            _ if method == "__hash__"
                && args.is_empty()
                && (builtin_hashable_ty(&obj_ty)
                    || matches!(&obj_ty, Ty::Variant(alternatives) if alternatives.iter().all(|alternative| self.is_hashable(alternative)))) =>
            {
                Ok(Some(MethodCallResolution {
                    conversion_score: 0,
                    slots: vec![],
                    positional_overflow: vec![],
                    keyword_overflow: vec![],
                    variadic_element: None,
                    keyword_element: None,
                    conventions: vec![],
                    self_convention: None,
                    return_type: Ty::UInt,
                    raises: false,
                    error: None,
                    mutates_receiver: false,
                    consumes_receiver: false,
                    lowered_name: None,
                    ref_params: vec![],
                    ref_return: None,
                    param_types: vec![],
                    param_decls: vec![],
                }))
            }
            _ => Ok(None),
        };
        let resolved = match resolved {
            Ok(Some(resolved)) => resolved,
            Ok(None) => {
                return Err(TypeError::NoSuchMethod {
                    object_type: obj_ty.to_string(),
                    method: method.to_string(),
                });
            }
            Err(OverloadSelect::NoMatch) => {
                return Err(TypeError::BadCall {
                    func: method.to_string(),
                    reason: "no overload matches the supplied arguments".to_string(),
                });
            }
            Err(OverloadSelect::Ambiguous) => {
                return Err(TypeError::BadCall {
                    func: method.to_string(),
                    reason: "ambiguous overloaded method call".to_string(),
                });
            }
        };
        if parameterized_syntax {
            self.operation_adjustments.borrow_mut().insert(
                span.clone(),
                crate::checked::SemanticAdjustment::ParameterizedMethodCall {
                    param_decls: resolved.param_decls.clone(),
                },
            );
        }
        let boundary_before = self.call_boundary_snapshot(&span, args, kwargs);
        self.record_selected_method_conversions(method, &resolved, args, kwargs)?;
        let call_error = resolved
            .raises
            .then(|| resolved.error.as_deref().cloned().unwrap_or(Ty::Error));
        if let Some(error) = &call_error {
            self.record_call_effect(span.clone(), error.clone());
            self.require_error(format!("call to raising method '{method}'"), error.clone())?;
        }
        let selected_target = resolved.lowered_name.clone().or_else(|| match &obj_ty {
            Ty::Struct(name, _) if self.structs.contains_key(name) => {
                Some(format!("{name}.{method}"))
            }
            _ => None,
        });
        if let Some(target) = &selected_target {
            self.overload_targets
                .borrow_mut()
                .insert(span.clone(), target.clone());
        }
        // A `mut self` method mutates its receiver, so the receiver must be a
        // writable place (the mutation is written back to it): a variable, a
        // field/index chain, or `self` in a `mut self` method.
        if resolved.mutates_receiver {
            let returned_reference = self
                .operation_adjustments
                .borrow()
                .get(&object.source_span())
                .and_then(|adjustment| match adjustment {
                    crate::checked::SemanticAdjustment::ReferenceResult { reference } => {
                        Some(reference.clone())
                    }
                    _ => None,
                });
            if let Some(reference) = returned_reference {
                if reference.mutability != crate::origin::Mutability::Mutable {
                    return Err(TypeError::ImmutableBinding(
                        "reference-returning method receiver".to_string(),
                    ));
                }
            } else {
                self.check_place(object)?;
            }
            if !preserves_receiver_interiors {
                self.record_interior_invalidation(span.clone(), object);
            }
        }
        let effective_receiver_convention = if resolved.self_convention == Some(ArgConvention::Ref)
            && self.reference_actual(object)?.mutability == crate::origin::Mutability::Immutable
        {
            Some(ArgConvention::Read)
        } else {
            resolved.self_convention
        };
        // A `deinit self` call always consumes its receiver. Mojo may satisfy
        // that consumption by implicitly copying an `ImplicitlyCopyable` place;
        // a merely movable (or explicitly-copy-only) place still requires `^`.
        if resolved.consumes_receiver && is_place_expr(object) {
            if !self.is_implicitly_copyable(&obj_ty) {
                return Err(TypeError::NonCopyable {
                    ty: obj_ty.to_string(),
                    context: format!(
                        "consuming receiver of method '{method}' must be transferred with '^'"
                    ),
                });
            }
            self.implicitly_copied_consuming_receivers
                .borrow_mut()
                .insert(span.clone());
        }
        if resolved.consumes_receiver
            && let Ty::Struct(name, _) = &obj_ty
            && self
                .structs
                .get(name)
                .is_some_and(|info| info.explicit_destructors.contains_key(method))
        {
            self.explicit_destroy_calls
                .borrow_mut()
                .insert(span.clone());
        }
        for (index, slot) in resolved.slots.iter().enumerate() {
            let expression = match slot {
                ArgSlot::Positional(position) => &args[*position],
                ArgSlot::Keyword(position) => &kwargs[*position].value,
                ArgSlot::Default => continue,
            };
            let ty = self.infer_with_expected(
                expression,
                resolved
                    .param_types
                    .get(index)
                    .expect("selected method slot has a parameter type"),
                true,
            )?;
            match resolved.conventions.get(index).copied().flatten() {
                Some(ArgConvention::Deinit)
                    if is_place_expr(expression) && !self.is_implicitly_copyable(&ty) =>
                {
                    return Err(TypeError::NonCopyable {
                        ty: ty.to_string(),
                        context: format!(
                            "deinit argument {} to method '{}' must be transferred with '^'",
                            index + 1,
                            method
                        ),
                    });
                }
                Some(ArgConvention::Var | ArgConvention::Deinit) => {
                    self.check_consuming(
                        expression,
                        &ty,
                        &format!("argument {} to method '{}'", index + 1, method),
                    )?;
                }
                _ => {}
            }
        }
        let (effective_conventions, solved_return) = self.solve_call_origins(
            &resolved.slots,
            &resolved.conventions,
            &resolved.ref_params,
            resolved.ref_return.as_ref(),
            args,
            kwargs,
        )?;
        let copied_reads = resolved
            .slots
            .iter()
            .enumerate()
            .map(|(index, slot)| {
                let expression = match slot {
                    ArgSlot::Positional(position) => &args[*position],
                    ArgSlot::Keyword(position) => &kwargs[*position].value,
                    ArgSlot::Default => return Ok(false),
                };
                let convention = effective_conventions.get(index).copied().flatten();
                Ok(
                    !matches!(convention, Some(ArgConvention::Mut | ArgConvention::Ref))
                        && self.is_copyable(
                            &self.infer_with_expected(
                                expression,
                                resolved
                                    .param_types
                                    .get(index)
                                    .expect("selected method slot has a parameter type"),
                                true,
                            )?,
                        ),
                )
            })
            .collect::<Result<Vec<_>, TypeError>>()?;
        check_call_aliasing(
            &resolved.slots,
            &effective_conventions,
            &copied_reads,
            args,
            kwargs,
        )?;
        check_receiver_aliasing(
            object,
            resolved.self_convention,
            &resolved.slots,
            &copied_reads,
            args,
            kwargs,
        )?;
        let reference_result = if let Some(signature) = &resolved.ref_return {
            let actual: Vec<_> = resolved
                .slots
                .iter()
                .map(|slot| match slot {
                    ArgSlot::Positional(position) => self
                        .reference_actual(&args[*position])
                        .ok()
                        .map(|reference| reference.origin),
                    ArgSlot::Keyword(position) => self
                        .reference_actual(&kwargs[*position].value)
                        .ok()
                        .map(|reference| reference.origin),
                    ArgSlot::Default => None,
                })
                .collect();
            let self_reference = self.reference_actual(object)?;
            let origin = substitute_sig_origin_with_self(
                &signature.origin,
                &actual,
                Some(self_reference.origin),
            );
            let mutable = match signature.mutability {
                crate::origin::SigMutability::Immutable => crate::origin::Mutability::Immutable,
                crate::origin::SigMutability::Mutable => crate::origin::Mutability::Mutable,
                _ if self_reference.mutability == crate::origin::Mutability::Mutable
                    || solved_return.is_some_and(|reference| {
                        reference.mutability == crate::origin::Mutability::Mutable
                    }) =>
                {
                    crate::origin::Mutability::Mutable
                }
                _ => crate::origin::Mutability::Immutable,
            };
            let reference = crate::origin::RefTy {
                referent: Box::new(resolved.return_type.clone()),
                origin,
                mutability: mutable,
            };
            self.operation_adjustments.borrow_mut().insert(
                span.clone(),
                crate::checked::SemanticAdjustment::ReferenceResult {
                    reference: reference.clone(),
                },
            );
            Some(reference)
        } else {
            None
        };

        let boundary = self.checked_call_boundary(&span, args, kwargs, &boundary_before);

        // Retain the complete selected-call payload independently of the
        // compatibility adjustment slot.  This is the authoritative handoff
        // for nominal subscripts, and lets reference results coexist with
        // descriptor and capture metadata at one source expression.
        if let Some(target) = selected_target {
            use crate::checked::{CheckedCallArgument, CheckedCallArgumentSource};
            let mut arguments = resolved
                .slots
                .iter()
                .enumerate()
                .map(|(index, slot)| CheckedCallArgument {
                    source: match slot {
                        ArgSlot::Positional(position) => {
                            CheckedCallArgumentSource::Positional(*position)
                        }
                        ArgSlot::Keyword(position) => CheckedCallArgumentSource::Keyword(*position),
                        ArgSlot::Default => CheckedCallArgumentSource::Default,
                    },
                    parameter_ty: resolved
                        .param_types
                        .get(index)
                        .cloned()
                        .unwrap_or(Ty::Error),
                    requires_place: matches!(
                        resolved.conventions.get(index).copied().flatten(),
                        Some(ArgConvention::Mut | ArgConvention::Ref)
                    ),
                    convention: effective_conventions.get(index).copied().flatten(),
                })
                .collect::<Vec<_>>();
            if let Some(element) = &resolved.variadic_element {
                arguments.extend(resolved.positional_overflow.iter().enumerate().map(
                    |(pack_index, position)| CheckedCallArgument {
                        source: CheckedCallArgumentSource::Positional(*position),
                        parameter_ty: match element {
                            Ty::RuntimePack(elements) => {
                                elements.get(pack_index).cloned().unwrap_or(Ty::Error)
                            }
                            _ => element.clone(),
                        },
                        requires_place: false,
                        convention: None,
                    },
                ));
            }
            if let Some(element) = &resolved.keyword_element {
                arguments.extend(resolved.keyword_overflow.iter().map(|position| {
                    CheckedCallArgument {
                        source: CheckedCallArgumentSource::Keyword(*position),
                        parameter_ty: element.clone(),
                        requires_place: false,
                        convention: None,
                    }
                }));
            }
            let argument_types = args
                .iter()
                .chain(kwargs.iter().map(|argument| &argument.value))
                .filter_map(|expression| {
                    self.expression_types
                        .borrow()
                        .get(&expression.source_span())
                        .cloned()
                })
                .collect::<Vec<_>>();
            let captures = self.call_capture_effects(&argument_types);
            let parameter_arguments = param_args
                .iter()
                .filter_map(|argument| {
                    let (name, argument) = match argument {
                        crate::ast::ParamArg::Named { name, value } => {
                            (Some(name.clone()), value.as_ref())
                        }
                        argument => (None, argument),
                    };
                    let value_source = match argument {
                        crate::ast::ParamArg::Type(_) => None,
                        crate::ast::ParamArg::Value(expression) => {
                            let erased = self
                                .operation_adjustments
                                .borrow()
                                .get(&expression.source_span())
                                .is_some_and(|adjustment| {
                                    matches!(
                                        adjustment,
                                        crate::checked::SemanticAdjustment::EraseCompileTimeArgument
                                    )
                                });
                            if erased {
                                return None;
                            }
                            Some(expression.source_span())
                        }
                        crate::ast::ParamArg::Named { .. } => unreachable!(),
                    };
                    Some(crate::checked::CheckedCallParameterArgument { name, value_source })
                })
                .collect();
            if reference_result.is_none() && !captures.is_empty() {
                self.operation_adjustments.borrow_mut().insert(
                    span.clone(),
                    crate::checked::SemanticAdjustment::CallableCaptureAccesses(captures.clone()),
                );
            }
            self.selected_calls.borrow_mut().insert(
                span,
                crate::checked::CheckedCallContract {
                    target,
                    raises: call_error,
                    result_ty: reference_result
                        .clone()
                        .map(Ty::Ref)
                        .unwrap_or_else(|| resolved.return_type.clone()),
                    receiver_requires_place: matches!(
                        resolved.self_convention,
                        Some(ArgConvention::Mut | ArgConvention::Ref)
                    ),
                    receiver_convention: effective_receiver_convention,
                    arguments,
                    captures,
                    reference_result: reference_result.clone(),
                    parameter_arguments,
                    param_decls: resolved.param_decls.clone(),
                    boundary,
                },
            );
        }
        Ok(reference_result
            .map(|reference| *reference.referent)
            .unwrap_or(resolved.return_type))
    }

    /// Apply the implicit conversions selected while scoring one concrete method
    /// overload. Keyword-overflow arguments are materialized into the callee's
    /// `StringDict`, so their conversions must be recorded just like conversions
    /// for ordinary parameter slots.
    fn record_selected_method_conversions(
        &self,
        method: &str,
        resolved: &MethodCallResolution,
        args: &[Expr],
        kwargs: &[crate::ast::KwArg],
    ) -> Result<(), TypeError> {
        for (index, slot) in resolved.slots.iter().enumerate() {
            let expression = match slot {
                ArgSlot::Positional(position) => &args[*position],
                ArgSlot::Keyword(position) => &kwargs[*position].value,
                ArgSlot::Default => continue,
            };
            if let Some(expected) = resolved.param_types.get(index) {
                let actual = self.infer_with_expected(expression, expected, true)?;
                if !self.has_index_normalization(expression, expected)
                    && !self.record_implicit_conversion(expression, &actual, expected)?
                {
                    return Err(TypeError::TypeMismatch {
                        expected: expected.to_string(),
                        found: actual.to_string(),
                        context: format!("argument {} to method '{method}'", index + 1),
                    });
                }
            }
        }
        if let Some(expected) = &resolved.keyword_element {
            for &position in &resolved.keyword_overflow {
                let expression = &kwargs[position].value;
                let actual = self.infer(expression)?;
                if !self.record_implicit_conversion(expression, &actual, expected)? {
                    return Err(TypeError::TypeMismatch {
                        expected: expected.to_string(),
                        found: actual.to_string(),
                        context: format!(
                            "keyword '{}' collected by method '{method}'",
                            kwargs[position].name
                        ),
                    });
                }
            }
        }
        Ok(())
    }

    fn call_boundary_snapshot(
        &self,
        span: &SourceSpan,
        args: &[Expr],
        kwargs: &[crate::ast::KwArg],
    ) -> CallBoundarySnapshot {
        let invalidations = self.interior_invalidations.borrow();
        let mut before = HashMap::new();
        for source in std::iter::once(span.clone())
            .chain(args.iter().map(Expr::source_span))
            .chain(kwargs.iter().map(|argument| argument.value.source_span()))
        {
            before
                .entry(source.clone())
                .or_insert_with(|| invalidations.get(&source).cloned().unwrap_or_default());
        }
        CallBoundarySnapshot {
            invalidations: before,
        }
    }

    /// Freeze the value adaptations and generation changes belonging to one
    /// selected call. A later call may reuse the same source occurrence (the
    /// getter/setter pair of augmented subscript assignment), so these facts must
    /// travel with the call contract rather than remain only in source-keyed maps.
    fn checked_call_boundary(
        &self,
        span: &SourceSpan,
        args: &[Expr],
        kwargs: &[crate::ast::KwArg],
        before: &CallBoundarySnapshot,
    ) -> crate::checked::CheckedCallBoundary {
        use crate::checked::{
            CheckedCallArgumentBoundary, CheckedCallArgumentSource, CheckedCallBoundary,
            CheckedCallValueAdjustment,
        };

        let overloads = self.overload_targets.borrow();
        let implicit = self.implicit_conversions.borrow();
        let operations = self.operation_adjustments.borrow();
        let expression_types = self.expression_types.borrow();
        let invalidations = self.interior_invalidations.borrow();
        let argument =
            |source: CheckedCallArgumentSource, expression: &Expr| -> CheckedCallArgumentBoundary {
                let value_source = expression.source_span();
                let adjustments =
                    if matches!(expression_types.get(&value_source), Some(Ty::Overload(_)))
                        && let Some(target) = overloads.get(&value_source)
                    {
                        vec![CheckedCallValueAdjustment::ResolveCallable {
                            target: target.clone(),
                        }]
                    } else if let Some(target) = implicit.get(&value_source) {
                        if crate::symbol::is_index_normalization_symbol(target) {
                            vec![CheckedCallValueAdjustment::IndexNormalization {
                                target: target.clone(),
                            }]
                        } else {
                            vec![CheckedCallValueAdjustment::ImplicitConversion {
                                target: target.clone(),
                            }]
                        }
                    } else {
                        operations
                            .get(&value_source)
                            .and_then(|adjustment| match adjustment {
                                crate::checked::SemanticAdjustment::MaterializeLiteral(target) => {
                                    Some(vec![CheckedCallValueAdjustment::MaterializeLiteral {
                                        target: Box::new(target.clone()),
                                    }])
                                }
                                _ => None,
                            })
                            .unwrap_or_default()
                    };
                let prior = before
                    .invalidations
                    .get(&value_source)
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                let call_invalidations = invalidations
                    .get(&value_source)
                    .into_iter()
                    .flatten()
                    .filter(|fact| !prior.contains(fact))
                    .cloned()
                    .collect();
                CheckedCallArgumentBoundary {
                    source,
                    value_source,
                    adjustments,
                    invalidations: call_invalidations,
                }
            };

        let arguments = args
            .iter()
            .enumerate()
            .map(|(index, expression)| {
                argument(CheckedCallArgumentSource::Positional(index), expression)
            })
            .chain(kwargs.iter().enumerate().map(|(index, argument_value)| {
                argument(
                    CheckedCallArgumentSource::Keyword(index),
                    &argument_value.value,
                )
            }))
            .collect();
        let prior = before
            .invalidations
            .get(span)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let call_invalidations = invalidations
            .get(span)
            .into_iter()
            .flatten()
            .filter(|fact| !prior.contains(fact))
            .cloned()
            .collect();
        CheckedCallBoundary {
            arguments,
            invalidations: call_invalidations,
        }
    }

    fn snapshot_value_adjustments(&self, sources: &[SourceSpan]) -> Vec<ValueAdjustmentSnapshot> {
        let overloads = self.overload_targets.borrow();
        let implicit = self.implicit_conversions.borrow();
        let operations = self.operation_adjustments.borrow();
        sources
            .iter()
            .map(|source| ValueAdjustmentSnapshot {
                source: source.clone(),
                overload_target: overloads.get(source).cloned(),
                implicit_conversion: implicit.get(source).cloned(),
                operation: operations.get(source).cloned(),
            })
            .collect()
    }

    /// Put shared source operands back into their pre-call state after freezing a
    /// call boundary. Augmented subscripts then select the setter independently;
    /// neither call can overwrite the other's conversion or normalization.
    fn restore_value_adjustments(&self, snapshots: &[ValueAdjustmentSnapshot]) {
        let mut overloads = self.overload_targets.borrow_mut();
        let mut implicit = self.implicit_conversions.borrow_mut();
        let mut operations = self.operation_adjustments.borrow_mut();
        for snapshot in snapshots {
            match &snapshot.overload_target {
                Some(target) => {
                    overloads.insert(snapshot.source.clone(), target.clone());
                }
                None => {
                    overloads.remove(&snapshot.source);
                }
            }
            match &snapshot.implicit_conversion {
                Some(target) => {
                    implicit.insert(snapshot.source.clone(), target.clone());
                }
                None => {
                    implicit.remove(&snapshot.source);
                }
            }
            match &snapshot.operation {
                Some(adjustment) => {
                    operations.insert(snapshot.source.clone(), adjustment.clone());
                }
                None => {
                    operations.remove(&snapshot.source);
                }
            }
        }
    }

    /// Remove call-local invalidations from the compatibility source tables once
    /// they have been frozen on a selected contract. Effects belonging to
    /// evaluation of the argument expression were present in the pre-call
    /// snapshot and therefore are not listed in `boundary` and remain untouched.
    fn remove_call_boundary_invalidations(
        &self,
        site: &SourceSpan,
        boundary: &crate::checked::CheckedCallBoundary,
    ) {
        let mut invalidations = self.interior_invalidations.borrow_mut();
        let mut remove = |source: &SourceSpan, facts: &[crate::checked::InteriorInvalidation]| {
            let empty = if let Some(current) = invalidations.get_mut(source) {
                current.retain(|fact| !facts.contains(fact));
                current.is_empty()
            } else {
                false
            };
            if empty {
                invalidations.remove(source);
            }
        };
        for argument in &boundary.arguments {
            remove(&argument.value_source, &argument.invalidations);
        }
        remove(site, &boundary.invalidations);
    }

    fn score_method_call(
        &self,
        signature: &MethodSig,
        params: &[Ty],
        variadic: Option<&Ty>,
        kw_variadic: Option<&Ty>,
        args: &[Expr],
        kwargs: &[crate::ast::KwArg],
    ) -> Result<MethodCallScore, TypeError> {
        let forwarded_element = self.forwarded_kwargs_element("method", kwargs)?;
        if forwarded_element.is_some() && kw_variadic.is_none() {
            return Err(TypeError::BadCall {
                func: "method".to_string(),
                reason: "`**kwargs^` requires a callee with a `**kwargs` collector".to_string(),
            });
        }
        let keyword_names: Vec<_> = kwargs
            .iter()
            .filter(|argument| !argument.is_forwarded())
            .map(|arg| arg.name.as_str())
            .collect();
        let matched = match_call_slots(
            &signature.names,
            &signature.required,
            signature.positional_only,
            signature.keyword_only,
            args.len(),
            &keyword_names,
            CallVariadics {
                positional: variadic.is_some(),
                keyword: kw_variadic.is_some(),
            },
        )
        .map_err(|error| error.into_type_error("method"))?;
        let (slots, overflow) = (matched.slots, matched.positional_overflow);
        let mut score = 0;
        for (index, slot) in slots.iter().enumerate() {
            let expression = match slot {
                ArgSlot::Positional(position) => &args[*position],
                ArgSlot::Keyword(position) => &kwargs[*position].value,
                ArgSlot::Default => continue,
            };
            let actual = self.infer_with_expected(expression, &params[index], false)?;
            if !self.has_index_normalization(expression, &params[index])
                && !self.value_coerces(&actual, &params[index])
                && (self.is_synthetic_slice_descriptor(expression)
                    || self
                        .implicit_conversion_target(&actual, &params[index])?
                        .is_none())
            {
                return Err(TypeError::TypeMismatch {
                    expected: params[index].to_string(),
                    found: actual.to_string(),
                    context: "method overload candidate".to_string(),
                });
            }
            score += conversion_count(&actual, &params[index]);
        }
        if let Some(element) = variadic {
            // A specialized heterogeneous pack (`Ty::RuntimePack`) checks each overflow
            // argument against its per-index element type with exact arity; an
            // ordinary variadic checks every argument against one element type.
            for (pack_index, &position) in overflow.iter().enumerate() {
                let expected = match element {
                    Ty::RuntimePack(elements) => {
                        elements
                            .get(pack_index)
                            .ok_or_else(|| TypeError::ArityMismatch {
                                name: "method".to_string(),
                                expected: elements.len(),
                                got: overflow.len(),
                            })?
                    }
                    _ => element,
                };
                let actual = self.infer_with_expected(&args[position], expected, false)?;
                if !coerces(&actual, expected) {
                    return Err(TypeError::TypeMismatch {
                        expected: expected.to_string(),
                        found: actual.to_string(),
                        context: "variadic method argument".to_string(),
                    });
                }
                score += conversion_count(&actual, expected);
            }
            if let Ty::RuntimePack(elements) = element
                && elements.len() != overflow.len()
            {
                return Err(TypeError::ArityMismatch {
                    name: "method".to_string(),
                    expected: elements.len(),
                    got: overflow.len(),
                });
            }
        }
        let keyword_overflow = matched.keyword_overflow;
        if let Some(element) = kw_variadic {
            for &position in &keyword_overflow {
                let expression = &kwargs[position].value;
                let actual = self.infer(expression)?;
                if !self.value_coerces(&actual, element)
                    && self.implicit_conversion_target(&actual, element)?.is_none()
                {
                    return Err(TypeError::TypeMismatch {
                        expected: element.to_string(),
                        found: actual.to_string(),
                        context: "keyword variadic method argument".to_string(),
                    });
                }
                self.check_consuming(
                    expression,
                    &actual,
                    &format!("keyword '{}' collected by method", kwargs[position].name),
                )?;
                score += conversion_count(&actual, element);
            }
            if let Some(actual) = forwarded_element
                && actual != *element
            {
                return Err(TypeError::TypeMismatch {
                    expected: format!("StringDict[{element}]"),
                    found: format!("StringDict[{actual}]"),
                    context: "forwarded keyword arguments to method".to_string(),
                });
            }
        }
        Ok(MethodCallScore {
            rank: overload_rank(score, variadic.is_some() || kw_variadic.is_some(), 0, false),
            slots,
            positional_overflow: overflow,
            keyword_overflow,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn instantiate_method_generics(
        &self,
        name: &str,
        signature: &MethodSig,
        params: &[Ty],
        variadic: Option<&Ty>,
        kw_variadic: Option<&Ty>,
        param_args: &[crate::ast::ParamArg],
        args: &[Expr],
        kwargs: &[crate::ast::KwArg],
    ) -> Result<MethodInstantiation, TypeError> {
        if signature.decls.is_empty() {
            if !param_args.is_empty() {
                return Err(TypeError::WrongTypeArgCount {
                    name: name.to_string(),
                    expected: 0,
                    got: param_args.len(),
                });
            }
            return Ok((
                params.to_vec(),
                variadic.cloned(),
                kw_variadic.cloned(),
                HashMap::new(),
                HashMap::new(),
            ));
        }
        let forwarded_element = self.forwarded_kwargs_element(name, kwargs)?;
        if forwarded_element.is_some() && kw_variadic.is_none() {
            return Err(TypeError::BadCall {
                func: name.to_string(),
                reason: "`**kwargs^` requires a callee with a `**kwargs` collector".to_string(),
            });
        }
        let keyword_names: Vec<_> = kwargs
            .iter()
            .filter(|argument| !argument.is_forwarded())
            .map(|arg| arg.name.as_str())
            .collect();
        let matched = match_call_slots(
            &signature.names,
            &signature.required,
            signature.positional_only,
            signature.keyword_only,
            args.len(),
            &keyword_names,
            CallVariadics {
                positional: variadic.is_some(),
                keyword: kw_variadic.is_some(),
            },
        )
        .map_err(|error| error.into_type_error(name))?;
        let mut patterns = Vec::new();
        let mut actuals = Vec::new();
        for (index, slot) in matched.slots.iter().enumerate() {
            let expression = match slot {
                ArgSlot::Positional(position) => &args[*position],
                ArgSlot::Keyword(position) => &kwargs[*position].value,
                ArgSlot::Default => continue,
            };
            patterns.push(params[index].clone());
            actuals.push(self.infer(expression)?);
        }
        if let Some(element) = variadic {
            for position in matched.positional_overflow {
                patterns.push(element.clone());
                actuals.push(self.infer(&args[position])?);
            }
        }
        if let Some(element) = kw_variadic {
            for position in matched.keyword_overflow {
                patterns.push(element.clone());
                actuals.push(self.infer(&kwargs[position].value)?);
            }
            if let Some(actual) = forwarded_element {
                patterns.push(element.clone());
                actuals.push(actual);
            }
        }
        let (subst, tyargs) =
            self.resolve_use_params(name, &signature.decls, param_args, &patterns, &actuals)?;
        let values = Self::value_argument_environment(&signature.decls, &tyargs);
        let resolve = |ty: &Ty| {
            let substituted = self.resolve_assoc_ty(&substitute(ty, &subst));
            self.resolve_dependent_ty(&substituted, &values)
        };
        let arguments = signature
            .decls
            .iter()
            .zip(tyargs.iter().cloned())
            .map(|(decl, argument)| (decl.name().trim_start_matches('*').to_string(), argument))
            .collect();
        Ok((
            params.iter().map(resolve).collect::<Result<Vec<_>, _>>()?,
            variadic.map(resolve).transpose()?,
            kw_variadic.map(resolve).transpose()?,
            subst,
            arguments,
        ))
    }

    fn method_constraints_apply(
        &self,
        signature: &MethodSig,
        arguments: &HashMap<String, TyArg>,
    ) -> bool {
        let borrowed: HashMap<&str, &TyArg> = arguments
            .iter()
            .map(|(name, argument)| (name.as_str(), argument))
            .collect();
        signature
            .availability
            .iter()
            .all(|constraint| self.eval_generic_constraint(constraint, &borrowed))
    }

    /// Type a static method on a parameterized built-in type. Currently only
    /// `UnsafePointer[T].alloc(count: Int) -> UnsafePointer[T]`.
    fn infer_static_method(
        &self,
        tyname: &str,
        targs: &[crate::ast::ParamArg],
        method: &str,
        args: &[Expr],
    ) -> Result<Ty, TypeError> {
        if tyname != "UnsafePointer" {
            return Err(TypeError::NoSuchMethod {
                object_type: format!("{tyname}[…]"),
                method: method.to_string(),
            });
        }
        let ptr_ty = self.pointer_type(targs)?;
        match method {
            "alloc" | "alloc_aligned" => {
                let expected = if method == "alloc" { 1 } else { 2 };
                if args.len() != expected {
                    return Err(TypeError::ArityMismatch {
                        name: method.to_string(),
                        expected,
                        got: args.len(),
                    });
                }
                for argument in args {
                    let aty = self.infer(argument)?;
                    if !coerces(&aty, &Ty::Int) {
                        return Err(TypeError::TypeMismatch {
                            expected: "Int".to_string(),
                            found: aty.to_string(),
                            context: format!("argument to 'UnsafePointer.{method}'"),
                        });
                    }
                }
                Ok(ptr_ty)
            }
            "dangling" => {
                if !args.is_empty() {
                    return Err(TypeError::ArityMismatch {
                        name: method.to_string(),
                        expected: 0,
                        got: args.len(),
                    });
                }
                Ok(ptr_ty)
            }
            _ => Err(TypeError::NoSuchMethod {
                object_type: ptr_ty.to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// Type an `UnsafePointer[T]` instance method. `take` and `destroy` are raw
    /// initialized-slot operations reserved for the bundled self-hosted
    /// collections; indexed load/store remain ordinary public pointer syntax.
    fn infer_pointer_method(
        &self,
        span: &SourceSpan,
        object: &Expr,
        method: &str,
        elem: &Ty,
        origin: &crate::origin::PointerOrigin,
        args: &[Expr],
    ) -> Result<Ty, TypeError> {
        match method {
            "free" => {
                if origin.as_origin().is_some() {
                    return Err(TypeError::Unsupported(
                        "free() is not supported on an origin-bearing UnsafePointer; \
                         it does not own an allocation"
                            .to_string(),
                    ));
                }
                if !args.is_empty() {
                    return Err(TypeError::ArityMismatch {
                        name: "free".to_string(),
                        expected: 0,
                        got: args.len(),
                    });
                }
                Ok(Ty::None)
            }
            "take" | "destroy" => {
                if !is_bundled_collection_source(object.source.as_deref()) {
                    return Err(TypeError::NoSuchMethod {
                        object_type: Ty::Pointer {
                            element: Box::new(elem.clone()),
                            origin: origin.clone(),
                        }
                        .to_string(),
                        method: method.to_string(),
                    });
                }
                if !matches!(origin, crate::origin::PointerOrigin::Legacy) {
                    return Err(TypeError::Unsupported(format!(
                        "{method}() is supported only on an allocation-owning \
                         UnsafePointer without an explicit origin"
                    )));
                }
                if args.len() != 1 {
                    return Err(TypeError::ArityMismatch {
                        name: method.to_string(),
                        expected: 1,
                        got: args.len(),
                    });
                }
                let index = self.infer(&args[0])?;
                if !coerces(&index, &Ty::Int) {
                    return Err(TypeError::TypeMismatch {
                        expected: "Int".to_string(),
                        found: index.to_string(),
                        context: format!("argument to compiler-private UnsafePointer.{method}"),
                    });
                }
                if method == "destroy" && !self.is_implicitly_deletable(elem) {
                    return Err(TypeError::TraitNotSatisfied {
                        param: "T".to_string(),
                        ty: elem.to_string(),
                        trait_name: "ImplicitlyDeletable".to_string(),
                        reason: self.trait_failure_reason(elem, "ImplicitlyDeletable"),
                    });
                }
                let adjustment = if method == "take" {
                    crate::checked::SemanticAdjustment::PointerStorageTake {
                        element: elem.clone(),
                    }
                } else {
                    crate::checked::SemanticAdjustment::PointerStorageDestroy {
                        element: elem.clone(),
                    }
                };
                self.operation_adjustments
                    .borrow_mut()
                    .insert(span.clone(), adjustment);
                Ok(if method == "take" {
                    elem.clone()
                } else {
                    Ty::None
                })
            }
            _ => Err(TypeError::NoSuchMethod {
                object_type: Ty::Pointer {
                    element: Box::new(elem.clone()),
                    origin: origin.clone(),
                }
                .to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// If `recv` is a `struct` defining dunder method `name`, type the implicit
    /// call `recv.name(args…)` — the operator / subscript / builtin dispatch that
    /// turns a user struct into a first-class value type. Checks arity and argument
    /// coercion and returns the (type-argument-substituted) result type. Returns
    /// `None` when `recv` isn't a struct or has no such method, so the caller falls
    /// back to its own operator/builtin error.
    fn struct_dunder(&self, recv: &Ty, name: &str, args: &[&Ty]) -> Option<Result<Ty, TypeError>> {
        let Ty::Struct(sname, targs) = recv else {
            return None;
        };
        let info = self.structs.get(sname)?;
        let sig = info
            .methods
            .get(name)?
            .iter()
            .find(|sig| sig.params.len() == args.len())?;
        let subst = struct_subst(&info.decls, targs);
        let params: Vec<Ty> = sig.params.iter().map(|t| substitute(t, &subst)).collect();
        if params.len() != args.len() {
            return Some(Err(TypeError::ArityMismatch {
                name: name.to_string(),
                expected: params.len(),
                got: args.len(),
            }));
        }
        for (arg, expected) in args.iter().zip(&params) {
            if !coerces(arg, expected) {
                return Some(Err(TypeError::TypeMismatch {
                    expected: expected.to_string(),
                    found: arg.to_string(),
                    context: format!("argument to '{name}'"),
                }));
            }
        }
        Some(Ok(substitute(&sig.ret, &subst)))
    }

    /// Resolve the exact methods used by the bundled List `for ref` bridge.
    /// These synthetic calls are checked here and retained on the iteration
    /// protocol; their syntax is never added to the source-expression arena.
    fn reference_iteration_protocol(
        &self,
        object: &Expr,
    ) -> Result<crate::checked::ReferenceIterationProtocol, TypeError> {
        let call_site = || {
            let mut expression = Expr::new(ExprKind::None, crate::token::DUMMY_SPAN);
            expression.source = object.source.clone();
            expression.source_span()
        };
        let len_site = call_site();
        self.infer_method_call(
            len_site.clone(),
            object,
            "__len__",
            MethodCallArguments::ordinary(&[], &[]),
        )?;
        let len = self
            .selected_calls
            .borrow_mut()
            .remove(&len_site)
            .ok_or_else(|| {
                TypeError::InvariantViolation(
                    "List reference iteration lost its selected __len__ contract".to_string(),
                )
            })?;

        let mut index = Expr::new(ExprKind::Int(0i64.into()), crate::token::DUMMY_SPAN);
        index.source = object.source.clone();
        let getitem_site = call_site();
        self.infer_method_call(
            getitem_site.clone(),
            object,
            "__getitem__",
            MethodCallArguments::ordinary(std::slice::from_ref(&index), &[]),
        )?;
        let getitem = self
            .selected_calls
            .borrow_mut()
            .remove(&getitem_site)
            .ok_or_else(|| {
                TypeError::InvariantViolation(
                    "List reference iteration lost its selected __getitem__ contract".to_string(),
                )
            })?;
        if getitem.reference_result.is_none() {
            return Err(TypeError::InvariantViolation(
                "List reference iteration requires a reference-returning __getitem__".to_string(),
            ));
        }
        Ok(crate::checked::ReferenceIterationProtocol { len, getitem })
    }

    /// Resolve a loop's complete iterator protocol.  In particular, owned
    /// iteration selects `__iter__(var self)` and never silently falls back to a
    /// borrowed `__iter__`.  The selected symbols cross the checked boundary so
    /// HIR/MIR/VM do not repeat overload selection.
    fn iteration_protocol(
        &self,
        ty: &Ty,
        owned: bool,
    ) -> Result<(Ty, crate::checked::IterationProtocol), TypeError> {
        use crate::checked::{IterationMode, IterationProtocol};
        let mode = if owned {
            IterationMode::Owned
        } else {
            IterationMode::Borrowed
        };
        let builtin = |element| {
            (
                element,
                IterationProtocol {
                    mode,
                    borrowed_origin: None,
                    reference: None,
                    prepare: Vec::new(),
                    has_next: None,
                    next: None,
                    exhaustion: None,
                },
            )
        };
        // Focused checker users may deliberately omit the implicit prelude.
        // Preserve the old intrinsic proof only in that compatibility mode;
        // linked production programs have registered nominal declarations and
        // must resolve their ordinary `__iter__`/`__next__` contracts below.
        if let Ty::Struct(name, _) = ty
            && !self.structs.contains_key(name)
        {
            if crate::types::is_range_type(ty) {
                return Ok(builtin(Ty::Int));
            }
            if let Some(element) = list_element(ty).or_else(|| set_element(ty)) {
                return Ok(builtin(element.clone()));
            }
            if let Some((key, _)) = dict_elements(ty) {
                return Ok(builtin(key.clone()));
            }
        }
        match ty {
            Ty::VariadicPack(element) => Ok(builtin((**element).clone())),
            Ty::Struct(..) => self.struct_iteration_protocol(ty, mode, 0),
            Ty::Param { bounds, .. } => {
                let required = if owned { "IterableOwned" } else { "Iterable" };
                if !bounds.iter().any(|bound| bound == required)
                    && self.lookup_trait_assoc_type(bounds, "Element").is_none()
                {
                    return Err(TypeError::TypeMismatch {
                        expected: format!("a type conforming to {required}"),
                        found: ty.to_string(),
                        context: "for-loop iterable".to_string(),
                    });
                }
                if owned && !bounds.iter().any(|bound| bound == "IterableOwned") {
                    return Err(TypeError::TraitNotSatisfied {
                        param: "T".to_string(),
                        ty: ty.to_string(),
                        trait_name: "IterableOwned".to_string(),
                        reason: Some(
                            "owned iteration requires an ownership-consuming iterator".to_string(),
                        ),
                    });
                }
                Ok((
                    Ty::Assoc {
                        base: Box::new(ty.clone()),
                        name: "Element".to_string(),
                    },
                    IterationProtocol {
                        mode,
                        borrowed_origin: None,
                        reference: None,
                        prepare: vec![crate::symbol::iterator_dispatch_symbol(match mode {
                            IterationMode::Borrowed => crate::ast::ArgConvention::Read,
                            IterationMode::Owned => crate::ast::ArgConvention::Var,
                        })],
                        has_next: Some("__iterator_dispatch.__len__".to_string()),
                        next: Some("__iterator_dispatch.__next__".to_string()),
                        exhaustion: None,
                    },
                ))
            }
            other => Err(TypeError::TypeMismatch {
                expected: if owned {
                    "a nominal collection or a type with __iter__(var self)"
                } else {
                    "a nominal collection or a type with borrowed __iter__"
                }
                .to_string(),
                found: other.to_string(),
                context: "for-loop iterable".to_string(),
            }),
        }
    }

    fn struct_iteration_protocol(
        &self,
        c_ty: &Ty,
        mode: crate::checked::IterationMode,
        depth: usize,
    ) -> Result<(Ty, crate::checked::IterationProtocol), TypeError> {
        use crate::checked::IterationMode;
        let no_method = |ty: &Ty, m: &str| TypeError::NoSuchMethod {
            object_type: ty.to_string(),
            method: m.to_string(),
        };
        if depth >= 8 {
            return Err(TypeError::Unsupported(
                "iterator normalization exceeded eight __iter__ steps".to_string(),
            ));
        }
        let Ty::Struct(cname, ctargs) = c_ty else {
            return Err(no_method(c_ty, "__iter__"));
        };
        let cinfo = self.structs.get(cname).ok_or_else(|| {
            TypeError::InvariantViolation(format!("struct '{cname}' was not registered"))
        })?;
        let candidates = cinfo
            .methods
            .get("__iter__")
            .ok_or_else(|| no_method(c_ty, "__iter__"))?;
        let matching = candidates
            .iter()
            .filter(|sig| match mode {
                IterationMode::Owned => sig.self_convention == Some(crate::ast::ArgConvention::Var),
                IterationMode::Borrowed => matches!(
                    sig.self_convention,
                    None | Some(crate::ast::ArgConvention::Read | crate::ast::ArgConvention::Ref)
                ),
            })
            .filter_map(|sig| {
                self.instantiate_iteration_method(cname, cinfo, ctargs, sig)
                    .map(|(ret, error)| (sig, ret, error))
            })
            .collect::<Vec<_>>();
        let [(iter_sig, it_ty, iter_error)] = matching.as_slice() else {
            if matching.len() > 1 {
                return Err(TypeError::BadCall {
                    func: format!("{cname}.__iter__"),
                    reason: "ambiguous iterator receiver convention".to_string(),
                });
            }
            return Err(TypeError::TypeMismatch {
                expected: match mode {
                    IterationMode::Owned => "an '__iter__(var self)' method",
                    IterationMode::Borrowed => "a borrowed '__iter__' method",
                }
                .to_string(),
                found: format!("{}.__iter__", c_ty),
                context: "for-loop iterator selection".to_string(),
            });
        };
        if let Some(error) = iter_error {
            self.require_error(
                format!("implicit call to raising method '{cname}.__iter__'"),
                error.clone(),
            )?;
        }
        let prepare_symbol = if candidates.len() > 1 {
            method_lowered_name(cname, "__iter__", iter_sig)
        } else {
            format!("{cname}.__iter__")
        };
        // The iterator must itself be a struct with `__next__`. Current Mojo
        // terminates iteration when that method raises the typed
        // `StopIteration`; the legacy bounded protocol additionally exposes
        // `__len__` and keeps the old nonraising `__next__` path available.
        let bad_iter = || TypeError::TypeMismatch {
            expected: "List or an iterator struct with __next__".to_string(),
            found: it_ty.to_string(),
            context: "__iter__ return type".to_string(),
        };
        let Ty::Struct(iname, itargs) = it_ty else {
            return Err(bad_iter());
        };
        let iinfo = self.structs.get(iname).ok_or_else(bad_iter)?;
        if !iinfo.methods.contains_key("__next__") && iinfo.methods.contains_key("__iter__") {
            let (element, mut nested) = self.struct_iteration_protocol(it_ty, mode, depth + 1)?;
            nested.prepare.insert(0, prepare_symbol);
            return Ok((element, nested));
        }
        // `__next__(mut self)` advances, so it must mutate `self`.
        let next_candidates = iinfo
            .methods
            .get("__next__")
            .ok_or_else(|| no_method(it_ty, "__next__"))?;
        let applicable_next = next_candidates
            .iter()
            .filter_map(|sig| {
                self.instantiate_iteration_method(iname, iinfo, itargs, sig)
                    .map(|(ret, error)| (sig, ret, error))
            })
            .collect::<Vec<_>>();
        let [(next_sig, element, next_error)] = applicable_next.as_slice() else {
            return Err(no_method(it_ty, "__next__"));
        };
        if !matches!(
            next_sig.self_convention,
            Some(crate::ast::ArgConvention::Mut)
        ) {
            return Err(TypeError::TypeMismatch {
                expected: "a 'mut self' __next__".to_string(),
                found: "read-only self".to_string(),
                context: "iterator '__next__'".to_string(),
            });
        }
        let next_symbol = if iinfo
            .methods
            .get("__next__")
            .is_some_and(|methods| methods.len() > 1)
        {
            method_lowered_name(iname, "__next__", next_sig)
        } else {
            format!("{iname}.__next__")
        };
        if next_sig.raises {
            let exhaustion = next_error.clone().unwrap_or(Ty::Error);
            let is_stop_iteration = matches!(
                &exhaustion,
                Ty::Struct(name, arguments)
                    if arguments.is_empty()
                        && (name == "StopIteration" || name.ends_with("$StopIteration"))
            );
            if !is_stop_iteration {
                return Err(TypeError::TypeMismatch {
                    expected: "an '__next__' that raises StopIteration".to_string(),
                    found: format!("raises {exhaustion}"),
                    context: "iterator '__next__' exhaustion contract".to_string(),
                });
            }
            return Ok((
                element.clone(),
                crate::checked::IterationProtocol {
                    mode,
                    borrowed_origin: None,
                    reference: None,
                    prepare: vec![prepare_symbol],
                    has_next: None,
                    next: Some(next_symbol),
                    exhaustion: Some(exhaustion),
                },
            ));
        }

        // Backward-compatible bounded iteration: `__len__(self) -> Int`
        // determines whether the nonraising `__next__` may be called.
        let len_candidates = iinfo
            .methods
            .get("__len__")
            .ok_or_else(|| no_method(it_ty, "__len__"))?;
        let applicable_len = len_candidates
            .iter()
            .filter_map(|sig| {
                self.instantiate_iteration_method(iname, iinfo, itargs, sig)
                    .map(|(ret, _)| (sig, ret))
            })
            .collect::<Vec<_>>();
        let [(len_sig, len_ret)] = applicable_len.as_slice() else {
            return Err(no_method(it_ty, "__len__"));
        };
        if *len_ret != Ty::Int {
            return Err(TypeError::TypeMismatch {
                expected: "Int".to_string(),
                found: len_ret.to_string(),
                context: "return type of iterator '__len__'".to_string(),
            });
        }
        Ok((
            element.clone(),
            crate::checked::IterationProtocol {
                mode,
                borrowed_origin: None,
                reference: None,
                prepare: vec![prepare_symbol],
                has_next: Some(
                    if iinfo
                        .methods
                        .get("__len__")
                        .is_some_and(|methods| methods.len() > 1)
                    {
                        method_lowered_name(iname, "__len__", len_sig)
                    } else {
                        format!("{iname}.__len__")
                    },
                ),
                next: Some(next_symbol),
                exhaustion: None,
            },
        ))
    }

    /// Instantiate one nullary iterator-protocol method exactly as an ordinary
    /// method call would. In particular, a method-level `where` clause may name
    /// either its own compile-time parameters or the receiver struct's
    /// parameters (`Self.T` is canonicalized to `T`). A declaration which is
    /// present by name but unavailable for this specialization is not a protocol
    /// implementation.
    fn instantiate_iteration_method(
        &self,
        owner: &str,
        info: &StructInfo,
        receiver_arguments: &[TyArg],
        signature: &MethodSig,
    ) -> Option<(Ty, Option<Ty>)> {
        if !signature.has_self || !signature.params.is_empty() {
            return None;
        }
        let receiver_subst = struct_subst(&info.decls, receiver_arguments);
        let params = signature
            .params
            .iter()
            .map(|ty| substitute(ty, &receiver_subst))
            .collect::<Vec<_>>();
        let receiver_variadic = signature
            .variadic
            .as_deref()
            .map(|ty| substitute(ty, &receiver_subst));
        let receiver_kw_variadic = signature
            .kw_variadic
            .as_deref()
            .map(|ty| substitute(ty, &receiver_subst));
        let (_, variadic, kw_variadic, method_subst, mut arguments) = self
            .instantiate_method_generics(
                &format!("{owner} iterator protocol"),
                signature,
                &params,
                receiver_variadic.as_ref(),
                receiver_kw_variadic.as_ref(),
                &[],
                &[],
                &[],
            )
            .ok()?;
        // Iterator dunders have no explicit runtime arguments. A variadic or
        // keyword-variadic declaration is not the exact protocol shape even
        // though an empty ordinary call could technically invoke it.
        if variadic.is_some() || kw_variadic.is_some() {
            return None;
        }
        for (decl, argument) in info.decls.iter().zip(receiver_arguments) {
            arguments.insert(
                decl.name().trim_start_matches('*').to_string(),
                argument.clone(),
            );
        }
        if !self.method_constraints_apply(signature, &arguments) {
            return None;
        }
        let instantiate = |ty: &Ty| substitute(&substitute(ty, &receiver_subst), &method_subst);
        Some((
            instantiate(&signature.ret),
            signature.raises.then(|| {
                signature
                    .error
                    .as_deref()
                    .map(instantiate)
                    .unwrap_or(Ty::Error)
            }),
        ))
    }

    /// Type a `List` method call. The **mutating** methods (`append`, `insert`,
    /// `remove`, `pop`, `clear`, `reverse`, `extend`) require a plain variable
    /// receiver (so they can mutate its binding in place); the **query** methods
    /// (`count`, `index`) work on any list. `remove`/`count`/`index` require an
    /// equatable element type.
    fn infer_list_method(
        &self,
        object: &Expr,
        method: &str,
        elem: &Ty,
        args: &[Expr],
    ) -> Result<Ty, TypeError> {
        let no_such = || TypeError::NoSuchMethod {
            object_type: list_type(elem.clone()).to_string(),
            method: method.to_string(),
        };
        let mutating = matches!(
            method,
            "append" | "insert" | "remove" | "pop" | "clear" | "reverse" | "extend"
        );
        // A mutating method mutates its receiver, so the receiver must be a
        // writable place (a variable or a field/index chain rooted at one) —
        // not a temporary. Reading `check_place` validates exactly that.
        if mutating && self.check_place(object).is_err() {
            return Err(TypeError::MutationRequiresVariable(method.to_string()));
        }
        // `remove`/`count`/`index` compare elements, so require an equatable type.
        if matches!(method, "remove" | "count" | "index") && !is_list_equatable(elem) {
            return Err(TypeError::TypeMismatch {
                expected: "an equatable element type".to_string(),
                found: elem.to_string(),
                context: format!("'{}'", method),
            });
        }
        // Require the argument at position `i` to coerce to the element type.
        let expect_elem = |tys: &[Ty], i: usize| -> Result<(), TypeError> {
            if coerces(&tys[i], elem) {
                Ok(())
            } else {
                Err(TypeError::TypeMismatch {
                    expected: elem.to_string(),
                    found: tys[i].to_string(),
                    context: format!("argument to '{}'", method),
                })
            }
        };
        match method {
            "append" => {
                let tys = self.builtin_args("append", 1, args)?;
                expect_elem(&tys, 0)?;
                Ok(Ty::None)
            }
            "insert" => {
                let tys = self.builtin_args("insert", 2, args)?;
                if !coerces(&tys[0], &Ty::Int) {
                    return Err(TypeError::TypeMismatch {
                        expected: "Int".to_string(),
                        found: tys[0].to_string(),
                        context: "insert index".to_string(),
                    });
                }
                expect_elem(&tys, 1)?;
                Ok(Ty::None)
            }
            "remove" => {
                let tys = self.builtin_args("remove", 1, args)?;
                expect_elem(&tys, 0)?;
                Ok(Ty::None)
            }
            "pop" => {
                // `pop()` (last) or `pop(i)` — an optional `Int` index.
                if args.len() > 1 {
                    return Err(TypeError::ArityMismatch {
                        name: "pop".into(),
                        expected: 1,
                        got: args.len(),
                    });
                }
                if let Some(a) = args.first() {
                    let ity = self.infer(a)?;
                    if !coerces(&ity, &Ty::Int) {
                        return Err(TypeError::TypeMismatch {
                            expected: "Int".to_string(),
                            found: ity.to_string(),
                            context: "pop index".to_string(),
                        });
                    }
                }
                Ok(elem.clone())
            }
            "clear" | "reverse" => {
                self.builtin_args(method, 0, args)?;
                Ok(Ty::None)
            }
            "extend" => {
                let tys = self.builtin_args("extend", 1, args)?;
                let expected = list_type(elem.clone());
                if tys[0] != expected {
                    return Err(TypeError::TypeMismatch {
                        expected: expected.to_string(),
                        found: tys[0].to_string(),
                        context: "argument to 'extend'".to_string(),
                    });
                }
                Ok(Ty::None)
            }
            "count" | "index" => {
                let tys = self.builtin_args(method, 1, args)?;
                expect_elem(&tys, 0)?;
                Ok(Ty::Int)
            }
            _ => Err(no_such()),
        }
    }

    /// Type the value-producing Tuple helpers in the current builtin surface.
    fn infer_tuple_method(
        &self,
        span: &SourceSpan,
        object: &Expr,
        method: &str,
        elements: &[Ty],
        call: MethodCallArguments<'_>,
    ) -> Result<Ty, TypeError> {
        let MethodCallArguments {
            param_args,
            args,
            parameterized_syntax,
            ..
        } = call;
        let receiver_implicitly_copyable = elements
            .iter()
            .all(|element| self.is_implicitly_copyable(element));
        match method {
            "reverse" => {
                if !param_args.is_empty() {
                    return Err(TypeError::WrongTypeArgCount {
                        name: "Tuple.reverse".to_string(),
                        expected: 0,
                        got: param_args.len(),
                    });
                }
                self.builtin_args("reverse", 0, args)?;
                if is_place_expr(object) && !receiver_implicitly_copyable {
                    return Err(TypeError::NonCopyable {
                        ty: nominal_tuple_type(elements.to_vec()).to_string(),
                        context:
                            "consuming receiver of method 'reverse' must be transferred with '^'"
                                .to_string(),
                    });
                }
                Ok(nominal_tuple_type(elements.iter().rev().cloned().collect()))
            }
            "concat" => {
                if !param_args.is_empty() {
                    return Err(TypeError::WrongTypeArgCount {
                        name: "Tuple.concat".to_string(),
                        expected: 0,
                        got: param_args.len(),
                    });
                }
                let tys = self.builtin_args("concat", 1, args)?;
                let Some(other) = tuple_elements(&tys[0]) else {
                    return Err(TypeError::TypeMismatch {
                        expected: "a Tuple".to_string(),
                        found: tys[0].to_string(),
                        context: "argument to 'concat'".to_string(),
                    });
                };
                if is_place_expr(object) && !receiver_implicitly_copyable {
                    return Err(TypeError::NonCopyable {
                        ty: nominal_tuple_type(elements.to_vec()).to_string(),
                        context:
                            "consuming receiver of method 'concat' must be transferred with '^'"
                                .to_string(),
                    });
                }
                if is_place_expr(&args[0])
                    && !other
                        .iter()
                        .all(|element| self.is_implicitly_copyable(element))
                {
                    return Err(TypeError::NonCopyable {
                        ty: tys[0].to_string(),
                        context:
                            "deinit argument 1 to method 'concat' must be transferred with '^'"
                                .to_string(),
                    });
                }
                let mut result = elements.to_vec();
                result.extend(other.into_iter().cloned());
                Ok(nominal_tuple_type(result))
            }
            "consume_elements" => {
                if !args.is_empty() {
                    return Err(TypeError::ArityMismatch {
                        name: "Tuple.consume_elements".to_string(),
                        expected: 0,
                        got: args.len(),
                    });
                }
                if is_place_expr(object) && !receiver_implicitly_copyable {
                    return Err(TypeError::NonCopyable {
                        ty: nominal_tuple_type(elements.to_vec()).to_string(),
                        context: "consuming receiver of method 'consume_elements' must be transferred with '^'"
                            .to_string(),
                    });
                }
                let index_decl = ParamDecl::Value {
                    name: "index".to_string(),
                    ty: Box::new(Ty::Int),
                    default: None,
                    callable_default: None,
                    infer_only: false,
                    variadic: false,
                    constraints: Vec::new(),
                };
                let handler = Ty::GenericFunc {
                    environment: crate::origin::CallableEnvironment::Capturing(
                        crate::origin::CaptureOriginSet::empty(),
                    ),
                    decls: vec![index_decl],
                    params: vec![Ty::Dependent(DependentType::Indexed {
                        elements: elements.to_vec(),
                        index: CtExpr::Param("index".to_string()),
                    })],
                    names: vec!["element".to_string()],
                    ret: Box::new(Ty::None),
                    required: vec![true],
                    variadic: None,
                    kw_variadic: None,
                    positional_only: None,
                    keyword_only: None,
                    raises: false,
                    error: None,
                    conventions: vec![Some(ArgConvention::Var)],
                    ref_params: Box::new(vec![None]),
                    ref_return: None,
                };
                let method_decls = vec![ParamDecl::Value {
                    name: "elt_handler".to_string(),
                    ty: Box::new(handler),
                    default: None,
                    callable_default: None,
                    infer_only: false,
                    variadic: false,
                    constraints: Vec::new(),
                }];
                self.resolve_use_params(
                    "Tuple.consume_elements",
                    &method_decls,
                    param_args,
                    &[],
                    &[],
                )?;
                if parameterized_syntax {
                    self.operation_adjustments.borrow_mut().insert(
                        span.clone(),
                        crate::checked::SemanticAdjustment::ParameterizedMethodCall {
                            param_decls: method_decls,
                        },
                    );
                }
                Ok(Ty::None)
            }
            _ => Err(TypeError::NoSuchMethod {
                object_type: nominal_tuple_type(elements.to_vec()).to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// Find every `method` required by the given trait `bounds`. Keeping the
    /// full candidate set is important: bounded calls use the same named-argument
    /// binder, generic specialization, overload ranking, and effect selection as
    /// concrete method calls.
    fn lookup_trait_methods(&self, bounds: &[String], method: &str, argc: usize) -> Vec<MethodSig> {
        let mut methods = Vec::new();
        // The built-in `Hashable` trait contributes `__hash__(self) -> UInt`
        // (roadmap milestone 6). A user trait cannot shadow a built-in name, so this is
        // unambiguous.
        if method == "__hash__" && argc == 0 && bounds.iter().any(|b| b == "Hashable") {
            methods.push(MethodSig::intrinsic(vec![], Ty::UInt));
        }
        // The built-in numeric-rounding traits contribute a `-> Self` dunder
        // (roadmap milestone 7), used by the self-hosted `math` module: `Floorable`/
        // `Ceilable`/`Truncable` a nullary `__floor__`/`__ceil__`/`__trunc__`,
        // and `CeilDivable`/`CeilDivableRaising` a unary `__ceildiv__(Self)`.
        let accepts = math_dunder_bound(method, argc);
        if !accepts.is_empty() && bounds.iter().any(|b| accepts.contains(&b.as_str())) {
            let params = if argc == 1 {
                vec![Ty::SelfType]
            } else {
                vec![]
            };
            methods.push(MethodSig::intrinsic(params, Ty::SelfType));
        }
        for bound in bounds {
            let Some(signatures) = self
                .traits
                .get(bound)
                .and_then(|info| info.methods.get(method))
            else {
                continue;
            };
            for signature in signatures {
                if !methods.contains(signature) {
                    methods.push(signature.clone());
                }
            }
        }
        methods
    }

    /// Find a type-valued associated comptime member required by any of the
    /// given trait bounds. Built-in bounds contribute none.
    fn lookup_trait_assoc_type(&self, bounds: &[String], member: &str) -> Option<Vec<String>> {
        bounds
            .iter()
            .filter_map(|b| self.traits.get(b))
            .find_map(|info| match info.comptime_members.get(member) {
                Some(CtMemberReq::Type { bounds }) => Some(bounds.clone()),
                _ => None,
            })
    }

    /// Find a value-valued associated comptime member required by a bound trait.
    fn lookup_trait_assoc_value_ty(&self, bounds: &[String], member: &str) -> Option<Ty> {
        bounds
            .iter()
            .filter_map(|b| self.traits.get(b))
            .find_map(|info| match info.comptime_members.get(member) {
                Some(CtMemberReq::Value(ty)) => Some((**ty).clone()),
                _ => None,
            })
    }

    fn infer_prefix(&self, op: PrefixOp, operand: &Expr) -> Result<Ty, TypeError> {
        let t = self.infer(operand)?;
        match (op, &t) {
            // Negation preserves the (possibly literal) numeric type, except UInt.
            (PrefixOp::Neg, Ty::Int | Ty::Float64 | Ty::IntLiteral | Ty::FloatLiteral) => {
                return Ok(t);
            }
            (PrefixOp::Not, Ty::Bool) => return Ok(Ty::Bool),
            _ => {}
        }
        // An opaque type parameter bounded by the prefix operator's trait
        // dispatches after erasure (`-x` needs `Negatable`, `not x` needs
        // `Boolable`); the concrete impl runs on the erased type.
        if param_has_bound(&t, prefix_operation_trait(op)) {
            return Ok(match op {
                PrefixOp::Neg => t,
                PrefixOp::Not => Ty::Bool,
            });
        }
        // A user struct routes through the operator's dunder (`-x` →
        // `x.__neg__() -> Self`, `not x` → `not x.__bool__() -> Bool`).
        if let Some(result) = self.struct_dunder(&t, op.dunder(), &[]) {
            let ret = result?;
            return match op {
                PrefixOp::Neg => Ok(ret),
                PrefixOp::Not => require_dunder_ret(ret, &Ty::Bool, "__bool__"),
            };
        }
        Err(TypeError::BadOperator {
            op: prefix_symbol(op).to_string(),
            operands: t.to_string(),
        })
    }

    fn infer_infix(
        &self,
        span: Option<SourceSpan>,
        op: InfixOp,
        left: &Expr,
        right: &Expr,
    ) -> Result<Ty, TypeError> {
        let lt = self.infer(left)?;
        let rt = self.infer(right)?;
        use InfixOp::*;

        // Membership `in` / `not in` — the right operand is a container.
        if matches!(op, In | NotIn) {
            return self.infer_membership(span, op, left, right, &lt, &rt);
        }
        // SIMD operators are elementwise (handled before the scalar-numeric path).
        if matches!(lt, Ty::Simd { .. }) || matches!(rt, Ty::Simd { .. }) {
            return self.infer_simd_infix(op, &lt, &rt);
        }
        // Arithmetic and identity comparison reason about allocation layout,
        // which an origin-bearing pointer to a single checked value does not
        // have; Mojo leaves such use undefined, so Mojito rejects it early.
        let origin_bearing =
            |ty: &Ty| matches!(ty, Ty::Pointer { origin, .. } if origin.as_origin().is_some());
        if matches!(op, Add | Sub | Eq | Ne)
            && (origin_bearing(&lt) || origin_bearing(&rt))
            && (matches!(lt, Ty::Pointer { .. }) || matches!(rt, Ty::Pointer { .. }))
        {
            return Err(TypeError::Unsupported(
                "pointer arithmetic and comparison are not supported on an \
                 origin-bearing UnsafePointer"
                    .to_string(),
            ));
        }
        if let Ty::Pointer { element, .. } = &lt {
            match (op, &rt) {
                (Add | Sub, Ty::Int | Ty::IntLiteral) => return Ok(lt.clone()),
                (Sub, Ty::Pointer { element: other, .. }) if element == other => {
                    return Ok(Ty::Int);
                }
                (Eq | Ne, Ty::Pointer { element: other, .. }) if element == other => {
                    return Ok(Ty::Bool);
                }
                _ => {}
            }
        }

        // Tuple comparisons are structural. Equality accepts independently
        // equatable element packs (different element types simply compare
        // unequal); ordering requires a lexicographically compatible prefix.
        if let (Ty::Tuple(left), Ty::Tuple(right)) = (&lt, &rt) {
            // Current Tuple comparison methods take `other: Self`: different
            // arities or element packs are not comparable merely because the VM
            // could walk both vectors. Literal element coercion may still make
            // the two tuple types the same contextual `Self`.
            let same_self = coerces(&lt, &rt) || coerces(&rt, &lt);
            let supported = match op {
                Eq | Ne => {
                    same_self && tuple_elements_equatable(left) && tuple_elements_equatable(right)
                }
                Lt | Gt | Le | Ge => same_self && tuple_order_compatible(left, right),
                _ => false,
            };
            if supported {
                return Ok(Ty::Bool);
            }
        }
        if matches!((&lt, &rt), (Ty::Struct(left, _), Ty::Struct(right, _))
            if !self.structs.contains_key(left) && !self.structs.contains_key(right))
            && let (Some(left), Some(right)) = (tuple_elements(&lt), tuple_elements(&rt))
        {
            let left = left.into_iter().cloned().collect::<Vec<_>>();
            let right = right.into_iter().cloned().collect::<Vec<_>>();
            let same_self = coerces(&lt, &rt) || coerces(&rt, &lt);
            let supported = match op {
                Eq | Ne => {
                    same_self && tuple_elements_equatable(&left) && tuple_elements_equatable(&right)
                }
                Lt | Gt | Le | Ge => same_self && tuple_order_compatible(&left, &right),
                _ => false,
            };
            if supported {
                return Ok(Ty::Bool);
            }
        }
        if let (Ty::Variant(left), Ty::Variant(right)) = (&lt, &rt)
            && left == right
            && matches!(op, Eq | Ne)
            && left
                .iter()
                .all(|alternative| has_equality_bound_or_concrete(self, alternative))
        {
            return Ok(Ty::Bool);
        }

        // Two equal opaque type parameters bounded by an arithmetic, bitwise,
        // or shift operation trait dispatch after erasure
        // (`def f[T: Addable](a: T, b: T) -> T: return a + b`). Comparison,
        // equality, and `**` params are handled in the result match below via
        // their (refinement-aware) bound helpers.
        if lt == rt
            && matches!(
                op,
                Add | Sub | Mul | FloorDiv | Mod | Div | Shl | Shr | BitAnd | BitOr | BitXor
            )
            && let Some(trait_name) = infix_operation_trait(op)
            && param_has_bound(&lt, trait_name)
        {
            return Ok(if matches!(op, Div) {
                Ty::Float64
            } else {
                lt.clone()
            });
        }

        // `common` is the unified numeric type when both operands are numeric
        // (literals coerced as needed), else None.
        let common = common_numeric(&lt, &rt);
        if let Some(target) = common.as_ref()
            && matches!(
                target,
                Ty::Int | Ty::UInt | Ty::Float64 | Ty::Simd { width: 1, .. }
            )
        {
            self.record_literal_materializations(left, &lt, target)?;
            self.record_literal_materializations(right, &rt, target)?;
        }
        // Integer powers of exact literals stay exact. A fractional exponent
        // is not rational in general, so this is the semantic boundary where
        // both operands become Float64 and runtime `powf` takes over.
        if matches!(op, Pow) && matches!(common.as_ref(), Some(Ty::FloatLiteral)) {
            let exponent_is_integer = match self.exact_literal_value(right) {
                Some(CtValue::IntLiteral(_)) => true,
                Some(CtValue::FloatLiteral(value)) => value.to_int_if_whole().is_some(),
                _ => false,
            };
            if !exponent_is_integer {
                self.record_literal_materializations(left, &lt, &Ty::Float64)?;
                self.record_literal_materializations(right, &rt, &Ty::Float64)?;
                return Ok(Ty::Float64);
            }
        }
        let result = match op {
            // Short-circuiting boolean logic requires `Bool` operands.
            And | Or if lt == Ty::Bool && rt == Ty::Bool => Some(Ty::Bool),
            // `+` concatenates String, or adds numbers (result = common type).
            Add if lt == Ty::String && rt == Ty::String => Some(Ty::String),
            // `**` between equal opaque type parameters bounded by `Powable`
            // (`__pow__(self, Self) -> Self`); the concrete impl runs after
            // erasure. Checked before the numeric arm since a `Param` isn't
            // numeric (so `common` is None here).
            Pow if lt == rt && param_has_bound(&lt, "Powable") => Some(lt.clone()),
            // Arithmetic that preserves the operand type.
            Add | Sub | Mul | FloorDiv | Mod | Pow => common,
            Shl | Shr | BitAnd | BitOr | BitXor
                if common
                    .as_ref()
                    .is_some_and(|ty| matches!(ty, Ty::Int | Ty::UInt | Ty::IntLiteral)) =>
            {
                common
            }
            // Literal division stays exact until a runtime context chooses
            // Float64; concrete operands perform fixed-width runtime division.
            Div if common.is_some() => Some(
                if matches!(common.as_ref(), Some(Ty::IntLiteral | Ty::FloatLiteral)) {
                    Ty::FloatLiteral
                } else {
                    Ty::Float64
                },
            ),
            // Ordering between numbers, or between equal opaque type parameters
            // whose bound promises an ordering (`T: Comparable`).
            Lt | Gt | Le | Ge
                if common.is_some()
                    || (lt == rt
                        && (has_order_bound(&lt)
                            || self.has_assumed_conformance(&lt, "Comparable"))) =>
            {
                Some(Ty::Bool)
            }
            // Equality: between numbers (any common type), or equal non-numeric
            // scalars (Bool/String/None).
            Eq | Ne
                if common.is_some()
                    || (lt == rt
                        && (is_scalar(&lt)
                            || has_equality_bound(&lt)
                            || self.has_assumed_conformance(&lt, "Equatable")
                            || self.has_assumed_conformance(&lt, "Comparable"))) =>
            {
                Some(Ty::Bool)
            }
            _ => None,
        };
        if let Some(ty) = result {
            return Ok(ty);
        }
        // Operator overloading: `a OP b` on a user struct dispatches to the left
        // operand's dunder method (`a.__add__(b)`, `a.__eq__(b)`, …).
        if let Some(dunder) = op.dunder()
            && let Some(r) = self.struct_dunder(&lt, dunder, &[&rt])
        {
            return r;
        }
        Err(TypeError::BadOperator {
            op: infix_symbol(op).to_string(),
            operands: format!("{} and {}", lt, rt),
        })
    }

    /// Type a membership test `x in c` / `x not in c` → `Bool`. The container is
    /// a `List[T]`, heterogeneous `Tuple`, or `String` (substring test).
    fn infer_membership(
        &self,
        span: Option<SourceSpan>,
        op: InfixOp,
        left: &Expr,
        right: &Expr,
        lt: &Ty,
        rt: &Ty,
    ) -> Result<Ty, TypeError> {
        let nominal_ok = match rt {
            Ty::Struct(name, _) if !self.structs.contains_key(name) => {
                if let Some(element) = list_element(rt).or_else(|| set_element(rt)) {
                    coerces(lt, element) && is_list_equatable(element)
                } else if let Some((key, _)) = dict_elements(rt) {
                    coerces(lt, key) && is_list_equatable(key)
                } else if let Some(elements) = tuple_elements(rt) {
                    elements
                        .into_iter()
                        .any(|element| coerces(lt, element) && is_list_equatable(element))
                } else {
                    false
                }
            }
            _ => false,
        };
        let ok = nominal_ok
            || match rt {
                Ty::Tuple(_) => match lt {
                    Ty::Tuple(elements) => tuple_elements_equatable(elements),
                    other => is_list_equatable(other),
                },
                Ty::String => *lt == Ty::String,
                _ => false,
            };
        if ok {
            return Ok(Ty::Bool);
        }
        // `x in c` on a user struct dispatches to the container's `__contains__`
        // (`c.__contains__(x)`), which must return `Bool`.
        if let Some(span) = span
            && matches!(rt, Ty::Struct(name, _) if self.structs.contains_key(name))
        {
            let ret = self.infer_method_call(
                span,
                right,
                "__contains__",
                MethodCallArguments::ordinary(std::slice::from_ref(left), &[]),
            )?;
            return require_dunder_ret(ret, &Ty::Bool, "__contains__");
        }
        if let Some(r) = self.struct_dunder(rt, "__contains__", &[lt]) {
            return r.and_then(|ret| require_dunder_ret(ret, &Ty::Bool, "__contains__"));
        }
        Err(TypeError::BadOperator {
            op: infix_symbol(op).to_string(),
            operands: format!("{} and {}", lt, rt),
        })
    }

    /// Type an elementwise SIMD operator. Both operands must be the same SIMD
    /// type, except a numeric *literal* splats to the other operand's type.
    /// Arithmetic keeps the operand type; comparisons return a `bool` mask.
    fn infer_simd_infix(&self, op: InfixOp, lt: &Ty, rt: &Ty) -> Result<Ty, TypeError> {
        use InfixOp::*;
        let bad = || TypeError::BadOperator {
            op: infix_symbol(op).to_string(),
            operands: format!("{} and {}", lt, rt),
        };
        // Determine the common SIMD type, allowing a numeric literal on one side.
        let simd = match (lt, rt) {
            (
                Ty::Simd {
                    dtype: d1,
                    width: w1,
                },
                Ty::Simd {
                    dtype: d2,
                    width: w2,
                },
            ) if d1 == d2 && w1 == w2 => Ty::Simd {
                dtype: *d1,
                width: *w1,
            },
            (Ty::Simd { dtype, width }, other) | (other, Ty::Simd { dtype, width })
                if splats_to(other, *dtype) =>
            {
                Ty::Simd {
                    dtype: *dtype,
                    width: *width,
                }
            }
            _ => return Err(bad()),
        };
        let Ty::Simd { dtype, width } = simd else {
            return Err(TypeError::InvariantViolation(
                "SIMD operator inference produced a non-SIMD type".to_string(),
            ));
        };
        match op {
            // Elementwise arithmetic on numeric lanes preserves the type.
            Add | Sub | Mul if dtype != Dtype::Bool => Ok(simd_ty(dtype, width)),
            // True division is defined on float lanes only.
            Div if dtype.is_float() => Ok(simd_ty(dtype, width)),
            // Equality on any lanes; ordering on numeric lanes — a bool mask.
            Eq | Ne => Ok(simd_ty(Dtype::Bool, width)),
            Lt | Gt | Le | Ge if dtype != Dtype::Bool => Ok(simd_ty(Dtype::Bool, width)),
            _ => Err(bad()),
        }
    }

    /// Type `SIMD[DType.<dt>, width](args)`: `width` element arguments, or a
    /// single argument that splats across all lanes; each must fit the dtype.
    fn infer_simd_construction(
        &self,
        param_args: &[crate::ast::ParamArg],
        args: &[Expr],
    ) -> Result<Ty, TypeError> {
        let (dtype, mut width) = self.simd_dims(param_args)?;
        if width == -1 {
            width = i64::try_from(args.len()).unwrap_or(0);
            if width < 1 || (width & (width - 1)) != 0 {
                return Err(TypeError::BadSimdWidth(width.to_string()));
            }
        }
        self.check_simd_args(dtype, width, args)?;
        Ok(simd_ty(dtype, width))
    }

    /// Type a scalar-alias construction `Int32(x)` = `SIMD[DType.int32, 1](x)`.
    fn infer_simd_alias_construction(
        &self,
        dtype: Dtype,
        param_args: &[crate::ast::ParamArg],
        args: &[Expr],
    ) -> Result<Ty, TypeError> {
        if !param_args.is_empty() {
            return Err(TypeError::WrongTypeArgCount {
                name: dtype.scalar_alias().unwrap_or("SIMD").to_string(),
                expected: 0,
                got: param_args.len(),
            });
        }
        self.check_simd_args(dtype, 1, args)?;
        Ok(Ty::Simd { dtype, width: 1 })
    }

    /// Check the element arguments of a SIMD construction: either `width` of them
    /// (one per lane) or exactly one (splatted), each fitting `dtype`.
    fn check_simd_args(&self, dtype: Dtype, width: i64, args: &[Expr]) -> Result<(), TypeError> {
        if args.len() != width as usize && args.len() != 1 {
            return Err(TypeError::SimdArity {
                width,
                got: args.len(),
            });
        }
        for arg in args {
            let aty = self.infer(arg)?;
            if !splats_to(&aty, dtype) {
                return Err(TypeError::TypeMismatch {
                    expected: format!("a DType.{} element", dtype.name()),
                    found: aty.to_string(),
                    context: "SIMD element".to_string(),
                });
            }
        }
        Ok(())
    }

    /// Type the built-in `Error(msg)` constructor: one `String` argument.
    fn infer_error_construction(&self, args: &[Expr]) -> Result<Ty, TypeError> {
        if args.len() != 1 {
            return Err(TypeError::ArityMismatch {
                name: "Error".to_string(),
                expected: 1,
                got: args.len(),
            });
        }
        let aty = self.infer(&args[0])?;
        if aty != Ty::String {
            return Err(TypeError::TypeMismatch {
                expected: "String".to_string(),
                found: aty.to_string(),
                context: "argument to 'Error'".to_string(),
            });
        }
        Ok(Ty::Error)
    }

    fn infer_slice_construction(&self, name: &str, args: &[Expr]) -> Result<Ty, TypeError> {
        let valid_arity = match name {
            "Slice" => matches!(args.len(), 2 | 3),
            "slice" => matches!(args.len(), 1..=3),
            _ => false,
        };
        if !valid_arity {
            return Err(TypeError::ArityMismatch {
                name: name.to_string(),
                expected: if name == "Slice" { 2 } else { 1 },
                got: args.len(),
            });
        }
        for argument in args {
            let found = self.infer(argument)?;
            if found != Ty::None && !coerces(&found, &Ty::Int) {
                return Err(TypeError::TypeMismatch {
                    expected: "Int or None".to_string(),
                    found: found.to_string(),
                    context: format!("argument to '{name}'"),
                });
            }
        }
        Ok(Ty::Struct("Slice".to_string(), Vec::new()))
    }

    /// Type `UnsafePointer(to=place)`: an origin-bearing pointer to existing
    /// checked storage. The element type is the place's type and the origin is
    /// the place itself, so loan analysis keeps the owner alive and rejects
    /// conflicting access. Execution represents the value as a frame/slot
    /// handle; only the VM erases the origin.
    fn infer_pointer_to(
        &self,
        span: SourceSpan,
        param_args: &[crate::ast::ParamArg],
        args: &[Expr],
        kwargs: &[crate::ast::KwArg],
    ) -> Result<Ty, TypeError> {
        if !param_args.is_empty() {
            return Err(TypeError::Unsupported(
                "UnsafePointer(to=...) infers its element type; explicit type \
                 arguments are not supported"
                    .to_string(),
            ));
        }
        if !args.is_empty() || kwargs.len() != 1 || kwargs[0].name != "to" {
            return Err(TypeError::BadCall {
                func: "UnsafePointer".to_string(),
                reason: "expected exactly one 'to=' keyword argument".to_string(),
            });
        }
        let value = &kwargs[0].value;
        if let ExprKind::Identifier(name) = &value.kind
            && (matches!(self.lookup(name), Some(Ty::Ref(_)))
                || self.lookup_reference_parameter(name).is_some())
        {
            return Err(TypeError::Unsupported(
                "UnsafePointer(to=...) through a 'ref' binding is not supported yet".to_string(),
            ));
        }
        let place = self.origin_place(value).map_err(|error| match error {
            TypeError::UndefinedVariable(_) => error,
            _ => TypeError::Unsupported(
                "UnsafePointer(to=...) requires a place expression".to_string(),
            ),
        })?;
        let element = self.infer(value)?;
        let mutable = self.owner_is_mutable(place.root);
        self.operation_adjustments.borrow_mut().insert(
            span,
            crate::checked::SemanticAdjustment::PointerToPlace { mutable },
        );
        Ok(Ty::Pointer {
            element: Box::new(element),
            origin: crate::origin::PointerOrigin::Place { place, mutable },
        })
    }

    fn infer_call(
        &self,
        span: SourceSpan,
        name: &str,
        param_args: &[crate::ast::ParamArg],
        args: &[Expr],
        kwargs: &[crate::ast::KwArg],
    ) -> Result<Ty, TypeError> {
        if is_variant_name(name)
            && (name != "Variant" || self.structs.contains_key(name))
            && self.lookup(name).is_none()
        {
            return self.infer_variant_construction(span, param_args, args, kwargs);
        }
        let ty = match self.lookup(name) {
            Some(ty) => ty.clone(),
            // Built-ins and struct construction, resolved only when the name
            // isn't shadowed by a binding.
            None => match name {
                _ if self.structs.contains_key(name) => {
                    return self.infer_construction(span, name, param_args, args, kwargs);
                }
                // Tuple specializations are predeclared as one closed set before
                // their members are checked.  A generated transform may therefore
                // construct its reverse result before that result's full StructInfo
                // has been populated (the reciprocal reverse direction makes any
                // sequential declaration order impossible).  Its concrete element
                // arguments are enough to validate the compiler-owned constructor;
                // `public_tuple_type` also proves that they select this exact
                // predeclared symbol.  Ordinary source constructors retain
                // sequential visibility because this gate is enabled only while a
                // compiler-generated Tuple implementation is being checked.
                _ if self.allow_generated_tuple_forward_types
                    && self.declared_structs.contains(name)
                    && (name.starts_with("Tuple$") || name.contains("$Tuple$"))
                    && param_args.is_empty()
                    && kwargs.is_empty() =>
                {
                    let tuple = self.infer_tuple_construction(&[], args)?;
                    if matches!(&tuple, Ty::Struct(target, _) if target == name) {
                        // Preserve the predeclared implementation as an exact
                        // checked callee.  This is intentionally redundant with
                        // the synthetic source spelling: MIR consumes checked
                        // call identity and never has to infer that a nominal
                        // Tuple construction is not the unspecialized template.
                        self.overload_targets
                            .borrow_mut()
                            .insert(span, name.to_string());
                        return Ok(tuple);
                    }
                    return Err(TypeError::BadCall {
                        func: name.to_string(),
                        reason: "generated Tuple constructor arguments select a different specialization"
                            .to_string(),
                    });
                }
                "UnsafePointer" if !kwargs.is_empty() => {
                    return self.infer_pointer_to(span, param_args, args, kwargs);
                }
                _ if !kwargs.is_empty() => {
                    return Err(TypeError::BadCall {
                        func: name.to_string(),
                        reason: "keyword arguments are not supported here".to_string(),
                    });
                }
                "print" => return self.infer_print(args),
                "String" => return self.infer_stringify(args),
                "repr" => {
                    let tys = self.builtin_args("repr", 1, args)?;
                    if self.conforms_to(&tys[0], "Writable") {
                        self.call_place_uses
                            .borrow_mut()
                            .insert(args[0].source_span());
                        return Ok(Ty::String);
                    }
                    return Err(TypeError::TypeMismatch {
                        expected: "Writable".to_string(),
                        found: tys[0].to_string(),
                        context: "argument to 'repr'".to_string(),
                    });
                }
                "hash" => {
                    let tys = self.builtin_args("hash", 1, args)?;
                    if self.conforms_to(&tys[0], "Hashable") {
                        return Ok(Ty::UInt);
                    }
                    return Err(TypeError::TraitNotSatisfied {
                        param: "T".to_string(),
                        ty: tys[0].to_string(),
                        trait_name: "Hashable".to_string(),
                        reason: self.trait_failure_reason(&tys[0], "Hashable"),
                    });
                }
                "abs" => return self.infer_abs(args),
                "min" | "max" => return self.infer_min_max(name, args),
                "round" => return self.infer_round(args),
                "input" => return self.infer_input(args),
                "len" => return self.infer_len(args),
                "range" => return self.infer_range(args),
                "Slice" | "slice" => return self.infer_slice_construction(name, args),
                "Int" => return self.infer_conversion(Ty::Int, args),
                "UInt" => return self.infer_conversion(Ty::UInt, args),
                "Float64" => return self.infer_conversion(Ty::Float64, args),
                "Bool" => return self.infer_conversion(Ty::Bool, args),
                "divmod" => return self.infer_divmod(args),
                "SIMD" => return self.infer_simd_construction(param_args, args),
                "Scalar" => {
                    if param_args.len() != 1 {
                        return Err(TypeError::WrongTypeArgCount {
                            name: "Scalar".to_string(),
                            expected: 1,
                            got: param_args.len(),
                        });
                    }
                    let dtype = dtype_from_arg(&param_args[0])?;
                    self.check_simd_args(dtype, 1, args)?;
                    return Ok(simd_ty(dtype, 1));
                }
                "List" => return self.infer_list_construction(param_args, args),
                "Set" => {
                    let collection = self.set_type(param_args)?;
                    let element = set_element(&collection)
                        .expect("Set type helper returns a nominal Set")
                        .clone();
                    for argument in args {
                        let actual = self.infer(argument)?;
                        if !coerces(&actual, &element) {
                            return Err(TypeError::TypeMismatch {
                                expected: element.to_string(),
                                found: actual.to_string(),
                                context: "Set construction element".to_string(),
                            });
                        }
                        self.record_literal_materializations(argument, &actual, &element)?;
                        self.check_consuming(argument, &actual, "Set construction element")?;
                    }
                    return Ok(set_type(element));
                }
                "Dict" => {
                    if !args.is_empty() {
                        return Err(TypeError::ArityMismatch {
                            name: "Dict".to_string(),
                            expected: 0,
                            got: args.len(),
                        });
                    }
                    return self.dict_type(param_args);
                }
                "Tuple" => {
                    let tuple = self.infer_tuple_construction(param_args, args)?;
                    if let Ty::Struct(target, _) = &tuple
                        && target != crate::types::TUPLE_TYPE_NAME
                    {
                        self.overload_targets
                            .borrow_mut()
                            .insert(span, target.clone());
                    }
                    return Ok(tuple);
                }
                "Error" => return self.infer_error_construction(args),
                _ if Dtype::from_scalar_alias(name).is_some() => {
                    let dtype = Dtype::from_scalar_alias(name)
                        .expect("match guard established a scalar alias");
                    return self.infer_simd_alias_construction(dtype, param_args, args);
                }
                _ => return Err(TypeError::UndefinedVariable(name.to_string())),
            },
        };
        self.record_permitted_call_capture(name);
        if let Some(owner) = self.lookup_owner(name) {
            self.expression_bindings
                .borrow_mut()
                .insert(span.clone(), owner);
        }
        let origin_signatures = self.lookup_callable_origins(name).unwrap_or_default();
        if let Ty::Overload(candidates) = ty {
            let mut matches = Vec::new();
            for (index, candidate) in candidates.iter().enumerate() {
                let saved_conversions = self.implicit_conversions.borrow().clone();
                let saved_invalidations = self.interior_invalidations.borrow().clone();
                let saved_call_place_uses = self.call_place_uses.borrow().clone();
                if let Ok((prepared, ordinary_param_args)) = self.prepare_callable_specialization(
                    name,
                    param_args,
                    candidate.clone(),
                    origin_signatures.get(index),
                ) && let Ok((ret, score, error)) =
                    self.infer_callable_ty(name, prepared, &ordinary_param_args, args, kwargs)
                    && let Some(target) = callable_lowered_name(name, candidate)
                {
                    matches.push((ret, score, target, error));
                }
                *self.implicit_conversions.borrow_mut() = saved_conversions;
                *self.interior_invalidations.borrow_mut() = saved_invalidations;
                *self.call_place_uses.borrow_mut() = saved_call_place_uses;
            }
            return match select_callable_overload(matches) {
                Ok((ret, target, error)) => {
                    self.overload_targets
                        .borrow_mut()
                        .insert(span.clone(), target.clone());
                    if let Some((index, selected)) =
                        candidates.iter().enumerate().find(|(_, candidate)| {
                            callable_lowered_name(name, candidate).as_deref()
                                == Some(target.as_str())
                        })
                    {
                        let (prepared, ordinary_param_args) = self
                            .prepare_callable_specialization(
                                name,
                                param_args,
                                selected.clone(),
                                origin_signatures.get(index),
                            )?;
                        self.infer_callable_ty(
                            name,
                            prepared.clone(),
                            &ordinary_param_args,
                            args,
                            kwargs,
                        )?;
                        self.record_call_environment_effects(
                            span.clone(),
                            &prepared,
                            &ordinary_param_args,
                            args,
                            kwargs,
                        )?;
                    }
                    if let Some(error) = error.filter(|ty| *ty != Ty::Never) {
                        self.record_call_effect(span.clone(), error.clone());
                        self.require_error(format!("call to raising function '{name}'"), error)?;
                    }
                    Ok(ret)
                }
                Err(OverloadSelect::NoMatch) => Err(TypeError::BadCall {
                    func: name.to_string(),
                    reason: "no overload matches the supplied arguments".to_string(),
                }),
                Err(OverloadSelect::Ambiguous) => Err(TypeError::BadCall {
                    func: name.to_string(),
                    reason: "ambiguous overloaded call".to_string(),
                }),
            };
        }
        let (ty, ordinary_param_args) =
            self.prepare_callable_specialization(name, param_args, ty, origin_signatures.first())?;
        let indirect_target = match &ty {
            Ty::Struct(..) => self.indirect_callable_target(&ty),
            _ if callable_contract_ty(&ty).is_some()
                && self.binding_scope(name).is_some_and(|scope| scope > 0) =>
            {
                self.indirect_callable_target(&ty)
            }
            _ => None,
        };
        if indirect_target.is_some() && matches!(ty, Ty::GenericFunc { .. }) {
            let (contract, arguments) =
                self.instantiate_generic_callable_value(name, ty.clone(), &ordinary_param_args)?;
            self.operation_adjustments.borrow_mut().insert(
                span.clone(),
                crate::checked::SemanticAdjustment::InstantiatedCallableContract {
                    contract,
                    arguments,
                },
            );
        }
        let (ret, _, error) =
            self.infer_callable_ty(name, ty.clone(), &ordinary_param_args, args, kwargs)?;
        self.record_call_environment_effects(
            span.clone(),
            &ty,
            &ordinary_param_args,
            args,
            kwargs,
        )?;
        if let Some(target) = indirect_target {
            self.overload_targets
                .borrow_mut()
                .insert(span.clone(), target);
        }
        if let Some(error) = error.filter(|ty| *ty != Ty::Never) {
            self.record_call_effect(span, error.clone());
            self.require_error(format!("call to raising function '{name}'"), error)?;
        }
        Ok(ret)
    }

    fn infer_callable_ty(
        &self,
        name: &str,
        ty: Ty,
        param_args: &[crate::ast::ParamArg],
        args: &[Expr],
        kwargs: &[crate::ast::KwArg],
    ) -> Result<(Ty, usize, Option<Ty>), TypeError> {
        let (
            params,
            names,
            ret,
            required,
            variadic,
            kw_variadic,
            positional_only,
            keyword_only,
            _raises,
            error,
            conventions,
            ref_params,
            ref_return,
        ) = match ty {
            Ty::Param {
                callable_bound: Some(bound),
                ..
            } => {
                return self.infer_callable_ty(name, *bound, param_args, args, kwargs);
            }
            Ty::Struct(struct_name, arguments) => {
                let actual = Ty::Struct(struct_name.clone(), arguments);
                let callable = self.declared_callable_contract(&actual).ok_or_else(|| {
                    TypeError::NotCallable {
                        name: name.to_string(),
                        ty: struct_name.clone(),
                    }
                })?;
                return self.infer_callable_ty(name, callable, param_args, args, kwargs);
            }
            // A non-generic function takes no compile-time parameters.
            Ty::Func {
                params,
                names,
                ret,
                required,
                variadic,
                kw_variadic,
                positional_only,
                keyword_only,
                raises,
                error,
                conventions,
                ref_params,
                ref_return,
                ..
            } => {
                if !param_args.is_empty() {
                    return Err(TypeError::WrongTypeArgCount {
                        name: name.to_string(),
                        expected: 0,
                        got: param_args.len(),
                    });
                }
                (
                    params,
                    names,
                    ret,
                    required,
                    variadic,
                    kw_variadic,
                    positional_only,
                    keyword_only,
                    raises,
                    error,
                    conventions,
                    ref_params,
                    ref_return,
                )
            }
            // Bind ordinary arguments first, then infer or apply the generic
            // function's compile-time parameters from the occupied slots.
            generic @ Ty::GenericFunc { .. } => {
                return self.infer_generic_call(name, &generic, param_args, args, kwargs);
            }
            other => {
                return Err(TypeError::NotCallable {
                    name: name.to_string(),
                    ty: other.to_string(),
                });
            }
        };

        // Match positional then keyword arguments to the regular parameter slots
        // (extra positional args overflow into a `*args` parameter), then check
        // each supplied argument coerces to its parameter's type (an unfilled slot
        // uses the default, already type-checked at the definition site).
        let forwarded_element = self.forwarded_kwargs_element(name, kwargs)?;
        let kw_names: Vec<&str> = kwargs
            .iter()
            .filter(|argument| !argument.is_forwarded())
            .map(|argument| argument.name.as_str())
            .collect();
        let has_kw_collector = kw_variadic.is_some();
        let kw_collector = kw_variadic.map(|element| *element);
        if forwarded_element.is_some() && kw_collector.is_none() {
            return Err(TypeError::BadCall {
                func: name.to_string(),
                reason: "`**kwargs^` requires a callee with a `**kwargs` collector".to_string(),
            });
        }
        let matched = match_call_slots(
            &names,
            &required,
            positional_only,
            keyword_only,
            args.len(),
            &kw_names,
            CallVariadics {
                positional: variadic.is_some(),
                keyword: kw_collector.is_some(),
            },
        )
        .map_err(|e| e.into_type_error(name))?;
        let (slots, overflow, kw_overflow) = (
            matched.slots,
            matched.positional_overflow,
            matched.keyword_overflow,
        );
        let mut score = 0;
        for (i, slot) in slots.iter().enumerate() {
            let arg = match slot {
                ArgSlot::Positional(p) => &args[*p],
                ArgSlot::Keyword(k) => &kwargs[*k].value,
                ArgSlot::Default => continue,
            };
            let arg_ty = self.infer_with_expected(arg, &params[i], true)?;
            if !self.record_implicit_conversion(arg, &arg_ty, &params[i])? {
                return Err(TypeError::TypeMismatch {
                    expected: params[i].to_string(),
                    found: arg_ty.to_string(),
                    context: format!("argument '{}' to '{}'", names[i], name),
                });
            }
            score += conversion_count(&arg_ty, &params[i]);
            // Only a `var`/`deinit` parameter *consumes* its argument (moving the
            // value in). `read` (the default), `mut`, and `ref` all **borrow** — no
            // copy — so passing a non-Copyable value to them is fine.
            if matches!(
                conventions.get(i),
                Some(Some(ArgConvention::Var | ArgConvention::Deinit))
            ) {
                self.check_consuming(
                    arg,
                    &arg_ty,
                    &format!("argument '{}' to '{}'", names[i], name),
                )?;
            }
        }
        // Each overflow argument must coerce to the `*args` element type.
        if let Some(elem) = &variadic {
            for (pack_index, &p) in overflow.iter().enumerate() {
                let expected = match &**elem {
                    Ty::RuntimePack(elements) => {
                        elements
                            .get(pack_index)
                            .ok_or_else(|| TypeError::ArityMismatch {
                                name: name.to_string(),
                                expected: elements.len(),
                                got: overflow.len(),
                            })?
                    }
                    _ => elem,
                };
                let arg_ty = self.infer_with_expected(&args[p], expected, true)?;
                if !coerces(&arg_ty, expected) {
                    return Err(TypeError::TypeMismatch {
                        expected: expected.to_string(),
                        found: arg_ty.to_string(),
                        context: format!("variadic argument to '{}'", name),
                    });
                }
                score += conversion_count(&arg_ty, expected);
            }
            if let Ty::RuntimePack(elements) = &**elem
                && elements.len() != overflow.len()
            {
                return Err(TypeError::ArityMismatch {
                    name: name.to_string(),
                    expected: elements.len(),
                    got: overflow.len(),
                });
            }
        }
        if let Some(elem) = kw_collector {
            for index in kw_overflow {
                let expression = &kwargs[index].value;
                let found = self.infer_with_expected(expression, &elem, true)?;
                if !self.record_implicit_conversion(expression, &found, &elem)? {
                    return Err(TypeError::TypeMismatch {
                        expected: elem.to_string(),
                        found: found.to_string(),
                        context: format!(
                            "keyword '{}' collected by '{}'",
                            kwargs[index].name, name
                        ),
                    });
                }
                self.check_consuming(
                    expression,
                    &found,
                    &format!("keyword '{}' collected by '{name}'", kwargs[index].name),
                )?;
                score += conversion_count(&found, &elem);
            }
            if let Some(found) = forwarded_element
                && found != elem
            {
                return Err(TypeError::TypeMismatch {
                    expected: format!("StringDict[{elem}]"),
                    found: format!("StringDict[{found}]"),
                    context: format!("forwarded keyword arguments to '{name}'"),
                });
            }
        }

        // Borrow check (mutable-XOR-shared), root-sensitive: within one call a
        // variable borrowed exclusively (`mut`/`ref`) or moved (`^`) may not be
        // borrowed again — mutably, shared, or moved.
        let (effective_conventions, return_ref) = self.solve_call_origins(
            &slots,
            &conventions,
            &ref_params,
            ref_return.as_deref(),
            args,
            kwargs,
        )?;
        let copied_reads = slots
            .iter()
            .enumerate()
            .map(|(index, slot)| {
                let expression = match slot {
                    ArgSlot::Positional(position) => &args[*position],
                    ArgSlot::Keyword(position) => &kwargs[*position].value,
                    ArgSlot::Default => return Ok(false),
                };
                let convention = effective_conventions.get(index).copied().flatten();
                Ok(
                    !matches!(convention, Some(ArgConvention::Mut | ArgConvention::Ref))
                        && self.is_copyable(&self.infer_with_expected(
                            expression,
                            &params[index],
                            true,
                        )?),
                )
            })
            .collect::<Result<Vec<_>, TypeError>>()?;
        check_call_aliasing(&slots, &effective_conventions, &copied_reads, args, kwargs)?;

        let result = return_ref
            .map(|mut reference| {
                reference.referent = ret.clone();
                Ty::Ref(reference)
            })
            .unwrap_or(*ret);
        Ok((
            result,
            overload_rank(score, variadic.is_some() || has_kw_collector, 0, false),
            error.map(|error| *error),
        ))
    }

    /// Type a call to a generic function: solve its type parameters from the
    /// argument types, then check each argument coerces to the substituted
    /// parameter type and return the substituted result type.
    fn infer_generic_call(
        &self,
        name: &str,
        generic: &Ty,
        param_args: &[crate::ast::ParamArg],
        args: &[Expr],
        kwargs: &[crate::ast::KwArg],
    ) -> Result<(Ty, usize, Option<Ty>), TypeError> {
        let Ty::GenericFunc {
            decls,
            params,
            names,
            ret,
            required,
            variadic,
            kw_variadic,
            positional_only,
            keyword_only,
            raises: _,
            error,
            conventions,
            ref_params,
            ref_return,
            ..
        } = generic
        else {
            return Err(TypeError::InvariantViolation(format!(
                "generic call inference received non-generic callee '{name}'"
            )));
        };
        let forwarded_element = self.forwarded_kwargs_element(name, kwargs)?;
        if forwarded_element.is_some() && kw_variadic.is_none() {
            return Err(TypeError::BadCall {
                func: name.to_string(),
                reason: "`**kwargs^` requires a callee with a `**kwargs` collector".to_string(),
            });
        }
        let kw_names: Vec<&str> = kwargs
            .iter()
            .filter(|argument| !argument.is_forwarded())
            .map(|argument| argument.name.as_str())
            .collect();
        let matched = match_call_slots(
            names,
            required,
            *positional_only,
            *keyword_only,
            args.len(),
            &kw_names,
            CallVariadics {
                positional: variadic.is_some(),
                keyword: kw_variadic.is_some(),
            },
        )
        .map_err(|e| e.into_type_error(name))?;
        let (slots, overflow, kw_overflow) = (
            matched.slots,
            matched.positional_overflow,
            matched.keyword_overflow,
        );
        let mut use_params = Vec::new();
        let mut arg_tys = Vec::new();
        let mut arg_exprs = Vec::new();
        for (i, slot) in slots.iter().enumerate() {
            let arg = match slot {
                ArgSlot::Positional(p) => &args[*p],
                ArgSlot::Keyword(k) => &kwargs[*k].value,
                ArgSlot::Default => continue,
            };
            use_params.push(params[i].clone());
            arg_tys.push(self.infer(arg)?);
            arg_exprs.push(arg);
        }
        if let Some(elem) = variadic.as_deref() {
            for &p in &overflow {
                use_params.push(elem.clone());
                arg_tys.push(self.infer(&args[p])?);
                arg_exprs.push(&args[p]);
            }
        }
        let mut keyword_actuals = Vec::new();
        if let Some(element) = kw_variadic.as_deref() {
            for &index in &kw_overflow {
                let actual = self.infer(&kwargs[index].value)?;
                use_params.push(element.clone());
                arg_tys.push(actual.clone());
                keyword_actuals.push((index, actual));
            }
            if let Some(actual) = &forwarded_element {
                use_params.push(element.clone());
                arg_tys.push(actual.clone());
            }
        }
        let (subst, tyargs) =
            self.resolve_use_params(name, decls, param_args, &use_params, &arg_tys)?;
        let values = Self::value_argument_environment(decls, &tyargs);
        let resolve = |ty: &Ty| {
            let substituted = self.resolve_assoc_ty(&substitute(ty, &subst));
            self.resolve_dependent_ty(&substituted, &values)
        };
        let mut conversions = 0;
        for ((aty, pty), expression) in arg_tys.iter().zip(&use_params).zip(arg_exprs) {
            if matches!(pty, Ty::Param { name, .. } if name.starts_with('*')) {
                // Each pack element was checked independently against the pack's
                // bounds during inference; there is intentionally no single
                // substituted element type to coerce every argument into.
                continue;
            }
            let expected = resolve(pty)?;
            // A dependent generic parameter can resolve to a reference-valued
            // type only after explicit value arguments have been substituted
            // (for example `Ts[index]` in Tuple.consume_elements). Re-infer in
            // that resolved context so the actual is the stored handle rather
            // than the ordinary read-through referent.
            let contextual;
            let actual = if self.type_contains_reference(&expected) {
                contextual = self.infer_with_expected(expression, &expected, true)?;
                &contextual
            } else {
                aty
            };
            if !self.record_implicit_conversion(expression, actual, &expected)? {
                return Err(TypeError::TypeMismatch {
                    expected: expected.to_string(),
                    found: actual.to_string(),
                    context: format!("argument to '{}'", name),
                });
            }
            conversions += conversion_count(actual, &expected);
        }
        if let Some(element) = kw_variadic.as_deref() {
            let expected = resolve(element)?;
            for (index, actual) in keyword_actuals {
                let expression = &kwargs[index].value;
                if !self.record_implicit_conversion(expression, &actual, &expected)? {
                    return Err(TypeError::TypeMismatch {
                        expected: expected.to_string(),
                        found: actual.to_string(),
                        context: format!(
                            "keyword '{}' collected by '{}'",
                            kwargs[index].name, name
                        ),
                    });
                }
                self.check_consuming(
                    expression,
                    &actual,
                    &format!("keyword '{}' collected by '{name}'", kwargs[index].name),
                )?;
                conversions += conversion_count(&actual, &expected);
            }
            if let Some(actual) = forwarded_element
                && actual != expected
            {
                return Err(TypeError::TypeMismatch {
                    expected: format!("StringDict[{expected}]"),
                    found: format!("StringDict[{actual}]"),
                    context: format!("forwarded keyword arguments to '{name}'"),
                });
            }
        }
        for (i, slot) in slots.iter().enumerate() {
            if matches!(
                conventions.get(i),
                Some(Some(ArgConvention::Var | ArgConvention::Deinit))
            ) {
                let arg = match slot {
                    ArgSlot::Positional(p) => &args[*p],
                    ArgSlot::Keyword(k) => &kwargs[*k].value,
                    ArgSlot::Default => continue,
                };
                let expected = resolve(&params[i])?;
                let ty = self.infer_with_expected(arg, &expected, true)?;
                self.check_consuming(arg, &ty, &format!("argument '{}' to '{}'", names[i], name))?;
            }
        }
        let (effective_conventions, return_ref) = self.solve_call_origins(
            &slots,
            conventions,
            ref_params,
            ref_return.as_deref(),
            args,
            kwargs,
        )?;
        let copied_reads = slots
            .iter()
            .enumerate()
            .map(|(index, slot)| {
                let expression = match slot {
                    ArgSlot::Positional(position) => &args[*position],
                    ArgSlot::Keyword(position) => &kwargs[*position].value,
                    ArgSlot::Default => return Ok(false),
                };
                let convention = effective_conventions.get(index).copied().flatten();
                Ok(
                    !matches!(convention, Some(ArgConvention::Mut | ArgConvention::Ref))
                        && self.is_copyable(&self.infer(expression)?),
                )
            })
            .collect::<Result<Vec<_>, TypeError>>()?;
        check_call_aliasing(&slots, &effective_conventions, &copied_reads, args, kwargs)?;
        let referent = self.canonicalize_public_tuple_types(resolve(ret)?);
        let result = return_ref
            .map(|mut reference| {
                reference.referent = Box::new(referent.clone());
                Ty::Ref(reference)
            })
            .unwrap_or(referent);
        let error = error.as_ref().map(|error| resolve(error)).transpose()?;
        Ok((
            result,
            overload_rank(
                conversions,
                variadic.is_some() || kw_variadic.is_some(),
                decls.len(),
                true,
            ),
            error,
        ))
    }

    fn forwarded_kwargs_element(
        &self,
        callee: &str,
        kwargs: &[crate::ast::KwArg],
    ) -> Result<Option<Ty>, TypeError> {
        let mut forwarded = kwargs.iter().filter(|argument| argument.is_forwarded());
        let Some(argument) = forwarded.next() else {
            return Ok(None);
        };
        if forwarded.next().is_some() {
            return Err(TypeError::BadCall {
                func: callee.to_string(),
                reason: "only one keyword dictionary can be forwarded".to_string(),
            });
        }
        if !matches!(&argument.value.kind, ExprKind::Transfer(_)) {
            return Err(TypeError::BadCall {
                func: callee.to_string(),
                reason: "keyword forwarding requires ownership transfer (`**kwargs^`)".to_string(),
            });
        }
        let found = self.infer(&argument.value)?;
        match found {
            Ty::Struct(name, args) if name == "StringDict" => match args.as_slice() {
                [TyArg::Ty(element)] => Ok(Some(element.clone())),
                _ => Err(TypeError::InvariantViolation(
                    "StringDict must carry one value type".to_string(),
                )),
            },
            other => Err(TypeError::TypeMismatch {
                expected: "StringDict[T]".to_string(),
                found: other.to_string(),
                context: format!("forwarded keyword arguments to '{callee}'"),
            }),
        }
    }

    fn solve_call_origins(
        &self,
        slots: &[ArgSlot],
        conventions: &[Option<ArgConvention>],
        signatures: &[Option<crate::origin::RefSig>],
        return_signature: Option<&crate::origin::RefSig>,
        args: &[Expr],
        kwargs: &[crate::ast::KwArg],
    ) -> Result<(Vec<Option<ArgConvention>>, Option<crate::origin::RefTy>), TypeError> {
        use crate::origin::{Mutability, Origin, RefTy, SigMutability};
        let mut effective = conventions.to_vec();
        let mut origins = vec![None; slots.len()];
        let mut mutable = vec![false; slots.len()];
        // The declaration convention, not the effective alias-checking
        // convention below, determines whether execution needs the caller's
        // place. An immutable `ref` becomes a shared read for conflict
        // checking, but the VM still needs its handle through the call.
        for (index, convention) in conventions.iter().enumerate() {
            if !matches!(convention, Some(ArgConvention::Mut | ArgConvention::Ref)) {
                continue;
            }
            let Some(slot) = slots.get(index) else {
                continue;
            };
            let expression = match slot {
                ArgSlot::Positional(position) => &args[*position],
                ArgSlot::Keyword(position) => &kwargs[*position].value,
                ArgSlot::Default => continue,
            };
            self.call_place_uses
                .borrow_mut()
                .insert(expression.source_span());
        }
        for (index, signature) in signatures.iter().enumerate() {
            let Some(signature) = signature else { continue };
            let Some(slot) = slots.get(index) else {
                continue;
            };
            let expression = match slot {
                ArgSlot::Positional(position) => &args[*position],
                ArgSlot::Keyword(position) => &kwargs[*position].value,
                ArgSlot::Default => continue,
            };
            let actual = self.reference_actual(expression)?;
            let is_mutable = actual.mutability == Mutability::Mutable;
            let requires_mutable = matches!(signature.mutability, SigMutability::Mutable);
            if requires_mutable && !is_mutable {
                return Err(TypeError::ImmutableBinding(
                    "reference argument".to_string(),
                ));
            }
            origins[index] = Some(actual.origin);
            mutable[index] = match signature.mutability {
                SigMutability::Immutable => false,
                SigMutability::Mutable => true,
                SigMutability::BoolParam(_) | SigMutability::Infer => is_mutable,
            };
            if !mutable[index] {
                effective[index] = Some(ArgConvention::Read);
            }
        }
        // A mutable or parametrically-mutable argument may redefine every
        // interior origin below the passed place. This is an explicit checked
        // call effect; lowering must not infer it from a generic `Call` place.
        for (index, convention) in effective.iter().enumerate() {
            if !matches!(convention, Some(ArgConvention::Mut | ArgConvention::Ref)) {
                continue;
            }
            let Some(slot) = slots.get(index) else {
                continue;
            };
            let expression = match slot {
                ArgSlot::Positional(position) => &args[*position],
                ArgSlot::Keyword(position) => &kwargs[*position].value,
                ArgSlot::Default => continue,
            };
            if let Some(origin) = origins.get(index).and_then(Clone::clone) {
                let except = match &expression.kind {
                    ExprKind::Identifier(name) if matches!(self.lookup(name), Some(Ty::Ref(_))) => {
                        self.lookup_owner(name)
                    }
                    _ => None,
                };
                self.record_aggregate_origin_invalidation_except(
                    expression.source_span(),
                    origin,
                    except,
                );
            } else {
                self.record_interior_invalidation(expression.source_span(), expression);
            }
        }
        for (index, signature) in signatures.iter().enumerate() {
            if signature.as_ref().is_some_and(|signature| {
                matches!(signature.origin, crate::origin::SigOrigin::Static)
            }) && origins
                .get(index)
                .and_then(Option::as_ref)
                .is_some_and(|origin| !matches!(origin, Origin::Static))
            {
                return Err(TypeError::Unsupported(
                    "a local place cannot satisfy StaticOrigin".to_string(),
                ));
            }
            if let Some(signature) = signature
                && sig_origin_has_bound(&signature.origin)
                && let Some(actual) = origins.get(index).and_then(Option::as_ref)
            {
                let allowed = substitute_sig_origin(&signature.origin, &origins);
                if !origin_is_within(actual, &allowed) {
                    return Err(TypeError::TypeMismatch {
                        expected: format!("the specialized origin {allowed:?}"),
                        found: format!("the argument origin {actual:?}"),
                        context: "call through an origin-specialized function value".to_string(),
                    });
                }
            }
        }
        let returned = return_signature.map(|signature| {
            let origin = substitute_sig_origin(&signature.origin, &origins);
            let is_mutable = match &signature.mutability {
                SigMutability::Immutable => false,
                SigMutability::Mutable => true,
                SigMutability::BoolParam(parameter) => {
                    signatures.iter().enumerate().any(|(i, sig)| {
                    sig.as_ref().is_some_and(|sig| {
                            matches!(sig.mutability, SigMutability::BoolParam(other) if other == *parameter)
                            && mutable[i]
                    })
                    })
                }
                SigMutability::Infer => origins
                    .iter()
                    .enumerate()
                    .any(|(i, o)| o.is_some() && mutable[i]),
            };
            RefTy {
                referent: Box::new(Ty::None), // replaced by the caller's declared return type
                origin,
                mutability: if is_mutable {
                    Mutability::Mutable
                } else {
                    Mutability::Immutable
                },
            }
        });
        Ok((effective, returned))
    }

    /// Type `print(...)`. Intrinsic scalars have builtin writing; nominal values,
    /// including public collections, opt into current `Writable`. During tuple
    /// specialization discovery an as-yet-unmaterialized nominal shape is checked
    /// element-wise; executable values always cross the concrete struct boundary.
    fn infer_print(&self, args: &[Expr]) -> Result<Ty, TypeError> {
        for (i, arg) in args.iter().enumerate() {
            let ty = self.infer(arg)?;
            let runtime_ty = default_literal(&ty);
            if runtime_ty != ty {
                self.record_literal_materializations(arg, &ty, &runtime_ty)?;
            }
            if let Ty::Struct(name, _) = &ty
                && !self.structs.contains_key(name)
                && (list_element(&ty).is_some_and(is_printable)
                    || set_element(&ty).is_some_and(is_printable)
                    || dict_elements(&ty)
                        .is_some_and(|(key, value)| is_printable(key) && is_printable(value))
                    || tuple_elements(&ty)
                        .is_some_and(|elements| elements.into_iter().all(is_printable)))
            {
                continue;
            }
            if matches!(ty, Ty::Struct(..) | Ty::Variant(_)) {
                if self.conforms_to(&ty, "Writable") {
                    continue;
                }
                return Err(TypeError::TypeMismatch {
                    expected: "Writable".to_string(),
                    found: ty.to_string(),
                    context: format!("argument {} to 'print'", i + 1),
                });
            }
            if matches!(ty, Ty::Param { .. }) && self.conforms_to(&ty, "Writable") {
                continue;
            }
            if !is_printable(&ty) {
                return Err(TypeError::TypeMismatch {
                    expected: "a printable value".to_string(),
                    found: ty.to_string(),
                    context: format!("argument {} to 'print'", i + 1),
                });
            }
        }
        Ok(Ty::None)
    }

    /// Type the built-in `input(prompt)`: prompt must be a `String`, result is the
    /// line read from standard input as a `String`.
    fn infer_input(&self, args: &[Expr]) -> Result<Ty, TypeError> {
        let tys = self.builtin_args("input", 1, args)?;
        if tys[0] == Ty::String {
            Ok(Ty::String)
        } else {
            Err(TypeError::TypeMismatch {
                expected: "String".to_string(),
                found: tys[0].to_string(),
                context: "argument to 'input'".to_string(),
            })
        }
    }

    /// Require a built-in call to have exactly `n` arguments, and return the
    /// inferred type of each.
    fn builtin_args(&self, name: &str, n: usize, args: &[Expr]) -> Result<Vec<Ty>, TypeError> {
        if args.len() != n {
            return Err(TypeError::ArityMismatch {
                name: name.to_string(),
                expected: n,
                got: args.len(),
            });
        }
        args.iter().map(|a| self.infer(a)).collect()
    }

    /// Type `String(x)`: stringify a numeric, `Bool`, or `String` value.
    fn infer_stringify(&self, args: &[Expr]) -> Result<Ty, TypeError> {
        let tys = self.builtin_args("String", 1, args)?;
        if is_numeric(&tys[0]) || tys[0] == Ty::Bool || tys[0] == Ty::String {
            let runtime_ty = default_literal(&tys[0]);
            if runtime_ty != tys[0] {
                self.record_literal_materializations(&args[0], &tys[0], &runtime_ty)?;
            }
            return Ok(Ty::String);
        }
        if self.conforms_to(&tys[0], "Writable") {
            // Like `print`, nominal String conversion formats through a
            // borrowed `Writable` receiver and must retain its caller storage
            // until that synchronous formatter returns.
            self.call_place_uses
                .borrow_mut()
                .insert(args[0].source_span());
            return Ok(Ty::String);
        }
        Err(TypeError::TypeMismatch {
            expected: "Writable".to_string(),
            found: tys[0].to_string(),
            context: "argument to 'String'".to_string(),
        })
    }

    /// Type `abs(x)`: a numeric argument, returning the same numeric type.
    fn infer_abs(&self, args: &[Expr]) -> Result<Ty, TypeError> {
        let tys = self.builtin_args("abs", 1, args)?;
        // A numeric value, or an opaque `T: Absable` — `abs` returns the same type
        // (`__abs__(self) -> Self`); the concrete impl runs after type erasure.
        if is_numeric(&tys[0]) || param_has_bound(&tys[0], "Absable") {
            Ok(tys[0].clone())
        } else if let Some(result) = self.struct_dunder(&tys[0], "__abs__", &[]) {
            // A concrete struct routes through `__abs__(self) -> Self`.
            result
        } else {
            Err(TypeError::TypeMismatch {
                expected: "a numeric value".to_string(),
                found: tys[0].to_string(),
                context: "argument to 'abs'".to_string(),
            })
        }
    }

    /// Type `min(a, b)` / `max(a, b)`: two numeric arguments unified like an
    /// operator (no concrete-type mixing), returning their common type.
    fn infer_min_max(&self, name: &str, args: &[Expr]) -> Result<Ty, TypeError> {
        let tys = self.builtin_args(name, 2, args)?;
        common_numeric(&tys[0], &tys[1]).ok_or_else(|| TypeError::BadOperator {
            op: name.to_string(),
            operands: format!("{} and {}", tys[0], tys[1]),
        })
    }

    /// Type `round(x)`: a `Float64` argument returning `Float64`, or an opaque
    /// `T: Roundable` returning the same type (`__round__(self) -> Self`; the
    /// concrete impl runs after type erasure).
    fn infer_round(&self, args: &[Expr]) -> Result<Ty, TypeError> {
        let tys = self.builtin_args("round", 1, args)?;
        if matches!(tys[0], Ty::Float64 | Ty::FloatLiteral) {
            Ok(Ty::Float64)
        } else if param_has_bound(&tys[0], "Roundable") {
            Ok(tys[0].clone())
        } else if let Some(result) = self.struct_dunder(&tys[0], "__round__", &[]) {
            // A concrete struct routes through `__round__(self) -> Self`.
            result
        } else {
            Err(TypeError::TypeMismatch {
                expected: "Float64".to_string(),
                found: tys[0].to_string(),
                context: "argument to 'round'".to_string(),
            })
        }
    }

    fn len_result_for_type(&self, ty: &Ty) -> Result<Option<Ty>, TypeError> {
        if let Ty::Dependent(DependentType::Indexed { elements, .. }) = ty {
            for element in elements {
                match self.len_result_for_type(element)? {
                    Some(Ty::Int) => {}
                    _ => return Ok(None),
                }
            }
            return Ok(Some(Ty::Int));
        }
        if matches!(
            ty,
            Ty::String
                | Ty::ComptimeList(_)
                | Ty::Tuple(_)
                | Ty::RuntimePack(_)
                | Ty::VariadicPack(_)
        ) {
            return Ok(Some(Ty::Int));
        }
        if let Ty::Struct(name, _) = ty
            && !self.structs.contains_key(name)
            && (list_element(ty).is_some()
                || set_element(ty).is_some()
                || dict_elements(ty).is_some()
                || tuple_elements(ty).is_some()
                || crate::types::is_range_type(ty))
        {
            return Ok(Some(Ty::Int));
        }
        // `len(c)` on a user struct dispatches to `c.__len__()` (`Sized`), which
        // must return `Int`.
        if let Some(result) = self.struct_dunder(ty, "__len__", &[]) {
            return result
                .and_then(|ret| require_dunder_ret(ret, &Ty::Int, "__len__"))
                .map(Some);
        }
        // `len(x)` on an opaque type parameter is permitted when its bound
        // promises a length (`T: Sized`) — the concrete type's `__len__` runs at
        // runtime after type erasure.
        if has_len_bound(ty) {
            return Ok(Some(Ty::Int));
        }
        Ok(None)
    }

    /// Type `len(x)`: every possible type of a dependent input must fulfill the
    /// same `Sized`/`__len__ -> Int` contract.
    fn infer_len(&self, args: &[Expr]) -> Result<Ty, TypeError> {
        let tys = self.builtin_args("len", 1, args)?;
        if let Some(result) = self.len_result_for_type(&tys[0])? {
            return Ok(result);
        }
        Err(TypeError::TypeMismatch {
            expected: "String, List, or Tuple".to_string(),
            found: tys[0].to_string(),
            context: "argument to 'len'".to_string(),
        })
    }

    /// Type the built-in `range(stop)` / `range(start, stop)` /
    /// `range(start, stop, step)`. All arguments must be `Int`; the result is a
    /// `range`. A zero `step` is a *runtime* value error, not a type error.
    fn infer_range(&self, args: &[Expr]) -> Result<Ty, TypeError> {
        if args.is_empty() {
            return Err(TypeError::ArityMismatch {
                name: "range".to_string(),
                expected: 1,
                got: 0,
            });
        }
        if args.len() > 3 {
            return Err(TypeError::ArityMismatch {
                name: "range".to_string(),
                expected: 3,
                got: args.len(),
            });
        }
        for (i, arg) in args.iter().enumerate() {
            let arg_ty = self.infer(arg)?;
            if !coerces(&arg_ty, &Ty::Int) {
                return Err(TypeError::TypeMismatch {
                    expected: "Int".to_string(),
                    found: arg_ty.to_string(),
                    context: format!("argument {} to 'range'", i + 1),
                });
            }
            self.record_literal_materializations(arg, &arg_ty, &Ty::Int)?;
        }
        Ok(range_type())
    }

    /// Type a conversion built-in `Int(x)` / `UInt(x)` / `Float64(x)` / `Bool(x)`:
    /// exactly one argument of a numeric or `Bool` type, producing `target`. An
    /// opaque type parameter is also accepted when its bound promises the
    /// conversion — `Int(x)` on `T: Intable`, `Float64(x)` on `T: Floatable`,
    /// `Bool(x)` on `T: Boolable` (`__int__`/`__float__`/`__bool__` run after
    /// type erasure).
    fn infer_conversion(&self, target: Ty, args: &[Expr]) -> Result<Ty, TypeError> {
        if args.len() != 1 {
            return Err(TypeError::ArityMismatch {
                name: target.to_string(),
                expected: 1,
                got: args.len(),
            });
        }
        let arg_ty = self.infer(&args[0])?;
        // A concrete value routes through its conversion dunder
        // (`Int(x)` → `x.__int__() -> Int`, `Float64`/`Bool` likewise); the
        // same protocol an opaque `T: Intable/Floatable/Boolable` uses.
        let conversion = match target {
            Ty::Int => Some(("__int__", Ty::Int)),
            Ty::Float64 => Some(("__float__", Ty::Float64)),
            Ty::Bool => Some(("__bool__", Ty::Bool)),
            _ => None,
        };
        if let Some((dunder, expected)) = &conversion
            && let Some(result) = self.struct_dunder(&arg_ty, dunder, &[])
        {
            require_dunder_ret(result?, expected, dunder)?;
            return Ok(target);
        }
        let bounded = match target {
            Ty::Int => param_has_bound(&arg_ty, "Intable"),
            Ty::Float64 => param_has_bound(&arg_ty, "Floatable"),
            Ty::Bool => param_has_bound(&arg_ty, "Boolable"),
            _ => false,
        };
        if !(is_numeric(&arg_ty) || arg_ty == Ty::Bool || bounded) {
            return Err(TypeError::TypeMismatch {
                expected: "a numeric or Bool value".to_string(),
                found: arg_ty.to_string(),
                context: format!("argument to '{}'", target),
            });
        }
        Ok(target)
    }

    /// Type the prelude built-in `divmod(a, b)` (`DivModable`) → `Tuple[T, T]`:
    /// two numeric arguments of a common type (like an operator), or two equal
    /// opaque type parameters bounded by `DivModable`.
    fn infer_divmod(&self, args: &[Expr]) -> Result<Ty, TypeError> {
        let tys = self.builtin_args("divmod", 2, args)?;
        if let Some(common) = common_numeric(&tys[0], &tys[1]) {
            return Ok(self.public_tuple_type(vec![common.clone(), common]));
        }
        if tys[0] == tys[1] && param_has_bound(&tys[0], "DivModable") {
            return Ok(self.public_tuple_type(vec![tys[0].clone(), tys[0].clone()]));
        }
        Err(TypeError::BadOperator {
            op: "divmod".to_string(),
            operands: format!("{} and {}", tys[0], tys[1]),
        })
    }
}

/// Mojo's built-in traits that mojito recognizes in a type-parameter bound.
/// User-defined traits (and conformance checking) are a later phase, so a bound
/// must name one of these. `AnyType` is the least restrictive.
const BUILTIN_TRAITS: &[&str] = &[
    "AnyType",
    "ImplicitlyDeletable",
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

mod places;
use places::*;

mod generics;
use generics::*;
mod declarations;
use declarations::*;

mod annotations;
use annotations::*;

mod calls;
use calls::*;
mod builtins;
pub(crate) use builtins::callable_environment_coerces;
use builtins::*;

impl Default for Checker {
    fn default() -> Self {
        Self::new()
    }
}

/// Recognize the current-nightly origin-attribute spelling
/// `base._get_owned_interior["tag"]`. It is accepted only in origin clauses;
/// ordinary expression typing still has no runtime member by this name.
fn interior_origin_syntax(expr: &Expr) -> Option<(&Expr, &str)> {
    let ExprKind::Index { object, index } = &expr.kind else {
        return None;
    };
    let ExprKind::Member {
        object: base,
        field,
    } = &object.kind
    else {
        return None;
    };
    let ExprKind::Str(name) = &index.kind else {
        return None;
    };
    (field == "_get_owned_interior").then_some((base, name.as_str()))
}

fn validate_origin_expr(
    expr: &Expr,
    origin_params: &HashSet<&str>,
    value_params: &HashSet<&str>,
) -> Result<(), TypeError> {
    if let Some((base, _)) = interior_origin_syntax(expr) {
        return validate_origin_expr(base, origin_params, value_params);
    }
    match &expr.kind {
        ExprKind::Identifier(name)
            if name == "_"
                || name == "self"
                || name == "StaticOrigin"
                || name == "UntrackedOrigin"
                || name == "UnsafeAnyOrigin"
                || origin_params.contains(name.as_str())
                || value_params.contains(name.as_str()) =>
        {
            Ok(())
        }
        ExprKind::Call {
            name,
            args,
            kwargs,
            param_args,
        } if name == "origin_of" && kwargs.is_empty() && param_args.is_empty() => {
            if args.is_empty() {
                return Err(TypeError::Unsupported(
                    "origin_of requires at least one parameter place".to_string(),
                ));
            }
            for argument in args {
                let Some((root, _)) = place_path(argument) else {
                    return Err(TypeError::Unsupported(
                        "origin_of requires parameter places".to_string(),
                    ));
                };
                if root != "self" && !value_params.contains(root) {
                    return Err(TypeError::UndefinedVariable(root.to_string()));
                }
            }
            Ok(())
        }
        ExprKind::Member { .. } | ExprKind::Index { .. } => {
            let Some((root, _)) = place_path(expr) else {
                return Err(TypeError::Unsupported("invalid origin place".to_string()));
            };
            if root == "self" || value_params.contains(root) {
                Ok(())
            } else {
                Err(TypeError::UndefinedVariable(root.to_string()))
            }
        }
        ExprKind::Identifier(name) => Err(TypeError::UndefinedVariable(name.clone())),
        _ => Err(TypeError::Unsupported(
            "origin clauses must name origins or parameter places".to_string(),
        )),
    }
}

fn lower_ref_param_sigs(
    type_params: &[crate::ast::TypeParam],
    params: &[&FnParam],
) -> Result<Vec<Option<crate::origin::RefSig>>, TypeError> {
    params
        .iter()
        .map(|param| {
            if param.convention != Some(ArgConvention::Ref) {
                return Ok(None);
            }
            match &param.origin {
                Some(spec) => lower_ref_sig(spec, type_params, params).map(Some),
                None => Ok(Some(crate::origin::RefSig {
                    origin: crate::origin::SigOrigin::Infer,
                    mutability: crate::origin::SigMutability::Infer,
                })),
            }
        })
        .collect()
}

fn callable_origin_signature(
    type_params: &[crate::ast::TypeParam],
    params: &[&FnParam],
) -> CallableOriginSignature {
    let origins = type_params
        .iter()
        .filter(|parameter| parameter.bounds.as_slice() == ["Origin"])
        .map(|parameter| CallableOriginParam {
            name: parameter.name.clone(),
            slots: params
                .iter()
                .enumerate()
                .filter_map(|(index, value_parameter)| {
                    value_parameter
                        .origin
                        .as_ref()
                        .is_some_and(|origin| {
                            origin.iter().any(|expression| {
                                matches!(
                                    &expression.kind,
                                    ExprKind::Identifier(name) if name == &parameter.name
                                )
                            })
                        })
                        .then_some(index)
                })
                .collect(),
        })
        .collect::<Vec<_>>();
    let source = type_params
        .iter()
        .map(|parameter| CallableSourceParam {
            name: parameter.name.clone(),
            infer_only: parameter.infer_only,
            origin: origins
                .iter()
                .position(|origin| origin.name == parameter.name),
            ordinary: !matches!(
                parameter.bounds.as_slice(),
                [only] if only == "Origin" || only == "OriginSet"
            ),
        })
        .collect();
    CallableOriginSignature { origins, source }
}

fn lower_ref_sig(
    spec: &crate::ast::OriginSpec,
    type_params: &[crate::ast::TypeParam],
    params: &[&FnParam],
) -> Result<crate::origin::RefSig, TypeError> {
    use crate::origin::{RefSig, SigMutability, SigOrigin};
    let mut members = Vec::new();
    let mut mutability = SigMutability::Infer;
    for expression in spec {
        if let Some((base, name)) = interior_origin_syntax(expression) {
            let base = lower_sig_origin_expression(base, type_params, params)?;
            members.push(SigOrigin::Projected(
                Box::new(base),
                vec![crate::origin::OriginSeg::Interior(name.to_string())],
            ));
            continue;
        }
        match &expression.kind {
            ExprKind::Identifier(name) if name == "_" => members.push(SigOrigin::Infer),
            ExprKind::Identifier(name) if name == "self" => members.push(SigOrigin::Self_),
            ExprKind::Identifier(name) if name == "StaticOrigin" => {
                members.push(SigOrigin::Static);
                mutability = SigMutability::Immutable;
            }
            ExprKind::Identifier(name) if name == "UntrackedOrigin" => {
                members.push(SigOrigin::Untracked { mutable: false });
                mutability = SigMutability::Immutable;
            }
            ExprKind::Identifier(name) if name == "UnsafeAnyOrigin" => {
                members.push(SigOrigin::Untracked { mutable: true });
                mutability = SigMutability::Mutable;
            }
            ExprKind::Identifier(name) => {
                if let Some(index) = params.iter().position(|param| param.name == *name) {
                    members.push(SigOrigin::Param(index));
                    continue;
                }
                let (origin_param_index, origin_param) = type_params
                    .iter()
                    .enumerate()
                    .find(|(_, param)| param.name == *name && param.bounds.as_slice() == ["Origin"])
                    .ok_or_else(|| TypeError::UndefinedVariable(name.clone()))?;
                mutability = match origin_param.origin_mutability.as_ref().map(|e| &e.kind) {
                    Some(ExprKind::Bool(true)) => SigMutability::Mutable,
                    Some(ExprKind::Bool(false)) => SigMutability::Immutable,
                    Some(ExprKind::Identifier(value)) => SigMutability::BoolParam(
                        type_params
                            .iter()
                            .position(|parameter| {
                                parameter.name == *value && parameter.bounds.as_slice() == ["Bool"]
                            })
                            .expect("validated Origin mutability names a Bool parameter"),
                    ),
                    _ => SigMutability::Infer,
                };
                let first_member = members.len();
                for (index, param) in params.iter().enumerate() {
                    if param.origin.as_ref().is_some_and(|origin| {
                        matches!(origin.as_slice(), [Expr { kind: ExprKind::Identifier(bound), .. }] if bound == name)
                    }) {
                        members.push(SigOrigin::Param(index));
                    }
                }
                // An enclosing struct Origin can be carried by reference-valued
                // fields even when no ordinary method parameter binds it. Keep
                // that checked semantic binder directly in the method contract
                // instead of collapsing it to an empty inferred union.
                if members.len() == first_member {
                    members.push(SigOrigin::Bound(crate::origin::Origin::Param(
                        crate::origin::OriginParamId(origin_param_index as u32),
                    )));
                }
            }
            ExprKind::Call { name, args, .. } if name == "origin_of" => {
                for argument in args {
                    let (root, path) = place_path(argument).ok_or_else(|| {
                        TypeError::Unsupported("origin_of requires parameter places".to_string())
                    })?;
                    let base = if root == "self" {
                        SigOrigin::Self_
                    } else {
                        let index = params
                            .iter()
                            .position(|param| param.name == root)
                            .ok_or_else(|| TypeError::UndefinedVariable(root.to_string()))?;
                        SigOrigin::Param(index)
                    };
                    members.push(project_sig_origin(base, &path));
                }
            }
            ExprKind::Member { .. } | ExprKind::Index { .. } => {
                let (root, path) = place_path(expression)
                    .ok_or_else(|| TypeError::Unsupported("invalid origin place".to_string()))?;
                let base = if root == "self" {
                    SigOrigin::Self_
                } else {
                    let index = params
                        .iter()
                        .position(|param| param.name == root)
                        .ok_or_else(|| TypeError::UndefinedVariable(root.to_string()))?;
                    SigOrigin::Param(index)
                };
                members.push(project_sig_origin(base, &path));
            }
            _ => {
                return Err(TypeError::Unsupported(
                    "unsupported origin contract".to_string(),
                ));
            }
        }
    }
    members.sort_by_key(|member| match member {
        SigOrigin::Self_ => 0,
        SigOrigin::Param(i) => i + 1,
        _ => usize::MAX,
    });
    members.dedup();
    let origin = match members.as_slice() {
        [] => SigOrigin::Infer,
        [single] => single.clone(),
        _ => SigOrigin::union(members),
    };
    Ok(RefSig { origin, mutability })
}

fn lower_sig_origin_expression(
    expression: &Expr,
    type_params: &[crate::ast::TypeParam],
    params: &[&FnParam],
) -> Result<crate::origin::SigOrigin, TypeError> {
    use crate::origin::SigOrigin;
    if let Some((base, name)) = interior_origin_syntax(expression) {
        return Ok(SigOrigin::Projected(
            Box::new(lower_sig_origin_expression(base, type_params, params)?),
            vec![crate::origin::OriginSeg::Interior(name.to_string())],
        ));
    }
    match &expression.kind {
        ExprKind::Identifier(name) if name == "self" => Ok(SigOrigin::Self_),
        ExprKind::Identifier(name) => {
            if let Some(index) = params.iter().position(|parameter| parameter.name == *name) {
                return Ok(SigOrigin::Param(index));
            }
            if type_params.iter().any(|parameter| {
                parameter.name == *name && parameter.bounds.as_slice() == ["Origin"]
            }) {
                // A named origin parameter is represented by the value
                // parameter(s) carrying it in this callable contract.
                let members = params
                    .iter()
                    .enumerate()
                    .filter_map(|(index, parameter)| {
                        parameter.origin.as_ref().is_some_and(|origin| {
                            matches!(origin.as_slice(), [Expr { kind: ExprKind::Identifier(bound), .. }] if bound == name)
                        }).then_some(SigOrigin::Param(index))
                    })
                    .collect::<Vec<_>>();
                return Ok(match members.as_slice() {
                    [] => SigOrigin::Infer,
                    [single] => single.clone(),
                    _ => SigOrigin::union(members),
                });
            }
            Err(TypeError::UndefinedVariable(name.clone()))
        }
        ExprKind::Call {
            name,
            args,
            kwargs,
            param_args,
        } if name == "origin_of" && kwargs.is_empty() && param_args.is_empty() => {
            let members = args
                .iter()
                .map(|argument| {
                    let (root, path) = place_path(argument).ok_or_else(|| {
                        TypeError::Unsupported("origin_of requires parameter places".to_string())
                    })?;
                    let base = if root == "self" {
                        SigOrigin::Self_
                    } else {
                        let index = params
                            .iter()
                            .position(|parameter| parameter.name == root)
                            .ok_or_else(|| TypeError::UndefinedVariable(root.to_string()))?;
                        SigOrigin::Param(index)
                    };
                    Ok(project_sig_origin(base, &path))
                })
                .collect::<Result<Vec<_>, TypeError>>()?;
            Ok(match members.as_slice() {
                [single] => single.clone(),
                _ => SigOrigin::union(members),
            })
        }
        ExprKind::Member { .. } | ExprKind::Index { .. } => {
            let (root, path) = place_path(expression)
                .ok_or_else(|| TypeError::Unsupported("invalid origin place".to_string()))?;
            let base = if root == "self" {
                SigOrigin::Self_
            } else {
                let index = params
                    .iter()
                    .position(|parameter| parameter.name == root)
                    .ok_or_else(|| TypeError::UndefinedVariable(root.to_string()))?;
                SigOrigin::Param(index)
            };
            Ok(project_sig_origin(base, &path))
        }
        _ => Err(TypeError::Unsupported(
            "unsupported origin contract".to_string(),
        )),
    }
}

fn project_sig_origin(
    base: crate::origin::SigOrigin,
    path: &[PlaceSeg],
) -> crate::origin::SigOrigin {
    crate::origin::SigOrigin::Projected(
        Box::new(base),
        path.iter()
            .map(|segment| match segment {
                PlaceSeg::Field(name) => crate::origin::OriginSeg::Field(name.clone()),
                PlaceSeg::Index => crate::origin::OriginSeg::AnyIndex,
            })
            .collect(),
    )
}

fn project_origin(
    origin: crate::origin::Origin,
    path: &[crate::origin::OriginSeg],
) -> crate::origin::Origin {
    use crate::origin::Origin;
    match origin {
        Origin::Place(mut place) => {
            place.path.extend_from_slice(path);
            Origin::Place(place)
        }
        Origin::Union(members) => Origin::union(
            members
                .into_iter()
                .map(|member| project_origin(member, path)),
        ),
        other => other,
    }
}

/// Replace the slot-relative parts belonging to source `Origin` parameters
/// with the concrete caller origins captured by a specialized function value.
fn bind_sig_origin(
    signature: &crate::origin::SigOrigin,
    bindings: &[(Vec<usize>, crate::origin::Origin)],
) -> crate::origin::SigOrigin {
    use crate::origin::SigOrigin;
    match signature {
        SigOrigin::Param(index) => bindings
            .iter()
            .find(|(slots, _)| slots.contains(index))
            .map(|(_, origin)| SigOrigin::Bound(origin.clone()))
            .unwrap_or_else(|| signature.clone()),
        SigOrigin::Projected(base, path) => {
            SigOrigin::Projected(Box::new(bind_sig_origin(base, bindings)), path.clone())
        }
        SigOrigin::Union(members) => SigOrigin::union(
            members
                .iter()
                .map(|member| bind_sig_origin(member, bindings)),
        ),
        _ => signature.clone(),
    }
}

fn sig_origin_has_bound(signature: &crate::origin::SigOrigin) -> bool {
    use crate::origin::SigOrigin;
    match signature {
        SigOrigin::Bound(_) => true,
        SigOrigin::Projected(base, _) => sig_origin_has_bound(base),
        SigOrigin::Union(members) => members.iter().any(sig_origin_has_bound),
        _ => false,
    }
}

fn substitute_sig_origin(
    signature: &crate::origin::SigOrigin,
    actual: &[Option<crate::origin::Origin>],
) -> crate::origin::Origin {
    use crate::origin::{Origin, SigOrigin};
    match signature {
        SigOrigin::Self_ => Origin::Union(vec![]),
        SigOrigin::Bound(origin) => origin.clone(),
        SigOrigin::Param(index) => actual
            .get(*index)
            .and_then(Clone::clone)
            .unwrap_or(Origin::Union(vec![])),
        SigOrigin::Static => Origin::Static,
        SigOrigin::Untracked { mutable } => Origin::Untracked { mutable: *mutable },
        SigOrigin::Projected(base, path) => {
            project_origin(substitute_sig_origin(base, actual), path)
        }
        SigOrigin::Union(members) => Origin::union(
            members
                .iter()
                .map(|member| substitute_sig_origin(member, actual)),
        ),
        SigOrigin::Infer => Origin::union(actual.iter().filter_map(Clone::clone)),
    }
}

fn substitute_sig_origin_with_self(
    signature: &crate::origin::SigOrigin,
    actual: &[Option<crate::origin::Origin>],
    self_origin: Option<crate::origin::Origin>,
) -> crate::origin::Origin {
    use crate::origin::{Origin, SigOrigin};
    match signature {
        SigOrigin::Self_ => self_origin.clone().unwrap_or_else(|| Origin::Union(vec![])),
        SigOrigin::Union(members) => Origin::union(
            members
                .iter()
                .map(|member| substitute_sig_origin_with_self(member, actual, self_origin.clone())),
        ),
        SigOrigin::Projected(base, path) => project_origin(
            substitute_sig_origin_with_self(base, actual, self_origin),
            path,
        ),
        _ => substitute_sig_origin(signature, actual),
    }
}

fn origin_is_within(actual: &crate::origin::Origin, allowed: &crate::origin::Origin) -> bool {
    use crate::origin::Origin;
    match actual {
        Origin::Union(members) => members
            .iter()
            .all(|member| origin_is_within(member, allowed)),
        _ => match allowed {
            Origin::Union(members) => members
                .iter()
                .any(|member| origin_is_within(actual, member)),
            _ => actual.overlaps(allowed),
        },
    }
}

fn ref_parameter_is_writable(parameter: &FnParam, type_params: &[crate::ast::TypeParam]) -> bool {
    ref_binding_is_writable(
        parameter.convention,
        parameter.origin.as_deref(),
        type_params,
    )
}

/// Whether a parameter/receiver may be mutated while its generic body is
/// checked. A bare `ref` has parametric mutability: it propagates the caller's
/// capability to returned references, but its body cannot assume that the
/// caller supplied mutable storage. Only an explicitly mutable origin grants
/// unconditional write access.
fn ref_binding_is_writable(
    convention: Option<ArgConvention>,
    origin: Option<&[Expr]>,
    type_params: &[crate::ast::TypeParam],
) -> bool {
    if convention != Some(ArgConvention::Ref) {
        return parameter_is_writable(convention);
    }
    let Some(
        [
            Expr {
                kind: ExprKind::Identifier(origin_name),
                ..
            },
        ],
    ) = origin
    else {
        return false;
    };
    if origin_name == "UnsafeAnyOrigin" {
        return true;
    }
    let Some(origin) = type_params.iter().find(|candidate| {
        candidate.name == *origin_name && candidate.bounds.as_slice() == ["Origin"]
    }) else {
        return false;
    };
    matches!(
        origin.origin_mutability.as_ref().map(|expr| &expr.kind),
        Some(ExprKind::Bool(true))
    )
}

/// The linker qualifies `from std.utils import Variant` declarations.  Keep the
/// intrinsic recognition narrow so an unrelated user type ending in `Variant`
/// does not silently acquire built-in semantics.
fn is_variant_name(name: &str) -> bool {
    matches!(
        name,
        "Variant" | "__module$std$utilsVariant" | "__module$std$utils$Variant"
    )
}

/// Whether control can leave the owned iterator before exhaustion. A `break`
/// belongs to the nearest loop, while return/raise escape through every nested
/// loop. This is used only when residual elements cannot be deleted implicitly.
fn block_can_escape_owned_iteration(statements: &[Stmt], nested_loops: usize) -> bool {
    statements.iter().any(|statement| match &statement.kind {
        StmtKind::Break => nested_loops == 0,
        StmtKind::Return(_) | StmtKind::Raise(_) => true,
        StmtKind::If { branches, orelse } | StmtKind::ComptimeIf { branches, orelse } => {
            branches
                .iter()
                .any(|(_, body)| block_can_escape_owned_iteration(body, nested_loops))
                || orelse
                    .as_ref()
                    .is_some_and(|body| block_can_escape_owned_iteration(body, nested_loops))
        }
        StmtKind::While { body, orelse, .. } | StmtKind::For { body, orelse, .. } => {
            block_can_escape_owned_iteration(body, nested_loops + 1)
                || orelse
                    .as_ref()
                    .is_some_and(|body| block_can_escape_owned_iteration(body, nested_loops))
        }
        StmtKind::Try {
            body,
            except,
            orelse,
            finalbody,
        } => {
            block_can_escape_owned_iteration(body, nested_loops)
                || except
                    .as_ref()
                    .is_some_and(|(_, body)| block_can_escape_owned_iteration(body, nested_loops))
                || orelse
                    .as_ref()
                    .is_some_and(|body| block_can_escape_owned_iteration(body, nested_loops))
                || finalbody
                    .as_ref()
                    .is_some_and(|body| block_can_escape_owned_iteration(body, nested_loops))
        }
        StmtKind::With { body, .. } => block_can_escape_owned_iteration(body, nested_loops),
        // Nested declarations do not execute as part of the loop body.
        StmtKind::Def { .. } | StmtKind::Struct { .. } | StmtKind::Trait { .. } => false,
        _ => false,
    })
}

/// A readable symbol for an infix operator, for error messages.
fn infix_symbol(op: InfixOp) -> &'static str {
    match op {
        InfixOp::Add => "+",
        InfixOp::Sub => "-",
        InfixOp::Mul => "*",
        InfixOp::Div => "/",
        InfixOp::FloorDiv => "//",
        InfixOp::Mod => "%",
        InfixOp::MatMul => "@",
        InfixOp::Shl => "<<",
        InfixOp::Shr => ">>",
        InfixOp::BitAnd => "&",
        InfixOp::BitOr => "|",
        InfixOp::BitXor => "^",
        InfixOp::Pow => "**",
        InfixOp::Eq => "==",
        InfixOp::Ne => "!=",
        InfixOp::Lt => "<",
        InfixOp::Gt => ">",
        InfixOp::Le => "<=",
        InfixOp::Ge => ">=",
        InfixOp::And => "and",
        InfixOp::Or => "or",
        InfixOp::In => "in",
        InfixOp::NotIn => "not in",
    }
}

/// A readable symbol for a prefix operator, for error messages.
fn prefix_symbol(op: PrefixOp) -> &'static str {
    match op {
        PrefixOp::Neg => "-",
        PrefixOp::Not => "not",
    }
}

#[cfg(test)]
mod dependent_callable_signature_tests {
    use super::*;

    fn indexed_callable(binder: &str, offset: i64) -> Ty {
        let index = if offset == 0 {
            CtExpr::Param(binder.to_string())
        } else {
            CtExpr::Add(
                Box::new(CtExpr::Param(binder.to_string())),
                Box::new(CtExpr::Value(CtValue::Int(offset))),
            )
        };
        Ty::GenericFunc {
            environment: crate::origin::CallableEnvironment::Thin,
            decls: vec![ParamDecl::Value {
                name: binder.to_string(),
                ty: Box::new(Ty::Int),
                default: None,
                callable_default: None,
                infer_only: false,
                variadic: false,
                constraints: Vec::new(),
            }],
            params: vec![Ty::Dependent(DependentType::Indexed {
                elements: vec![Ty::Int, Ty::String],
                index,
            })],
            names: vec!["element".to_string()],
            ret: Box::new(Ty::None),
            required: vec![true],
            variadic: None,
            kw_variadic: None,
            positional_only: None,
            keyword_only: None,
            raises: false,
            error: None,
            conventions: vec![Some(ArgConvention::Var)],
            ref_params: Box::new(vec![None]),
            ref_return: None,
        }
    }

    #[test]
    fn dependent_callable_binders_are_alpha_equivalent() {
        assert!(same_callable_signature(
            &indexed_callable("index", 0),
            &indexed_callable("i", 0),
        ));
        assert!(!same_callable_signature(
            &indexed_callable("index", 0),
            &indexed_callable("i", 1),
        ));
    }
}
