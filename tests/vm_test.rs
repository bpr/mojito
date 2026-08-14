//! Register-VM execution tests.
//!
//! Each test asserts the VM's exact `print` output for a program. `vm`/`parity`
//! return that output (`parity` is a historical helper name); `run` returns a
//! `Result` for error cases.
//!
//! Coverage tracks `backend/vm.rs`: scalars/operators, literal coercion,
//! short-circuit `and`/`or`, `if`/`while`, `for`/`range` (iterator protocol) and
//! `for` over lists, variables, user `def` calls (default/keyword/variadic ABI,
//! `mut`/`ref` reference-param write-back) + recursion, `return`, structs
//! (fieldwise construction, field read, `mut self`), `List`/`Tuple`, SIMD
//! construction + lane read, destructor (`__deinit__`) calls, `try`/`except`/`else`/
//! `finally` with exceptional-edge cleanup, and `print`/`String`/`len`. Remaining
//! gaps — a `return`/`break`/`continue` crossing a `try` boundary, and methods with
//! `mut`/`ref` ordinary params — are covered by `vm_reports_unsupported_features_cleanly`
//! and `vm_refuses_mut_ref_via_non_place_argument`: the VM must error cleanly, never
//! diverge.

use mojito::{BackendKind, Compiler, check, elaborate, link_source, parse};
use std::path::Path;

/// Run `src` through the VM backend (the sole executor) and return its captured
/// output, or a stage error string.
fn run(src: &str) -> Result<String, String> {
    let program =
        link_source(src, Path::new("vm_test.mojo")).map_err(|e| format!("link error: {e}"))?;
    let program = elaborate(program).map_err(|e| format!("comptime error: {e}"))?;
    let checked = mojito::check_program(&program).map_err(|e| format!("type error: {e:?}"))?;
    let mut backend = BackendKind::make("vm").expect("the register VM is implemented");
    backend
        .run(&checked)
        .map_err(|e| format!("runtime error: {e:?}"))?;
    Ok(backend.output())
}

/// Run source through the authoritative discovery/specialization pipeline.
/// Tests whose intrinsic results are public nominal Tuples need the generated
/// concrete Tuple declaration; the lower-level helper above intentionally does
/// not perform that whole-program handoff.
fn run_compiled(src: &str) -> Result<String, String> {
    let compiler = Compiler::default().with_snippet_module_scope();
    let program = compiler
        .compile_source(src, Path::new("vm_test.mojo"))
        .map_err(|error| format!("compile error: {error}"))?;
    compiler
        .execute(&program)
        .map(|execution| execution.output)
        .map_err(|error| format!("runtime error: {error}"))
}

/// Whether `src` is a statically valid program (parses + type-checks) — used to
/// show a program is well-formed but exercises a VM coverage gap.
fn checks_ok(src: &str) -> bool {
    parse(src).is_ok_and(|p| check(&p).is_ok())
}

/// The VM's output for a program that must succeed (exact-output assertions).
fn vm(src: &str) -> String {
    run(src).expect("vm backend failed")
}

/// Alias retained by the exact-output tests.
fn parity(src: &str) -> String {
    vm(src)
}

#[test]
fn arithmetic_and_precedence() {
    assert_eq!(
        parity("print(1 + 2 * 3)\nprint((1 + 2) * 3)\nprint(2 ** 10)\nprint(-7 // 2)\n"),
        "7\n9\n1024\n-4\n"
    );
}

#[test]
fn float_arithmetic_same_type() {
    // Both operands are float literals, so no contextual int→float coercion is needed.
    assert_eq!(vm("print(1.0 / 2.0)\nprint(3.5 + 1.5)\n"), "0.5\n5.0\n");
    parity("print(1.0 / 2.0)\nprint(3.5 + 1.5)\n");
}

#[test]
fn boolean_and_comparison() {
    parity("print(1 < 2 and not False)\nprint(3 == 3)\nprint(2 > 5 or 1 == 1)\n");
}

#[test]
fn collection_displays_and_comprehensions_execute_in_source_order() {
    let output = vm(include_str!(
        "../conformance/fixtures/collection_comprehensions.mojo"
    ));
    assert_eq!(
        output,
        "3 True False\n2 9 True\n0\n[0, 4, 16]\n[0, 1, 2, 10, 11, 12]\n{0, 1, 2}\n{0: 0, 1: 1, 2: 4, 3: 9}\n"
    );
}

#[test]
fn writable_formatting_retains_pointer_backed_arguments_through_the_call() {
    assert_eq!(
        vm("def main():\n    var values = {3}\n    print(values)\n"),
        "{3}\n"
    );
    assert_eq!(
        vm("def main():\n    var values = {3}\n    var text = String(values)\n    print(text)\n"),
        "{3}\n"
    );
}

#[test]
fn nominal_collection_protocols_match_the_differential_fixtures() {
    assert_eq!(
        run_compiled(include_str!(
            "../conformance/fixtures/protocolized_collections.mojo"
        ))
        .expect("compile nominal collection protocols"),
        "4 4 True\n2 9\n16\n"
    );
    assert_eq!(
        run_compiled(include_str!("../conformance/fixtures/tuple_values.mojo"))
            .expect("compile nominal Tuple values"),
        "3 seven 3\n2 True True\nTrue\n(seven, 3)\n(3, seven, True)\n(3, seven, True)\n"
    );
    assert_eq!(
        run_compiled(include_str!(
            "../conformance/fixtures/tuple_consume_elements.mojo"
        ))
        .expect("compile nominal Tuple consumption"),
        "3\n3\n"
    );
    assert_eq!(
        run_compiled(include_str!(
            "../conformance/fixtures/tuple_move_transforms.mojo"
        ))
        .expect("compile nominal Tuple move transforms"),
        "2 1\n3 4\n"
    );
}

#[test]
fn comprehension_binders_do_not_overwrite_outer_or_shadowed_bindings() {
    assert_eq!(
        vm(
            "def main():\n    var x = 100\n    var values = [x for x in range(3)]\n    var nested = [x for x in range(2) for x in range(x + 1)]\n    print(x, values)\n    print(nested)\n"
        ),
        "100 [0, 1, 2]\n[0, 0, 1]\n"
    );
}

#[test]
fn collection_displays_materialize_contextual_element_types() {
    assert_eq!(
        vm(
            "def empty() -> Set[Float64]:\n    return {}\n\ndef numbers() -> Set[Float64]:\n    return {1, 2}\n\ndef show(values: Set[Float64]):\n    print(values)\n\ndef main():\n    show({})\n    show({1, 2})\n    print(empty())\n    print(numbers())\n"
        ),
        "{}\n{1.0, 2.0}\n{}\n{1.0, 2.0}\n"
    );
}

#[test]
fn self_hosted_list_moves_and_destroys_raw_storage_exactly() {
    let source = "def main():\n    var values: List[Int] = [1, 2, 3]\n    values.insert(1, 9)\n    var removed = values.pop(2)\n    values.reverse()\n    print(removed, values)\n    values.clear()\n    print(len(values))\n";
    assert_eq!(vm(source), "2 [3, 9, 1]\n0\n");
}

#[test]
fn discarded_set_elements_and_replaced_dictionary_values_are_destroyed() {
    let output = vm(
        "struct Token(Equatable, Copyable, Movable):\n    var id: Int\n    def __init__(out self, id: Int):\n        self.id = id\n    def __deinit__(deinit self):\n        print(\"drop\", self.id)\n    def __hash__(self) -> UInt:\n        return UInt(self.id)\n    def __eq__(self, other: Self) -> Bool:\n        return self.id == other.id\n\ndef main():\n    var dictionary = {0: Token(1), 0: Token(2)}\n    print(\"built dict\", len(dictionary))\n    var values = {Token(3), Token(3)}\n    print(\"built set\", len(values))\n",
    );
    assert!(output.contains("built dict 1\n"), "{output}");
    assert!(output.contains("built set 1\n"), "{output}");
    assert_eq!(output.lines().filter(|line| *line == "drop 1").count(), 1);
    assert_eq!(output.lines().filter(|line| *line == "drop 2").count(), 1);
    assert_eq!(output.lines().filter(|line| *line == "drop 3").count(), 2);
}

#[test]
fn owned_iteration_moves_elements_and_drops_the_residual_on_break() {
    let output = vm(include_str!("../conformance/fixtures/owned_iteration.mojo"));
    assert_eq!(output, "take 1\ndrop 1\ntake 2\ndrop 2\ndrop 3\ndone\n");
}

#[test]
fn loop_binding_modes_are_independent_of_the_source_mode() {
    // A List[Int] source under each `{immutable, var, ref} x {borrowed, consumed}`
    // combination: `var` binds a mutable copy (the source is unchanged), `ref`
    // writes through to the borrowed element, and the plain target is immutable.
    let output = vm(include_str!(
        "../conformance/fixtures/loop_binding_modes.mojo"
    ));
    assert_eq!(
        output,
        "imm borrowed 1\nvar borrowed 12\nvar source 2\nref borrowed 13\nref source 13\n\
         imm consumed 4\nvar consumed 15\nref consumed 16\n"
    );
}

#[test]
fn value_iteration_binding_modes_over_a_user_iterator() {
    // A user iterator that yields owned values: `var`/`ref` targets can transfer
    // the yielded item onward, while the immutable target reads it.
    let output = vm(include_str!(
        "../conformance/fixtures/value_iteration_binding_modes.mojo"
    ));
    assert_eq!(
        output,
        "imm borrowed 1\ntake 2\nref borrowed 13\nimm consumed 4\ntake 5\nref consumed 16\n"
    );
}

#[test]
fn value_iteration_binds_droppable_items_per_iteration() {
    // Each yielded value lives in the loop variable's own per-iteration storage
    // and is destroyed at its last use, whether the target is immutable, `var`,
    // or `ref`.
    let output = vm(include_str!(
        "../conformance/fixtures/value_iteration_cleanup.mojo"
    ));
    assert_eq!(
        output,
        "drop 1\nimm 1\ndrop 2\nvar 2\ndrop 13\nref 13\ndone\n"
    );
}

#[test]
fn value_iteration_ref_target_transfers_the_yielded_item() {
    let output = vm(include_str!(
        "../conformance/fixtures/value_iteration_reference_transfer.mojo"
    ));
    assert_eq!(output, "take 1\ndone\n");
}

#[test]
fn reference_iteration_binding_modes_borrow_or_copy_the_referent() {
    // A reference-yielding iterator: the plain and `ref` targets borrow the
    // referent through the retained handle; a `var` target copies it into owned
    // storage, leaving the source untouched.
    let output = vm(include_str!(
        "../conformance/fixtures/reference_iteration_binding_modes.mojo"
    ));
    assert_eq!(output, "imm 1\nvar 102\nsrc 2\nref 3\n");
}

#[test]
fn string_concat_and_builtins() {
    assert_eq!(
        parity("var s: String = \"ab\" + \"cd\"\nprint(s)\nprint(len(s))\nprint(String(42))\n"),
        "abcd\n4\n42\n"
    );
}

#[test]
fn if_elif_else_via_function() {
    let src = "def sign(n: Int) -> Int:\n    if n > 0:\n        return 1\n    elif n < 0:\n        return -1\n    else:\n        return 0\n\ndef main():\n    print(sign(7))\n    print(sign(-4))\n    print(sign(0))\n";
    assert_eq!(parity(src), "1\n-1\n0\n");
}

#[test]
fn while_loop_accumulates() {
    let src = "def main():\n    var i: Int = 0\n    var total: Int = 0\n    while i < 5:\n        total = total + i\n        i = i + 1\n    print(total)\n";
    assert_eq!(parity(src), "10\n");
}

#[test]
fn function_calls_and_recursion() {
    let src = "def fib(n: Int) -> Int:\n    if n < 2:\n        return n\n    return fib(n - 1) + fib(n - 2)\n\ndef main():\n    print(fib(10))\n";
    assert_eq!(parity(src), "55\n");
}

#[test]
fn deep_free_function_recursion_uses_vm_frames() {
    assert_eq!(
        run("def countdown(n: Int) -> Int:\n    if n == 0:\n        return 0\n    return countdown(n - 1)\n\ndef main():\n    print(countdown(10000))\n")
            .unwrap(),
        "0\n"
    );
}

#[test]
fn nested_calls_evaluate_in_order() {
    let src = "def add(a: Int, b: Int) -> Int:\n    return a + b\n\ndef sq(n: Int) -> Int:\n    return n * n\n\ndef main():\n    print(sq(add(1, 2)))\n";
    assert_eq!(parity(src), "9\n");
}

#[test]
fn top_level_then_main_entry() {
    // Top-level statements run first, then the synthesized `main()` entry.
    let src = "print(1)\n\ndef main():\n    print(2)\n";
    assert_eq!(parity(src), "1\n2\n");
}

