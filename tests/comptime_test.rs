//! Compile-time elaboration (`comptime if` / `comptime for`). Each test runs the
//! real pipeline stage order — parse → **elaborate** → check → VM — so it exercises
//! the phase-distinction semantics: unselected branches are dropped before checking,
//! and `comptime for` unrolls with the loop variable substituted as a literal.

use mojito::{Compiler, CtValue, Ty, elaborate, parse};

fn run(src: &str) -> Result<String, String> {
    run_compiled(src)
}

/// Run source through the authoritative two-pass compiler pipeline. Public
/// `Tuple` is a nominal variadic struct, so tests that construct a Tuple from
/// inferred argument types must include the discovery/materialization pass
/// rather than invoking the lower-level elaborator directly.
fn run_compiled(src: &str) -> Result<String, String> {
    let compiler = Compiler::default();
    let program = compiler
        .compile_unlinked(src)
        .map_err(|error| error.to_string())?;
    compiler
        .execute(&program)
        .map(|execution| execution.output)
        .map_err(|error| error.to_string())
}

#[test]
fn ct_value_can_carry_a_type_without_runtime_materialization() {
    let ty = Ty::ComptimeList(Box::new(Ty::Int));
    let value = CtValue::Type(Box::new(ty));

    assert_eq!(value.to_string(), "<comptime-list[Int]>");
    assert!(value.materialize((0, 0)).is_none());
}

#[test]
fn comptime_if_selects_a_branch() {
    let src = "comptime N = 8\n\ndef main():\n    comptime if N > 4:\n        print(\"big\")\n    elif N > 0:\n        print(\"small\")\n    else:\n        print(\"zero\")\n";
    assert_eq!(run(src).unwrap(), "big\n");
}

#[test]
fn comptime_if_drops_unselected_branch_before_checking() {
    // The `else` branch has a type error, but it is dropped by elaboration, so the
    // program still type-checks and runs — the key metaprogramming property.
    let src = "comptime FLAG = 1\n\ndef main():\n    comptime if FLAG == 1:\n        print(\"ok\")\n    else:\n        var bad: Int = \"not an int\"\n        print(bad)\n";
    assert_eq!(run(src).unwrap(), "ok\n");
}

#[test]
fn comptime_for_unrolls_with_substitution() {
    // `i` becomes a literal in each unrolled copy (0², 1², 2², 3²).
    let src = "def main():\n    comptime for i in range(4):\n        print(i, i * i)\n";
    assert_eq!(run(src).unwrap(), "0 0\n1 1\n2 4\n3 9\n");
}

#[test]
fn comptime_for_over_a_const_with_nested_comptime_if() {
    let src = "comptime COUNT = 5\n\ndef main():\n    comptime for i in range(COUNT):\n        comptime if i % 2 == 0:\n            print(i, \"even\")\n        else:\n            print(i, \"odd\")\n";
    assert_eq!(run(src).unwrap(), "0 even\n1 odd\n2 even\n3 odd\n4 even\n");
}

#[test]
fn comptime_for_range_variants_and_reverse() {
    let src = "def main():\n    comptime for i in range(2, 8, 2):\n        print(i)\n    comptime for j in range(3, 0, -1):\n        print(j)\n";
    assert_eq!(run(src).unwrap(), "2\n4\n6\n3\n2\n1\n");
}

#[test]
fn comptime_for_quota_rejects_a_huge_unroll() {
    let err =
        run("def main():\n    comptime for i in range(1000000):\n        print(i)\n").unwrap_err();
    assert!(err.contains("quota"), "got {err}");
}

#[test]
fn comptime_integer_arithmetic_is_arbitrary_precision() {
    let output =
        run("def main():\n    comptime huge = 2 ** 200\n    print((huge + 1) - huge)\n").unwrap();
    assert_eq!(output, "1\n");
}

#[test]
fn comptime_for_iterates_a_heterogeneous_tuple() {
    // The payoff: `t[i]` needs a compile-time-constant index (tuple elements are
    // heterogeneous), which a runtime `for` can't provide — but `comptime for`
    // substitutes `i` with a literal, so each `t[i]` type-checks.
    let src = "def main():\n    var t: Tuple[Int, String, Bool] = (42, \"hi\", True)\n    comptime for i in range(3):\n        print(t[i])\n";
    assert_eq!(run(src).unwrap(), "42\nhi\nTrue\n");
}

#[test]
fn cloned_comptime_bodies_keep_distinct_checked_occurrence_facts() {
    let src = "def outer[n: Int]():\n    comptime for value in (1, True):\n        if True:\n            var x = value\n            def show() {x}:\n                print(x)\n            show()\n\ndef main():\n    outer[0]()\n";
    assert_eq!(run(src).unwrap(), "1\nTrue\n");
}

#[test]
fn comptime_for_over_a_tuple_of_strings() {
    // The codex-direction milestone: iterate a compile-time tuple of strings.
    let src = "comptime states = (\"empty\", \"occupied\", \"deleted\")\n\ndef main():\n    comptime for state in states:\n        print(state)\n";
    assert_eq!(run(src).unwrap(), "empty\noccupied\ndeleted\n");
}

#[test]
fn heterogeneous_type_pack_round_trips_through_tuple_spread() {
    // Mirrors current Mojo: a heterogeneous variadic pack can be transferred
    // into `Tuple[*Ts]`; this is not general fixed-arity call spreading.
    let src = "def repack[*Ts: Movable](var *args: *Ts) -> Tuple[*Ts]:\n    return Tuple[*Ts](*args^)\n\ndef main():\n    var values: Tuple[Int, String, Bool] = repack(3, \"seven\", True)\n    print(values)\n";
    assert_eq!(run(src).unwrap(), "(3, seven, True)\n");
}

#[test]
fn runtime_pack_spread_rejects_shadowing_value_bindings() {
    let block = "def inspect[*Ts: Movable & ImplicitlyDeletable](var *args: *Ts):\n    if True:\n        var args = Tuple(9, 10)\n        var local = Tuple(*args^)\n        print(local)\n\ndef main():\n    inspect(1, True)\n";
    let nested = "def inspect[*Ts: Movable & ImplicitlyDeletable](var *args: *Ts):\n    def nested(var args: Tuple[Int, Int]):\n        print(Tuple(*args^))\n    nested(Tuple(9, 10))\n\ndef main():\n    inspect(1, True)\n";
    let loop_binding = "def inspect[*Ts: Movable & ImplicitlyDeletable](var *args: *Ts):\n    for args in [Tuple(9, 10)]:\n        print(Tuple(*args))\n\ndef main():\n    inspect(1, True)\n";
    let comprehension = "def inspect[*Ts: Movable & ImplicitlyDeletable](var *args: *Ts):\n    var lengths = [len(Tuple(*args)) for args in [Tuple(9, 10)]]\n    print(lengths[0])\n\ndef main():\n    inspect(1, True)\n";
    let sibling_method = "struct Pair[*Ts: Copyable & Movable](Copyable, Movable):\n    var storage: Tuple[*Ts]\n    def __init__(out self, var *args: *Ts):\n        self.storage = Tuple(*args^)\n    def shadow(self, var args: Tuple[Int, Int]):\n        print(Tuple(*args^))\n\ndef main():\n    var pair = Pair[Int](1)\n    pair.shadow(Tuple(9, 10))\n";

    for source in [block, nested, loop_binding, comprehension, sibling_method] {
        let error = run(source).unwrap_err();
        assert!(
            error.contains("call spread outside a specialized type pack"),
            "got: {error}"
        );
    }
}

