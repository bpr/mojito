use mojito::{Lexer, Parser, SemanticAdjustment, TypeError, check, check_program, parse};

/// Parse `source` and run the type checker, returning its result.
fn check_source(source: &str) -> Result<(), TypeError> {
    let program = Parser::new(Lexer::new(source))
        .parse_program()
        .expect("parse error");
    check(&program)
}

/// Type-check a program that is expected to pass.
fn ok(source: &str) {
    check_source(source).expect("expected the program to type-check");
}

/// Type-check a program that is expected to fail, returning the error.
fn err(source: &str) -> TypeError {
    check_source(source).expect_err("expected a type error")
}

// --- Programs that should type-check ---

#[test]
fn accepts_well_typed_declarations() {
    ok(
        "var a: Int = 1 + 2 * 3\nvar ok: Bool = 1 < 2 and not False\nvar s: StringLiteral = \"a\" + \"b\"\n",
    );
}

#[test]
fn accepts_functions_and_calls() {
    ok("def add(x: Int, y: Int) -> Int:\n    return x + y\n\nvar s: Int = add(3, 4)\n");
}

#[test]
fn accepts_a_named_out_result_as_the_function_return() {
    ok(
        "def doubled(value: Int, out result: Int):\n    result = value * 2\n\ndef main():\n    var answer: Int = doubled(21)\n",
    );
}

#[test]
fn rejects_an_uninitialized_named_out_result() {
    assert_eq!(
        err("def missing(out result: Int):\n    pass\n"),
        TypeError::MissingReturn("missing".into())
    );
}

#[test]
fn accepts_recursion() {
    ok("def f(n: Int) -> Int:\n    return f(n)\n\nvar x: Int = f(0)\n");
}

#[test]
fn accepts_lexical_capture_downward_funarg() {
    ok(
        "def adder(n: Int) -> Int:\n    def add_n(x: Int) unified {n} -> Int:\n        return x + n\n    return add_n(100)\n\nvar c: Int = adder(42)\n",
    );
}

#[test]
fn checked_declarations_preserve_shadowed_function_and_unused_capture_identities() {
    let source = "def main():\n    var values = [40]\n    var marker = 1\n    if True:\n        def choose() -> Int:\n            return 1\n        print(choose())\n    if True:\n        def choose() -> Int:\n            return 42\n        print(choose())\n    def inspect() {var marker}:\n        pass\n    def take() {var values^}:\n        pass\n";
    let program = parse(source).expect("parse");
    let checked = check_program(&program).expect("check");

    let choices: Vec<_> = checked
        .declarations()
        .iter()
        .filter(|declaration| declaration.name == "choose")
        .collect();
    assert_eq!(choices.len(), 2);
    assert_ne!(choices[0].id, choices[1].id);
    assert_ne!(choices[0].binding, choices[1].binding);

    let call_bindings: Vec<_> = checked
        .expressions()
        .iter()
        .filter_map(|expression| match &expression.syntax.kind {
            mojito::ast::ExprKind::Call { name, .. } if name == "choose" => expression.binding,
            _ => None,
        })
        .collect();
    assert_eq!(call_bindings.len(), 2);
    assert_ne!(call_bindings[0], call_bindings[1]);

    let values = checked
        .declarations()
        .iter()
        .find(|declaration| declaration.name == "values")
        .and_then(|declaration| declaration.binding)
        .expect("values binding");
    let take = checked
        .declarations()
        .iter()
        .find(|declaration| declaration.name == "take")
        .expect("take declaration");
    assert!(matches!(
        take.captures.as_slice(),
        [capture]
            if capture.binding == values
                && capture.kind == mojito::ast::CaptureKind::Move
    ));
    assert_eq!(
        take.captures[0].ty,
        mojito::types::list_type(mojito::Ty::Int)
    );
    let marker = checked
        .declarations()
        .iter()
        .find(|declaration| declaration.name == "marker")
        .and_then(|declaration| declaration.binding)
        .expect("marker binding");
    let inspect = checked
        .declarations()
        .iter()
        .find(|declaration| declaration.name == "inspect")
        .expect("inspect declaration");
    assert!(matches!(
        inspect.captures.as_slice(),
        [capture]
            if capture.binding == marker
                && capture.kind == mojito::ast::CaptureKind::Copy
    ));
    assert_eq!(inspect.captures[0].ty, mojito::Ty::Int);
}

#[test]
fn checked_boundary_rekeys_cloned_source_provenance_by_occurrence() {
    let source = "def outer[n: Int]():\n    comptime for value in (1, True):\n        if True:\n            var x = value\n            def show() {x}:\n                print(x)\n            show()\n\ndef main():\n    outer[0]()\n";
    let parsed = parse(source).expect("parse");
    let elaborated = mojito::elaborate(parsed).expect("elaborate");
    let checked = check_program(&elaborated).expect("check");
    let xs: Vec<_> = checked
        .declarations()
        .iter()
        .filter(|declaration| declaration.name == "x")
        .collect();
    assert_eq!(xs.len(), 2);
    assert!(xs[0].location.same_provenance(&xs[1].location));
    assert_ne!(xs[0].location.syntax, xs[1].location.syntax);

    for expression in checked.expressions() {
        assert_eq!(
            checked.expression_ids_at(&expression.syntax.source_span()),
            &[expression.id],
            "one checked syntax occurrence must resolve to exactly one node"
        );
    }
}

#[test]
fn occurrence_keys_preserve_public_lookup_from_unique_input_syntax() {
    let program = parse(
        "def pick(value: Int) -> Int:\n    return value\ndef pick(value: StringLiteral) -> StringLiteral:\n    return value\ndef main():\n    print(pick(1))\n",
    )
    .expect("parse");
    let call = match &program[2].kind {
        mojito::ast::StmtKind::Def { body, .. } => match &body[0].kind {
            mojito::ast::StmtKind::Expr(expression) => match &expression.kind {
                mojito::ast::ExprKind::Call { args, .. } => &args[0],
                _ => panic!("expected print call"),
            },
            _ => panic!("expected expression statement"),
        },
        _ => panic!("expected main"),
    };
    let checked = check_program(&program).expect("check");
    assert_eq!(
        checked.overload_targets().get(&call.source_span()),
        Some(&"pick$ov$Int".to_string())
    );
}

#[test]
fn copied_capture_requires_implicit_copy_capability() {
    let explicit = err(
        "@fieldwise_init\nstruct Item:\n    var value: Int\n\ndef main():\n    var item = Item(42)\n    def read() {var item} -> Int:\n        return item.value\n    print(read())\n",
    );
    assert!(matches!(
        explicit,
        TypeError::TraitNotSatisfied { trait_name, .. }
            if trait_name == "ImplicitlyCopyable"
    ));

    let defaulted = err(
        "@fieldwise_init\nstruct Item:\n    var value: Int\n\ndef main():\n    var item = Item(42)\n    def read() {var} -> Int:\n        return item.value\n    print(read())\n",
    );
    assert!(matches!(
        defaulted,
        TypeError::TraitNotSatisfied { trait_name, .. }
            if trait_name == "ImplicitlyCopyable"
    ));
}

#[test]
fn nominal_callable_rejects_a_value_result_for_a_reference_result_contract() {
    let error = err(
        "struct Bad(def[origin: Origin[mut=True]](ref[origin] Int) -> ref[origin] Int):\n    def __call__[origin: Origin[mut=True]](self, ref[origin] value: Int, /) capturing -> Int:\n        return value\n",
    );
    assert!(matches!(error, TypeError::TraitMethodMismatch { .. }));
}

#[test]
fn nominal_callable_selects_the_contract_matching_same_arity_overload() {
    let source = "@fieldwise_init\nstruct Choose(def(Int) -> Int):\n    def __call__(self, value: Bool) -> Int:\n        return 0\n\n    def __call__(self, value: Int) -> Int:\n        return value + 1\n\ndef invoke(callback: def(Int) -> Int) -> Int:\n    return callback(41)\n\ndef main():\n    print(Choose()(41))\n    print(invoke(Choose()))\n";
    let program = mojito::parse(source).expect("parse overloaded nominal callable");
    let checked = mojito::check_program(&program).expect("check overloaded nominal callable");
    assert!(
        checked
            .overload_targets()
            .values()
            .any(|target| target == "Choose.__call__$ov$Int"),
        "the concrete callable target must retain the selected Int overload"
    );
    assert!(
        checked
            .overload_targets()
            .values()
            .any(|target| target == "__trait_dispatch.__call__$ov$Int"),
        "the def-typed parameter call must retain its contract signature"
    );
}

#[test]
fn nominal_callable_rejects_when_no_same_arity_overload_matches_the_contract() {
    let error = err(
        "struct Bad(def(StringLiteral) -> Int):\n    def __call__(self, value: Bool) -> Int:\n        return 0\n\n    def __call__(self, value: Int) -> Int:\n        return value\n",
    );
    assert!(matches!(error, TypeError::TraitMethodMismatch { .. }));
}

#[test]
fn nominal_callable_rejects_a_reference_result_from_the_wrong_parameter() {
    let error = err(
        "struct Bad(def[left: Origin[mut=True], right: Origin[mut=True]](ref[left] Int, ref[right] Int) -> ref[left] Int):\n    def __call__[left: Origin[mut=True], right: Origin[mut=True]](self, ref[left] first: Int, ref[right] second: Int, /) capturing -> ref[right] Int:\n        return second\n",
    );
    assert!(matches!(error, TypeError::TraitMethodMismatch { .. }));
}

#[test]
fn nominal_callable_accepts_alpha_equivalent_mutability_binders() {
    ok(
        "struct Good(def[is_mutable: Bool, origin: Origin[mut=is_mutable]](ref[origin] Int) -> ref[origin] Int):\n    def __call__[mutable: Bool, other: Origin[mut=mutable]](self, ref[other] value: Int, /) capturing -> ref[other] Int:\n        return value\n",
    );
}

#[test]
fn nominal_callable_rejects_a_reference_mutability_mismatch() {
    let error = err(
        "struct Bad(def[origin: Origin[mut=True]](ref[origin] Int) -> None):\n    def __call__[origin: Origin[mut=False]](self, ref[origin] value: Int, /) capturing:\n        print(value)\n",
    );
    assert!(matches!(error, TypeError::TraitMethodMismatch { .. }));
}

#[test]
fn nominal_callable_origin_unions_ignore_projected_member_order() {
    ok(
        "@fieldwise_init\nstruct Pair:\n    var left: Int\n    var right: Int\n\nstruct Good(def(ref[origin_of(first.left)] first: Pair, ref[origin_of(second.left)] second: Pair) -> ref[origin_of(first.left), origin_of(second.left)] Int):\n    def __call__(self, ref[origin_of(a.left)] a: Pair, ref[origin_of(b.left)] b: Pair, /) capturing -> ref[origin_of(b.left), origin_of(a.left)] Int:\n        return a.left\n",
    );
}

#[test]
fn accepts_shadowing_across_scopes() {
    ok("var x: Int = 1\ndef shadow() -> Int:\n    var x: Int = 99\n    return x\n");
}

#[test]
fn accepts_omitted_return_type_as_none() {
    ok("def noop(x: Int):\n    return\n\nnoop(1)\n");
}

// --- Annotation enforcement ---

#[test]
fn rejects_var_init_type_mismatch() {
    let e = err("var x: Int = True\n");
    assert!(matches!(e, TypeError::TypeMismatch { .. }), "got {:?}", e);
}

#[test]
fn rejects_wrong_argument_type() {
    let e = err("def f(x: Int) -> Int:\n    return x\n\nvar y: Int = f(True)\n");
    match e {
        TypeError::TypeMismatch {
            expected, found, ..
        } => {
            assert_eq!(expected, "Int");
            assert_eq!(found, "Bool");
        }
        other => panic!("expected a type mismatch, got {:?}", other),
    }
}

#[test]
fn rejects_wrong_return_type() {
    let e = err("def f() -> Int:\n    return True\n");
    assert!(matches!(e, TypeError::TypeMismatch { .. }), "got {:?}", e);
}

#[test]
fn rejects_arity_mismatch() {
    let e = err("def f(x: Int) -> Int:\n    return x\n\nvar y: Int = f(1, 2)\n");
    assert_eq!(
        e,
        TypeError::ArityMismatch {
            name: "f".into(),
            expected: 1,
            got: 2
        }
    );
}

// --- Errors that used to surface only at runtime ---

#[test]
fn rejects_operand_type_mismatch() {
    let e = err("var x: Int = 1 + True\n");
    assert!(matches!(e, TypeError::BadOperator { .. }), "got {:?}", e);
}

#[test]
fn rejects_ordering_on_strings() {
    let e = err("var x: Bool = \"a\" < \"b\"\n");
    assert!(matches!(e, TypeError::BadOperator { .. }), "got {:?}", e);
}

#[test]
fn rejects_cross_type_equality() {
    let e = err("var x: Bool = 1 == True\n");
    assert!(matches!(e, TypeError::BadOperator { .. }), "got {:?}", e);
}

#[test]
fn rejects_undefined_variable() {
    let e = err("var x: Int = y\n");
    assert_eq!(e, TypeError::UndefinedVariable("y".into()));
}

#[test]
fn rejects_calling_a_non_function() {
    let e = err("var x: Int = 1\nx(2)\n");
    assert!(matches!(e, TypeError::NotCallable { .. }), "got {:?}", e);
}

#[test]
fn rejects_escaping_closure() {
    let e =
        err("def make() -> Int:\n    def helper() -> Int:\n        return 1\n    return helper\n");
    assert_eq!(e, TypeError::ClosureEscape);
}

// --- Statically enforced Mojo rules ---

#[test]
fn rejects_same_scope_redeclaration() {
    let e = err("var x: Int = 1\nvar x: Int = 2\n");
    assert_eq!(e, TypeError::Redeclaration("x".into()));
}

#[test]
fn rejects_var_redeclaring_param() {
    let e = err("def f(x: Int) -> Int:\n    var x: Int = 1\n    return x\n");
    assert_eq!(e, TypeError::Redeclaration("x".into()));
}

#[test]
fn rejects_return_outside_function() {
    let e = err("return 1\n");
    assert_eq!(e, TypeError::ReturnOutsideFunction);
}

// --- Structs ---

const POINT: &str = "@fieldwise_init\nstruct Point:\n    var x: Int\n    var y: Int\n\n    def sum(self) -> Int:\n        return self.x + self.y\n\n";

#[test]
fn accepts_struct_construction_field_and_method() {
    ok(&format!(
        "{POINT}var p: Point = Point(1, 2)\nvar s: Int = p.sum()\nvar a: Int = p.x\n"
    ));
}

#[test]
fn accepts_struct_typed_field_and_chained_access() {
    ok(
        "@fieldwise_init\nstruct Inner:\n    var v: Int\n\n@fieldwise_init\nstruct Outer:\n    var inner: Inner\n    var tag: Int\n\nvar o: Outer = Outer(Inner(5), 9)\nvar v: Int = o.inner.v\n",
    );
}

#[test]
fn rejects_unknown_field() {
    let e = err(&format!(
        "{POINT}var p: Point = Point(1, 2)\nvar z: Int = p.z\n"
    ));
    assert!(matches!(e, TypeError::NoSuchField { .. }), "got {:?}", e);
}

#[test]
fn rejects_unknown_method() {
    let e = err(&format!(
        "{POINT}var p: Point = Point(1, 2)\nvar z: Int = p.nope()\n"
    ));
    assert!(matches!(e, TypeError::NoSuchMethod { .. }), "got {:?}", e);
}

#[test]
fn rejects_construction_arity_and_type() {
    assert!(matches!(
        err(&format!("{POINT}var p: Point = Point(1)\n")),
        TypeError::ArityMismatch { .. }
    ));
    assert!(matches!(
        err(&format!("{POINT}var p: Point = Point(1, True)\n")),
        TypeError::TypeMismatch { .. }
    ));
}

#[test]
fn rejects_struct_without_constructor() {
    let e = err("struct P:\n    var x: Int\n\nvar p: P = P(1)\n");
    assert_eq!(e, TypeError::NoConstructor("P".into()));
}

#[test]
fn rejects_unknown_type_annotation() {
    let e = err("var p: Bogus = 1\n");
    assert_eq!(e, TypeError::UnknownType("Bogus".into()));
}

#[test]
fn rejects_member_access_on_non_struct() {
    let e = err("var x: Int = 1\nvar y: Int = x.field\n");
    assert!(matches!(e, TypeError::NoSuchField { .. }), "got {:?}", e);
}

#[test]
fn checks_method_body_against_field_types() {
    // `get` is declared to return Int but returns a Bool field.
    let e = err(
        "@fieldwise_init\nstruct P:\n    var b: Bool\n\n    def get(self) -> Int:\n        return self.b\n",
    );
    assert!(matches!(e, TypeError::TypeMismatch { .. }), "got {:?}", e);
}

// --- Numbers: Float64, UInt, conversions ---

#[test]
fn accepts_float64_arithmetic_and_division() {
    ok("var x: Float64 = 1.5 + 2.0 * 3.0\nvar y: Float64 = 10.0 / 4.0\nvar ok: Bool = x < y\n");
}

#[test]
fn accepts_uint_via_conversion() {
    ok("var u: UInt = UInt(0)\nu = u + UInt(1)\nvar ok: Bool = u < UInt(10)\n");
}

#[test]
fn accepts_numeric_conversions() {
    ok("var i: Int = Int(3.9)\nvar f: Float64 = Float64(3)\nvar u: UInt = UInt(True)\n");
}

#[test]
fn int_literal_coerces_to_uint_and_float() {
    // Literal coercion: bare literals materialize to the annotated type.
    ok("var u: UInt = 0\nvar f: Float64 = 3\nvar g: Float64 = 1 + 2\nu = u + 1\n");
}

#[test]
fn literal_arithmetic_mixes_but_concrete_types_do_not() {
    // Mixing two literals is fine (`1.0 + 2` is Float64)...
    ok("var x: Float64 = 1.0 + 2\n");
    // ...but mixing two *concrete* numeric types is rejected.
    let e = err("var i: Int = 1\nvar u: UInt = UInt(2)\nvar bad: Int = i + u\n");
    assert!(matches!(e, TypeError::BadOperator { .. }), "got {:?}", e);
}

#[test]
fn true_division_yields_float64() {
    // `/` returns Float64 even on integers, so `7 / 2` is 3.5 (not an Int).
    ok("var x: Float64 = 7 / 2\n");
    let e = err("var x: Int = 7 / 2\n");
    match e {
        TypeError::TypeMismatch {
            expected, found, ..
        } => {
            assert_eq!(expected, "Int");
            assert_eq!(found, "Float64");
        }
        other => panic!("expected a type mismatch, got {:?}", other),
    }
}

#[test]
fn floor_div_mod_pow_preserve_type() {
    ok(
        "var a: Int = 7 // 2\nvar b: Int = 7 % 2\nvar c: Int = 2 ** 10\nvar d: Float64 = 7.0 // 2.0\n",
    );
}

#[test]
fn rejects_unary_minus_on_uint() {
    let e = err("var u: UInt = UInt(1)\nvar n: UInt = -u\n");
    assert!(matches!(e, TypeError::BadOperator { .. }), "got {:?}", e);
}

#[test]
fn rejects_cross_numeric_equality() {
    let e = err("var ok: Bool = Int(1) == UInt(1)\n");
    assert!(matches!(e, TypeError::BadOperator { .. }), "got {:?}", e);
}

#[test]
fn rejects_conversion_of_string() {
    let e = err("var i: Int = Int(\"5\")\n");
    assert!(matches!(e, TypeError::TypeMismatch { .. }), "got {:?}", e);
}

// --- Assignment ---

#[test]
fn accepts_assignment_keeping_the_declared_type() {
    ok("var x: Int = 1\nx = 2\nx = x + 40\n");
}

#[test]
fn accepts_assignment_to_an_outer_variable_from_a_loop() {
    ok("var total: Int = 0\nfor i in range(10):\n    total = total + i\n");
}

#[test]
fn var_less_introduction_is_rejected() {
    // Mojito requires `var` to declare a new variable; a bare assignment to an
    // undeclared name is a hard error (matching Mojo's single declaration path).
    assert!(matches!(
        err("x = 1\n"),
        TypeError::AssignToUndeclared(name) if name == "x"
    ));
}

#[test]
fn var_is_block_scoped_and_bare_assignment_needs_prior_declaration() {
    // Reassigning a declared `var` from inside a branch is fine.
    ok("def f() -> Int:\n    var value = 0\n    if True:\n        value = 7\n    return value\n");

    // A `var` declared inside a block does not leak past it — reading it after the
    // block is an undefined-variable error, not a function-scoped binding.
    assert!(matches!(
        err("def f() -> Int:\n    if True:\n        var value = 7\n    return value\n"),
        TypeError::UndefinedVariable(name) if name == "value"
    ));

    // A bare assignment nested in a block still requires `var` to declare.
    assert!(matches!(
        err("def f() -> Int:\n    if True:\n        value = 7\n    return value\n"),
        TypeError::AssignToUndeclared(name) if name == "value"
    ));
}

#[test]
fn rejects_assignment_of_wrong_type() {
    let e = err("var x: Int = 1\nx = True\n");
    match e {
        TypeError::TypeMismatch {
            expected, found, ..
        } => {
            assert_eq!(expected, "Int");
            assert_eq!(found, "Bool");
        }
        other => panic!("expected a type mismatch, got {:?}", other),
    }
}

#[test]
fn rejects_assigning_a_closure() {
    let e = err("def f() -> Int:\n    return 1\n\ndef g() -> Int:\n    return 2\n\nf = g\n");
    assert_eq!(e, TypeError::ClosureEscape);
}

// --- Control flow: if / while / for / break / continue ---

#[test]
fn accepts_if_elif_else() {
    ok(
        "var x: Int = 3\nif x > 0:\n    var a: Int = 1\nelif x == 0:\n    var b: Int = 2\nelse:\n    var c: Int = 3\n",
    );
}

#[test]
fn accepts_while_with_bool_condition() {
    ok("if True:\n    while False:\n        break\n");
}

#[test]
fn accepts_for_over_range_with_int_loop_var() {
    // The loop variable is Int and is in scope in the body.
    ok("for i in range(10):\n    var x: Int = i + 1\n");
}

#[test]
fn accepts_range_with_one_two_or_three_args() {
    ok("for i in range(0, 10, 2):\n    pass\n");
}

#[test]
fn accepts_break_and_continue_inside_loop() {
    ok("for i in range(10):\n    if i == 5:\n        break\n    continue\n");
}

#[test]
fn checks_owned_iteration_and_collection_comprehensions() {
    let move_only = "struct Item:\n    var value: Int\n    def __init__(out self, value: Int):\n        self.value = value\n\n";
    ok(&format!(
        "{move_only}def main():\n    var values = [Item(1), Item(2)]\n    for var item in values^:\n        print(item.value)\n"
    ));
    // Binding convention and source mode are independent: a `var` target over a
    // borrowed source binds a mutable copy, and an immutable target over a
    // consumed source is accepted. Neither the old `var`-requires-transfer nor
    // transfer-requires-`var` coupling exists any longer.
    ok(
        "def main():\n    var values = [1, 2]\n    for var item in values:\n        item += 1\n        print(item)\n",
    );
    ok("def main():\n    var values = [1, 2]\n    for item in values^:\n        print(item)\n");
    // An unadorned target is immutable regardless of the source mode, so
    // mutating it is rejected.
    assert!(matches!(
        err("def main():\n    var values = [1, 2]\n    for item in values:\n        item += 1\n"),
        TypeError::ImmutableBinding(name) if name == "item"
    ));

    ok(
        "def main():\n    var xs = [x * x for x in range(5) if x % 2 == 0]\n    var s = {x % 3 for x in range(8)}\n    var d = {x: x * x for x in range(4)}\n    print(len(xs), len(s), d[3])\n",
    );

    let linear = "@explicit_destroy(\"close Item\")\nstruct Linear(ImplicitlyDeletable where False):\n    var value: Int\n    def __init__(out self, value: Int):\n        self.value = value\n    def close(deinit self):\n        pass\n\n";
    ok(&format!(
        "{linear}def main():\n    var values = [Linear(1), Linear(2)]\n    for var item in values^:\n        item^.close()\n"
    ));
    assert!(matches!(
        err(&format!(
            "{linear}def main():\n    var values = [Linear(1), Linear(2)]\n    for var item in values^:\n        item^.close()\n        break\n"
        )),
        TypeError::Unsupported(message) if message.contains("residual elements")
    ));
}

#[test]
fn comprehension_binders_are_lexical_and_enforce_linear_cleanup() {
    ok(
        "def main():\n    var x = 100\n    var values = [x for x in range(3)]\n    print(x, values)\n",
    );
    ok(
        "def main():\n    var values = [x for x in range(2) for x in range(x + 1)]\n    print(values)\n",
    );

    let linear = "@explicit_destroy(\"close Item\")\nstruct Item(ImplicitlyDeletable where False):\n    var value: Int\n    def __init__(out self, value: Int):\n        self.value = value\n    def close(deinit self):\n        pass\n\n";
    assert!(matches!(
        err(&format!(
            "{linear}def main():\n    var values = [Item(1)]\n    var result = [item.value for var item in values^]\n"
        )),
        TypeError::ExplicitDestroy { var, .. } if var == "item"
    ));
    ok(&format!(
        "{linear}def main():\n    var values = [Item(1)]\n    var result = [item^.close() for var item in values^]\n"
    ));
}

#[test]
fn collection_displays_use_parameter_and_return_context() {
    ok(
        "def consume(values: Set[Float64]):\n    pass\n\ndef empty() -> Set[Float64]:\n    return {}\n\ndef numbers() -> Set[Float64]:\n    return {1, 2}\n\ndef main():\n    consume({})\n    consume({1, 2})\n    print(empty(), numbers())\n",
    );
    ok(
        "def consume(values: Dict[StringLiteral, Float64]):\n    pass\n\ndef main():\n    consume({})\n    consume({\"one\": 1, \"two\": 2})\n",
    );
}

#[test]
fn rejects_non_bool_if_condition() {
    let e = err("if 1:\n    pass\n");
    match e {
        TypeError::TypeMismatch {
            expected, found, ..
        } => {
            assert_eq!(expected, "Bool");
            assert_eq!(found, "Int");
        }
        other => panic!("expected a type mismatch, got {:?}", other),
    }
}

#[test]
fn rejects_non_bool_while_condition() {
    let e = err("while 1:\n    pass\n");
    assert!(matches!(e, TypeError::TypeMismatch { .. }), "got {:?}", e);
}

#[test]
fn rejects_for_over_non_range() {
    let e = err("for i in 5:\n    pass\n");
    match e {
        TypeError::TypeMismatch {
            expected, found, ..
        } => {
            assert_eq!(
                expected,
                "a nominal collection or a type with borrowed __iter__"
            );
            assert_eq!(found, "Int");
        }
        other => panic!("expected a type mismatch, got {:?}", other),
    }
}

#[test]
fn rejects_range_with_non_int_argument() {
    let e = err("for i in range(True):\n    pass\n");
    assert!(matches!(e, TypeError::TypeMismatch { .. }), "got {:?}", e);
}

#[test]
fn rejects_range_with_no_arguments() {
    let e = err("for i in range():\n    pass\n");
    assert!(matches!(e, TypeError::ArityMismatch { .. }), "got {:?}", e);
}

#[test]
fn rejects_break_outside_loop() {
    let e = err("break\n");
    assert_eq!(e, TypeError::BreakOutsideLoop);
}

#[test]
fn rejects_continue_outside_loop() {
    let e = err("continue\n");
    assert_eq!(e, TypeError::ContinueOutsideLoop);
}

#[test]
fn rejects_break_in_nested_def_inside_loop() {
    // A function boundary resets the loop context.
    let e = err("while True:\n    def g() -> Int:\n        break\n        return 1\n");
    assert_eq!(e, TypeError::BreakOutsideLoop);
}

#[test]
fn loop_variable_does_not_leak_after_for() {
    // `i` is scoped to the loop body, so it is undefined afterwards.
    let e = err("for i in range(3):\n    pass\nvar x: Int = i\n");
    assert_eq!(e, TypeError::UndefinedVariable("i".into()));
}

// --- Parameterization (generics) ---

/// A generic `Pair[T]` struct used by several tests below.
const PAIR: &str = "@fieldwise_init\nstruct Pair[T: Copyable & Movable]:\n    var left: Self.T\n    var right: Self.T\n";

