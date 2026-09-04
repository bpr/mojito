use mojito::ast::{
    ArgConvention, Capture, CaptureKind, CollectionKind, ComprehensionClause, Decorator, Expr,
    ExprKind, FnParam, FunctionTypeParam, ImportName, ImportNames, InfixOp, KwArg, LoopBindingMode,
    Method, Param, ParamArg, ParamKind, PrefixOp, Stmt, StmtKind, StructComptime, TStringPart,
    TraitComptime, TraitMethod, Type, TypeParam, WithItem,
};
use mojito::{FloatLiteral, Lexer, Parser, parse_diagnostics};

/// Box an `ExprKind` into a `Box<Expr>` child (dummy span; equality ignores it).
fn bx(kind: ExprKind) -> Box<Expr> {
    Box::new(Expr::from(kind))
}

/// The `@fieldwise_init` decorator, as it appears in a parsed struct's list.
fn fieldwise_deco() -> Decorator {
    Decorator {
        path: vec!["fieldwise_init".into()],
        args: vec![],
        kwargs: vec![],
    }
}

/// A plain (regular, no-default, no-convention) function parameter.
fn fnparam(name: &str, ty: Type) -> FnParam {
    FnParam {
        name: name.into(),
        ty,
        default: None,
        kind: ParamKind::Regular,
        convention: None,
        origin: None,
    }
}

fn iname(name: &str, alias: Option<&str>) -> ImportName {
    ImportName {
        name: name.into(),
        alias: alias.map(Into::into),
    }
}

fn parse(source: &str) -> Vec<Stmt> {
    let mut parser = Parser::new(Lexer::new(source));
    parser.parse_program().expect("parse error")
}

/// Parse a single bare-expression statement and return its expression.
fn parse_expr(source: &str) -> Expr {
    let stmts = parse(source);
    assert_eq!(stmts.len(), 1, "expected exactly one statement");
    match stmts.into_iter().next().unwrap().kind {
        StmtKind::Expr(expr) => expr,
        other => panic!("expected an expression statement, got {:?}", other),
    }
}

fn int(n: i64) -> Box<Expr> {
    bx(ExprKind::Int(n.into()))
}

fn int_expr(n: i64) -> Expr {
    Expr::from(ExprKind::Int(n.into()))
}

fn float_literal(n: f64) -> FloatLiteral {
    FloatLiteral::from_f64(n).expect("finite float literal")
}

fn ident(name: &str) -> Box<Expr> {
    bx(ExprKind::Identifier(name.into()))
}

#[test]
fn product_binds_tighter_than_sum() {
    // 1 + 2 * 3  ==  1 + (2 * 3)
    assert_eq!(
        parse_expr("1 + 2 * 3"),
        Expr::from(ExprKind::Infix(
            InfixOp::Add,
            int(1),
            bx(ExprKind::Infix(InfixOp::Mul, int(2), int(3)))
        ))
    );
}

#[test]
fn parentheses_override_precedence() {
    // (1 + 2) * 3
    assert_eq!(
        parse_expr("(1 + 2) * 3"),
        Expr::from(ExprKind::Infix(
            InfixOp::Mul,
            bx(ExprKind::Infix(InfixOp::Add, int(1), int(2))),
            int(3)
        ))
    );
}

#[test]
fn subtraction_is_left_associative() {
    // 1 - 2 - 3  ==  (1 - 2) - 3
    assert_eq!(
        parse_expr("1 - 2 - 3"),
        Expr::from(ExprKind::Infix(
            InfixOp::Sub,
            bx(ExprKind::Infix(InfixOp::Sub, int(1), int(2))),
            int(3)
        ))
    );
}

#[test]
fn unary_minus_binds_tighter_than_sum() {
    // -a + 1  ==  (-a) + 1
    assert_eq!(
        parse_expr("-a + 1"),
        Expr::from(ExprKind::Infix(
            InfixOp::Add,
            bx(ExprKind::Prefix(PrefixOp::Neg, ident("a"))),
            int(1)
        ))
    );
}

#[test]
fn not_binds_looser_than_comparison() {
    // not a == b  ==  not (a == b)
    assert_eq!(
        parse_expr("not a == b"),
        Expr::from(ExprKind::Prefix(
            PrefixOp::Not,
            bx(ExprKind::Infix(InfixOp::Eq, ident("a"), ident("b")))
        ))
    );
}

#[test]
fn or_is_looser_than_and() {
    // a or b and c  ==  a or (b and c)
    assert_eq!(
        parse_expr("a or b and c"),
        Expr::from(ExprKind::Infix(
            InfixOp::Or,
            ident("a"),
            bx(ExprKind::Infix(InfixOp::And, ident("b"), ident("c")))
        ))
    );
}

#[test]
fn parses_call_with_args() {
    assert_eq!(
        parse_expr("f(1, a)"),
        Expr::from(ExprKind::Call {
            name: "f".into(),
            param_args: vec![],
            args: vec![int_expr(1), Expr::from(ExprKind::Identifier("a".into()))],
            kwargs: vec![],
        })
    );
}

#[test]
fn parses_struct_with_field_and_method() {
    let stmts = parse(
        "@fieldwise_init\nstruct Point:\n    var x: Int\n\n    def get(self) -> Int:\n        return self.x\n",
    );
    assert_eq!(
        stmts[0],
        Stmt::from(StmtKind::Struct {
            name: "Point".into(),
            decorators: vec![fieldwise_deco()],
            type_params: vec![],
            conforms: vec![],
            callable_conformance: None,
            conformance_conditions: vec![],
            where_clauses: Vec::new(),
            fields: vec![Param {
                name: "x".into(),
                ty: Type::Int
            }],
            associated: vec![],
            methods: vec![Method {
                where_clauses: Vec::new(),
                type_params: vec![],
                name: "get".into(),
                has_self: true,
                self_convention: None,
                self_origin: None,
                decorators: vec![],
                params: vec![],
                positional_only: None,
                keyword_only: None,
                raises: false,
                raises_type: None,
                ret: Some(Type::Int),
                body: vec![Stmt::from(StmtKind::Return(Some(Expr::from(
                    ExprKind::Member {
                        object: ident("self"),
                        field: "x".into(),
                    }
                ))))],
            }],
            fieldwise_init: true,
        })
    );
}

#[test]
fn parses_member_access_and_method_call() {
    assert_eq!(
        parse_expr("p.x"),
        Expr::from(ExprKind::Member {
            object: ident("p"),
            field: "x".into()
        })
    );
    assert_eq!(
        parse_expr("p.move(1, a)"),
        Expr::from(ExprKind::MethodCall {
            object: ident("p"),
            method: "move".into(),
            args: vec![int_expr(1), Expr::from(ExprKind::Identifier("a".into()))],
            kwargs: vec![],
        })
    );
}

#[test]
fn member_access_chains_left_to_right() {
    // a.b.c  ==  (a.b).c
    assert_eq!(
        parse_expr("a.b.c"),
        Expr::from(ExprKind::Member {
            object: bx(ExprKind::Member {
                object: ident("a"),
                field: "b".into()
            }),
            field: "c".into(),
        })
    );
}

#[test]
fn power_is_right_associative_and_binds_tighter_than_unary_minus() {
    // 2 ** 3 ** 2  ==  2 ** (3 ** 2)
    assert_eq!(
        parse_expr("2 ** 3 ** 2"),
        Expr::from(ExprKind::Infix(
            InfixOp::Pow,
            int(2),
            bx(ExprKind::Infix(InfixOp::Pow, int(3), int(2))),
        ))
    );
    // -2 ** 2  ==  -(2 ** 2)
    assert_eq!(
        parse_expr("-2 ** 2"),
        Expr::from(ExprKind::Prefix(
            PrefixOp::Neg,
            bx(ExprKind::Infix(InfixOp::Pow, int(2), int(2))),
        ))
    );
}

#[test]
fn floor_div_and_mod_have_product_precedence() {
    // 1 + 6 // 4 % 3  ==  1 + ((6 // 4) % 3)
    assert_eq!(
        parse_expr("1 + 6 // 4 % 3"),
        Expr::from(ExprKind::Infix(
            InfixOp::Add,
            int(1),
            bx(ExprKind::Infix(
                InfixOp::Mod,
                bx(ExprKind::Infix(InfixOp::FloorDiv, int(6), int(4))),
                int(3),
            )),
        ))
    );
}

#[test]
fn parses_float_literal_and_division() {
    // 1.0 / 2.0 + 3  ==  (1.0 / 2.0) + 3   ('/' has product precedence)
    assert_eq!(
        parse_expr("1.0 / 2.0 + 3"),
        Expr::from(ExprKind::Infix(
            InfixOp::Add,
            bx(ExprKind::Infix(
                InfixOp::Div,
                bx(ExprKind::Float(float_literal(1.0))),
                bx(ExprKind::Float(float_literal(2.0))),
            )),
            int(3),
        ))
    );
}

#[test]
fn parses_uint_and_float64_annotations() {
    assert_eq!(
        parse("var u: UInt = UInt(0)")[0],
        Stmt::from(StmtKind::VarDecl {
            name: "u".into(),
            ty: Some(Type::UInt),
            value: Expr::from(ExprKind::Call {
                name: "UInt".into(),
                param_args: vec![],
                args: vec![int_expr(0)],
                kwargs: vec![]
            }),
        })
    );
    assert_eq!(
        parse("var f: Float64 = 3.5")[0],
        Stmt::from(StmtKind::VarDecl {
            name: "f".into(),
            ty: Some(Type::Float64),
            value: Expr::from(ExprKind::Float(float_literal(3.5))),
        })
    );
}

#[test]
fn parses_typed_var_decl() {
    assert_eq!(
        parse("var x: Int = 1 + 2")[0],
        Stmt::from(StmtKind::VarDecl {
            name: "x".into(),
            ty: Some(Type::Int),
            value: Expr::from(ExprKind::Infix(InfixOp::Add, int(1), int(2))),
        })
    );
}

#[test]
fn parses_def_signature_and_body() {
    let stmts = parse("def add(a: Int, b: Int) -> Int:\n    return a + b\n");
    assert_eq!(
        stmts[0],
        Stmt::from(StmtKind::Def {
            where_clauses: Vec::new(),
            name: "add".into(),
            decorators: vec![],
            type_params: vec![],
            params: vec![fnparam("a", Type::Int), fnparam("b", Type::Int)],
            positional_only: None,
            keyword_only: None,
            captures: None,
            raises: false,
            raises_type: None,
            ret: Some(Type::Int),
            body: vec![Stmt::from(StmtKind::Return(Some(Expr::from(
                ExprKind::Infix(InfixOp::Add, ident("a"), ident("b"))
            ))))],
        })
    );
}

#[test]
fn parses_if_elif_else() {
    let stmts = parse("if a:\n    pass\nelif b:\n    pass\nelse:\n    pass\n");
    assert_eq!(
        stmts[0],
        Stmt::from(StmtKind::If {
            branches: vec![
                (
                    Expr::from(ExprKind::Identifier("a".into())),
                    vec![Stmt::from(StmtKind::Pass)]
                ),
                (
                    Expr::from(ExprKind::Identifier("b".into())),
                    vec![Stmt::from(StmtKind::Pass)]
                ),
            ],
            orelse: Some(vec![Stmt::from(StmtKind::Pass)]),
        })
    );
}

#[test]
fn parses_if_without_else() {
    let stmts = parse("if a:\n    pass\n");
    assert_eq!(
        stmts[0],
        Stmt::from(StmtKind::If {
            branches: vec![(
                Expr::from(ExprKind::Identifier("a".into())),
                vec![Stmt::from(StmtKind::Pass)]
            )],
            orelse: None,
        })
    );
}

#[test]
fn parses_while() {
    let stmts = parse("while a:\n    pass\n");
    assert_eq!(
        stmts[0],
        Stmt::from(StmtKind::While {
            cond: Expr::from(ExprKind::Identifier("a".into())),
            body: vec![Stmt::from(StmtKind::Pass)],
            orelse: None,
        })
    );
}

#[test]
fn parses_for_over_range() {
    let stmts = parse("for i in range(n):\n    pass\n");
    assert_eq!(
        stmts[0],
        Stmt::from(StmtKind::For {
            var: "i".into(),
            binding: LoopBindingMode::Immutable,
            iter: Expr::from(ExprKind::Call {
                name: "range".into(),
                param_args: vec![],
                args: vec![Expr::from(ExprKind::Identifier("n".into()))],
                kwargs: vec![],
            }),
            body: vec![Stmt::from(StmtKind::Pass)],
            orelse: None,
        })
    );
}

