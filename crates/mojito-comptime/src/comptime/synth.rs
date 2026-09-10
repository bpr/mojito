//! Synthesized conformance methods: `Copyable.copy` and
//! `Hashable.__hash__` bodies, plus the `Hasher` protocol's wildcard vector
//! parameter desugar and its eager per-leaf clone requests.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

/// Materialize the `Copyable` trait's default `copy` method (current Mojo:
/// `def copy(self) -> Self: return Self(copy=self)`; overriding it is not
/// allowed). Mojito models the built-in traits structurally, so every struct
/// declaring `Copyable` — directly or through `ImplicitlyCopyable` — gains the
/// method here as ordinary source AST, keeping the checker, HIR, MIR, and VM on
/// their existing method and constructor paths. A struct with an explicit copy
/// constructor delegates to it (propagating its raising effect); a fieldwise
/// Copyable struct uses the same explicit copy-construction spelling, which
/// lowers to the synthesized fieldwise copy lifecycle. A conditional conformance carries its predicate over as the
/// method's `where` clause. Structs that already spell `copy` keep their own
/// (the self-hosted collections predate this synthesis).
pub(super) fn synthesize_copyable_copy(program: &mut [Stmt]) {
    for statement in program {
        let span = statement.span;
        let StmtKind::Struct {
            name,
            conforms,
            conformance_conditions,
            methods,
            ..
        } = &mut statement.kind
        else {
            continue;
        };
        let copyable = |conformance: &String| {
            matches!(conformance.as_str(), "Copyable" | "ImplicitlyCopyable")
        };
        if !conforms.iter().any(copyable) || methods.iter().any(|m| m.name == "copy") {
            continue;
        }
        let copy_constructor = methods
            .iter()
            .find(|m| mojito_symbol::symbol::lifecycle_method_name(m) == "__copyinit__");
        let result = if copy_constructor.is_some() {
            Expr::new(
                ExprKind::Call {
                    name: name.clone(),
                    param_args: Vec::new(),
                    args: Vec::new(),
                    kwargs: vec![mojito_ast::ast::KwArg {
                        name: "copy".to_string(),
                        value: Expr::new(ExprKind::Identifier("self".to_string()), span),
                    }],
                },
                span,
            )
        } else {
            Expr::new(
                ExprKind::Call {
                    name: "__mojito_fieldwise_copy".to_string(),
                    param_args: Vec::new(),
                    args: vec![Expr::new(ExprKind::Identifier("self".to_string()), span)],
                    kwargs: Vec::new(),
                },
                span,
            )
        };
        let (raises, raises_type) = copy_constructor.map_or((false, None), |constructor| {
            (constructor.raises, constructor.raises_type.clone())
        });
        let where_clauses = conformance_conditions
            .iter()
            .find(|(trait_name, _)| trait_name == "Copyable")
            .or_else(|| {
                conformance_conditions
                    .iter()
                    .find(|(trait_name, _)| trait_name == "ImplicitlyCopyable")
            })
            .map(|(_, condition)| vec![condition.clone()])
            .unwrap_or_default();
        methods.push(mojito_ast::ast::Method {
            name: "copy".to_string(),
            type_params: Vec::new(),
            has_self: true,
            self_convention: None,
            self_origin: None,
            decorators: Vec::new(),
            params: Vec::new(),
            positional_only: None,
            keyword_only: None,
            raises,
            raises_type,
            ret: Some(Type::SelfType),
            where_clauses,
            self_ty: None,
            body: vec![mk(StmtKind::Return(Some(result)), span)],
        });
    }
}