#[test]
fn accepts_generic_struct_construction_and_member() {
    ok(&format!(
        "{PAIR}var p: Pair[Int] = Pair(3, 4)\nvar a: Int = p.left\nvar b: Int = p.right\n"
    ));
}

#[test]
fn infers_struct_type_argument_from_construction() {
    // No explicit `Pair[Int]` annotation: T is inferred from the arguments.
    ok(&format!(
        "{PAIR}var p: Pair[Float64] = Pair(1.5, 2.5)\nvar f: Float64 = p.left\n"
    ));
}

#[test]
fn accepts_generic_function_identity() {
    ok(
        "def id[T: Copyable & Movable](x: T) -> T:\n    return x\n\nvar n: Int = id(5)\nvar s: StringLiteral = id(\"hi\")\n",
    );
}

#[test]
fn accepts_generic_function_over_generic_struct() {
    ok(&format!(
        "{PAIR}def first[T: Copyable & Movable](p: Pair[T]) -> T:\n    return p.left\n\nvar p: Pair[Int] = Pair(1, 2)\nvar x: Int = first(p)\n"
    ));
}

#[test]
fn accepts_generic_struct_method_returning_self_param() {
    ok(
        "@fieldwise_init\nstruct Box[T: Copyable & Movable]:\n    var val: Self.T\n\n    def get(self) -> Self.T:\n        return self.val\n\nvar b: Box[Int] = Box(7)\nvar g: Int = b.get()\n",
    );
}

#[test]
fn rejects_operator_on_opaque_type_parameter() {
    // An unconstrained `T` supports no operators.
    let e = err("def bad[T: Copyable & Movable](x: T) -> T:\n    return x + x\n");
    assert!(matches!(e, TypeError::BadOperator { .. }), "got {:?}", e);
}

#[test]
fn rejects_wrong_struct_type_argument() {
    let e = err(&format!("{PAIR}var p: Pair[Int] = Pair(1.5, 2.5)\n"));
    assert!(matches!(e, TypeError::TypeMismatch { .. }), "got {:?}", e);
}

#[test]
fn rejects_conflicting_type_parameter_solutions() {
    // A concrete Float64 parameter and an Int-literal argument do not unify:
    // the literal is defaulted to Int, so `Pair(1.0, 2)` is a conflict, not
    // `Pair[Float64]` (keeps generic specialization and VM storage consistent).
    let e = err(&format!("{PAIR}var p: Pair[Float64] = Pair(1.0, 2)\n"));
    assert!(matches!(e, TypeError::TypeMismatch { .. }), "got {:?}", e);
}

#[test]
fn rejects_unknown_trait_bound() {
    let e = err("def bad[T: Frobnicate](x: T) -> T:\n    return x\n");
    assert_eq!(e, TypeError::UnknownTrait("Frobnicate".into()));
}

#[test]
fn rejects_bare_type_parameter_as_struct_field() {
    // Inside a struct a parameter must be written `Self.T`, not bare `T`.
    let e = err("@fieldwise_init\nstruct Bad[T: Copyable & Movable]:\n    var v: T\n");
    assert_eq!(e, TypeError::UnknownType("T".into()));
}

#[test]
fn rejects_wrong_type_argument_count() {
    let e = err(
        "@fieldwise_init\nstruct Box[T: Copyable & Movable]:\n    var v: Self.T\n\nvar b: Box[Int, Int] = Box(1)\n",
    );
    assert!(
        matches!(
            e,
            TypeError::WrongTypeArgCount {
                expected: 1,
                got: 2,
                ..
            }
        ),
        "got {:?}",
        e
    );
}

#[test]
fn rejects_type_arguments_on_non_generic_struct() {
    let e = err("@fieldwise_init\nstruct Point:\n    var x: Int\n\nvar p: Point[Int] = Point(1)\n");
    assert!(
        matches!(
            e,
            TypeError::WrongTypeArgCount {
                expected: 0,
                got: 1,
                ..
            }
        ),
        "got {:?}",
        e
    );
}

#[test]
fn generic_call_sites_record_their_resolved_instantiations() {
    // Every bound-generic call site retains its resolved application (callee
    // plus exact compile-time arguments) for instantiation discovery —
    // explicit and inferred alike.
    let program = Parser::new(Lexer::new(
        "def pick[T: Movable](x: T) -> T:\n    return x^\n\ndef main():\n    var a = pick[Int](1)\n    var b = pick(\"s\")\n",
    ))
    .parse_program()
    .expect("parse error");
    let checked = check_program(&program).expect("check");
    let picks: Vec<String> = checked
        .generic_instantiations()
        .values()
        .filter(|instantiation| instantiation.callee == "pick")
        .map(|instantiation| format!("{:?}", instantiation.arguments))
        .collect();
    assert_eq!(picks.len(), 2, "got {picks:?}");
    assert!(
        picks.iter().any(|arguments| arguments.contains("Int")),
        "got {picks:?}"
    );
    assert!(
        picks.iter().any(|arguments| arguments.contains("String")),
        "got {picks:?}"
    );
}

#[test]
fn abstract_element_copyability_derives_from_member_bounds() {
    // A `C.Element` copy is proven by the bound trait's member declaration,
    // never vacuously: a Movable-only element cannot be returned by copy from
    // a generic loop, while a Copyable-bounded one can.
    let movable = "trait Items:\n    comptime Element: Movable\n\ndef first[C: Items](xs: C) raises -> C.Element:\n    for item in xs:\n        return item\n    raise \"empty\"\n";
    let error = err(movable);
    assert!(
        matches!(&error, TypeError::NonCopyable { ty, .. } if ty == "C.Element"),
        "got {error:?}"
    );
    let copyable = "trait Items:\n    comptime Element: Copyable & Movable\n\ndef first[C: Items](xs: C) raises -> C.Element:\n    for item in xs:\n        return item\n    raise \"empty\"\n";
    ok(copyable);
}

#[test]
fn abstract_bound_for_ref_is_rejected() {
    // A generic bound advances through the abstract `__iterator_dispatch`
    // contract, whose `__next__` yields `Element` values — mutating the
    // per-iteration copy would silently lose writes, so a `ref` target is
    // rejected in both statement and comprehension form.
    let header = "@fieldwise_init\nstruct StopIteration:\n    pass\n\ntrait ItemIterator:\n    comptime Element: Movable\n    def __next__(mut self) raises StopIteration -> Self.Element: ...\n\ntrait Items:\n    comptime Element: Movable\n    comptime Iter: ItemIterator\n    def __iter__(ref self) -> Self.Iter: ...\n\n";
    let statement =
        format!("{header}def touch[C: Items](xs: C):\n    for ref x in xs:\n        pass\n");
    let error = err(&statement);
    assert!(
        matches!(&error, TypeError::Unsupported(message) if message.contains("generic Iterable bound")),
        "got {error:?}"
    );
    let comprehension =
        format!("{header}def collect[C: Items](xs: C):\n    var ys = [x for ref x in xs]\n");
    let error = err(&comprehension);
    assert!(
        matches!(&error, TypeError::Unsupported(message) if message.contains("generic Iterable bound")),
        "got {error:?}"
    );
}

#[test]
fn projected_origin_iterator_contract_carries_parametric_mutability() {
    // A yielded reference declared as a projection off a parametric-mut
    // struct origin (`ref[o._get_owned_interior["element"]] T`) lowers with
    // the origin parameter's declared mutability: a mutable source permits
    // `for ref` write-through, while a read-parameter source rejects it.
    let header = "@fieldwise_init\nstruct StopIteration:\n    pass\n\n@fieldwise_init\nstruct ProjIter[m: Bool, //, o: Origin[mut=m]]:\n    var src: ref[o] List[Int]\n    var index: Int\n\n    def __next__(mut self) raises StopIteration -> ref[o._get_owned_interior[\"element\"]] Int:\n        if self.index >= len(self.src):\n            raise StopIteration()\n        var r = self.index\n        self.index += 1\n        return self.src[r]\n\nstruct Numbers:\n    var items: List[Int]\n\n    def __init__(out self):\n        self.items = [4, 5, 6]\n\n    def __iter__(ref self) -> ProjIter:\n        ref items = self.items\n        return ProjIter(items, 0)\n\n";
    let mutable = format!(
        "{header}def main():\n    var nums = Numbers()\n    for ref x in nums:\n        x += 1\n"
    );
    ok(&mutable);
    let immutable =
        format!("{header}def bump(nums: Numbers):\n    for ref x in nums:\n        x += 1\n");
    let error = err(&immutable);
    assert!(
        matches!(&error, TypeError::ImmutableBinding(name) if name == "x")
            || matches!(&error, TypeError::Unsupported(_)),
        "got {error:?}"
    );
}

#[test]
fn fixed_immutable_origin_iterator_still_binds_immutably() {
    // Loop-site resolution touches only parametric mutability: a declared
    // `Origin[mut=False]` iterator yields immutable references even over a
    // mutable source, so a `for ref` write is rejected.
    let error = err(
        "@fieldwise_init\nstruct StopIteration:\n    pass\n\n@fieldwise_init\nstruct FrozenIter[o: Origin[mut=False]]:\n    var src: ref[o] List[Int]\n    var index: Int\n\n    def __next__(mut self) raises StopIteration -> ref[o] Int:\n        if self.index >= len(self.src):\n            raise StopIteration()\n        var r = self.index\n        self.index += 1\n        return self.src[r]\n\nstruct Numbers:\n    var items: List[Int]\n\n    def __init__(out self):\n        self.items = [1, 2]\n\n    def __iter__(ref self) -> FrozenIter:\n        ref items = self.items\n        return FrozenIter(items, 0)\n\ndef main():\n    var nums = Numbers()\n    for ref x in nums:\n        x += 1\n",
    );
    assert!(
        matches!(&error, TypeError::ImmutableBinding(name) if name == "x")
            || matches!(&error, TypeError::Unsupported(_)),
        "got {error:?}"
    );
}

#[test]
fn parametric_mut_origin_binder_is_erased_from_struct_parameters() {
    // The infer-only `Bool` that binds a sibling origin parameter's `mut=`
    // erases with the origin parameter: an application supplies only the
    // ordinary type parameter, and spelling the Bool explicitly over-applies.
    ok(
        "@fieldwise_init\nstruct Slot[m: Bool, //, T: Movable, o: Origin[mut=m]]:\n    var value: ref[o] Self.T\n\ndef main():\n    var n = 5\n    ref h = n\n    var s = Slot[Int](h)\n    print(s.value)\n",
    );
    let error = err(
        "@fieldwise_init\nstruct Slot[m: Bool, //, T: Movable, o: Origin[mut=m]]:\n    var value: ref[o] Self.T\n\ndef main():\n    var n = 5\n    ref h = n\n    var s = Slot[True, Int](h)\n    print(s.value)\n",
    );
    assert!(
        matches!(&error, TypeError::WrongTypeArgCount { .. }),
        "got {error:?}"
    );
}

#[test]
fn struct_declarations_resolve_order_independently() {
    // Signatures (fields, member types, method signatures) register for every
    // top-level struct before any body checks, so same-module declarations may
    // reference each other in either order — including mutually, as an
    // iterator holding `ref[o] Container` declared above its container.
    ok(
        "@fieldwise_init\nstruct Holder:\n    var b: Bin\n\n@fieldwise_init\nstruct Bin(Copyable, Movable):\n    var n: Int\n\ndef main():\n    var h = Holder(Bin(3))\n    print(h.b.n)\n",
    );
    ok(
        "struct A:\n    var n: Int\n\n    def __init__(out self):\n        self.n = 1\n\n    def make(self) -> Int:\n        var b = B(2)\n        return b.n + self.n\n\n@fieldwise_init\nstruct B:\n    var n: Int\n\ndef main():\n    print(A().make())\n",
    );
}

#[test]
fn rejects_by_value_struct_self_containment() {
    // Order-independent shells would otherwise let a struct contain itself (or
    // a mutual peer) by value; only reference or pointer indirection breaks
    // the infinite layout.
    let direct = err("struct S:\n    var s: S\n");
    assert!(
        matches!(&direct, TypeError::Unsupported(message) if message.contains("cannot contain itself by value")),
        "got {direct:?}"
    );
    let mutual = err(
        "@fieldwise_init\nstruct A:\n    var b: B\n\n@fieldwise_init\nstruct B:\n    var a: A\n",
    );
    assert!(
        matches!(&mutual, TypeError::Unsupported(message) if message.contains("cannot contain itself by value")),
        "got {mutual:?}"
    );
}

#[test]
fn rejects_self_param_naming_unknown_parameter() {
    let e = err("@fieldwise_init\nstruct Box[T: Copyable & Movable]:\n    var v: Self.U\n");
    assert_eq!(e, TypeError::UnknownSelfParam("U".into()));
}

#[test]
fn rejects_uninferable_type_parameter() {
    // `T` is not mentioned by any field, so a construction can't solve it (there
    // is no explicit `Phantom[Int](...)` syntax to supply it).
    let e = err(
        "@fieldwise_init\nstruct Phantom[T: AnyType]:\n    var x: Int\n\nvar p: Phantom[Int] = Phantom(1)\n",
    );
    assert!(
        matches!(&e, TypeError::CannotInferTypeParam { param, .. } if param == "T"),
        "got {:?}",
        e
    );
}

#[test]
fn rejects_duplicate_type_parameter_names() {
    let e = err("def f[T: AnyType, T: AnyType](x: T) -> T:\n    return x\n");
    assert_eq!(e, TypeError::Redeclaration("T".into()));
}

// --- Traits (Phase 1b) ---

/// A `Quackable` trait, a conforming `Duck`, and a bounded generic function.
const QUACK: &str = "trait Quackable:\n    def quack(self) -> StringLiteral:\n        ...\n\n@fieldwise_init\nstruct Duck(Quackable):\n    var name: StringLiteral\n\n    def quack(self) -> StringLiteral:\n        return \"Quack\"\n\ndef make_it_quack[T: Quackable](x: T) -> StringLiteral:\n    return x.quack()\n";

#[test]
fn accepts_trait_conformance_and_bounded_call() {
    ok(&format!(
        "{QUACK}var d: Duck = Duck(\"Donald\")\nvar s: StringLiteral = make_it_quack(d)\n"
    ));
}

#[test]
fn accepts_self_type_in_trait_requirement() {
    ok(
        "trait Eq2:\n    def same(self, other: Self) -> Bool:\n        ...\n\n@fieldwise_init\nstruct P(Eq2):\n    var x: Int\n\n    def same(self, other: Self) -> Bool:\n        return self.x == other.x\n\nvar a: P = P(1)\nvar r: Bool = a.same(P(2))\n",
    );
}

#[test]
fn accepts_calling_bound_trait_method_on_type_parameter() {
    // Inside `make_it_quack`, `x: T` with `T: Quackable` can call `quack()`.
    ok(QUACK);
}

#[test]
fn accepts_trait_receiver_convention_requirements() {
    ok(
        "trait Bumpable:\n    def bump(mut self):\n        ...\n\n@fieldwise_init\nstruct Counter(Bumpable):\n    var n: Int\n\n    def bump(mut self):\n        self.n = self.n + 1\n\ndef inc[T: Bumpable](mut x: T):\n    x.bump()\n",
    );
    ok(
        "trait Consumable:\n    def consume(var self):\n        ...\n\n@fieldwise_init\nstruct Box(Consumable):\n    var n: Int\n\n    def consume(var self):\n        pass\n",
    );
}

#[test]
fn rejects_assignment_to_immutable_parameter_but_allows_mut_parameter() {
    let e = err("def f(a: Int):\n    a = a + 1\n");
    assert_eq!(e, TypeError::ImmutableBinding("a".into()));

    ok("def f(mut a: Int):\n    a = a + 1\n");
}

#[test]
fn rejects_trait_receiver_convention_mismatch() {
    let e = err(
        "trait Bumpable:\n    def bump(mut self):\n        ...\n\n@fieldwise_init\nstruct Counter(Bumpable):\n    var n: Int\n\n    def bump(self):\n        pass\n",
    );
    assert!(
        matches!(e, TypeError::TraitMethodMismatch { .. }),
        "got {:?}",
        e
    );
}

#[test]
fn rejects_argument_not_conforming_to_bound() {
    let e = err(&format!(
        "{QUACK}@fieldwise_init\nstruct Cat:\n    var n: Int\n\nvar s: StringLiteral = make_it_quack(Cat(1))\n"
    ));
    assert!(
        matches!(&e, TypeError::TraitNotSatisfied { trait_name, .. } if trait_name == "Quackable"),
        "got {:?}",
        e
    );
}

#[test]
fn rejects_struct_missing_a_required_trait_method() {
    let e = err(
        "trait Quackable:\n    def quack(self) -> StringLiteral:\n        ...\n\n@fieldwise_init\nstruct Duck(Quackable):\n    var name: StringLiteral\n",
    );
    assert!(
        matches!(&e, TypeError::MissingTraitMethod { method, .. } if method == "quack"),
        "got {:?}",
        e
    );
}

#[test]
fn rejects_struct_with_mismatched_trait_method_signature() {
    // `quack` returns Int, but the trait requires it to return String.
    let e = err(
        "trait Quackable:\n    def quack(self) -> StringLiteral:\n        ...\n\n@fieldwise_init\nstruct Duck(Quackable):\n    var name: StringLiteral\n\n    def quack(self) -> Int:\n        return 1\n",
    );
    assert!(
        matches!(e, TypeError::TraitMethodMismatch { .. }),
        "got {:?}",
        e
    );
}

#[test]
fn copyable_reference_next_refines_a_value_requirement() {
    ok(
        "trait IteratorContract:\n    comptime Element: Movable\n    def __next__(mut self) -> Self.Element: ...\n\n@fieldwise_init\nstruct IntIterator(IteratorContract):\n    comptime Element = Int\n    var value: Int\n    def __next__(mut self) -> ref[origin_of(self.value)] Int:\n        return self.value\n",
    );

    // The rule is Copyable, not the stronger ImplicitlyCopyable contract.
    ok(
        "trait IteratorContract:\n    comptime Element: Movable\n    def __next__(mut self) -> Self.Element: ...\n\n@fieldwise_init\nstruct Box(Copyable, Movable):\n    var value: Int\n\n@fieldwise_init\nstruct BoxIterator(IteratorContract):\n    comptime Element = Box\n    var value: Box\n    def __next__(mut self) -> ref[origin_of(self.value)] Box:\n        return self.value\n",
    );

    // A conditional generic conformance supplies the Copyable proof while the
    // iterator template is still symbolic.
    ok(
        "trait IteratorContract:\n    comptime Element: Movable\n    def __next__(mut self) -> Self.Element: ...\n\n@fieldwise_init\nstruct GenericIterator[T: Movable](\n    IteratorContract where conforms_to(T, Copyable)\n):\n    comptime Element = Self.T\n    var value: Self.T\n    def __next__(mut self) -> ref[origin_of(self.value)] Self.T where conforms_to(Self.T, Copyable):\n        return self.value\n",
    );
}

#[test]
fn reference_next_refinement_rejects_noncopyable_or_mismatched_referents() {
    let noncopyable = err(
        "trait IteratorContract:\n    comptime Element: Movable\n    def __next__(mut self) -> Self.Element: ...\n\n@fieldwise_init\nstruct Token(Movable):\n    var value: Int\n\n@fieldwise_init\nstruct TokenIterator(IteratorContract):\n    comptime Element = Token\n    var value: Token\n    def __next__(mut self) -> ref[origin_of(self.value)] Token:\n        return self.value\n",
    );
    assert!(matches!(
        noncopyable,
        TypeError::TraitMethodMismatch { method, .. } if method == "__next__"
    ));

    let unsatisfied_conditional = err(
        "trait IteratorContract:\n    comptime Element: Movable\n    def __next__(mut self) -> Self.Element: ...\n\n@fieldwise_init\nstruct NeverCopyable(Copyable where False, Movable):\n    var value: Int\n\n@fieldwise_init\nstruct ConditionalIterator(IteratorContract):\n    comptime Element = NeverCopyable\n    var value: NeverCopyable\n    def __next__(mut self) -> ref[origin_of(self.value)] NeverCopyable:\n        return self.value\n",
    );
    assert!(
        matches!(
            &unsatisfied_conditional,
            TypeError::TraitMethodMismatch { method, .. } if method == "__next__"
        ),
        "got {unsatisfied_conditional:?}"
    );

    let unavailable_copy_initializer = err(
        "trait IteratorContract:\n    comptime Element: Movable\n    def __next__(mut self) -> Self.Element: ...\n\nstruct UnavailableCopy(Movable):\n    var value: Int\n    def __init__(out self, value: Int):\n        self.value = value\n    def __init__(out self, *, copy: Self) where False:\n        self.value = copy.value\n\n@fieldwise_init\nstruct UnavailableCopyIterator(IteratorContract):\n    comptime Element = UnavailableCopy\n    var value: UnavailableCopy\n    def __next__(mut self) -> ref[origin_of(self.value)] UnavailableCopy:\n        return self.value\n",
    );
    assert!(matches!(
        unavailable_copy_initializer,
        TypeError::TraitMethodMismatch { ref method, .. } if method == "__next__"
    ));

    let mismatched = err(
        "trait IteratorContract:\n    comptime Element: Movable\n    def __next__(mut self) -> Self.Element: ...\n\n@fieldwise_init\nstruct BadIterator(IteratorContract):\n    comptime Element = Int\n    var value: Bool\n    def __next__(mut self) -> ref[origin_of(self.value)] Bool:\n        return self.value\n",
    );
    assert!(matches!(
        mismatched,
        TypeError::TraitMethodMismatch { ref method, .. } if method == "__next__"
    ));
}

#[test]
fn value_next_cannot_refine_a_reference_requirement() {
    let error = err(
        "trait ReferenceIteratorContract:\n    def __next__(mut self) -> ref[UnsafeAnyOrigin] Int: ...\n\n@fieldwise_init\nstruct ValueIterator(ReferenceIteratorContract):\n    var value: Int\n    def __next__(mut self) -> Int:\n        return self.value\n",
    );
    assert!(matches!(
        error,
        TypeError::TraitMethodMismatch { ref method, .. } if method == "__next__"
    ));
}

#[test]
fn a_registered_iterator_trait_is_not_a_shallow_builtin_marker() {
    let error = err(
        "trait Iterator:\n    comptime Element: Movable\n    def __next__(mut self) -> Self.Element: ...\n\n@fieldwise_init\nstruct MissingNext(Iterator):\n    comptime Element = Int\n    var value: Int\n",
    );
    assert!(matches!(
        error,
        TypeError::MissingTraitMethod { ref trait_name, ref method, .. }
            if trait_name == "Iterator" && method == "__next__"
    ));
}

#[test]
fn trait_method_requirements_preserve_and_enforce_raises_effects() {
    let declarations = "trait Fallible:\n    def run(self) raises -> Int: ...\n\n@fieldwise_init\nstruct Failure(Fallible):\n    var code: Int\n    def run(self) raises -> Int:\n        raise \"failed\"\n        return self.code\n";

    ok(&format!(
        "{declarations}\ndef invoke[T: Fallible](value: T) raises -> Int:\n    return value.run()\n"
    ));
    assert!(matches!(
        err(&format!(
            "{declarations}\ndef invoke[T: Fallible](value: T) -> Int:\n    return value.run()\n"
        )),
        TypeError::UnhandledRaise(_)
    ));
}

#[test]
fn bounded_trait_overloads_select_the_matching_effect_contract() {
    let declarations = "trait Picks:\n    def pick(self, value: Int) -> Int: ...\n    def pick(self, value: StringLiteral) raises -> Int: ...\n";

    ok(&format!(
        "{declarations}\ndef safe[T: Picks](value: T) -> Int:\n    return value.pick(7)\n\ndef fallible[T: Picks](value: T) raises -> Int:\n    return value.pick(\"boom\")\n"
    ));
    assert!(matches!(
        err(&format!(
            "{declarations}\ndef unsafe[T: Picks](value: T) -> Int:\n    return value.pick(\"boom\")\n"
        )),
        TypeError::UnhandledRaise(_)
    ));
}

#[test]
fn trait_method_conformance_may_narrow_but_not_widen_effects() {
    ok(
        "trait Fallible:\n    def run(self) raises -> Int: ...\n\n@fieldwise_init\nstruct Safe(Fallible):\n    var value: Int\n    def run(self) -> Int:\n        return self.value\n",
    );

    let error = err(
        "trait Infallible:\n    def run(self) -> Int: ...\n\n@fieldwise_init\nstruct Unsafe(Infallible):\n    var value: Int\n    def run(self) raises -> Int:\n        raise \"failed\"\n        return self.value\n",
    );
    assert!(matches!(error, TypeError::TraitMethodMismatch { .. }));
}

#[test]
fn typed_trait_method_effects_reject_a_wider_error_family() {
    ok(
        "@fieldwise_init\nstruct ValidationError:\n    var reason: StringLiteral\n\ntrait Validates:\n    def validate(self) raises ValidationError -> Int: ...\n\n@fieldwise_init\nstruct Validator(Validates):\n    var value: Int\n    def validate(self) raises ValidationError -> Int:\n        raise ValidationError(\"bad\")\n        return self.value\n",
    );

    let error = err(
        "@fieldwise_init\nstruct ValidationError:\n    var reason: StringLiteral\n\ntrait Validates:\n    def validate(self) raises ValidationError -> Int: ...\n\n@fieldwise_init\nstruct Validator(Validates):\n    var value: Int\n    def validate(self) raises -> Int:\n        raise \"bad\"\n        return self.value\n",
    );
    assert!(matches!(error, TypeError::TraitMethodMismatch { .. }));

    let error = err(
        "@fieldwise_init\nstruct ValidationError:\n    var reason: StringLiteral\n\ntrait Fallible:\n    def run(self) raises -> Int: ...\n\n@fieldwise_init\nstruct Typed(Fallible):\n    var value: Int\n    def run(self) raises ValidationError -> Int:\n        raise ValidationError(\"bad\")\n        return self.value\n",
    );
    assert!(matches!(error, TypeError::TraitMethodMismatch { .. }));
}

#[test]
fn typed_call_effects_require_the_same_enclosing_error_type() {
    let declarations = "@fieldwise_init\nstruct ValidationError:\n    var reason: StringLiteral\n\ndef validate() raises ValidationError -> Int:\n    raise ValidationError(\"bad\")\n    return 0\n";
    ok(&format!(
        "{declarations}\ndef caller() raises ValidationError -> Int:\n    return validate()\n"
    ));
    assert!(matches!(
        err(&format!(
            "{declarations}\ndef caller() raises -> Int:\n    return validate()\n"
        )),
        TypeError::RaiseTypeMismatch { .. }
    ));
}

#[test]
fn accepts_trait_comptime_type_member_conformance() {
    ok(
        "trait HasElement:\n    comptime Element: AnyType\n\n@fieldwise_init\nstruct Box[T: AnyType](HasElement):\n    comptime Element = Self.T\n    var value: Self.T\n",
    );
}

#[test]
fn accepts_trait_comptime_value_member_conformance() {
    ok(
        "trait Fixed:\n    comptime size: Int\n\n@fieldwise_init\nstruct Buffer[size: Int](Fixed):\n    comptime size = Self.size\n    var tag: Int\n",
    );
}

#[test]
fn rejects_missing_trait_comptime_member() {
    let e = err(
        "trait HasElement:\n    comptime Element: AnyType\n\n@fieldwise_init\nstruct Box[T: AnyType](HasElement):\n    var value: Self.T\n",
    );
    assert!(
        matches!(
            &e,
            TypeError::MissingTraitComptimeMember { member, .. } if member == "Element"
        ),
        "got {:?}",
        e
    );
}

#[test]
fn rejects_trait_comptime_member_kind_mismatch() {
    let e = err(
        "trait HasElement:\n    comptime Element: AnyType\n\n@fieldwise_init\nstruct Box(HasElement):\n    comptime Element = 1\n    var value: Int\n",
    );
    assert!(
        matches!(
            &e,
            TypeError::TraitComptimeMemberMismatch { member, .. } if member == "Element"
        ),
        "got {:?}",
        e
    );
}

#[test]
fn rejects_associated_value_in_type_position() {
    let e = err(
        "trait Fixed:\n    comptime size: Int\n\ndef bad[C: Fixed](c: C) -> C.size:\n    return 0\n",
    );
    assert!(
        matches!(
            &e,
            TypeError::NoSuchAssociatedType { member, .. } if member == "size"
        ),
        "got {:?}",
        e
    );
}