#[test]
fn parses_var_loop_binding_and_collection_comprehensions() {
    let loop_statement = parse("for var item in values^:\n    pass\n");
    assert!(matches!(
        &loop_statement[0].kind,
        StmtKind::For {
            var,
            binding: LoopBindingMode::Var,
            iter: Expr {
                kind: ExprKind::Transfer(_),
                ..
            },
            ..
        } if var == "item"
    ));

    let statement =
        parse("var result = {x: x * y for x in range(3) for y in range(2) if y == 1}\n");
    let StmtKind::VarDecl { value, .. } = &statement[0].kind else {
        panic!("expected variable declaration");
    };
    let ExprKind::Comprehension {
        kind, key, clauses, ..
    } = &value.kind
    else {
        panic!("expected dictionary comprehension");
    };
    assert_eq!(*kind, CollectionKind::Dict);
    assert!(key.is_some());
    assert_eq!(clauses.len(), 3);
    assert!(matches!(
        clauses[0],
        ComprehensionClause::For {
            binding: LoopBindingMode::Immutable,
            ..
        }
    ));
    assert!(matches!(
        clauses[1],
        ComprehensionClause::For {
            binding: LoopBindingMode::Immutable,
            ..
        }
    ));
    assert!(matches!(clauses[2], ComprehensionClause::If(_)));
}

#[test]
fn parses_explicit_reference_loop_bindings() {
    let loop_statement = parse("for ref item in values:\n    pass\n");
    assert!(matches!(
        &loop_statement[0].kind,
        StmtKind::For {
            var,
            binding: LoopBindingMode::Ref,
            ..
        } if var == "item"
    ));

    let statement = parse("var result = [item for ref item in values]\n");
    let StmtKind::VarDecl { value, .. } = &statement[0].kind else {
        panic!("expected variable declaration");
    };
    let ExprKind::Comprehension { clauses, .. } = &value.kind else {
        panic!("expected comprehension");
    };
    assert!(matches!(
        &clauses[0],
        ComprehensionClause::For {
            var,
            binding: LoopBindingMode::Ref,
            ..
        } if var == "item"
    ));
}

#[test]
fn parses_assignment() {
    assert_eq!(
        parse("x = 1 + 2")[0],
        Stmt::from(StmtKind::Assign {
            name: "x".into(),
            value: Expr::from(ExprKind::Infix(InfixOp::Add, int(1), int(2))),
        })
    );
}

#[test]
fn rejects_non_identifier_assignment_target() {
    let mut parser = Parser::new(Lexer::new("1 = 2\n"));
    assert!(parser.parse_program().is_err());
}

#[test]
fn parses_break_and_continue() {
    let stmts = parse("while a:\n    break\n    continue\n");
    assert_eq!(
        stmts[0],
        Stmt::from(StmtKind::While {
            cond: Expr::from(ExprKind::Identifier("a".into())),
            body: vec![Stmt::from(StmtKind::Break), Stmt::from(StmtKind::Continue)],
            orelse: None,
        })
    );
}

// --- Parameterization (generics) ---

#[test]
fn parses_generic_struct_header_and_self_param_field() {
    let stmts = parse(
        "@fieldwise_init\nstruct Pair[T: Copyable & Movable]:\n    var left: Self.T\n    var right: Self.T\n",
    );
    assert_eq!(
        stmts[0],
        Stmt::from(StmtKind::Struct {
            name: "Pair".into(),
            decorators: vec![fieldwise_deco()],
            type_params: vec![TypeParam {
                constraints: vec![],
                value_type: None,
                name: "T".into(),
                bounds: vec!["Copyable".into(), "Movable".into()],
                callable_bound: None,
                origin_mutability: None,
                infer_only: false,
                default: None,
            }],
            conforms: vec![],
            callable_conformance: None,
            conformance_conditions: vec![],
            where_clauses: Vec::new(),
            fields: vec![
                Param {
                    name: "left".into(),
                    ty: Type::SelfParam("T".into())
                },
                Param {
                    name: "right".into(),
                    ty: Type::SelfParam("T".into())
                },
            ],
            associated: vec![],
            methods: vec![],
            fieldwise_init: true,
        })
    );
}

#[test]
fn parses_generic_def_with_type_param_signature() {
    let stmts = parse("def id[T: AnyType](x: T) -> T:\n    return x\n");
    assert_eq!(
        stmts[0],
        Stmt::from(StmtKind::Def {
            where_clauses: Vec::new(),
            name: "id".into(),
            decorators: vec![],
            type_params: vec![TypeParam {
                constraints: vec![],
                value_type: None,
                name: "T".into(),
                bounds: vec!["AnyType".into()],
                callable_bound: None,
                origin_mutability: None,
                infer_only: false,
                default: None,
            }],
            params: vec![fnparam("x", Type::Named("T".into(), vec![]))],
            positional_only: None,
            keyword_only: None,
            captures: None,
            raises: false,
            raises_type: None,
            ret: Some(Type::Named("T".into(), vec![])),
            body: vec![Stmt::from(StmtKind::Return(Some(Expr::from(
                ExprKind::Identifier("x".into())
            ))))],
        })
    );
}

#[test]
fn parses_parameterized_type_annotation() {
    // `Pair[Int]` in a `var` annotation carries its type argument.
    let stmts = parse("var p: Pair[Int] = q\n");
    match &stmts[0].kind {
        StmtKind::VarDecl { ty: Some(ty), .. } => {
            assert_eq!(
                *ty,
                Type::Named("Pair".into(), vec![ParamArg::Type(Type::Int)])
            );
        }
        other => panic!("expected a var decl, got {:?}", other),
    }
}

#[test]
fn rejects_type_parameter_without_a_bound() {
    // Mojo has no unconstrained type parameters, so `[T]` is a parse error.
    let mut parser = Parser::new(Lexer::new("def f[T](x: T) -> T:\n    return x\n"));
    assert!(parser.parse_program().is_err());
}

// --- Traits (Phase 1b) ---

#[test]
fn parses_trait_with_method_requirements() {
    let stmts = parse(
        "trait Quackable:\n    def quack(self) -> String:\n        ...\n    def volume(self, loud: Bool) -> Int:\n        ...\n",
    );
    assert_eq!(
        stmts[0],
        Stmt::from(StmtKind::Trait {
            name: "Quackable".into(),
            refines: vec![],
            methods: vec![
                TraitMethod {
                    where_clauses: Vec::new(),
                    type_params: vec![],
                    name: "quack".into(),
                    self_convention: None,
                    self_origin: None,
                    params: vec![],
                    positional_only: None,
                    keyword_only: None,
                    raises: false,
                    raises_type: None,
                    ret: Some(Type::Named("String".into(), vec![])),
                    default_body: None,
                },
                TraitMethod {
                    where_clauses: Vec::new(),
                    type_params: vec![],
                    name: "volume".into(),
                    self_convention: None,
                    self_origin: None,
                    params: vec![fnparam("loud", Type::Bool)],
                    positional_only: None,
                    keyword_only: None,
                    raises: false,
                    raises_type: None,
                    ret: Some(Type::Int),
                    default_body: None,
                },
            ],
            comptime_members: vec![],
        })
    );
}

#[test]
fn parses_single_line_trait_method_requirement() {
    let stmts = parse("trait Animal:\n    def make_sound(self): ...\n");
    match &stmts[0].kind {
        StmtKind::Trait { methods, .. } => {
            assert_eq!(methods.len(), 1);
            assert_eq!(methods[0].name, "make_sound");
            assert_eq!(methods[0].default_body, None);
        }
        other => panic!("expected a trait, got {:?}", other),
    }
}

#[test]
fn parses_raises_effect_on_trait_requirements() {
    let stmts = parse("trait Fallible:\n    def run(self) raises ValidationError -> Int: ...\n");
    let StmtKind::Trait { methods, .. } = &stmts[0].kind else {
        panic!("expected a trait");
    };
    assert!(methods[0].raises);
    assert_eq!(
        methods[0].raises_type,
        Some(Type::Named("ValidationError".into(), Vec::new()))
    );
}

#[test]
fn parses_single_line_pass_suite() {
    let stmts = parse("def noop(): pass\n");
    match &stmts[0].kind {
        StmtKind::Def { body, .. } => {
            assert_eq!(body, &vec![Stmt::from(StmtKind::Pass)]);
        }
        other => panic!("expected a def, got {:?}", other),
    }
}

#[test]
fn parses_placeholder_and_semicolon_function_styles() {
    let stmts = parse(concat!(
        "def func1(r: Int): ...\n",
        "def func2(): pass\n",
        "def func3(): print(\"Hello World!\"); print(\"Good bye!\")\n",
        "def func4():\n",
        "    pass\n",
        "def main(): func3()\n",
    ));
    assert_eq!(stmts.len(), 5);
    match &stmts[0].kind {
        StmtKind::Def { body, .. } => assert_eq!(body, &vec![Stmt::from(StmtKind::Pass)]),
        other => panic!("expected a def, got {:?}", other),
    }
    match &stmts[2].kind {
        StmtKind::Def { body, .. } => assert_eq!(body.len(), 2),
        other => panic!("expected a def, got {:?}", other),
    }
    match &stmts[4].kind {
        StmtKind::Def { body, .. } => assert_eq!(body.len(), 1),
        other => panic!("expected a def, got {:?}", other),
    }
}

#[test]
fn parses_trait_receiver_conventions() {
    let stmts = parse(
        "trait Receivers:\n    def read_it(self):\n        ...\n    def mutate(mut self):\n        ...\n    def consume(var self):\n        ...\n    def borrow(ref self):\n        ...\n",
    );
    match &stmts[0].kind {
        StmtKind::Trait { methods, .. } => {
            assert_eq!(methods[0].self_convention, None);
            assert_eq!(methods[1].self_convention, Some(ArgConvention::Mut));
            assert_eq!(methods[2].self_convention, Some(ArgConvention::Var));
            assert_eq!(methods[3].self_convention, Some(ArgConvention::Ref));
        }
        other => panic!("expected a trait, got {:?}", other),
    }
}

#[test]
fn parses_struct_conformance_list() {
    let stmts = parse("@fieldwise_init\nstruct Duck(Copyable, Quackable):\n    var name: String\n");
    match &stmts[0].kind {
        StmtKind::Struct { conforms, .. } => {
            assert_eq!(
                conforms,
                &vec!["Copyable".to_string(), "Quackable".to_string()]
            );
        }
        other => panic!("expected a struct, got {:?}", other),
    }
}

#[test]
fn retains_conditional_struct_conformance_predicates() {
    let statements =
        parse("struct Wrapper[T: AnyType](Writable where conforms_to(T, Writable)):\n    pass\n");
    let StmtKind::Struct {
        conforms,
        conformance_conditions,
        ..
    } = &statements[0].kind
    else {
        panic!("expected struct");
    };
    assert_eq!(conforms, &["Writable"]);
    assert_eq!(conformance_conditions.len(), 1);
    assert_eq!(conformance_conditions[0].0, "Writable");
}

#[test]
fn parses_bare_self_type_in_trait_method() {
    // `other: Self` — the `Self` type in a trait requirement.
    let stmts = parse("trait Eq2:\n    def same(self, other: Self) -> Bool:\n        ...\n");
    match &stmts[0].kind {
        StmtKind::Trait { methods, .. } => {
            assert_eq!(methods[0].params[0].ty, Type::SelfType);
        }
        other => panic!("expected a trait, got {:?}", other),
    }
}

#[test]
fn parses_trait_default_method_body() {
    // A real method body parses as a default implementation (was a parse error);
    // the checker flags it — see the checker/asset tests.
    match &parse("trait Q:\n    def q(self) -> Int:\n        return 1\n")[0].kind {
        StmtKind::Trait { methods, .. } => {
            assert_eq!(
                methods[0].default_body,
                Some(vec![Stmt::from(StmtKind::Return(Some(Expr::from(
                    ExprKind::Int(1i64.into())
                ))))])
            );
        }
        other => panic!("expected a trait, got {:?}", other),
    }
}

#[test]
fn parses_trait_inheritance_list() {
    // `trait Bird(Animal, Named):` — the refinement (super-trait) list.
    match &parse("trait Bird(Animal, Named):\n    def fly(self):\n        ...\n")[0].kind {
        StmtKind::Trait {
            refines, methods, ..
        } => {
            assert_eq!(refines, &vec!["Animal".to_string(), "Named".to_string()]);
            assert_eq!(methods[0].default_body, None); // `...` is a pure requirement
        }
        other => panic!("expected a trait, got {:?}", other),
    }
}

#[test]
fn parses_trait_comptime_member() {
    // `comptime count: Int` — a compile-time member requirement.
    match &parse("trait Repeater:\n    comptime count: Int\n")[0].kind {
        StmtKind::Trait {
            comptime_members, ..
        } => {
            assert_eq!(
                comptime_members,
                &vec![TraitComptime {
                    name: "count".into(),
                    params: vec![],
                    ty: Type::Int,
                    where_clauses: Vec::new(),
                }]
            );
        }
        other => panic!("expected a trait, got {:?}", other),
    }
}

#[test]
fn parses_associated_type_annotation() {
    match &parse("def first[C: Iterable](c: C) -> C.Element:\n    pass\n")[0].kind {
        StmtKind::Def { ret, .. } => {
            assert_eq!(
                ret,
                &Some(Type::Assoc {
                    base: Box::new(Type::Named("C".into(), vec![])),
                    name: "Element".into(),
                    args: vec![],
                })
            );
        }
        other => panic!("expected a def, got {:?}", other),
    }
}