#[test]
fn boolean_short_circuit_skips_rhs() {
    // The MIR lowers `and`/`or` to CFG blocks, so a side-effecting right operand is
    // NOT evaluated when the left settles the result.
    // `loud()` prints; its absence from the output proves the skip.
    let src = "def loud() -> Bool:\n    print(\"called\")\n    return True\n\ndef main():\n    var a: Bool = False and loud()\n    print(a)\n    var b: Bool = True or loud()\n    print(b)\n    if False and loud():\n        print(\"nope\")\n    print(\"done\")\n";
    assert_eq!(parity(src), "False\nTrue\ndone\n");

    // When the left operand does NOT settle it, the right IS evaluated.
    let src2 = "def loud() -> Bool:\n    print(\"called\")\n    return True\n\ndef main():\n    var a: Bool = True and loud()\n    print(a)\n";
    assert_eq!(parity(src2), "called\nTrue\n");

    // Nested short-circuits compose.
    parity("print(True or (False and False))\nprint((1 < 2) and (2 < 3) and (3 < 4))\n");
}

#[test]
fn literal_coercion_at_binding_sites() {
    // An int literal materializes to the annotated/parameter type — the one place
    // the untyped MIR used to diverge. Now the MIR carries the annotation and the
    // VM applies the checked binding coercion.
    assert_eq!(parity("var f: Float64 = 3\nprint(f)\n"), "3.0\n");
    assert_eq!(parity("var u: UInt = 0\nu = u + 1\nprint(u)\n"), "1\n");
    // Int literal into a Float64 parameter, then float arithmetic.
    let src = "def scale(x: Float64) -> Float64:\n    return x * 2.0\n\ndef main():\n    print(scale(5))\n";
    assert_eq!(parity(src), "10.0\n");
    // Inferred `var` keeps the literal's natural kind (int stays Int).
    assert_eq!(parity("var n = 7\nprint(n)\nprint(n + 1)\n"), "7\n8\n");
}

#[test]
fn for_range_iterator_protocol() {
    // `for`/`range` lowers to the iterator protocol (HasNext/Next over a Range),
    // covering step direction, break/continue, and nesting.
    assert_eq!(
        parity("var t: Int = 0\nfor i in range(5):\n    t = t + i\nprint(t)\n"),
        "10\n"
    );
    assert_eq!(
        parity("for j in range(2, 8, 2):\n    print(j)\n"),
        "2\n4\n6\n"
    );
    assert_eq!(
        parity("for k in range(3, 0, -1):\n    print(k)\n"),
        "3\n2\n1\n"
    );
    // An empty range runs the body zero times.
    assert_eq!(
        parity("for x in range(0):\n    print(x)\nprint(99)\n"),
        "99\n"
    );
    // break/continue.
    let bc = "def main():\n    for m in range(10):\n        if m == 3:\n            break\n        if m == 1:\n            continue\n        print(m)\n";
    assert_eq!(parity(bc), "0\n2\n");
    // Nested loops.
    let nested = "for a in range(2):\n    for b in range(2):\n        print(a * 10 + b)\n";
    assert_eq!(parity(nested), "0\n1\n10\n11\n");
}

#[test]
fn return_crossing_try_runs_with_finally() {
    // A `return` inside a `try` crosses the boundary and runs the `finally` on the
    // way out through the VM's Flow-based region execution. A
    // `finally` that itself returns overrides (Python/Mojo semantics).
    let f = "def f() -> Int:\n    try:\n        return 1\n    finally:\n        print(\"fin\")\n\ndef main():\n    print(f())\n";
    assert_eq!(parity(f), "fin\n1\n");
    let override_ = "def g() -> Int:\n    try:\n        return 1\n    finally:\n        return 2\n\ndef main():\n    print(g())\n";
    assert_eq!(parity(override_), "2\n");
    let caught = "def h() -> Int:\n    try:\n        raise \"x\"\n    except e:\n        return 5\n    finally:\n        print(\"h-fin\")\n\ndef main():\n    print(h())\n";
    assert_eq!(parity(caught), "h-fin\n5\n");
}

#[test]
fn break_continue_crossing_try_runs_with_finally() {
    // `break`/`continue` inside a `try` that target an outer loop cross the boundary
    // and run each `finally` on the way out.
    let brk = "def main():\n    for i in range(5):\n        try:\n            if i == 3:\n                break\n            if i % 2 == 0:\n                continue\n            print(\"odd\", i)\n        finally:\n            print(\"fin\", i)\n    print(\"done\")\n";
    assert_eq!(parity(brk), "fin 0\nodd 1\nfin 1\nfin 2\nfin 3\ndone\n");
    // `break` in an `except`; a `finally` that itself `break`s overrides a body
    // `continue`; nested try/finally both run before the jump reaches the loop.
    let exc = "def main():\n    for i in range(4):\n        try:\n            raise \"x\"\n        except e:\n            break\n        finally:\n            print(\"fin\", i)\n    print(\"done\")\n";
    assert_eq!(parity(exc), "fin 0\ndone\n");
    let fin = "def main():\n    for i in range(3):\n        try:\n            continue\n        finally:\n            print(\"f\", i)\n            break\n    print(\"done\")\n";
    assert_eq!(parity(fin), "f 0\ndone\n");
    let nested = "def main():\n    for i in range(3):\n        try:\n            try:\n                break\n            finally:\n                print(\"in\", i)\n        finally:\n            print(\"out\", i)\n    print(\"done\")\n";
    assert_eq!(parity(nested), "in 0\nout 0\ndone\n");
}

#[test]
fn vm_reports_unsupported_features_cleanly() {
    // A remaining coverage gap must surface as a clean error, not a wrong answer or
    // a panic. `break`/`continue` crossing a `try` now works when the target loop is
    // function-level; the still-refused case is a loop declared *inside* a `try`,
    // broken by a nested `try` (a region-local target the mini-CFG can't name) — a
    // statically valid program the VM must reject cleanly, not diverge.
    let program = "def main():\n    try:\n        for i in range(3):\n            try:\n                break\n            finally:\n                print(\"fin\", i)\n    finally:\n        print(\"outer\")\n";
    assert!(checks_ok(program), "the program is statically valid");
    assert!(
        run(program).is_err(),
        "a break targeting a loop declared inside an enclosing try is not supported — must error"
    );
}

#[test]
fn structs_construction_fields_and_mut_self() {
    // Construction, field read, a read-only method, and a `mut self` method whose
    // mutation persists (written back through the receiver place).
    let src = "@fieldwise_init\nstruct Counter:\n    var n: Int\n\n    def get(self) -> Int:\n        return self.n\n\n    def bump(mut self, k: Int):\n        self.n += k\n\ndef main():\n    var c: Counter = Counter(10)\n    print(c.get())\n    c.bump(5)\n    c.bump(2)\n    print(c.n)\n";
    assert_eq!(parity(src), "10\n17\n");
}

#[test]
fn lists_tuples_and_indexing() {
    // List literal + index + mutation + membership; tuple return + const index.
    assert_eq!(
        parity(
            "var xs: List[Int] = [1, 2, 3]\nxs.append(4)\nprint(xs[0])\nprint(len(xs))\nprint(3 in xs)\n"
        ),
        "1\n4\nTrue\n"
    );
    let tup = "def pair() -> Tuple[Int, Int]:\n    return (7, 9)\n\ndef main():\n    var t = pair()\n    print(t[0])\n    print(t[1])\n";
    assert_eq!(
        run_compiled(tup).expect("compile nominal Tuple return"),
        "7\n9\n"
    );
}

#[test]
fn fixed_size_array_display_and_methods() {
    // Contextual display construction of `Array[Int, 3]`, by-reference
    // indexing, augmented element writes, copy/equality/containment, and
    // borrowed + owned iteration.
    let src = "def main():\n    var a: Array[Int, 3] = [1, 2, 3]\n    print(len(a), a[0], a[2])\n    a[1] += 5\n    print(a)\n    var b = a.copy()\n    print(a == b, 2 in b)\n    var total = 0\n    for x in a:\n        total += x\n    print(total)\n    var moved = 0\n    for var x in a^:\n        moved += x\n    print(moved)\n";
    assert_eq!(
        run_compiled(src).expect("compile the Array display program"),
        "3 1 3\n[1, 7, 3]\nTrue False\n11\n11\n"
    );
}

#[test]
fn plain_subscript_assignment_writes_through_a_reference_getter() {
    // No `__setitem__` anywhere: `a[i] = v` selects the mutable-reference
    // `__getitem__` and finishes with a reference write, on Array and on a
    // user struct alike.
    let src = "@fieldwise_init\nstruct Cell:\n    var v: Int\n\n@fieldwise_init\nstruct Grid:\n    var cell: Cell\n    def __getitem__(ref self, i: Int) -> ref[origin_of(self)] Cell:\n        return self.cell\n\ndef main():\n    var a = [1, 2, 3]\n    a[0] = 5\n    a[1] = a[2] + 10\n    print(a)\n    var g = Grid(Cell(1))\n    g[0] = Cell(9)\n    print(g[0].v)\n";
    assert_eq!(
        run_compiled(src).expect("compile the reference-getter assignment"),
        "[5, 13, 3]\n9\n"
    );
}

#[test]
fn argument_matching_default_keyword_variadic() {
    assert_eq!(
        parity(
            "def p(b: Int, e: Int = 2) -> Int:\n    return b ** e\n\ndef main():\n    print(p(3))\n    print(p(3, 3))\n    print(p(e=4, b=2))\n"
        ),
        "9\n27\n16\n"
    );
    let variadic = "def total(*xs: Int) -> Int:\n    var s: Int = 0\n    for x in xs:\n        s = s + x\n    return s\n\ndef main():\n    print(total())\n    print(total(1, 2, 3))\n";
    assert_eq!(parity(variadic), "0\n6\n");
}

#[test]
fn user_static_methods_use_the_shared_call_abi() {
    assert_eq!(
        parity(
            "struct S:\n    @staticmethod\n    def add(a: Int, b: Int = 2) -> Int:\n        return a + b\n\ndef main():\n    print(S.add(3), S.add(b=4, a=3))\n"
        ),
        "5 7\n"
    );
}

#[test]
fn argument_markers_positional_only_keyword_only_and_variadic_tail() {
    let src = "def first(a: Int, b: Int, /) -> Int:\n    return a\n\ndef scale(a: Int, *, by: Int) -> Int:\n    return a * by\n\ndef total(*xs: Int, scale: Int) -> Int:\n    var s: Int = 0\n    for x in xs:\n        s = s + x\n    return s * scale\n\ndef main():\n    print(first(8, 9))\n    print(scale(6, by=7))\n    print(total(1, 2, 3, scale=10))\n";
    assert_eq!(parity(src), "8\n42\n60\n");
}

#[test]
fn simd_construction_elementwise_and_lane() {
    let src = "var v: SIMD[DType.float64, 4] = SIMD[DType.float64, 4](1.0, 2.0, 3.0, 4.0)\nvar scaled = v * 2.0\nprint(scaled[3])\n";
    assert_eq!(parity(src), "8.0\n");
}

#[test]
fn vm_mut_ref_params_write_back() {
    // A `mut`/`ref` reference parameter mutates the caller's variable — the VM
    // writes each one's final value back to the caller's argument place after the
    // call.
    assert_eq!(
        parity(
            "def incr(mut x: Int):\n    x = x + 1\n\ndef main():\n    var n: Int = 5\n    incr(n)\n    incr(n)\n    print(n)\n"
        ),
        "7\n"
    );
    // An explicitly mutable `ref` writes back too; write-back through a struct
    // field place persists.
    assert_eq!(
        parity(
            "def set_to[origin: Origin[mut=True]](ref[origin] x: Int, v: Int):\n    x = v\n\ndef main():\n    var n: Int = 0\n    set_to(n, 42)\n    print(n)\n"
        ),
        "42\n"
    );
    let field = "@fieldwise_init\nstruct Counter:\n    var n: Int\n\ndef bump(mut c: Counter, k: Int):\n    c.n = c.n + k\n\ndef main():\n    var c: Counter = Counter(0)\n    bump(c, 5)\n    bump(c, 3)\n    print(c.n)\n";
    assert_eq!(parity(field), "8\n");
}

#[test]
fn method_mut_ref_param_writeback_parity() {
    // A method with a `mut` *ordinary* parameter writes the mutated argument back
    // to the caller's place through the VM call ABI.
    let src = "@fieldwise_init\nstruct C:\n    var n: Int\n    def combine(self, mut other: C):\n        other.n = other.n + self.n\n\ndef main():\n    var a: C = C(1)\n    var b: C = C(2)\n    a.combine(b)\n    print(b.n)\n";
    assert_eq!(parity(src), "3\n");
}