#[test]
fn composes_inherited_associated_type_bounds() {
    ok(
        "trait HasElement:\n    comptime Element: AnyType\n\ntrait HasCopyableElement:\n    comptime Element: Copyable\n\ntrait Collection(HasElement, HasCopyableElement):\n    def size(self) -> Int: ...\n\n@fieldwise_init\nstruct IntCollection(Collection):\n    comptime Element = Int\n    var value: Int\n    def size(self) -> Int:\n        return 1\n",
    );
}

#[test]
fn checks_composed_associated_type_bounds() {
    ok(
        "trait Container:\n    comptime Element: Writable & Copyable & ImplicitlyDeletable\n\n@fieldwise_init\nstruct IntContainer(Container):\n    comptime Element = Int\n    var value: Int\n",
    );

    let e = err(
        "trait Container:\n    comptime Element: Writable & Copyable\n\n@fieldwise_init\nstruct Opaque:\n    var value: Int\n\n@fieldwise_init\nstruct Bad(Container):\n    comptime Element = Opaque\n    var value: Opaque\n",
    );
    assert!(matches!(
        e,
        TypeError::TraitComptimeMemberMismatch { member, .. } if member == "Element"
    ));
}

#[test]
fn rejects_conflicting_inherited_associated_member_kinds() {
    let e = err(
        "trait TypeMember:\n    comptime Item: AnyType\n\ntrait ValueMember:\n    comptime Item: Int\n\ntrait Invalid(TypeMember, ValueMember):\n    def marker(self): ...\n",
    );
    assert!(
        matches!(e, TypeError::Unsupported(message) if message.contains("conflicting inherited associated member 'Item'"))
    );
}

#[test]
fn applies_conditional_conformance_after_specialization() {
    ok(
        "@fieldwise_init\nstruct Wrapper[T: AnyType](Writable where conforms_to(T, Writable)):\n    var value: Self.T\n\ndef accept[T: Writable](value: T):\n    pass\n\ndef main():\n    accept(Wrapper[Int](1))\n",
    );

    let e = err(
        "@fieldwise_init\nstruct Opaque:\n    pass\n\n@fieldwise_init\nstruct Wrapper[T: AnyType](Writable where conforms_to(T, Writable)):\n    var value: Self.T\n\ndef accept[T: Writable](value: T):\n    pass\n\ndef main():\n    accept(Wrapper[Opaque](Opaque()))\n",
    );
    assert!(
        matches!(&e, TypeError::TraitNotSatisfied { trait_name, .. } if trait_name == "Writable"),
        "got {e:?}"
    );
}

#[test]
fn conditional_conformance_accepts_value_predicates() {
    ok(
        "@fieldwise_init\nstruct Window[size: Int](Writable where size > 0):\n    var value: Int\n\ndef accept[T: Writable](value: T):\n    pass\n\ndef main():\n    accept(Window[1](4))\n",
    );
    let e = err(
        "@fieldwise_init\nstruct Window[size: Int](Writable where size > 0):\n    var value: Int\n\ndef accept[T: Writable](value: T):\n    pass\n\ndef main():\n    accept(Window[0](4))\n",
    );
    assert!(matches!(
        e,
        TypeError::TraitNotSatisfied { trait_name, .. } if trait_name == "Writable"
    ));
}

#[test]
fn rejects_method_not_required_by_any_bound() {
    // `waddle` is not required by `Quackable`, so it can't be called on a `T`.
    let e = err(
        "trait Quackable:\n    def quack(self) -> StringLiteral:\n        ...\n\ndef f[T: Quackable](x: T) -> StringLiteral:\n    return x.waddle()\n",
    );
    assert!(matches!(e, TypeError::NoSuchMethod { .. }), "got {:?}", e);
}

#[test]
fn rejects_unknown_trait_in_conformance_list() {
    let e = err("@fieldwise_init\nstruct D(Bogus):\n    var n: Int\n");
    assert_eq!(e, TypeError::UnknownTrait("Bogus".into()));
}

#[test]
fn rejects_bounded_type_parameter_forwarded_to_stronger_bound() {
    // `T: AnyType` cannot be passed where `Quackable` is required.
    let e = err(
        "trait Quackable:\n    def quack(self) -> StringLiteral:\n        ...\n\ndef needs[U: Quackable](x: U) -> StringLiteral:\n    return x.quack()\n\ndef weak[T: AnyType](x: T) -> StringLiteral:\n    return needs(x)\n",
    );
    assert!(
        matches!(e, TypeError::TraitNotSatisfied { .. }),
        "got {:?}",
        e
    );
}

#[test]
fn accepts_forwarding_a_bounded_type_parameter() {
    // `T: Quackable` may be forwarded to a `[U: Quackable]` parameter.
    ok(
        "trait Quackable:\n    def quack(self) -> StringLiteral:\n        ...\n\ndef needs[U: Quackable](x: U) -> StringLiteral:\n    return x.quack()\n\ndef fwd[T: Quackable](x: T) -> StringLiteral:\n    return needs(x)\n",
    );
}

#[test]
fn rejects_duplicate_trait_declaration() {
    let e = err(
        "trait Q:\n    def m(self) -> Int:\n        ...\n\ntrait Q:\n    def n(self) -> Int:\n        ...\n",
    );
    assert_eq!(e, TypeError::Redeclaration("Q".into()));
}

// --- Value parameters + comptime (Phase 2) ---

const FIXEDBUF: &str = "@fieldwise_init\nstruct FixedBuffer[size: Int]:\n    var tag: Int\n\n    def capacity(self) -> Int:\n        return Self.size\n";

#[test]
fn accepts_value_parameter_struct_and_self_read() {
    ok(&format!(
        "{FIXEDBUF}var b: FixedBuffer[8] = FixedBuffer[8](0)\nvar c: Int = b.capacity()\n"
    ));
}

#[test]
fn accepts_comptime_arithmetic_argument() {
    ok(&format!(
        "{FIXEDBUF}var b: FixedBuffer[2 + 3 * 2] = FixedBuffer[8](0)\n"
    ));
}

#[test]
fn accepts_value_parameter_function() {
    ok("def doubled[n: Int]() -> Int:\n    return n * 2\n\nvar d: Int = doubled[21]()\n");
}

#[test]
fn accepts_comptime_constant_as_value_argument() {
    ok(&format!(
        "comptime N = 4 + 4\n{FIXEDBUF}var b: FixedBuffer[N] = FixedBuffer[N](0)\nvar n: Int = N\n"
    ));
}

#[test]
fn accepts_explicit_type_argument() {
    ok(
        "@fieldwise_init\nstruct Box[T: Copyable & Movable]:\n    var v: Self.T\n\nvar b: Box[Int] = Box[Int](5)\n",
    );
}

#[test]
fn distinct_value_arguments_are_distinct_types() {
    // `FixedBuffer[5]` and `FixedBuffer[6]` are different types.
    let e = err(&format!(
        "{FIXEDBUF}var b: FixedBuffer[5] = FixedBuffer[6](0)\n"
    ));
    assert!(matches!(e, TypeError::TypeMismatch { .. }), "got {:?}", e);
}

#[test]
fn rejects_uninferable_value_parameter() {
    // A value parameter cannot be inferred; it must be supplied explicitly.
    let e = err(&format!(
        "{FIXEDBUF}var b: FixedBuffer[4] = FixedBuffer(0)\n"
    ));
    assert!(
        matches!(&e, TypeError::CannotInferTypeParam { param, .. } if param == "size"),
        "got {:?}",
        e
    );
}

#[test]
fn accepts_float_value_parameter_type() {
    ok("@fieldwise_init\nstruct Measurement[value: Float64]:\n    var tag: Int\n");
}

#[test]
fn rejects_non_comptime_value_argument() {
    // A runtime variable is not a compile-time constant.
    let e = err(&format!(
        "{FIXEDBUF}var x: Int = 3\nvar b: FixedBuffer[x] = FixedBuffer[x](0)\n"
    ));
    assert!(matches!(e, TypeError::NotComptime(_)), "got {:?}", e);
}

#[test]
fn rejects_type_argument_for_value_parameter() {
    let e = err(&format!(
        "{FIXEDBUF}var b: FixedBuffer[Int] = FixedBuffer[Int](0)\n"
    ));
    assert!(matches!(e, TypeError::TypeMismatch { .. }), "got {:?}", e);
}

#[test]
fn rejects_wrong_parameter_count() {
    let e = err(&format!(
        "{FIXEDBUF}var b: FixedBuffer[8, 9] = FixedBuffer[8, 9](0)\n"
    ));
    assert!(
        matches!(e, TypeError::WrongTypeArgCount { .. }),
        "got {:?}",
        e
    );
}

#[test]
fn rejects_non_comptime_constant_definition() {
    // A `comptime NAME = <runtime value>` is now rejected by the compile-time
    // elaborator (the comptime validator), not the checker directly.
    let program = mojito::parse("var x: Int = 1\ncomptime N = x\n").unwrap();
    assert!(mojito::elaborate(program).is_err());
}

#[test]
fn rejects_explicit_params_on_non_generic_function() {
    let e = err("def f(x: Int) -> Int:\n    return x\n\nvar y: Int = f[Int](1)\n");
    assert!(
        matches!(e, TypeError::WrongTypeArgCount { expected: 0, .. }),
        "got {:?}",
        e
    );
}

// --- SIMD ---

#[test]
fn accepts_simd_construction_ops_and_index() {
    ok(
        "var v: SIMD[DType.int32, 4] = SIMD[DType.int32, 4](1, 2, 3, 4)\nvar w: SIMD[DType.int32, 4] = v + v * v\nvar lane: Int32 = w[0]\n",
    );
}

#[test]
fn accepts_simd_splat_construction() {
    ok("var v: SIMD[DType.uint8, 8] = SIMD[DType.uint8, 8](7)\n");
}

#[test]
fn accepts_simd_comparison_mask() {
    ok(
        "var v: SIMD[DType.float32, 4] = SIMD[DType.float32, 4](1.0, 2.0, 3.0, 4.0)\nvar m: SIMD[DType.bool, 4] = v < v\n",
    );
}

#[test]
fn accepts_literal_splat_in_simd_operator() {
    ok(
        "var v: SIMD[DType.int32, 4] = SIMD[DType.int32, 4](1, 2, 3, 4)\nvar w: SIMD[DType.int32, 4] = v + 100\n",
    );
}

#[test]
fn accepts_scalar_alias_types_and_construction() {
    ok("var a: Int8 = Int8(5)\nvar b: UInt32 = UInt32(9)\nvar c: Float32 = Float32(1.5)\n");
}

#[test]
fn byte_alias_contextually_materializes_with_integer_wrapping() {
    ok("var byte: Byte = 255\nvar same: UInt8 = byte\n");
    ok("var wrapped_zero: Byte = 256\nvar wrapped_max: Byte = -1\n");
}

#[test]
fn accepts_comptime_simd_width() {
    ok("comptime W = 2 * 2\nvar v: SIMD[DType.int32, W] = SIMD[DType.int32, W](1, 2, 3, 4)\n");
}

#[test]
fn rejects_non_power_of_two_width() {
    let e = err("var v: SIMD[DType.int32, 3] = SIMD[DType.int32, 3](1, 2, 3)\n");
    assert!(matches!(e, TypeError::BadSimdWidth(_)), "got {:?}", e);
}

#[test]
fn rejects_unknown_dtype() {
    let e = err("var v: SIMD[DType.foo, 4] = SIMD[DType.foo, 4](1)\n");
    assert!(matches!(e, TypeError::BadDtype(_)), "got {:?}", e);
}

#[test]
fn rejects_wrong_simd_element_count() {
    let e = err("var v: SIMD[DType.int32, 4] = SIMD[DType.int32, 4](1, 2)\n");
    assert!(
        matches!(e, TypeError::SimdArity { width: 4, got: 2 }),
        "got {:?}",
        e
    );
}

#[test]
fn rejects_mixed_dtype_operands() {
    let e = err(
        "var a: SIMD[DType.int32, 4] = SIMD[DType.int32, 4](1, 2, 3, 4)\nvar b: SIMD[DType.int64, 4] = SIMD[DType.int64, 4](1, 2, 3, 4)\nvar c: SIMD[DType.int32, 4] = a + b\n",
    );
    assert!(matches!(e, TypeError::BadOperator { .. }), "got {:?}", e);
}

#[test]
fn rejects_division_on_integer_simd() {
    let e = err(
        "var a: SIMD[DType.int32, 4] = SIMD[DType.int32, 4](1, 2, 3, 4)\nvar b: SIMD[DType.int32, 4] = a / a\n",
    );
    assert!(matches!(e, TypeError::BadOperator { .. }), "got {:?}", e);
}

#[test]
fn rejects_indexing_a_non_simd() {
    let e = err("var x: Int = 5\nvar y: Int = x[0]\n");
    assert!(matches!(e, TypeError::NotIndexable(_)), "got {:?}", e);
}

#[test]
fn accepts_simd_negation_select_and_reductions() {
    ok(
        "def main():\n    var v = SIMD[DType.int32, 4](1, 2, 3, 4)\n    var n: SIMD[DType.int32, 4] = -v\n    var total: Int32 = v.reduce_add()\n    var m = v < SIMD[DType.int32, 4](3, 3, 3, 3)\n    var blended: SIMD[DType.int32, 4] = m.select(v, -v)\n    var splatted: SIMD[DType.int32, 4] = m.select(v, 0)\n    var all_set: Bool = m.reduce_and()\n    var any_set: Bool = m.reduce_or()\n    var i: Int = SIMD[DType.int, 2](1, 2).reduce_add()\n",
    );
}

#[test]
fn accepts_simd_dtype_cast() {
    ok(
        "def main():\n    var v = SIMD[DType.int32, 4](1, -2, 3, -300)\n    var u: SIMD[DType.uint8, 4] = v.cast[DType.uint8]()\n    var f: SIMD[DType.float32, 4] = v.cast[DType.float32]()\n    var back: SIMD[DType.int32, 4] = f.cast[DType.int32]()\n    var scalar: Int8 = Byte(200).cast[DType.int8]()\n",
    );
}

#[test]
fn rejects_bool_simd_dtype_casts() {
    // Bool dtype casts are deferred in both directions.
    assert!(matches!(
        err(
            "def main():\n    var v = SIMD[DType.int32, 2](1, 2)\n    var bad = v.cast[DType.bool]()\n"
        ),
        TypeError::TypeMismatch { .. }
    ));
    assert!(matches!(
        err(
            "def main():\n    var m = SIMD[DType.int32, 2](1, 2) < SIMD[DType.int32, 2](2, 2)\n    var bad = m.cast[DType.int32]()\n"
        ),
        TypeError::TypeMismatch { .. }
    ));
}

#[test]
fn accepts_simd_shuffle_masks() {
    ok(
        "def main():\n    var v = SIMD[DType.int32, 4](10, 20, 30, 40)\n    var r: SIMD[DType.int32, 4] = v.shuffle[3, 2, 1, 0]()\n    var pair: SIMD[DType.int32, 2] = v.shuffle[1, 1]()\n    var one: Int32 = v.shuffle[0]()\n",
    );
}

#[test]
fn rejects_bad_simd_shuffle_masks() {
    // A lane index beyond the receiver's width.
    assert!(matches!(
        err(
            "def main():\n    var v = SIMD[DType.int32, 2](1, 2)\n    var bad = v.shuffle[2, 0]()\n"
        ),
        TypeError::TypeMismatch { .. }
    ));
    // The mask length must itself be a valid SIMD width.
    assert!(matches!(
        err(
            "def main():\n    var v = SIMD[DType.int32, 4](1, 2, 3, 4)\n    var bad = v.shuffle[0, 1, 2]()\n"
        ),
        TypeError::BadSimdWidth(_)
    ));
    // Lane indices are compile-time values.
    assert!(err(
        "def main():\n    var v = SIMD[DType.int32, 2](1, 2)\n    var i = 1\n    var bad = v.shuffle[i]()\n"
    )
    .to_string()
    .contains("compile-time"));
}

#[test]
fn rejects_simd_mask_negation_and_shape_misuse() {
    // A bool mask does not negate.
    assert!(matches!(
        err(
            "def main():\n    var m = SIMD[DType.int32, 2](1, 2) < SIMD[DType.int32, 2](2, 2)\n    var bad = -m\n"
        ),
        TypeError::BadOperator { .. }
    ));
    // `select` lives on the mask, not a numeric vector.
    assert!(matches!(
        err("def main():\n    var v = SIMD[DType.int32, 2](1, 2)\n    var bad = v.select(v, v)\n"),
        TypeError::NoSuchMethod { .. }
    ));
    // The cases must share the mask's width.
    assert!(matches!(
        err(
            "def main():\n    var m = SIMD[DType.int32, 2](1, 2) < SIMD[DType.int32, 2](2, 2)\n    var bad = m.select(SIMD[DType.int32, 4](1, 2, 3, 4), 0)\n"
        ),
        TypeError::TypeMismatch { .. }
    ));
    // Bool reductions live on masks only.
    assert!(matches!(
        err("def main():\n    var v = SIMD[DType.int32, 2](1, 2)\n    var bad = v.reduce_and()\n"),
        TypeError::NoSuchMethod { .. }
    ));
}

#[test]
fn accepts_runtime_scalar_dtype_conversions() {
    // Explicit construction converts: runtime integers wrap to any integer
    // width and convert to float lanes; floats adjust precision.
    ok(
        "def main():\n    var i = 300\n    var b: Byte = Byte(i)\n    var u: UInt32 = UInt32(UInt(70000))\n    var w: Int32 = Int32(b)\n    var f: Float32 = Float32(2.75)\n    var g: Float32 = Float32(i)\n    var v: SIMD[DType.uint8, 4] = SIMD[DType.uint8, 4](i, 1, 2, 3)\n",
    );
}

#[test]
fn accepts_intable_sources_for_integer_lanes() {
    // Integer-Scalar construction accepts any Intable value: a bounded
    // parameter or a conforming struct, converted through `__int__`.
    ok(
        "@fieldwise_init\nstruct W(Intable):\n    var n: Int\n\n    def __int__(self) -> Int:\n        return self.n\n\ndef to_byte[T: Intable](x: T) -> Byte:\n    return Byte(x)\n\ndef main():\n    var b = to_byte(W(261))\n    var c = Byte(W(300))\n",
    );
}

#[test]
fn rejects_float_sources_for_integer_lanes() {
    // A float source spells its truncation explicitly (`Int(x)` first).
    assert!(matches!(
        err("def main():\n    var b = Byte(1.5)\n"),
        TypeError::TypeMismatch { .. }
    ));
    assert!(matches!(
        err("def main():\n    var f = 2.5\n    var b = Int8(f)\n"),
        TypeError::TypeMismatch { .. }
    ));
    assert!(matches!(
        err("def main():\n    var g: Float32 = Float32(1.0)\n    var b = Int8(g)\n"),
        TypeError::TypeMismatch { .. }
    ));
}

#[test]
fn rejects_float_literal_for_integer_dtype() {
    let e = err("var v: SIMD[DType.int32, 2] = SIMD[DType.int32, 2](1.5, 2.5)\n");
    assert!(matches!(e, TypeError::TypeMismatch { .. }), "got {:?}", e);
}

#[test]
fn distinct_simd_widths_are_distinct_types() {
    let e = err("var v: SIMD[DType.int32, 4] = SIMD[DType.int32, 2](1, 2)\n");
    assert!(matches!(e, TypeError::TypeMismatch { .. }), "got {:?}", e);
}

// --- Path-sensitive return checking ---

#[test]
fn accepts_return_on_all_if_paths() {
    // Every arm (including else) returns.
    ok(
        "def sign(n: Int) -> Int:\n    if n > 0:\n        return 1\n    elif n < 0:\n        return -1\n    else:\n        return 0\n",
    );
}

#[test]
fn accepts_trailing_return_after_a_loop() {
    ok("def f(n: Int) -> Int:\n    for i in range(n):\n        return i\n    return -1\n");
}

#[test]
fn accepts_no_return_when_return_type_is_none() {
    // A function without a declared return type may fall off the end (yields None).
    ok("def noop(x: Int):\n    pass\n");
}

#[test]
fn rejects_falling_off_the_end() {
    let e = err("def f() -> Int:\n    var x: Int = 1\n");
    assert_eq!(e, TypeError::MissingReturn("f".into()));
}

#[test]
fn rejects_if_without_else() {
    // No `else`, so a path falls through without returning.
    let e = err("def f(n: Int) -> Int:\n    if n > 0:\n        return 1\n");
    assert_eq!(e, TypeError::MissingReturn("f".into()));
}

#[test]
fn rejects_return_only_inside_a_loop() {
    // Conservative: a loop may run zero times, so it doesn't guarantee a return.
    let e = err("def f(n: Int) -> Int:\n    for i in range(n):\n        return i\n");
    assert_eq!(e, TypeError::MissingReturn("f".into()));
}

#[test]
fn rejects_method_missing_return() {
    let e = err(
        "@fieldwise_init\nstruct P:\n    var x: Int\n\n    def get(self) -> Int:\n        pass\n",
    );
    assert_eq!(e, TypeError::MissingReturn("get".into()));
}

// --- Exceptions ---

#[test]
fn accepts_raise_and_try_except() {
    ok("try:\n    raise Error(\"boom\")\nexcept e:\n    var x: Int = 1\n");
}

#[test]
fn accepts_string_shorthand_raise() {
    ok("def boom() raises:\n    raise \"oops\"\n");
}

#[test]
fn accepts_try_else_finally_and_bound_error() {
    // `except e:` binds `e: Error`, usable (e.g. re-raised).
    ok(
        "def reraiser() raises:\n    try:\n        raise \"x\"\n    except e:\n        raise e^\n    else:\n        pass\n    finally:\n        pass\n",
    );
}

#[test]
fn enforces_raises_effect_at_call_sites() {
    let source = "def boom() raises -> Int:\n    raise \"deep\"\n    return 0\n\ndef caller() -> Int:\n    return boom()\n";
    assert!(matches!(err(source), TypeError::UnhandledRaise(_)));
    ok(
        "def boom() raises -> Int:\n    raise \"deep\"\n    return 0\n\ndef caller() raises -> Int:\n    return boom()\n",
    );
}

#[test]
fn enforces_raises_effect_at_instance_and_static_method_calls() {
    let declarations = "@fieldwise_init\nstruct Bomb:\n    var code: Int\n    def explode(self) raises -> Int:\n        raise \"boom\"\n    @staticmethod\n    def static_explode() raises -> Int:\n        raise \"boom\"\n";
    assert!(matches!(
        err(&format!(
            "{declarations}\ndef caller(b: Bomb) -> Int:\n    return b.explode()\n"
        )),
        TypeError::UnhandledRaise(_)
    ));
    assert!(matches!(
        err(&format!(
            "{declarations}\ndef caller() -> Int:\n    return Bomb.static_explode()\n"
        )),
        TypeError::UnhandledRaise(_)
    ));
    ok(&format!(
        "{declarations}\ndef caller(b: Bomb) raises -> Int:\n    return b.explode()\n"
    ));
}

#[test]
fn enforces_effects_after_free_overload_and_callable_selection() {
    let overloads = "def select(value: Int) -> Int:\n    return value\n\ndef select(value: StringLiteral) raises -> Int:\n    raise value\n    return 0\n";
    ok(&format!(
        "{overloads}\ndef safe() -> Int:\n    return select(7)\n"
    ));
    assert!(matches!(
        err(&format!(
            "{overloads}\ndef unsafe() -> Int:\n    return select(\"boom\")\n"
        )),
        TypeError::UnhandledRaise(_)
    ));

    let callable = "def boom() raises -> Int:\n    raise \"boom\"\n    return 0\n\ndef invoke(callback: def() raises -> Int) raises -> Int:\n    return callback()\n";
    ok(callable);
    assert!(matches!(
        err(
            "def boom() raises -> Int:\n    raise \"boom\"\n    return 0\n\ndef invoke(callback: def() -> Int) -> Int:\n    return callback()\n\ndef main():\n    print(invoke(boom))\n"
        ),
        TypeError::TypeMismatch { .. }
    ));
}

#[test]
fn accepts_error_typed_variable() {
    ok("var e: Error = Error(\"msg\")\n");
}

#[test]
fn accepts_transfer_sigil_as_identity() {
    ok("var x: Int = 1\nvar y: Int = x^\n");
}

#[test]
fn accepts_typed_errors_and_infers_the_except_binding() {
    ok(
        "@fieldwise_init\nstruct ValidationError:\n    var field: StringLiteral\n    var reason: StringLiteral\n\ndef validate(name: StringLiteral) raises ValidationError -> Int:\n    if name == \"\":\n        raise ValidationError(\"name\", \"empty\")\n    return 1\n\ndef main():\n    try:\n        var ignored = validate(\"\")\n    except error:\n        var field: StringLiteral = error.field\n",
    );
}

#[test]
fn rejects_an_error_that_differs_from_the_declared_type() {
    let e = err(
        "@fieldwise_init\nstruct ValidationError:\n    var reason: StringLiteral\n\ndef bad() raises ValidationError:\n    raise Error(\"wrong family\")\n",
    );
    assert!(
        matches!(e, TypeError::RaiseTypeMismatch { .. }),
        "got {e:?}"
    );
}

#[test]
fn never_is_a_bottom_type_and_raises_never_is_nonraising() {
    ok(
        "def stop() raises -> Never:\n    raise \"stop\"\n\ndef choose() raises -> Int:\n    return stop()\n\ndef safe() raises Never -> Int:\n    return 7\n\ndef caller() -> Int:\n    return safe()\n",
    );
}

#[test]
fn infers_parametric_error_types_from_callable_arguments() {
    ok(
        "@fieldwise_init\nstruct ValidationError:\n    var reason: StringLiteral\n\ndef run_action[E: AnyType](action: def() raises E -> Int) raises E -> Int:\n    return action()\n\ndef safe() -> Int:\n    return 7\n\ndef fail() raises ValidationError -> Int:\n    raise ValidationError(\"failed\")\n\ndef safe_caller() -> Int:\n    return run_action(safe)\n\ndef typed_caller() raises ValidationError -> Int:\n    return run_action(fail)\n",
    );
}

#[test]
fn rejects_error_constructor_with_non_string() {
    let e = err("var e: Error = Error(5)\n");
    assert!(matches!(e, TypeError::TypeMismatch { .. }), "got {:?}", e);
}

#[test]
fn except_binding_is_scoped_to_its_clause() {
    // `e` from `except e:` does not leak past the try statement.
    let e = err("try:\n    raise \"x\"\nexcept e:\n    pass\nvar leaked: Error = e\n");
    assert_eq!(e, TypeError::UndefinedVariable("e".into()));
}

// --- Imports (parsed, not resolved) ---

#[test]
fn import_statements_are_accepted_as_no_ops() {
    // Imports type-check (they are no-ops); real code alongside still runs.
    ok("import mypackage.mymodule as mm\nfrom other import a, b as c\nvar x: Int = 1 + 2\n");
}

#[test]
fn imported_names_are_not_made_available() {
    // Since imports are not resolved, an imported name is still undefined.
    let e = err("from mymodule import foo\nvar x: Int = foo\n");
    assert_eq!(e, TypeError::UndefinedVariable("foo".into()));
}

// --- print ---

#[test]
fn accepts_print_of_various_values() {
    ok(
        "@fieldwise_init\nstruct P(Writable):\n    var x: Int\n    def write_to(self, mut writer: Some[Writer]):\n        writer.write(self.x)\n\nprint(1, \"a\", True, 3.5, None, P(5))\nprint()\n",
    );
}

#[test]
fn rejects_printing_a_function() {
    let e = err("def f() -> Int:\n    return 1\n\nprint(f)\n");
    assert!(matches!(e, TypeError::TypeMismatch { .. }), "got {:?}", e);
}

// --- Builtins: StringLiteral / abs / min / max / round / len ---

#[test]
fn accepts_string_and_numeric_builtins() {
    ok(
        "var s: StringLiteral = \"n=\" + String(42)\nvar a: Int = abs(-7)\nvar f: Float64 = abs(-2.5)\nvar lo: Int = min(3, 8)\nvar hi: Float64 = max(1.0, 2.0)\nvar r: Float64 = round(3.7)\nvar n: Int = len(\"hello\")\n",
    );
}