#[test]
fn runtime_pack_binding_is_restored_after_block_and_loop_shadows() {
    let src = "def count[*Ts: Movable & ImplicitlyDeletable](var *args: *Ts) -> Int:\n    if True:\n        var args = Tuple(7, 8)\n        print(len(args))\n    for args in [Tuple(9, 10)]:\n        print(len(args))\n    var packed = Tuple(*args^)\n    return len(packed)\n\ndef main():\n    print(count(1, \"two\", True))\n";
    assert_eq!(run(src).unwrap(), "2\n2\n3\n");
}

#[test]
fn empty_runtime_pack_is_recognized_by_binding_presence() {
    let src = "def repack[*Ts: Movable](var *args: *Ts) -> Tuple[*Ts]:\n    return Tuple[*Ts](*args^)\n\ndef main():\n    var values = repack()\n    print(len(values))\n";
    assert_eq!(run(src).unwrap(), "0\n");
}

#[test]
fn type_pack_expansion_respects_nested_type_parameter_shadowing() {
    let src = "def inspect[*Ts: Copyable & ImplicitlyDeletable](*args: *Ts):\n    def nested[Ts: AnyType](value: Tuple[*Ts]) -> Int:\n        return len(value)\n    print(nested[Int](Tuple(1, True)))\n\ndef main():\n    inspect(9, False)\n";
    let error = run(src).unwrap_err();
    assert!(error.contains("unknown type '*Ts'"), "got: {error}");
}

#[test]
fn nested_heterogeneous_pack_specializes_at_its_lexical_declaration() {
    let src = "def count[*Ts: Movable & ImplicitlyDeletable](var *args: *Ts) -> Int:\n    def nested[*Ts: Movable & ImplicitlyDeletable](var *args: *Ts) -> Int:\n        return len(Tuple(*args^))\n    return nested(1, \"two\", True) + len(Tuple(*args^))\n\ndef main():\n    print(count(9, False))\n";
    assert_eq!(run(src).unwrap(), "5\n");
}

#[test]
fn nested_pack_supports_empty_and_distinct_specializations() {
    let src = "def outer():\n    def count[*Ts: Movable & ImplicitlyDeletable](var *args: *Ts) -> Int:\n        return len(args)\n    print(count())\n    print(count(1))\n    print(count(1, \"two\"))\n\ndef main():\n    outer()\n";
    assert_eq!(run(src).unwrap(), "0\n1\n2\n");
}

#[test]
fn nested_value_parameter_specialization_resolves_comptime_control_flow() {
    let src = "def outer():\n    def choose[flag: Bool]() -> Int:\n        comptime if flag:\n            return 41\n        else:\n            return 1\n    print(choose[True]() + choose[False]())\n\ndef main():\n    outer()\n";
    assert_eq!(run(src).unwrap(), "42\n");
}

#[test]
fn nested_pack_can_forward_to_an_earlier_pack_sibling() {
    let src = "def outer() -> Int:\n    def count[*Ts: Movable & ImplicitlyDeletable](var *args: *Ts) -> Int:\n        return len(args)\n    def relay[*Ts: Movable & ImplicitlyDeletable](var *args: *Ts) -> Int:\n        return count(*args^)\n    return relay(1, \"two\", True)\n\ndef main():\n    print(outer())\n";
    assert_eq!(run(src).unwrap(), "3\n");
}

#[test]
fn captured_outer_pack_forwarding_infers_only_the_variadic_overflow() {
    let src = "def outer[*Ts: Movable & ImplicitlyDeletable](var *args: *Ts) -> Int:\n    def score[*Us: Movable & ImplicitlyDeletable](head: Int, var *values: *Us) -> Int:\n        return head + len(values)\n    def relay() unified {args^} -> Int:\n        return score(40, *args^)\n    return relay()\n\ndef main():\n    print(outer(1, True))\n";
    assert_eq!(run(src).unwrap(), "42\n");
}

#[test]
fn nested_pack_reference_return_preserves_the_caller_handle() {
    let src = "def main():\n    var value = 40\n    def borrow[*Ts: Movable & ImplicitlyDeletable](ref item: Int, var *args: *Ts) -> ref[item] Int:\n        return item\n    ref borrowed = borrow(value, True)\n    borrowed += 2\n    print(value)\n";
    assert_eq!(run(src).unwrap(), "42\n");
}

#[test]
fn nested_pack_forwarding_transfers_move_only_elements() {
    let src = "struct Item(Movable):\n    var value: Int\n    def __init__(out self, value: Int):\n        self.value = value\n    def __init__(out self, *, deinit move: Self):\n        self.value = move.value\n    def __del__(deinit self):\n        print(\"drop\", self.value)\n\ndef outer() -> Int:\n    def first[*Ts: Movable & ImplicitlyDeletable](var *args: *Ts) -> Int:\n        return args[0].value\n    def relay[*Ts: Movable & ImplicitlyDeletable](var *args: *Ts) -> Int:\n        return first(*args^)\n    return relay(Item(42))\n\ndef main():\n    print(outer())\n";
    assert_eq!(run(src).unwrap(), "drop 42\n42\n");
}

#[test]
fn nested_whole_pack_forwarding_preserves_fixed_prefix_and_keyword_tail() {
    let src = "struct Item(Movable):\n    var value: Int\n    def __init__(out self, value: Int):\n        self.value = value\n    def __init__(out self, *, deinit move: Self):\n        self.value = move.value\n    def __del__(deinit self):\n        print(\"drop\", self.value)\n\ndef outer() -> Int:\n    def score[*Ts: Movable & ImplicitlyDeletable](out result: Int, head: Int, var *values: *Ts, scale: Int = 1):\n        result = (head + values[0].value) * scale\n    def relay[*Ts: Movable & ImplicitlyDeletable](var *values: *Ts) -> Int:\n        return score(1, *values^, scale=2)\n    return relay(Item(20))\n\ndef main():\n    print(outer())\n";
    assert_eq!(run(src).unwrap(), "drop 20\n42\n");
}

#[test]
fn top_level_whole_pack_forwarding_preserves_fixed_prefix_and_linear_values() {
    let src = "struct Item(Movable):\n    var value: Int\n    def __init__(out self, value: Int):\n        self.value = value\n    def __init__(out self, *, deinit move: Self):\n        self.value = move.value\n    def __del__(deinit self):\n        print(\"drop\", self.value)\n\ndef score[*Ts: Movable & ImplicitlyDeletable](head: Int, var *values: *Ts) -> Int:\n    return head + values[0].value\n\ndef inner_relay[*Ts: Movable & ImplicitlyDeletable](var *values: *Ts) -> Int:\n    return score(2, *values^)\n\ndef relay[*Ts: Movable & ImplicitlyDeletable](var *values: *Ts) -> Int:\n    return inner_relay(*values^)\n\ndef main():\n    print(relay(Item(40)))\n";
    assert_eq!(run(src).unwrap(), "drop 40\n42\n");
}