#[test]
fn parses_dependent_indexed_type_projection_structurally() {
    let program = parse(
        "def outer():\n    var values = (1, True)\n    def visit[index: Int](value: values.element_types[index + 1]):\n        pass\n",
    );
    let StmtKind::Def { body, .. } = &program[0].kind else {
        panic!("expected outer def");
    };
    let StmtKind::Def { params, .. } = &body[1].kind else {
        panic!("expected nested def");
    };
    assert_eq!(
        params[0].ty,
        Type::IndexedProjection {
            base: Box::new(Type::Assoc {
                base: Box::new(Type::Named("values".into(), vec![])),
                name: "element_types".into(),
                args: vec![],
            }),
            index: Box::new(Expr::from(ExprKind::Infix(
                InfixOp::Add,
                Box::new(Expr::from(ExprKind::Identifier("index".into()))),
                Box::new(int_expr(1)),
            ))),
        }
    );
}

#[test]
fn parses_struct_comptime_associated_member() {
    match &parse("@fieldwise_init\nstruct Box[T: AnyType]:\n    comptime Element = Self.T\n    var value: Self.T\n")[0].kind {
        StmtKind::Struct {
            associated,
            fields,
            ..
        } => {
            assert_eq!(
                associated,
                &vec![StructComptime {
                    name: "Element".into(),
                    params: vec![],
                    ty: None,
                    where_clauses: Vec::new(),
                    value: Expr::from(ExprKind::Member {
                        object: ident("Self"),
                        field: "T".into(),
                    })
                }]
            );
            assert_eq!(fields[0].name, "value");
        }
        other => panic!("expected a struct, got {:?}", other),
    }
}

// --- Value parameters + comptime (Phase 2) ---

#[test]
fn parses_comptime_constant() {
    let stmts = parse("comptime N = 2 * 4\n");
    assert_eq!(
        stmts[0],
        Stmt::from(StmtKind::Comptime {
            name: "N".into(),
            type_params: vec![],
            ty: None,
            where_clauses: Vec::new(),
            value: Expr::from(ExprKind::Infix(InfixOp::Mul, int(2), int(4))),
        })
    );
}

#[test]
fn parses_annotated_comptime_constant() {
    let stmts = parse("comptime counter: Int = 1\n");
    assert_eq!(
        stmts[0],
        Stmt::from(StmtKind::Comptime {
            name: "counter".into(),
            type_params: vec![],
            ty: Some(Type::Int),
            where_clauses: Vec::new(),
            value: int_expr(1),
        })
    );
}

#[test]
fn retains_where_messages_on_struct_and_comptime_declarations() {
    let statements = parse(
        "struct Box[T: AnyType] where (True, \"Box enabled\"):\n    comptime Item[U: AnyType]: AnyType where (True, \"Item enabled\") = U\n\ntrait HasItem:\n    comptime Item: AnyType where (True, \"requirement enabled\")\n\ncomptime Alias[T: AnyType]: AnyType where (True, \"Alias enabled\") = T\n",
    );

    let StmtKind::Struct {
        where_clauses,
        associated,
        ..
    } = &statements[0].kind
    else {
        panic!("expected struct declaration");
    };
    assert_eq!(where_clauses.len(), 1);
    assert_eq!(
        associated[0].ty,
        Some(Type::Named("AnyType".into(), vec![]))
    );
    assert_eq!(associated[0].where_clauses.len(), 1);

    let StmtKind::Trait {
        comptime_members, ..
    } = &statements[1].kind
    else {
        panic!("expected trait declaration");
    };
    assert_eq!(comptime_members[0].where_clauses.len(), 1);

    let StmtKind::Comptime {
        type_params,
        ty,
        where_clauses,
        ..
    } = &statements[2].kind
    else {
        panic!("expected comptime declaration");
    };
    assert_eq!(type_params.len(), 1);
    assert_eq!(ty, &Some(Type::Named("AnyType".into(), vec![])));
    assert_eq!(where_clauses.len(), 1);
}

#[test]
fn retains_repeated_where_clauses_independently_on_every_declaration_family() {
    let statements = parse(
        "def f[T: AnyType]() -> Int where (True, \"m1\") where (True, \"m2\"):\n    return 0\n\nstruct Box[T: AnyType] where (True, \"m1\") where (True, \"m2\"):\n    comptime Item[U: AnyType]: AnyType where (True, \"m1\") where (True, \"m2\") = U\n    def get[U: AnyType](self) -> Int where (True, \"m1\") where (True, \"m2\"):\n        return 0\n\ntrait HasItem:\n    comptime Item: AnyType where (True, \"m1\") where (True, \"m2\")\n    def req[U: AnyType](self) -> Int where (True, \"m1\") where (True, \"m2\"):\n        ...\n\ncomptime Alias[T: AnyType]: AnyType where (True, \"m1\") where (True, \"m2\") = T\n",
    );

    let StmtKind::Def { where_clauses, .. } = &statements[0].kind else {
        panic!("expected def declaration");
    };
    assert_eq!(where_clauses.len(), 2);

    let StmtKind::Struct {
        where_clauses,
        associated,
        methods,
        ..
    } = &statements[1].kind
    else {
        panic!("expected struct declaration");
    };
    assert_eq!(where_clauses.len(), 2);
    assert_eq!(associated[0].where_clauses.len(), 2);
    assert_eq!(methods[0].where_clauses.len(), 2);

    let StmtKind::Trait {
        comptime_members,
        methods,
        ..
    } = &statements[2].kind
    else {
        panic!("expected trait declaration");
    };
    assert_eq!(comptime_members[0].where_clauses.len(), 2);
    assert_eq!(methods[0].where_clauses.len(), 2);

    let StmtKind::Comptime { where_clauses, .. } = &statements[3].kind else {
        panic!("expected comptime declaration");
    };
    assert_eq!(where_clauses.len(), 2);
}

#[test]
fn parses_docstrings_in_declaration_positions() {
    let _ = parse(
        "\"\"\"Module docs.\"\"\"\n\nstruct S:\n    \"\"\"Struct docs.\"\"\"\n    var x: Int\n    \"\"\"Field docs.\"\"\"\n    comptime N = 1\n    \"\"\"Constant docs.\"\"\"\n    def get(self) -> Int:\n        \"\"\"Method docs.\"\"\"\n        return self.x\n\ntrait T:\n    \"\"\"Trait docs.\"\"\"\n    def get(self) -> Int:\n        ...\n",
    );
}

#[test]
fn parses_comptime_if_with_else() {
    // `comptime if` mirrors a normal `if` (branches + optional else).
    match &parse("comptime if N > 4:\n    pass\nelse:\n    pass\n")[0].kind {
        StmtKind::ComptimeIf { branches, orelse } => {
            assert_eq!(branches.len(), 1);
            assert_eq!(
                branches[0].0,
                Expr::from(ExprKind::Infix(InfixOp::Gt, ident("N"), int(4)))
            );
            assert_eq!(branches[0].1, vec![Stmt::from(StmtKind::Pass)]);
            assert_eq!(orelse, &Some(vec![Stmt::from(StmtKind::Pass)]));
        }
        other => panic!("expected a ComptimeIf, got {:?}", other),
    }
}

#[test]
fn parses_comptime_for() {
    assert_eq!(
        parse("comptime for i in range(4):\n    pass\n")[0],
        Stmt::from(StmtKind::ComptimeFor {
            var: "i".into(),
            iter: Expr::from(ExprKind::Call {
                name: "range".into(),
                param_args: vec![],
                args: vec![int_expr(4)],
                kwargs: vec![],
            }),
            body: vec![Stmt::from(StmtKind::Pass)],
        })
    );
}

#[test]
fn parses_value_parameter_header() {
    // `[size: Int]` parses like any other parameter (the checker classifies it).
    let stmts = parse("@fieldwise_init\nstruct FixedBuffer[size: Int]:\n    var tag: Int\n");
    match &stmts[0].kind {
        StmtKind::Struct { type_params, .. } => {
            assert_eq!(
                type_params,
                &vec![TypeParam {
                    constraints: vec![],
                    value_type: None,
                    name: "size".into(),
                    bounds: vec!["Int".into()],
                    callable_bound: None,
                    origin_mutability: None,
                    infer_only: false,
                    default: None,
                }]
            );
        }
        other => panic!("expected a struct, got {:?}", other),
    }
}

#[test]
fn retains_named_parameter_arguments_and_generic_method_parameters() {
    let statements = parse(
        "struct Factory:\n    def make[T: AnyType](self, value: T) -> T:\n        return value\n\ndef main():\n    var x = Factory.make[kind=Int](1)\n",
    );
    let StmtKind::Struct { methods, .. } = &statements[0].kind else {
        panic!("expected struct");
    };
    assert_eq!(methods[0].type_params.len(), 1);
}

#[test]
fn retains_trailing_where_constraints() {
    let statements = parse("def f[n: Int]() -> Int where n > 0 and n < 10:\n    return n\n");
    let StmtKind::Def {
        type_params,
        where_clauses,
        ..
    } = &statements[0].kind
    else {
        panic!("expected def");
    };
    assert!(type_params[0].constraints.is_empty());
    assert_eq!(where_clauses.len(), 1);
}

#[test]
fn rejects_removed_parameter_list_where_constraints() {
    assert!(mojito::parse("def f[n: Int where n > 0]():\n    pass\n").is_err());
}

#[test]
fn parses_explicit_value_argument_in_annotation_and_call() {
    // Value argument in an annotation: `FixedBuffer[2 + 3]`.
    let stmts = parse("var b: FixedBuffer[2 + 3] = FixedBuffer[5](0)\n");
    match &stmts[0].kind {
        StmtKind::VarDecl {
            ty: Some(ty),
            value,
            ..
        } => {
            assert_eq!(
                *ty,
                Type::Named(
                    "FixedBuffer".into(),
                    vec![ParamArg::Value(Expr::from(ExprKind::Infix(
                        InfixOp::Add,
                        int(2),
                        int(3)
                    )))],
                )
            );
            // Value argument in a construction: `FixedBuffer[5](0)`.
            assert_eq!(
                *value,
                Expr::from(ExprKind::Call {
                    name: "FixedBuffer".into(),
                    param_args: vec![ParamArg::Value(int_expr(5))],
                    args: vec![int_expr(0)],
                    kwargs: vec![],
                })
            );
        }
        other => panic!("expected a var decl, got {:?}", other),
    }
}

// --- Imports (parsed, not resolved) ---

#[test]
fn parses_import_dotted_with_alias() {
    assert_eq!(
        parse("import mypackage.mymodule as mm\n")[0],
        Stmt::from(StmtKind::Import {
            path: vec!["mypackage".into(), "mymodule".into()],
            alias: Some("mm".into())
        })
    );
    assert_eq!(
        parse("import mymodule\n")[0],
        Stmt::from(StmtKind::Import {
            path: vec!["mymodule".into()],
            alias: None
        })
    );
}

#[test]
fn parses_from_import_names_and_aliases() {
    assert_eq!(
        parse("from mypackage.mymodule import a, b as c, d\n")[0],
        Stmt::from(StmtKind::FromImport {
            level: 0,
            path: vec!["mypackage".into(), "mymodule".into()],
            names: ImportNames::Names(vec![
                iname("a", None),
                iname("b", Some("c")),
                iname("d", None),
            ]),
        })
    );
}

#[test]
fn parses_from_import_wildcard() {
    assert_eq!(
        parse("from mymodule import *\n")[0],
        Stmt::from(StmtKind::FromImport {
            level: 0,
            path: vec!["mymodule".into()],
            names: ImportNames::Wildcard
        })
    );
}

#[test]
fn parses_relative_imports() {
    // One dot before a module.
    assert_eq!(
        parse("from .mymodule import X\n")[0],
        Stmt::from(StmtKind::FromImport {
            level: 1,
            path: vec!["mymodule".into()],
            names: ImportNames::Names(vec![iname("X", None)])
        })
    );
    // Dots only (`from . import X`).
    assert_eq!(
        parse("from . import X\n")[0],
        Stmt::from(StmtKind::FromImport {
            level: 1,
            path: vec![],
            names: ImportNames::Names(vec![iname("X", None)])
        })
    );
    // Two dots.
    assert_eq!(
        parse("from ..pkg import X\n")[0],
        Stmt::from(StmtKind::FromImport {
            level: 2,
            path: vec!["pkg".into()],
            names: ImportNames::Names(vec![iname("X", None)])
        })
    );
    // Three dots come through as a single ellipsis token.
    assert_eq!(
        parse("from ...pkg import X\n")[0],
        Stmt::from(StmtKind::FromImport {
            level: 3,
            path: vec!["pkg".into()],
            names: ImportNames::Names(vec![iname("X", None)])
        })
    );
}

#[test]
fn rejects_from_without_a_module() {
    let mut parser = Parser::new(Lexer::new("from import X\n"));
    assert!(parser.parse_program().is_err());
}

// --- Exceptions ---

#[test]
fn parses_raise() {
    let stmts = parse("raise Error(\"boom\")\n");
    assert_eq!(
        stmts[0],
        Stmt::from(StmtKind::Raise(Expr::from(ExprKind::Call {
            name: "Error".into(),
            param_args: vec![],
            args: vec![Expr::from(ExprKind::Str("boom".into()))],
            kwargs: vec![],
        })))
    );
}

