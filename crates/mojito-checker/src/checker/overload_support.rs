//! Overload and signature support: canonical generic signatures,
//! callable identity/equivalence, lowered names, transfer-effect
//! wrapping, ranking, and overload selection.

use super::*;
pub use mojito_types::types::canonical_generic_signature;

/// Whether control can leave the owned iterator before exhaustion. A `break`
/// belongs to the nearest loop, while return/raise escape through every nested
/// loop. This is used only when residual elements cannot be deleted implicitly.
pub(super) fn block_can_escape_owned_iteration(statements: &[Stmt], nested_loops: usize) -> bool {
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

/// The required kind/type of a trait `comptime NAME: Annotation` member.
#[derive(Clone, PartialEq)]
pub(super) enum CtMemberReq {
    /// A compile-time value whose value type must match this type.
    Value(Box<Ty>),
    /// A compile-time type value whose type must conform to these trait bounds.
    /// `params` is non-empty for a parameterized associated type such as
    /// `comptime IteratorType[iterable_mut: Bool, //, iterable_origin:
    /// Origin[mut=iterable_mut]]: Iterator`; the raw `TypeParam`s are retained
    /// (rather than classified `ParamDecl`s) because `classify_params` erases
    /// origin parameters, which the application arity check needs.
    Type {
        bounds: Vec<String>,
        params: Vec<mojito_ast::ast::TypeParam>,
    },
}

pub(super) enum OverloadSelect {
    NoMatch,
    Ambiguous,
}

pub(super) fn ct_integer(value: &CtValue) -> Option<mojito_common::literal::IntLiteral> {
    match value {
        CtValue::Int(value) => Some((*value).into()),
        CtValue::UInt(value) => Some((*value).into()),
        CtValue::IntLiteral(value) => Some(value.clone()),
        _ => None,
    }
}

/// A deliberately small implication relation for declaration availability.
/// It proves only facts which are syntactically present in a positive
/// conjunction (plus exact predicates).  In particular it does not turn a
/// failed symbolic evaluation, a negation, or either arm of a disjunction into
/// an assumption.
pub(super) fn generic_constraint_implies(
    premise: &GenericConstraint,
    consequence: &GenericConstraint,
) -> bool {
    if let GenericConstraint::WithMessage(condition, _) = premise {
        return generic_constraint_implies(condition, consequence);
    }
    if let GenericConstraint::WithMessage(condition, _) = consequence {
        return generic_constraint_implies(premise, condition);
    }
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

/// Fold a clause list into one right-nested conjunction for truth-only
/// operations such as implication. Storage keeps the un-folded list so each
/// clause retains its own diagnostic message.
pub(super) fn fold_constraint_conjunction(constraints: &[GenericConstraint]) -> GenericConstraint {
    let mut folded: Option<GenericConstraint> = None;
    for constraint in constraints.iter().rev() {
        folded = Some(match folded {
            None => constraint.clone(),
            Some(rest) => GenericConstraint::And(Box::new(constraint.clone()), Box::new(rest)),
        });
    }
    folded.unwrap_or(GenericConstraint::Bool(true))
}

pub(super) fn canonical_generic_parameter_shape(
    decls: &[ParamDecl],
    params: &[Ty],
    variadic: Option<&Ty>,
    kw_variadic: Option<&Ty>,
) -> (Vec<ParamDecl>, Vec<Ty>, Option<Ty>, Option<Ty>) {
    let mut signature = params.to_vec();
    let variadic_index = variadic.map(|parameter| {
        let index = signature.len();
        signature.push(parameter.clone());
        index
    });
    let kw_variadic_index = kw_variadic.map(|parameter| {
        let index = signature.len();
        signature.push(parameter.clone());
        index
    });
    let (decls, signature) = canonical_generic_signature(decls, &signature);
    (
        decls,
        signature[..params.len()].to_vec(),
        variadic_index.map(|index| signature[index].clone()),
        kw_variadic_index.map(|index| signature[index].clone()),
    )
}

pub(super) fn callable_parameter_count(ty: &Ty) -> Option<usize> {
    match ty {
        Ty::Func { params, .. } => Some(params.len()),
        Ty::GenericFunc { params, .. } => Some(params.len()),
        _ => None,
    }
}

pub(super) fn method_arity_range(sig: &MethodSig) -> (usize, usize) {
    (sig.params.len(), sig.params.len())
}

pub(super) fn guaranteed_conformance_atoms(
    constraint: &GenericConstraint,
    output: &mut Vec<(String, String)>,
) {
    match constraint {
        GenericConstraint::WithMessage(condition, _) => {
            guaranteed_conformance_atoms(condition, output);
        }
        GenericConstraint::Conforms { param, trait_name } => {
            let atom = (param.clone(), trait_name.clone());
            if !output.contains(&atom) {
                output.push(atom);
            }
        }
        // `where IsTrivially*[T]` guarantees the facet (and, through the
        // assumption table's implications, the base capability) inside the
        // body, recorded under the predicate's spelling.
        GenericConstraint::Trivial(kind, mojito_types::types::ConstraintOperand::Param(param)) => {
            let atom = (
                param.clone(),
                mojito_types::types::trivial_predicate_spelling(*kind).to_string(),
            );
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

/// Overload identity works at the mangled-symbol level: the compile-time
/// `StringLiteral` and the nominal stdlib `String` deliberately share the
/// stable `String` symbol spelling, so two overloads differing only in that
/// pair collide and are rejected as redeclarations.
pub(super) fn symbol_identity_ty(ty: &Ty) -> std::borrow::Cow<'_, Ty> {
    match ty {
        Ty::Struct(name, args)
            if args.is_empty() && mojito_symbol::symbol::is_stdlib_string_struct(name) =>
        {
            std::borrow::Cow::Owned(Ty::StringLiteral)
        }
        other => std::borrow::Cow::Borrowed(other),
    }
}

pub(super) fn symbol_equivalent_params(a: &[Ty], b: &[Ty]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(a, b)| symbol_identity_ty(a) == symbol_identity_ty(b))
}

pub(super) fn same_callable_signature(a: &Ty, b: &Ty) -> bool {
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
        ) => symbol_equivalent_params(ap, bp) && av == bv && akw == bkw,
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
            canonical_generic_parameter_shape(ad, ap, av.as_deref(), akw.as_deref())
                == canonical_generic_parameter_shape(bd, bp, bv.as_deref(), bkw.as_deref())
        }
        _ => false,
    }
}