#[test]
fn whole_pack_forwarding_reaches_mir_as_one_tuple_move() {
    let src = "def sink[*Ts: Movable & ImplicitlyDeletable](var *values: *Ts) -> Int:\n    return len(values)\n\ndef relay[*Ts: Movable & ImplicitlyDeletable](var *values: *Ts) -> Int:\n    return sink(*values^)\n\ndef main():\n    print(relay(42))\n";
    let program = elaborate(parse(src).expect("parse")).expect("specialize");
    let checked = mojito::check_program(&program).expect("check");
    let mir = mojito::mir::lower_checked_program(&checked);
    let relay = mir
        .functions
        .iter()
        .find(|(name, _)| name.starts_with("relay$") && !name.ends_with("$whole_pack"))
        .map(|(_, function)| function)
        .expect("relay specialization");
    let instructions = relay
        .blocks
        .iter()
        .flat_map(|block| &block.instrs)
        .collect::<Vec<_>>();

    assert!(instructions.iter().any(|instruction| matches!(
        instruction,
        mojito::mir::MirInstr::UseVar {
            mode: mojito::mir::UseMode::Move,
            ..
        }
    )));
    assert!(
        instructions
            .iter()
            .all(|instruction| !matches!(instruction, mojito::mir::MirInstr::MovePlace { .. })),
        "forwarding must not synthesize indexed movable places"
    );
    assert!(instructions.iter().any(|instruction| matches!(
        instruction,
        mojito::mir::MirInstr::Call { func, args, .. }
            if func.0.ends_with("$whole_pack") && args.len() == 1
    )));
}

#[test]
fn nested_pack_forwarding_rejects_multiple_or_mixed_segments() {
    let multiple = "def outer() -> Int:\n    def count[*Ts: Movable & ImplicitlyDeletable](var *values: *Ts) -> Int:\n        return len(values)\n    def relay[*Ts: Movable & ImplicitlyDeletable](var *values: *Ts) -> Int:\n        return count(*values^, *values^)\n    return relay(1, True)\n\ndef main():\n    print(outer())\n";
    let mixed = "def outer() -> Int:\n    def count[*Ts: Movable & ImplicitlyDeletable](var *values: *Ts) -> Int:\n        return len(values)\n    def relay[*Ts: Movable & ImplicitlyDeletable](var *values: *Ts) -> Int:\n        return count(*values^, 9)\n    return relay(1, True)\n\ndef main():\n    print(outer())\n";
    let top_level_multiple = "def count[*Ts: Movable & ImplicitlyDeletable](var *values: *Ts) -> Int:\n    return len(values)\n\ndef relay[*Ts: Movable & ImplicitlyDeletable](var *values: *Ts) -> Int:\n    return count(*values^, *values^)\n\ndef main():\n    print(relay(1, True))\n";
    let top_level_mixed = "def count[*Ts: Movable & ImplicitlyDeletable](var *values: *Ts) -> Int:\n    return len(values)\n\ndef relay[*Ts: Movable & ImplicitlyDeletable](var *values: *Ts) -> Int:\n    return count(*values^, 9)\n\ndef main():\n    print(relay(1, True))\n";

    for source in [multiple, top_level_multiple] {
        let error = run(source).unwrap_err();
        assert!(
            error.contains("at most one runtime-pack spread"),
            "got: {error}"
        );
    }
    for source in [mixed, top_level_mixed] {
        let error = run(source).unwrap_err();
        assert!(
            error.contains("cannot be mixed with explicit overflow arguments"),
            "got: {error}"
        );
    }
}

#[test]
fn method_local_nested_pack_preserves_self_capture() {
    let src = "@fieldwise_init\nstruct Box:\n    var base: Int\n    def run(self) -> Int:\n        def count[*Ts: Movable & ImplicitlyDeletable](var *args: *Ts) unified {self} -> Int:\n            return self.base + len(args)\n        return count(1, True)\n\ndef main():\n    print(Box(40).run())\n";
    assert_eq!(run(src).unwrap(), "42\n");
}

#[test]
fn pack_forwarding_to_fixed_arity_remains_rejected() {
    let src = "def outer() -> Int:\n    def fixed(value: Int) -> Int:\n        return value\n    def relay[*Ts: Movable & ImplicitlyDeletable](var *args: *Ts) -> Int:\n        return fixed(*args^)\n    return relay(42)\n\ndef main():\n    print(outer())\n";
    let error = run(src).unwrap_err();
    assert!(
        error.contains("call spread outside a specialized type pack"),
        "got: {error}"
    );
}

#[test]
fn nested_pack_forwarding_preserves_nested_call_keywords_and_defaults() {
    let src = "def outer() -> Int:\n    def score[*Ts: Movable & ImplicitlyDeletable](head: Int, /, var *args: *Ts, scale: Int = 1) -> Int:\n        return (head + len(args)) * scale\n    return score[Int, Bool](10, 1, True, scale=3) + score[Int, Bool](10, 1, True)\n\ndef main():\n    print(outer())\n";
    assert_eq!(run(src).unwrap(), "48\n");
}

#[test]
fn heterogeneous_pack_inference_uses_only_variadic_overflow_arguments() {
    let top_level = "def count[*Ts: Movable & ImplicitlyDeletable](head: Int, var *args: *Ts) -> Int:\n    return head + len(args)\n\ndef main():\n    print(count(40, \"one\", True))\n";
    let nested = "def outer() -> Int:\n    def count[*Ts: Movable & ImplicitlyDeletable](head: Int, var *args: *Ts) -> Int:\n        return head + len(args)\n    return count(40, \"one\", True)\n\ndef main():\n    print(outer())\n";
    assert_eq!(run(top_level).unwrap(), "42\n");
    assert_eq!(run(nested).unwrap(), "42\n");
}

#[test]
fn nested_pack_named_result_is_not_part_of_the_call_abi() {
    let src = "def outer() -> Int:\n    def count[*Ts: Movable & ImplicitlyDeletable](out result: Int, var *args: *Ts):\n        result = len(args)\n    return count[Int, Bool](1, True)\n\ndef main():\n    print(outer())\n";
    assert_eq!(run(src).unwrap(), "2\n");
}

#[test]
fn nested_whole_pack_forwarding_can_chain_without_copying() {
    let src = "struct Item(Movable):\n    var value: Int\n    def __init__(out self, value: Int):\n        self.value = value\n    def __init__(out self, *, deinit move: Self):\n        self.value = move.value\n\ndef outer() -> Int:\n    def first[*Ts: Movable & ImplicitlyDeletable](var *args: *Ts) -> Int:\n        return args[0].value\n    def second[*Ts: Movable & ImplicitlyDeletable](var *args: *Ts) -> Int:\n        return first(*args^)\n    def third[*Ts: Movable & ImplicitlyDeletable](var *args: *Ts) -> Int:\n        return second(*args^)\n    return third(Item(42))\n\ndef main():\n    print(outer())\n";
    assert_eq!(run(src).unwrap(), "42\n");
}

#[test]
fn walrus_bindings_shadow_pack_templates_for_the_whole_function() {
    let top_level = "def choose[*Ts: Movable & ImplicitlyDeletable](var *args: *Ts) -> Int:\n    return len(args)\n\ndef main():\n    if True:\n        var ignored = (choose := 5)\n    print(choose(2))\n";
    let nested = "def outer():\n    def choose[*Ts: Movable & ImplicitlyDeletable](var *args: *Ts) -> Int:\n        return len(args)\n    if True:\n        var ignored = (choose := 5)\n    print(choose(2))\n\ndef main():\n    outer()\n";
    for source in [top_level, nested] {
        let error = run(source).unwrap_err();
        assert!(
            error.contains("'choose' has type Int and is not callable"),
            "got: {error}"
        );
    }
}

#[test]
fn specialization_materializes_runtime_defaults_that_use_value_parameters() {
    let top_level = "def choose[n: Int](value: Int = n) -> Int:\n    comptime if n >= 0:\n        pass\n    return value\n\ndef main():\n    print(choose[42]())\n";
    let nested = "def outer() -> Int:\n    def choose[n: Int](value: Int = n) -> Int:\n        comptime if n >= 0:\n            pass\n        return value\n    return choose[42]()\n\ndef main():\n    print(outer())\n";
    assert_eq!(run(top_level).unwrap(), "42\n");
    assert_eq!(run(nested).unwrap(), "42\n");
}