#[test]
fn parses_try_except_else_finally() {
    let stmts = parse("try:\n    pass\nexcept e:\n    pass\nelse:\n    pass\nfinally:\n    pass\n");
    assert_eq!(
        stmts[0],
        Stmt::from(StmtKind::Try {
            body: vec![Stmt::from(StmtKind::Pass)],
            except: Some((Some("e".into()), vec![Stmt::from(StmtKind::Pass)])),
            orelse: Some(vec![Stmt::from(StmtKind::Pass)]),
            finalbody: Some(vec![Stmt::from(StmtKind::Pass)]),
        })
    );
}

#[test]
fn parses_try_with_only_finally_and_bare_except() {
    // A bare `except:` (no name) and finally-only forms.
    assert_eq!(
        parse("try:\n    pass\nfinally:\n    pass\n")[0],
        Stmt::from(StmtKind::Try {
            body: vec![Stmt::from(StmtKind::Pass)],
            except: None,
            orelse: None,
            finalbody: Some(vec![Stmt::from(StmtKind::Pass)])
        })
    );
    assert_eq!(
        parse("try:\n    pass\nexcept:\n    pass\n")[0],
        Stmt::from(StmtKind::Try {
            body: vec![Stmt::from(StmtKind::Pass)],
            except: Some((None, vec![Stmt::from(StmtKind::Pass)])),
            orelse: None,
            finalbody: None
        })
    );
}

// --- With statements (context managers) ---

#[test]
fn parses_with_single_item_and_binding() {
    assert_eq!(
        parse("with open(p) as f:\n    pass\n")[0],
        Stmt::from(StmtKind::With {
            items: vec![WithItem {
                context: Expr::from(ExprKind::Call {
                    name: "open".into(),
                    param_args: vec![],
                    args: vec![Expr::from(ExprKind::Identifier("p".into()))],
                    kwargs: vec![],
                }),
                var: Some("f".into()),
            }],
            body: vec![Stmt::from(StmtKind::Pass)],
        })
    );
}

#[test]
fn parses_with_multiple_items_and_optional_binding() {
    // Comma-separated managers; the `as` binding is optional per item.
    match &parse("with a() as x, lock():\n    pass\n")[0].kind {
        StmtKind::With { items, body } => {
            assert_eq!(items.len(), 2);
            assert_eq!(items[0].var, Some("x".into()));
            assert_eq!(items[1].var, None);
            assert_eq!(body, &vec![Stmt::from(StmtKind::Pass)]);
        }
        other => panic!("expected a With statement, got {:?}", other),
    }
}

#[test]
fn rejects_with_missing_name_after_as() {
    let mut parser = Parser::new(Lexer::new("with open(p) as:\n    pass\n"));
    assert!(parser.parse_program().is_err());
}

#[test]
fn bare_raises_before_where_clauses_takes_no_error_type() {
    // A contextual `where` after a bare `raises` starts the constraint
    // clauses; it must not be consumed as the raises error type.
    match &parse("def f(x: Int) raises where conforms_to(Int, Copyable):\n    return x\n")[0].kind {
        StmtKind::Def {
            raises,
            raises_type,
            where_clauses,
            ..
        } => {
            assert!(*raises);
            assert_eq!(raises_type, &None);
            assert_eq!(where_clauses.len(), 1);
        }
        other => panic!("expected a def, got {:?}", other),
    }
}

#[test]
fn parses_raises_effect_on_def() {
    // Both the effect and its optional typed error are retained.
    match &parse("def f(x: Int) raises ValidationError -> Int:\n    return x\n")[0].kind {
        StmtKind::Def {
            raises,
            raises_type,
            ..
        } => {
            assert!(*raises);
            assert_eq!(
                raises_type,
                &Some(Type::Named("ValidationError".into(), Vec::new()))
            );
        }
        other => panic!("expected a def, got {:?}", other),
    }
    match &parse("def g(x: Int) -> Int:\n    return x\n")[0].kind {
        StmtKind::Def { raises, .. } => assert!(!*raises),
        other => panic!("expected a def, got {:?}", other),
    }
}

#[test]
fn parses_capturing_and_raises_in_either_effect_order() {
    for source in [
        "def first() capturing raises -> Int:\n    raise Error(\"boom\")\n",
        "def second() raises capturing -> Int:\n    raise Error(\"boom\")\n",
    ] {
        assert!(matches!(
            &parse(source)[0].kind,
            StmtKind::Def { raises: true, .. }
        ));
    }

    let structure = parse(
        "struct Callable:\n    def __call__(self) capturing raises -> Int:\n        raise Error(\"boom\")\n",
    );
    assert!(matches!(
        &structure[0].kind,
        StmtKind::Struct { methods, .. } if methods[0].raises
    ));

    let requirement =
        parse("trait Callable:\n    def __call__(self) capturing raises -> Int: ...\n");
    assert!(matches!(
        &requirement[0].kind,
        StmtKind::Trait { methods, .. } if methods[0].raises
    ));

    assert!(matches!(
        var_anno_type("var callback: def() capturing raises -> Int = first\n"),
        Type::Func { raises: true, .. }
    ));
}

#[test]
fn parses_current_and_legacy_closure_capture_lists() {
    let program = parse(
        "def outer():\n    var a = 1\n    var b = 2\n    var c = 3\n    var d = 4\n    var e = 5\n    def inner() raises {mut a, b, var c, var d^, ref e, imm}:\n        pass\n",
    );
    let StmtKind::Def { body, .. } = &program[0].kind else {
        panic!("expected outer def");
    };
    let StmtKind::Def {
        captures: Some(captures),
        ..
    } = &body[5].kind
    else {
        panic!("expected nested closure");
    };
    assert_eq!(captures.default, Some(CaptureKind::Imm));
    assert_eq!(
        captures.entries,
        vec![
            Capture {
                name: "a".into(),
                kind: CaptureKind::Mut,
            },
            Capture {
                name: "b".into(),
                kind: CaptureKind::Imm,
            },
            Capture {
                name: "c".into(),
                kind: CaptureKind::Copy,
            },
            Capture {
                name: "d".into(),
                kind: CaptureKind::Move,
            },
            Capture {
                name: "e".into(),
                kind: CaptureKind::Ref,
            },
        ]
    );

    let bare = parse("def outer():\n    var value = 1\n    def inner() {value}:\n        pass\n");
    let StmtKind::Def { body, .. } = &bare[0].kind else {
        panic!("expected outer def");
    };
    let StmtKind::Def {
        captures: Some(captures),
        ..
    } = &body[1].kind
    else {
        panic!("expected nested closure");
    };
    assert_eq!(captures.entries[0].kind, CaptureKind::Imm);
}

#[test]
fn rejects_removed_unified_capture_spelling() {
    let mut parser = Parser::new(Lexer::new(
        "def outer():\n    var value = 1\n    def inner() unified {value}:\n        pass\n",
    ));
    let err = parser
        .parse_program()
        .expect_err("unified must be rejected");
    assert!(
        format!("{err:?}").contains("'unified {...}' capture spelling is not accepted"),
        "unexpected diagnostic: {err:?}"
    );
}

#[test]
fn parses_transfer_sigil() {
    assert_eq!(parse_expr("x^"), Expr::from(ExprKind::Transfer(ident("x"))));
    // `raise e^` — transfer inside a raise.
    assert_eq!(
        parse("raise e^\n")[0],
        Stmt::from(StmtKind::Raise(Expr::from(ExprKind::Transfer(ident("e")))))
    );
}

#[test]
fn rejects_try_without_except_or_finally() {
    let mut parser = Parser::new(Lexer::new("try:\n    pass\nelse:\n    pass\n"));
    assert!(parser.parse_program().is_err());
}

// --- SIMD ---

#[test]
fn parses_simd_type_and_construction() {
    let stmts = parse("var v: SIMD[DType.int32, 4] = SIMD[DType.int32, 4](1, 2, 3, 4)\n");
    match &stmts[0].kind {
        StmtKind::VarDecl {
            ty: Some(ty),
            value,
            ..
        } => {
            assert_eq!(
                *ty,
                Type::Named(
                    "SIMD".into(),
                    vec![
                        ParamArg::Value(Expr::from(ExprKind::Member {
                            object: ident("DType"),
                            field: "int32".into(),
                        })),
                        ParamArg::Value(int_expr(4)),
                    ],
                )
            );
            match &value.kind {
                ExprKind::Call {
                    name,
                    param_args,
                    args,
                    ..
                } => {
                    assert_eq!(name, "SIMD");
                    assert_eq!(param_args.len(), 2);
                    assert_eq!(args.len(), 4);
                }
                other => panic!("expected a SIMD construction, got {:?}", other),
            }
        }
        other => panic!("expected a var decl, got {:?}", other),
    }
}

#[test]
fn parses_subscript_as_index() {
    // `v[0]` (no following `(`) is a subscript, not a generic call.
    assert_eq!(
        parse_expr("v[0]"),
        Expr::from(ExprKind::Index {
            object: ident("v"),
            index: int(0)
        })
    );
}

#[test]
fn parses_is_and_is_not_as_comparisons() {
    // `x is None` is an identity comparison (dispatching to `__is__`); the
    // two-word `is not` is one operator, never `is (not None)`.
    assert!(matches!(
        parse_expr("x is None").kind,
        ExprKind::Infix(InfixOp::Is, ref left, ref right)
            if matches!(left.kind, ExprKind::Identifier(ref name) if name == "x")
                && matches!(right.kind, ExprKind::None)
    ));
    assert!(matches!(
        parse_expr("x is not None").kind,
        ExprKind::Infix(InfixOp::IsNot, ref left, ref right)
            if matches!(left.kind, ExprKind::Identifier(ref name) if name == "x")
                && matches!(right.kind, ExprKind::None)
    ));
    // `is` sits at comparison precedence: a sum binds tighter on either side.
    assert!(matches!(
        parse_expr("a + 1 is b").kind,
        ExprKind::Infix(InfixOp::Is, ref left, _)
            if matches!(left.kind, ExprKind::Infix(InfixOp::Add, _, _))
    ));
}

#[test]
fn parses_empty_subscript_as_pointer_dereference_marker() {
    // `p[]` is the pointer-dereference subscript: the index child is the
    // dedicated marker, distinct from a source `p[None]` index expression.
    assert!(matches!(
        parse_expr("p[]").kind,
        ExprKind::Index { object, index }
            if matches!(object.kind, ExprKind::Identifier(ref name) if name == "p")
                && matches!(index.kind, ExprKind::EmptySubscript)
    ));
    // `p[None]` is not a dereference either: over a value base the bracket
    // is an ordinary subscript whose index is the `None` value (a
    // `Dict[Optional[T], _]` key), never the marker.
    assert!(matches!(
        parse_expr("p[None]").kind,
        ExprKind::Index { object, index }
            if matches!(object.kind, ExprKind::Identifier(ref name) if name == "p")
                && matches!(index.kind, ExprKind::None)
    ));
    // Over a type name the same bracket stays compile-time parameter
    // application.
    assert!(matches!(
        parse_expr("Wrapper[None]").kind,
        ExprKind::TypeApply { .. }
    ));
    // The marker composes as an ordinary suffix: deref of an offset call.
    assert!(matches!(
        parse_expr("p.unsafe_offset(1)[]").kind,
        ExprKind::Index { object, index }
            if matches!(object.kind, ExprKind::MethodCall { .. })
                && matches!(index.kind, ExprKind::EmptySubscript)
    ));
}

#[test]
fn distinguishes_origin_specialization_from_runtime_indexing() {
    let specialized = parse_expr("borrow[origin_of(value)]");
    match specialized.kind {
        ExprKind::TypeApply { name, args } => {
            assert_eq!(name, "borrow");
            assert!(matches!(
                args.as_slice(),
                [ParamArg::Value(Expr {
                    kind: ExprKind::Call { name, args, .. },
                    ..
                })] if name == "origin_of"
                    && matches!(args.as_slice(), [Expr { kind: ExprKind::Identifier(value), .. }] if value == "value")
            ));
        }
        other => panic!("expected an origin-specialized function value, got {other:?}"),
    }

    assert!(matches!(
        parse_expr("values[index]").kind,
        ExprKind::Index { index, .. }
            if matches!(index.kind, ExprKind::Identifier(ref name) if name == "index")
    ));

    let specialized = parse_expr("choose[origin_of(left), origin_of(right)]");
    assert!(matches!(
        specialized.kind,
        ExprKind::TypeApply { name, args }
            if name == "choose"
                && args.len() == 2
                && args.iter().all(|argument| matches!(
                    argument,
                    ParamArg::Value(Expr {
                        kind: ExprKind::Call { name, .. },
                        ..
                    }) if name == "origin_of"
                ))
    ));

    assert!(matches!(
        parse_expr("grid[row, column]").kind,
        ExprKind::MultiIndex { args, .. }
            if matches!(args.as_slice(), [
                mojito::ast::SubscriptArg::Index(Expr {
                    kind: ExprKind::Identifier(row),
                    ..
                }),
                mojito::ast::SubscriptArg::Index(Expr {
                    kind: ExprKind::Identifier(column),
                    ..
                }),
            ] if row == "row" && column == "column")
    ));
}