#[test]
fn method_argument_binding_matches_free_functions() {
    let src = "@fieldwise_init\nstruct Acc:\n    var total: Int\n    def add(mut self, x: Int, /, y: Int = 2, *rest: Int, scale: Int = 1) -> Int:\n        var amount: Int = x + y\n        for value in rest:\n            amount = amount + value\n        self.total = self.total + amount * scale\n        return self.total\n    def bump_arg(self, mut value: Int, delta: Int = 1):\n        value = value + delta\n\ndef main():\n    var acc: Acc = Acc(0)\n    print(acc.add(3, y=4, scale=2))\n    print(acc.add(1, 5, 6, 7, scale=3))\n    var n: Int = 10\n    acc.bump_arg(n, delta=4)\n    print(n)\n";
    assert_eq!(parity(src), "14\n71\n14\n");
}

#[test]
fn generic_argument_binding_matches_free_functions() {
    let src = "def collect[T: AnyType](head: T, /, extra: Int = 2, *rest: Int, scale: Int = 1) -> Int:\n    return (extra + len(rest)) * scale\n\ndef replace[T: Copyable & Movable](mut value: T, replacement: T):\n    value = replacement\n\ndef main():\n    print(collect(\"x\", extra=3, scale=4))\n    print(collect(1, 2, 8, 9, scale=3))\n    var n: Int = 5\n    replace(n, replacement=9)\n    print(n)\n";
    assert_eq!(parity(src), "12\n12\n9\n");
}

#[test]
fn try_except_else_finally() {
    // Full structured exceptions cover a
    // caught raise, the `else` on normal completion, and `finally` on every path.
    let caught = "def main():\n    try:\n        print(\"body\")\n        raise \"x\"\n        print(\"unreached\")\n    except e:\n        print(\"caught\")\n    finally:\n        print(\"fin\")\n    print(\"after\")\n";
    assert_eq!(parity(caught), "body\ncaught\nfin\nafter\n");

    let no_raise = "def main():\n    try:\n        print(\"body\")\n    except e:\n        print(\"caught\")\n    else:\n        print(\"elseran\")\n    finally:\n        print(\"fin\")\n";
    assert_eq!(parity(no_raise), "body\nelseran\nfin\n");
}

#[test]
fn partial_move_field_read_parity() {
    // A partial move `p.a^` followed by reads of the moved value and the retained
    // sibling runs identically on both backends: the field read now lowers to a
    // `LoadPlace`, and `^` on a field to a `MovePlace`, preserving the moved value.
    let src = "@fieldwise_init\nstruct Inner:\n    var id: Int\n\n@fieldwise_init\nstruct Pair:\n    var a: Inner\n    var b: Inner\n\ndef main():\n    var p: Pair = Pair(Inner(1), Inner(2))\n    var x: Inner = p.a^\n    print(x.id)\n    print(p.b.id)\n    p.a = Inner(9)\n    print(p.a.id)\n";
    assert_eq!(parity(src), "1\n2\n9\n");
}

#[test]
fn utility_builtins_parity() {
    // abs/min/max/round + Int/UInt/Float64 conversions use shared runtime helpers.
    let src = "def main():\n    print(abs(-5))\n    print(abs(-3.5))\n    print(min(3, 7), max(3, 7))\n    print(round(2.5), round(2.4))\n    print(Int(3.9), UInt(42), Float64(7))\n";
    assert_eq!(parity(src), "5\n3.5\n3 7\n3.0 2.0\n3 42 7.0\n");
}

#[test]
fn simd_lane_write_parity() {
    // `v[i] = e` / `v[i] += e`, both bare and through a struct field, now update a
    // SIMD lane through `store_place`/`set_simd_lane`.
    let src = "@fieldwise_init\nstruct Vec4:\n    var data: SIMD[DType.int32, 4]\n\ndef main():\n    var v: SIMD[DType.int32, 4] = SIMD[DType.int32, 4](1, 2, 3, 4)\n    v[0] = 10\n    v[2] += 5\n    print(v[0], v[1], v[2], v[3])\n    var w: Vec4 = Vec4(SIMD[DType.int32, 4](0, 0, 0, 0))\n    w.data[1] = 42\n    print(w.data[1])\n";
    assert_eq!(parity(src), "10 2 8 4\n42\n");
}

#[test]
fn value_parameterized_generics_parity() {
    // A value-parameterized struct reifies its value parameter (read via
    // `Self.size`), and a value-parameterized function binds it as a local — both
    // execute through the same VM frame representation.
    let src = "@fieldwise_init\nstruct FixedBuffer[size: Int]:\n    var tag: Int\n    def capacity(self) -> Int:\n        return Self.size\n\ndef scaled[factor: Int](x: Int) -> Int:\n    return x * factor\n\ndef main():\n    var b: FixedBuffer[8] = FixedBuffer[8](3)\n    print(b.capacity(), b.tag)\n    print(scaled[10](4))\n";
    assert_eq!(parity(src), "8 3\n40\n");
}

#[test]
fn nested_def_closures_parity() {
    // Nested `def`s cover a
    // read-capture, a write-capture (reference semantics), and self-recursion.
    let read = "def adder(n: Int) -> Int:\n    def add_n(x: Int) {n} -> Int:\n        return x + n\n    return add_n(100)\n\ndef main():\n    print(adder(42))\n";
    assert_eq!(parity(read), "142\n");
    let write = "def counter() -> Int:\n    var total: Int = 0\n    def add(x: Int) {mut total}:\n        total = total + x\n    add(5)\n    add(3)\n    return total\n\ndef main():\n    print(counter())\n";
    assert_eq!(parity(write), "8\n");
    let rec = "def factorial(base: Int) -> Int:\n    def fact(n: Int) {base} -> Int:\n        if n <= 1:\n            return base\n        return n * fact(n - 1)\n    return fact(5)\n\ndef main():\n    print(factorial(1))\n";
    assert_eq!(parity(rec), "120\n");
}

#[test]
fn owned_closure_captures_materialize_at_the_declaration() {
    let copy = "def main():\n    var x = 40\n    def snapshot() {var x} -> Int:\n        return x\n    x = 42\n    print(snapshot(), x)\n";
    assert_eq!(parity(copy), "40 42\n");

    // A move capture transfers the source at the declaration, then owns one
    // persistent environment slot. Calls borrow that slot instead of cloning it.
    let moved = "def main():\n    var box = [40]\n    def get() {var box^} -> Int:\n        box[0] += 1\n        return box[0]\n    print(get())\n    print(get())\n";
    assert_eq!(parity(moved), "41\n42\n");

    // Reference environments deliberately remain live views rather than
    // snapshots: mutation between declaration and invocation is observable.
    let reference = "def main():\n    var x = 40\n    def read_x() {imm x} -> Int:\n        return x\n    x = 42\n    print(read_x())\n";
    assert_eq!(parity(reference), "42\n");
}

#[test]
fn first_class_owned_closure_uses_its_stored_snapshot() {
    // A capture-bearing nested def binds only to a `capturing[...]` contract;
    // the unqualified `def(...)` binding is rejected (see `checker_test`).
    let src = "def invoke(callback: def() capturing[_] -> Int) -> Int:\n    return callback()\n\ndef main():\n    var x = 40\n    def snapshot() {var x} -> Int:\n        return x\n    x = 42\n    print(snapshot(), invoke(snapshot), x)\n";
    assert_eq!(parity(src), "40 40 42\n");

    // A non-escaping reference closure keeps its outer frame slot alive
    // through the indirect consumer call; the handle must not observe an
    // ASAP-dropped `None` slot.
    let reference = "def invoke(callback: def() capturing[_] -> Int) -> Int:\n    return callback()\n\ndef main():\n    var x = 42\n    def get() {ref x} -> Int:\n        return x\n    print(invoke(get))\n";
    assert_eq!(parity(reference), "42\n");
}

#[test]
fn nested_def_calling_sibling_forwards_its_closure_environment() {
    let src = "def outer() -> Int:\n    var b: Int = 10\n    def helper(x: Int) {b} -> Int:\n        return x + b\n    def caller(y: Int) {helper} -> Int:\n        return helper(y) + 1\n    return caller(5)\n\ndef main():\n    print(outer())\n";
    assert_eq!(parity(src), "16\n");
}

#[test]
fn sibling_capture_loads_the_materialized_closure_snapshot() {
    let src = "def main():\n    var x = 1\n    def helper() {var x} -> Int:\n        return x\n    x = 2\n    def caller() {helper} -> Int:\n        return helper()\n    print(caller())\n";
    assert_eq!(vm(src), "1\n");
}

#[test]
fn same_named_block_defs_and_shadow_captures_use_binding_identity() {
    let src = "def main():\n    var x = 1\n    if True:\n        def choose() -> Int:\n            return 1\n        print(choose())\n    if True:\n        def choose() -> Int:\n            return 42\n        print(choose())\n    if True:\n        var x = 40\n        def read() {x} -> Int:\n            return x\n        print(read())\n    print(x)\n";
    assert_eq!(vm(src), "1\n42\n40\n1\n");
}

#[test]
fn shadowed_runtime_loop_capture_keeps_the_loop_owner() {
    let src = "def main():\n    var item = 1\n    for item in [40]:\n        def read() {item} -> Int:\n            return item\n        print(read())\n    print(item)\n";
    assert_eq!(vm(src), "40\n1\n");
}

#[test]
fn unpack_and_exception_targets_seed_typed_capture_slots() {
    let src = "def main():\n    if True:\n        var left, label = (40, \"two\")\n        def show() {left, label}:\n            print(left, label)\n        show()\n    if True:\n        var left, label = (42, True)\n        def show() {left, label}:\n            print(left, label)\n        show()\n    var error = 40\n    try:\n        raise \"caught\"\n    except error:\n        def show_error() {error}:\n            print(error)\n        show_error()\n    print(error)\n";
    assert_eq!(
        run_compiled(src).expect("compile nominal Tuple unpacking"),
        "40 two\n42 True\nError(\"caught\")\n40\n"
    );
}

#[test]
fn cloned_trait_defaults_rekey_nested_self_captures() {
    let src = "trait Valued:\n    def answer(self) -> Int:\n        def inner() {self} -> Int:\n            return self.value\n        return inner()\n\n@fieldwise_init\nstruct First(Valued):\n    var value: Int\n\n@fieldwise_init\nstruct Second(Valued):\n    var value: Int\n\ndef main():\n    print(First(41).answer())\n    print(Second(42).answer())\n";
    assert_eq!(vm(src), "41\n42\n");
}

#[test]
fn nested_parameter_shadow_capture_uses_the_nearest_owner() {
    let src = "def outer(value: Int, delta: Int) -> Int:\n    def middle(value: Int) {delta} -> Int:\n        def inner() {value, delta} -> Int:\n            return value + delta\n        return inner()\n    return middle(value)\n\ndef main():\n    print(outer(40, 2))\n";
    assert_eq!(vm(src), "42\n");
}

#[test]
fn nested_defs_lift_and_forward_captures_at_arbitrary_depth() {
    let src = "def outer() -> Int:\n    var value = 40\n    def middle() {value} -> Int:\n        def inner() {value} -> Int:\n            return value + 2\n        return inner()\n    return middle()\n\ndef main():\n    print(outer())\n";
    assert_eq!(parity(src), "42\n");
}

#[test]
fn deep_mutable_capture_updates_its_lexical_owner() {
    let src = "def outer() -> Int:\n    var total = 40\n    def middle() {mut total}:\n        def inner() {mut total}:\n            total += 2\n        inner()\n    middle()\n    return total\n\ndef main():\n    print(outer())\n";
    assert_eq!(parity(src), "42\n");
}

#[test]
fn intermediate_default_capture_policy_forwards_descendant_environment() {
    let src = "def outer() -> Int:\n    var value = 40\n    def middle() {imm} -> Int:\n        def inner() {value} -> Int:\n            return value + 2\n        return inner()\n    return middle()\n\ndef main():\n    print(outer())\n";
    assert_eq!(parity(src), "42\n");
}

#[test]
fn deep_nested_callable_preserves_effects_and_argument_markers() {
    let src = "def outer() raises -> Int:\n    var base = 40\n    def middle() raises {base} -> Int:\n        def inner(head: Int, /, tail: Int = 0, *, bump: Int = 0) raises {base} -> Int:\n            if bump < 0:\n                raise Error(\"negative\")\n            return base + head + tail + bump\n        return inner(1, tail=0, bump=1)\n    return middle()\n\ndef main():\n    try:\n        print(outer())\n    except error:\n        print(error)\n";
    assert_eq!(parity(src), "42\n");
}

#[test]
fn deep_nested_callable_preserves_reference_and_named_result_abis() {
    let references = "def main():\n    var value = 40\n    def middle(mut middle_item: Int):\n        def inner(ref inner_item: Int) -> ref[inner_item] Int:\n            return inner_item\n        ref alias = inner(middle_item)\n        alias += 2\n    middle(value)\n    print(value)\n";
    assert_eq!(parity(references), "42\n");

    let named_result = "def outer() -> Int:\n    var base = 40\n    def middle() {base} -> Int:\n        def inner(value: Int, out result: Int) {base}:\n            result = base + value\n        return inner(2)\n    return middle()\n\ndef main():\n    print(outer())\n";
    assert_eq!(parity(named_result), "42\n");
}