#[test]
fn nested_pack_identity_includes_the_outer_specialization() {
    let src = "def outer[n: Int]() -> Int:\n    comptime if n >= 0:\n        pass\n    def nested[*InnerTypes: Movable & ImplicitlyDeletable](var *inner_args: *InnerTypes) -> Int:\n        return n * 10 + len(inner_args)\n    return nested(1, \"two\", True)\n\ndef main():\n    print(outer[2]())\n    print(outer[1]())\n";
    assert_eq!(run(src).unwrap(), "23\n13\n");
}

#[test]
fn nested_pack_specialization_preserves_explicit_captures() {
    let src = "def outer() -> Int:\n    var base = 40\n    def nested[*Ts: Movable & ImplicitlyDeletable](var *args: *Ts) unified {base} -> Int:\n        return base + len(args)\n    return nested(1, True)\n\ndef main():\n    print(outer())\n";
    assert_eq!(run(src).unwrap(), "42\n");
}

#[test]
fn same_spelled_nested_pack_templates_have_distinct_lexical_identities() {
    let src = "def outer():\n    if True:\n        def count[*Ts: Movable & ImplicitlyDeletable](var *args: *Ts) -> Int:\n            return len(args)\n        print(count(1))\n    if True:\n        def count[*Ts: Movable & ImplicitlyDeletable](var *args: *Ts) -> Int:\n            return 40 + len(args)\n        print(count(1, True))\n\ndef main():\n    outer()\n";
    assert_eq!(run(src).unwrap(), "1\n42\n");
}

#[test]
fn local_callable_shadows_a_top_level_pack_template_during_specialization() {
    let src = "def choose[*Ts: Movable & ImplicitlyDeletable](var *args: *Ts) -> Int:\n    return len(args)\n\ndef main():\n    def choose(value: Int) -> Int:\n        return value + 40\n    print(choose(2))\n";
    assert_eq!(run(src).unwrap(), "42\n");
}

#[test]
fn comptime_for_over_a_list_and_string_concat() {
    // A compile-time list of ints, and compile-time string concatenation (used to
    // pick a branch, so the concatenated value is consumed at compile time).
    let src = "comptime sizes = [1, 2, 4, 8]\n\ndef main():\n    comptime for n in sizes:\n        print(n)\n    comptime if \"a\" + \"b\" == \"ab\":\n        print(\"concat-ok\")\n";
    assert_eq!(run(src).unwrap(), "1\n2\n4\n8\nconcat-ok\n");
}

#[test]
fn comptime_for_enables_compile_time_tuple_indexing() {
    // Substituting the loop var with a literal makes `t[i]` a compile-time-constant
    // index — so a heterogeneous tuple can be walked (a runtime `for` can't).
    let src = "def main():\n    var t: Tuple[Int, String, Bool] = (1, \"two\", True)\n    comptime for i in range(3):\n        print(t[i])\n";
    assert_eq!(run(src).unwrap(), "1\ntwo\nTrue\n");
}

#[test]
fn non_comptime_binding_is_rejected_by_elaboration() {
    // `comptime NAME = <runtime value>` is rejected at compile-time elaboration.
    let program = parse("var x: Int = 3\ncomptime N = x\n").unwrap();
    assert!(elaborate(program).is_err());
}

#[test]
fn ctfe_runs_a_pure_function_at_compile_time() {
    // A pure top-level function (loops + locals) executes at compile time.
    let src = "def next_pow2(n: Int) -> Int:\n    var p: Int = 1\n    while p < n:\n        p = p * 2\n    return p\n\ncomptime CAP = next_pow2(17)\n\ndef main():\n    comptime for i in range(CAP):\n        pass\n    print(CAP)\n";
    assert_eq!(run(src).unwrap(), "32\n");
}

#[test]
fn ctfe_supports_recursion() {
    let src = "def fact(n: Int) -> Int:\n    if n <= 1:\n        return 1\n    return n * fact(n - 1)\n\ncomptime F = fact(5)\n\ndef main():\n    print(F)\n";
    assert_eq!(run(src).unwrap(), "120\n");
}

#[test]
fn ctfe_is_fuel_bounded() {
    let err = run("def spin(n: Int) -> Int:\n    var i = n\n    while True:\n        i = i + 1\n    return i\ncomptime X = spin(0)\n\ndef main():\n    print(X)\n").unwrap_err();
    assert!(err.contains("quota"), "got {err}");
}

#[test]
fn module_comptime_constants_materialize_into_functions() {
    // A top-level comptime constant is usable inside a function (materialized as a
    // literal, closing the module-global-in-function gap): as a value returned from
    // a function, and as a value-parameter argument (`Box[N]`).
    let src = "comptime GREETING = \"hi\"\ncomptime N = 8\n\ndef greet() -> String:\n    return GREETING\n\n@fieldwise_init\nstruct Box[size: Int]:\n    var v: Int\n    def cap(self) -> Int:\n        return Self.size\n\ndef main():\n    print(greet())\n    var b: Box[N] = Box[N](0)\n    print(b.cap())\n";
    assert_eq!(run(src).unwrap(), "hi\n8\n");
}

#[test]
fn ctfe_computed_value_parameter_argument() {
    // Phase 1 regression (docs/notes/comptime.md): a CTFE-computed comptime constant flows
    // into a value-parameter argument through the shared compile-time value model —
    // `pow2(3)` runs at compile time to `8`, materializes into `scale[N]`, and the
    // checker resolves `scale`'s value parameter `n` from it.
    let src = "def scale[n: Int](x: Int) -> Int:\n    return x * n\n\ndef pow2(k: Int) -> Int:\n    var x: Int = 1\n    for i in range(k):\n        x = x * 2\n    return x\n\ncomptime N = pow2(3)\n\ndef main():\n    print(scale[N](5))\n";
    assert_eq!(run(src).unwrap(), "40\n");
}

#[test]
fn generic_value_param_comptime_if_selects_per_instantiation() {
    // Phase 6 (docs/notes/comptime.md): `comptime if` inside a generic value-parameter `def`
    // is resolved per call — `f[0]` takes the `if` branch, `f[1]` the `else`. This
    // needs monomorphization: the template is specialized after its argument known.
    let src = "def f[n: Int]() -> Int:\n    comptime if n == 0:\n        return 10\n    else:\n        return 20\n\ndef main():\n    print(f[0](), f[1]())\n";
    assert_eq!(run(src).unwrap(), "10 20\n");
}

#[test]
fn string_value_parameter_specializes_and_materializes() {
    let src = "def label[text: String]() -> String:\n    comptime if text == \"short\":\n        return text + \"!\"\n    else:\n        return \"other\"\n\ndef main():\n    print(label[\"short\"]())\n    print(label[\"long\"]())\n";
    assert_eq!(run(src).unwrap(), "short!\nother\n");
}

#[test]
fn specialization_uses_defaulted_compile_time_value_parameter() {
    let src = "def width[n: Int = 4]() -> Int:\n    comptime if n == 4:\n        return n\n    else:\n        return 0\n\ndef main():\n    print(width())\n    print(width[8]())\n";
    assert_eq!(run(src).unwrap(), "4\n0\n");
}

#[test]
fn specialization_evaluates_dependent_parameter_defaults() {
    let src = "def columns[rows: Int, count: Int = rows + 1]() -> Int:\n    comptime if count > rows:\n        return count\n    else:\n        return 0\n\ndef main():\n    print(columns[3]())\n";
    assert_eq!(run(src).unwrap(), "4\n");
}