#[test]
fn parses_nested_type_argument() {
    // A parameterized type as a type argument: `Box[Pair[Int]]`.
    let stmts = parse("var b: Box[Pair[Int]] = q\n");
    match &stmts[0].kind {
        StmtKind::VarDecl { ty: Some(ty), .. } => {
            assert_eq!(
                *ty,
                Type::Named(
                    "Box".into(),
                    vec![ParamArg::Type(Type::Named(
                        "Pair".into(),
                        vec![ParamArg::Type(Type::Int)],
                    ))],
                )
            );
        }
        other => panic!("expected a var decl, got {:?}", other),
    }
}

// --- List literals ---

#[test]
fn parses_list_literal() {
    assert_eq!(
        parse_expr("[1, 2, 3]"),
        Expr::from(ExprKind::ListLit(vec![
            int_expr(1),
            int_expr(2),
            int_expr(3)
        ]))
    );
}

#[test]
fn parses_empty_list_literal() {
    assert_eq!(parse_expr("[]"), Expr::from(ExprKind::ListLit(vec![])));
}

// --- Membership: in / not in ---

#[test]
fn parses_in_and_not_in() {
    assert_eq!(
        parse_expr("x in xs"),
        Expr::from(ExprKind::Infix(InfixOp::In, ident("x"), ident("xs")))
    );
    assert_eq!(
        parse_expr("x not in xs"),
        Expr::from(ExprKind::Infix(InfixOp::NotIn, ident("x"), ident("xs")))
    );
}

#[test]
fn in_shares_comparison_precedence() {
    // `1 in xs and 2 in ys` == `(1 in xs) and (2 in ys)`
    assert_eq!(
        parse_expr("1 in xs and 2 in ys"),
        Expr::from(ExprKind::Infix(
            InfixOp::And,
            bx(ExprKind::Infix(InfixOp::In, int(1), ident("xs"))),
            bx(ExprKind::Infix(InfixOp::In, int(2), ident("ys"))),
        ))
    );
}

#[test]
fn rejects_not_without_in() {
    let mut parser = Parser::new(Lexer::new("var a: Bool = 1 not xs\n"));
    assert!(parser.parse_program().is_err());
}

// --- Member-write: mut self + place assignment ---

#[test]
fn parses_mut_self_method() {
    let stmts = parse(
        "@fieldwise_init\nstruct C:\n    var n: Int\n\n    def inc(mut self):\n        self.n = self.n + 1\n",
    );
    let StmtKind::Struct { methods, .. } = &stmts[0].kind else {
        panic!("expected a struct")
    };
    assert_eq!(
        methods[0].self_convention,
        Some(ArgConvention::Mut),
        "method should be mut self"
    );
}

#[test]
fn parses_field_and_nested_place_assignment() {
    // `p.x = e` → SetPlace with a Member place.
    match &parse("p.x = 1\n")[0].kind {
        StmtKind::SetPlace { place, .. } => {
            assert_eq!(
                *place,
                Expr::from(ExprKind::Member {
                    object: ident("p"),
                    field: "x".into()
                })
            );
        }
        other => panic!("expected SetPlace, got {:?}", other),
    }
    // `xs[i].y = e` is also a place.
    assert!(matches!(
        &parse("xs[0].y = 1\n")[0].kind,
        StmtKind::SetPlace { .. }
    ));
}

#[test]
fn rejects_non_place_assignment_target() {
    let mut parser = Parser::new(Lexer::new("f() = 1\n"));
    assert!(parser.parse_program().is_err());
}

// --- Tuple unpacking and declaration destructuring ---

#[test]
fn parses_tuple_unpacking() {
    assert_eq!(
        parse("x, y = point\n")[0],
        Stmt::from(StmtKind::Unpack {
            targets: vec![
                Expr::from(ExprKind::Identifier("x".into())),
                Expr::from(ExprKind::Identifier("y".into()))
            ],
            value: Expr::from(ExprKind::Identifier("point".into())),
            declares: false,
        })
    );
}

#[test]
fn parses_var_tuple_destructuring() {
    assert!(matches!(
        &parse("var left, right = pair\n")[0].kind,
        StmtKind::Unpack { targets, value, declares }
            if *declares
                && targets.len() == 2
                && matches!(&value.kind, ExprKind::Identifier(name) if name == "pair")
    ));
}

#[test]
fn tuple_unpacking_allows_place_targets() {
    // Each target obeys the assignment-target rule: a NAME or a place.
    assert!(matches!(
        &parse("p.x, xs[0] = t\n")[0].kind,
        StmtKind::Unpack { targets, .. }
            if matches!(targets[0].kind, ExprKind::Member { .. })
                && matches!(targets[1].kind, ExprKind::Index { .. })
    ));
}

#[test]
fn tuple_unpacking_allows_a_trailing_comma() {
    // `a, = t` is a one-target unpack (a trailing comma terminates the list).
    assert_eq!(
        parse("a, = t\n")[0],
        Stmt::from(StmtKind::Unpack {
            targets: vec![Expr::from(ExprKind::Identifier("a".into()))],
            value: Expr::from(ExprKind::Identifier("t".into())),
            declares: false,
        })
    );
}

#[test]
fn rejects_non_place_unpacking_target() {
    let mut parser = Parser::new(Lexer::new("a, f() = t\n"));
    assert!(parser.parse_program().is_err());
}

// --- The `parse` convenience helper (parse-only front end) ---

#[test]
fn parse_helper_matches_parser() {
    let src = "var x: Int = 1\n";
    assert_eq!(mojito::parse(src).unwrap(), parse(src));
}

#[test]
fn parse_helper_surfaces_errors() {
    assert!(mojito::parse("var x: Int =\n").is_err());
}

// --- Augmented assignment ---

#[test]
fn parses_augmented_assignment() {
    assert_eq!(
        parse("x += 1\n")[0],
        Stmt::from(StmtKind::AugAssign {
            place: Expr::from(ExprKind::Identifier("x".into())),
            op: InfixOp::Add,
            value: int_expr(1)
        })
    );
    // A place target is allowed too.
    assert!(matches!(
        &parse("xs[0] *= 2\n")[0].kind,
        StmtKind::AugAssign {
            op: InfixOp::Mul,
            ..
        }
    ));
    assert!(matches!(
        &parse("grid[0, 1] += 2\n")[0].kind,
        StmtKind::AugAssign {
            place: Expr {
                kind: ExprKind::MultiIndex { .. },
                ..
            },
            ..
        }
    ));
    assert!(matches!(
        &parse("window[1:4] += 2\n")[0].kind,
        StmtKind::AugAssign {
            place: Expr {
                kind: ExprKind::Slice { .. },
                ..
            },
            ..
        }
    ));
    for (source, expected) in [
        ("x &= 1\n", InfixOp::BitAnd),
        ("x |= 2\n", InfixOp::BitOr),
        ("x ^= 3\n", InfixOp::BitXor),
    ] {
        assert!(matches!(
            &parse(source)[0].kind,
            StmtKind::AugAssign { op, .. } if *op == expected
        ));
    }
}

#[test]
fn rejects_augmented_assignment_to_non_place() {
    let mut parser = Parser::new(Lexer::new("f() += 1\n"));
    assert!(parser.parse_program().is_err());
}

// --- Walrus / named expression ---

#[test]
fn parses_walrus_as_named_expression() {
    assert_eq!(
        parse_expr("(n := 5)"),
        Expr::from(ExprKind::Named {
            name: "n".into(),
            value: int(5)
        })
    );
}

#[test]
fn walrus_binds_looser_than_comparison() {
    // `(n := a > b)` == `n := (a > b)`
    assert_eq!(
        parse_expr("(n := a > b)"),
        Expr::from(ExprKind::Named {
            name: "n".into(),
            value: bx(ExprKind::Infix(InfixOp::Gt, ident("a"), ident("b"))),
        })
    );
}

#[test]
fn rejects_walrus_with_non_name_target() {
    let mut parser = Parser::new(Lexer::new("var y: Int = (1 := 5)\n"));
    assert!(parser.parse_program().is_err());
}

// --- Inferred `var` (no annotation) ---

#[test]
fn parses_inferred_var_decl() {
    assert_eq!(
        parse("var x = 1 + 2")[0],
        Stmt::from(StmtKind::VarDecl {
            name: "x".into(),
            ty: None,
            value: Expr::from(ExprKind::Infix(InfixOp::Add, int(1), int(2))),
        })
    );
}

#[test]
fn annotated_var_still_parses_with_some_ty() {
    match &parse("var x: Int = 5")[0].kind {
        StmtKind::VarDecl {
            ty: Some(Type::Int),
            ..
        } => {}
        other => panic!("expected an annotated var decl, got {:?}", other),
    }
}

// --- Tuple literals ---

#[test]
fn parses_tuple_literals_and_grouping() {
    // A comma makes a tuple; a bare `(e)` is grouping (not a 1-tuple).
    assert_eq!(
        parse_expr("(1, 2, 3)"),
        Expr::from(ExprKind::TupleLit(vec![
            int_expr(1),
            int_expr(2),
            int_expr(3)
        ]))
    );
    assert_eq!(
        parse_expr("(1 + 2)"),
        Expr::from(ExprKind::Infix(InfixOp::Add, int(1), int(2)))
    );
    assert_eq!(parse_expr("()"), Expr::from(ExprKind::TupleLit(vec![])));
    // Trailing comma: `(a,)` is a 1-tuple.
    assert_eq!(
        parse_expr("(7,)"),
        Expr::from(ExprKind::TupleLit(vec![int_expr(7)]))
    );
}

#[test]
fn parses_bare_comma_tuple_displays() {
    assert_eq!(
        parse_expr("1, \"one\"\n"),
        Expr::from(ExprKind::TupleLit(vec![
            int_expr(1),
            Expr::from(ExprKind::Str("one".into())),
        ]))
    );
    assert!(matches!(
        &parse("var pair = 2, 3\n")[0].kind,
        StmtKind::VarDecl {
            value: Expr {
                kind: ExprKind::TupleLit(elements),
                ..
            },
            ..
        } if elements.len() == 2
    ));
    assert!(matches!(
        &parse("def pair() -> Tuple[Int, Int]:\n    return 4, 5\n")[0].kind,
        StmtKind::Def { body, .. }
            if matches!(&body[0].kind, StmtKind::Return(Some(Expr { kind: ExprKind::TupleLit(elements), .. })) if elements.len() == 2)
    ));
}

// --- Function-argument forms (parsed; semantics deferred) ---

/// Extract a `def`'s params + marker positions from a one-def program.
fn def_params(src: &str) -> (Vec<FnParam>, Option<usize>, Option<usize>) {
    match parse(src).into_iter().next().unwrap().kind {
        StmtKind::Def {
            params,
            positional_only,
            keyword_only,
            ..
        } => (params, positional_only, keyword_only),
        other => panic!("expected a def, got {:?}", other),
    }
}

#[test]
fn parses_default_argument_value() {
    let (params, _, _) =
        def_params("def my_pow(base: Int, exp: Int = 2) -> Int:\n    return base\n");
    assert_eq!(params[0].default, None);
    assert_eq!(params[1].default, Some(int_expr(2)));
    assert_eq!(params[1].kind, ParamKind::Regular);
}

#[test]
fn parses_variadic_and_kw_variadic() {
    let (p, _, _) = def_params("def sum(*values: Int) -> Int:\n    return 0\n");
    assert_eq!(p[0].kind, ParamKind::Variadic);
    assert_eq!(p[0].name, "values");
    let (p, _, _) = def_params("def opts(var **kw: Int):\n    pass\n");
    assert_eq!(p[0].kind, ParamKind::KwVariadic);
    assert_eq!(p[0].convention, Some(ArgConvention::Var));
    let error = mojito::parse("def opts(**kw: Int):\n    pass\n").expect_err("bare ** must reject");
    assert!(format!("{error}").contains("var **name: Type"));
}

#[test]
fn parses_generic_method_keyword_collectors_and_forwarding() {
    let parsed = parse(
        "struct Relay:\n    def target[T: Copyable & Movable](self, var **options: T):\n        pass\n    def forward(self, var **options: Int):\n        self.target(**options^)\n",
    );
    let StmtKind::Struct { methods, .. } = &parsed[0].kind else {
        panic!("expected struct declaration")
    };
    assert_eq!(methods[0].type_params[0].name, "T");
    assert_eq!(methods[0].params[0].kind, ParamKind::KwVariadic);
    assert_eq!(methods[1].params[0].kind, ParamKind::KwVariadic);
    assert!(matches!(
        &methods[1].body[0].kind,
        StmtKind::Expr(Expr {
            kind: ExprKind::MethodCall { kwargs, .. },
            ..
        }) if kwargs.len() == 1 && kwargs[0].is_forwarded()
    ));
}

#[test]
fn retains_variadic_type_pack_declarations_and_uses() {
    let parsed =
        parse("def count[*ArgTypes: AnyType](*args: *ArgTypes) -> Int:\n    return len(args)\n");
    let StmtKind::Def {
        type_params,
        params,
        ..
    } = &parsed[0].kind
    else {
        panic!("expected function declaration")
    };
    assert_eq!(type_params[0].name, "*ArgTypes");
    assert_eq!(params[0].ty, Type::Named("*ArgTypes".into(), Vec::new()));
}