#[test]
fn deep_lexical_callable_registry_prefers_the_nearest_shadow() {
    let src = "def outer() -> Int:\n    var base = 40\n    def helper() {base} -> Int:\n        return base + 1\n    def middle() {base} -> Int:\n        def helper() {base} -> Int:\n            return base + 2\n        def inner() {helper} -> Int:\n            return helper()\n        return inner()\n    return middle()\n\ndef main():\n    print(outer())\n";
    assert_eq!(parity(src), "42\n");
}

#[test]
fn deep_lexical_callable_registry_forwards_an_ancestor_sibling() {
    let src = "def outer() -> Int:\n    var base = 40\n    def helper() {base} -> Int:\n        return base + 2\n    def middle() {helper} -> Int:\n        def inner() {helper} -> Int:\n            return helper()\n        return inner()\n    return middle()\n\ndef main():\n    print(outer())\n";
    assert_eq!(parity(src), "42\n");
}

#[test]
fn method_rooted_closure_tree_forwards_self_at_arbitrary_depth() {
    let src = "@fieldwise_init\nstruct Box:\n    var base: Int\n\n    def answer(self) -> Int:\n        def middle() {self} -> Int:\n            def inner() {self} -> Int:\n                return self.base + 2\n            return inner()\n        return middle()\n\ndef main():\n    print(Box(40).answer())\n";
    assert_eq!(parity(src), "42\n");
}

#[test]
fn empty_intermediate_capture_policy_does_not_forward_descendant_environment() {
    let src = "def outer() -> Int:\n    var value = 42\n    def middle() {} -> Int:\n        def inner() {value} -> Int:\n            return value\n        return inner()\n    return middle()\n\ndef main():\n    print(outer())\n";
    assert!(!checks_ok(src));
}

#[test]
fn nested_reference_returns_preserve_the_caller_handle() {
    let src = "def main():\n    var value = 40\n    def borrow(ref item: Int) -> ref[item] Int:\n        return item\n    ref first = borrow(value)\n    first += 1\n    try:\n        ref second = borrow(value)\n        second += 1\n    except error:\n        print(error)\n    print(value)\n";
    assert_eq!(parity(src), "42\n");
}

#[test]
fn ref_returning_subscript_preserves_receiver_storage_through_the_read() {
    let src = "@fieldwise_init\nstruct Box:\n    var value: Int\n\n    def __getitem__(\n        ref self, index: Int\n    ) -> ref[origin_of(self.value)] Int:\n        return self.value\n\n    def __deinit__(deinit self):\n        print(\"drop\")\n\ndef main():\n    var box = Box(40)\n    print(box[0])\n";
    assert_eq!(vm(src), "40\ndrop\n");
}

#[test]
fn ref_returning_list_subscript_copies_in_value_context_and_aliases_in_ref_context() {
    let src = "from std.collections.list import List\n\ndef main():\n    var rows = List[List[Int]]()\n    var row = List[Int]()\n    row.append(1)\n    rows.append(row)\n\n    var copied: List[Int] = rows[0]\n    copied.append(2)\n\n    ref alias = rows[0]\n    alias.append(3)\n\n    var original: List[Int] = rows[0]\n    print(len(original), original[1])\n    print(len(copied), copied[1])\n";
    assert_eq!(run_compiled(src).unwrap(), "2 3\n2 2\n");
}

#[test]
fn structured_reference_calls_cross_try_boundaries() {
    let src = include_str!("../conformance/fixtures/structured_reference_calls.mojo");
    assert_eq!(
        vm(src),
        "42\n42\n42\n10 22 30\nfree caught\n21\nmethod caught\n21\n"
    );
}

#[test]
fn structured_reference_call_rebases_reference_bearing_aggregate_returns() {
    let src = "@fieldwise_init\nstruct RefBox[origin: Origin[mut=True]]:\n    var value: ref[origin] Int\n\ndef wrap[origin: Origin[mut=True]](\n    ref[origin] item: Int\n) -> RefBox:\n    return RefBox(item)\n\ndef main():\n    var value = 40\n    try:\n        var box = wrap(value)\n        box.value += 2\n    except error:\n        print(error)\n    print(value)\n";
    assert_eq!(vm(src), "42\n");
}

#[test]
fn structured_reference_call_handwritten_constructors_use_caller_places() {
    let src = "struct Snapshot:\n    var value: Int\n\n    def __init__(out self, ref source: Int):\n        self.value = source\n\nstruct Bump:\n    var seen: Int\n\n    def __init__(out self, mut source: Int, fail: Bool) raises:\n        source += 1\n        self.seen = source\n        if fail:\n            raise Error(\"failed\")\n\ndef main():\n    var source = 40\n    try:\n        var snapshot = Snapshot(source=source)\n        print(snapshot.value)\n    except error:\n        print(error)\n\n    var changed = 40\n    try:\n        var bump = Bump(source=changed, fail=False)\n        print(bump.seen)\n    except error:\n        print(error)\n    print(changed)\n\n    try:\n        var unused = Bump(source=changed, fail=True)\n    except error:\n        print(\"caught\")\n    print(changed)\n";
    assert_eq!(vm(src), "40\n41\n41\ncaught\n42\n");
}

#[test]
fn generic_nested_def_is_type_erased_after_checker_inference() {
    let src = "def outer() -> Int:\n    def identity[T: Copyable & Movable](value: T) {} -> T:\n        return value\n    return identity(42)\n\ndef main():\n    print(outer())\n";
    assert_eq!(parity(src), "42\n");
}

#[test]
fn callable_type_bounds_execute_top_level_captured_and_nominal_values() {
    let src = "def apply[T: Copyable & Deinitable, F: def(T) -> T](callback: F, value: T) -> T:\n    return callback(value)\n\ndef increment(value: Int) -> Int:\n    return value + 1\n\n@fieldwise_init\nstruct Add(def(Int) -> Int):\n    var delta: Int\n    def __call__(self, value: Int) -> Int:\n        return value + self.delta\n\ndef main():\n    print(apply(increment, 41))\n    var offset = 2\n    def captured(value: Int) {imm offset} -> Int:\n        return value + offset\n    print(apply(captured, 40))\n    print(apply(Add(3), 39))\n";
    assert_eq!(vm(src), "42\n42\n42\n");
}

#[test]
fn nominal_callable_struct_requires_and_executes_call_contract() {
    let src = "@fieldwise_init\nstruct Scale(def(Int) -> Int):\n    var factor: Int\n    def __call__(self, value: Int) -> Int:\n        return value * self.factor\n\ndef apply(callback: def(Int) -> Int, value: Int) -> Int:\n    return callback(value)\n\ndef main():\n    var scale = Scale(3)\n    print(apply(scale, 14))\n";
    assert_eq!(parity(src), "42\n");
}

#[test]
fn nominal_callable_same_arity_overload_uses_the_checked_target_in_both_vm_paths() {
    let src = "@fieldwise_init\nstruct Choose(def(Int) -> Int):\n    def __call__(self, value: Bool) -> Int:\n        return 0\n\n    def __call__(self, value: Int) -> Int:\n        return value + 1\n\ndef invoke(callback: def(Int) -> Int, value: Int) -> Int:\n    return callback(value)\n\ndef main():\n    print(invoke(Choose(), 41))\n    try:\n        print(Choose()(41))\n    except error:\n        print(error)\n";
    assert_eq!(vm(src), "42\n42\n");
}

#[test]
fn nominal_callable_contract_preserves_mut_parameter_convention() {
    let src = "@fieldwise_init\nstruct Mutator(def(mut Int) -> None):\n    def __call__(self, mut value: Int, /) capturing:\n        value += 1\n\ndef apply(callback: def(mut Int) -> None, mut value: Int):\n    callback(value)\n\ndef main():\n    var value = 41\n    apply(Mutator(), value)\n    print(value)\n";
    assert_eq!(vm(src), "42\n");
}

#[test]
fn nominal_callable_contract_preserves_reference_return_origin() {
    let src = "@fieldwise_init\nstruct Borrower(def[origin: Origin[mut=True]](ref[origin] Int) -> ref[origin] Int):\n    def __call__[origin: Origin[mut=True]](self, ref[origin] value: Int, /) capturing -> ref[origin] Int:\n        return value\n\ndef main():\n    var value = 40\n    ref borrowed = Borrower()(value)\n    borrowed += 2\n    print(value)\n";
    assert_eq!(vm(src), "42\n");
}

#[test]
fn origin_specialized_function_value_executes_indirectly() {
    let src = "def borrow[origin: Origin[mut=True]](ref[origin] value: Int) -> ref[origin] Int:\n    return value\n\ndef main():\n    var value = 40\n    var function = borrow[origin_of(value)]\n    ref result = function(value)\n    result += 2\n    print(value)\n";
    assert_eq!(vm(src), "42\n");
}

#[test]
fn multiple_positional_origins_specialize_one_function_value() {
    let src = "def choose[first: Origin[mut=True], second: Origin[mut=True]](ref[first] left: Int, ref[second] right: Int, use_right: Bool) -> ref[first, second] Int:\n    if use_right:\n        return right\n    return left\n\ndef main():\n    var left = 1\n    var right = 2\n    var function = choose[origin_of(left), origin_of(right)]\n    ref selected = function(left, right, True)\n    selected += 40\n    print(left, right)\n";
    assert_eq!(vm(src), "1 42\n");
}

#[test]
fn explicit_origins_select_overloads_and_compose_with_generic_parameters() {
    let direct_overload = "def choose[origin: Origin[mut=True]](ref[origin] value: Int) -> ref[origin] Int:\n    return value\n\ndef choose[origin: Origin[mut=True]](ref[origin] value: Float64) -> ref[origin] Float64:\n    return value\n\ndef main():\n    var value = 40\n    ref selected = choose[origin_of(value)](value)\n    selected += 2\n    print(value)\n";
    assert_eq!(vm(direct_overload), "42\n");

    let generic_value = "def borrow[T: Copyable & Deinitable, origin: Origin[mut=True]](ref[origin] value: T) -> ref[origin] T:\n    return value\n\ndef main():\n    var value = 40\n    var function = borrow[Int, origin_of(value)]\n    ref selected = function(value)\n    selected += 2\n    print(value)\n";
    assert_eq!(vm(generic_value), "42\n");

    let named_inferred = "def borrow[T: Copyable & Deinitable, origin: Origin[mut=True]](ref[origin] value: T) -> ref[origin] T:\n    return value\n\ndef main():\n    var value = 40\n    ref selected = borrow[origin=origin_of(value)](value)\n    selected += 2\n    print(value)\n";
    assert_eq!(vm(named_inferred), "42\n");
}

#[test]
fn contextual_origin_contract_selects_an_overloaded_function_value() {
    let src = "def choose[origin: Origin[mut=True]](ref[origin] value: Int) -> ref[origin] Int:\n    return value\n\ndef choose[origin: Origin[mut=True]](ref[origin] value: Float64) -> ref[origin] Float64:\n    return value\n\ndef main():\n    var value = 40\n    var function: def(ref[origin_of(value)] Int) thin -> ref[origin_of(value)] Int = choose[origin_of(value)]\n    ref selected = function(value)\n    selected += 2\n    print(value)\n";
    assert_eq!(vm(src), "42\n");
}

#[test]
fn nested_origin_specialized_function_values_load_their_closure() {
    let stateless =
        include_str!("../conformance/fixtures/nested_origin_specialized_function_value.mojo");
    assert_eq!(vm(stateless), "42\n");
}

#[test]
fn structured_nominal_callable_mut_self_uses_its_callee_place() {
    let src = "@fieldwise_init\nstruct Counter(def() raises -> Int):\n    var value: Int\n\n    def __call__(mut self) raises -> Int:\n        self.value += 1\n        if self.value == 2:\n            raise Error(\"two\")\n        return self.value\n\ndef main():\n    var counter = Counter(0)\n    try:\n        print(counter())\n        print(counter())\n    except:\n        print(\"caught\")\n    print(counter.value)\n";
    assert_eq!(vm(src), "1\ncaught\n2\n");
}

#[test]
fn overloaded_callable_values_execute_contextual_targets() {
    let src = include_str!("../conformance/fixtures/overloaded_callable_values.mojo");
    assert_eq!(parity(src), "42\ncaught\n");
}

#[test]
fn overload_symbols_distinguish_stropped_type_names() {
    let src = "@fieldwise_init\nstruct `A-B`:\n    var x: Int\n\n@fieldwise_init\nstruct `A_B`:\n    var x: Int\n\ndef choose(x: `A-B`) -> Int:\n    return 1\n\ndef choose(x: `A_B`) -> Int:\n    return 2\n\ndef main():\n    print(choose(`A-B`(0)))\n    print(choose(`A_B`(0)))\n";
    assert_eq!(vm(src), "1\n2\n");
}

