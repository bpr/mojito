//! Synthesized conformance methods: `Copyable.copy` and
//! `Hashable.__hash__` bodies, plus upstream's `SIMD[_, _]` parameter
//! desugar, the vector-alias bound fold, and the value reading of a
//! `Self.`-spelled `materialize` operand.

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
        let module = statement.module.clone();
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
        // The body keeps its struct's module, as a linked source body does,
        // so a specialization traces its copy to this template.
        let mut body = vec![mk(StmtKind::Return(Some(result)), span)];
        if let Some(module) = &module {
            mojito_ast::ast::stamp_source(&mut body, module);
        }
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
            body,
            provenance: mojito_ast::ast::MethodProvenance::SynthesizedDefault,
        });
    }
}

/// Materialize Hashable's reflective field default as ordinary source AST.
/// An explicit hasher-fed `__hash__(self, mut hasher)` wins; any other
/// `__hash__` is an ordinary overload beside the default, as in current Mojo.
/// Conditional conformances carry the same availability predicate onto the
/// synthesized method.
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
            || methods.iter().any(|method| {
                method.name == "__hash__"
                    && matches!(
                        method.params.as_slice(),
                        [parameter] if parameter.convention == Some(mojito_ast::ast::ArgConvention::Mut)
                    )
            })
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
            body,
            provenance: mojito_ast::ast::MethodProvenance::SynthesizedDefault,
        });
    }
}

pub(super) use mojito_ast::simd_width::method_constructs_at_own_lane as constructs_at_own_lane;

/// Desugar upstream's `value: SIMD[_, _]` parameter spelling on a `def`, a
/// struct method, or a trait requirement into the parameters it stands for: an infer-only `DType`
/// binder and an infer-only width binder of SIMD's own width type, over
/// `SIMD[dtype, length]`, as upstream's automatic parameterization does. The
/// argument's vector type solves both at each call, so the template serves
/// every call and the elaborator instantiates it per vector type. `$` keeps
/// the binders unspellable in source.
pub(super) fn desugar_simd_wildcard_parameters(program: &mut [Stmt]) {
    for statement in program {
        match &mut statement.kind {
            StmtKind::Def {
                type_params,
                params,
                ..
            } => desugar_wildcard_parameters(type_params, params),
            StmtKind::Struct { methods, .. } => {
                for method in methods.iter_mut() {
                    desugar_wildcard_parameters(&mut method.type_params, &mut method.params);
                }
            }
            StmtKind::Trait { methods, .. } => {
                for method in methods.iter_mut() {
                    desugar_wildcard_parameters(&mut method.type_params, &mut method.params);
                }
            }
            _ => {}
        }
    }
}

/// `materialize`'s one parameter is a value, so a `Self.n` or
/// `Self.values[i]` operand the parser read as a type spelling is the value
/// expression it spells: the member read, or the subscript of one.
pub(super) fn read_materialize_self_operands(program: &mut [Stmt]) {
    struct Operands;
    impl mojito_ast::visit::MutVisitor for Operands {
        fn visit_expr_mut(&mut self, expr: &mut Expr) {
            if let ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } = &mut expr.kind
                && name == "materialize"
                && args.is_empty()
                && kwargs.is_empty()
                && let [ParamArg::Type(operand)] = param_args.as_slice()
                && let Some(operand) = self_value_operand(operand, expr.span)
            {
                param_args[0] = ParamArg::Value(operand);
            }
        }
    }
    mojito_ast::visit::walk_block_mut(&mut Operands, program);
}

fn desugar_wildcard_parameters(
    type_params: &mut Vec<TypeParam>,
    params: &mut [mojito_ast::ast::FnParam],
) {
    let mut binders = Vec::new();
    for parameter in params
        .iter_mut()
        .filter(|parameter| is_simd_wildcard_type(&parameter.ty))
    {
        let dtype = format!("$dtype_{}", parameter.name);
        let length = format!("$length_{}", parameter.name);
        // Each `_` becomes a reference to its binder, keeping its span.
        if let Type::Named(_, arguments) = &mut parameter.ty {
            for (argument, name) in arguments.iter_mut().zip([&dtype, &length]) {
                if let ParamArg::Value(Expr {
                    kind: ExprKind::Identifier(hole),
                    ..
                }) = argument
                {
                    hole.clone_from(name);
                }
            }
        }
        for (name, bound) in [(dtype, "DType"), (length, "SIMDLength")] {
            binders.push(TypeParam {
                name,
                bounds: vec![bound.to_string()],
                value_type: None,
                callable_bound: None,
                origin_mutability: None,
                infer_only: true,
                default: None,
                constraints: Vec::new(),
            });
        }
    }
    type_params.splice(0..0, binders);
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

/// The value expression a `Self.`-rooted type spelling denotes
/// ([`read_materialize_self_operands`]).
fn self_value_operand(operand: &Type, span: Span) -> Option<Expr> {
    match operand {
        Type::SelfParam(name) if !name.starts_with('*') => Some(Expr::new(
            ExprKind::Member {
                object: Box::new(Expr::new(ExprKind::Identifier("Self".to_string()), span)),
                field: name.clone(),
            },
            span,
        )),
        Type::IndexedProjection { base, index } => Some(Expr::new(
            ExprKind::Index {
                object: Box::new(self_value_operand(base, span)?),
                index: index.clone(),
            },
            span,
        )),
        _ => None,
    }
}