/// Materialize Hashable's reflective field default as ordinary source AST.
/// Explicit implementations win; conditional conformances carry the same
/// availability predicate onto the synthesized method.
pub(super) fn synthesize_hashable_hash(program: &mut [Stmt]) {
    for statement in program {
        let span = statement.span;
        let StmtKind::Struct {
            conforms,
            conformance_conditions,
            fields,
            methods,
            ..
        } = &mut statement.kind
        else {
            continue;
        };
        if !conforms.iter().any(|conformance| conformance == "Hashable")
            || methods.iter().any(|method| method.name == "__hash__")
        {
            continue;
        }
        let hasher = Expr::new(ExprKind::Identifier("hasher".to_string()), span);
        let body = if fields.is_empty() {
            vec![mk(StmtKind::Pass, span)]
        } else {
            fields
                .iter()
                .map(|field| {
                    let value = Expr::new(
                        ExprKind::Member {
                            object: Box::new(Expr::new(
                                ExprKind::Identifier("self".to_string()),
                                span,
                            )),
                            field: field.name.clone(),
                        },
                        span,
                    );
                    mk(
                        StmtKind::Expr(Expr::new(
                            ExprKind::MethodCall {
                                object: Box::new(hasher.clone()),
                                method: "update".to_string(),
                                args: vec![value],
                                kwargs: Vec::new(),
                            },
                            span,
                        )),
                        span,
                    )
                })
                .collect()
        };
        let where_clauses = conformance_conditions
            .iter()
            .find(|(trait_name, _)| trait_name == "Hashable")
            .map(|(_, condition)| vec![condition.clone()])
            .unwrap_or_default();
        methods.push(mojito_ast::ast::Method {
            name: "__hash__".to_string(),
            type_params: Vec::new(),
            has_self: true,
            self_convention: None,
            self_origin: None,
            decorators: Vec::new(),
            params: vec![mojito_ast::ast::FnParam {
                name: "hasher".to_string(),
                ty: Type::Named(
                    "Some".to_string(),
                    vec![ParamArg::Type(Type::Named(
                        "Hasher".to_string(),
                        Vec::new(),
                    ))],
                ),
                default: None,
                kind: mojito_ast::ast::ParamKind::Regular,
                convention: Some(mojito_ast::ast::ArgConvention::Mut),
                origin: None,
            }],
            positional_only: None,
            keyword_only: None,
            raises: false,
            raises_type: None,
            ret: None,
            where_clauses,
            self_ty: None,
            body,
        });
    }
}

/// The hidden vector parameter `_update_with_simd(mut self, value: SIMD[_, _])`
/// desugars to — an infer-only type parameter bounded by the compiler's
/// `$SIMD` (any SIMD-valued type). `$` keeps both names unspellable in source.
pub(super) const SIMD_WILDCARD_PARAM: &str = "$simd";
pub(super) const SIMD_WILDCARD_BOUND: &str = "$SIMD";

/// Whether a struct method carries the desugared wildcard vector parameter.
pub(super) fn is_simd_keyed_method(method: &mojito_ast::ast::Method) -> bool {
    method
        .type_params
        .iter()
        .any(|parameter| parameter.name == SIMD_WILDCARD_PARAM)
}

/// Desugar upstream's `value: SIMD[_, _]` parameter spelling on a struct
/// method into an inferred type parameter: the argument's own vector type
/// keys a per-call clone (`_update_with_simd$y3:Int`), the only shape under
/// which the body's `to_bits`/`.length` spellings check concretely. The
/// template body never checks — elaboration installs a trap stub in its
/// place. A method with more than one wildcard parameter is left alone (the
/// checker rejects the spelling).
pub(super) fn desugar_simd_keyed_methods(program: &mut [Stmt]) {
    for statement in program {
        let StmtKind::Struct { methods, .. } = &mut statement.kind else {
            continue;
        };
        for method in methods.iter_mut() {
            if is_simd_keyed_method(method) {
                continue;
            }
            let wildcards: Vec<usize> = method
                .params
                .iter()
                .enumerate()
                .filter(|(_, parameter)| is_simd_wildcard_type(&parameter.ty))
                .map(|(index, _)| index)
                .collect();
            let [index] = wildcards.as_slice() else {
                continue;
            };
            method.params[*index].ty = Type::Named(SIMD_WILDCARD_PARAM.to_string(), Vec::new());
            method.type_params.insert(
                0,
                TypeParam {
                    name: SIMD_WILDCARD_PARAM.to_string(),
                    bounds: vec![SIMD_WILDCARD_BOUND.to_string()],
                    value_type: None,
                    callable_bound: None,
                    origin_mutability: None,
                    infer_only: true,
                    default: None,
                    constraints: Vec::new(),
                },
            );
        }
    }
}