#[test]
fn parses_positional_only_and_keyword_only_markers() {
    let (p, slash, star) = def_params("def mn(a: Int, b: Int, /) -> Int:\n    return a\n");
    assert_eq!(p.len(), 2);
    assert_eq!(slash, Some(2));
    assert_eq!(star, None);
    let (p, slash, star) = def_params("def kw(a: Int, *, d: Bool) -> Int:\n    return a\n");
    assert_eq!(p.len(), 2); // a and d; the bare `*` is a marker, not a param
    assert_eq!(slash, None);
    assert_eq!(star, Some(1));
}

#[test]
fn parses_argument_conventions() {
    let (p, _, _) =
        def_params("def f(mut x: Int, var y: String, out z: Bool, imm w: Int):\n    pass\n");
    assert_eq!(p[0].convention, Some(ArgConvention::Mut));
    assert_eq!(p[1].convention, Some(ArgConvention::Var));
    assert_eq!(p[2].convention, Some(ArgConvention::Out));
    assert_eq!(p[3].convention, Some(ArgConvention::Imm));
}

#[test]
fn convention_word_stays_usable_as_a_param_name() {
    // `read` followed by `:` is the parameter name, not a convention.
    let (p, _, _) = def_params("def f(read: Int, mut: Bool):\n    pass\n");
    assert_eq!(p[0].name, "read");
    assert_eq!(p[0].convention, None);
    assert_eq!(p[1].name, "mut");
    assert_eq!(p[1].convention, None);
    // `ref` too — contextual, still a usable name when followed by `:`.
    let (p, _, _) = def_params("def g(ref: Int):\n    pass\n");
    assert_eq!(p[0].name, "ref");
    assert_eq!(p[0].convention, None);
}

#[test]
fn rejects_removed_read_convention_with_migration_diagnostic() {
    // Upstream made `read` a hard error (2026-08): `'read' was removed; use
    // 'imm'`. Every convention position rejects with the targeted message;
    // `read` as a parameter NAME (`read: Int`) stays usable above.
    for source in [
        "def f(read b: Int) -> Int:\n    return b\n",
        "struct S:\n    def m(read self):\n        pass\n",
        "def outer():\n    var a = 1\n    def inner() {read a}:\n        pass\n",
        "def f(cb: def(read Int) -> Int):\n    pass\n",
    ] {
        let error = mojito::parse(source).unwrap_err().to_string();
        assert!(
            error.contains("'read' was removed; use 'imm'"),
            "missing migration diagnostic for {source:?}: {error}"
        );
    }
}

#[test]
fn parses_ref_convention_with_optional_origin() {
    // `ref x` and `ref[origin] x` both give the Ref convention; the origin
    // specifier (an expression, or `_`) is retained.
    let (p, _, _) = def_params("def f(ref a: Int, ref[b] c: Int, ref[_] d: Int):\n    pass\n");
    assert_eq!(p[0].convention, Some(ArgConvention::Ref));
    assert_eq!(p[0].name, "a");
    assert_eq!(p[1].convention, Some(ArgConvention::Ref));
    assert_eq!(p[1].name, "c");
    assert!(matches!(
        p[1].origin.as_deref(),
        Some([Expr { kind: ExprKind::Identifier(name), .. }]) if name == "b"
    ));
    assert_eq!(p[2].convention, Some(ArgConvention::Ref));
    assert_eq!(p[2].name, "d");
}

#[test]
fn parses_origin_unions_parameters_and_reference_bindings() {
    let source = "def pick[is_mutable: Bool, //, origin: Origin[mut=is_mutable]](ref[origin] a: String, ref[a, origin_of(a)] b: String) -> ref[a, b] String:\n    ref selected = a\n    return selected\n";
    let stmts = parse(source);
    let StmtKind::Def {
        type_params,
        params,
        ret,
        body,
        ..
    } = &stmts[0].kind
    else {
        panic!("expected a def");
    };
    assert_eq!(type_params[0].name, "is_mutable");
    assert_eq!(type_params[1].name, "origin");
    assert_eq!(type_params[1].bounds, vec!["Origin"]);
    assert!(type_params[0].infer_only);
    assert!(!type_params[1].infer_only);
    assert!(type_params[1].origin_mutability.is_some());
    assert_eq!(params[0].convention, Some(ArgConvention::Ref));
    assert_eq!(params[1].convention, Some(ArgConvention::Ref));
    assert!(matches!(
        ret,
        Some(Type::Ref { referent, origin: Some(origins) })
            if **referent == Type::Named("String".into(), vec![]) && origins.len() == 2
    ));
    assert!(matches!(
        &body[0].kind,
        StmtKind::RefDecl { name, .. } if name == "selected"
    ));
}

#[test]
fn parses_ref_self_receiver() {
    // `ref self` (with an optional discarded origin) is recognized as a receiver.
    let stmts = parse(
        "struct S:\n    def get(ref self) -> Int:\n        return 0\n    def peek(ref[o] self) -> Int:\n        return 0\n",
    );
    match &stmts[0].kind {
        StmtKind::Struct { methods, .. } => {
            assert_eq!(methods[0].self_convention, Some(ArgConvention::Ref));
            assert!(methods[0].has_self);
            assert_eq!(methods[1].self_convention, Some(ArgConvention::Ref));
            assert_eq!(methods[1].self_origin.as_ref().map(Vec::len), Some(1));
        }
        other => panic!("expected a struct, got {:?}", other),
    }
}

#[test]
fn parses_ref_return_type() {
    // `-> ref[origin] T` retains both the referent and origin expression.
    let stmts = parse("def f(x: Int) -> ref[origin_of(x)] Int:\n    return x\n");
    match &stmts[0].kind {
        StmtKind::Def { ret, .. } => {
            assert!(matches!(
                ret,
                Some(Type::Ref { referent, origin: Some(origins) })
                    if **referent == Type::Int && origins.len() == 1
            ));
        }
        other => panic!("expected a def, got {:?}", other),
    }
}

#[test]
fn parses_keyword_call_arguments() {
    assert_eq!(
        parse_expr("f(a=1, b=2)"),
        Expr::from(ExprKind::Call {
            name: "f".into(),
            param_args: vec![],
            args: vec![],
            kwargs: vec![
                KwArg {
                    name: "a".into(),
                    value: int_expr(1)
                },
                KwArg {
                    name: "b".into(),
                    value: int_expr(2)
                },
            ],
        })
    );
    assert_eq!(
        parse_expr("f(a: 1, b: 2)"),
        Expr::from(ExprKind::Call {
            name: "f".into(),
            param_args: vec![],
            args: vec![],
            kwargs: vec![
                KwArg {
                    name: "a".into(),
                    value: int_expr(1)
                },
                KwArg {
                    name: "b".into(),
                    value: int_expr(2)
                },
            ],
        })
    );
    // Mixed: positional then keyword.
    assert_eq!(
        parse_expr("f(1, b=2)"),
        Expr::from(ExprKind::Call {
            name: "f".into(),
            param_args: vec![],
            args: vec![int_expr(1)],
            kwargs: vec![KwArg {
                name: "b".into(),
                value: int_expr(2)
            }],
        })
    );
}

#[test]
fn rejects_positional_after_keyword_argument() {
    let mut parser = Parser::new(Lexer::new("f(a=1, 2)\n"));
    assert!(parser.parse_program().is_err());
}

#[test]
fn parses_transferred_keyword_dictionary_forwarding() {
    assert_eq!(
        parse_expr("f(prefix=1, **kwargs^)"),
        Expr::from(ExprKind::Call {
            name: "f".into(),
            param_args: vec![],
            args: vec![],
            kwargs: vec![
                KwArg {
                    name: "prefix".into(),
                    value: int_expr(1),
                },
                KwArg {
                    name: mojito::ast::FORWARDED_KWARGS_NAME.into(),
                    value: Expr::from(ExprKind::Transfer(ident("kwargs"))),
                },
            ],
        })
    );

    let mut parser = Parser::new(Lexer::new("f(**kwargs)\n"));
    assert!(parser.parse_program().is_err());
}

// --- Expressions: ternary, chained comparison, slices (parsed; semantics deferred) ---

#[test]
fn parses_conditional_expression() {
    assert_eq!(
        parse_expr("a if c else b"),
        Expr::from(ExprKind::IfExpr {
            cond: ident("c"),
            then_branch: ident("a"),
            else_branch: ident("b"),
        })
    );
}

#[test]
fn conditional_expression_nests_right() {
    // a if p else b if q else c  ==  a if p else (b if q else c)
    assert_eq!(
        parse_expr("a if p else b if q else c"),
        Expr::from(ExprKind::IfExpr {
            cond: ident("p"),
            then_branch: ident("a"),
            else_branch: bx(ExprKind::IfExpr {
                cond: ident("q"),
                then_branch: ident("b"),
                else_branch: ident("c"),
            }),
        })
    );
}

#[test]
fn parses_chained_comparison() {
    // 0 <= i < n  becomes a Compare node with two links.
    assert_eq!(
        parse_expr("0 <= i < n"),
        Expr::from(ExprKind::Compare {
            first: int(0),
            rest: vec![
                (InfixOp::Le, Expr::from(ExprKind::Identifier("i".into()))),
                (InfixOp::Lt, Expr::from(ExprKind::Identifier("n".into()))),
            ],
        })
    );
}

#[test]
fn single_comparison_stays_infix() {
    // A lone comparison is unchanged (not a Compare node).
    assert_eq!(
        parse_expr("a < b"),
        Expr::from(ExprKind::Infix(InfixOp::Lt, ident("a"), ident("b")))
    );
    assert_eq!(
        parse_expr("a not in b"),
        Expr::from(ExprKind::Infix(InfixOp::NotIn, ident("a"), ident("b")))
    );
}

#[test]
fn parses_keyword_slice_subscripts() {
    // A named bracket argument whose value is a slice is a keyword slice
    // (a MultiIndex with one KeywordSlice argument), preserving omitted
    // bounds and the explicit-stride marker.
    assert_eq!(
        parse_expr("s[byte=1:3]"),
        Expr::from(ExprKind::MultiIndex {
            object: ident("s"),
            args: vec![mojito::ast::SubscriptArg::KeywordSlice {
                name: "byte".to_string(),
                lower: Some(int(1)),
                upper: Some(int(3)),
                step: None,
                explicit_step: false,
            }],
        })
    );
    assert_eq!(
        parse_expr("s[byte=:3]"),
        Expr::from(ExprKind::MultiIndex {
            object: ident("s"),
            args: vec![mojito::ast::SubscriptArg::KeywordSlice {
                name: "byte".to_string(),
                lower: None,
                upper: Some(int(3)),
                step: None,
                explicit_step: false,
            }],
        })
    );
    assert_eq!(
        parse_expr("s[byte=::2]"),
        Expr::from(ExprKind::MultiIndex {
            object: ident("s"),
            args: vec![mojito::ast::SubscriptArg::KeywordSlice {
                name: "byte".to_string(),
                lower: None,
                upper: None,
                step: Some(int(2)),
                explicit_step: true,
            }],
        })
    );
    // Mixed with a positional index the subscript stays a MultiIndex in
    // source order.
    assert_eq!(
        parse_expr("s[0, byte=1:]"),
        Expr::from(ExprKind::MultiIndex {
            object: ident("s"),
            args: vec![
                mojito::ast::SubscriptArg::Index(*int(0)),
                mojito::ast::SubscriptArg::KeywordSlice {
                    name: "byte".to_string(),
                    lower: Some(int(1)),
                    upper: None,
                    step: None,
                    explicit_step: false,
                },
            ],
        })
    );
}

#[test]
fn parses_slice_subscripts() {
    assert_eq!(
        parse_expr("xs[1:2]"),
        Expr::from(ExprKind::Slice {
            object: ident("xs"),
            lower: Some(int(1)),
            upper: Some(int(2)),
            step: None,
            explicit_step: false,
        })
    );
    assert_eq!(
        parse_expr("xs[::2]"),
        Expr::from(ExprKind::Slice {
            object: ident("xs"),
            lower: None,
            upper: None,
            step: Some(int(2)),
            explicit_step: true,
        })
    );
    assert_eq!(
        parse_expr("xs[i:]"),
        Expr::from(ExprKind::Slice {
            object: ident("xs"),
            lower: Some(bx(ExprKind::Identifier("i".into()))),
            upper: None,
            step: None,
            explicit_step: false,
        })
    );

    assert_eq!(
        parse_expr("grid[row, 1:5:2]"),
        Expr::from(ExprKind::MultiIndex {
            object: ident("grid"),
            args: vec![
                mojito::ast::SubscriptArg::Index(Expr::from(ExprKind::Identifier("row".into(),))),
                mojito::ast::SubscriptArg::Slice {
                    lower: Some(Box::new(int_expr(1))),
                    upper: Some(Box::new(int_expr(5))),
                    step: Some(Box::new(int_expr(2))),
                    explicit_step: true,
                },
            ],
        })
    );
}