#[test]
fn overload_symbols_fold_comptime_value_arguments() {
    let src = "@fieldwise_init\nstruct FixedBuffer[size: Int]:\n    var value: Int\n\ncomptime N = 2 + 6\n\ndef choose(x: FixedBuffer[N]) -> Int:\n    return 1\n\ndef choose(x: Int) -> Int:\n    return 2\n\ndef main():\n    print(choose(FixedBuffer[8](7)))\n";
    assert!(vm(src).lines().any(|line| line == "1"));
}

#[test]
fn dunder_operator_and_builtin_dispatch() {
    // Operators + `len`/`String`/subscript/`in` on a user struct dispatch to its
    // dunder methods (operator overloading), running on the VM.
    let src = "@fieldwise_init\nstruct Vec2(Writable):\n    var x: Int\n    var y: Int\n    def __add__(self, o: Vec2) -> Vec2:\n        return Vec2(self.x + o.x, self.y + o.y)\n    def __eq__(self, o: Vec2) -> Bool:\n        return self.x == o.x and self.y == o.y\n    def write_to(self, mut writer: Some[Writer]):\n        writer.write(\"V(\", self.x, \",\", self.y, \")\")\n    def __len__(self) -> Int:\n        return 2\n    def __getitem__(self, i: Int) -> Int:\n        if i == 0:\n            return self.x\n        return self.y\n    def __contains__(self, v: Int) -> Bool:\n        return self.x == v or self.y == v\n\ndef main():\n    var a: Vec2 = Vec2(1, 2)\n    print(String(a + Vec2(3, 4)))\n    print(a == Vec2(1, 2))\n    print(len(a), a[0], a[1])\n    print(2 in a, 9 not in a)\n";
    assert_eq!(parity(src), "V(4,6)\nTrue\n2 1 2\nTrue True\n");
}

#[test]
fn dunder_augmented_assignment_uses_iadd() {
    // `c += d` dispatches to the dedicated in-place dunder `__iadd__(mut self, …)`,
    // which mutates the receiver in place; Mojo does not fall back to `__add__`.
    let src = "@fieldwise_init\nstruct Acc(Writable):\n    var n: Int\n    def __iadd__(mut self, o: Acc):\n        self.n += o.n\n    def write_to(self, mut writer: Some[Writer]):\n        writer.write(self.n)\n\ndef main():\n    var c: Acc = Acc(1)\n    c += Acc(10)\n    c += Acc(100)\n    print(String(c))\n";
    assert_eq!(parity(src), "111\n");
}

#[test]
fn dunder_setitem_writes_back() {
    // `c[i] = e` dispatches to `__setitem__(mut self, …)` and the mutation persists;
    // `c[i] += e` reads via `__getitem__` and writes via `__setitem__`; a nested
    // place (`h.p[i] = e`) writes back through the outer struct.
    let src = "@fieldwise_init\nstruct Pair:\n    var a: Int\n    var b: Int\n    def __getitem__(self, i: Int) -> Int:\n        if i == 0:\n            return self.a\n        return self.b\n    def __setitem__(mut self, i: Int, v: Int):\n        if i == 0:\n            self.a = v\n        else:\n            self.b = v\n\n@fieldwise_init\nstruct Holder:\n    var p: Pair\n\ndef main():\n    var p: Pair = Pair(1, 2)\n    p[0] = 10\n    p[1] = 20\n    p[0] += 5\n    print(p[0], p[1])\n    var h: Holder = Holder(Pair(5, 6))\n    h.p[1] = 99\n    print(h.p[0], h.p[1])\n";
    assert_eq!(parity(src), "15 20\n5 99\n");
}

#[test]
fn hand_written_init_constructs_and_coerces() {
    // A `def __init__(out self, …)` builds the struct: fields are set in the body,
    // and arguments are coerced to the parameter types (Int literal → Float64).
    let src = "struct Point:\n    var x: Int\n    var y: Int\n    def __init__(out self, x: Int, y: Int):\n        self.x = x\n        self.y = y\n    def sum(self) -> Int:\n        return self.x + self.y\n\nstruct Scaled:\n    var v: Float64\n    def __init__(out self, n: Float64):\n        self.v = n * 2.0\n\ndef main():\n    var p: Point = Point(3, 4)\n    print(p.x, p.y, p.sum())\n    var s: Scaled = Scaled(5)\n    print(s.v)\n";
    assert_eq!(parity(src), "3 4 7\n10.0\n");
}

#[test]
fn user_iterator_protocol() {
    // `for x in c` on a user type dispatches `c.__iter__()` → loop while
    // `len(iter) > 0` binding `x = iter.__next__()`; break/continue compose.
    let src = "@fieldwise_init\nstruct It:\n    var cur: Int\n    var stop: Int\n    def __len__(self) -> Int:\n        return self.stop - self.cur\n    def __next__(mut self) -> Int:\n        var v: Int = self.cur\n        self.cur = self.cur + 1\n        return v\n\n@fieldwise_init\nstruct Nums:\n    var n: Int\n    def __iter__(self) -> It:\n        return It(0, self.n)\n\ndef main():\n    for x in Nums(6):\n        if x == 4:\n            break\n        if x == 1:\n            continue\n        print(x)\n";
    assert_eq!(parity(src), "0\n2\n3\n");
}

#[test]
fn borrowed_list_iteration_observes_element_replacement_without_copying() {
    let source = include_str!("../conformance/fixtures/borrowed_iteration_mutation.mojo");
    assert_eq!(
        run_compiled(source).expect("borrowed List iteration compiles"),
        "1\n9\n3\n"
    );

    let lifetime = "@fieldwise_init\nstruct Holder(Deinitable):\n    var values: List[Int]\n    def __deinit__(deinit self):\n        print(\"drop holder\")\n\ndef main():\n    var holder = Holder([1, 2, 3])\n    for value in holder.values:\n        print(value)\n";
    assert_eq!(
        run_compiled(lifetime).expect("borrowed iterator retains its owner"),
        "1\n2\n3\ndrop holder\n"
    );
}

#[test]
fn reference_yielding_iteration_borrows_a_named_source() {
    // `for x in nums` over a user reference-yielding iterator borrows the *named*
    // source (a live shared loan), not a copy: reads flow through the yielded
    // references, `nums` stays usable after the loop, and its `__deinit__` (`-1`)
    // runs exactly once at its ASAP last use — not the two drops a copy emits.
    let source = include_str!("../assets/ok/reference_yielding_iteration_named_source.mojo");
    assert_eq!(
        run_compiled(source).expect("named-source reference iteration compiles"),
        "-2\n0\n10\n20\n-1\n3\n-3\n"
    );
}

#[test]
fn comprehension_borrows_a_named_user_source() {
    // A comprehension over a named user iterable follows the same borrowed-
    // source rules as a `for` statement: the source is bound by reference (one
    // `__deinit__`, at its ASAP last use), stays usable after the comprehension,
    // and the yielded references feed the comprehension element expression.
    let source = include_str!("../assets/ok/comprehension_borrowed_named_source.mojo");
    assert_eq!(
        run_compiled(source).expect("named-source comprehension compiles"),
        "-2\n0\n20\n40\n-1\n3\n-3\n"
    );
}

#[test]
fn returned_origin_pointer_deref_reads_and_writes_through_the_pointee() {
    // `def get(self) -> ref[o] Int: return self.p[0]` over an origin-bearing
    // `UnsafePointer[Int, o]` field executes: the returned handle is re-rooted at
    // the single pointee and the offset-0 index is forwarded to it. Immutable
    // origin reads `7`; a mutable origin writes `42` back through the source.
    let read = include_str!("../assets/origin_ok/returned_pointer_deref.mojo");
    assert_eq!(
        run_compiled(read).expect("immutable pointer-deref return reads the pointee"),
        "7\n"
    );
    let write = include_str!("../assets/origin_ok/returned_pointer_deref_mut.mojo");
    assert_eq!(
        run_compiled(write).expect("mutable pointer-deref return writes through"),
        "7\n42\n"
    );
}

#[test]
fn reference_list_iteration_writes_through_checked_element_handles() {
    let source = include_str!("../conformance/fixtures/reference_iteration.mojo");
    assert_eq!(
        run_compiled(source).expect("reference List iteration compiles"),
        "11 12 13\n"
    );
}

#[test]
fn borrowed_list_iterator_rejects_use_after_structural_invalidation() {
    let source = "def main():\n    var values: List[Int] = [1, 2, 3, 4]\n    for value in values:\n        print(value)\n        if value == 1:\n            values.append(5)\n";
    let error = run_compiled(source).expect_err("append may reallocate borrowed storage");
    assert!(
        error.contains("invalidated interior reference '$iter")
            && error.contains("values[\"element\"]"),
        "{error}"
    );
}

#[test]
fn raising_iterator_catches_only_typed_stop_iteration() {
    let src = "@fieldwise_init\nstruct StopIteration:\n    var marker: Int\n\n@fieldwise_init\nstruct CounterIterator:\n    var current: Int\n    var end: Int\n    def __next__(mut self) raises StopIteration -> Int:\n        if self.current >= self.end:\n            raise StopIteration(0)\n        var result = self.current\n        self.current += 1\n        return result\n\n@fieldwise_init\nstruct Counter:\n    var start: Int\n    var end: Int\n    def __iter__(self) -> CounterIterator:\n        return CounterIterator(self.start, self.end)\n\ndef main():\n    for value in Counter(3, 6):\n        print(value)\n";
    assert_eq!(parity(src), "3\n4\n5\n");
}

#[test]
fn abstract_next_copies_a_copyable_reference_result() {
    let source = include_str!("../conformance/fixtures/copyable_iterator_refinement.mojo");
    assert_eq!(
        run_compiled(source).expect("copyable iterator refinement runs"),
        "copy\n41\n"
    );
}

#[test]
fn generic_iteration_applies_the_copyable_reference_adapter() {
    let source = include_str!("../assets/ok/generic_copyable_iterator_refinement.mojo");
    assert_eq!(
        run_compiled(source).expect("generic refined iterator runs"),
        "42\n"
    );
}

#[test]
fn reference_iteration_over_a_temporary_list_writes_through() {
    // Previously a checker error under the List bridge; the generic protocol
    // retains the temporary in a loop-owned (mutable) slot.
    let source = include_str!("../assets/ok/reference_iteration_temporary_list.mojo");
    assert_eq!(
        run_compiled(source).expect("temporary-List reference iteration runs"),
        "36\n"
    );
}

#[test]
fn set_reference_iteration_writes_through_the_generic_protocol() {
    // Set was never bridged: its `for ref` runs the generic reference-yielding
    // protocol through the delegated borrowed `_ListIter`, writing into the
    // set's backing storage two borrow frames deep.
    let source = include_str!("../assets/ok/set_reference_iteration_write_through.mojo");
    assert_eq!(
        run_compiled(source).expect("set reference iteration writes through"),
        "2\nTrue True\n23\n"
    );
}

#[test]
fn parametric_mut_iterator_writes_through_a_mutable_source() {
    // The loop site resolves `Mutability::Param` from the source: a mutable
    // named source yields mutable references, `for ref x: x += 10` lands in
    // the source, and the comprehension path resolves the same way.
    let source = include_str!("../assets/ok/reference_yielding_iteration_parametric_mut.mojo");
    assert_eq!(
        run_compiled(source).expect("parametric-mut iterator writes through"),
        "45\n28 32\n"
    );
}

#[test]
fn parametric_mut_iterator_reads_through_the_immutable_fallback() {
    // A parametric-mut origin iterator (`m: Bool, //, o: Origin[mut=m]`) over
    // a read-only loop: the unresolved `Mutability::Param` binds immutably.
    let source = include_str!("../assets/ok/parametric_mut_iterator_read.mojo");
    assert_eq!(
        run_compiled(source).expect("parametric-mut iterator reads"),
        "15\n"
    );
}

#[test]
fn generic_borrowed_dispatch_reaches_an_overloaded_ref_self_iter() {
    let source = include_str!("../assets/ok/generic_borrowed_dispatch_overloaded_iter.mojo");
    assert_eq!(
        run_compiled(source).expect("overloaded ref-self __iter__ dispatches generically"),
        "0\n-1\n"
    );
}

#[test]
fn abstract_next_copy_keeps_nested_reference_origins_reachable() {
    let source = include_str!("../assets/ok/copyable_iterator_reference_aggregate.mojo");
    assert_eq!(
        run_compiled(source).expect("reference-bearing iterator element copies"),
        "copy 41\n41\n"
    );
}

#[test]
fn abstract_next_adapts_a_reference_into_a_read_self_frame() {
    let source =
        include_str!("../conformance/fixtures/copyable_iterator_refinement_read_self.mojo");
    assert_eq!(
        run_compiled(source).expect("read-self refined iterator result runs"),
        "7\n"
    );
}