#[test]
fn unified_reflection_handle_exposes_struct_field_facts() {
    let src = "@fieldwise_init\nstruct Point:\n    var x: Int\n    var label: String\n\ndef main():\n    comptime r = reflect[Point]\n    comptime count = r.field_count()\n    comptime names = r.field_names()\n    comptime types = r.field_types()\n    print(count, names[0], names[1])\n    comptime if is_same_type[types[0], Int]():\n        print(\"int\")\n";
    assert_eq!(run(src).unwrap(), "2 x label\nint\n");
}

#[test]
fn reflection_supports_named_indexed_and_chainable_field_handles() {
    let src = "struct Coordinates:\n    var x: Int\n    var y: Float64\n\nstruct Point:\n    var coordinates: Coordinates\n\ndef main():\n    comptime r = reflect[Point]\n    comptime index = r.field_index[\"coordinates\"]()\n    comptime reflected = r.field[\"coordinates\"].field_at[1]\n    var value: reflected.T = 3.5\n    print(index, value)\n";
    assert_eq!(run(src).unwrap(), "0 3.5\n");
}

#[test]
fn reflection_field_handles_substitute_generic_struct_arguments() {
    let src = "@fieldwise_init\nstruct Boxed[T: Copyable & Movable]:\n    var value: Self.T\n\ndef main():\n    comptime reflected = reflect[Boxed[String]].field_at[0]\n    var value: reflected.T = \"generic\"\n    print(value)\n";
    assert_eq!(run(src).unwrap(), "generic\n");
}

#[test]
fn reflection_rejects_removed_field_type_spelling() {
    let error = run("struct Point:\n    var x: Int\n\ndef main():\n    comptime reflected = reflect[Point].field_type[\"x\"]()\n")
        .unwrap_err();
    assert!(
        error.contains("field_type was removed") && error.contains("field[name]"),
        "got {error}"
    );
}

#[test]
fn reflection_rejects_invalid_named_and_indexed_field_selection() {
    let missing = run("struct Point:\n    var x: Int\n\ndef main():\n    comptime reflected = reflect[Point].field[\"missing\"]\n")
        .unwrap_err();
    assert!(
        missing.contains("has no field named 'missing'"),
        "got {missing}"
    );

    let out_of_range = run("struct Point:\n    var x: Int\n\ndef main():\n    comptime reflected = reflect[Point].field_at[1]\n")
        .unwrap_err();
    assert!(
        out_of_range.contains("field index 1 is out of range"),
        "got {out_of_range}"
    );
}

#[test]
fn reflection_can_conditionally_generate_a_declaration() {
    let src = "struct Unit:\n    var value: Int\n\ncomptime reflected = reflect[Unit]\ncomptime if reflected.field_count() == 1:\n    def generated() -> String:\n        return \"generated\"\nelse:\n    def generated() -> String:\n        return \"wrong\"\n\ndef main():\n    print(generated())\n";
    assert_eq!(run(src).unwrap(), "generated\n");
}

#[test]
fn string_value_parameter_rejects_a_value_of_the_wrong_type() {
    let src = "def label[text: String]() -> String:\n    return text\n\ndef main():\n    print(label[1]())\n";
    let error = run(src).unwrap_err();
    assert!(
        error.contains("expected String") && error.contains("found Int"),
        "got {error}"
    );
}

#[test]
fn dropped_comptime_if_branch_is_not_checked() {
    // The `else` branch returns a `String` from an `-> Int` function — a type error
    // — but only `f[0]` is instantiated, which selects the `if` branch, so the bad
    // branch is dropped before checking and the program is accepted.
    let src = "def f[n: Int]() -> Int:\n    comptime if n == 0:\n        return 1\n    else:\n        return \"bad\"\n\ndef main():\n    print(f[0]())\n";
    assert_eq!(run(src).unwrap(), "1\n");
}

#[test]
fn instantiated_comptime_if_branch_is_checked() {
    // Instantiating `f[1]` selects the bad `else` branch, so its type error surfaces.
    let src = "def f[n: Int]() -> Int:\n    comptime if n == 0:\n        return 1\n    else:\n        return \"bad\"\n\ndef main():\n    print(f[1]())\n";
    let err = run(src).unwrap_err();
    assert!(err.contains("expected Int, found String"), "got {err}");
}

#[test]
fn generic_comptime_specialization_recurses_and_unrolls() {
    // A specialized body can request further specializations: `sumto[n]` recurses to
    // `sumto[n - 1]` (each a distinct instantiation), and `comptime for` unrolls
    // against the value parameter. sumto[4] = 4+3+2+1+0 = 10; repeat[5] = 0..4 = 10.
    let src = "def sumto[n: Int]() -> Int:\n    comptime if n == 0:\n        return 0\n    else:\n        return n + sumto[n - 1]()\n\ndef repeat[k: Int]() -> Int:\n    var total: Int = 0\n    comptime for i in range(k):\n        total = total + i\n    return total\n\ndef main():\n    print(sumto[4]())\n    print(repeat[5]())\n";
    assert_eq!(run(src).unwrap(), "10\n10\n");
}

#[test]
fn heterogeneous_pack_length_drives_comptime_iteration() {
    let src = "def sum_values[*ArgTypes: Intable](*args: *ArgTypes) -> Int:\n    var total: Int = 0\n    comptime for i in range(args.__len__()):\n        total = total + Int(args[i])\n    return total\n\ndef main():\n    print(sum_values(1, True, 2.0))\n";
    assert_eq!(run(src).unwrap(), "4\n");
}

#[test]
fn heterogeneous_pack_bound_failure_names_the_call_element() {
    let src = "def count[*ArgTypes: Intable](*args: *ArgTypes) -> Int:\n    return len(args)\n\ndef main():\n    print(count(1, \"two\", True))\n";
    let error = run(src).unwrap_err();
    assert!(
        error.contains("type-pack bound failed at 'count' instantiation"),
        "got: {error}"
    );
    assert!(
        error.contains("element 2 of type pack 'ArgTypes' has type 'String'"),
        "got: {error}"
    );
    assert!(error.contains("'Intable'"), "got: {error}");
}

#[test]
fn heterogeneous_pack_bound_oracle_uses_nominal_user_conformance() {
    let declarations = "trait Valued:\n    def value(self) -> Int: ...\n\n@fieldwise_init\nstruct Number(Valued):\n    var data: Int\n\n    def value(self) -> Int:\n        return self.data\n\n@fieldwise_init\nstruct Opaque:\n    var data: Int\n\ndef count[*Types: Valued](*args: *Types) -> Int:\n    return len(args)\n\n";
    let accepted = format!("{declarations}def main():\n    print(count(Number(1), Number(2)))\n");
    assert_eq!(run(&accepted).unwrap(), "2\n");

    let rejected = format!("{declarations}def main():\n    print(count(Number(1), Opaque(2)))\n");
    let error = run(&rejected).unwrap_err();
    assert!(
        error.contains("element 2 of type pack 'Types' has type 'Opaque'"),
        "got: {error}"
    );
    assert!(error.contains("'Valued'"), "got: {error}");
}

#[test]
fn heterogeneous_pack_indexes_expose_concrete_element_types() {
    let src = "def first_plus_one[*Types: Copyable](*args: *Types) -> Int:\n    comptime if is_same_type[Types[0], Int]():\n        return args[0] + 1\n    else:\n        return 0\n\ndef main():\n    print(first_plus_one(4, \"tail\"))\n    print(first_plus_one(\"head\", 4))\n";
    assert_eq!(run(src).unwrap(), "5\n0\n");
}

