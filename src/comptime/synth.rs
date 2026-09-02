//! Synthesized conformance methods: `Copyable.copy` and
//! `Hashable.__hash__` bodies.

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
            .find(|m| crate::symbol::lifecycle_method_name(m) == "__copyinit__");
        let result = if copy_constructor.is_some() {
            Expr::new(
                ExprKind::Call {
                    name: name.clone(),
                    param_args: Vec::new(),
                    args: Vec::new(),
                    kwargs: vec![crate::ast::KwArg {
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
        let (raises, raises_type) = copy_constructor
            .map(|constructor| (constructor.raises, constructor.raises_type.clone()))
            .unwrap_or((false, None));
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
        methods.push(crate::ast::Method {
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
        methods.push(crate::ast::Method {
            name: "__hash__".to_string(),
            type_params: Vec::new(),
            has_self: true,
            self_convention: None,
            self_origin: None,
            decorators: Vec::new(),
            params: vec![crate::ast::FnParam {
                name: "hasher".to_string(),
                ty: Type::Named(
                    "Some".to_string(),
                    vec![ParamArg::Type(Type::Named(
                        "Hasher".to_string(),
                        Vec::new(),
                    ))],
                ),
                default: None,
                kind: crate::ast::ParamKind::Regular,
                convention: Some(crate::ast::ArgConvention::Mut),
                origin: None,
            }],
            positional_only: None,
            keyword_only: None,
            raises: false,
            raises_type: None,
            ret: None,
            where_clauses,
            body,
        });
    }
}