#[test]
fn abs_preserves_the_numeric_type() {
    // abs of a Float64 stays Float64 (would fail to bind to Int if it returned Int).
    ok("var x: Float64 = 1.5\nvar y: Float64 = abs(x)\n");
    let e = err("var x: Float64 = 1.5\nvar y: Int = abs(x)\n");
    assert!(matches!(e, TypeError::TypeMismatch { .. }), "got {:?}", e);
}

#[test]
fn rejects_stringify_of_non_stringable() {
    let e = err("def f() -> Int:\n    return 1\n\nvar s: StringLiteral = String(f)\n");
    assert!(matches!(e, TypeError::TypeMismatch { .. }), "got {:?}", e);
}

#[test]
fn rejects_len_of_non_string() {
    let e = err("var n: Int = len(5)\n");
    assert!(matches!(e, TypeError::TypeMismatch { .. }), "got {:?}", e);
}

#[test]
fn rejects_min_mixing_concrete_types() {
    let e = err("var i: Int = 1\nvar u: UInt = UInt(2)\nvar m: Int = min(i, u)\n");
    assert!(matches!(e, TypeError::BadOperator { .. }), "got {:?}", e);
}

#[test]
fn rejects_round_of_an_int() {
    let e = err("var r: Float64 = round(5)\n");
    assert!(matches!(e, TypeError::TypeMismatch { .. }), "got {:?}", e);
}

#[test]
fn rejects_builtin_arity_mismatch() {
    assert!(matches!(
        err("var a: Int = abs(1, 2)\n"),
        TypeError::ArityMismatch { .. }
    ));
    assert!(matches!(
        err("var a: Int = min(1)\n"),
        TypeError::ArityMismatch { .. }
    ));
}

// --- List (Step 1: construction / read / iterate / len) ---

#[test]
fn accepts_list_construction_forms() {
    ok(
        "var a: List[Int] = [1, 2, 3]\nvar b: List[Int] = List[Int](1, 2)\nvar c: List[Int] = List(4, 5)\nvar d: List[Int] = List[Int]()\n",
    );
}

#[test]
fn infers_list_element_type_with_widening() {
    ok("var xs: List[Float64] = [1, 2.0, 3]\nvar ys: List[StringLiteral] = [\"a\", \"b\"]\n");
}

#[test]
fn collection_literal_annotations_solve_only_direct_type_holes() {
    ok(
        "var inferred_list: List[_] = [1, 2]\nvar bare_list: List = [3, 4]\nvar inferred_set: Set[_] = {1, 2}\nvar inferred_dict: Dict[_, _] = {\"one\": 1}\nvar value_hole: Dict[StringLiteral, _] = {\"two\": 2}\n",
    );

    assert!(matches!(
        err("var empty: List[_] = []\n"),
        TypeError::CannotInferTypeParam { .. }
    ));
    let nested = err("var nested: List[List[_]] = [[1, 2]]\n");
    assert!(
        nested.to_string().contains("Infer")
            || nested.to_string().contains("concrete")
            || nested.to_string().contains("_"),
        "got {nested:?}"
    );
}

#[test]
fn accepts_len_index_and_iteration() {
    ok(
        "var xs: List[Int] = [10, 20, 30]\nvar n: Int = len(xs)\nvar first: Int = xs[0]\nvar sum: Int = 0\nfor x in xs:\n    sum = sum + x\n",
    );
}

#[test]
fn accepts_nested_and_struct_lists() {
    ok("var m: List[List[Int]] = [[1, 2], [3, 4]]\nvar v: Int = m[1][0]\n");
    ok(
        "@fieldwise_init\nstruct P:\n    var x: Int\n\nvar ps: List[P] = [P(1), P(2)]\nvar fx: Int = ps[0].x\n",
    );
}

#[test]
fn rejects_heterogeneous_list_literal() {
    let e = err("var xs: List[Int] = [1, \"a\"]\n");
    assert!(matches!(e, TypeError::TypeMismatch { .. }), "got {:?}", e);
}

#[test]
fn rejects_wrong_list_element_type() {
    let e = err("var xs: List[Int] = List[Int](1, True)\n");
    assert!(matches!(e, TypeError::TypeMismatch { .. }), "got {:?}", e);
}

#[test]
fn rejects_empty_inferred_list() {
    let e = err("var xs: List[Int] = List()\n");
    assert!(
        matches!(e, TypeError::CannotInferTypeParam { .. }),
        "got {:?}",
        e
    );
}

#[test]
fn rejects_uncontextualized_empty_list_literal() {
    let e = err("var xs = []\n");
    assert!(
        matches!(e, TypeError::CannotInferTypeParam { .. }),
        "got {:?}",
        e
    );
}

#[test]
fn rejects_non_int_list_index() {
    let e = err("var xs: List[Int] = [1, 2]\nvar y: Int = xs[True]\n");
    assert!(matches!(e, TypeError::TypeMismatch { .. }), "got {:?}", e);
}

// --- List (Steps 2 & 3: index-assign, append, pop) ---

#[test]
fn accepts_list_mutation() {
    ok(
        "var xs: List[Int] = List[Int]()\nxs.append(1)\nxs.append(2)\nxs[0] = 10\nvar last: Int = xs.pop()\n",
    );
}

#[test]
fn rejects_index_assign_wrong_type() {
    let e = err("var xs: List[Int] = [1]\nxs[0] = True\n");
    assert!(matches!(e, TypeError::TypeMismatch { .. }), "got {:?}", e);
}

#[test]
fn rejects_append_wrong_type() {
    let e = err("var xs: List[Int] = [1]\nxs.append(True)\n");
    assert!(matches!(e, TypeError::TypeMismatch { .. }), "got {:?}", e);
}

#[test]
fn rejects_mutating_a_non_variable_list() {
    let e = err("List[Int](1, 2).append(3)\n");
    assert_eq!(e, TypeError::MutationRequiresVariable("append".into()));
}

#[test]
fn accepts_simd_lane_write() {
    // A SIMD lane write `v[i] = e` takes a splatting literal or a same-dtype scalar.
    ok(
        "var v: SIMD[DType.int32, 4] = SIMD[DType.int32, 4](1, 2, 3, 4)\nv[0] = 9\nv[1] = Int32(5)\nv[2] += 100\n",
    );
    // ...but not a value of the wrong element kind, nor a whole vector.
    assert!(matches!(
        err("var v: SIMD[DType.int32, 2] = SIMD[DType.int32, 2](1, 2)\nv[0] = True\n"),
        TypeError::TypeMismatch { .. }
    ));
    assert!(matches!(
        err(
            "var v: SIMD[DType.int32, 2] = SIMD[DType.int32, 2](1, 2)\nv[0] = SIMD[DType.int32, 2](1, 2)\n"
        ),
        TypeError::TypeMismatch { .. }
    ));
}

#[test]
fn rejects_unknown_list_method() {
    let e = err("var xs: List[Int] = [1]\nxs.frobnicate()\n");
    assert!(matches!(e, TypeError::NoSuchMethod { .. }), "got {:?}", e);
}

#[test]
fn pop_returns_the_element_type() {
    ok("var xs: List[StringLiteral] = [\"a\", \"b\"]\nvar s: StringLiteral = xs.pop()\n");
}

// --- List (more methods: insert/remove/pop(i)/clear/reverse/extend/count/index) ---

#[test]
fn accepts_new_list_methods() {
    ok(
        "var xs: List[Int] = [1, 2, 3]\nxs.insert(1, 99)\nxs.remove(2)\nvar m: Int = xs.pop(0)\nxs.reverse()\nvar b: List[Int] = [4, 5]\nxs.extend(b)\nvar c: Int = xs.count(99)\nvar i: Int = xs.index(4)\nxs.clear()\n",
    );
}

#[test]
fn count_and_index_work_on_a_temporary() {
    ok("var c: Int = [1, 1, 2].count(1)\nvar i: Int = [3, 4, 5].index(4)\n");
}

#[test]
fn rejects_remove_on_non_equatable_elements() {
    let e = err(
        "@fieldwise_init\nstruct P:\n    var x: Int\n\nvar ps: List[P] = [P(1)]\nps.remove(P(1))\n",
    );
    assert!(
        matches!(&e, TypeError::TypeMismatch { context, .. } if context == "'remove'"),
        "got {:?}",
        e
    );
}

#[test]
fn rejects_extend_with_wrong_element_type() {
    let e = err("var a: List[Int] = [1]\nvar b: List[StringLiteral] = [\"x\"]\na.extend(b)\n");
    assert!(matches!(e, TypeError::TypeMismatch { .. }), "got {:?}", e);
}

#[test]
fn rejects_non_int_insert_index() {
    let e = err("var xs: List[Int] = [1]\nxs.insert(True, 2)\n");
    assert!(matches!(e, TypeError::TypeMismatch { .. }), "got {:?}", e);
}

#[test]
fn rejects_count_on_a_non_variable_when_temporary_is_not_equatable() {
    // count needs equatable elements even for a temporary.
    let e = err("@fieldwise_init\nstruct P:\n    var x: Int\n\nvar c: Int = [P(1)].count(P(1))\n");
    assert!(matches!(e, TypeError::TypeMismatch { .. }), "got {:?}", e);
}

// --- Membership: in / not in ---

#[test]
fn accepts_membership_on_list_and_string() {
    ok(
        "var xs: List[Int] = [1, 2, 3]\nvar a: Bool = 2 in xs\nvar b: Bool = 5 not in xs\nvar s: StringLiteral = \"hello\"\nvar c: Bool = \"ell\" in s\n",
    );
}

#[test]
fn membership_returns_bool() {
    let e = err("var xs: List[Int] = [1]\nvar n: Int = 1 in xs\n");
    assert!(matches!(e, TypeError::TypeMismatch { .. }), "got {:?}", e);
}

#[test]
fn rejects_membership_on_non_container() {
    let e = err("var a: Bool = 1 in 5\n");
    assert!(matches!(e, TypeError::BadOperator { .. }), "got {:?}", e);
}

#[test]
fn rejects_membership_element_type_mismatch() {
    let e = err("var xs: List[Int] = [1, 2]\nvar a: Bool = True in xs\n");
    assert!(matches!(e, TypeError::BadOperator { .. }), "got {:?}", e);
}

#[test]
fn rejects_membership_on_non_equatable_list() {
    let e = err(
        "@fieldwise_init\nstruct P:\n    var x: Int\n\nvar ps: List[P] = [P(1)]\nvar a: Bool = P(1) in ps\n",
    );
    assert!(matches!(e, TypeError::BadOperator { .. }), "got {:?}", e);
}

// --- Member-write: place assignment + mut self ---

const MUTPT: &str = "@fieldwise_init\nstruct Point:\n    var x: Int\n    var y: Int\n\n";

#[test]
fn accepts_field_and_nested_place_writes() {
    ok(&format!(
        "{MUTPT}var p: Point = Point(1, 2)\np.x = 10\np.y = p.x + 1\n"
    ));
    ok(
        "@fieldwise_init\nstruct In:\n    var v: Int\n\n@fieldwise_init\nstruct Out:\n    var inner: In\n\nvar o: Out = Out(In(5))\no.inner.v = 100\n",
    );
    ok(&format!(
        "{MUTPT}var ps: List[Point] = [Point(1, 1)]\nps[0].x = 9\n"
    ));
}

#[test]
fn accepts_mut_self_method_and_mutation() {
    ok(
        "@fieldwise_init\nstruct Counter:\n    var n: Int\n\n    def inc(mut self):\n        self.n = self.n + 1\n\nvar c: Counter = Counter(0)\nc.inc()\n",
    );
}

#[test]
fn accepts_list_method_through_a_field() {
    ok(
        "@fieldwise_init\nstruct Bag:\n    var items: List[Int]\n\nvar b: Bag = Bag([1])\nb.items.append(2)\nb.items[0] = 9\n",
    );
}

#[test]
fn rejects_field_write_on_read_only_self() {
    let e = err(
        "@fieldwise_init\nstruct S:\n    var x: Int\n\n    def bad(self):\n        self.x = 9\n",
    );
    assert_eq!(e, TypeError::ImmutableSelf);
}

#[test]
fn rejects_mut_self_call_on_a_temporary() {
    let e = err(
        "@fieldwise_init\nstruct C:\n    var n: Int\n\n    def inc(mut self):\n        self.n = self.n + 1\n\nC(0).inc()\n",
    );
    assert!(
        matches!(e, TypeError::InvalidAssignTarget(_)),
        "got {:?}",
        e
    );
}

#[test]
fn rejects_unknown_field_write() {
    let e = err(&format!("{MUTPT}var p: Point = Point(1, 2)\np.z = 3\n"));
    assert!(matches!(e, TypeError::NoSuchField { .. }), "got {:?}", e);
}

#[test]
fn rejects_wrong_type_field_write() {
    let e = err(&format!("{MUTPT}var p: Point = Point(1, 2)\np.x = True\n"));
    assert!(matches!(e, TypeError::TypeMismatch { .. }), "got {:?}", e);
}

#[test]
fn rejects_write_to_a_call_result() {
    let e = err(&format!(
        "{MUTPT}def mk() -> Point:\n    return Point(0, 0)\n\nmk().x = 5\n"
    ));
    assert!(
        matches!(e, TypeError::InvalidAssignTarget(_)),
        "got {:?}",
        e
    );
}

// --- Augmented assignment ---

#[test]
fn accepts_augmented_assignment_forms() {
    ok("var i: Int = 0\ni += 1\ni -= 1\ni *= 2\ni //= 2\ni %= 3\ni **= 2\n");
    ok("var s: StringLiteral = \"a\"\ns += \"b\"\n");
    ok(
        "@fieldwise_init\nstruct C:\n    var n: Int\n\n    def bump(mut self):\n        self.n += 1\n\nvar c: C = C(0)\nc.n += 5\n",
    );
    ok("var xs: List[Int] = [1, 2]\nxs[0] += 10\n");
}

#[test]
fn rejects_augmented_assignment_that_changes_type() {
    // `/` yields Float64, which does not fit an Int target.
    let e = err("var i: Int = 10\ni /= 2\n");
    assert!(
        matches!(&e, TypeError::TypeMismatch { context, .. } if context == "augmented assignment"),
        "got {:?}",
        e
    );
}

#[test]
fn rejects_augmented_assignment_bad_operands() {
    assert!(matches!(
        err("var i: Int = 1\ni += True\n"),
        TypeError::BadOperator { .. }
    ));
    assert_eq!(err("x += 1\n"), TypeError::UndefinedVariable("x".into()));
}

#[test]
fn rejects_augmented_assignment_on_read_only_self() {
    let e = err(
        "@fieldwise_init\nstruct S:\n    var x: Int\n\n    def bad(self):\n        self.x += 1\n",
    );
    assert_eq!(e, TypeError::ImmutableSelf);
}

// --- Float64 / SIMD unification ---

#[test]
fn float64_and_simd_float64_1_are_the_same_type() {
    // Interchangeable both ways, since Float64 == SIMD[DType.float64, 1].
    ok("var a: Float64 = 3.0\nvar b: SIMD[DType.float64, 1] = a\nvar c: Float64 = b\n");
}

#[test]
fn accepts_float64_vectors_and_lane_ops() {
    ok(
        "var v: SIMD[DType.float64, 4] = SIMD[DType.float64, 4](1.0, 2.0, 3.0, 4.0)\nvar d: SIMD[DType.float64, 4] = v + v\nvar q: SIMD[DType.float64, 4] = v / SIMD[DType.float64, 4](2.0)\nvar lane: Float64 = v[2]\nv[0] = 9.0\nv[1] += 1.0\nvar m: SIMD[DType.bool, 4] = v < d\n",
    );
}

#[test]
fn float64_scalar_splats_into_a_float64_vector() {
    ok(
        "var a: Float64 = 2.0\nvar v: SIMD[DType.float64, 2] = SIMD[DType.float64, 2](1.0, 2.0)\nvar w: SIMD[DType.float64, 2] = v + a\nv[0] = a\n",
    );
}

#[test]
fn rejects_float64_vector_dtype_mismatch() {
    // A float32 scalar does not splat into a float64 vector (distinct dtypes).
    let e = err(
        "var v: SIMD[DType.float64, 2] = SIMD[DType.float64, 2](1.0, 2.0)\nvar w: SIMD[DType.float64, 2] = v + Float32(1.0)\n",
    );
    assert!(matches!(e, TypeError::BadOperator { .. }), "got {:?}", e);
}

// --- Inferred `var` (type from the initializer) ---

#[test]
fn accepts_inferred_var_and_uses_its_type() {
    // The inferred type flows to later uses (Int arithmetic, String concat, List).
    ok("var n = 40\nvar m: Int = n + 2\n");
    ok("var s = \"hi\"\nvar t: StringLiteral = s + \"!\"\n");
    ok("var xs = [1, 2, 3]\nxs.append(4)\nvar k: Int = xs[0]\n");
    ok("var f = 3.5\nvar g: Float64 = f * 2.0\n");
}

#[test]
fn inferred_var_rejects_wrong_later_use() {
    // `n` is inferred `Int`, so using it as a `Bool` condition is a type error.
    let e = err("var n = 5\nif n:\n    pass\n");
    assert!(matches!(e, TypeError::TypeMismatch { .. }), "got {:?}", e);
}

#[test]
fn rejects_inferred_var_of_a_closure() {
    let e = err("def outer():\n    def inner() -> Int:\n        return 1\n    var f = inner\n");
    assert_eq!(e, TypeError::ClosureEscape);
}

#[test]
fn accepts_inferred_nominal_range() {
    ok("var r = range(5)\nvar n: Int = len(r)\n");
}

// --- Tuples ---

#[test]
fn accepts_tuple_annotated_inferred_and_indexed() {
    ok(
        "var t: Tuple[Int, Float64, StringLiteral] = (1, 2.5, \"hi\")\nvar a: Int = t[0]\nvar b: Float64 = t[1]\nvar c: StringLiteral = t[2]\n",
    );
    ok("var u = (10, 20)\nvar first: Int = u[0]\n");
    ok(
        "def stats() -> Tuple[Int, Int]:\n    return (512, 4)\n\nvar s = stats()\nvar p: Int = s[0]\n",
    );
}

#[test]
fn tuple_element_coercion_materializes_literals() {
    // `(1, 2)` fits `Tuple[Float64, Float64]` element-wise.
    ok("var t: Tuple[Float64, Float64] = (1, 2)\n");
}

#[test]
fn accepts_tuple_constructors_and_structural_operations() {
    ok("var inferred = Tuple(1, \"one\")\nvar typed = Tuple[Float64, StringLiteral](2, \"two\")\n");
    ok(
        "var pair = 1, \"one\"\nvar n: Int = len(pair)\nvar has: Bool = 1 in pair\nvar lacks: Bool = 9 not in pair\n",
    );
    ok(
        "var a = Tuple(1, 2)\nvar b = Tuple(1, 3)\nvar eq: Bool = a == b\nvar lt: Bool = a < b\nvar ge: Bool = b >= a\n",
    );
    ok(
        "var pair = Tuple(1, \"one\")\nvar reversed: Tuple[StringLiteral, Int] = pair.reverse()\nvar joined: Tuple[Int, StringLiteral, Bool] = pair.concat(Tuple(True))\n",
    );
}

#[test]
fn consuming_tuple_transforms_copy_only_implicitly_copyable_places() {
    ok(
        "def main():\n    var pair = Tuple(1, 2)\n    var suffix = Tuple(3)\n    var reversed = pair.reverse()\n    var joined = pair.concat(suffix)\n    print(pair, suffix, reversed, joined)\n",
    );

    let source = "@fieldwise_init\nstruct Token(Movable, ImplicitlyDeletable):\n    var id: Int\n\ndef main():\n    var pair = Tuple(Token(1), Token(2))\n    var reversed = pair.reverse()\n    print(reversed[0].id)\n";
    assert!(matches!(err(source), TypeError::NonCopyable { .. }));

    ok(
        "@fieldwise_init\nstruct Token(Movable, ImplicitlyDeletable):\n    var id: Int\n\ndef main():\n    var pair = Tuple(Token(1), Token(2))\n    var reversed = pair^.reverse()\n    print(reversed[0].id)\n",
    );
}

#[test]
fn public_tuple_structurally_satisfies_comparable_during_discovery() {
    ok(
        "def ordered[T: Comparable](left: T, right: T) -> Bool:\n    return left < right\n\ndef main():\n    print(ordered(Tuple(1, 2), Tuple(1, 3)))\n",
    );
}