#[test]
fn variadic_value_pack_specializes_and_unrolls() {
    let src = "def total[*values: Int]() -> Int:\n    var result = 0\n    comptime for value in values:\n        result = result + value\n    return result\n\ndef main():\n    print(total[1, 2, 3, 4]())\n";
    assert_eq!(run(src).unwrap(), "10\n");
}

#[test]
fn type_predicate_selects_comptime_branch() {
    // Phase 7 (docs/notes/comptime.md): the built-in `is_same_type[T, U]()` type predicate lets
    // a `comptime if` branch on a type parameter — `name[Int]` takes the `int`
    // branch, `name[String]` the `other` branch (each a distinct specialization).
    let src = "def name[T: AnyType]() -> String:\n    comptime if is_same_type[T, Int]():\n        return \"int\"\n    else:\n        return \"other\"\n\ndef main():\n    print(name[Int]())\n    print(name[String]())\n";
    assert_eq!(run(src).unwrap(), "int\nother\n");
}

#[test]
fn type_predicate_in_runtime_if_is_rejected() {
    // A type predicate has no runtime `Bool` form — used in a runtime `if` (not a
    // `comptime if`) it is not a resolvable value, so the program is rejected.
    let src = "def name[T: AnyType]() -> String:\n    if is_same_type[T, Int]():\n        return \"int\"\n    else:\n        return \"other\"\n\ndef main():\n    print(name[Int]())\n";
    assert!(run(src).is_err());
}

#[test]
fn type_and_value_predicates_compose() {
    // A mixed type+value generic: the type predicate picks the outer branch and the
    // value-parameter predicate the inner one, each resolved per instantiation.
    let src = "def tag[T: AnyType, n: Int]() -> String:\n    comptime if is_same_type[T, Int]():\n        comptime if n == 0:\n            return \"int-zero\"\n        else:\n            return \"int-n\"\n    else:\n        return \"other\"\n\ndef main():\n    print(tag[Int, 0]())\n    print(tag[Int, 5]())\n    print(tag[String, 0]())\n";
    assert_eq!(run(src).unwrap(), "int-zero\nint-n\nother\n");
}

#[test]
fn specialization_retains_thin_callable_value_arguments() {
    let src = "def increment(value: Int) -> Int:\n    return value + 1\n\ndef select[enabled: Bool, callback: def(Int) thin -> Int](value: Int) -> Int:\n    comptime if enabled:\n        return callback(value)\n    else:\n        return value\n\ndef main():\n    print(select[True, increment](41))\n";
    assert_eq!(run(src).unwrap(), "42\n");
}

#[test]
fn specialization_retains_defaulted_thin_callable_arguments() {
    let src = "def increment(value: Int) -> Int:\n    return value + 1\n\ndef select[enabled: Bool, callback: def(Int) thin -> Int = increment](value: Int) -> Int:\n    comptime if enabled:\n        return callback(value)\n    else:\n        return value\n\ndef main():\n    print(select[True](41))\n";
    assert_eq!(run(src).unwrap(), "42\n");
}

#[test]
fn specialization_retains_capturing_callable_and_inferred_origin_set() {
    let src = "def select[origins: OriginSet, //, enabled: Bool, callback: def(Int) capturing[origins] -> Int](value: Int) -> Int:\n    comptime if enabled:\n        return callback(value)\n    else:\n        return value\n\ndef main():\n    var offset = 1\n    @parameter\n    def add(value: Int) -> Int:\n        return value + offset\n    print(select[True, add](41))\n";
    assert_eq!(run(src).unwrap(), "42\n");
}

#[test]
fn nested_specialization_retains_capturing_callable_arguments() {
    let src = "def main():\n    var offset = 1\n    @parameter\n    def add(value: Int) -> Int:\n        return value + offset\n    def select[origins: OriginSet, //, enabled: Bool, callback: def(Int) capturing[origins] -> Int](value: Int) -> Int:\n        comptime if enabled:\n            return callback(value)\n        else:\n            return value\n    print(select[True, add](41))\n";
    assert_eq!(run(src).unwrap(), "42\n");
}

#[test]
fn type_pack_specialization_accepts_explicit_origin_before_pack() {
    let src = "def choose[origin: Origin[mut=True], *Ts: Copyable & ImplicitlyDeletable](ref[origin] value: Int, var *args: *Ts) -> ref[origin] Int:\n    comptime for i in range(args.__len__()):\n        pass\n    return value\n\ndef main():\n    var value = 40\n    ref result = choose[origin_of(value), Int, Bool](value, 1, True)\n    result += 2\n    print(value)\n";
    assert_eq!(run(src).unwrap(), "42\n");
}

#[test]
fn type_pack_specialization_accepts_named_origin_after_pack() {
    let src = "def choose[*Ts: Copyable & ImplicitlyDeletable, origin: Origin[mut=True]](ref[origin] value: Int, var *args: *Ts) -> ref[origin] Int:\n    comptime for i in range(args.__len__()):\n        pass\n    return value\n\ndef main():\n    var value = 40\n    ref result = choose[Int, Bool, origin=origin_of(value)](value, 1, True)\n    result += 2\n    print(value)\n";
    assert_eq!(run(src).unwrap(), "42\n");
}

#[test]
fn type_pack_specialization_skips_infer_only_origin_before_pack() {
    let src = "def choose[origin: Origin[mut=True], //, *Ts: Copyable & ImplicitlyDeletable](ref[origin] value: Int, var *args: *Ts) -> ref[origin] Int:\n    comptime for i in range(args.__len__()):\n        pass\n    return value\n\ndef main():\n    var value = 40\n    ref result = choose[Int, Bool](value, 1, True)\n    result += 2\n    print(value)\n";
    assert_eq!(run(src).unwrap(), "42\n");
}

#[test]
fn value_pack_specialization_accepts_named_origin_after_pack() {
    let src = "def add_all[*values: Int, origin: Origin[mut=True]](ref[origin] result: Int):\n    comptime for value in values:\n        result += value\n\ndef main():\n    var result = 40\n    add_all[1, 1, origin=origin_of(result)](result)\n    print(result)\n";
    assert_eq!(run(src).unwrap(), "42\n");
}

// --- Variadic-generic structs (`struct S[*Ts: Bound]`) ----------------------
//
// Compile-time elaboration specializes a variadic struct template per
// instantiation (mirroring pack functions): `Tuple[*Ts]` members expand to the
// concrete element list, and the template itself is dropped.

#[test]
fn variadic_struct_specializes_with_per_index_typed_storage() {
    // `p.storage[0]` has the exact element type (Int here), so it participates
    // in Int arithmetic; `p.storage[1]` is exactly Bool.
    let src = "@fieldwise_init\nstruct Pair[*Ts: Copyable & Movable](Copyable, Movable):\n    var storage: Tuple[*Ts]\n\ndef main():\n    var p = Pair[Int, Bool]((1, True))\n    var n: Int = p.storage[0] + 41\n    var b: Bool = p.storage[1]\n    print(n)\n    print(b)\n";
    assert_eq!(run(src).unwrap(), "42\nTrue\n");
}

#[test]
fn variadic_struct_element_type_mismatch_is_rejected() {
    // Per-index typing is exact: reading the Int element into a Bool is a type
    // error, not a common-bound erasure.
    let src = "@fieldwise_init\nstruct Pair[*Ts: Copyable & Movable](Copyable, Movable):\n    var storage: Tuple[*Ts]\n\ndef main():\n    var p = Pair[Int, Bool]((1, True))\n    var b: Bool = p.storage[0]\n    print(b)\n";
    let err = run(src).unwrap_err();
    assert!(err.contains("expected Bool, found Int"), "got: {err}");
}