#[test]
fn plain_index_is_not_a_slice() {
    assert_eq!(
        parse_expr("xs[i]"),
        Expr::from(ExprKind::Index {
            object: ident("xs"),
            index: ident("i")
        })
    );
}

// --- Decorators (general grammar) + dunder / receiver conventions ---

#[test]
fn parses_general_decorators_on_def() {
    let stmts = parse("@staticmethod\n@a.b(1, k=2)\ndef f(x: Int) -> Int:\n    return x\n");
    let StmtKind::Def { decorators, .. } = &stmts[0].kind else {
        panic!("expected a def")
    };
    assert_eq!(decorators.len(), 2);
    assert_eq!(decorators[0].path, vec!["staticmethod".to_string()]);
    assert_eq!(decorators[1].path, vec!["a".to_string(), "b".to_string()]);
    assert_eq!(decorators[1].args, vec![int_expr(1)]);
    assert_eq!(
        decorators[1].kwargs,
        vec![KwArg {
            name: "k".into(),
            value: int_expr(2)
        }]
    );
}

#[test]
fn parses_decorator_on_struct_and_keeps_fieldwise_init() {
    let stmts = parse("@value\n@fieldwise_init\nstruct P:\n    var x: Int\n");
    let StmtKind::Struct {
        decorators,
        fieldwise_init,
        ..
    } = &stmts[0].kind
    else {
        panic!("expected a struct")
    };
    assert_eq!(decorators.len(), 2);
    assert!(
        *fieldwise_init,
        "@fieldwise_init should still be recognized"
    );
}

#[test]
fn parses_receiver_conventions_and_static_methods() {
    let stmts = parse(
        "struct S:\n    var n: Int\n    def a(mut self):\n        pass\n    def b(out self):\n        pass\n    @staticmethod\n    def c(x: Int) -> Int:\n        return x\n",
    );
    let StmtKind::Struct { methods, .. } = &stmts[0].kind else {
        panic!("expected a struct")
    };
    assert_eq!(methods[0].self_convention, Some(ArgConvention::Mut));
    assert!(methods[0].has_self);
    assert_eq!(methods[1].self_convention, Some(ArgConvention::Out));
    assert!(methods[1].has_self);
    assert!(!methods[2].has_self, "@staticmethod has no self");
    assert_eq!(methods[2].decorators.len(), 1);
}

#[test]
fn parses_dunder_method_names() {
    let stmts = parse(
        "@fieldwise_init\nstruct V:\n    var x: Int\n    def __eq__(self, o: V) -> Bool:\n        return self.x == o.x\n",
    );
    let StmtKind::Struct { methods, .. } = &stmts[0].kind else {
        panic!("expected a struct")
    };
    assert_eq!(methods[0].name, "__eq__");
}

// --- Function/closure type annotations (parsed; semantics deferred) ---

/// Extract the annotated type from `var NAME: TYPE = expr`.
fn var_anno_type(src: &str) -> Type {
    match parse(src).into_iter().next().unwrap().kind {
        StmtKind::VarDecl { ty: Some(ty), .. } => ty,
        other => panic!("expected an annotated var decl, got {:?}", other),
    }
}

fn function_type_param(ty: Type) -> FunctionTypeParam {
    FunctionTypeParam {
        name: None,
        kind: ParamKind::Regular,
        convention: None,
        origin: None,
        ty,
    }
}

#[test]
fn parses_leading_dot_contextual_members() {
    // `.red()` is a MethodCall over the compiler-internal `$contextual`
    // sentinel; `.red` a Member; postfix chains attach normally.
    let exprs = parse("var c = .red()\nvar m = .red\nvar b = .red().brighten(2)\n");
    let object_name = |expr: &Expr| -> String {
        match &expr.kind {
            ExprKind::MethodCall { object, .. } | ExprKind::Member { object, .. } => {
                match &object.kind {
                    ExprKind::Identifier(name) => name.clone(),
                    other => panic!("unexpected object {other:?}"),
                }
            }
            other => panic!("unexpected expression {other:?}"),
        }
    };
    let value = |stmt: &Stmt| -> Expr {
        match &stmt.kind {
            StmtKind::VarDecl { value, .. } => value.clone(),
            other => panic!("unexpected statement {other:?}"),
        }
    };
    assert_eq!(object_name(&value(&exprs[0])), "$contextual");
    assert_eq!(object_name(&value(&exprs[1])), "$contextual");
    // The chained call's outer object is the inner `.red()` MethodCall.
    let outer = value(&exprs[2]);
    let ExprKind::MethodCall { object, method, .. } = &outer.kind else {
        panic!("expected a chained method call");
    };
    assert_eq!(method, "brighten");
    assert_eq!(object_name(object), "$contextual");
}

#[test]
fn parses_function_type_where_clauses_bind_innermost() {
    // Trailing `where` clauses attach to the function type itself.
    let ty = var_anno_type("var f: def[w: Int](Int) thin -> None where (w > 0, \"m\") = g\n");
    let Type::Func { where_clauses, .. } = ty else {
        panic!("expected a function type");
    };
    assert_eq!(where_clauses.len(), 1);
    // A function-type RETURN type greedily consumes a following `where`
    // (upstream's innermost-binding rule).
    let ty = var_anno_type("var f: def() thin -> def(Int) thin -> None where (True, \"m\") = g\n");
    let Type::Func {
        where_clauses, ret, ..
    } = ty
    else {
        panic!("expected a function type");
    };
    assert!(where_clauses.is_empty());
    let Type::Func {
        where_clauses: inner,
        ..
    } = *ret
    else {
        panic!("expected a function-type result");
    };
    assert_eq!(inner.len(), 1);
}

#[test]
fn parses_function_type_annotations() {
    assert_eq!(
        var_anno_type("var f: def(Int) -> Int = g\n"),
        Type::Func {
            type_params: vec![],
            params: vec![function_type_param(Type::Int)],
            ret: Box::new(Type::Int),
            thin: false,
            capturing: None,
            raises: false,
            raises_type: None,
            where_clauses: vec![],
        }
    );
    // `thin` (non-capturing) after the parameter list, multiple params.
    assert_eq!(
        var_anno_type("var h: def(Int, Bool) thin -> String = k\n"),
        Type::Func {
            type_params: vec![],
            params: vec![
                function_type_param(Type::Int),
                function_type_param(Type::Bool),
            ],
            ret: Box::new(Type::Named("String".into(), vec![])),
            thin: true,
            capturing: None,
            raises: false,
            raises_type: None,
            where_clauses: vec![],
        }
    );
    // No params + `raises` effect.
    assert_eq!(
        var_anno_type("var n: def() raises -> None = m\n"),
        Type::Func {
            type_params: vec![],
            params: vec![],
            ret: Box::new(Type::None),
            thin: false,
            capturing: None,
            raises: true,
            raises_type: None,
            where_clauses: vec![],
        }
    );
    // Current Mojo permits `-> None` to be omitted from callable types.
    assert_eq!(
        var_anno_type("var sink: def(Int) thin = callback\n"),
        Type::Func {
            type_params: vec![],
            params: vec![function_type_param(Type::Int)],
            ret: Box::new(Type::None),
            thin: true,
            capturing: None,
            raises: false,
            raises_type: None,
            where_clauses: vec![],
        }
    );
}

#[test]
fn function_types_retain_the_var_keyword_variadic_role() {
    let ty = var_anno_type("var callback: def(var **options: Int) -> Int = target\n");
    let Type::Func { params, .. } = ty else {
        panic!("expected a function type");
    };
    assert!(matches!(
        params.as_slice(),
        [FunctionTypeParam {
            name: Some(name),
            kind: ParamKind::KwVariadic,
            convention: Some(ArgConvention::Var),
            ty: Type::Int,
            ..
        }] if name == "options"
    ));

    let error = mojito::parse("var callback: def(**options: Int) -> Int = target\n")
        .expect_err("bare function-type ** must reject");
    assert!(format!("{error}").contains("var **name: Type"));
}

#[test]
fn parses_parameterized_capturing_function_type_values() {
    let stmt = parse("comptime Callback = def[width: Int](Int) capturing[_] -> None\n")
        .into_iter()
        .next()
        .unwrap();
    let StmtKind::Comptime {
        value:
            Expr {
                kind:
                    ExprKind::TypeValue(Type::Func {
                        type_params,
                        capturing,
                        ..
                    }),
                ..
            },
        ..
    } = stmt.kind
    else {
        panic!("expected a parameterized function type value");
    };
    assert_eq!(type_params.len(), 1);
    assert!(matches!(
        capturing.as_deref(),
        Some([Expr {
            kind: ExprKind::Identifier(origin),
            ..
        }]) if origin == "_"
    ));
}

#[test]
fn retains_callable_parameter_bounds_and_capture_effects() {
    let statements = parse(
        "def invoke[F: def(Int) -> Int, callback: def(Int) capturing[origins], bare: def(Int) capturing, sink: def(Int) thin](value: Int):\n    pass\n",
    );
    let StmtKind::Def { type_params, .. } = &statements[0].kind else {
        panic!("expected a def");
    };
    assert_eq!(type_params.len(), 4);
    assert_eq!(type_params[0].bounds, vec!["<function type>"]);
    assert!(matches!(
        &type_params[0].callable_bound,
        Some(Type::Func {
            ret,
            thin: false,
            capturing: None,
            ..
        }) if **ret == Type::Int
    ));
    assert!(matches!(
        &type_params[1].callable_bound,
        Some(Type::Func {
            ret,
            capturing: Some(origins),
            ..
        }) if **ret == Type::None
            && matches!(origins.as_slice(), [Expr {
                kind: ExprKind::Identifier(origin),
                ..
            }] if origin == "origins")
    ));
    assert!(matches!(
        &type_params[2].callable_bound,
        Some(Type::Func {
            capturing: Some(origins),
            ..
        }) if origins.is_empty()
    ));
    assert!(matches!(
        &type_params[3].callable_bound,
        Some(Type::Func {
            ret,
            thin: true,
            capturing: None,
            ..
        }) if **ret == Type::None
    ));
}

#[test]
fn infer_only_marker_applies_to_the_parameter_prefix() {
    let statements =
        parse("def select[first: Int, second: Bool, //, explicit: Int](value: Int):\n    pass\n");
    let StmtKind::Def { type_params, .. } = &statements[0].kind else {
        panic!("expected a def");
    };
    assert!(type_params[0].infer_only);
    assert!(type_params[1].infer_only);
    assert!(!type_params[2].infer_only);
}

#[test]
fn parses_parenthesized_from_imports() {
    assert_eq!(
        parse("from .backend import (\n    tile,\n    vectorize as vec,\n)\n"),
        vec![Stmt::from(StmtKind::FromImport {
            level: 1,
            path: vec!["backend".into()],
            names: ImportNames::Names(vec![iname("tile", None), iname("vectorize", Some("vec")),]),
        })]
    );
}

#[test]
fn diagnostic_parse_recovers_at_statement_boundaries() {
    let report = parse_diagnostics(
        "var first: = 1\nvar ok = 2\nvar second: = 3\nvar third: = 4\n",
        20,
    );
    assert!(report.errors.len() >= 3, "{report:#?}");
    assert!(!report.truncated);
}

#[test]
fn strict_parse_remains_fail_fast() {
    let source = "var first: = 1\nvar second: = 2\n";
    assert!(mojito::parse(source).is_err());
}

#[test]
fn function_type_return_nests() {
    // `def(Int) -> def(Int) -> Int` — the return type is itself a function type.
    assert_eq!(
        var_anno_type("var c: def(Int) -> def(Int) -> Int = mk\n"),
        Type::Func {
            type_params: vec![],
            params: vec![function_type_param(Type::Int)],
            ret: Box::new(Type::Func {
                type_params: vec![],
                params: vec![function_type_param(Type::Int)],
                ret: Box::new(Type::Int),
                thin: false,
                capturing: None,
                raises: false,
                raises_type: None,
                where_clauses: vec![],
            }),
            thin: false,
            capturing: None,
            raises: false,
            raises_type: None,
            where_clauses: vec![],
        }
    );
}

#[test]
fn parses_function_typed_parameter() {
    // A function-typed parameter (with `thin`) parses.
    let stmts = parse("def apply(cb: def(Int) thin -> Int, x: Int) -> Int:\n    return x\n");
    let StmtKind::Def { params, .. } = &stmts[0].kind else {
        panic!("expected a def")
    };
    assert_eq!(
        params[0].ty,
        Type::Func {
            type_params: vec![],
            params: vec![function_type_param(Type::Int)],
            ret: Box::new(Type::Int),
            thin: true,
            capturing: None,
            raises: false,
            raises_type: None,
            where_clauses: vec![],
        }
    );
}