/// Bundled collection mutators store an argument into `self` through
/// pointer intrinsics the body-level store rule cannot see; their transfer
/// effects are seeded here (the declared-metadata pattern used for interior
/// projections) instead of inferred from bodies.
pub(super) fn seeded_transfer_effects()
-> HashMap<String, Vec<mojito_checked::checked::TransferEffect>> {
    use mojito_types::origin::SigOrigin;
    let effect = |src: usize| {
        vec![mojito_checked::checked::TransferEffect {
            dest: SigOrigin::Self_,
            src: SigOrigin::Param(src),
            // `var` element parameters own a moved value: only carried
            // loans transfer, never the parameter slot's own storage.
            src_is_place: false,
            mutable: true,
        }]
    };
    HashMap::from([
        ("List.append".to_string(), effect(0)),
        ("List.insert".to_string(), effect(1)),
        ("List.__setitem__".to_string(), effect(1)),
    ])
}

pub(super) fn ct_values_equal(left: &CtValue, right: &CtValue) -> bool {
    match (ct_integer(left), ct_integer(right)) {
        (Some(left), Some(right)) => left == right,
        _ => left == right,
    }
}

pub(super) fn place_root_name(expr: &Expr) -> Option<&str> {
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

pub(super) fn place_has_index(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::Index { .. } | ExprKind::Slice { .. } | ExprKind::MultiIndex { .. } => true,
        ExprKind::Member { object, .. } => place_has_index(object),
        _ => false,
    }
}

/// The lowered symbol the checker records as the resolved callee of an
/// overloaded free-function call — formatted by the canonical symbol module so
/// it names exactly the `MirFunction` the MIR emits for that definition.
pub(super) fn callable_lowered_name(name: &str, ty: &Ty) -> Option<String> {
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
        .collect();
    let signature = mojito_symbol::symbol::SignatureKey::from_tys(signature_types)
        .with_kw_variadic(kw_variadic.as_deref());
    Some(mojito_symbol::symbol::function_symbol(name, &signature))
}