#[test]
fn variadic_struct_distinct_instantiations_coexist() {
    // Two specializations of one template are distinct concrete structs with
    // independent field types (regression: annotation sites keyed by span
    // collided across specializations sharing the template's span).
    let src = "@fieldwise_init\nstruct Pair[*Ts: Copyable & Movable](Copyable, Movable):\n    var storage: Tuple[*Ts]\n\ndef main():\n    var a = Pair[Int, Bool]((1, True))\n    var b = Pair[Int, Int]((2, 3))\n    var c = Pair[String]((\"solo\",))\n    print(a.storage[0] + b.storage[1])\n    print(c.storage[0])\n";
    assert_eq!(run(src).unwrap(), "4\nsolo\n");
}

#[test]
fn variadic_struct_annotations_and_methods_use_the_specialization() {
    // The struct type appears in a def parameter annotation (rewritten to the
    // specialized struct), and a concrete method runs against the expanded
    // storage.
    let src = "@fieldwise_init\nstruct Pair[*Ts: Copyable & Movable](Copyable, Movable):\n    var storage: Tuple[*Ts]\n\n    def size(self) -> Int:\n        return len(self.storage)\n\ndef first_int(p: Pair[Int, Bool]) -> Int:\n    return p.storage[0]\n\ndef main():\n    var p: Pair[Int, Bool] = Pair[Int, Bool]((1, True))\n    var q = p\n    print(first_int(q))\n    print(q.size())\n";
    assert_eq!(run(src).unwrap(), "1\n2\n");
}

#[test]
fn variadic_struct_requires_explicit_type_arguments() {
    let src = "@fieldwise_init\nstruct Pair[*Ts: Copyable & Movable](Copyable, Movable):\n    var storage: Tuple[*Ts]\n\ndef main():\n    var p = Pair((1, True))\n    print(p.storage[0])\n";
    let err = run(src).unwrap_err();
    assert!(
        err.contains("variadic struct 'Pair' requires explicit compile-time type arguments"),
        "got: {err}"
    );
}

#[test]
fn variadic_struct_bare_template_use_is_rejected() {
    let src = "@fieldwise_init\nstruct Pair[*Ts: Copyable & Movable](Copyable, Movable):\n    var storage: Tuple[*Ts]\n\ndef main():\n    var x = Pair\n    print(\"unreachable\")\n";
    let err = run(src).unwrap_err();
    assert!(
        err.contains("variadic struct 'Pair' requires explicit compile-time type arguments"),
        "got: {err}"
    );
}

#[test]
fn variadic_struct_supports_exactly_one_pack() {
    // One trailing pack and no other compile-time parameters (current scope).
    let src = "@fieldwise_init\nstruct Bad[T: Copyable & Movable, *Ts: Copyable & Movable](Copyable, Movable):\n    var storage: Tuple[*Ts]\n\ndef main():\n    var x = Bad[Int, Bool]((True,))\n    print(\"unreachable\")\n";
    let err = run(src).unwrap_err();
    assert!(
        err.contains("supports exactly one type-parameter pack"),
        "got: {err}"
    );
}

#[test]
fn variadic_struct_runtime_index_is_rejected() {
    let src = "@fieldwise_init\nstruct Pair[*Ts: Copyable & Movable](Copyable, Movable):\n    var storage: Tuple[*Ts]\n\ndef main():\n    var p = Pair[Int, Bool]((1, True))\n    var i = 0\n    print(p.storage[i])\n";
    let err = run(src).unwrap_err();
    assert!(err.contains("compile-time Int index"), "got: {err}");
}

#[test]
fn variadic_struct_pack_init_constructs_per_position() {
    // Real Mojo's Tuple constructor shape: `var *args: *Ts` binds the
    // heterogeneous pack (each argument checked against its per-index element
    // type) and `Tuple(*args^)` transfers the elements into storage.
    let src = "struct Pair[*Ts: Copyable & Movable](Copyable, Movable):\n    var storage: Tuple[*Ts]\n\n    def __init__(out self, var *args: *Ts):\n        self.storage = Tuple(*args^)\n\n    def size(self) -> Int:\n        return len(self.storage)\n\ndef main():\n    var p = Pair[Int, String, Bool](7, \"x\", False)\n    print(p.size())\n    print(p.storage[0])\n    print(p.storage[1])\n    print(p.storage[2])\n";
    assert_eq!(run(src).unwrap(), "3\n7\nx\nFalse\n");
}

#[test]
fn variadic_struct_pack_init_rejects_wrong_arity_and_types() {
    let template = "struct Pair[*Ts: Copyable & Movable](Copyable, Movable):\n    var storage: Tuple[*Ts]\n\n    def __init__(out self, var *args: *Ts):\n        self.storage = Tuple(*args^)\n\n";
    // Too few arguments for the pack.
    let arity = format!(
        "{template}def main():\n    var p = Pair[Int, String](1)\n    print(p.storage[0])\n"
    );
    let err = run(&arity).unwrap_err();
    assert!(
        err.contains("no constructor overload matches"),
        "got: {err}"
    );
    // A per-position element type mismatch (Bool where Int is declared).
    let mistyped = format!(
        "{template}def main():\n    var p = Pair[Int, String](True, \"hi\")\n    print(p.storage[0])\n"
    );
    let err = run(&mistyped).unwrap_err();
    assert!(
        err.contains("no constructor overload matches"),
        "got: {err}"
    );
}

#[test]
fn variadic_struct_dependent_getitem_unrolls_per_element() {
    // Real Mojo's dependent accessor `def __getitem__[i: Int](self) -> Ts[i]`
    // unrolls into one concrete accessor per pack element at specialization;
    // `p[k]` requires a compile-time-constant index, has the exact element
    // type, and dispatches the checker-resolved accessor.
    let src = "struct Pair[*Ts: Copyable & Movable](Copyable, Movable):\n    var storage: Tuple[*Ts]\n\n    def __init__(out self, var *args: *Ts):\n        self.storage = Tuple(*args^)\n\n    def __getitem__[i: Int](self) -> Ts[i]:\n        return self.storage[i]\n\n    def __len__(self) -> Int:\n        return len(self.storage)\n\ndef main():\n    var p = Pair[Int, String, Bool](7, \"mid\", True)\n    var n: Int = p[0]\n    var s: String = p[1]\n    var b: Bool = p[2]\n    print(n)\n    print(s)\n    print(b)\n    print(len(p))\n";
    assert_eq!(run_compiled(src).unwrap(), "7\nmid\nTrue\n3\n");
}

#[test]
fn current_getitem_param_hook_handles_places_and_rvalues() {
    // Current Mojo spells a compile-time parameter subscript hook
    // `__getitem_param__`. A place preserves its reference result, while an
    // implicitly-copyable rvalue uses the generated value-returning twin.
    let src = "struct CurrentPair[*Ts: ImplicitlyCopyable & Copyable & Movable](Copyable, Movable):\n    var storage: Tuple[*Ts]\n\n    def __init__(out self, var *args: *Ts):\n        self.storage = Tuple(*args^)\n\n    def __getitem_param__[i: Int](ref self) -> ref[origin_of(self)] Ts[i]:\n        return self.storage[i]\n\ndef main():\n    var pair = CurrentPair[Int, String](7, \"current\")\n    print(pair[0], pair[1])\n    print(CurrentPair[Int, String](9, \"rvalue\")[0])\n";
    assert_eq!(run_compiled(src).unwrap(), "7 current\n9\n");
}