#[test]
fn raising_iter_normalization_propagates_to_the_enclosing_try() {
    let src = "@fieldwise_init\nstruct IterError:\n    var code: Int\n\n@fieldwise_init\nstruct I:\n    var index: Int\n    def __len__(self) -> Int:\n        return 0\n    def __next__(mut self) -> Int:\n        return 0\n\n@fieldwise_init\nstruct C:\n    var marker: Int\n    def __iter__(self) raises IterError -> I:\n        raise IterError(self.marker)\n        return I(0)\n\ndef main():\n    try:\n        for value in C(7):\n            print(value)\n    except error:\n        print(error.code)\n";
    assert_eq!(run_compiled(src).expect("compiler pipeline failed"), "7\n");
}

#[test]
fn slice_bounds_construct_nominal_optional_values() {
    let source = "struct Optional[T: Movable]:\n    var value: Int\n    var present: Bool\n    def __init__(out self):\n        self.value = 0\n        self.present = False\n    def __init__(out self, value: Int, present: Bool):\n        self.value = value\n        self.present = present\n    def or_else(self, default: Int) -> Int:\n        if self.present:\n            return self.value\n        return default\n\ndef main():\n    var present = Slice(1, 4).start.or_else(9)\n    var absent = Slice(None, 4).start.or_else(9)\n    print(present, absent)\n";
    assert_eq!(parity(source), "1 9\n");
}

#[test]
fn unsafe_pointer_alloc_load_store_alias() {
    // `UnsafePointer[T].alloc`/`ptr[i]` load+store, `ptr[i] += e`, and aliasing (a
    // copied pointer shares storage), running over the VM heap arena.
    let src = "from std.memory import unsafe_alloc\n\ndef main():\n    var p: UnsafePointer[Int] = unsafe_alloc[Int](3)\n    p[0] = 10\n    p[1] = 20\n    p[1] += 5\n    var q: UnsafePointer[Int] = p\n    q[0] = 99\n    print(p[0], p[1])\n";
    assert_eq!(parity(src), "99 25\n");
}

#[test]
fn empty_subscript_reads_and_writes_the_pointee() {
    // `p[]` is offset-0 load/store on a heap pointer, and the direct pointee
    // access on a place-origin pointer (writes reach the owner).
    let src = "from std.memory import unsafe_alloc\n\ndef main():\n    var p = unsafe_alloc[Int](1)\n    p[] = 41\n    p[] += 1\n    print(p[])\n    var x = 5\n    var q = Pointer(to=x)\n    q[] += 1\n    print(q[])\n    print(x)\n";
    assert_eq!(parity(src), "42\n6\n6\n");
}

#[test]
fn unsafe_pointer_vocabulary_round_trip() {
    // unsafe_write / unsafe_offset chaining / unsafe_take_pointee /
    // unsafe_deinit_pointee / unsafe_free over the heap arena, plus the
    // write-through on a place-origin pointer.
    let src = "from std.memory import unsafe_alloc\n\ndef main():\n    var p = unsafe_alloc[Int](2)\n    p.unsafe_write(41)\n    p.unsafe_offset(1).unsafe_write(1)\n    print(p[] + p.unsafe_offset(1)[])\n    var taken = p.unsafe_take_pointee()\n    print(taken)\n    p.unsafe_offset(1).unsafe_deinit_pointee()\n    p.unsafe_free()\n    var x = 5\n    var q = Pointer(to=x)\n    q.unsafe_write(9)\n    print(x)\n";
    assert_eq!(parity(src), "42\n41\n9\n");
}

#[test]
fn pointer_keyword_subscript_dereferences() {
    // The keyword spelling executes as the same indexed dereference on heap
    // and place pointers.
    let src = "from std.memory import unsafe_alloc\n\ndef main():\n    var p = unsafe_alloc[Int](2)\n    p.unsafe_write(1)\n    p.unsafe_offset(1).unsafe_write(2)\n    print(p[unsafe_offset=0], p[unsafe_offset=1])\n    p.unsafe_free()\n    var x = 5\n    var q = Pointer(to=x)\n    print(q[unsafe_offset=0])\n";
    assert_eq!(parity(src), "1 2\n5\n");
}

#[test]
fn unsafe_write_copy_keeps_the_source_alive() {
    // The copy= overload runs the element's copy lifecycle: the heap slot owns
    // an independent List while the source stays usable and mutable.
    let src = "from std.memory import unsafe_alloc\n\ndef main():\n    var xs: List[Int] = [1, 2]\n    var p = unsafe_alloc[List[Int]](1)\n    p.unsafe_write(copy=xs)\n    xs.append(3)\n    var stored = p.unsafe_take_pointee()\n    print(len(stored), len(xs))\n    p.unsafe_free()\n";
    assert_eq!(parity(src), "2 3\n");
}

#[test]
fn layout_allocation_round_trip_and_linearity() {
    // The §4 model end to end: alloc(Layout[T](count=n)) → unsafe_ptr →
    // unsafe_offset/unsafe_write → dealloc(allocation^).
    let src = "from std.memory import Layout, dealloc\n\ndef main():\n    var allocation = alloc(Layout[Int](count=4))\n    var ptr = allocation.unsafe_ptr()\n    ptr.unsafe_offset(0).unsafe_write(42)\n    print(ptr[])\n    print(allocation.layout().count())\n    dealloc(allocation^)\n";
    assert_eq!(parity(src), "42\n4\n");
    // dealloc consumes the Allocation: a later use is a transfer error.
    let error = run(
        "from std.memory import Layout, dealloc\n\ndef main():\n    var a = alloc(Layout[Int](count=1))\n    dealloc(a^)\n    var p = a.unsafe_ptr()\n",
    )
    .expect_err("expected a use-after-transfer rejection");
    assert!(error.contains("after it was transferred"), "{error}");
    // …and dropping one implicitly abandons its obligation.
    let error = run(
        "from std.memory import Layout\n\ndef main():\n    var a = alloc(Layout[Int](count=1))\n    print(1)\n",
    )
    .expect_err("expected an abandoned explicit-destroy obligation");
    assert!(error.contains("dealloc(allocation^)"), "{error}");
}

#[test]
fn unsafe_pointer_rejects_reads_from_uninitialized_storage() {
    let error =
        run("from std.memory import unsafe_alloc\n\ndef main():\n    var pointer = unsafe_alloc[Int](1)\n    print(pointer[0])\n")
            .expect_err("UnsafePointer.alloc reserves raw, uninitialized storage");
    assert!(
        error.contains("read of uninitialized Pointer storage"),
        "{error}"
    );
}

#[test]
fn self_hosted_vec_over_unsafe_pointer() {
    // A heap-owning container written in mojito: `push` mutates storage through
    // the pointer (aliased across the value-type copy); the size is written back.
    let src = "from std.memory import unsafe_alloc\n\nstruct IntVec:\n    var data: UnsafePointer[Int]\n    var size: Int\n    def __init__(out self, cap: Int):\n        self.data = unsafe_alloc[Int](cap)\n        self.size = 0\n    def push(mut self, v: Int):\n        self.data[self.size] = v\n        self.size = self.size + 1\n    def get(self, i: Int) -> Int:\n        return self.data[i]\n\ndef main():\n    var xs: IntVec = IntVec(8)\n    xs.push(7)\n    xs.push(8)\n    xs.push(9)\n    print(xs.size, xs.get(0), xs.get(2))\n";
    assert_eq!(parity(src), "3 7 9\n");
}

#[test]
fn copyinit_gives_value_semantics() {
    // A pointer-owning struct with `__copyinit__` deep-copies on `var b = a` and on
    // pass-by-value, so writes through one don't affect the other. `__moveinit__`
    // relocates on `^`.
    let src = "from std.memory import unsafe_alloc\n\nstruct Buf:\n    var data: UnsafePointer[Int]\n    var n: Int\n    def __init__(out self, n: Int):\n        self.data = unsafe_alloc[Int](n)\n        self.n = n\n    def __copyinit__(out self, e: Buf):\n        self.n = e.n\n        self.data = unsafe_alloc[Int](e.n)\n        var i: Int = 0\n        while i < e.n:\n            self.data[i] = e.data[i]\n            i = i + 1\n    def __moveinit__(out self, deinit e: Buf):\n        self.n = e.n\n        self.data = e.data\n    def set(mut self, i: Int, v: Int):\n        self.data[i] = v\n    def get(self, i: Int) -> Int:\n        return self.data[i]\n\ndef main():\n    var a: Buf = Buf(2)\n    a.set(0, 5)\n    a.set(1, 6)\n    var b: Buf = a\n    b.set(0, 9)\n    print(a.get(0), b.get(0))\n    var c: Buf = b^\n    print(c.get(0))\n";
    assert_eq!(parity(src), "5 9\n9\n");
}

#[test]
fn current_unified_move_initializer_uses_the_deinit_convention() {
    let src = include_str!("../conformance/fixtures/unified_lifecycle.mojo");
    assert_eq!(parity(src), "move\n7 7\n");
}

#[test]
fn mojo_copy_constructor_gives_value_semantics() {
    let src = "from std.memory import unsafe_alloc\n\nstruct Buf(Copyable):\n    var data: UnsafePointer[Int]\n    var n: Int\n    def __init__(out self, n: Int):\n        self.data = unsafe_alloc[Int](n)\n        self.n = n\n    def __init__(out self, *, copy: Self):\n        self.n = copy.n\n        self.data = unsafe_alloc[Int](copy.n)\n        var i: Int = 0\n        while i < copy.n:\n            self.data[i] = copy.data[i]\n            i = i + 1\n    def set(mut self, i: Int, v: Int):\n        self.data[i] = v\n    def get(self, i: Int) -> Int:\n        return self.data[i]\n\ndef main():\n    var a: Buf = Buf(2)\n    a.set(0, 5)\n    a.set(1, 6)\n    var b: Buf = Buf(copy: a)\n    b.set(0, 9)\n    print(a.get(0), b.get(0))\n    var c: Buf = a\n    c.set(0, 11)\n    print(a.get(0), c.get(0))\n";
    assert_eq!(parity(src), "5 9\n5 11\n");
}

#[test]
fn copy_constructor_deep_copies_copyable_nominal_fields() {
    // Dict's copy constructor assigns its pointer-owning List field from the
    // borrowed source. That projected read is an ownership-producing copy: it
    // must invoke List.__copyinit__, not duplicate the source allocation handle.
    let src = "from std.collections.dict import Dict\n\ndef main() raises:\n    var original = Dict[String, String]()\n    original[\"phase\"] = \"source\"\n    var copied = original.copy()\n    copied[\"phase\"] = \"copy\"\n    print(original[\"phase\"])\n    print(copied[\"phase\"])\n";
    assert_eq!(
        run_compiled(src).expect("compile nested lifecycle-field copy"),
        "source\ncopy\n"
    );
}

#[test]
fn ternary_and_chained_comparison_run() {
    // Ternary picks a branch; chained comparison evaluates each operand once and
    // short-circuits (a middle False → the rest is not evaluated).
    let src = "def loud(n: Int) -> Int:\n    print(\"e\", n)\n    return n\n\ndef main():\n    var x: Int = 5\n    var m: Int = 10 if x > 0 else 20\n    print(m)\n    print(0 <= x < 10)\n    print(0 <= x < 3)\n    print(1 < 0 < loud(99))\n";
    // loud(99) must NOT run (1 < 0 is False), so no "e 99" line.
    assert_eq!(parity(src), "10\nTrue\nFalse\nFalse\n");
}

#[test]
fn tuple_unpacking_runs() {
    // Unpack into names; swap through an rvalue tuple (RHS built once).
    let src = "def main():\n    var t: Tuple[Int, Int, Int] = (1, 2, 3)\n    var a, b, c = t\n    print(a, b, c)\n    var x: Int = 10\n    var y: Int = 20\n    x, y = (y, x)\n    print(x, y)\n";
    assert_eq!(
        run_compiled(src).expect("compile nominal Tuple unpacking"),
        "1 2 3\n20 10\n"
    );
}

#[test]
fn current_tuple_core_runs() {
    let src = "def main():\n    var bare = 4, \"four\"\n    var left, right = bare\n    print(bare)\n    print(left, right)\n    print(Tuple())\n    print(Tuple(7))\n    print(Tuple[Float64, String](2, \"two\"))\n    print(Tuple[Float64](2)[0])\n    print(len(bare), 4 in bare, 9 not in bare)\n    print(Tuple(1, 2) == Tuple(1, 2))\n    print(Tuple(1, 2) != Tuple(1, 3))\n    print(Tuple(1, 2) < Tuple(1, 3), Tuple(2, 0) >= Tuple(1, 9))\n    print(bare.reverse())\n    print(bare.concat(Tuple(True)))\n";
    assert_eq!(
        run_compiled(src).expect("compile nominal Tuple core"),
        "(4, four)\n4 four\n()\n(7,)\n(2.0, two)\n2.0\n2 True True\nTrue\nTrue\nTrue True\n(four, 4)\n(4, four, True)\n"
    );
}