/// The lowered symbol of an overloaded method/constructor resolution, likewise
/// canonical. `sig.params` are the declared parameter types with `Self`
/// substituted to the enclosing struct at declaration time; `self_ty` (the
/// receiver struct's instance type) canonicalizes those occurrences back to
/// `Self`, so the key matches the MIR definition side, which mangles the bare
/// `Self` annotation. Pass `None` for abstract trait dispatch, whose `sig`
/// parameters retain `Ty::SelfType` and already spell `Self`.
pub(super) fn method_lowered_name(
    type_name: &str,
    method: &str,
    sig: &MethodSig,
    self_ty: Option<&Ty>,
) -> String {
    // The variadic element participates in overload identity at its declared
    // position (the MIR symbol walks parameters in source order).
    let mut signature_types: Vec<&Ty> = sig.params.iter().collect();
    if let Some(element) = sig.variadic.as_deref() {
        let index = sig
            .variadic_index
            .unwrap_or(signature_types.len())
            .min(signature_types.len());
        signature_types.insert(index, element);
    }
    let signature_types = signature_types.into_iter();
    let keyword_names = match sig.keyword_only {
        Some(index) => sig.names[index..].to_vec(),
        None => Vec::new(),
    };
    let signature =
        mojito_symbol::symbol::SignatureKey::from_tys_with_self(signature_types, self_ty)
            .with_kw_variadic(sig.kw_variadic.as_deref())
            .with_keyword_names(keyword_names);
    if mojito_symbol::symbol::receiver_overloaded_method(method) {
        mojito_symbol::symbol::receiver_method_symbol(
            type_name,
            method,
            sig.self_convention,
            &signature,
        )
    } else {
        mojito_symbol::symbol::method_symbol(type_name, method, &signature)
    }
}

/// The raw-slot operations on `UnsafePointer` are an implementation privilege,
/// not source-language API. Linked expressions retain their exact source path;
/// only files physically shipped in the compiler's collection library receive
/// the checked adjustment that can lower these operations.
pub(super) fn is_bundled_collection_source(source: Option<&str>) -> bool {
    let (Some(root), Some(source)) = (mojito_module::module::bundled_root(), source) else {
        return false;
    };
    let stdlib = root.join("stdlib");
    let source = Path::new(stamped_source_module(source));
    source == stdlib.join("std/collections/list.mojo")
        || source == stdlib.join("list.mojo")
        || source == stdlib.join("std/collections/dict.mojo")
        || source == stdlib.join("dict.mojo")
        || source == stdlib.join("std/collections/array.mojo")
        || source == stdlib.join("std/optional.mojo")
        || source == stdlib.join("optional.mojo")
        || source == stdlib.join("std/memory.mojo")
}

/// The source a struct declaration belongs to for the bundled-crossing gate:
/// its module, or — for a specialized variadic struct, whose top-level
/// module is cleared so its per-accessor source tags survive — the stamped
/// source of its first method body statement.
pub(super) fn bundled_struct_source<'a>(
    module: Option<&'a str>,
    methods: &'a [mojito_ast::ast::Method],
) -> Option<&'a str> {
    module.or_else(|| {
        methods.iter().find_map(|method| {
            method
                .body
                .first()
                .and_then(|statement| statement.module.as_deref())
        })
    })
}

/// Static `UnsafePointer[T].alloc[_aligned]` is the compiler's heap primitive,
/// retired from source-language API by the current layout-based model (the
/// audited head rejects it). `std/memory.mojo` is the bundled allocation
/// crossing; every other module — stdlib included — allocates through
/// `std.memory`. `std/utils/variant.mojo` is the second crossing: the
/// self-hosted `Variant` names the compiler-private `__VariantStorage`.
/// A specialization's source tag still belongs to its crossing module.
pub(super) fn is_bundled_stdlib_source(source: Option<&str>) -> bool {
    let (Some(root), Some(source)) = (mojito_module::module::bundled_root(), source) else {
        return false;
    };
    let stdlib = root.join("stdlib");
    let source = Path::new(stamped_source_module(source));
    source == stdlib.join("std/memory.mojo") || source == stdlib.join("std/utils/variant.mojo")
}

/// Whether a source belongs to a bundled standard-library module, directly
/// or through a specialization tag layered on its path. Generic-struct
/// instances reached only from there keep the erased path (always correct):
/// a program without its own instantiations mints no clones, and the
/// instances a user-reachable clone reaches through its storage and
/// signatures are minted by the specializer itself.
pub fn is_bundled_module_source(source: Option<&str>) -> bool {
    let (Some(root), Some(source)) = (mojito_module::module::bundled_root(), source) else {
        return false;
    };
    Path::new(stamped_source_module(source)).starts_with(root.join("stdlib"))
}

