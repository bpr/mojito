//! Synthesized conformance methods: `Copyable.copy`, `Hashable.__hash__`,
//! `Equatable.__eq__`, and `Writable.write_to`/`write_repr_to` bodies, plus upstream's `SIMD[_, _]` parameter
//! desugar, the vector-alias bound fold, the KGEN-typed binder fold, and
//! the value reading of a `Self.`-spelled `materialize` operand.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::{TYPE_OF_ALL_TYPES, TyKind};

/// Materialize the `Copyable` trait's default `copy` method (current Mojo:
/// `def copy(self) -> Self: return Self(copy=self)`; overriding it is not
/// allowed). Mojito models the built-in traits structurally, so every struct
/// declaring `Copyable` — directly or through a trait refining it
/// (`ImplicitlyCopyable`, `TrivialRegisterPassable`), whose parents the
/// program's own trait declarations or the `installed` package's answer —
/// gains the method here as ordinary source AST, keeping the checker, HIR, MIR, and VM on
/// their existing method and constructor paths. A struct with an explicit copy
/// constructor delegates to it (propagating its raising effect); a fieldwise
/// Copyable struct uses the same explicit copy-construction spelling, which
/// lowers to the synthesized fieldwise copy lifecycle. A conditional conformance carries its predicate over as the
/// method's `where` clause. Structs that already spell `copy` keep their own
/// (the self-hosted collections predate this synthesis).
pub(super) fn synthesize_copyable_copy(
    program: &mut [Stmt],
    installed: Option<&InstalledInterfaces>,
) {
    let declared = declared_trait_parents(program);
    let copyable =
        |conformance: &String| conformance_reaches(&declared, installed, conformance, "Copyable");
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
        // The member is as available as the struct's `Copyable` conformance:
        // unconditionally when it declares `Copyable` without a condition,
        // under a refining conformance's condition when only that declares
        // it.
        let declares_copyable = conforms.iter().any(|conformance| conformance == "Copyable");
        let where_clauses = conformance_conditions
            .iter()
            .find(|(trait_name, _)| trait_name == "Copyable")
            .or_else(|| {
                conformance_conditions
                    .iter()
                    .find(|(trait_name, _)| !declares_copyable && copyable(trait_name))
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

/// Give the members of an `enum` that upstream's `__Enum` supplies, and
/// its case constructors, the source of the module implementing enums,
/// `std.builtin._enum`, where upstream writes their bodies: they mark the
/// result initialized, write the storage, and return references into it,
/// which only that module's bodies may do. Elaboration stamps every statement with its own module; this
/// runs after.
pub(super) fn stamp_enum_storage_writes(program: &mut [Stmt]) {
    const ENUM_MEMBERS: [&str; 4] = [
        "__enum_init",
        "__get_enum_storage",
        "_get_enum_discriminant",
        "_unsafe_get_enum_payload",
    ];
    let Some(source) = mojito_checker::checker::enum_support_source() else {
        return;
    };
    for statement in program {
        if let StmtKind::Struct {
            methods,
            enum_cases: Some(cases),
            ..
        } = &mut statement.kind
        {
            for method in methods.iter_mut().filter(|method| {
                ENUM_MEMBERS.contains(&method.name.as_str())
                    || cases.iter().any(|case| case.name == method.name)
            }) {
                mojito_ast::ast::stamp_source(&mut method.body, &source);
            }
        }
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
                        mojito_checker::checker::hasher_trait(),
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

/// Materialize `Writable`'s reflective `write_to` and `write_repr_to`
/// defaults as ordinary source AST, for a struct conforming to `Writable`
/// directly or through a declared refinement without that member taking a
/// `Writer`. As upstream's default over `reflect[Self]` does, the body
/// writes `Name[params](field=value, ...)`: the type through
/// `_unqualified_type_name[Self]()`, each field through the bundled
/// `_reflection_write_field_to`, whose `conforms_to(FieldType, Writable)`
/// assert defers the field check to the default's instantiation. A
/// `write_to` taking any other writer is an ordinary overload beside the
/// default, as in current Mojo. An `enum`'s members are the parser's, and a
/// conditional conformance carries its predicate onto the synthesized
/// methods.
pub(super) fn synthesize_writable_defaults(
    program: &mut [Stmt],
    installed: Option<&InstalledInterfaces>,
) {
    let declared = declared_trait_parents(program);
    let writable =
        |conformance: &String| conformance_reaches(&declared, installed, conformance, "Writable");
    let field_writer = mojito_checker::checker::writable_field_writer();
    for statement in program {
        let span = statement.span;
        let module = statement.module.clone();
        let StmtKind::Struct {
            conforms,
            conformance_conditions,
            fields,
            methods,
            enum_cases: None,
            ..
        } = &mut statement.kind
        else {
            continue;
        };
        if !conforms.iter().any(writable) {
            continue;
        }
        let declares_writable = conforms.iter().any(|conformance| conformance == "Writable");
        let where_clauses = conformance_conditions
            .iter()
            .find(|(trait_name, _)| trait_name == "Writable")
            .or_else(|| {
                conformance_conditions
                    .iter()
                    .find(|(trait_name, _)| !declares_writable && writable(trait_name))
            })
            .map(|(_, condition)| vec![condition.clone()])
            .unwrap_or_default();
        for (member, is_repr) in [("write_to", false), ("write_repr_to", true)] {
            if methods
                .iter()
                .any(|method| method.name == member && takes_writer(method))
            {
                continue;
            }
            let mut body = writable_default_body(fields, &field_writer, is_repr, span);
            if let Some(module) = &module {
                mojito_ast::ast::stamp_source(&mut body, module);
            }
            methods.push(mojito_ast::ast::Method {
                name: member.to_string(),
                type_params: Vec::new(),
                has_self: true,
                self_convention: None,
                self_origin: None,
                decorators: Vec::new(),
                params: vec![mojito_ast::ast::FnParam {
                    name: "writer".to_string(),
                    ty: Type::Named(
                        "Some".to_string(),
                        vec![ParamArg::Type(Type::Named(
                            "Writer".to_string(),
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
                where_clauses: where_clauses.clone(),
                body,
                provenance: mojito_ast::ast::MethodProvenance::SynthesizedDefault,
            });
        }
    }
}

/// Materialize `Equatable`'s reflective `__eq__` default as ordinary source
/// AST, for a struct declaring `Equatable` or `Comparable` without an
/// `__eq__` taking `Self`: every field compares with `!=` in declaration
/// order, the first unequal pair answering `False`, as upstream's default
/// does over `reflect[Self]`. An `enum` derives none, as upstream rejects
/// deriving one. A conditional conformance carries its predicate onto the
/// synthesized method.
pub(super) fn synthesize_equatable_eq(program: &mut [Stmt]) {
    for statement in program {
        let span = statement.span;
        let StmtKind::Struct {
            name,
            conforms,
            conformance_conditions,
            fields,
            methods,
            enum_cases: None,
            ..
        } = &mut statement.kind
        else {
            continue;
        };
        let equatable = |trait_name: &str| matches!(trait_name, "Equatable" | "Comparable");
        let takes_self = |method: &mojito_ast::ast::Method| {
            matches!(
                method.params.as_slice(),
                [parameter] if matches!(&parameter.ty, Type::SelfType)
                    || matches!(&parameter.ty, Type::Named(named, _) if named == name)
            )
        };
        if !conforms.iter().any(|conformance| equatable(conformance))
            || methods
                .iter()
                .any(|method| method.name == "__eq__" && takes_self(method))
        {
            continue;
        }
        let identifier = |name: &str| Expr::new(ExprKind::Identifier(name.to_string()), span);
        let field_of = |object: &str, field: &str| {
            Expr::new(
                ExprKind::Member {
                    object: Box::new(identifier(object)),
                    field: field.to_string(),
                },
                span,
            )
        };
        let answer = |value: bool| {
            mk(
                StmtKind::Return(Some(Expr::new(ExprKind::Bool(value), span))),
                span,
            )
        };
        let mut body: Vec<Stmt> = fields
            .iter()
            .map(|field| {
                let differs = Expr::new(
                    ExprKind::Infix(
                        mojito_ast::ast::InfixOp::Ne,
                        Box::new(field_of("self", &field.name)),
                        Box::new(field_of("other", &field.name)),
                    ),
                    span,
                );
                mk(
                    StmtKind::If {
                        branches: vec![(
                            mojito_ast::ast::Condition::Expr(differs),
                            vec![answer(false)],
                        )],
                        orelse: None,
                    },
                    span,
                )
            })
            .collect();
        body.push(answer(true));
        if let Some(module) = &statement.module {
            mojito_ast::ast::stamp_source(&mut body, module);
        }
        // The member is as available as the struct's `Equatable`
        // conformance: unconditionally when it declares `Equatable` without
        // a condition, under `Comparable`'s when only that refines it.
        let declares_equatable = conforms
            .iter()
            .any(|conformance| conformance == "Equatable");
        let where_clauses = conformance_conditions
            .iter()
            .find(|(trait_name, _)| trait_name == "Equatable")
            .or_else(|| {
                conformance_conditions
                    .iter()
                    .find(|(trait_name, _)| !declares_equatable && trait_name == "Comparable")
            })
            .map(|(_, condition)| vec![condition.clone()])
            .unwrap_or_default();
        methods.push(mojito_ast::ast::Method {
            name: "__eq__".to_string(),
            type_params: Vec::new(),
            has_self: true,
            self_convention: None,
            self_origin: None,
            decorators: Vec::new(),
            params: vec![mojito_ast::ast::FnParam {
                name: "other".to_string(),
                ty: Type::SelfType,
                default: None,
                kind: mojito_ast::ast::ParamKind::Regular,
                convention: None,
                origin: None,
            }],
            positional_only: Some(1),
            keyword_only: None,
            raises: false,
            raises_type: None,
            ret: Some(Type::Named("Bool".to_string(), Vec::new())),
            where_clauses,
            body,
            provenance: mojito_ast::ast::MethodProvenance::SynthesizedDefault,
        });
    }
}

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

/// Each trait the program declares, with the traits it refines.
fn declared_trait_parents(program: &[Stmt]) -> HashMap<String, Vec<String>> {
    program
        .iter()
        .filter_map(|statement| match &statement.kind {
            StmtKind::Trait { name, refines, .. } => Some((name.clone(), refines.clone())),
            _ => None,
        })
        .collect()
}

/// Whether a struct's declared conformance `conformance` is `required` or
/// refines it, through the program's `declared` traits and the `installed`
/// package's.
fn conformance_reaches(
    declared: &HashMap<String, Vec<String>>,
    installed: Option<&InstalledInterfaces>,
    conformance: &str,
    required: &str,
) -> bool {
    let parents = |name: &str| {
        declared
            .get(name)
            .map(Vec::as_slice)
            .or_else(|| installed?.trait_parents(name))
    };
    conformance == required || mojito_types::conformance::refines(&parents, conformance, required)
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
/// An imported package's aliases, as its build elaborated them, bound
/// `program`'s parameters too, each looked up by the bound's name.
pub(super) fn fold_simd_alias_bounds(
    program: &mut [Stmt],
    installed: Option<&InstalledInterfaces>,
) {
    let imported = |name: &str| {
        let ty = installed?.type_alias(name)?;
        if !matches!(*ty.kind(), TyKind::Simd { .. }) {
            return None;
        }
        match source_type_from_ty(ty)? {
            Type::Named(_, args) => Some(args),
            _ => None,
        }
    };
    let own: HashMap<String, Vec<ParamArg>> = program
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
    if own.is_empty() && installed.is_none() {
        return;
    }
    for statement in program {
        let StmtKind::Struct { type_params, .. } = &mut statement.kind else {
            continue;
        };
        for parameter in type_params {
            if parameter.value_type.is_none()
                && let [only] = parameter.bounds.as_slice()
                && let Some(args) = own.get(only).cloned().or_else(|| imported(only))
            {
                parameter.value_type = Some(Type::Named("SIMD".to_string(), args));
                parameter.bounds = vec!["SIMD".to_string()];
            }
        }
    }
}

/// Fold every parameter whose type is one of upstream's KGEN types,
/// spelled directly, through a module alias of it (the program's own or an
/// imported package's), or through a member alias of it on one of the
/// program's own structs (`elt_type: Self.KGENTypeType`, `T:
/// _MLIR.KGENTypeType`, `s: KGENString`).
///
/// A parameter typed `` __mlir_type.`!kgen.type` `` becomes a type binder
/// bounded by [`TYPE_OF_ALL_TYPES`], as upstream binds a type to such a
/// parameter. The bound itself names no declaration, so a parameter
/// spelling it is rejected, as at the pin. A parameter typed
/// `` __mlir_type.`!kgen.string` `` becomes a value binder of that type
/// spelled directly, since an alias name in a binder's type otherwise reads
/// as a trait bound.
pub(super) fn fold_kgen_type_parameters(
    program: &mut [Stmt],
    installed: Option<&InstalledInterfaces>,
) -> Result<(), ComptimeError> {
    let module_aliases: HashMap<String, Ty> = program
        .iter()
        .filter_map(|statement| match &statement.kind {
            StmtKind::Comptime {
                name,
                type_params,
                value,
                ..
            } if type_params.is_empty() => Some((name.clone(), spelled_kgen_type(value)?)),
            _ => None,
        })
        .collect();
    let members: HashMap<String, HashMap<String, Ty>> = program
        .iter()
        .filter_map(|statement| match &statement.kind {
            StmtKind::Struct {
                name, associated, ..
            } => Some((
                name.clone(),
                associated
                    .iter()
                    .filter(|member| member.params.is_empty())
                    .filter_map(|member| {
                        Some((member.name.clone(), spelled_kgen_type(&member.value)?))
                    })
                    .collect::<HashMap<_, _>>(),
            )),
            _ => None,
        })
        .filter(|(_, aliases)| !aliases.is_empty())
        .collect();
    let mut fold = KgenTypeFold {
        module_aliases: &module_aliases,
        installed,
        members: &members,
        own: None,
        error: None,
    };
    for statement in program {
        fold.own = match &statement.kind {
            StmtKind::Struct { name, .. } => members.get(name),
            _ => None,
        };
        mojito_ast::visit::walk_stmt_mut(&mut fold, statement);
    }
    fold.error.map_or(Ok(()), Err)
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

/// The KGEN type `expr` spells, `` __mlir_type.`!kgen.type` `` or
/// `` __mlir_type.`!kgen.string` ``.
fn spelled_kgen_type(expr: &Expr) -> Option<Ty> {
    match &expr.kind {
        ExprKind::Member { object, field } if matches!(&object.kind, ExprKind::Identifier(name) if name == "__mlir_type") => {
            mojito_types::types::mlir_type_named(field)
        }
        _ => None,
    }
}

/// The [`fold_kgen_type_parameters`] walk: the program's KGEN type aliases,
/// and those of the struct being walked, which `Self.` names.
struct KgenTypeFold<'a> {
    module_aliases: &'a HashMap<String, Ty>,
    installed: Option<&'a InstalledInterfaces>,
    members: &'a HashMap<String, HashMap<String, Ty>>,
    own: Option<&'a HashMap<String, Ty>>,
    error: Option<ComptimeError>,
}

impl KgenTypeFold<'_> {
    /// The KGEN type `parameter`'s type spelling denotes, if any.
    fn denoted(&self, parameter: &TypeParam) -> Option<Ty> {
        match &parameter.value_type {
            Some(Type::Assoc { base, name, args }) if args.is_empty() => match base.as_ref() {
                Type::Named(base, base_args) if base_args.is_empty() => {
                    if base == "__mlir_type" {
                        mojito_types::types::mlir_type_named(name)
                    } else {
                        self.members.get(base)?.get(name).cloned()
                    }
                }
                _ => None,
            },
            Some(Type::SelfParam(name)) => self.own?.get(name).cloned(),
            Some(_) => None,
            None => match parameter.bounds.as_slice() {
                [only] => self.module_aliases.get(only).cloned().or_else(|| {
                    self.installed?
                        .type_alias(only)
                        .filter(|ty| matches!(ty.kind(), TyKind::TypeType | TyKind::KgenString))
                        .cloned()
                }),
                _ => None,
            },
        }
    }
}

impl mojito_ast::visit::MutVisitor for KgenTypeFold<'_> {
    fn visit_type_param_mut(&mut self, parameter: &mut TypeParam) {
        if parameter.value_type.is_none()
            && parameter
                .bounds
                .iter()
                .any(|bound| bound == TYPE_OF_ALL_TYPES)
        {
            self.error.get_or_insert_with(|| {
                ComptimeError::Type(mojito_common::error::TypeError::UnknownTrait(
                    TYPE_OF_ALL_TYPES.to_string(),
                ))
            });
            return;
        }
        match self.denoted(parameter).as_ref().map(Ty::kind) {
            Some(TyKind::TypeType) => {
                parameter.value_type = None;
                parameter.bounds = vec![TYPE_OF_ALL_TYPES.to_string()];
            }
            Some(TyKind::KgenString) => {
                parameter.value_type = Some(kgen_string_source_type());
                parameter.bounds = vec!["__mlir_type".to_string()];
            }
            _ => {}
        }
    }
}

/// Whether `method` takes one `mut` writer, `Some[Writer]` or a binder of
/// its own bounded by `Writer`, as `Writable`'s requirements do.
fn takes_writer(method: &mojito_ast::ast::Method) -> bool {
    let names_writer = |arg: &ParamArg| match arg {
        ParamArg::Type(Type::Named(name, args)) => name == "Writer" && args.is_empty(),
        ParamArg::Value(expr) => {
            matches!(&expr.kind, ExprKind::Identifier(name) if name == "Writer")
        }
        _ => false,
    };
    matches!(
        method.params.as_slice(),
        [parameter] if parameter.convention == Some(mojito_ast::ast::ArgConvention::Mut)
            && match &parameter.ty {
                Type::Named(some, args) if some == "Some" => {
                    matches!(args.as_slice(), [arg] if names_writer(arg))
                }
                Type::Named(binder, args) if args.is_empty() => method
                    .type_params
                    .iter()
                    .any(|param| param.name == *binder && param.bounds.iter().any(|bound| bound == "Writer")),
                _ => false,
            }
    )
}

/// The body of a synthesized `Writable` default: `Name[params](`, each
/// field's `name=` and value through `field_writer`, then `)`.
fn writable_default_body(
    fields: &[mojito_ast::ast::Param],
    field_writer: &str,
    is_repr: bool,
    span: Span,
) -> Vec<Stmt> {
    let identifier = |name: &str| Expr::new(ExprKind::Identifier(name.to_string()), span);
    let write_string = |text: Expr| {
        mk(
            StmtKind::Expr(Expr::new(
                ExprKind::MethodCall {
                    object: Box::new(identifier("writer")),
                    method: "write_string".to_string(),
                    args: vec![text],
                    kwargs: Vec::new(),
                },
                span,
            )),
            span,
        )
    };
    let literal = |text: String| Expr::new(ExprKind::Str(text), span);
    let type_name = Expr::new(
        ExprKind::Call {
            name: "_unqualified_type_name".to_string(),
            param_args: vec![ParamArg::Type(Type::SelfType)],
            args: Vec::new(),
            kwargs: Vec::new(),
        },
        span,
    );
    let mut body = vec![
        write_string(type_name),
        write_string(literal("(".to_string())),
    ];
    for (index, field) in fields.iter().enumerate() {
        let separator = if index == 0 { "" } else { ", " };
        body.push(write_string(literal(format!("{separator}{}=", field.name))));
        let value = Expr::new(
            ExprKind::Member {
                object: Box::new(identifier("self")),
                field: field.name.clone(),
            },
            span,
        );
        body.push(mk(
            StmtKind::Expr(Expr::new(
                ExprKind::Call {
                    name: field_writer.to_string(),
                    param_args: vec![ParamArg::Value(Expr::new(ExprKind::Bool(is_repr), span))],
                    args: vec![value, identifier("writer")],
                    kwargs: Vec::new(),
                },
                span,
            )),
            span,
        ));
    }
    body.push(write_string(literal(")".to_string())));
    body
}