#[test]
fn current_getitem_param_reference_result_can_bind_an_explicit_ref() {
    let src = "struct CurrentPair[*Ts: ImplicitlyCopyable & Copyable & Movable](Copyable, Movable):\n    var storage: Tuple[*Ts]\n\n    def __init__(out self, var *args: *Ts):\n        self.storage = Tuple(*args^)\n\n    def __getitem_param__[i: Int](ref self) -> ref[origin_of(self)] Ts[i]:\n        return self.storage[i]\n\ndef main():\n    var pair = CurrentPair[Int, String](7, \"current\")\n    ref alias = pair[0]\n    alias += 5\n    print(alias)\n    print(pair[0])\n";
    assert_eq!(run_compiled(src).unwrap(), "12\n12\n");
}

#[test]
fn general_getitem_param_hook_uses_a_checked_value_parameter() {
    let source = include_str!("../conformance/fixtures/current_parameter_indexing.mojo");
    assert_eq!(run_compiled(source).unwrap(), "7 8\n9\n");
}

#[test]
fn variadic_struct_dependent_getitem_dispatches_per_instantiation() {
    // Two specializations resolve their own accessor families independently.
    let src = "struct Pair[*Ts: Copyable & Movable](Copyable, Movable):\n    var storage: Tuple[*Ts]\n\n    def __init__(out self, var *args: *Ts):\n        self.storage = Tuple(*args^)\n\n    def __getitem__[i: Int](self) -> Ts[i]:\n        return self.storage[i]\n\ndef main():\n    var a = Pair[Int, Bool](1, True)\n    var b = Pair[String, Int](\"s\", 5)\n    print(a[0] + b[1])\n    print(b[0])\n";
    assert_eq!(run_compiled(src).unwrap(), "6\ns\n");
}

#[test]
fn variadic_struct_dependent_getitem_rejects_bad_indices() {
    let template = "struct Pair[*Ts: Copyable & Movable](Copyable, Movable):\n    var storage: Tuple[*Ts]\n\n    def __init__(out self, var *args: *Ts):\n        self.storage = Tuple(*args^)\n\n    def __getitem__[i: Int](self) -> Ts[i]:\n        return self.storage[i]\n\n";
    // A runtime-varying index cannot select among heterogeneous elements.
    let runtime = format!(
        "{template}def main():\n    var p = Pair[Int, Bool](1, True)\n    var i = 0\n    print(p[i])\n"
    );
    let err = run(&runtime).unwrap_err();
    assert!(err.contains("compile-time Int index"), "got: {err}");
    // A constant index outside the pack.
    let range =
        format!("{template}def main():\n    var p = Pair[Int, Bool](1, True)\n    print(p[5])\n");
    let err = run(&range).unwrap_err();
    assert!(err.contains("pack index in 0..2"), "got: {err}");
    // No `__setitem__`: element writes are rejected (immutability preserved).
    let write = format!(
        "{template}def main():\n    var p = Pair[Int, Bool](1, True)\n    p[0] = 9\n    print(p[0])\n"
    );
    let err = run(&write).unwrap_err();
    assert!(err.contains("cannot be indexed here"), "got: {err}");
}

#[test]
fn variadic_struct_bound_violation_rejects_via_spec_conformance() {
    // A pack element that breaks the struct's own conformance surface
    // (non-Copyable element inside a Copyable struct) is rejected when the
    // specialization's declared conformances are verified. Def-pack bounds are
    // diagnosed independently at their requesting call, before specialization.
    let src = "struct NoCopy(Movable):\n    var x: Int\n\n    def __init__(out self, x: Int):\n        self.x = x\n\nstruct Pair[*Ts: Copyable & Movable](Copyable, Movable):\n    var storage: Tuple[*Ts]\n\n    def __init__(out self, var *args: *Ts):\n        self.storage = Tuple(*args^)\n\ndef main():\n    var p = Pair[NoCopy, Int](NoCopy(1), 2)\n    print(p.storage[1])\n";
    let err = run(src).unwrap_err();
    assert!(err.contains("not Copyable"), "got: {err}");
}

#[test]
fn associated_type_facts_request_nested_variadic_struct_specializations() {
    let src = "@fieldwise_init\nstruct Nested[*Ts: Movable](Movable):\n    pass\n\n@fieldwise_init\nstruct Family[*Ts: Movable](Movable):\n    comptime NestedType = Nested[*Ts]\n    var marker: Int\n\ndef main():\n    var value = Family[Int, Bool](42)\n    print(value.marker)\n";
    assert_eq!(run(src).unwrap(), "42\n");
}

#[test]
fn explicit_type_argument_naming_a_non_generic_struct_specializes() {
    // Regression: the retained `[Plain]` argument on the rewritten call used to
    // be misresolved by the checker as an undefined value. A concrete type
    // argument is now baked into the clone and dropped from the call.
    let src = "@fieldwise_init\nstruct Plain(Copyable, Movable):\n    var n: Int\n\ndef pick[T: Movable](x: T) -> Int:\n    comptime if is_same_type[T, Plain]():\n        return 1\n    else:\n        return 0\n\ndef main():\n    print(pick[Plain](Plain(3)))\n    print(pick[Int](4))\n";
    assert_eq!(run(src).unwrap(), "1\n0\n");
}

#[test]
fn explicit_type_argument_bound_violation_is_reported_at_the_call() {
    // A dropped type argument is never re-validated by the checker against the
    // residual signature, so the elaborator enforces the parameter's trait
    // bounds when the instantiation is requested.
    let src = "struct Pinned:\n    var n: Int\n    def __init__(out self, n: Int):\n        self.n = n\n\ndef pick[T: Copyable](x: T) -> Int:\n    comptime if is_same_type[T, Int]():\n        return 1\n    else:\n        return 0\n\ndef main():\n    print(pick[Pinned](Pinned(3)))\n";
    let error = run(src).unwrap_err();
    assert!(
        error.contains("generic bound failed at 'pick' instantiation"),
        "got: {error}"
    );
    assert!(
        error.contains("type parameter 'T' received type 'Pinned'"),
        "got: {error}"
    );
    assert!(error.contains("'Copyable'"), "got: {error}");
}

#[test]
fn bound_generic_clone_reports_concrete_body_errors() {
    // A plain trait-bound generic (no comptime constructs) monomorphizes per
    // explicit concrete application, so a body-invalid instantiation fails
    // against the concrete type — Mojo's post-instantiation error — rather
    // than an abstract trait query on `T`.
    let src = "@fieldwise_init\nstruct Plain(Copyable, Movable):\n    var n: Int\n\ndef broken[T: Movable](x: T) -> Int:\n    return x.definitely_missing_member\n\ndef main():\n    print(broken[Plain](Plain(3)))\n";
    let error = run(src).unwrap_err();
    assert!(
        error.contains("type 'Plain' has no field 'definitely_missing_member'"),
        "got: {error}"
    );
}

#[test]
fn bound_generic_template_survives_for_inferred_calls() {
    // Mixed usage of one bound generic: the explicit application monomorphizes
    // while the inferred call stays on the retained template's abstract
    // erased-dispatch path.
    let src = "def ident[T: Copyable & Movable](x: T) -> T:\n    return x\n\ndef main():\n    print(ident[Int](1))\n    print(ident(2))\n";
    assert_eq!(run(src).unwrap(), "1\n2\n");
}