#[test]
fn checked_calls_mark_ordinary_and_parameterized_implicit_receiver_copies() {
    let source = "@fieldwise_init\nstruct Counter(ImplicitlyCopyable):\n    var value: Int\n    def take(deinit self) -> Int:\n        return self.value\n    def take_param[n: Int](deinit self) -> Int:\n        return self.value + n\n\ndef main():\n    var counter = Counter(40)\n    print(counter.take())\n    print(counter.take_param[2]())\n    print(counter.value)\n";
    let checked = check_program(&parse(source).expect("parse")).expect("check");
    let copied_calls = checked
        .expressions()
        .iter()
        .filter(|expression| {
            expression.adjustments.iter().any(|adjustment| {
                matches!(
                    adjustment,
                    SemanticAdjustment::ImplicitlyCopyConsumingReceiver
                )
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(copied_calls.len(), 2, "{copied_calls:#?}");
    assert!(copied_calls.iter().any(|expression| {
        matches!(
            expression.syntax.kind,
            mojito::ast::ExprKind::MethodCall { .. }
        )
    }));
    assert!(copied_calls.iter().any(|expression| {
        matches!(expression.syntax.kind, mojito::ast::ExprKind::Invoke { .. })
            && expression.adjustments.iter().any(|adjustment| {
                matches!(
                    adjustment,
                    SemanticAdjustment::ParameterizedMethodCall { .. }
                )
            })
    }));
}

#[test]
fn tuple_comparison_requires_the_same_tuple_self_type() {
    assert!(matches!(
        err("var result = Tuple(1) == Tuple(1, 2)\n"),
        TypeError::BadOperator { .. }
    ));
    assert!(matches!(
        err("var result = Tuple[Int](1) < Tuple[UInt](1)\n"),
        TypeError::BadOperator { .. }
    ));
}

#[test]
fn rejects_bad_typed_tuple_construction() {
    assert!(matches!(
        err("var t = Tuple[Int, StringLiteral](1)\n"),
        TypeError::ArityMismatch { .. }
    ));
    assert!(matches!(
        err("var t = Tuple[Int](\"wrong\")\n"),
        TypeError::TypeMismatch { .. }
    ));
}

#[test]
fn rejects_tuple_wrong_element_type() {
    let e = err("var t: Tuple[Int, Int] = (1, True)\n");
    assert!(matches!(e, TypeError::TypeMismatch { .. }), "got {:?}", e);
}

#[test]
fn rejects_runtime_tuple_index() {
    let e = err("var t: Tuple[Int, StringLiteral] = (1, \"x\")\nvar i: Int = 0\nvar y = t[i]\n");
    assert!(
        matches!(&e, TypeError::TypeMismatch { context, .. } if context == "tuple index"),
        "got {:?}",
        e
    );
}

#[test]
fn rejects_out_of_range_tuple_index() {
    let e = err("var t: Tuple[Int, Int] = (1, 2)\nvar y = t[5]\n");
    assert!(
        matches!(&e, TypeError::TypeMismatch { context, .. } if context == "tuple index"),
        "got {:?}",
        e
    );
}

#[test]
fn rejects_tuple_element_write() {
    // Tuples are immutable — no element assignment.
    let e = err("var t: Tuple[Int, Int] = (1, 2)\nt[0] = 9\n");
    assert!(
        matches!(
            e,
            TypeError::NotIndexable(_) | TypeError::InvalidAssignTarget(_)
        ),
        "got {:?}",
        e
    );
}

// --- Function-argument forms ---

#[test]
fn flags_advanced_parameter_forms_as_unsupported() {
    let src = "def f(out x: Int, out y: Int):\n    x = 1\n    y = 2\n";
    assert!(
        matches!(err(src), TypeError::Unsupported(_)),
        "expected Unsupported for: {src}"
    );
}

#[test]
fn kwargs_collect_unknown_keywords_and_check_values() {
    ok("def f(x: Int, **opts: Int):\n    pass\n\nf(1, a=2, b=3)\n");
    assert!(matches!(
        err("def f(**opts: Int):\n    pass\n\nf(a=\"wrong\")\n"),
        TypeError::TypeMismatch { .. }
    ));
    assert!(matches!(
        err("def f(x: Int, **opts: Int):\n    pass\n\nf(x=1, x=2)\n"),
        TypeError::BadCall { .. }
    ));
}

#[test]
fn transferred_string_dict_can_forward_keyword_arguments() {
    ok(
        "def target(prefix: Int, **options: Int):\n    pass\n\ndef relay(**options: Int):\n    target(prefix=7, **options^)\n\nrelay(left=20, right=22)\n",
    );
    assert!(matches!(
        err(
            "def target(**options: StringLiteral):\n    pass\n\ndef relay(**options: Int):\n    target(**options^)\n"
        ),
        TypeError::TypeMismatch { .. }
    ));
}

#[test]
fn generic_and_method_kwargs_share_collection_and_forwarding_checks() {
    ok(
        "def generic_size[T: Copyable & Movable](**options: T) -> Int:\n    return 0\n\n@fieldwise_init\nstruct Counter:\n    var bias: Int\n    def size[T: Copyable & Movable](self, **options: T) -> Int:\n        return self.bias\n    def relay(self, **options: Int) -> Int:\n        return self.size(**options^)\n    @staticmethod\n    def static_size[T: Copyable & Movable](**options: T) -> Int:\n        return 0\n\ndef main():\n    var counter = Counter(10)\n    print(generic_size(first=1, second=2))\n    print(counter.size(left=\"a\", right=\"b\"))\n    print(counter.relay(one=1, two=2, three=3))\n    print(Counter.static_size(a=1, b=2, c=3, d=4))\n",
    );
    assert!(matches!(
        err(
            "def generic_size[T: Copyable & Movable](**options: T) -> Int:\n    return 0\n\nvar value = generic_size(first=1, second=\"wrong\")\n"
        ),
        TypeError::TypeMismatch { .. }
    ));
    assert!(matches!(
        err(
            "@fieldwise_init\nstruct Counter:\n    var bias: Int\n    def size(self, **options: StringLiteral) -> Int:\n        return self.bias\n\nvar counter = Counter(0)\nvar value = counter.size(first=1)\n"
        ),
        TypeError::BadCall { .. } | TypeError::TypeMismatch { .. }
    ));
    assert!(matches!(
        err(
            "@fieldwise_init\nstruct Counter:\n    var bias: Int\n    def size(self, **options: Int) -> Int:\n        return self.bias\n\nvar counter = Counter(0)\nvar value = counter.size(first=1, first=2)\n"
        ),
        TypeError::BadCall { .. }
    ));
    assert!(matches!(
        err(
            "@fieldwise_init\nstruct Counter:\n    var bias: Int\n    def fail(self, **options: Int) raises -> Int:\n        raise \"failed\"\n\ndef call(counter: Counter) -> Int:\n    return counter.fail(code=1)\n"
        ),
        TypeError::UnhandledRaise(_)
    ));
    let ownership_error = err(
        "struct Token:\n    var value: Int\n    def __init__(out self, value: Int):\n        self.value = value\n    def __moveinit__(out self, deinit other: Self):\n        self.value = other.value\n\n@fieldwise_init\nstruct Collector:\n    var marker: Int\n    def take(self, **options: Token):\n        pass\n\nvar collector = Collector(0)\nvar token = Token(1)\ncollector.take(item=token)\n",
    );
    assert!(
        matches!(
            ownership_error,
            TypeError::NonCopyable { .. }
                | TypeError::BadCall { .. }
                | TypeError::TraitNotSatisfied { .. }
        ),
        "got {ownership_error:?}"
    );
    let storage_error = err(
        "struct Token:\n    var value: Int\n    def __init__(out self, value: Int):\n        self.value = value\n    def __moveinit__(out self, deinit other: Self):\n        self.value = other.value\n\n@fieldwise_init\nstruct Collector:\n    var marker: Int\n    def take(self, **options: Token):\n        pass\n",
    );
    assert!(
        matches!(
            &storage_error,
            TypeError::TraitNotSatisfied { trait_name, .. } if trait_name == "Copyable"
        ),
        "got {storage_error:?}"
    );
}

#[test]
fn bounded_trait_methods_share_generic_keyword_collection_and_effect_checks() {
    ok(
        "trait Collects:\n    def count[Element: Copyable & Movable](self, **options: Element) -> Int: ...\n\ndef direct[Target: Collects](target: Target) -> Int:\n    return target.count(first=1, second=2)\n\ndef relay[Target: Collects](target: Target, **options: Int) -> Int:\n    return target.count(**options^)\n",
    );
    assert!(matches!(
        err(
            "trait Collects:\n    def count[Element: Copyable & Movable](self, **options: Element) -> Int: ...\n\ndef bad[Target: Collects](target: Target) -> Int:\n    return target.count(first=1, second=\"wrong\")\n"
        ),
        TypeError::BadCall { .. } | TypeError::TypeMismatch { .. }
    ));
    assert!(matches!(
        err(
            "trait FallibleCollector:\n    def collect(self, **options: Int) raises -> Int: ...\n\ndef call[Target: FallibleCollector](target: Target) -> Int:\n    return target.collect(code=1)\n"
        ),
        TypeError::UnhandledRaise(_)
    ));
}

#[test]
fn accepts_positional_only_and_keyword_only_markers() {
    ok("def first(a: Int, b: Int, /) -> Int:\n    return a\n\nvar x: Int = first(1, 2)\n");
    ok("def scale(a: Int, *, by: Int) -> Int:\n    return a * by\n\nvar x: Int = scale(4, by=5)\n");
    ok(
        "def total(*values: Int, scale: Int) -> Int:\n    var t: Int = 0\n    for v in values:\n        t = t + v\n    return t * scale\n\nvar x: Int = total(1, 2, 3, scale=10)\n",
    );
    ok(
        "def needs_kw(a: Int = 1, *, b: Int) -> Int:\n    return a + b\n\nvar x: Int = needs_kw(b=4)\n",
    );

    assert!(matches!(
        err("def first(a: Int, /) -> Int:\n    return a\n\nvar x: Int = first(a=1)\n"),
        TypeError::BadCall { .. }
    ));
    assert!(matches!(
        err(
            "def scale(a: Int, *, by: Int) -> Int:\n    return a * by\n\nvar x: Int = scale(4, 5)\n"
        ),
        TypeError::ArityMismatch { .. }
    ));
    assert!(matches!(
        err(
            "def needs_kw(a: Int = 1, *, b: Int) -> Int:\n    return a + b\n\nvar x: Int = needs_kw()\n"
        ),
        TypeError::BadCall { .. }
    ));
}

#[test]
fn accepts_variadic_args() {
    // A `*args` parameter is a `List[T]` in the body; a call collects extras.
    ok(
        "def sum(*values: Int) -> Int:\n    var t: Int = 0\n    for v in values:\n        t = t + v\n    return t\n\nvar a: Int = sum()\nvar b: Int = sum(1, 2, 3)\n",
    );
    // Regular params before the variadic.
    ok(
        "def tag(label: StringLiteral, *nums: Int) -> Int:\n    return len(nums)\n\nvar a: Int = tag(\"x\", 1, 2)\n",
    );
    // Each overflow argument must match the element type.
    assert!(matches!(
        err("def sum(*values: Int) -> Int:\n    return 0\n\nvar z: Int = sum(1, \"x\")\n"),
        TypeError::TypeMismatch { .. }
    ));
    // A required regular parameter is still enforced.
    assert!(matches!(
        err(
            "def tag(label: StringLiteral, *nums: Int) -> Int:\n    return 0\n\nvar z: Int = tag()\n"
        ),
        TypeError::BadCall { .. }
    ));
    ok("def f[T: AnyType](*a: T) -> Int:\n    return len(a)\n\nvar n: Int = f(1, 2, 3)\n");
}

#[test]
fn accepts_a_heterogeneous_variadic_type_pack() {
    ok(
        "def count[*ArgTypes: AnyType](*args: *ArgTypes) -> Int:\n    return len(args)\n\ndef main():\n    var n: Int = count(1, \"two\", True)\n",
    );
}

#[test]
fn accepts_string_compile_time_value_parameters() {
    ok(
        "@fieldwise_init\nstruct Named[label: StringLiteral]:\n    var value: Int\n\nvar item: Named[\"answer\"] = Named[\"answer\"](42)\n",
    );
}

#[test]
fn applies_defaulted_compile_time_value_parameters() {
    ok(
        "@fieldwise_init\nstruct Buffer[size: Int = 4]:\n    var value: Int\n\nvar inferred: Buffer = Buffer(1)\nvar explicit: Buffer[8] = Buffer[8](2)\n",
    );
}

#[test]
fn binds_named_and_defaulted_compile_time_parameters() {
    ok(
        "@fieldwise_init\nstruct Matrix[rows: Int = 2, columns: Int = 3]:\n    var value: Int\n\nvar a: Matrix[columns=4, rows=1] = Matrix[columns=4, rows=1](7)\nvar b: Matrix = Matrix(8)\n",
    );
}

#[test]
fn evaluates_defaults_that_depend_on_earlier_parameters() {
    ok(
        "@fieldwise_init\nstruct Matrix[rows: Int, columns: Int = rows + 1]:\n    var value: Int\n\nvar matrix: Matrix[3] = Matrix[3](7)\n",
    );
}

#[test]
fn applies_defaulted_type_parameters() {
    ok(
        "@fieldwise_init\nstruct Box[T: Copyable = Int]:\n    var value: Self.T\n\nvar box: Box = Box(7)\n",
    );
}

#[test]
fn supports_parameterized_aggregate_compile_time_values() {
    ok(
        "@fieldwise_init\nstruct Selection[indices: List[Int]]:\n    var value: Int\n\nvar selected: Selection[[0, 2, 4]] = Selection[[0, 2, 4]](9)\n",
    );
}

#[test]
fn rejects_explicit_infer_only_compile_time_parameters() {
    let error = err(
        "def identity[T: Copyable, // U: AnyType](value: T) -> T:\n    return value\n\nvar value: Int = identity[Int, Int](1)\n",
    );
    assert!(
        matches!(&error, TypeError::Unsupported(message) if message.contains("infer-only")),
        "got {error:?}"
    );
}

#[test]
fn infers_generic_static_and_instance_method_parameters() {
    ok(
        "@fieldwise_init\nstruct Factory:\n    var marker: Int\n    @staticmethod\n    def make[T: Copyable](value: T) -> T:\n        return value\n    def echo[T: Copyable](self, value: T) -> T:\n        return value\n\nvar factory = Factory(0)\nvar a: Int = Factory.make(1)\nvar b: StringLiteral = factory.echo(\"ok\")\n",
    );
}

#[test]
fn checks_generic_trait_method_signatures_in_their_parameter_scope() {
    ok(
        "trait Echoer:\n    def echo[T: Copyable](self, value: T) -> T:\n        ...\n\n@fieldwise_init\nstruct Echo(Echoer):\n    var marker: Int\n    def echo[T: Copyable](self, value: T) -> T:\n        return value\n\nvar echo = Echo(0)\nvar answer: Int = echo.echo(42)\n",
    );
}

#[test]
fn enforces_trailing_where_constraints_and_type_equality() {
    ok(
        "def positive[n: Int]() -> Int where n > 0:\n    return n\n\ndef comparable[T: AnyType](value: T) -> Int where conforms_to(T, Comparable) and T == Int:\n    return 1\n\nvar a = positive[2]()\nvar b = comparable(3)\n",
    );
    assert!(matches!(
        err("def positive[n: Int]() -> Int where n > 0:\n    return n\n\nvar a = positive[0]()\n"),
        TypeError::BadCall { .. }
    ));
    assert!(matches!(
        err(
            "def comparable[T: AnyType](value: T) -> Int where conforms_to(T, Comparable):\n    return 1\n\nvar a = comparable(\"no\")\n"
        ),
        TypeError::BadCall { .. }
    ));
}

#[test]
fn gates_methods_with_parent_parameter_where_constraints() {
    ok(
        "@fieldwise_init\nstruct Wrapper[T: Copyable]:\n    var value: Self.T\n    def compare(self, other: Self.T) -> Bool where conforms_to(Self.T, Comparable):\n        return True\n\nvar wrapped = Wrapper[Int](1)\nvar same = wrapped.compare(1)\n",
    );
    assert!(matches!(
        err(
            "@fieldwise_init\nstruct Wrapper[T: Copyable]:\n    var value: Self.T\n    def compare(self, other: Self.T) -> Bool where conforms_to(Self.T, Comparable):\n        return False\n\nvar wrapped = Wrapper[StringLiteral](\"x\")\nvar same = wrapped.compare(\"x\")\n"
        ),
        TypeError::NoSuchMethod { .. } | TypeError::BadCall { .. }
    ));
}

#[test]
fn applies_implicit_conversion_to_a_specialized_generic_target() {
    ok(
        "struct Box[T: AnyType]:\n    var value: Self.T\n    @implicit\n    def __init__(out self, value: Self.T):\n        self.value = value\n\ndef take(value: Box[Int]) -> Int:\n    return value.value\n\nvar answer: Int = take(42)\n",
    );
}

#[test]
fn checks_each_heterogeneous_pack_element_against_its_bound() {
    let error = err(
        "def count[*ArgTypes: Intable](*args: *ArgTypes) -> Int:\n    return len(args)\n\ndef main():\n    var n: Int = count(1, \"two\")\n",
    );
    assert!(matches!(error, TypeError::TraitNotSatisfied { .. }));
}

#[test]
fn nightly_pack_conformance_checks_every_type_value() {
    ok(
        "def count[*Ts: AnyType](*args: *Ts) -> Int where conforms_to(Ts.values, Writable):\n    return len(args)\n\ndef main():\n    var n = count(1, \"two\", True)\n",
    );
    assert!(matches!(
        err(
            "@fieldwise_init\nstruct Opaque:\n    var n: Int\n\ndef count[*Ts: AnyType](*args: *Ts) -> Int where conforms_to(Ts.values, Writable):\n    return len(args)\n\ndef main():\n    var n = count(Opaque(1))\n"
        ),
        TypeError::BadCall { .. }
    ));
}

#[test]
fn int_is_scalar_dtype_int_and_simdlength_drives_width_values() {
    ok(
        "def width_value[width: SIMDLength]() -> Int where width > 0:\n    return width\n\ndef main():\n    var scalar: Scalar[DType.int] = 41\n    var same: Int = Scalar[DType.int](scalar + 1)\n    var vector: SIMD[DType.int, 4] = SIMD[DType.int, _](1, 2, 3, 4)\n    var width = width_value[4]()\n",
    );
}

#[test]
fn deprecated_simdsize_alias_stays_accepted() {
    // The transitional `SIMDSize` spelling classifies a width value parameter
    // exactly like `SIMDLength`; it is accepted but never emitted.
    ok(
        "def width_value[width: SIMDSize]() -> Int where width > 0:\n    return width\n\ndef main():\n    var width = width_value[4]()\n",
    );
}

#[test]
fn accepts_default_argument_values() {
    // A default lets a call omit the trailing argument.
    ok(
        "def my_pow(base: Int, exp: Int = 2) -> Int:\n    return base ** exp\n\nvar a: Int = my_pow(3)\nvar b: Int = my_pow(3, 3)\n",
    );
}

#[test]
fn rejects_bad_default_values_and_arity() {
    // Default value must fit the parameter type.
    assert!(matches!(
        err("def f(a: Int, b: Int = \"x\") -> Int:\n    return a\n"),
        TypeError::TypeMismatch { .. }
    ));
    // A required parameter cannot follow a defaulted one.
    assert!(matches!(
        err("def f(a: Int = 1, b: Int) -> Int:\n    return a\n"),
        TypeError::Unsupported(_)
    ));
    // Fewer than the required count leaves a required argument unbound (BadCall);
    // more than the total is a too-many-positional arity error.
    assert!(matches!(
        err("def f(a: Int, b: Int = 2) -> Int:\n    return a\n\nvar z: Int = f()\n"),
        TypeError::BadCall { .. }
    ));
    assert!(matches!(
        err("def f(a: Int, b: Int = 2) -> Int:\n    return a\n\nvar z: Int = f(1, 2, 3)\n"),
        TypeError::ArityMismatch { .. }
    ));
    ok("def f[T: Copyable & Movable](a: T, b: Int = 2) -> T:\n    return a\n\nvar n: Int = f(1)\n");
}

#[test]
fn generic_calls_share_ordinary_argument_binding() {
    ok(
        "def choose[T: Copyable & Movable](first: T, /, offset: Int = 0, *, use_offset: Bool = False) -> T:\n    return first\n\nvar a: Int = choose(1, offset=2, use_offset=True)\n",
    );
    assert!(matches!(
        err("def f[T: Copyable & Movable](x: T, /) -> T:\n    return x\n\nvar n: Int = f(x=1)\n"),
        TypeError::BadCall { .. }
    ));
}

#[test]
fn accepts_keyword_arguments_and_reports_mismatches() {
    // Keyword args to a free function type-check (any order, mixed with positional).
    ok(
        "def f(a: Int, b: Int) -> Int:\n    return a - b\n\nvar z: Int = f(b=1, a=2)\nvar w: Int = f(2, b=1)\n",
    );
    // Unknown keyword, an argument bound twice, or a missing required argument.
    for src in [
        "def f(a: Int) -> Int:\n    return a\n\nvar z: Int = f(x=1)\n",
        "def f(a: Int, b: Int) -> Int:\n    return a\n\nvar z: Int = f(1, a=2)\n",
        "def f(a: Int, b: Int) -> Int:\n    return a\n\nvar z: Int = f(b=2)\n",
    ] {
        assert!(
            matches!(err(src), TypeError::BadCall { .. }),
            "expected BadCall for: {src}"
        );
    }
    // A keyword-argument type mismatch is still a TypeMismatch.
    assert!(matches!(
        err("def f(a: Int) -> Int:\n    return a\n\nvar z: Int = f(a=\"x\")\n"),
        TypeError::TypeMismatch { .. }
    ));
    // Keyword args to a built-in are rejected.
    assert!(matches!(
        err("def main():\n    print(len(x=\"hi\"))\n"),
        TypeError::BadCall { .. }
    ));
    // Ordinary user methods use the same keyword matcher.
    ok(
        "@fieldwise_init\nstruct C:\n    var n: Int\n\n    def g(self, k: Int) -> Int:\n        return k\n\nvar z: Int = C(1).g(k=2)\n",
    );
}

#[test]
fn method_argument_markers_and_keywords_are_checked() {
    ok(
        "@fieldwise_init\nstruct C:\n    var n: Int\n    def f(self, x: Int, /, y: Int = 2, *, scale: Int = 1) -> Int:\n        return (x + y) * scale\n\nvar c: C = C(0)\nvar n: Int = c.f(1, scale=3)\n",
    );
    let error = err(
        "@fieldwise_init\nstruct C:\n    var n: Int\n    def f(self, x: Int, /) -> Int:\n        return x\n\nvar c: C = C(0)\nvar n: Int = c.f(x=1)\n",
    );
    assert!(matches!(error, TypeError::BadCall { .. }), "got {error:?}");
}

#[test]
fn flags_advanced_forms_on_methods_and_traits() {
    ok(
        "@fieldwise_init\nstruct C:\n    var n: Int\n\n    def f(self, k: Int = 1):\n        pass\n\nvar c: C = C(0)\nc.f()\n",
    );
    assert!(matches!(
        err("trait T:\n    def m(self, *args: Int) -> Int:\n        ...\n"),
        TypeError::Unsupported(_)
    ));
}

#[test]
fn plain_signatures_are_unaffected() {
    ok("def add(a: Int, b: Int) -> Int:\n    return a + b\n\nvar z: Int = add(1, 2)\n");
    // A parameter literally named after a convention word still works.
    ok("def g(read: Int) -> Int:\n    return read\n\nvar z: Int = g(5)\n");
}

// --- Expression syntax parsed but semantics deferred (syntax-first phase) ---

#[test]
fn unlinked_string_annotation_fails_explicitly() {
    // Without the prelude, `String` is not a declared type: the raw seam
    // rejects the annotation instead of silently diverging from the linked
    // build's nominal-String takeover. Focused tests spell `StringLiteral`.
    let error = err("var s: String = \"x\"\n");
    assert!(
        format!("{error:?}").contains("String"),
        "expected an unknown-type error naming String, got {error:?}"
    );
}

#[test]
fn expression_surface_type_checks() {
    // A t-string is a lazy TString value, not a builtin String.
    let error = err("var s: StringLiteral = t\"x{1}\"\n");
    assert!(
        format!("{error:?}").contains("TString"),
        "expected a TString mismatch, got {error:?}"
    );
    ok("print(t\"x{1}\")\n");
    ok("var one: Int = (n := 1)\nprint(n)\n");
    // Ternary, chained comparison, and slices are implemented too.
    ok("var m: Int = 1 if True else 2\nprint(m)\n");
    ok("var t: Bool = 0 < 1 < 2\nprint(t)\n");
    ok("var xs: List[Int] = [1, 2, 3]\nvar ys: List[Int] = xs[0:2]\nprint(ys)\n");
    // A lone comparison is unaffected (still type-checks).
    ok("var b: Bool = 1 < 2\n");
}

#[test]
fn ternary_chained_comparison_and_unpacking_type_check() {
    // Ternary branches must unify; the condition must be Bool.
    ok("def f(n: Int) -> Int:\n    return 1 if n > 0 else -1\n");
    assert!(matches!(
        err("var m: Int = 1 if 5 else 2\n"),
        TypeError::TypeMismatch { .. }
    ));
    // Chained comparison → Bool.
    ok("def g(i: Int, n: Int) -> Bool:\n    return 0 <= i < n\n");
    // Tuple unpacking: arity + element types.
    ok(
        "var t: Tuple[Int, StringLiteral] = (1, \"a\")\nvar x: Int = 0\nvar s: StringLiteral = \"\"\nx, s = t\n",
    );
    assert!(matches!(
        err(
            "var t: Tuple[Int, Int] = (1, 2)\nvar a: Int = 0\nvar b: Int = 0\nvar c: Int = 0\na, b, c = t\n"
        ),
        TypeError::TypeMismatch { .. }
    ));
}

// --- Decorators + dunder / receiver conventions (parse-only; semantics deferred) ---

#[test]
fn accepts_decorators_and_plain_dunders() {
    // Unmodeled decorators on a def/struct are ignored by the checker.
    ok("@always_inline\ndef f(x: Int) -> Int:\n    return x\n\nvar z: Int = f(1)\n");
    ok("@value\n@fieldwise_init\nstruct P:\n    var x: Int\n\nvar p: P = P(1)\n");
    // A dunder method with a plain `self` type-checks like any method.
    ok(
        "@fieldwise_init\nstruct V:\n    var x: Int\n\n    def __eq__(self, o: V) -> Bool:\n        return self.x == o.x\n",
    );
}

#[test]
fn flags_out_self_and_accepts_static_methods() {
    // `out self` on a non-__init__ method is still unsupported (only `__init__`
    // may initialize the receiver).
    assert!(matches!(
        err("struct W:\n    var x: Int\n\n    def reset(out self):\n        self.x = 0\n"),
        TypeError::Unsupported(_)
    ));
    ok(
        "struct S:\n    @staticmethod\n    def make(x: Int) -> Int:\n        return x\n\ndef main():\n    print(S.make(2))\n",
    );
}

#[test]
fn hand_written_init_out_self() {
    // `def __init__(out self, …)` constructs a struct without `@fieldwise_init`.
    ok(
        "struct P:\n    var x: Int\n    var y: Int\n    def __init__(out self, x: Int, y: Int):\n        self.x = x\n        self.y = y\n\ndef main():\n    var p: P = P(1, 2)\n    print(p.x)\n",
    );
    // Definite initialization: every field must be assigned in the body.
    assert!(matches!(
        err(
            "struct P:\n    var x: Int\n    var y: Int\n    def __init__(out self, x: Int):\n        self.x = x\n"
        ),
        TypeError::UninitializedField { .. }
    ));
    // A struct cannot have both `@fieldwise_init` and a hand-written `__init__`.
    assert!(matches!(
        err(
            "@fieldwise_init\nstruct W:\n    var x: Int\n    def __init__(out self, x: Int):\n        self.x = x\n"
        ),
        TypeError::ConflictingConstructor(_)
    ));
    // Neither a constructor nor `@fieldwise_init` → no constructor.
    assert!(matches!(
        err(
            "struct P:\n    var x: Int\n    def get(self) -> Int:\n        return self.x\n\ndef main():\n    var p: P = P(1)\n"
        ),
        TypeError::NoConstructor(_)
    ));
}

#[test]
fn handwritten_constructor_uses_default_and_keyword_binding() {
    ok(
        "struct Box:\n    var value: Int\n    def __init__(out self, value: Int = 3):\n        self.value = value\n\ndef main():\n    var a = Box()\n    var b = Box(value=7)\n    print(a.value, b.value)\n",
    );
}

#[test]
fn lifecycle_initialization_is_required_on_every_value_producing_path() {
    ok(
        "struct Choice:\n    var value: Int\n    def __init__(out self, choose: Bool):\n        if choose:\n            self.value = 1\n        else:\n            self.value = 2\n",
    );
    assert!(matches!(
        err(
            "struct Choice:\n    var value: Int\n    def __init__(out self, choose: Bool):\n        if choose:\n            self.value = 1\n"
        ),
        TypeError::UninitializedField { .. }
    ));
    ok(
        "struct Choice:\n    var value: Int\n    def __init__(out self, choose: Bool) raises:\n        if choose:\n            self.value = 1\n        else:\n            raise \"no value\"\n",
    );
}

#[test]
fn trait_refinement_inherits_requirements_and_capabilities() {
    ok(
        "trait Named:\n    def name(self) -> StringLiteral: ...\n\ntrait Described(Named):\n    def description(self) -> StringLiteral: ...\n\n@fieldwise_init\nstruct Item(Described):\n    var label: StringLiteral\n    def name(self) -> StringLiteral:\n        return self.label\n    def description(self) -> StringLiteral:\n        return self.label\n\ndef require_named[T: Named](value: T) -> StringLiteral:\n    return value.name()\n\ndef main():\n    var item = Item(\"x\")\n    print(require_named(item))\n",
    );
    assert!(matches!(
        err(
            "trait Named:\n    def name(self) -> StringLiteral: ...\n\ntrait Described(Named):\n    def description(self) -> StringLiteral: ...\n\n@fieldwise_init\nstruct Bad(Described):\n    var label: StringLiteral\n    def description(self) -> StringLiteral:\n        return self.label\n"
        ),
        TypeError::MissingTraitMethod { method, .. } if method == "name"
    ));
}

#[test]
fn checks_callable_parameters_and_indirect_invocation() {
    ok(
        "def increment(x: Int) -> Int:\n    return x + 1\n\ndef apply(cb: def(Int) -> Int, x: Int) -> Int:\n    return cb(x)\n\ndef main():\n    var callback: def(Int) -> Int = increment\n    print(apply(callback, 41))\n",
    );
    assert!(matches!(
        err(
            "def wrong(x: StringLiteral) -> Int:\n    return 0\n\ndef apply(cb: def(Int) -> Int, x: Int) -> Int:\n    return cb(x)\n\ndef main():\n    print(apply(wrong, 41))\n"
        ),
        TypeError::TypeMismatch { .. }
    ));
}

#[test]
fn callable_type_bounds_are_dependent_nominal_and_directional() {
    ok(
        "def apply[T: Copyable & ImplicitlyDeletable, F: def(T) -> T](callback: F, value: T) -> T:\n    return callback(value)\n\ndef increment(value: Int) -> Int:\n    return value + 1\n\ndef main():\n    print(apply(increment, 41))\n",
    );
    ok(
        "@fieldwise_init\nstruct Add(def(Int) -> Int):\n    var delta: Int\n    def __call__(self, value: Int) -> Int:\n        return value + self.delta\n\ndef apply[F: def(Int) -> Int](callback: F, value: Int) -> Int:\n    return callback(value)\n\ndef main():\n    print(apply(Add(1), 41))\n",
    );
    // A read-only implementation demands less access than the mutable bound
    // promises to provide, and a non-raising implementation satisfies a raising
    // bound.
    ok(
        "def inspect(value: Int):\n    pass\n\ndef apply[F: def(mut Int) raises -> None](callback: F, mut value: Int) raises:\n    callback(value)\n\ndef main() raises:\n    var value = 1\n    apply(inspect, value)\n",
    );
}

#[test]
fn generic_anonymous_callable_bounds_are_alpha_equivalent_and_invokable() {
    ok(
        "def identity[U: ImplicitlyCopyable & ImplicitlyDeletable](value: U) -> U:\n    return value\n\ndef apply[F: def[T: ImplicitlyCopyable & ImplicitlyDeletable](T) -> T](callback: F) -> Int:\n    return callback(42)\n\ndef main():\n    print(apply(identity))\n",
    );
}

#[test]
fn generic_anonymous_callable_contracts_preserve_binder_shape_and_bounds() {
    let wrong_bounds = err(
        "def identity[U: ImplicitlyCopyable](value: U) -> U:\n    return value\n\ndef apply[F: def[T: ImplicitlyCopyable & ImplicitlyDeletable](T) -> T](callback: F) -> Int:\n    return callback(42)\n\ndef main():\n    print(apply(identity))\n",
    );
    assert!(matches!(wrong_bounds, TypeError::TraitNotSatisfied { .. }));

    let wrong_arity = err(
        "def first[U: ImplicitlyCopyable & ImplicitlyDeletable](left: U, right: U) -> U:\n    return left\n\ndef apply[F: def[T: ImplicitlyCopyable & ImplicitlyDeletable](T) -> T](callback: F) -> Int:\n    return callback(42)\n\ndef main():\n    print(apply(first))\n",
    );
    assert!(matches!(wrong_arity, TypeError::TraitNotSatisfied { .. }));
}

#[test]
fn rejects_parametric_function_types_as_runtime_parameter_annotations() {
    let error = err(
        "def consume(callback: def[T: ImplicitlyCopyable & ImplicitlyDeletable](T) -> T):\n    pass\n",
    );
    assert!(matches!(
        error,
        TypeError::Unsupported(ref feature)
            if feature.contains("runtime parameter cannot use a parametric function type")
    ));
}

#[test]
fn callable_type_bounds_reject_structural_generic_overloaded_and_stronger_effects() {
    let structural = err(
        "@fieldwise_init\nstruct Structural:\n    def __call__(self, value: Int) -> Int:\n        return value\n\ndef apply[F: def(Int) -> Int](callback: F) -> Int:\n    return callback(1)\n\ndef main():\n    print(apply(Structural()))\n",
    );
    assert!(matches!(structural, TypeError::TraitNotSatisfied { .. }));

    let raising = err(
        "def fail(value: Int) raises -> Int:\n    raise Error(\"failure\")\n\ndef apply[F: def(Int) -> Int](callback: F) -> Int:\n    return callback(1)\n\ndef main():\n    print(apply(fail))\n",
    );
    assert!(matches!(raising, TypeError::TraitNotSatisfied { .. }));

    let mutable = err(
        "def change(mut value: Int):\n    value += 1\n\ndef apply[F: def(Int) -> None](callback: F, value: Int):\n    callback(value)\n\ndef main():\n    apply(change, 1)\n",
    );
    assert!(matches!(mutable, TypeError::TraitNotSatisfied { .. }));

    let generic = err(
        "def identity[T: Copyable & ImplicitlyDeletable](value: T) -> T:\n    return value\n\ndef apply[F: def(Int) -> Int](callback: F) -> Int:\n    return callback(1)\n\ndef main():\n    print(apply(identity))\n",
    );
    assert!(matches!(generic, TypeError::TraitNotSatisfied { .. }));

    let overloaded = err(
        "def choose(value: Int) -> Int:\n    return value\n\ndef choose(value: StringLiteral) -> Int:\n    return len(value)\n\ndef apply[F: def(Int) -> Int](callback: F) -> Int:\n    return callback(1)\n\ndef main():\n    print(apply(choose))\n",
    );
    assert!(matches!(overloaded, TypeError::TraitNotSatisfied { .. }));
}

#[test]
fn contextually_instantiates_a_generic_callable_value() {
    ok(
        "def identity[T: Copyable & Movable](value: T) -> T:\n    return value\n\ndef main():\n    var callback: def(Int) -> Int = identity\n    print(callback(42))\n",
    );
}

#[test]
fn explicitly_specializes_callable_origin_values() {
    let declaration = "def borrow[origin: Origin[mut=True]](ref[origin] value: Int) -> ref[origin] Int:\n    return value\n\n";
    ok(&format!(
        "{declaration}def main():\n    var value = 40\n    var function = borrow[origin_of(value)]\n    ref result = function(value)\n    result += 2\n"
    ));

    let error = err(&format!(
        "{declaration}def main():\n    var value = 40\n    var other = 2\n    var function = borrow[origin_of(value)]\n    ref result = function(other)\n"
    ));
    assert!(
        matches!(error, TypeError::TypeMismatch { ref context, .. }
            if context == "call through an origin-specialized function value"),
        "got {error:?}"
    );
}

#[test]
fn explicitly_specializes_nested_callable_origin_values() {
    ok(include_str!(
        "../conformance/fixtures/nested_origin_specialized_function_value.mojo"
    ));
    ok(
        "def main():\n    var marker = 2\n    var value = 40\n    def borrow[origin: Origin[mut=True]](ref[origin] item: Int) {imm marker} -> ref[origin] Int:\n        print(marker)\n        return item\n    var function = borrow[origin_of(value)]\n    ref result = function(value)\n    result += marker\n",
    );
}

#[test]
fn explicit_origins_participate_in_overload_and_generic_candidate_selection() {
    ok(
        "def choose[origin: Origin[mut=True]](ref[origin] value: Int) -> ref[origin] Int:\n    return value\n\ndef choose[origin: Origin[mut=True]](ref[origin] value: Float64) -> ref[origin] Float64:\n    return value\n\ndef main():\n    var value = 40\n    ref selected = choose[origin_of(value)](value)\n    selected += 2\n",
    );
    ok(
        "def borrow[T: Copyable & ImplicitlyDeletable, origin: Origin[mut=True]](ref[origin] value: T) -> ref[origin] T:\n    return value\n\ndef main():\n    var value = 40\n    var function = borrow[Int, origin_of(value)]\n    ref selected = function(value)\n    selected += 2\n",
    );
    ok(
        "def borrow[T: Copyable & ImplicitlyDeletable, origin: Origin[mut=True]](ref[origin] value: T) -> ref[origin] T:\n    return value\n\ndef main():\n    var value = 40\n    ref selected = borrow[origin=origin_of(value)](value)\n    selected += 2\n",
    );
}

#[test]
fn contextual_type_selects_an_origin_specialized_overload_value() {
    let declarations = "def choose[origin: Origin[mut=True]](ref[origin] value: Int) -> ref[origin] Int:\n    return value\n\ndef choose[origin: Origin[mut=True]](ref[origin] value: Float64) -> ref[origin] Float64:\n    return value\n\n";
    ok(&format!(
        "{declarations}def main():\n    var value = 40\n    var function: def(ref[origin_of(value)] Int) thin -> ref[origin_of(value)] Int = choose[origin_of(value)]\n    ref selected = function(value)\n    selected += 2\n"
    ));

    let error = err(&format!(
        "{declarations}def main():\n    var value = 40\n    var function = choose[origin_of(value)]\n"
    ));
    assert!(
        matches!(error, TypeError::BadCall { ref reason, .. }
            if reason.contains("contextual callable type")),
        "got {error:?}"
    );
}

#[test]
fn contextually_selects_an_overloaded_callable_value() {
    ok(
        "def choose(value: Int) -> Int:\n    return value + 1\n\ndef choose(value: StringLiteral) -> Int:\n    return len(value)\n\ndef main():\n    var callback: def(Int) -> Int = choose\n    var result: Int = callback(41)\n",
    );
}

#[test]
fn accepts_numeric_bases_and_string_forms_and_tstrings() {
    // Based integers, digit separators, and single/triple-quoted strings are fully
    // supported (they are ordinary Int/String values).
    ok(
        "var a: Int = 0xFF\nvar b: Int = 1_000_000\nvar c: StringLiteral = 'single'\nvar d: StringLiteral = \"\"\"triple\"\"\"\n",
    );
    // A t-string types as the lazy TString; explicit `String(...)`
    // materializes it into the builtin string.
    ok("var y: Int = 1\nvar s: StringLiteral = String(t\"x={y}\")\n");
    let error = err("var y: Int = 1\nvar s: StringLiteral = t\"x={y}\"\n");
    assert!(
        format!("{error:?}").contains("TString"),
        "expected a TString mismatch, got {error:?}"
    );
}

#[test]
fn owned_self_and_owned_params_are_accepted() {
    // `deinit self` and `var`/`read` parameter
    // conventions bind by value and now type-check (their ownership meaning is
    // handled by the ownership analysis / ASAP drops).
    assert!(
        check_source("@fieldwise_init\nstruct R:\n    var x: Int\n    def __del__(deinit self):\n        print(self.x)\n").is_ok()
    );
    assert!(check_source("def f(var a: Int, read b: Int) -> Int:\n    return a + b\n").is_ok());
}

#[test]
fn nightly_implicit_deletion_controls_linearity_independently_of_the_decorator() {
    // The decorator only customizes a diagnostic. Without an explicit false
    // conformance, the ordinary ASAP destruction path remains available.
    ok(
        "@explicit_destroy(\"unused diagnostic\")\n@fieldwise_init\nstruct Ordinary:\n    var id: Int\n\ndef main():\n    var value = Ordinary(1)\n",
    );

    let error = err(
        "struct Linear(ImplicitlyDeletable where False):\n    def __init__(out self):\n        pass\n    def close(deinit self):\n        pass\n\ndef main():\n    var value = Linear()\n",
    );
    assert!(matches!(
        error,
        TypeError::ExplicitDestroy { message, .. }
            if message.contains("not implicitly deletable")
    ));
}

#[test]
fn conditional_deletability_is_retained_at_checked_binding_sites() {
    let conditional = "@explicit_destroy(\"close Box\")\nstruct Box[T: Movable](\n    ImplicitlyDeletable where conforms_to(T, ImplicitlyDeletable)\n):\n    var value: Self.T\n    def __init__(out self, value: Self.T):\n        self.value = value\n    def close(deinit self):\n        pass\n\n";

    // A positive `where` fact applies to both the consuming parameter and the
    // local that receives it. The explicit-destroy pass must consume the exact
    // checked-site facts rather than reclassifying the nominal `Box[T]` later.
    ok(&format!(
        "{conditional}def consume[T: Movable](var incoming: Box[T]) where conforms_to(T, ImplicitlyDeletable):\n    var local = incoming^\n\nstruct Owner[T: Movable]:\n    def consume(self, var incoming: Box[Self.T]) where conforms_to(Self.T, ImplicitlyDeletable):\n        var local = incoming^\n"
    ));

    // Without the positive constraint a generic `Box[T]` remains linear. This
    // guards against accidentally making the nominal type globally deletable.
    let error = err(&format!(
        "{conditional}def abandon[T: Movable](var incoming: Box[T]):\n    var local = incoming^\n"
    ));
    assert!(
        matches!(error, TypeError::ExplicitDestroy { ref var, .. } if var == "local"),
        "got {error:?}"
    );
}

#[test]
fn explicit_destroy_requires_a_diagnostic_message() {
    assert!(matches!(
        err("@explicit_destroy\nstruct Resource:\n    pass\n"),
        TypeError::Unsupported(message)
            if message.contains("requires exactly one positional string message")
    ));
}

#[test]
fn named_out_result_mut_self_and_parametric_ref_self_are_checked() {
    // A named `out` result is caller-transparent. `mut self` is writable;
    // bare `ref self` propagates caller mutability but cannot assume it while
    // checking the unspecialized method body.
    assert!(check_source("def f(out a: Int):\n    a = 1\n").is_ok());
    assert!(
        check_source(
            "@fieldwise_init\nstruct R:\n    var x: Int\n    def m(mut self):\n        self.x = 2\n    def get(ref self) -> Int:\n        return self.x\n"
        )
        .is_ok()
    );
    assert!(matches!(
        check_source(
            "@fieldwise_init\nstruct R:\n    var x: Int\n    def m(ref self):\n        self.x = 2\n"
        ),
        Err(TypeError::ImmutableSelf)
    ));
}

#[test]
fn validates_named_origins_and_parametric_mutability() {
    assert!(
        check_source(
            "def observe[is_mutable: Bool, //, origin: Origin[mut=is_mutable]](ref[origin] value: Int):\n    print(value)\n"
        )
        .is_ok()
    );
    assert!(matches!(
        check_source("def bad(ref[missing] value: Int):\n    pass\n"),
        Err(TypeError::UndefinedVariable(name)) if name == "missing"
    ));
    assert!(
        check_source(
            "def pair[origin: Origin[mut=False]](ref[origin] left: Int, ref[origin] right: Int):\n    print(left + right)\n"
        )
        .is_ok()
    );
    assert!(matches!(
        check_source(
            "def bad[origin: Origin[mut=False]](ref[origin] value: Int):\n    value = 2\n"
        ),
        Err(TypeError::ImmutableBinding(name)) if name == "value"
    ));
}

#[test]
fn checks_reference_returns_substitution_and_escapes() {
    assert!(check_source(
        "def borrow[origin: Origin[mut=False]](ref[origin] value: Int) -> ref[origin] Int:\n    return value\n"
    )
    .is_ok());
    assert!(check_source(
        "def choose(ref left: Int, ref right: Int, flag: Bool) -> ref[left, right] Int:\n    if flag:\n        return left\n    else:\n        return right\n"
    )
    .is_ok());
    assert!(matches!(
        check_source(
            "def bad(ref source: Int) -> ref[source] Int:\n    var local = 1\n    return local\n"
        ),
        Err(TypeError::ReturnsReferenceToLocal)
    ));
    assert!(check_source(
        "def pair[origin: Origin[mut=False]](ref[origin] left: Int, ref[origin] right: Int):\n    print(left + right)\n\ndef main():\n    var value = 1\n    pair(value, value)\n"
    )
    .is_ok());
}

#[test]
fn checks_reference_aggregate_permissions_initialization_and_escape() {
    let mutable_box = "@fieldwise_init\nstruct RefBox[origin: Origin[mut=True]]:\n    var value: ref[origin] Int\n\n";
    let immutable_box = "@fieldwise_init\nstruct RefBox[origin: Origin[mut=False]]:\n    var value: ref[origin] Int\n\n";

    assert!(matches!(
        check_source(&format!(
            "{immutable_box}def main():\n    var value = 1\n    ref alias = value\n    var box = RefBox(alias)\n    box.value += 1\n"
        )),
        Err(TypeError::ImmutableBinding(_))
    ));
    assert!(matches!(
        check_source(&format!(
            "{mutable_box}def make() -> RefBox:\n    var value = 1\n    ref alias = value\n    return RefBox(alias)\n"
        )),
        Err(TypeError::ReturnsReferenceToLocal)
    ));
    assert!(check_source(
        "struct RefBox[origin: Origin[mut=True]]:\n    var value: ref[origin] Int\n    def __init__(out self, ref[origin] value: Int):\n        self.value = value\n"
    )
    .is_ok());
    assert!(check_source("struct Seen:\n    var value: ref[UntrackedOrigin] Int\n").is_ok());
    assert!(matches!(
        check_source("struct Hidden:\n    var value: ref[UnsafeAnyOrigin] Int\n"),
        Err(TypeError::Unsupported(message)) if message.contains("UnsafeAnyOrigin")
    ));
    assert!(check_source(
        "@fieldwise_init\nstruct RefTuple[origin: Origin[mut=True]]:\n    var values: Tuple[ref[origin] Int, ref[origin] Int]\n\ndef main():\n    var left = 1\n    var right = 2\n    ref a = left\n    ref b = right\n    var pair = RefTuple((a, b))\n    print(pair.values[0], pair.values[1])\n"
    )
    .is_ok());
    assert!(check_source(
        "@fieldwise_init\nstruct RefList[origin: Origin[mut=True]]:\n    var values: List[ref[origin] Int]\n\ndef main():\n    var left = 1\n    var right = 2\n    ref a = left\n    ref b = right\n    var pair = RefList([a, b])\n    pair.values[1] += 1\n    print(right)\n"
    )
    .is_ok());
}

#[test]
fn handwritten_reference_constructor_borrows_a_noncopyable_actual() {
    assert!(check_source(
        "struct Item:\n    var value: Int\n    def __init__(out self, value: Int):\n        self.value = value\n\nstruct RefBox[origin: Origin[mut=True]]:\n    var value: ref[origin] Item\n    def __init__(out self, ref[origin] value: Item):\n        self.value = value\n\ndef main():\n    var item = Item(1)\n    var box = RefBox(item)\n    print(box.value.value)\n"
    )
    .is_ok());
}

#[test]
fn immutable_returned_reference_cannot_escalate_to_mutable() {
    assert!(matches!(
        check_source(
            "def borrow[origin: Origin[mut=False]](ref[origin] value: Int) -> ref[origin] Int:\n    return value\n\ndef alter[origin: Origin[mut=True]](ref[origin] value: Int):\n    value = 2\n\ndef main():\n    var value = 1\n    ref alias = borrow(value)\n    alter(alias)\n"
        ),
        Err(TypeError::ImmutableBinding(_))
    ));
}

#[test]
fn checks_current_pointer_origin_aggregate_fields() {
    assert!(
        check_source(
            "struct Holder[origin: Origin]:\n    var ptr: UnsafePointer[Int, Self.origin]\n"
        )
        .is_ok()
    );
    assert!(check_source(
        "struct MutableExternal:\n    var ptr: UnsafePointer[Int, MutUntrackedOrigin]\n\nstruct ImmutableExternal:\n    var ptr: UnsafePointer[Int, ImmutUntrackedOrigin]\n\nstruct StaticView:\n    var ptr: UnsafePointer[Int, StaticConstantOrigin]\n"
    )
    .is_ok());
    for origin in ["MutUnsafeAnyOrigin", "ImmutUnsafeAnyOrigin"] {
        let source = format!("struct Hidden:\n    var ptr: UnsafePointer[Int, {origin}]\n");
        assert!(matches!(
            check_source(&source),
            Err(TypeError::Unsupported(message))
                if message.contains("cannot hide") && message.contains(origin)
        ));
    }
}

#[test]
fn pointer_to_place_infers_an_origin_bearing_pointer() {
    ok("def main():\n    var x = 42\n    var p = UnsafePointer(to=x)\n    print(p[0])\n");
    ok("def main():\n    var xs = [1, 2]\n    var p = UnsafePointer(to=xs[0])\n    print(p[0])\n");
    ok(
        "@fieldwise_init\nstruct Pair:\n    var a: Int\n    var b: Int\n\ndef main():\n    var pair = Pair(1, 2)\n    var p = UnsafePointer(to=pair.b)\n    p[0] = 3\n",
    );
}

#[test]
fn pointer_to_place_requires_a_single_to_keyword_place() {
    assert!(matches!(
        err("def main():\n    var p = UnsafePointer(to=1 + 2)\n"),
        TypeError::Unsupported(message) if message.contains("requires a place expression")
    ));
    assert!(matches!(
        err("def main():\n    var x = 1\n    var p = UnsafePointer(at=x)\n"),
        TypeError::BadCall { .. }
    ));
    assert!(matches!(
        err("def main():\n    var x = 1\n    var p = UnsafePointer[Int](to=x)\n"),
        TypeError::Unsupported(message) if message.contains("explicit type")
    ));
    assert!(matches!(
        err("def main():\n    var x = 1\n    ref alias = x\n    var p = UnsafePointer(to=alias)\n"),
        TypeError::Unsupported(message) if message.contains("'ref' binding")
    ));
}

#[test]
fn origin_bearing_pointer_writes_require_mutable_provenance() {
    ok("def main():\n    var x = 1\n    var p = UnsafePointer(to=x)\n    p[0] = 2\n");
    assert!(matches!(
        err("def f(x: Int):\n    var p = UnsafePointer(to=x)\n    p[0] = 1\n\ndef main():\n    f(3)\n"),
        TypeError::Unsupported(message) if message.contains("immutable origin")
    ));
}

#[test]
fn origin_bearing_pointer_rejects_allocation_operations() {
    assert!(matches!(
        err("def main():\n    var x = 1\n    var p = UnsafePointer(to=x)\n    print(p[1])\n"),
        TypeError::Unsupported(message) if message.contains("only offset 0")
    ));
    assert!(matches!(
        err("def main():\n    var x = 1\n    var p = UnsafePointer(to=x)\n    var q = p + 1\n"),
        TypeError::Unsupported(message) if message.contains("arithmetic and comparison")
    ));
    assert!(matches!(
        err(
            "def main():\n    var x = 1\n    var p = UnsafePointer(to=x)\n    var q = UnsafePointer(to=x)\n    print(p == q)\n"
        ),
        TypeError::Unsupported(message) if message.contains("arithmetic and comparison")
    ));
    assert!(matches!(
        err("def main():\n    var x = 1\n    var p = UnsafePointer(to=x)\n    p.free()\n"),
        TypeError::Unsupported(message) if message.contains("does not own an allocation")
    ));
}

#[test]
fn returned_place_pointer_is_a_precise_escape_error() {
    assert!(matches!(
        err(
            "def escape() -> UnsafePointer[Int]:\n    var local = 7\n    return UnsafePointer(to=local)\n\ndef main():\n    print(1)\n"
        ),
        TypeError::PointerEscapesOrigin
    ));
}

#[test]
fn pointer_aggregates_bind_declared_origin_parameters_per_binding() {
    // A mutable place binds a symbolic or mutable field origin.
    ok(
        "@fieldwise_init\nstruct Borrowed[origin: Origin]:\n    var ptr: UnsafePointer[Int, Self.origin]\n\ndef main():\n    var value = 42\n    var b = Borrowed(UnsafePointer(to=value))\n    print(b.ptr[0])\n",
    );
    // An immutable place binds only an explicitly immutable field origin, and
    // writes through the stored pointer stay rejected.
    ok(
        "@fieldwise_init\nstruct View[origin: Origin[mut=False]]:\n    var ptr: UnsafePointer[Int, Self.origin]\n\ndef f(x: Int):\n    var v = View(UnsafePointer(to=x))\n    print(v.ptr[0])\n\ndef main():\n    f(3)\n",
    );
    assert!(matches!(
        err(
            "@fieldwise_init\nstruct Borrowed[origin: Origin]:\n    var ptr: UnsafePointer[Int, Self.origin]\n\ndef f(x: Int):\n    var b = Borrowed(UnsafePointer(to=x))\n\ndef main():\n    f(3)\n"
        ),
        TypeError::TypeMismatch { .. }
    ));
    assert!(matches!(
        err(
            "@fieldwise_init\nstruct View[origin: Origin[mut=False]]:\n    var ptr: UnsafePointer[Int, Self.origin]\n\ndef f(x: Int):\n    var v = View(UnsafePointer(to=x))\n    v.ptr[0] = 1\n\ndef main():\n    f(3)\n"
        ),
        TypeError::Unsupported(message) if message.contains("immutable origin")
    ));
}

#[test]
fn checks_ref_self_return_origin() {
    assert!(check_source(
        "@fieldwise_init\nstruct Box:\n    var value: Int\n    def get(ref self) -> ref[self] Int:\n        return self.value\n"
    )
    .is_ok());
    assert!(matches!(
        check_source(
            "@fieldwise_init\nstruct PairBox:\n    var left: Int\n    var right: Int\n    def bad(ref self) -> ref[origin_of(self.left)] Int:\n        return self.right\n"
        ),
        Err(TypeError::ReturnsReferenceToLocal)
    ));
}

#[test]
fn structs_are_non_copyable_by_default() {
    // Move-only: copying a struct value (binding it to a new variable) is rejected;
    // a `^` transfer, or making the struct `Copyable`, is fine. Scalars copy freely.
    let nc = "@fieldwise_init\nstruct T:\n    var x: Int\n\ndef main():\n    var a: T = T(1)\n    var b: T = a\n    print(b.x)\n";
    assert!(matches!(
        check_source(nc),
        Err(TypeError::NonCopyable { .. })
    ));

    let moved = "@fieldwise_init\nstruct T:\n    var x: Int\n\ndef main():\n    var a: T = T(1)\n    var b: T = a^\n    print(b.x)\n";
    assert!(check_source(moved).is_ok());

    let copyable = "@fieldwise_init\nstruct T(Copyable):\n    var x: Int\n\ndef main():\n    var a: T = T(1)\n    var b: T = a\n    print(b.x)\n";
    assert!(check_source(copyable).is_ok());

    // Scalars are Copyable.
    assert!(
        check_source("def main():\n    var a: Int = 1\n    var b: Int = a\n    print(a + b)\n")
            .is_ok()
    );
}

#[test]
fn ordinary_reference_result_reads_require_copyable_referents() {
    let declarations = "@fieldwise_init\nstruct Token:\n    var value: Int\n    def __del__(deinit self):\n        pass\n\n@fieldwise_init\nstruct Holder:\n    var token: Token\n    def get(ref self) -> ref[origin_of(self.token)] Token:\n        return self.token\n\n";

    assert!(matches!(
        check_source(&format!(
            "{declarations}def main():\n    var holder = Holder(Token(1))\n    var duplicate = holder.get()\n"
        )),
        Err(TypeError::NonCopyable { ty, context })
            if ty == "Token" && context.contains("reference result")
    ));

    assert!(check_source(&format!(
        "{declarations}def main():\n    var holder = Holder(Token(1))\n    ref alias = holder.get()\n    print(alias.value)\n"
    ))
    .is_ok());
}

#[test]
fn trait_bound_diagnostics_name_the_blocking_field_or_operation() {
    let marker = err(
        "@fieldwise_init\nstruct Item:\n    var value: Int\n\n@fieldwise_init\nstruct Box(ImplicitlyCopyable):\n    var item: Item\n",
    );
    assert!(
        marker.to_string().contains("field 'item'")
            && marker.to_string().contains("not ImplicitlyCopyable"),
        "got {marker}"
    );

    let hashable = err(
        "@fieldwise_init\nstruct Item:\n    var value: Int\n\ndef use[K: Hashable](key: K):\n    pass\n\ndef main():\n    var item: Item = Item(1)\n    use(item)\n",
    );
    assert!(
        hashable
            .to_string()
            .contains("missing required operation '__hash__() -> UInt'"),
        "got {hashable}"
    );

    let numeric = err(
        "def magnitude[T: Absable](value: T) -> T:\n    return abs(value)\n\ndef main():\n    print(magnitude(\"nope\"))\n",
    );
    assert!(
        numeric
            .to_string()
            .contains("missing required operation '__abs__() -> Self'"),
        "got {numeric}"
    );
}

#[test]
fn owned_arg_consumes_but_read_and_mut_borrow() {
    let common = "@fieldwise_init\nstruct T:\n    var x: Int\n\n";
    // `owned` consumes → copying a non-Copyable value into it is rejected.
    let owned = format!(
        "{common}def take(var t: T) -> Int:\n    return t.x\n\ndef main():\n    var a: T = T(1)\n    print(take(a))\n"
    );
    assert!(matches!(
        check_source(&owned),
        Err(TypeError::NonCopyable { .. })
    ));
    // `owned` with `^` is a move → fine.
    let moved = format!(
        "{common}def take(var t: T) -> Int:\n    return t.x\n\ndef main():\n    var a: T = T(1)\n    print(take(a^))\n"
    );
    assert!(check_source(&moved).is_ok());
    // `read` (default) and `mut` borrow → no copy, fine.
    let read = format!(
        "{common}def peek(t: T) -> Int:\n    return t.x\n\ndef main():\n    var a: T = T(1)\n    print(peek(a))\n"
    );
    assert!(check_source(&read).is_ok());
    let mutp = format!(
        "{common}def bump(mut t: T):\n    t.x = t.x + 1\n\ndef main():\n    var a: T = T(1)\n    bump(a)\n    print(a.x)\n"
    );
    assert!(check_source(&mutp).is_ok());
}

#[test]
fn borrow_check_rejects_mutable_aliasing() {
    // Mutable-XOR-shared, root-sensitive: a `mut` borrow of a variable must be
    // exclusive for the call.
    let two_mut = "def f(mut a: Int, mut b: Int):\n    a = b\n\ndef main():\n    var x: Int = 5\n    f(x, x)\n";
    assert!(matches!(
        check_source(two_mut),
        Err(TypeError::AliasingViolation { .. })
    ));

    let mut_and_shared = "def f(mut a: Int, b: Int):\n    a = a + b\n\ndef main():\n    var x: Int = 5\n    f(x, x)\n";
    assert!(check_source(mut_and_shared).is_ok());

    let noncopyable = "@fieldwise_init\nstruct Box:\n    var x: Int\n\ndef f(mut a: Box, b: Box):\n    pass\n\ndef main():\n    var x = Box(5)\n    f(x, x)\n";
    assert!(matches!(
        check_source(noncopyable),
        Err(TypeError::AliasingViolation { .. })
    ));

    // Distinct variables, or two shared borrows, are fine.
    let distinct = "def f(mut a: Int, mut b: Int):\n    a = b\n\ndef main():\n    var x: Int = 5\n    var y: Int = 6\n    f(x, y)\n";
    assert!(check_source(distinct).is_ok());
    let two_shared = "def f(a: Int, b: Int) -> Int:\n    return a + b\n\ndef main():\n    var x: Int = 5\n    print(f(x, x))\n";
    assert!(check_source(two_shared).is_ok());
}

#[test]
fn borrow_check_rejects_move_while_borrowed() {
    // Moving a variable (`^`) while it is also borrowed in the same call is a
    // conflict (can't move an aliased value).
    let common = "@fieldwise_init\nstruct T:\n    var x: Int\n\n";
    let mut_and_move = format!(
        "{common}def f(mut a: T, var b: T):\n    a.x = b.x\n\ndef main():\n    var p: T = T(1)\n    f(p, p^)\n"
    );
    assert!(matches!(
        check_source(&mut_and_move),
        Err(TypeError::AliasingViolation { .. })
    ));
}

#[test]
fn deinit_self_is_the_current_destructor_convention() {
    // Current Mojo spells the destructor receiver `deinit self`; the older
    // `deinit self` is the current consuming destructor receiver.
    let deinit = "@fieldwise_init\nstruct R:\n    var id: Int\n    def __del__(deinit self):\n        print(self.id)\n\ndef main():\n    var a: R = R(1)\n    print(a.id)\n";
    assert!(check_source(deinit).is_ok());
    let owned = "@fieldwise_init\nstruct R:\n    var id: Int\n    def __del__(deinit self):\n        print(self.id)\n\ndef main():\n    var a: R = R(1)\n    print(a.id)\n";
    assert!(check_source(owned).is_ok());
}

#[test]
fn borrow_check_is_place_sensitive() {
    // Field-aware: two `mut` borrows of *disjoint* fields of the same variable are
    // fine. An overlapping read of a Copyable value is materialized before the
    // exclusive access, matching Mojo's call semantics.
    let common = "@fieldwise_init\nstruct P(Copyable):\n    var a: Int\n    var b: Int\n\n";
    let disjoint = format!(
        "{common}def f(mut x: Int, mut y: Int):\n    x = y\n\ndef main():\n    var p: P = P(1, 2)\n    f(p.a, p.b)\n    print(p.a)\n"
    );
    assert!(
        check_source(&disjoint).is_ok(),
        "disjoint fields must be allowed"
    );

    let same_field = format!(
        "{common}def f(mut x: Int, y: Int):\n    x = y\n\ndef main():\n    var p: P = P(1, 2)\n    f(p.a, p.a)\n    print(p.a)\n"
    );
    assert!(check_source(&same_field).is_ok());

    let whole_vs_field = format!(
        "{common}def g(mut x: P, y: Int):\n    pass\n\ndef main():\n    var p: P = P(1, 2)\n    g(p, p.a)\n    print(p.a)\n"
    );
    assert!(check_source(&whole_vs_field).is_ok());
}

// --- Operator overloading (dunder dispatch) ---

const VEC2: &str = "@fieldwise_init\nstruct Vec2(Writable):\n    var x: Int\n    def __add__(self, o: Vec2) -> Vec2:\n        return Vec2(self.x + o.x)\n    def __eq__(self, o: Vec2) -> Bool:\n        return self.x == o.x\n    def __getitem__(self, i: Int) -> Int:\n        return self.x\n    def __len__(self) -> Int:\n        return 1\n    def write_to(self, mut writer: Some[Writer]):\n        writer.write(self.x)\n    def __contains__(self, v: Int) -> Bool:\n        return self.x == v\n\n";

#[test]
fn dunder_dispatch_type_checks_operators_and_builtins() {
    // A struct with the right dunders participates in `+`, `==`, subscript, `len`,
    // `String`, and `in` — each typed by the dunder's signature.
    ok(&format!(
        "{VEC2}def main():\n    var a: Vec2 = Vec2(1)\n    var b: Vec2 = a + a\n    var e: Bool = a == b\n    var i: Int = a[0]\n    var n: Int = len(a)\n    var s: StringLiteral = String(a)\n    var m: Bool = 3 in a\n"
    ));
}

#[test]
fn operator_without_dunder_is_rejected() {
    // A struct that doesn't define the operator's dunder still fails to type-check.
    let e = err(
        "@fieldwise_init\nstruct P:\n    var x: Int\n\ndef main():\n    var a: P = P(1)\n    var b: P = a + a\n",
    );
    assert!(matches!(e, TypeError::BadOperator { .. }), "got {e:?}");
    // `!=` is NOT auto-derived from `__eq__` (strict subset — Mojo requires `__ne__`).
    let e = err(
        "@fieldwise_init\nstruct Q:\n    var x: Int\n    def __eq__(self, o: Q) -> Bool:\n        return self.x == o.x\n\ndef main():\n    var m: Bool = Q(1) != Q(2)\n",
    );
    assert!(matches!(e, TypeError::BadOperator { .. }), "got {e:?}");
}

#[test]
fn len_dunder_must_return_int() {
    // `len(x)` requires `__len__ -> Int`; a wrong return type is a type error.
    let e = err(
        "@fieldwise_init\nstruct Bad:\n    var x: Int\n    def __len__(self) -> StringLiteral:\n        return \"nope\"\n\ndef main():\n    var n: Int = len(Bad(1))\n",
    );
    assert!(matches!(e, TypeError::TypeMismatch { .. }), "got {e:?}");
}

#[test]
fn setitem_dunder_typing_and_errors() {
    // `c[i] = e` needs `__setitem__`; a struct with only `__getitem__` can't be
    // assigned into.
    let e = err(
        "@fieldwise_init\nstruct P:\n    var a: Int\n    def __getitem__(self, i: Int) -> Int:\n        return self.a\n\ndef main():\n    var p: P = P(1)\n    p[0] = 9\n",
    );
    assert!(matches!(e, TypeError::NotIndexable(_)), "got {e:?}");
    // `__setitem__` must take `mut self` (else the write couldn't persist).
    let e = err(
        "@fieldwise_init\nstruct P:\n    var a: Int\n    def __setitem__(self, i: Int, v: Int):\n        pass\n\ndef main():\n    var p: P = P(1)\n    p[0] = 9\n",
    );
    assert!(matches!(e, TypeError::TypeMismatch { .. }), "got {e:?}");
    // A well-formed `__setitem__` type-checks (value coerces to the 2nd parameter).
    ok(
        "@fieldwise_init\nstruct P:\n    var a: Int\n    def __setitem__(mut self, i: Int, v: Int):\n        self.a = v\n\ndef main():\n    var p: P = P(1)\n    p[0] = 9\n",
    );

    // Slice and mixed-dimensional assignment use the same checked contract.
    ok(
        "@fieldwise_init\nstruct Grid:\n    var a: Int\n    def __setitem__(mut self, row: Int, columns: Slice, value: Int):\n        self.a = value\n\ndef main():\n    var grid = Grid(0)\n    grid[3, 1:8:2] = 9\n",
    );
    let e = err(
        "@fieldwise_init\nstruct Grid:\n    var a: Int\n    def __setitem__(mut self, row: Int, columns: Slice, value: Int):\n        self.a = value\n\ndef main():\n    var grid = Grid(0)\n    grid[3, 1:8:2] = \"wrong\"\n",
    );
    assert!(
        matches!(
            e,
            TypeError::TypeMismatch { .. } | TypeError::BadCall { .. }
        ),
        "got {e:?}"
    );
    let e = err(
        "@fieldwise_init\nstruct Grid:\n    var a: Int\n    def __setitem__(self, row: Int, columns: Slice, value: Int):\n        pass\n\ndef main():\n    var grid = Grid(0)\n    grid[3, 1:8:2] = 9\n",
    );
    assert!(matches!(e, TypeError::TypeMismatch { .. }), "got {e:?}");

    // Generic setter parameters are inferred from the implicit assignment RHS.
    ok(
        "@fieldwise_init\nstruct Sink:\n    var value: Int\n    def __setitem__[T: Copyable & ImplicitlyDeletable](mut self, index: Int, value: T):\n        self.value = index\n\ndef main():\n    var sink = Sink(0)\n    sink[42] = True\n",
    );
}

#[test]
fn slice_literals_do_not_use_user_implicit_conversions() {
    ok(include_str!(
        "../conformance/fixtures/index_implicit_conversion.mojo"
    ));
    let source = include_str!("../conformance/fixtures/slice_implicit_conversion_rejected.mojo");
    assert!(matches!(
        err(source),
        TypeError::BadCall { .. } | TypeError::TypeMismatch { .. }
    ));
}

#[test]
fn setitem_assignment_scores_positional_and_keyword_rhs_shapes_together() {
    // Mojito extension over the pinned nightly, which currently rejects this
    // focused positional-only/keyword-only setter-overload pair.
    let source = include_str!("../conformance/fixtures/setter_overload_extension.mojo");
    let checked = check_program(&parse(source).expect("parse")).expect("check");
    let contracts = checked
        .expressions()
        .iter()
        .filter(|expression| matches!(expression.syntax.kind, mojito::ast::ExprKind::Index { .. }))
        .filter_map(|expression| {
            expression
                .adjustments
                .iter()
                .find_map(|adjustment| match adjustment {
                    SemanticAdjustment::SelectedCall(contract) => Some(contract),
                    _ => None,
                })
        })
        .collect::<Vec<_>>();
    assert_eq!(contracts.len(), 2);
    assert!(contracts.iter().any(|contract| {
        contract.arguments.iter().any(|argument| {
            argument.source == mojito::checked::CheckedCallArgumentSource::Keyword(0)
                && argument.parameter_ty == mojito::Ty::Bool
        })
    }));
    assert!(contracts.iter().any(|contract| {
        contract.arguments.iter().any(|argument| {
            argument.source == mojito::checked::CheckedCallArgumentSource::Positional(1)
                && argument.parameter_ty == mojito::Ty::Int
        })
    }));

    assert!(matches!(
        err(
            "@fieldwise_init\nstruct Sink:\n    var value: Int\n    def __setitem__(mut self, index: Int, value: Int, /):\n        self.value = value\n    def __setitem__(mut self, index: Int, *, value: Bool):\n        pass\n\ndef main():\n    var sink = Sink(0)\n    sink[1] = \"wrong\"\n"
        ),
        TypeError::BadCall { .. }
    ));
}

#[test]
fn augmented_subscript_retains_distinct_getter_and_setter_contracts() {
    let source = include_str!("../conformance/fixtures/augmented_subscript_contract.mojo");
    let checked = check_program(&parse(source).expect("parse")).expect("check");
    let expression = checked
        .expressions()
        .iter()
        .find(|expression| {
            expression
                .adjustments
                .iter()
                .any(|adjustment| matches!(adjustment, SemanticAdjustment::AugmentedSubscript(_)))
        })
        .expect("augmented subscript checked node");
    let pair = expression
        .adjustments
        .iter()
        .find_map(|adjustment| match adjustment {
            SemanticAdjustment::AugmentedSubscript(pair) => Some(pair),
            _ => None,
        })
        .expect("augmented pair");
    assert!(pair.getter.target.starts_with("Counter.__getitem__"));
    assert!(
        pair.setter
            .as_ref()
            .is_some_and(|setter| setter.target.starts_with("Counter.__setitem__"))
    );
    assert_eq!(pair.operand_ty, mojito::Ty::Int);
    assert_eq!(pair.result_ty, mojito::Ty::Int);
    assert!(expression.adjustments.iter().any(|adjustment| {
        matches!(
            adjustment,
            SemanticAdjustment::SelectedCall(contract)
                if contract.target.starts_with("Counter.__setitem__")
        )
    }));
}

#[test]
fn augmented_subscript_freezes_distinct_getter_and_setter_value_adjustments() {
    use mojito::checked::CheckedCallValueAdjustment;

    let source = "struct Wrapped(Copyable, Movable):\n    var value: Int\n    @implicit\n    def __init__(out self, value: Int):\n        self.value = value\n\n@fieldwise_init\nstruct Box:\n    var value: Int\n    def __getitem__(self, index: Wrapped) -> Int:\n        return self.value + index.value\n    def __setitem__(mut self, index: Int, value: Int):\n        self.value = value + index\n\ndef index() -> Int:\n    return 2\n\ndef main():\n    var box = Box(1)\n    box[index()] += 3\n";
    let checked = check_program(&parse(source).expect("parse")).expect("check");
    let pair = checked
        .expressions()
        .iter()
        .flat_map(|expression| &expression.adjustments)
        .find_map(|adjustment| match adjustment {
            SemanticAdjustment::AugmentedSubscript(pair) => Some(pair),
            _ => None,
        })
        .expect("augmented pair");
    let getter_index = &pair.getter.boundary.arguments[0];
    assert!(
        matches!(
        getter_index.adjustments.as_slice(),
        [CheckedCallValueAdjustment::ImplicitConversion { target }]
            if target.starts_with("Wrapped")
        ),
        "getter adjustments: {:?}",
        getter_index.adjustments
    );
    let setter = pair.setter.as_ref().expect("value getter needs setter");
    let setter_index = &setter.boundary.arguments[0];
    assert!(setter_index.adjustments.is_empty());
    assert_eq!(getter_index.value_source, setter_index.value_source);
}

#[test]
fn augmented_subscript_freezes_setter_only_index_normalization() {
    use mojito::checked::CheckedCallValueAdjustment;

    let source = "@fieldwise_init\nstruct Axis(Indexer):\n    var value: Int\n    def __mlir_index__(self) -> Int:\n        return self.value\n\n@fieldwise_init\nstruct Box:\n    var value: Int\n    def __getitem__(self, index: Axis) -> Int:\n        return self.value\n    def __setitem__(mut self, index: Int, value: Int):\n        self.value = value + index\n\ndef main():\n    var box = Box(1)\n    var axis = Axis(0)\n    box[axis] += 2\n";
    let checked = check_program(&parse(source).expect("parse")).expect("check");
    let pair = checked
        .expressions()
        .iter()
        .flat_map(|expression| &expression.adjustments)
        .find_map(|adjustment| match adjustment {
            SemanticAdjustment::AugmentedSubscript(pair) => Some(pair),
            _ => None,
        })
        .expect("augmented pair");
    assert!(pair.getter.boundary.arguments[0].adjustments.is_empty());
    let setter = pair.setter.as_ref().expect("value getter needs setter");
    assert!(matches!(
        setter.boundary.arguments[0].adjustments.as_slice(),
        [CheckedCallValueAdjustment::IndexNormalization { target }]
            if target.starts_with("Axis.__mlir_index__")
    ));
}

#[test]
fn augmented_subscript_call_effects_remain_attached_to_their_call() {
    let source = "@fieldwise_init\nstruct Box:\n    var value: Int\n    var seen: Int\n    def __getitem__(self, mut index: Int) -> Int:\n        index += 1\n        return self.value\n    def __setitem__(mut self, index: Int, value: Int):\n        self.seen = index\n        self.value = value\n\ndef main():\n    var box = Box(40, -1)\n    var index = 0\n    box[index] += 2\n";
    let checked = check_program(&parse(source).expect("parse")).expect("check");
    let pair = checked
        .expressions()
        .iter()
        .flat_map(|expression| &expression.adjustments)
        .find_map(|adjustment| match adjustment {
            SemanticAdjustment::AugmentedSubscript(pair) => Some(pair),
            _ => None,
        })
        .expect("augmented pair");
    assert!(!pair.getter.boundary.arguments[0].invalidations.is_empty());
    assert!(
        pair.setter
            .as_ref()
            .expect("value getter needs setter")
            .boundary
            .arguments[0]
            .invalidations
            .is_empty()
    );
}

#[test]
fn augmented_subscript_mutable_reference_getter_writes_without_a_setter() {
    let source = "@fieldwise_init\nstruct Box:\n    var value: Int\n    def __getitem__(mut self, index: Int) -> ref[origin_of(self.value)] Int:\n        return self.value\n\ndef main():\n    var box = Box(40)\n    box[0] += 2\n";
    let checked = check_program(&parse(source).expect("parse")).expect("check");
    let pair = checked
        .expressions()
        .iter()
        .flat_map(|expression| &expression.adjustments)
        .find_map(|adjustment| match adjustment {
            SemanticAdjustment::AugmentedSubscript(pair) => Some(pair),
            _ => None,
        })
        .expect("augmented pair");
    assert_eq!(
        pair.getter
            .reference_result
            .as_ref()
            .expect("reference getter")
            .mutability,
        mojito::origin::Mutability::Mutable
    );
    assert!(pair.setter.is_none());
    assert!(pair.value_source.is_none());
}

#[test]
fn augmented_subscript_rejects_an_immutable_reference_getter() {
    let source = "@fieldwise_init\nstruct Box:\n    var marker: Int\n    def __getitem__[origin: Origin[mut=False]](\n        self, ref[origin] index: Int\n    ) -> ref[origin] Int:\n        return index\n\ndef main():\n    var box = Box(0)\n    var index = 40\n    box[index] += 2\n";
    let error = err(source);
    assert!(
        matches!(
            &error,
            TypeError::ImmutableBinding(message)
                if message.contains("immutable reference returned by '__getitem__'")
        ),
        "unexpected error: {error:?}"
    );
}

#[test]
fn multi_indexer_normalization_uses_each_argument_position() {
    let source = "@fieldwise_init\nstruct Axis(Indexer):\n    var value: Int\n    def __mlir_index__(self) -> Int:\n        return self.value\n\n@fieldwise_init\nstruct Grid:\n    var value: Int\n    def __getitem__(self, row: Axis, column: Int) -> Int:\n        return row.value + column\n    def __setitem__(mut self, row: Axis, column: Int, value: Int):\n        self.value = row.value + column + value\n\ndef main():\n    var grid = Grid(0)\n    print(grid[Axis(1), Axis(2)])\n    grid[Axis(3), Axis(4)] = 5\n";
    let checked = check_program(&parse(source).expect("parse")).expect("check");
    let normalizations = checked
        .expressions()
        .iter()
        .flat_map(|expression| &expression.adjustments)
        .filter(|adjustment| matches!(adjustment, SemanticAdjustment::IndexNormalization { .. }))
        .count();
    assert_eq!(normalizations, 2);

    assert!(matches!(
        err(
            "@fieldwise_init\nstruct Axis(Indexer):\n    var value: Int\n    def __mlir_index__(self) -> Int:\n        return self.value\n\n@fieldwise_init\nstruct Plain:\n    var value: Int\n\n@fieldwise_init\nstruct Grid:\n    var value: Int\n    def __getitem__(self, row: Axis, column: Int) -> Int:\n        return row.value + column\n\ndef main():\n    var grid = Grid(0)\n    print(grid[Axis(1), Plain(2)])\n"
        ),
        TypeError::BadCall { .. }
    ));
}

#[test]
fn checked_subscript_retains_the_selected_call_contract() {
    let source = "@fieldwise_init\nstruct Box(Copyable, Movable):\n    var value: Int\n    def __getitem__(\n        ref self, mut index: Int\n    ) raises -> ref[origin_of(self.value)] Int:\n        if index < 0:\n            raise Error(\"negative\")\n        index += 1\n        return self.value\n\ndef main():\n    var box = Box(40)\n    var index = 0\n    try:\n        print(box[index])\n    except error:\n        pass\n";
    let checked = check_program(&parse(source).expect("parse")).expect("check");
    let contract = checked
        .expressions()
        .iter()
        .find_map(|expression| {
            matches!(expression.syntax.kind, mojito::ast::ExprKind::Index { .. }).then(|| {
                expression
                    .adjustments
                    .iter()
                    .find_map(|adjustment| match adjustment {
                        SemanticAdjustment::SelectedCall(contract) => Some(contract),
                        _ => None,
                    })
            })?
        })
        .expect("selected Index contract");
    assert_eq!(contract.target, "Box.__getitem__");
    assert_eq!(contract.raises, Some(mojito::Ty::Error));
    assert!(contract.receiver_requires_place);
    assert_eq!(
        contract.receiver_convention,
        Some(mojito::ast::ArgConvention::Ref)
    );
    assert_eq!(contract.arguments.len(), 1);
    assert_eq!(
        contract.arguments[0].source,
        mojito::checked::CheckedCallArgumentSource::Positional(0)
    );
    assert!(contract.arguments[0].requires_place);
    assert_eq!(
        contract.arguments[0].convention,
        Some(mojito::ast::ArgConvention::Mut)
    );
    assert!(contract.reference_result.is_some());
}

#[test]
fn subscript_contract_rejects_overlapping_mutable_receiver_and_argument() {
    assert!(matches!(
        check_source(include_str!(
            "../conformance/fixtures/subscript_mutable_aliasing.mojo"
        )),
        Err(TypeError::AliasingViolation { .. })
    ));
}

#[test]
fn user_iterator_protocol_typing() {
    // A struct with a valid `__iter__` → iterator (`__next__`/`__len__`) is iterable.
    ok(
        "@fieldwise_init\nstruct I:\n    var c: Int\n    var s: Int\n    def __len__(self) -> Int:\n        return self.s - self.c\n    def __next__(mut self) -> Int:\n        var v: Int = self.c\n        self.c = self.c + 1\n        return v\n\n@fieldwise_init\nstruct C:\n    var n: Int\n    def __iter__(self) -> I:\n        return I(0, self.n)\n\ndef main():\n    for x in C(3):\n        print(x)\n",
    );
    // No `__iter__` → not iterable.
    assert!(matches!(
        err(
            "@fieldwise_init\nstruct P:\n    var x: Int\n\ndef main():\n    for i in P(1):\n        print(i)\n"
        ),
        TypeError::NoSuchMethod { .. }
    ));
    // The iterator's `__next__` must be `mut self` (it advances).
    assert!(matches!(
        err(
            "@fieldwise_init\nstruct I:\n    var c: Int\n    def __len__(self) -> Int:\n        return 0\n    def __next__(self) -> Int:\n        return 0\n@fieldwise_init\nstruct C:\n    var n: Int\n    def __iter__(self) -> I:\n        return I(0)\n\ndef main():\n    for x in C(1):\n        print(x)\n"
        ),
        TypeError::TypeMismatch { .. }
    ));
}

#[test]
fn raising_iterator_protocol_uses_typed_stop_iteration_without_len() {
    let source = "@fieldwise_init\nstruct StopIteration:\n    var marker: Int\n\n@fieldwise_init\nstruct I:\n    var current: Int\n    var end: Int\n    def __next__(mut self) raises StopIteration -> Int:\n        if self.current >= self.end:\n            raise StopIteration(0)\n        var result = self.current\n        self.current += 1\n        return result\n\n@fieldwise_init\nstruct C:\n    var end: Int\n    def __iter__(self) -> I:\n        return I(0, self.end)\n\ndef main():\n    for value in C(3):\n        print(value)\n";
    let program = parse(source).expect("parse value-yielding iterator");
    let checked = check_program(&program).expect("check value-yielding iterator");
    let value = checked
        .declarations()
        .iter()
        .find(|declaration| declaration.name == "value")
        .expect("loop binding");
    assert_eq!(value.ty.as_ref(), Some(&mojito::Ty::Int));

    let error = err(
        "@fieldwise_init\nstruct OtherError:\n    var marker: Int\n\n@fieldwise_init\nstruct I:\n    var current: Int\n    def __next__(mut self) raises OtherError -> Int:\n        raise OtherError(0)\n        return self.current\n\n@fieldwise_init\nstruct C:\n    var end: Int\n    def __iter__(self) -> I:\n        return I(self.end)\n\ndef main():\n    for value in C(3):\n        print(value)\n",
    );
    assert!(
        matches!(error, TypeError::TypeMismatch { ref context, .. }
            if context == "iterator '__next__' exhaustion contract"),
        "got {error:?}"
    );
}

#[test]
fn raising_reference_iterator_preserves_the_reference_loop_binding_type() {
    let source = "@fieldwise_init\nstruct StopIteration:\n    var marker: Int\n\n@fieldwise_init\nstruct RefIter[o: Origin[mut=False]]:\n    var source: ref[o] Int\n    var done: Bool\n    def __next__(mut self) raises StopIteration -> ref[o] Int:\n        if self.done:\n            raise StopIteration(0)\n        self.done = True\n        return self.source\n\n@fieldwise_init\nstruct RefSource:\n    var value: Int\n    def __iter__(ref self) -> RefIter:\n        ref value = self.value\n        return RefIter(value, False)\n\ndef main():\n    var source = RefSource(42)\n    for item in source:\n        print(item)\n";
    let program = parse(source).expect("parse reference-yielding iterator");
    let checked = check_program(&program).expect("check reference-yielding iterator");
    let item = checked
        .declarations()
        .iter()
        .find(|declaration| declaration.name == "item")
        .expect("loop binding");
    assert!(matches!(
        item.ty.as_ref(),
        Some(mojito::Ty::Ref(reference)) if reference.referent.as_ref() == &mojito::Ty::Int
    ));
}

#[test]
fn iterator_protocol_respects_conditional_method_availability() {
    let prelude = "@fieldwise_init\nstruct Token(Movable):\n    var value: Int\n\n";

    // Merely finding `__iter__` by name is insufficient: its receiver
    // specialization must satisfy the method's `where` clause.
    let unavailable_iter = format!(
        "{prelude}@fieldwise_init\nstruct I:\n    var index: Int\n    def __len__(self) -> Int:\n        return 0\n    def __next__(mut self) -> Int:\n        return 0\n\n@fieldwise_init\nstruct C[T: Movable]:\n    var marker: Int\n    def __iter__(self) -> I where conforms_to(Self.T, Copyable):\n        return I(0)\n\ndef main():\n    var values = C[Token](0)\n    for value in values:\n        print(value)\n"
    );
    assert!(matches!(
        err(&unavailable_iter),
        TypeError::TypeMismatch { ref context, .. }
            if context == "for-loop iterator selection"
    ));

    // The same specialization rule applies after normalization, both to the
    // advancing operation and to the legacy bounded protocol's length query.
    let unavailable_next = format!(
        "{prelude}@fieldwise_init\nstruct I[T: Movable]:\n    var index: Int\n    def __len__(self) -> Int:\n        return 0\n    def __next__(mut self) -> Int where conforms_to(Self.T, Copyable):\n        return 0\n\n@fieldwise_init\nstruct C[T: Movable]:\n    var marker: Int\n    def __iter__(self) -> I[Self.T]:\n        return I[Self.T](0)\n\ndef main():\n    var values = C[Token](0)\n    for value in values:\n        print(value)\n"
    );
    assert!(matches!(
        err(&unavailable_next),
        TypeError::NoSuchMethod { ref method, .. } if method == "__next__"
    ));

    let unavailable_len = format!(
        "{prelude}@fieldwise_init\nstruct I[T: Movable]:\n    var index: Int\n    def __len__(self) -> Int where conforms_to(Self.T, Copyable):\n        return 0\n    def __next__(mut self) -> Int:\n        return 0\n\n@fieldwise_init\nstruct C[T: Movable]:\n    var marker: Int\n    def __iter__(self) -> I[Self.T]:\n        return I[Self.T](0)\n\ndef main():\n    var values = C[Token](0)\n    for value in values:\n        print(value)\n"
    );
    assert!(matches!(
        err(&unavailable_len),
        TypeError::NoSuchMethod { ref method, .. } if method == "__len__"
    ));
}

#[test]
fn raising_iter_normalization_obeys_the_enclosing_effect_context() {
    let declarations = "@fieldwise_init\nstruct IterError:\n    var code: Int\n\n@fieldwise_init\nstruct I:\n    var index: Int\n    def __len__(self) -> Int:\n        return 0\n    def __next__(mut self) -> Int:\n        return 0\n\n@fieldwise_init\nstruct C:\n    var marker: Int\n    def __iter__(self) raises IterError -> I:\n        raise IterError(self.marker)\n        return I(0)\n\n";

    assert!(matches!(
        err(&format!(
            "{declarations}def main():\n    for value in C(7):\n        print(value)\n"
        )),
        TypeError::UnhandledRaise(operation)
            if operation.contains("implicit call") && operation.contains("C.__iter__")
    ));

    ok(&format!(
        "{declarations}def run() raises IterError:\n    for value in C(7):\n        print(value)\n"
    ));
    ok(&format!(
        "{declarations}def main():\n    try:\n        for value in C(7):\n            print(value)\n    except error:\n        print(error.code)\n"
    ));
}

#[test]
fn unsafe_pointer_typing() {
    // alloc + index read/write + free type-check.
    ok(
        "def main():\n    var p: UnsafePointer[Int] = UnsafePointer[Int].alloc(4)\n    p[0] = 1\n    var x: Int = p[0]\n    p.free()\n",
    );
    // `alloc` needs an Int count.
    assert!(matches!(
        err("def main():\n    var p: UnsafePointer[Int] = UnsafePointer[Int].alloc(\"x\")\n"),
        TypeError::TypeMismatch { .. }
    ));
    // A bare parameterized type is not a value.
    assert!(matches!(
        err("def main():\n    var x = UnsafePointer[Int]\n    print(1)\n"),
        TypeError::TypeMismatch { .. }
    ));
    // A store must match the pointee type.
    assert!(matches!(
        err(
            "def main():\n    var p: UnsafePointer[Int] = UnsafePointer[Int].alloc(1)\n    p[0] = \"no\"\n"
        ),
        TypeError::TypeMismatch { .. }
    ));
}

#[test]
fn compiler_private_pointer_storage_methods_are_not_user_visible() {
    for method in ["take", "destroy"] {
        let source = format!(
            "def main():\n    var p = UnsafePointer[Int].alloc(1)\n    p[0] = 7\n    var result = p.{method}(0)\n"
        );
        assert!(
            matches!(err(&source), TypeError::NoSuchMethod { method: ref found, .. } if found == method),
            "ordinary source unexpectedly acquired UnsafePointer.{method}"
        );
    }
}

#[test]
fn copyinit_makes_type_copyable_and_checks_di() {
    // Defining `__copyinit__` makes a struct Copyable, so `var q = p` is allowed.
    ok(
        "struct P:\n    var a: Int\n    def __init__(out self):\n        self.a = 1\n    def __copyinit__(out self, e: P):\n        self.a = e.a\n\ndef main():\n    var p: P = P()\n    var q: P = p\n    print(q.a)\n",
    );
    // Current Mojo spells the copy constructor as an `__init__` overload with a
    // keyword-only `copy: Self`; mojito registers that as the lifecycle copy
    // initializer internally.
    ok(
        "struct Q:\n    var a: Int\n    def __init__(out self):\n        self.a = 1\n    def __init__(out self, *, copy: Self):\n        self.a = copy.a\n\ndef main():\n    var p: Q = Q()\n    var q: Q = Q(copy: p)\n    print(q.a)\n",
    );
    // A struct without `__copyinit__`/Copyable is move-only: `var q = p` is rejected.
    assert!(matches!(
        err(
            "struct P:\n    var a: Int\n    def __init__(out self):\n        self.a = 1\n\ndef main():\n    var p: P = P()\n    var q: P = p\n"
        ),
        TypeError::NonCopyable { .. }
    ));
    // Definite-init applies to `__copyinit__` too (must set every field).
    assert!(matches!(
        err(
            "struct P:\n    var a: Int\n    var b: Int\n    def __init__(out self):\n        self.a = 0\n        self.b = 0\n    def __copyinit__(out self, e: P):\n        self.a = e.a\n\ndef main():\n    var p: P = P()\n    var q: P = p\n"
        ),
        TypeError::UninitializedField { .. }
    ));
}

#[test]
fn generic_hand_written_init() {
    // A hand-written `__init__` on a *generic* struct: the type parameter is solved
    // by unifying the constructor's parameters against the arguments (inferred), or
    // supplied explicitly (`Box[Int](5)`).
    ok(
        "struct Box[T: Copyable & Movable]:\n    var v: Self.T\n    def __init__(out self, v: Self.T):\n        self.v = v\n    def get(self) -> Self.T:\n        return self.v\n\ndef main():\n    var a: Box[Int] = Box(5)\n    var b: Box[Int] = Box[Int](6)\n    print(a.get(), b.get())\n",
    );
    // A UnsafePointer field of the type parameter, allocated with `Self.T`.
    ok(
        "struct Buf[T: Copyable & Movable]:\n    var data: UnsafePointer[Self.T]\n    def __init__(out self):\n        self.data = UnsafePointer[Self.T].alloc(4)\n    def set0(mut self, v: Self.T):\n        self.data[0] = v\n    def get0(self) -> Self.T:\n        return self.data[0]\n\ndef main():\n    var b: Buf[Int] = Buf[Int]()\n    b.set0(9)\n    print(b.get0())\n",
    );
    // A wrong-typed constructor argument is still rejected (solved T = Int here).
    assert!(matches!(
        err(
            "struct Box[T: Copyable & Movable]:\n    var v: Self.T\n    def __init__(out self, v: Self.T):\n        self.v = v\n\ndef main():\n    var a: Box[Int] = Box[Int](\"no\")\n"
        ),
        TypeError::TypeMismatch { .. }
    ));
}

#[test]
fn comparable_bound_permits_ordering() {
    // `<`/`<=`/`>`/`>=` between equal opaque type parameters type-check when the
    // parameter is bounded by `Comparable` (Phase 4).
    ok("def less[T: Comparable](a: T, b: T) -> Bool:\n    return a < b\n");
    ok(
        "def ordered[T: Comparable](a: T, b: T) -> Bool:\n    return a < b and a <= b and a > b and a >= b\n",
    );
    // `Comparable` implies equality-capable in mojito (as in current Mojo).
    ok("def eq[T: Comparable](a: T, b: T) -> Bool:\n    return a == b\n");
}

#[test]
fn equatable_bound_does_not_permit_ordering() {
    // A plain `T: Equatable` grants `==`/`!=` but *not* ordering — `<` on such a
    // `T` is a `BadOperator` (Phase 4).
    ok("def eq[T: Equatable](a: T, b: T) -> Bool:\n    return a == b\n");
    assert!(matches!(
        err("def less[T: Equatable](a: T, b: T) -> Bool:\n    return a < b\n"),
        TypeError::BadOperator { .. }
    ));
}

#[test]
fn sized_bound_permits_len() {
    // `len(x)` on an opaque type parameter type-checks when the parameter carries
    // a `Sized` bound (Phase 5). `SizedRaising` also promises a length.
    ok("def is_empty[T: Sized](x: T) -> Bool:\n    return len(x) == 0\n");
    ok("def sz[T: Sized](x: T) -> Int:\n    return len(x)\n");
    ok("def szr[T: SizedRaising](x: T) -> Int:\n    return len(x)\n");
}

#[test]
fn any_type_bound_does_not_permit_len() {
    // A plain `T: AnyType` promises no length — `len(x)` on it is a type error
    // (Phase 5 stop point: `Sized` enables container helpers, `AnyType` does not).
    assert!(matches!(
        err("def is_empty[T: AnyType](x: T) -> Bool:\n    return len(x) == 0\n"),
        TypeError::TypeMismatch { .. }
    ));
}

#[test]
fn hashable_bound_permits_hash() {
    // `x.__hash__()` type-checks (→ `UInt`) on an opaque `K: Hashable` (Phase 6),
    // and on concrete hashable built-ins (so a key struct can combine fields).
    ok("def h[K: Hashable](k: K) -> UInt:\n    return k.__hash__()\n");
    ok("def hi(n: Int) -> UInt:\n    return n.__hash__()\n");
    ok("def hs(s: StringLiteral) -> UInt:\n    return s.__hash__()\n");
}

#[test]
fn hashable_bound_does_not_permit_equality() {
    // Design decision: `Hashable` does *not* imply `Equatable` — `==` on a
    // `K: Hashable` (without an `Equatable` bound) is a `BadOperator` (Phase 6).
    assert!(matches!(
        err("def eq[K: Hashable](a: K, b: K) -> Bool:\n    return a == b\n"),
        TypeError::BadOperator { .. }
    ));
    // Naming both bounds restores equality (a hash-backed key's typical bound).
    ok("def eq[K: Hashable & Equatable](a: K, b: K) -> Bool:\n    return a == b\n");
}

#[test]
fn any_type_bound_does_not_permit_hash() {
    // A plain `T: AnyType` promises no hash — `x.__hash__()` on it is rejected.
    assert!(matches!(
        err("def h[T: AnyType](x: T) -> UInt:\n    return x.__hash__()\n"),
        TypeError::NoSuchMethod { .. }
    ));
}

#[test]
fn numeric_operation_traits_permit_their_operation() {
    // Phase 7: a numeric-operation bound enables the matching builtin/operator on
    // an opaque `T`, returning the operation's result type.
    ok("def absolute[T: Absable](x: T) -> T:\n    return abs(x)\n");
    ok("def rounded[T: Roundable](x: T) -> T:\n    return round(x)\n");
    ok("def powit[T: Powable](x: T, y: T) -> T:\n    return x ** y\n");
    ok("def to_int[T: Intable](x: T) -> Int:\n    return Int(x)\n");
    ok("def to_flt[T: Floatable](x: T) -> Float64:\n    return Float64(x)\n");
    // `Boolable` -> `Bool(x)`, and `DivModable` -> `divmod(a, b)` (prelude
    // builtins), plus the `math`-module rounding dunders on their bounds.
    ok("def to_bool[T: Boolable](x: T) -> Bool:\n    return Bool(x)\n");
    ok("def dm[T: DivModable](a: T, b: T) -> Tuple[T, T]:\n    return divmod(a, b)\n");
    ok("def flr[T: Floorable](x: T) -> T:\n    return x.__floor__()\n");
    ok("def cl[T: Ceilable](x: T) -> T:\n    return x.__ceil__()\n");
    ok("def tr[T: Truncable](x: T) -> T:\n    return x.__trunc__()\n");
    ok("def cd[T: CeilDivable](a: T, b: T) -> T:\n    return a.__ceildiv__(b)\n");
    // The raising sibling grants `__ceildiv__` too (effect model is deferred).
    ok("def cdr[T: CeilDivableRaising](a: T, b: T) -> T:\n    return a.__ceildiv__(b)\n");
    // Concrete numerics also convert/divmod directly.
    ok("def main():\n    print(Bool(0), divmod(7, 2)[0])\n");
}

#[test]
fn any_type_bound_does_not_permit_numeric_operations() {
    // A plain `T: AnyType` grants none of the numeric operations.
    assert!(matches!(
        err("def absolute[T: AnyType](x: T) -> T:\n    return abs(x)\n"),
        TypeError::TypeMismatch { .. }
    ));
    assert!(matches!(
        err("def rounded[T: AnyType](x: T) -> T:\n    return round(x)\n"),
        TypeError::TypeMismatch { .. }
    ));
    assert!(matches!(
        err("def powit[T: AnyType](x: T, y: T) -> T:\n    return x ** y\n"),
        TypeError::BadOperator { .. }
    ));
    assert!(matches!(
        err("def to_int[T: AnyType](x: T) -> Int:\n    return Int(x)\n"),
        TypeError::TypeMismatch { .. }
    ));
    // A bound also does not grant an *unrelated* operation: `Absable` gives `abs`
    // but not `**`.
    assert!(matches!(
        err("def powit[T: Absable](x: T, y: T) -> T:\n    return x ** y\n"),
        TypeError::BadOperator { .. }
    ));
    // `Bool(x)`/`divmod` and the rounding dunders are likewise gated.
    assert!(matches!(
        err("def to_bool[T: AnyType](x: T) -> Bool:\n    return Bool(x)\n"),
        TypeError::TypeMismatch { .. }
    ));
    assert!(matches!(
        err("def dm[T: AnyType](a: T, b: T) -> Tuple[T, T]:\n    return divmod(a, b)\n"),
        TypeError::BadOperator { .. }
    ));
    assert!(matches!(
        err("def flr[T: AnyType](x: T) -> T:\n    return x.__floor__()\n"),
        TypeError::NoSuchMethod { .. }
    ));
    // `Floorable` grants `__floor__` but not `__ceildiv__`.
    assert!(matches!(
        err("def cd[T: Floorable](a: T, b: T) -> T:\n    return a.__ceildiv__(b)\n"),
        TypeError::NoSuchMethod { .. }
    ));
}

#[test]
fn operator_traits_gate_generic_numeric_code() {
    // Generic arithmetic bounded by an operation trait type-checks.
    ok(
        "def sum3[T: Addable](a: T, b: T, c: T) -> T:\n    return a + b + c\n\ndef main():\n    print(sum3(1, 2, 3))\n    print(sum3(1.5, 2.5, 3.0))\n",
    );
    // Shift/bitwise operation traits are integer-only.
    assert!(matches!(
        err(
            "def f[T: ShiftLeftable](a: T, b: T) -> T:\n    return a << b\n\ndef main():\n    print(f(1.5, 2.0))\n"
        ),
        TypeError::TraitNotSatisfied { .. }
    ));
    // Bool does not conform to an arithmetic trait.
    assert!(matches!(
        err("def g[T: Addable](a: T) -> T:\n    return a + a\n\ndef main():\n    print(g(True))\n"),
        TypeError::TraitNotSatisfied { .. }
    ));
}

#[test]
fn operation_trait_conformance_requires_the_dunder() {
    // Declaring an operation trait without its dunder is rejected.
    assert!(matches!(
        check_source("struct Bad(Addable):\n    pass\n"),
        Err(TypeError::TraitNotSatisfied { .. })
    ));
    // Defining the dunder satisfies the declared conformance.
    ok(
        "@fieldwise_init\nstruct Good(Addable):\n    var n: Int\n    def __add__(self, other: Good) -> Good:\n        return Good(self.n + other.n)\n",
    );
}

#[test]
fn user_struct_prefix_operators_type_check() {
    ok(
        "@fieldwise_init\nstruct V:\n    var n: Int\n    def __neg__(self) -> V:\n        return V(-self.n)\n\ndef main():\n    var a = V(3)\n    print((-a).n)\n",
    );
    ok(
        "@fieldwise_init\nstruct Flag:\n    var on: Bool\n    def __bool__(self) -> Bool:\n        return self.on\n\ndef main():\n    var f = Flag(False)\n    if not f:\n        print(\"off\")\n",
    );
}

#[test]
fn concrete_conversions_and_abs_route_through_dunders() {
    // A concrete struct with the conversion/abs dunders type-checks the builtins.
    ok(
        "@fieldwise_init\nstruct M:\n    var n: Int\n    def __int__(self) -> Int:\n        return self.n\n    def __abs__(self) -> M:\n        return M(self.n)\n\ndef main():\n    print(Int(M(1)))\n    print(abs(M(2)).n)\n",
    );
    // A struct lacking the dunder is still rejected.
    assert!(matches!(
        err("@fieldwise_init\nstruct P:\n    var n: Int\n\ndef main():\n    print(Int(P(1)))\n"),
        TypeError::TypeMismatch { .. }
    ));
}

#[test]
fn conditional_iterable_conformance_proves_its_associated_iterator_contract() {
    ok(
        "trait IteratorContract:\n    comptime Element: Movable\n    def next(self) -> Self.Element: ...\n\ntrait IterableContract:\n    comptime Element: Movable\n    comptime Iter: IteratorContract\n    def iter(self) -> Self.Iter: ...\n\n@fieldwise_init\nstruct Cursor[T: Movable](IteratorContract where conforms_to(T, Copyable)):\n    comptime Element = Self.T\n    var value: Self.T\n    def next(self) -> Self.T where conforms_to(Self.T, Copyable):\n        return self.value\n\n@fieldwise_init\nstruct Container[T: Movable](IterableContract where conforms_to(T, Copyable)):\n    comptime Element = Self.T\n    comptime Iter = Cursor[Self.T]\n    var value: Self.T\n    def iter(self) -> Cursor[Self.T] where conforms_to(Self.T, Copyable):\n        return Cursor[Self.T](self.value)\n",
    );
}

#[test]
fn unrelated_conditional_conformance_does_not_prove_an_iterator_contract() {
    let error = err(
        "trait IteratorContract:\n    comptime Element: Movable\n    def next(self) -> Self.Element: ...\n\ntrait IterableContract:\n    comptime Element: Movable\n    comptime Iter: IteratorContract\n    def iter(self) -> Self.Iter: ...\n\n@fieldwise_init\nstruct Cursor[T: Movable](IteratorContract where conforms_to(T, Copyable)):\n    comptime Element = Self.T\n    var value: Self.T\n    def next(self) -> Self.T where conforms_to(Self.T, Copyable):\n        return self.value\n\n@fieldwise_init\nstruct BadContainer[T: Movable](IterableContract where conforms_to(T, Equatable)):\n    comptime Element = Self.T\n    comptime Iter = Cursor[Self.T]\n    var value: Self.T\n    def iter(self) -> Cursor[Self.T] where conforms_to(Self.T, Equatable):\n        return Cursor[Self.T](self.value)\n",
    );
    assert!(
        matches!(
            &error,
            TypeError::TraitComptimeMemberMismatch { .. }
                | TypeError::TraitMethodMismatch { .. }
                | TypeError::TraitNotSatisfied { .. }
        ),
        "got {error:?}"
    );
}

#[test]
fn self_origin_resolves_a_parameterized_associated_member_on_a_conformer() {
    // A trait method returning `Self.IterType[origin_of(self)]` has no bound
    // `self` in its abstract signature; the origin argument is the symbolic
    // self-origin. A conforming struct's member (`= Bag`) then resolves the
    // requirement concretely (the origin erases), so a concrete `-> Bag`
    // implementation satisfies it.
    ok(
        "trait HasIter:\n    comptime IterType[mut: Bool, //, origin: Origin[mut=mut]]: AnyType\n    def make(ref self) -> Self.IterType[origin_of(self)]: ...\n\n@fieldwise_init\nstruct Bag(Copyable, Movable):\n    var n: Int\n\n@fieldwise_init\nstruct Holder(HasIter):\n    comptime IterType[mut: Bool, //, origin: Origin[mut=mut]] = Bag\n    var v: Int\n    def make(ref self) -> Bag:\n        return Bag(self.v)\n",
    );
}

#[test]
fn self_origin_requirement_resolves_concretely_and_rejects_a_mismatch() {
    // The resolved requirement is exactly the conformer's member (`Bag`), so a
    // method returning a different type fails conformance — proving the
    // application resolved concretely rather than staying unconstrained.
    let error = err(
        "trait HasIter:\n    comptime IterType[mut: Bool, //, origin: Origin[mut=mut]]: AnyType\n    def make(ref self) -> Self.IterType[origin_of(self)]: ...\n\n@fieldwise_init\nstruct Bag(Copyable, Movable):\n    var n: Int\n\n@fieldwise_init\nstruct Other(Copyable, Movable):\n    var n: Int\n\n@fieldwise_init\nstruct Holder(HasIter):\n    comptime IterType[mut: Bool, //, origin: Origin[mut=mut]] = Bag\n    var v: Int\n    def make(ref self) -> Other:\n        return Other(self.v)\n",
    );
    assert!(
        matches!(
            &error,
            TypeError::TraitMethodMismatch { .. } | TypeError::TraitNotSatisfied { .. }
        ),
        "got {error:?}"
    );
}

#[test]
fn conformer_may_return_the_parameterized_associated_application_directly() {
    // A conforming struct spells the application as its own return type
    // (`-> Self.IterType[origin_of(self)]`). With the concrete struct substituted
    // for `Self`, the indexed application resolves through the struct's member
    // (`= Bag`) instead of failing as a dependent index.
    ok(
        "trait HasIter:\n    comptime IterType[mut: Bool, //, origin: Origin[mut=mut]]: AnyType\n    def make(ref self) -> Self.IterType[origin_of(self)]: ...\n\n@fieldwise_init\nstruct Bag(Copyable, Movable):\n    var n: Int\n\n@fieldwise_init\nstruct Holder(HasIter):\n    comptime IterType[mut: Bool, //, origin: Origin[mut=mut]] = Bag\n    var v: Int\n    def make(ref self) -> Self.IterType[origin_of(self)]:\n        return Bag(self.v)\n",
    );
}

#[test]
fn conformer_return_application_resolves_to_the_concrete_member() {
    // `Self.IterType[origin_of(self)]` resolves to the struct's member `Bag`, so a
    // body returning a different type is a return mismatch — proving the
    // application resolved concretely rather than staying an unconstrained index.
    let error = err(
        "trait HasIter:\n    comptime IterType[mut: Bool, //, origin: Origin[mut=mut]]: AnyType\n    def make(ref self) -> Self.IterType[origin_of(self)]: ...\n\n@fieldwise_init\nstruct Bag(Copyable, Movable):\n    var n: Int\n\n@fieldwise_init\nstruct Other(Copyable, Movable):\n    var n: Int\n\n@fieldwise_init\nstruct Holder(HasIter):\n    comptime IterType[mut: Bool, //, origin: Origin[mut=mut]] = Bag\n    var v: Int\n    def make(ref self) -> Self.IterType[origin_of(self)]:\n        return Other(self.v)\n",
    );
    assert!(
        matches!(&error, TypeError::TypeMismatch { .. }),
        "got {error:?}"
    );
}

#[test]
fn parameterized_associated_application_rejects_a_wrong_explicit_arity() {
    // The application supplies the explicit (post-`//`) arguments; one origin
    // argument under-applies a two-explicit-parameter member.
    let error = err(
        "trait HasIter:\n    comptime IterType[a_mut: Bool, b_mut: Bool, //, a: Origin[mut=a_mut], b: Origin[mut=b_mut]]: AnyType\n    def make(ref self) -> Self.IterType[origin_of(self)]: ...\n",
    );
    assert!(
        matches!(&error, TypeError::WrongTypeArgCount { .. }),
        "got {error:?}"
    );
}

#[test]
fn parameterized_member_must_satisfy_its_declared_bound() {
    // Conformance instantiates the definition's template with placeholder
    // explicit arguments and enforces the requirement's bound; a member that
    // resolves to a non-conforming type is rejected, not waved through on
    // arity alone.
    let error = err(
        "trait NextLike:\n    def step(self) -> Int: ...\n\n@fieldwise_init\nstruct Bag(Copyable, Movable):\n    var n: Int\n\ntrait HasIter:\n    comptime IterType[mut: Bool, //, origin: Origin[mut=mut]]: NextLike\n    def make(ref self) -> Self.IterType[origin_of(self)]: ...\n\n@fieldwise_init\nstruct Holder(HasIter):\n    comptime IterType[mut: Bool, //, origin: Origin[mut=mut]] = Bag\n    var v: Int\n    def make(ref self) -> Bag:\n        return Bag(self.v)\n",
    );
    assert!(
        matches!(&error, TypeError::TraitComptimeMemberMismatch { .. }),
        "got {error:?}"
    );
}

#[test]
fn parameterized_member_satisfying_its_bound_conforms() {
    ok(
        "trait NextLike:\n    def step(self) -> Int: ...\n\n@fieldwise_init\nstruct Stepper(Copyable, Movable, NextLike):\n    var n: Int\n    def step(self) -> Int:\n        return self.n\n\ntrait HasIter:\n    comptime IterType[mut: Bool, //, origin: Origin[mut=mut]]: NextLike\n    def make(ref self) -> Self.IterType[origin_of(self)]: ...\n\n@fieldwise_init\nstruct Holder(HasIter):\n    comptime IterType[mut: Bool, //, origin: Origin[mut=mut]] = Stepper\n    var v: Int\n    def make(ref self) -> Self.IterType[origin_of(self)]:\n        return Stepper(self.v)\n",
    );
}

#[test]
fn conditional_conformance_discharges_a_parameterized_member_bound() {
    // The member's `IteratorContract` bound on `Cursor[Self.T]` holds only
    // where `conforms_to(T, Copyable)`; the struct's conformance condition is
    // the assumption that discharges it — the shape the origin-parameterized
    // collection iterators use.
    ok(
        "trait IteratorContract:\n    comptime Element: Movable\n    def next(self) -> Self.Element: ...\n\ntrait IterableContract:\n    comptime Element: Movable\n    comptime IterType[mut: Bool, //, origin: Origin[mut=mut]]: IteratorContract\n    def make(ref self) -> Self.IterType[origin_of(self)]: ...\n\n@fieldwise_init\nstruct Cursor[T: Movable](IteratorContract where conforms_to(T, Copyable)):\n    comptime Element = Self.T\n    var value: Self.T\n    def next(self) -> Self.T where conforms_to(Self.T, Copyable):\n        return self.value\n\n@fieldwise_init\nstruct Container[T: Movable](IterableContract where conforms_to(T, Copyable)):\n    comptime Element = Self.T\n    comptime IterType[mut: Bool, //, origin: Origin[mut=mut]] = Cursor[Self.T]\n    var value: Self.T\n    def make(ref self) -> Self.IterType[origin_of(self)] where conforms_to(Self.T, Copyable):\n        return Cursor[Self.T](self.value)\n",
    );
}

#[test]
fn owned_iterable_conformance_binds_iterator_owned_type() {
    // The owned protocol's associated iterator is the monomorphic
    // `IteratorOwnedType` (current Mojo's owned-iterator member), consumed by an
    // ownership-taking `__iter__(var self)`.
    ok(
        "trait Iterator:\n    comptime Element: Movable\n    def __next__(mut self) -> Self.Element: ...\n\ntrait IterableOwned:\n    comptime Element: Movable\n    comptime IteratorOwnedType: Iterator\n    def __iter__(var self) -> Self.IteratorOwnedType: ...\n\n@fieldwise_init\nstruct Drain[T: Movable](Iterator):\n    comptime Element = Self.T\n    var value: Self.T\n    def __next__(mut self) -> Self.T:\n        return self.value^\n\n@fieldwise_init\nstruct Bucket[T: Movable](IterableOwned):\n    comptime Element = Self.T\n    comptime IteratorOwnedType = Drain[Self.T]\n    var value: Self.T\n    def __iter__(var self) -> Drain[Self.T]:\n        return Drain[Self.T](self.value^)\n",
    );
}

#[test]
fn owned_iterable_rejects_the_legacy_owned_iter_member_name() {
    // The owned protocol's iterator member is now `IteratorOwnedType`. A
    // conformer that keeps the pre-migration `OwnedIter` name cannot name its own
    // `Self.IteratorOwnedType`, the exact contract `IterableOwned.__iter__`
    // requires — the same failure the bundled `List` would hit if the trait and
    // its conformance drifted apart.
    let error = err(
        "trait Iterator:\n    comptime Element: Movable\n    def __next__(mut self) -> Self.Element: ...\n\ntrait IterableOwned:\n    comptime Element: Movable\n    comptime IteratorOwnedType: Iterator\n    def __iter__(var self) -> Self.IteratorOwnedType: ...\n\n@fieldwise_init\nstruct IntDrain(Iterator):\n    comptime Element = Int\n    var value: Int\n    def __next__(mut self) -> Int:\n        return self.value\n\n@fieldwise_init\nstruct IntBucket(IterableOwned):\n    comptime Element = Int\n    comptime OwnedIter = IntDrain\n    var value: Int\n    def __iter__(var self) -> Self.IteratorOwnedType:\n        return IntDrain(self.value)\n",
    );
    assert!(
        matches!(&error, TypeError::NoSuchAssociatedType { .. }),
        "got {error:?}"
    );
}

#[test]
fn variadic_struct_template_cannot_be_checked_raw() {
    // A `struct S[*Ts]` template is compiled by compile-time specialization;
    // reaching the checker unspecialized (i.e. without elaboration) is rejected
    // with a contextual error rather than checked erased.
    assert!(matches!(
        err("struct Pair[*Ts: Copyable & Movable](Copyable, Movable):\n    var storage: Tuple[*Ts]\n\ndef main():\n    pass\n"),
        TypeError::Unsupported(message) if message.contains("variadic struct 'Pair' is compiled by compile-time specialization")
    ));
}

#[test]
fn stores_into_outliving_storage_reject_frame_local_loans() {
    // The store-outward twin of the return escape: a reference-bearing value
    // rooted at a method-local cannot be stored into `self`, and a `ref`
    // field cannot be rebound to a frame-local place.
    let carrier = "@fieldwise_init\nstruct RefBox[origin: Origin[mut=True]]:\n    var value: ref[origin] List[Int]\n\n@fieldwise_init\nstruct Holder:\n    var slot: RefBox\n    def swap(mut self):\n        var local = [9]\n        ref alias = local\n        self.slot = RefBox(alias)\n\ndef main():\n    var keep = [1]\n    ref whole = keep\n    var holder = Holder(RefBox(whole))\n    holder.swap()\n";
    assert!(matches!(
        err(carrier),
        TypeError::StoredReferenceEscapesOrigin
    ));

    let rebind = "@fieldwise_init\nstruct RefBox[origin: Origin[mut=True]]:\n    var value: ref[origin] List[Int]\n    def refresh(mut self):\n        var local = [9]\n        self.value = local\n\ndef main():\n    var values = [1, 2]\n    ref whole = values\n    var box = RefBox(whole)\n    box.refresh()\n";
    assert!(matches!(
        err(rebind),
        TypeError::StoredReferenceEscapesOrigin
    ));
}

#[test]
fn stores_of_parameter_rooted_loans_into_self_stay_accepted() {
    // An origin that outlives the frame (a caller-owned parameter place) may
    // be stored outward; only frame-local roots escape.
    let src = "@fieldwise_init\nstruct RefBox[origin: Origin[mut=True]]:\n    var value: ref[origin] List[Int]\n\n@fieldwise_init\nstruct Holder:\n    var slot: RefBox\n    def rebind_to(mut self, mut source: List[Int]):\n        ref alias = source\n        self.slot = RefBox(alias)\n\ndef main():\n    var keep = [1]\n    ref whole = keep\n    var holder = Holder(RefBox(whole))\n    var other = [5]\n    holder.rebind_to(other)\n";
    assert!(check(&parse(src).expect("parse")).is_ok());
}

#[test]
fn immutable_origin_cast_pins_reference_results_read_only() {
    // `Origin[mut=False].cast_from[o]` in a reference result signature pins
    // the yielded capability to read-only regardless of the origin
    // parameter's own parametric `mut=` — the loop site never upgrades a
    // declared-immutable yield from the source's mutability.
    let template = "@fieldwise_init\nstruct StopIteration:\n    pass\n\n@fieldwise_init\nstruct NumbersIter[m: Bool, //, o: Origin[mut=m]]:\n    var src: ref[o] List[Int]\n    var index: Int\n    def __next__(mut self) raises StopIteration -> ref[Origin[mut=False].cast_from[o]] Int:\n        if self.index >= len(self.src):\n            raise StopIteration()\n        var r = self.index\n        self.index += 1\n        return self.src[r]\n\nstruct Numbers:\n    var items: List[Int]\n    def __init__(out self):\n        self.items = [4, 5, 6]\n    def __iter__(ref self) -> NumbersIter:\n        ref items = self.items\n        return NumbersIter(items, 0)\n\ndef main():\n    var nums = Numbers()\n";
    let write = format!("{template}    for ref x in nums:\n        x += 10\n");
    assert!(matches!(
        err(&write),
        TypeError::ImmutableBinding(_) | TypeError::TypeMismatch { .. }
    ));
    let read = format!("{template}    var total = 0\n    for ref x in nums:\n        total += x\n");
    assert!(check(&parse(&read).expect("parse")).is_ok());
}

#[test]
fn origin_cast_cannot_upgrade_capability() {
    let src = "@fieldwise_init\nstruct It[m: Bool, //, o: Origin[mut=m]]:\n    var src: ref[o] List[Int]\n    def first(self) -> ref[Origin[mut=True].cast_from[o]] Int:\n        return self.src[0]\n\ndef main():\n    print(1)\n";
    assert!(matches!(err(src), TypeError::Unsupported(_)));
}

#[test]
fn parameter_rooted_transfers_stay_accepted() {
    let src = "@fieldwise_init\nstruct RefBox[origin: Origin[mut=True]]:\n    var value: ref[origin] List[Int]\n\ndef fill(mut source: List[Int]) -> List[RefBox]:\n    var sink: List[RefBox] = List[RefBox]()\n    ref alias = source\n    sink.append(RefBox(alias))\n    return sink^\n\ndef main():\n    var keep = [4]\n    var got = fill(keep)\n";
    assert!(check(&parse(src).expect("parse")).is_ok());
}