/// The width-1 vector types every hasher's `_update_with_simd` is cloned for
/// eagerly: the native scalars plus one lane of every dtype. Hashing reaches
/// the hasher through erased paths (`hash[T]`, `update(Some[Hashable])`) that
/// record no call-site instantiation, and the VM-CTFE subprogram has no
/// discovery loop at all, so the closed scalar set is minted up front; wider
/// vectors arrive through the checker's `hash_leaf_types` demand channel.
pub(super) fn eager_hash_leaf_types() -> Vec<Ty> {
    use mojito_ast::ast::Dtype;
    let mut leaves = vec![Ty::Int, Ty::UInt, Ty::Float64];
    for dtype in [
        Dtype::Int,
        Dtype::Int8,
        Dtype::Int16,
        Dtype::Int32,
        Dtype::Int64,
        Dtype::UInt8,
        Dtype::UInt16,
        Dtype::UInt32,
        Dtype::UInt64,
        Dtype::Float32,
        Dtype::Float64,
        Dtype::Bool,
    ] {
        let leaf = mojito_types::types::canonical_simd_ty(dtype, 1);
        if !leaves.contains(&leaf) {
            leaves.push(leaf);
        }
    }
    leaves
}

/// The per-call clone requests for a `Hasher` conformer's SIMD-keyed
/// `_update_with_simd`: one per eager leaf type plus the program's demanded
/// wider vectors; empty for any other statement.
pub(super) fn hasher_leaf_requests(
    statement: &Stmt,
    extra_leaves: &[Ty],
) -> Vec<MethodSpecializationRequest> {
    let StmtKind::Struct {
        name,
        conforms,
        methods,
        ..
    } = &statement.kind
    else {
        return Vec::new();
    };
    if !conforms.iter().any(|conformance| conformance == "Hasher") {
        return Vec::new();
    }
    let Some(method) = methods
        .iter()
        .find(|method| method.name == "_update_with_simd" && is_simd_keyed_method(method))
    else {
        return Vec::new();
    };
    let parameter_names: Vec<String> = method
        .params
        .iter()
        .filter(|parameter| parameter.kind == ParamKind::Regular)
        .map(|parameter| parameter.name.clone())
        .collect();
    eager_hash_leaf_types()
        .into_iter()
        .chain(extra_leaves.iter().cloned())
        .map(|leaf| {
            MethodSpecializationRequest::new(
                SourceSpan::new(None, mojito_common::token::DUMMY_SPAN),
                name.clone(),
                "_update_with_simd".to_string(),
                parameter_names.clone(),
                vec![TyArg::Ty(leaf)],
            )
        })
        .collect()
}

fn is_simd_wildcard_type(ty: &Type) -> bool {
    matches!(ty, Type::Named(name, args)
    if name == "SIMD"
        && args.len() == 2
        && args.iter().all(|argument| {
            matches!(argument, ParamArg::Value(Expr { kind: ExprKind::Identifier(id), .. }) if id == "_")
        }))
}

/// Fold a module-scope vector alias (`comptime U256 = SIMD[DType.uint64, 4]`)
/// used as a struct parameter's bound (`[key: U256]`) into the spelling the
/// parser gives `[key: SIMD[DType.uint64, 4]]` — a value parameter of the
/// vector type — before the specialization registry classifies it. Uses in
/// annotations and constructions fold through the ordinary alias machinery.
pub(super) fn fold_simd_alias_bounds(program: &mut [Stmt]) {
    let aliases: HashMap<String, Vec<ParamArg>> = program
        .iter()
        .filter_map(|statement| match &statement.kind {
            StmtKind::Comptime {
                name,
                type_params,
                value,
                ..
            } if type_params.is_empty() => match &value.kind {
                ExprKind::TypeApply {
                    name: applied,
                    args,
                } if applied == "SIMD" && simd_source_dims(args).is_some() => {
                    Some((name.clone(), args.clone()))
                }
                _ => None,
            },
            _ => None,
        })
        .collect();
    if aliases.is_empty() {
        return;
    }
    for statement in program {
        let StmtKind::Struct { type_params, .. } = &mut statement.kind else {
            continue;
        };
        for parameter in type_params {
            if parameter.value_type.is_none()
                && let [only] = parameter.bounds.as_slice()
                && let Some(args) = aliases.get(only)
            {
                parameter.value_type = Some(Type::Named("SIMD".to_string(), args.clone()));
                parameter.bounds = vec!["SIMD".to_string()];
            }
        }
    }
}

/// Replace every SIMD-keyed method body of a struct with the trap stub: the
/// template never checks with its vector type unbound.
pub(super) fn stub_simd_keyed_methods(owner: &str, methods: &mut [mojito_ast::ast::Method]) {
    for method in methods {
        if is_simd_keyed_method(method) {
            method.body = vec![super::specialize::unspecialized_method_stub(owner, method)];
        }
    }
}