#[test]
fn reciprocal_tuple_reverse_specializations_use_predeclared_identities() {
    let source = "def main():\n    var numbers_first = Tuple(1, \"two\")\n    var words_first = Tuple(\"three\", 4)\n    print(numbers_first.reverse())\n    print(words_first.reverse())\n";
    assert_eq!(
        run_compiled(source).expect("compile reciprocal Tuple reverse specializations"),
        "(two, 1)\n(4, three)\n"
    );
}

#[test]
fn implicitly_copyable_named_tuple_transforms_preserve_the_source() {
    let source = "def main():\n    var pair = Tuple(1, 2)\n    var suffix = Tuple(3)\n    var reversed = pair.reverse()\n    var joined = pair.concat(suffix)\n    print(pair)\n    print(suffix)\n    print(reversed)\n    print(joined)\n";
    assert_eq!(
        run_compiled(source).expect("compile named Tuple transforms"),
        "(1, 2)\n(3,)\n(2, 1)\n(1, 2, 3)\n"
    );
}

#[test]
fn tuple_comparable_conformance_survives_discovery_and_specialization() {
    let source = "def ordered[T: Comparable](left: T, right: T) -> Bool:\n    return left < right\n\ndef main():\n    print(ordered(Tuple(1, 2), Tuple(1, 3)))\n";
    assert_eq!(
        run_compiled(source).expect("compile generic Tuple comparison"),
        "True\n"
    );
}

#[test]
fn slice_subscript_runs() {
    // List + String slicing: strict contiguous bounds, strided normalization
    // (optional bounds, negative indices, reversal), StringLiteral slices.
    let src = "def main():\n    var xs: List[Int] = [0, 1, 2, 3, 4, 5]\n    print(xs[1:4])\n    print(xs[::2])\n    print(xs[::-1])\n    print(xs[-2::1])\n    var s: StringLiteral = \"hello\"\n    print(s[1:4])\n    print(s[::-1])\n";
    assert_eq!(
        run_compiled(src).expect("compile nominal List slice overloads"),
        "[1, 2, 3]\n[0, 2, 4]\n[5, 4, 3, 2, 1, 0]\n[4, 5]\nell\nolleh\n"
    );
}

#[test]
fn contiguous_list_slice_bounds_abort() {
    // Strict contiguous bounds: negative, out-of-range, and reversed bounds
    // abort instead of normalizing through `Slice.indices()`.
    for slice in ["[-2:]", "[0:9]", "[3:1]"] {
        let src =
            format!("def main():\n    var xs: List[Int] = [0, 1, 2, 3, 4]\n    print(xs{slice})\n");
        let err = run_compiled(&src).expect_err("strict contiguous bounds abort");
        assert!(
            err.contains("abort: List slice bounds out of range"),
            "unexpected error for {slice}: {err}"
        );
    }
}

#[test]
fn abort_is_not_catchable() {
    // `os.abort` is an uncatchable trap: `try`/`except` observes only raised
    // errors, so the abort propagates to the top.
    let src = "from os import abort\n\ndef main():\n    try:\n        abort(\"boom\")\n        print(\"unreachable\")\n    except e:\n        print(\"caught\", e)\n";
    let err = run_compiled(src).expect_err("abort escapes try/except");
    assert!(err.contains("abort: boom"), "unexpected error: {err}");
}

#[test]
fn string_mutation_during_grapheme_iteration_rejects() {
    // Grapheme iteration borrows the String for the whole loop: mutating it
    // inside the body is rejected.
    let src = "def main():\n    var s = String(\"abc\")\n    for g in s:\n        s += \"!\"\n        print(g)\n";
    let err = run_compiled(src).expect_err("mutation during iteration rejects");
    assert!(
        err.contains("conflicts with live reference"),
        "unexpected error: {err}"
    );
}

#[test]
fn string_view_conflicts_with_source_mutation() {
    // A live StringSpan holds a shared loan on its source String: mutating
    // the String while any view is alive is rejected.
    let src = "def main():\n    var s = String(\"hello\")\n    var v = s[byte=1:3]\n    s += \"!\"\n    print(v)\n";
    let err = run_compiled(src).expect_err("mutation under a live view rejects");
    assert!(
        err.contains("conflicts with live reference"),
        "unexpected error: {err}"
    );
}

#[test]
fn string_slice_alias_resolves_to_string_span() {
    // `StringSlice` stays accepted as an upstream compatibility alias for
    // the canonical StringSpan in annotations.
    let src = "def main():\n    var s = String(\"hello\")\n    var v: StringSlice = s[byte=1:3]\n    print(v)\n";
    assert_eq!(
        run_compiled(src).expect("compile the StringSlice alias"),
        "el\n"
    );
}

#[test]
fn span_conflicts_with_source_mutation() {
    // A live Span holds a shared loan on its source List: a structural
    // mutation of the List while any view is alive is rejected.
    let src = "def main():\n    var xs: List[Int] = [1, 2, 3]\n    var sp = Span(xs)\n    xs.append(4)\n    print(sp[0])\n";
    let err = run_compiled(src).expect_err("mutation under a live span rejects");
    assert!(
        err.contains("conflicts with live reference"),
        "unexpected error: {err}"
    );
}

#[test]
fn span_subslice_keeps_the_source_borrowed() {
    // A sub-span carries the same loan as its parent view: mutating the
    // source while only the sub-span is alive still conflicts.
    let src = "def main():\n    var xs: List[Int] = [1, 2, 3, 4]\n    var sub = Span(xs)[1:3]\n    xs.append(5)\n    print(sub[0])\n";
    let err = run_compiled(src).expect_err("sub-span keeps the loan");
    assert!(
        err.contains("conflicts with live reference"),
        "unexpected error: {err}"
    );
}

#[test]
fn list_unsafe_ptr_stales_after_append() {
    // The linked `List.unsafe_ptr()` pointer carries the element
    // interior-generation loan: append starts a new generation, so a later
    // use of the pointer is rejected. (The link-free twin of this contract is
    // assets/ownership_error/interior_pointer_stale_after_mutation.mojo.)
    let src = "def main():\n    var xs: List[Int] = [10, 20, 30]\n    var p = xs.unsafe_ptr()\n    xs.append(40)\n    print(p[0])\n";
    let err = run_compiled(src).expect_err("stale interior pointer rejects");
    assert!(
        err.contains("invalidated interior reference"),
        "unexpected error: {err}"
    );
}

#[test]
fn pointer_to_place_aliases_the_source_place() {
    let src = "def main():\n    var x = 42\n    var p = UnsafePointer(to=x)\n    print(p[0])\n    p[0] = 7\n    p[0] += 1\n    print(x)\n";
    assert_eq!(vm(src), "42\n8\n");
}

#[test]
fn pointer_owner_drops_after_the_pointer_last_use() {
    let src = "struct Box:\n    var n: Int\n    def __init__(out self, n: Int):\n        self.n = n\n    def __deinit__(deinit self):\n        print(\"drop\", self.n)\n\ndef main():\n    var box = Box(1)\n    var p = UnsafePointer(to=box.n)\n    print(\"before\")\n    print(p[0])\n    print(\"after\")\n";
    assert_eq!(vm(src), "before\ndrop 1\n1\nafter\n");
}

#[test]
fn pointer_aggregate_derefs_through_the_stored_handle() {
    let src = "@fieldwise_init\nstruct Borrowed[origin: Origin]:\n    var ptr: UnsafePointer[Int, Self.origin]\n\ndef main():\n    var value = 42\n    var b = Borrowed(UnsafePointer(to=value))\n    print(b.ptr[0])\n    b.ptr[0] = 9\n    print(value)\n";
    assert_eq!(vm(src), "42\n9\n");
}

#[test]
fn immutable_pointer_allows_concurrent_owner_reads() {
    let src = "def observe(x: Int):\n    var p = UnsafePointer(to=x)\n    print(p[0], x)\n\ndef main():\n    observe(5)\n";
    assert_eq!(vm(src), "5 5\n");
}

#[test]
fn reference_valued_aggregate_preserves_and_writes_through_handle() {
    let src = include_str!("../conformance/fixtures/reference_valued_aggregate.mojo");
    assert_eq!(vm(src), "42\n42\n42\n");
}

#[test]
fn handwritten_initializer_stores_reference_field_handle() {
    let src = "struct RefBox[origin: Origin[mut=True]]:\n    var value: ref[origin] Int\n    def __init__(out self, ref[origin] value: Int):\n        self.value = value\n\ndef main():\n    var value = 40\n    var box = RefBox(value)\n    box.value += 2\n    print(value)\n";
    assert_eq!(parity(src), "42\n");
}

#[test]
fn runtime_returned_interior_reference_stores_in_fieldwise_aggregate() {
    let src = "@fieldwise_init\nstruct RefBox[origin: Origin[mut=True]]:\n    var value: ref[origin] Int\n\ndef element(ref values: List[Int]) -> ref[origin_of(values)._get_owned_interior[\"element\"]] Int:\n    return values[0]\n\ndef main():\n    var values: List[Int] = [40]\n    ref item = element(values)\n    var box = RefBox(item)\n    box.value += 2\n    print(values[0])\n";
    assert_eq!(vm(src), "42\n");
}

#[test]
fn nested_reference_aggregate_preserves_handles() {
    // Mojito-only executable-ref-field proof; current Mojo uses origin-bearing
    // pointer aggregates for stored provenance.
    let tuple = "@fieldwise_init\nstruct RefTuple[origin: Origin[mut=True]]:\n    var values: Tuple[ref[origin] Int, ref[origin] Int]\n\ndef main():\n    var left = 4\n    var right = 8\n    ref a = left\n    ref b = right\n    var pair = RefTuple((a, b))\n    print(pair.values[0], pair.values[1])\n";
    assert_eq!(
        run_compiled(tuple).expect("compile reference-valued nominal Tuple"),
        "4 8\n"
    );

    let list = "@fieldwise_init\nstruct RefList[origin: Origin[mut=True]]:\n    var values: List[ref[origin] Int]\n\ndef main():\n    var left = 4\n    var right = 8\n    ref a = left\n    ref b = right\n    var pair = RefList([a, b])\n    pair.values[1] += 2\n    print(left, right)\n";
    assert_eq!(vm(list), "4 10\n");
}

#[test]
fn chained_method_on_reference_valued_list_element_uses_the_peeled_handle() {
    let src = "@fieldwise_init\nstruct Item(Copyable, Movable):\n    var value: Int\n    def bump(mut self):\n        self.value += 2\n\n@fieldwise_init\nstruct RefList[origin: Origin[mut=True]]:\n    var values: List[ref[origin] Item]\n\ndef main():\n    var item = Item(40)\n    ref alias = item\n    var refs = RefList([alias])\n    refs.values[0].bump()\n    print(item.value)\n";
    assert_eq!(vm(src), "42\n");
}

#[test]
fn dynamic_reference_returning_actual_is_evaluated_once_and_writes_back() {
    let src = "@fieldwise_init\nstruct Box:\n    var value: Int\n    def __getitem__(ref self, index: Int) -> ref[origin_of(self.value)] Int:\n        return self.value\n\ndef index() -> Int:\n    print(\"index\")\n    return 0\n\ndef bump(mut value: Int):\n    value += 2\n\ndef observe(ref value: Int):\n    print(value)\n\ndef main():\n    var box = Box(40)\n    bump(box[index()])\n    observe(box[index()])\n    print(box.value)\n";
    assert_eq!(vm(src), "index\nindex\n42\n42\n");
}

#[test]
fn dynamic_intrinsic_index_actual_is_evaluated_once_and_writes_back() {
    let src = "def index() -> Int:\n    print(\"index\")\n    return 0\n\ndef bump(mut value: Int):\n    value += 2\n\ndef main():\n    var lanes = SIMD[DType.int, 2](40, 0)\n    bump(lanes[index()])\n    print(lanes[0])\n";
    assert_eq!(vm(src), "index\n42\n");
}

#[test]
fn nominal_setter_evaluates_receiver_then_subscripts_then_rhs() {
    let src = "@fieldwise_init\nstruct Box(Copyable, Movable):\n    var value: Int\n    def __setitem__(mut self, index: Int, value: Int):\n        self.value = value + index\n\n@fieldwise_init\nstruct Outer(Copyable, Movable):\n    var box: Box\n    def __getitem__(ref self, index: Int) -> ref[origin_of(self.box)] Box:\n        return self.box\n\ndef rhs() -> Int:\n    print(\"rhs\")\n    return 40\n\ndef receiver_index() -> Int:\n    print(\"receiver\")\n    return 0\n\ndef index() -> Int:\n    print(\"index\")\n    return 2\n\ndef main():\n    var outer = Outer(Box(0))\n    outer[receiver_index()][index()] = rhs()\n    print(outer.box.value)\n";
    assert_eq!(vm(src), "receiver\nindex\nrhs\n42\n");
}