#[test]
fn parses_tstring_interpolations_into_subexprs() {
    assert_eq!(
        parse_expr("t\"n={n}, x={a + b}\""),
        Expr::from(ExprKind::TString {
            parts: vec![
                TStringPart::Literal("n=".into()),
                TStringPart::Expr(ident("n")),
                TStringPart::Literal(", x=".into()),
                TStringPart::Expr(bx(ExprKind::Infix(InfixOp::Add, ident("a"), ident("b")))),
            ],
            raw: false,
        })
    );
    // A raw t-string sets `raw`.
    assert_eq!(
        parse_expr("rt\"v={x}\""),
        Expr::from(ExprKind::TString {
            parts: vec![
                TStringPart::Literal("v=".into()),
                TStringPart::Expr(ident("x"))
            ],
            raw: true,
        })
    );
}

#[test]
fn concatenates_the_full_adjacent_string_family() {
    assert_eq!(
        parse_expr("'a' \"\"\"b\"\"\" r'c'"),
        Expr::from(ExprKind::Str("abc".into()))
    );
    assert_eq!(
        parse_expr("t\"answer: \" t\"{value}\" t\"!\""),
        Expr::from(ExprKind::TString {
            parts: vec![
                TStringPart::Literal("answer: ".into()),
                TStringPart::Expr(ident("value")),
                TStringPart::Literal("!".into()),
            ],
            raw: false,
        })
    );
    assert_eq!(
        parse_expr("(\"a\"\n \"b\")"),
        Expr::from(ExprKind::Str("ab".into()))
    );

    let statements = parse("var text = t\"a\"\n    t\"{value}\"\n");
    let StmtKind::VarDecl { value, .. } = &statements[0].kind else {
        panic!("expected a variable declaration");
    };
    assert_eq!(
        value,
        &Expr::from(ExprKind::TString {
            parts: vec![
                TStringPart::Literal("a".into()),
                TStringPart::Expr(ident("value")),
            ],
            raw: false,
        })
    );
}

#[test]
fn expr_and_stmt_nodes_carry_real_source_spans() {
    // Both `Expr` and `Stmt` are stamped with byte ranges that slice their exact
    // source text (spans are metadata — equality above ignores them, so this is
    // the only place they're asserted).
    let src = "var total: Int = 40 + 2\n";
    let stmts = parse(src);
    // The statement spans the whole `var ... = 40 + 2`.
    assert_eq!(
        &src[stmts[0].span.0..stmts[0].span.1],
        "var total: Int = 40 + 2"
    );
    // Its initializer expression spans just `40 + 2`.
    let StmtKind::VarDecl { value, .. } = &stmts[0].kind else {
        panic!("expected a var decl")
    };
    assert_eq!(&src[value.span.0..value.span.1], "40 + 2");

    // A second statement's span starts after the first (not at 0 / DUMMY_SPAN).
    let two = parse("pass\nreturn x\n");
    assert_eq!(
        &"pass\nreturn x\n"[two[1].span.0..two[1].span.1],
        "return x"
    );
    assert_ne!(two[1].span, (0, 0));
}

#[test]
fn keyword_subscripts_parse_on_value_bases_and_type_applications_stay() {
    // `s[byte=i]` over a lowercase (value) base is a keyword subscript; a
    // named bracket over a capitalized type name stays compile-time
    // parameter application.
    let program = mojito::parse("x = buf[byte=1]\n").expect("parse");
    let StmtKind::Assign { value, .. } = &program[0].kind else {
        panic!("assign");
    };
    let ExprKind::MultiIndex { args, .. } = &value.kind else {
        panic!("expected keyword subscript, got {:?}", value.kind);
    };
    assert!(matches!(
        args.as_slice(),
        [mojito::ast::SubscriptArg::Keyword { name, .. }] if name == "byte"
    ));

    let program = mojito::parse("x = Origin[mut=True]\n").expect("parse");
    let StmtKind::Assign { value, .. } = &program[0].kind else {
        panic!("assign");
    };
    assert!(
        matches!(&value.kind, ExprKind::TypeApply { name, .. } if name == "Origin"),
        "named bracket over a type name stays TypeApply: {:?}",
        value.kind
    );
}

#[test]
fn normalizes_deprecated_lifecycle_spellings_at_parse_time() {
    // `ImplicitlyDeletable` and `__del__` are upstream-deprecated compat
    // spellings of `Deinitable`/`__deinit__`; the parser normalizes them so
    // every later phase sees one canonical vocabulary.
    let stmts = parse(
        "struct Res(Movable, ImplicitlyDeletable where False):\n    var id: Int\n    def __del__(deinit self):\n        pass\n",
    );
    let StmtKind::Struct {
        conforms,
        conformance_conditions,
        methods,
        ..
    } = &stmts[0].kind
    else {
        panic!("expected a struct");
    };
    assert_eq!(
        conforms,
        &vec!["Movable".to_string(), "Deinitable".to_string()]
    );
    assert_eq!(conformance_conditions[0].0, "Deinitable");
    assert_eq!(methods[0].name, "__deinit__");

    // Bounds normalize too.
    let stmts = parse("def consume[T: Movable & ImplicitlyDeletable](var value: T):\n    pass\n");
    let StmtKind::Def { type_params, .. } = &stmts[0].kind else {
        panic!("expected a def");
    };
    assert_eq!(type_params[0].bounds, vec!["Movable", "Deinitable"]);
}

/// The hidden definition synthesized for a lambda whose `lambda` keyword
/// starts at byte offset `start` (dummy spans; equality ignores them).
#[allow(clippy::too_many_arguments)]
fn lambda_expr(
    start: usize,
    type_params: Vec<TypeParam>,
    params: Vec<FnParam>,
    captures: Option<mojito::ast::CaptureList>,
    raises: bool,
    ret: Option<Type>,
    body: ExprKind,
) -> Expr {
    Expr::from(ExprKind::Lambda {
        def: Box::new(Stmt::from(StmtKind::Def {
            name: format!("$lambda${start}"),
            decorators: vec![],
            type_params,
            params,
            positional_only: None,
            keyword_only: None,
            captures,
            raises,
            raises_type: None,
            ret,
            where_clauses: vec![],
            body: vec![Stmt::from(StmtKind::Return(Some(Expr::from(body))))],
        })),
    })
}

#[test]
fn parses_minimal_lambda() {
    // `lambda: None` — no parameters, no arguments, no captures, no return
    // type; the hidden def's body is `return None`.
    assert_eq!(
        parse_expr("lambda: None\n"),
        lambda_expr(0, vec![], vec![], None, false, None, ExprKind::None)
    );
}

#[test]
fn parses_fully_explicit_lambda() {
    // Every optional part present: lambda-owned parameter list, typed
    // argument, `raises`, capture list, and return type.
    assert_eq!(
        parse_expr("lambda [N: Int](x: Int) raises {imm y} -> Int: x\n"),
        lambda_expr(
            0,
            vec![TypeParam {
                constraints: vec![],
                value_type: None,
                name: "N".into(),
                bounds: vec!["Int".into()],
                callable_bound: None,
                origin_mutability: None,
                infer_only: false,
                default: None,
            }],
            vec![fnparam("x", Type::Int)],
            Some(mojito::ast::CaptureList {
                entries: vec![Capture {
                    name: "y".into(),
                    kind: CaptureKind::Imm,
                }],
                default: None,
            }),
            true,
            Some(Type::Int),
            ExprKind::Identifier("x".into()),
        )
    );
}

#[test]
fn parses_lambda_omission_forms() {
    // No argument list.
    assert_eq!(
        parse_expr("lambda -> Int: 42\n"),
        lambda_expr(
            0,
            vec![],
            vec![],
            None,
            false,
            Some(Type::Int),
            ExprKind::Int(42.into())
        )
    );
    // No return type (fixed `None` downstream, not inferred).
    assert_eq!(
        parse_expr("lambda (x: Int): x\n"),
        lambda_expr(
            0,
            vec![],
            vec![fnparam("x", Type::Int)],
            None,
            false,
            None,
            ExprKind::Identifier("x".into()),
        )
    );
    // Captures only.
    assert_eq!(
        parse_expr("lambda {mut lst}: lst\n"),
        lambda_expr(
            0,
            vec![],
            vec![],
            Some(mojito::ast::CaptureList {
                entries: vec![Capture {
                    name: "lst".into(),
                    kind: CaptureKind::Mut,
                }],
                default: None,
            }),
            false,
            None,
            ExprKind::Identifier("lst".into()),
        )
    );
    // Explicit empty capture list stays distinct from an omitted one.
    assert_eq!(
        parse_expr("lambda (x: Int) {} -> Int: x + 1\n"),
        lambda_expr(
            0,
            vec![],
            vec![fnparam("x", Type::Int)],
            Some(mojito::ast::CaptureList {
                entries: vec![],
                default: None,
            }),
            false,
            Some(Type::Int),
            ExprKind::Infix(InfixOp::Add, ident("x"), int(1)),
        )
    );
    // Trailing comma in the argument list.
    assert_eq!(
        parse_expr("lambda (x: Int,) -> Int: x\n"),
        lambda_expr(
            0,
            vec![],
            vec![fnparam("x", Type::Int)],
            None,
            false,
            Some(Type::Int),
            ExprKind::Identifier("x".into()),
        )
    );
}

#[test]
fn lambda_body_extends_through_ternary() {
    // The body is one expression at conditional level: a trailing ternary
    // belongs to the body.
    assert_eq!(
        parse_expr("lambda (x: Bool) -> Int: 1 if x else 2\n"),
        lambda_expr(
            0,
            vec![],
            vec![fnparam("x", Type::Bool)],
            None,
            false,
            Some(Type::Int),
            ExprKind::IfExpr {
                cond: ident("x"),
                then_branch: int(1),
                else_branch: int(2),
            },
        )
    );
}

#[test]
fn lambda_as_call_argument_stops_at_comma() {
    // `f(lambda: x, y)` passes a lambda and a second argument; the comma is
    // not part of the lambda body.
    assert_eq!(
        parse_expr("f(lambda: x, y)\n"),
        Expr::from(ExprKind::Call {
            name: "f".into(),
            param_args: vec![],
            args: vec![
                lambda_expr(
                    2,
                    vec![],
                    vec![],
                    None,
                    false,
                    None,
                    ExprKind::Identifier("x".into())
                ),
                Expr::from(ExprKind::Identifier("y".into())),
            ],
            kwargs: vec![],
        })
    );
}

#[test]
fn parses_nested_lambda_in_body() {
    // A lambda body may contain (and immediately invoke) another lambda.
    let expr = parse_expr("lambda (x: Int) -> Int: (lambda (y: Int) -> Int: y)(x)\n");
    let ExprKind::Lambda { def } = &expr.kind else {
        panic!("expected a lambda");
    };
    let StmtKind::Def { body, .. } = &def.kind else {
        panic!("expected the hidden def");
    };
    let StmtKind::Return(Some(value)) = &body[0].kind else {
        panic!("expected the synthesized return");
    };
    let ExprKind::Invoke { callee, args, .. } = &value.kind else {
        panic!("expected an invocation of the inner lambda, got {value:?}");
    };
    assert!(matches!(callee.kind, ExprKind::Lambda { .. }));
    assert_eq!(args[0], Expr::from(ExprKind::Identifier("x".into())));
}

#[test]
fn rejects_unparenthesized_lambda_arguments() {
    let mut parser = Parser::new(Lexer::new("var f = lambda x: x\n"));
    let err = parser
        .parse_program()
        .expect_err("unparenthesized lambda arguments must be rejected");
    assert!(
        format!("{err:?}").contains("lambda arguments must be parenthesized and typed"),
        "unexpected diagnostic: {err:?}"
    );
}

#[test]
fn rejects_lambda_without_body_colon() {
    let mut parser = Parser::new(Lexer::new("var f = lambda (x: Int) x\n"));
    let err = parser
        .parse_program()
        .expect_err("a lambda without ':' must be rejected");
    assert!(
        format!("{err:?}").contains("Expected ':' before the lambda body"),
        "unexpected diagnostic: {err:?}"
    );
}

#[test]
fn lambda_is_a_reserved_word() {
    // Upstream made `lambda` a keyword: it is no longer usable as a name.
    let mut parser = Parser::new(Lexer::new("def lambda():\n    pass\n"));
    parser
        .parse_program()
        .expect_err("'lambda' as a def name must be rejected");
    let mut parser = Parser::new(Lexer::new("var lambda = 1\n"));
    parser
        .parse_program()
        .expect_err("'lambda' as a var name must be rejected");
}

#[test]
fn postfix_transfer_binds_tighter_than_infix_operators() {
    // `primary '^'` is a postfix sigil: `p + q^` transfers `q`, not the sum,
    // while a whitespace-separated `^` stays the bitwise-xor operator.
    assert_eq!(
        parse_expr("p + q^"),
        Expr::from(ExprKind::Infix(
            InfixOp::Add,
            ident("p"),
            bx(ExprKind::Transfer(ident("q")))
        ))
    );
    assert_eq!(
        parse_expr("a ^ b"),
        Expr::from(ExprKind::Infix(InfixOp::BitXor, ident("a"), ident("b")))
    );
    assert_eq!(
        parse_expr("p + q^ ^ r"),
        Expr::from(ExprKind::Infix(
            InfixOp::BitXor,
            bx(ExprKind::Infix(
                InfixOp::Add,
                ident("p"),
                bx(ExprKind::Transfer(ident("q")))
            )),
            ident("r")
        ))
    );
}