/// The module path beneath a specialization source tag: a specialized
/// variadic struct, its unrolled accessors, and a per-instantiation method
/// clone are stamped `<module>$<instance>…`, and keep the module's
/// privileges.
fn stamped_source_module(source: &str) -> &str {
    source.split('$').next().unwrap_or(source)
}

/// Select the current Mojo parameter-index hook first, while retaining the
/// earlier spelling as an intentional source-compatibility fallback.
pub(super) fn dependent_index_accessor_family(
    info: &StructInfo,
) -> Option<DependentIndexAccessorFamily> {
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

/// View a statement as a struct declaration, for the order-independent
/// declaration pre-passes and the source-order walk alike.
pub(super) fn struct_declaration(stmt: &Stmt) -> Option<StructDeclaration<'_>> {
    let StmtKind::Struct {
        name,
        type_params,
        conforms,
        callable_conformance,
        conformance_conditions,
        where_clauses,
        fields,
        associated,
        methods,
        fieldwise_init,
        decorators,
    } = &stmt.kind
    else {
        return None;
    };
    Some(StructDeclaration {
        module: &stmt.module,
        name,
        type_params,
        conforms,
        callable_conformance,
        conformance_conditions,
        where_clauses,
        fields,
        associated,
        methods,
        fieldwise_init: *fieldwise_init,
        decorators,
    })
}

/// Merge a callable's committed transfer effects into a function type taken
/// as a value, so an indirect call replays them from the type itself. Union,
/// never replacement: a rebake after the entry grew keeps earlier effects.
pub(super) fn with_transfer_effects(
    mut callable: Ty,
    effects: &[mojito_checked::checked::TransferEffect],
) -> Ty {
    match &mut callable {
        Ty::Func { transfers, .. } | Ty::GenericFunc { transfers, .. } => {
            for effect in effects {
                if !transfers.0.contains(effect) {
                    transfers.0.push(effect.clone());
                }
            }
        }
        _ => {}
    }
    callable
}

/// The transfer effects carried by a callable value's checked type (empty
/// for non-callables and for contracts that never had effects baked).
pub(super) fn contract_transfer_effects(ty: &Ty) -> &[mojito_checked::checked::TransferEffect] {
    match callable_contract_ty(ty) {
        Some(Ty::Func { transfers, .. }) | Some(Ty::GenericFunc { transfers, .. }) => &transfers.0,
        _ => &[],
    }
}

pub(super) fn with_callable_environment(
    mut callable: Ty,
    environment: mojito_types::origin::CallableEnvironment,
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

pub(super) fn overload_rank(
    conversions: usize,
    variadic: bool,
    signature_len: usize,
    generic: bool,
) -> usize {
    conversions * CONVERSION_RANK
        + usize::from(variadic) * VARIADIC_RANK
        + signature_len * SIGNATURE_LENGTH_RANK
        + usize::from(generic)
}

pub(super) fn conversion_count(actual: &Ty, expected: &Ty) -> usize {
    if actual == expected
        || matches!(actual, Ty::IntLiteral) && matches!(expected, Ty::Int)
        || matches!(actual, Ty::FloatLiteral) && matches!(expected, Ty::Float64)
    {
        0
    } else {
        1
    }
}

pub(super) fn select_callable_overload(
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

pub(super) fn select_method_overload(
    _method: &str,
    matches: Vec<MethodCallResolution>,
    receiver_transferred: Option<bool>,
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
        return Ok(best_matches.remove(0));
    }
    // Current Mojo overloads some methods purely on the receiver convention
    // (a consuming `x^.m()` beside a borrowing `x.m()` — see
    // `symbol::receiver_overloaded_method`). Argument scoring cannot separate
    // those, so an otherwise tied set falls back to the call's explicit
    // receiver transfer. Only an exact single survivor resolves; anything
    // else stays ambiguous. The implicitly-copyable place satisfying a sole
    // deinit overload is unaffected — that path never reaches a tie.
    if let Some(transferred) = receiver_transferred {
        let survivors: Vec<usize> = best_matches
            .iter()
            .enumerate()
            .filter(|(_, candidate)| candidate.consumes_receiver == transferred)
            .map(|(index, _)| index)
            .collect();
        if let [index] = survivors.as_slice() {
            return Ok(best_matches.remove(*index));
        }
    }
    Err(OverloadSelect::Ambiguous)
}

pub(super) fn overload_candidates(existing: &Ty, new_ty: &Ty) -> Option<Vec<Ty>> {
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