#[test]
fn union_return_through_runtime_reference_arguments_keeps_the_selected_owner_alive() {
    let src = "def element(ref values: List[Int]) -> ref[origin_of(values)._get_owned_interior[\"element\"]] Int:\n    return values[0]\n\ndef choose(ref left: Int, ref right: Int, flag: Bool) -> ref[left, right] Int:\n    if flag:\n        return left\n    return right\n\ndef main():\n    var left_values: List[Int] = [1]\n    var right_values: List[Int] = [2]\n    ref left = element(left_values)\n    ref right = element(right_values)\n    ref selected = choose(left, right, False)\n    print(selected)\n";
    assert_eq!(vm(src), "2\n");
}

#[test]
fn variant_projection_is_a_tag_checked_place() {
    let src = "struct Variant:\n    pass\n\ndef main():\n    var value = Variant[Int, String](7)\n    value[Int] += 5\n    print(value[Int])\n";
    assert_eq!(vm(src), "12\n");

    // Mojito's executable local-ref extension exercises the same place as a
    // persistent frame/slot handle, rather than a cloned VariantGet payload.
    let src = "struct Variant:\n    pass\n\ndef main():\n    var value = Variant[Int, String](7)\n    ref payload = value[Int]\n    payload += 5\n    print(value[Int])\n";
    assert_eq!(vm(src), "12\n");
}

#[test]
fn user_slice_dispatches_through_checked_getitem() {
    let src = "@fieldwise_init\nstruct Window:\n    var size: Int\n\n    def __getitem__(self, part: Slice) -> Int:\n        var normalized = part.indices(self.size)\n        return normalized[0] + normalized[1] + normalized[2]\n\n@fieldwise_init\nstruct Grid:\n    def __getitem__(self, row: Int, columns: Slice) -> Int:\n        var normalized = columns.indices(10)\n        return row * 100 + normalized[0] + normalized[1] + normalized[2]\n\ndef main():\n    var window = Window(10)\n    print(window[:5])\n    print(window[::-1])\n    var grid = Grid()\n    print(grid[3, 1:8:2])\n";
    assert_eq!(run_compiled(src).expect("vm backend failed"), "6\n7\n311\n");
}

#[test]
fn slice_descriptor_overloads_are_selected_statically() {
    let src = "@fieldwise_init\nstruct Probe:\n    def __getitem__(self, part: ContiguousSlice) -> Int:\n        return 1\n    def __getitem__(self, part: StridedSlice) -> Int:\n        return 2\n\ndef main():\n    var probe = Probe()\n    print(probe[1:5], probe[1:5:2], probe[::])\n";
    assert_eq!(parity(src), "1 2 2\n");
}

#[test]
fn multi_index_dispatch_supports_variadic_getitem() {
    let src = "@fieldwise_init\nstruct Cube:\n    def __getitem__(self, *indices: Int) -> Int:\n        return indices[0] * 100 + indices[1] * 10 + indices[2]\n\ndef main():\n    var cube = Cube()\n    print(cube[1, 2, 3])\n";
    assert_eq!(parity(src), "123\n");
}

#[test]
fn mixed_slice_assignment_dispatches_to_fixed_setitem() {
    let src = "@fieldwise_init\nstruct Grid:\n    var value: Int\n    def __setitem__(mut self, row: Int, columns: Slice, value: Int):\n        var normalized = columns.indices(10)\n        self.value = row * 100 + normalized[0] + normalized[1] + normalized[2] + value\n\ndef main():\n    var grid = Grid(0)\n    grid[3, 1:8:2] = 9\n    print(grid.value)\n";
    assert_eq!(run_compiled(src).expect("vm backend failed"), "320\n");
}

#[test]
fn multidimensional_assignment_supports_variadic_setitem() {
    let src = "@fieldwise_init\nstruct Cube:\n    var value: Int\n    def __setitem__(mut self, *indices: Int, *, value: Int):\n        self.value = indices[0] * 1000 + indices[1] * 100 + indices[2] * 10 + value\n\ndef main():\n    var cube = Cube(0)\n    cube[1, 2, 3] = 4\n    print(cube.value)\n";
    assert_eq!(parity(src), "1234\n");
}

#[test]
fn subscript_call_contracts_execute_uniformly() {
    assert_eq!(
        run_compiled(include_str!(
            "../conformance/fixtures/subscript_call_contracts.mojo"
        ))
        .expect("compile complete subscript call contracts"),
        "42\n44\n311\n320\ncaught getter\ncaught setter\n7\n1\n41 1 41\n41 41\n42\n"
    );
}

#[test]
fn subscript_contract_edges_match_current_mojo() {
    assert_eq!(
        run_compiled(include_str!(
            "../conformance/fixtures/subscript_contract_edges.mojo"
        ))
        .expect("compile subscript contract edge cases"),
        "receiver\nindex\nrhs\n42\nnext\n42\n7\n"
    );
}

#[test]
fn owned_nominal_element_copy_uses_the_checked_accessor() {
    assert_eq!(
        run_compiled(include_str!(
            "../conformance/fixtures/owned_nominal_element_copy.mojo"
        ))
        .expect("compile owned nominal element copy"),
        "10\n"
    );
}

#[test]
fn dict_element_copies_execute_in_bindings_and_consuming_arguments() {
    let source = "from std.collections.dict import Dict\n\ndef take(value: Int) -> Int:\n    return value\n\ndef main() raises:\n    var values = Dict[String, Int]()\n    values[\"one\"] = 10\n    var copied: Int = values[\"one\"]\n    print(copied, take(values[\"one\"]))\n";
    assert_eq!(
        run_compiled(source).expect("compile owned Dict element copies"),
        "10 10\n"
    );
}

#[test]
fn ordinary_index_arguments_still_execute_selected_implicit_conversions() {
    assert_eq!(
        run_compiled(include_str!(
            "../conformance/fixtures/index_implicit_conversion.mojo"
        ))
        .expect("compile implicit index conversion"),
        "13\n"
    );
}

#[test]
fn projected_nominal_references_execute_and_pass_as_call_places() {
    assert_eq!(
        run_compiled(include_str!(
            "../conformance/fixtures/projected_subscript_reference.mojo"
        ))
        .expect("compile projected nominal references"),
        "10\n11\n11\n"
    );
}

#[test]
fn projected_pointer_actuals_evaluate_dynamic_indices_once() {
    assert_eq!(
        run_compiled(include_str!(
            "../conformance/fixtures/projected_pointer_subscript.mojo"
        ))
        .expect("compile projected pointer actuals"),
        "index\nindex\n42\n42 2\n"
    );
}

#[test]
fn augmented_nominal_subscripts_match_current_mojo_order() {
    assert_eq!(
        run_compiled(include_str!(
            "../conformance/fixtures/augmented_subscript_contract.mojo"
        ))
        .expect("compile augmented nominal subscript"),
        "index\nrhs\nget 0\nset 0 15\n15\n"
    );
    assert_eq!(
        run_compiled(include_str!(
            "../conformance/fixtures/augmented_subscript_shapes.mojo"
        ))
        .expect("compile augmented multi/slice subscripts"),
        "first\nsecond\nrhs\nmulti get 1 2\nmulti set 1 2 13\n13\n\
         first\nsecond\nrhs\nslice get\nslice set 23\n23\n"
    );
    assert_eq!(
        run_compiled(include_str!(
            "../conformance/fixtures/augmented_keyword_setter.mojo"
        ))
        .expect("compile augmented keyword-only setter conversion"),
        "index\nrhs\nget 0\nconvert 15\nset 0 15\n15\n"
    );
    assert_eq!(
        run_compiled(include_str!(
            "../conformance/fixtures/augmented_index_conversion.mojo"
        ))
        .expect("compile getter-specific augmented index conversion"),
        "index\nrhs\nconvert 0\nget 0\nset 0 42\n42\n"
    );
    assert_eq!(
        run_compiled(include_str!(
            "../conformance/fixtures/augmented_mutating_index.mojo"
        ))
        .expect("compile augmented mutating index reload"),
        "1 1\n"
    );
    assert_eq!(
        run_compiled(include_str!(
            "../conformance/fixtures/augmented_reference_getter.mojo"
        ))
        .expect("compile mutable-reference augmented getter"),
        "index\nget 0\nrhs\n42\n"
    );
}

#[test]
fn explicit_slice_values_expose_optional_fields_and_indices() {
    let src = "def main():\n    var span = Slice(None, None, -1)\n    print(span.start.is_some(), span.end.or_else(9), span.step.or_else(1))\n    print(span.indices(4))\n    print(slice(3).indices(10))\n";
    assert_eq!(
        run_compiled(src).expect("vm backend failed"),
        "False 9 -1\n(3, -1, -1)\n(0, 3, 1)\n"
    );
}

#[test]
fn struct_prefix_operators_dispatch_through_dunders() {
    assert_eq!(
        vm(
            "@fieldwise_init\nstruct V:\n    var n: Int\n    def __neg__(self) -> V:\n        return V(-self.n)\n\ndef main():\n    var a = V(3)\n    print((-a).n)\n"
        ),
        "-3\n"
    );
    assert_eq!(
        vm(
            "@fieldwise_init\nstruct Flag:\n    var on: Bool\n    def __bool__(self) -> Bool:\n        return self.on\n\ndef main():\n    var f = Flag(False)\n    if not f:\n        print(\"off\")\n"
        ),
        "off\n"
    );
}

#[test]
fn struct_conversions_and_rounding_dispatch_through_dunders() {
    let src = "@fieldwise_init\nstruct Money:\n    var cents: Int\n    def __int__(self) -> Int:\n        return self.cents\n    def __bool__(self) -> Bool:\n        return self.cents != 0\n    def __abs__(self) -> Money:\n        return Money(-self.cents) if self.cents < 0 else Money(self.cents)\n\ndef main():\n    print(Int(Money(-250)))\n    print(abs(Money(-250)).cents)\n    print(Bool(Money(0)))\n";
    assert_eq!(vm(src), "-250\n250\nFalse\n");
}

#[test]
fn explicit_bound_generic_application_iterates_concrete_collections() {
    // An explicit concrete application of a plain trait-bound generic clones
    // and re-checks the body with `C := List[Int]` / `Set[Int]`, so the
    // generic `for` runs through ordinary concrete borrowed iteration with no
    // erased dispatch at these call sites.
    let src = "def total[C: Iterable](c: C) -> Int:\n    var acc = 0\n    for item in c:\n        acc += item\n    return acc\n\ndef main():\n    var xs: List[Int] = [3, 4, 5]\n    print(total[List[Int]](xs))\n    var s: Set[Int] = Set[Int]()\n    s.add(30)\n    print(total[Set[Int]](s))\n";
    assert_eq!(run_compiled(src).unwrap(), "12\n30\n");
}

#[test]
fn inferred_bound_generic_applications_monomorphize_end_to_end() {
    // The checker-discovered instantiation replays through the compiler's
    // discovery fixpoint: inferred stdlib and local calls run through concrete
    // clones with unchanged results.
    assert_eq!(
        run_compiled(
            "from std.algorithms import first_or\n\ndef main() raises:\n    print(first_or(range(3, 7), -1))\n"
        )
        .expect("inferred stdlib generic runs"),
        "3\n"
    );
    assert_eq!(
        run_compiled(
            "def inner[T: Copyable & Movable](x: T) -> T:\n    return x\n\ndef outer[T: Copyable & Movable](x: T) -> T:\n    return inner(x)\n\ndef main():\n    print(outer(7))\n"
        )
        .expect("round-two inferred instantiation runs"),
        "7\n"
    );
}

#[test]
fn raw_seam_executes_inferred_polymorphic_recursion_abstractly() {
    // Only the authoritative `Compiler` owns the discovery fixpoint (and its
    // divergence cap); the stage-composed seam stays request-free, so the
    // program the compiler rejects as divergent executes here on the erased
    // path.
    let src = "def wrap[T: Copyable & Movable](x: T, depth: Int) -> Int:\n    if depth <= 0:\n        return 0\n    return wrap([x], depth - 1)\n\ndef main():\n    print(wrap(1, 3))\n";
    assert_eq!(run(src).expect("abstract execution"), "0\n");
}

#[test]
fn retained_template_executes_erased_dispatch_under_the_compiler() {
    // The runtime half of the erased-dispatch residue witness: the retained
    // abstract template's `__iterator_dispatch` protocol and copy adapter
    // execute end to end through the authoritative pipeline.
    let src = "from std.iterable import Iterable\n\ndef first[C: Iterable](items: C, default: C.Element) -> C.Element:\n    for item in items:\n        return item\n    return default\n\ndef main():\n    comptime for i in (1, \"s\"):\n        print(first([i, i], i))\n";
    assert_eq!(run_compiled(src).expect("erased path runs"), "1\ns\n");
}

#[test]
fn bound_generic_function_value_invokes_through_the_compiler() {
    // The function-value fallback executes under the authoritative pipeline:
    // the template stays abstract and the indirect call retargets at runtime.
    let src = "def ident[T: Copyable & Movable](x: T) -> T:\n    return x\n\ndef main():\n    var callback: def(Int) -> Int = ident\n    print(callback(41))\n";
    assert_eq!(run_compiled(src).expect("function value runs"), "41\n");
}
