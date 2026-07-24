//! Phase 4 — ASAP destruction (`__del__`) tests.
//!
//! The VM elaborates drops (`analysis::elaborate_drops_program`) before executing,
//! splicing a `DropVar` at each variable's last use. A struct's `__del__` runs
//! there — *at last use*, not at scope end — and a value's struct fields are
//! destroyed in reverse declaration order. Native Tuple storage follows Mojo's
//! distinct left-to-right element order. These behaviors are asserted directly
//! on VM output.
//!
//! `__del__` uses Mojo's `def __del__(deinit self)` signature.
//! by the checker); the VM treats a method named `__del__` as the destructor.

use mojito::{BackendKind, check_program, elaborate, parse};

fn vm(src: &str) -> String {
    let program = parse(src).expect("parse error");
    let checked = check_program(&program).expect("type error");
    let mut backend = BackendKind::make("vm").expect("the register VM is implemented");
    backend.run(&checked).expect("vm run failed");
    backend.output()
}

fn vm_elaborated(src: &str) -> String {
    let program = parse(src).expect("parse error");
    let program = elaborate(program).expect("comptime elaboration error");
    let checked = check_program(&program).expect("type error");
    let mut backend = BackendKind::make("vm").expect("the register VM is implemented");
    backend.run(&checked).expect("vm run failed");
    backend.output()
}

const RES: &str = "@fieldwise_init\nstruct Res:\n    var id: Int\n    def __del__(deinit self):\n        print(\"del\", self.id)\n\n";

const MOVABLE_NOISY: &str = "struct Noisy(Movable):\n    var id: Int\n\n    def __init__(out self, id: Int):\n        self.id = id\n\n    def __init__(out self, *, deinit move: Self):\n        self.id = move.id\n\n    def __del__(deinit self):\n        print(\"drop\", self.id)\n\n";

#[test]
fn del_runs_at_last_use_not_scope_end() {
    // `a`'s last use is `a.id`; ASAP destruction runs `__del__` there — *before*
    // the following statement (scope-end semantics would print "del 1" last).
    let src = format!(
        "{RES}def main():\n    var a: Res = Res(1)\n    var n: Int = a.id\n    print(\"after a\")\n    print(n)\n"
    );
    assert_eq!(vm(&src), "del 1\nafter a\n1\n");
}

#[test]
fn each_value_dropped_at_its_own_last_use() {
    // Two independently-used values are each destroyed at their own last use, so
    // the teardown is interleaved with the body — not batched at the end.
    let src = format!(
        "{RES}def main():\n    var a: Res = Res(1)\n    print(\"use a\", a.id)\n    var b: Res = Res(2)\n    print(\"use b\", b.id)\n    print(\"done\")\n"
    );
    assert_eq!(vm(&src), "del 1\nuse a 1\ndel 2\nuse b 2\ndone\n");
}

#[test]
fn transferred_value_is_dropped_once_at_destination() {
    // `b = a^` moves the value; it is destroyed once, at `b`'s last use — the moved
    // source `a` is not dropped (no double-free).
    let src = format!(
        "{RES}def main():\n    var a: Res = Res(5)\n    var b: Res = a^\n    print(b.id)\n"
    );
    let out = vm(&src);
    assert_eq!(
        out.matches("del 5").count(),
        1,
        "moved value dropped exactly once"
    );
    assert_eq!(out, "del 5\n5\n");
}

#[test]
fn partially_moved_field_is_dropped_once_at_its_new_owner() {
    // `p.a^` moves one field out to `x`; the moved field is destroyed exactly once
    // — at `x`'s last use — and dropping the whole `p` skips the moved field (no
    // double-drop) while still destroying the retained field `b`.
    let src = "@fieldwise_init\nstruct Inner:\n    var id: Int\n    def __del__(deinit self):\n        print(\"del\", self.id)\n\n@fieldwise_init\nstruct Pair:\n    var a: Inner\n    var b: Inner\n\ndef main():\n    var p: Pair = Pair(Inner(1), Inner(2))\n    var x: Inner = p.a^\n    print(\"x =\", x.id)\n    print(\"b =\", p.b.id)\n";
    let out = vm(src);
    assert_eq!(
        out.matches("del 1").count(),
        1,
        "moved field dropped exactly once"
    );
    assert_eq!(
        out.matches("del 2").count(),
        1,
        "retained field dropped once"
    );
    // `x` (Inner 1) dies at `x.id`; `p`'s retained field `b` (Inner 2) dies at `p.b.id`.
    assert_eq!(out, "del 1\nx = 1\ndel 2\nb = 2\n");
}

#[test]
fn fields_drop_in_reverse_declaration_order() {
    // Destroying a struct runs its `__del__`, then its fields' — in reverse order.
    let src = "@fieldwise_init\nstruct Inner:\n    var id: Int\n    def __del__(deinit self):\n        print(\"del inner\", self.id)\n\n@fieldwise_init\nstruct Outer:\n    var a: Inner\n    var b: Inner\n    def __del__(deinit self):\n        print(\"del outer\")\n\ndef main():\n    var o: Outer = Outer(Inner(1), Inner(2))\n    print(o.a.id)\n";
    // `del outer` first, then field `b` (Inner 2) before field `a` (Inner 1).
    assert_eq!(vm(src), "del outer\ndel inner 2\ndel inner 1\n1\n");
}

#[test]
fn native_tuple_field_elements_drop_once_in_mojo_order() {
    let src = format!(
        "{MOVABLE_NOISY}@fieldwise_init\nstruct Holder:\n    var storage: Tuple[Noisy, Noisy]\n\ndef main():\n    var holder = Holder((Noisy(1), Noisy(2)))\n    print(\"use\", holder.storage[0].id)\n    var keep_alive = holder.storage[1].id\n"
    );
    assert_eq!(vm(&src), "use 1\ndrop 1\ndrop 2\n");
}

#[test]
fn nested_native_tuple_fields_are_recursively_droppable() {
    let src = format!(
        "{MOVABLE_NOISY}@fieldwise_init\nstruct Nested:\n    var storage: Tuple[Int, Tuple[Noisy, Noisy], Noisy]\n\ndef main():\n    var value = Nested((0, (Noisy(3), Noisy(4)), Noisy(5)))\n    print(\"use\", value.storage[0])\n    var keep_alive = value.storage[2].id\n"
    );
    assert_eq!(vm(&src), "use 0\ndrop 3\ndrop 4\ndrop 5\n");
}

#[test]
fn direct_native_tuple_elements_drop_left_to_right() {
    let src = format!(
        "{MOVABLE_NOISY}def main():\n    var values = (Noisy(11), Noisy(12))\n    print(\"use\", values[0].id)\n    var keep_alive = values[1].id\n"
    );
    assert_eq!(vm(&src), "use 11\ndrop 11\ndrop 12\n");
}

#[test]
fn noncopyable_native_tuple_element_transfer_is_rejected() {
    let src = format!(
        "{MOVABLE_NOISY}def main():\n    var values = (Noisy(13), Noisy(14))\n    var first = values[0]^\n    print(first.id)\n"
    );
    let program = parse(&src).expect("parse error");
    let error = match check_program(&program) {
        Ok(_) => panic!("linear tuple element transfer should be rejected"),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .contains("non-implicitly-copyable indexed value")
    );
}

#[test]
fn noncopyable_field_below_native_tuple_index_is_rejected() {
    let src = format!(
        "{MOVABLE_NOISY}@fieldwise_init\nstruct Wrapped:\n    var item: Noisy\n\ndef main():\n    var values = (Wrapped(Noisy(15)),)\n    var item = values[0].item^\n    print(item.id)\n"
    );
    let program = parse(&src).expect("parse error");
    let error = match check_program(&program) {
        Ok(_) => panic!("linear field below a tuple index should be rejected"),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .contains("non-implicitly-copyable indexed value")
    );
}

#[test]
fn copyable_native_tuple_element_transfer_remains_a_copy() {
    let src = "def main():\n    var values = (13, 14)\n    var first = values[0]^\n    print(first, values[0])\n";
    assert_eq!(vm(src), "13 13\n");
}

#[test]
fn moved_variadic_pack_becomes_tuple_storage_without_double_drop() {
    let src = format!(
        "{MOVABLE_NOISY}struct PackHolder[*Ts: Movable](Movable):\n    var storage: Tuple[*Ts]\n\n    def __init__(out self, var *args: *Ts):\n        self.storage = Tuple(*args^)\n\ndef main():\n    var value = PackHolder[Noisy, Int](Noisy(7), 9)\n    print(\"use\", value.storage[1])\n    var keep_alive = value.storage[1]\n"
    );
    let output = vm_elaborated(&src);
    assert_eq!(output, "use 9\ndrop 7\n");
    assert_eq!(output.matches("drop 7").count(), 1);
}

#[test]
fn homogeneous_tuple_variadic_remains_a_list_collector() {
    let src = "def show(*items: Tuple[Int, Int]):\n    print(len(items))\n    print(items[0][1])\n    print(items[1][0])\n\ndef main():\n    show((1, 2), (3, 4))\n";
    assert_eq!(vm(src), "2\n2\n3\n");
}

#[test]
fn tuple_field_droppable_is_cleaned_on_early_and_normal_edges() {
    let src = format!(
        "{MOVABLE_NOISY}@fieldwise_init\nstruct Holder:\n    var storage: Tuple[Noisy, Int]\n\ndef run(flag: Bool):\n    var value = Holder((Noisy(8), 0))\n    if flag:\n        print(\"early\")\n        return\n    print(\"normal\", value.storage[1])\n\ndef main():\n    run(True)\n    run(False)\n"
    );
    let output = vm(&src);
    assert_eq!(output, "drop 8\nearly\ndrop 8\nnormal 0\n");
    assert_eq!(output.matches("drop 8").count(), 2);
}

#[test]
fn tuple_field_is_cleaned_on_a_raising_try_edge() {
    let src = format!(
        "{MOVABLE_NOISY}@fieldwise_init\nstruct Holder:\n    var storage: Tuple[Noisy, Int]\n\ndef run(flag: Bool):\n    try:\n        var value = Holder((Noisy(10), 0))\n        if flag:\n            print(\"raising\")\n            raise \"boom\"\n        print(\"normal\", value.storage[1])\n    except e:\n        print(\"caught\")\n\ndef main():\n    run(True)\n"
    );
    let output = vm(&src);
    assert_eq!(output, "raising\ndrop 10\ncaught\n");
    assert_eq!(output.matches("drop 10").count(), 1);
}

#[test]
fn owned_parameter_is_dropped_by_the_callee() {
    // `consume(a^)` transfers `a` to a `var` parameter: the value is destroyed
    // once, inside the callee (at the parameter's last use) — not by the caller,
    // and not twice. Consuming parameter ownership is what
    // makes this expressible.
    let src = format!(
        "{RES}def consume(var t: Res):\n    print(\"consuming\", t.id)\n\ndef main():\n    var a: Res = Res(1)\n    consume(a^)\n    print(\"done\")\n"
    );
    let out = vm(&src);
    assert_eq!(out.matches("del 1").count(), 1, "destroyed exactly once");
    assert_eq!(out, "del 1\nconsuming 1\ndone\n");
}

#[test]
fn borrowed_parameter_is_not_dropped_by_the_callee() {
    // A plain (borrowed) parameter is owned by the caller: the callee does not drop
    // it, so the destructor runs once — at the caller's last use of `a`.
    let src = format!(
        "{RES}def peek(t: Res) -> Int:\n    return t.id\n\ndef main():\n    var a: Res = Res(2)\n    var n: Int = peek(a)\n    print(n)\n"
    );
    let out = vm(&src);
    assert_eq!(
        out.matches("del 2").count(),
        1,
        "destroyed exactly once (by the caller)"
    );
}

#[test]
fn destructor_less_values_have_no_observable_drop() {
    // A struct without `__del__`, and scalars, drop silently — nothing printed.
    let src = "@fieldwise_init\nstruct Plain:\n    var x: Int\n\ndef main():\n    var p: Plain = Plain(1)\n    var n: Int = 2\n    print(p.x + n)\n";
    assert_eq!(vm(src), "3\n");
}

#[test]
fn value_dying_unused_on_a_branch_is_dropped_on_that_edge() {
    // `a` is used on the `if` arm but not the fall-through: on each path it is
    // destroyed exactly once — at its last use on the taken arm, or on the edge
    // where it dies unused (cross-branch drop elaboration; no leak, no double-free).
    let prog = |flag: &str| {
        format!(
            "{RES}def main():\n    var flag: Bool = {flag}\n    var a: Res = Res(1)\n    if flag:\n        print(\"used\", a.id)\n    print(\"done\")\n"
        )
    };
    assert_eq!(vm(&prog("True")), "del 1\nused 1\ndone\n");
    assert_eq!(vm(&prog("False")), "del 1\ndone\n");
}

#[test]
fn del_in_a_loop_runs_each_iteration() {
    // A value constructed and destroyed inside a loop body is torn down every
    // iteration (ASAP), not once at the end.
    let src = format!(
        "{RES}def main():\n    for i in range(3):\n        var r: Res = Res(i)\n        print(\"iter\", r.id)\n"
    );
    assert_eq!(vm(&src), "del 0\niter 0\ndel 1\niter 1\ndel 2\niter 2\n");
}

#[test]
fn del_runs_when_a_try_body_is_left() {
    // A value constructed in a `try` body is destroyed when the body is left —
    // whether it raises (exceptional-edge cleanup, before the handler) or completes
    // normally (scope-exit) — exactly once on each path.
    let raising = format!(
        "{RES}def main():\n    try:\n        var r: Res = Res(1)\n        print(\"have\", r.id)\n        raise \"boom\"\n    except e:\n        print(\"caught\")\n    print(\"done\")\n"
    );
    assert_eq!(vm(&raising), "have 1\ndel 1\ncaught\ndone\n");

    let normal = format!(
        "{RES}def main():\n    try:\n        var r: Res = Res(2)\n        print(\"have\", r.id)\n    except e:\n        print(\"caught\")\n    print(\"done\")\n"
    );
    assert_eq!(vm(&normal), "have 2\ndel 2\ndone\n");
}

#[test]
fn break_crossing_try_drops_body_local_and_outer_loop_local() {
    // Two values die when a `break` escapes a `try`: a body-local (declared inside
    // the try — dropped via `Try.cleanup`) and an outer loop-body-local (declared
    // in the loop body, used inside the try — dropped via `EscapeJump.cleanup`).
    // Each is destroyed exactly once, and the loop variable survives for `finally`.
    let src = "@fieldwise_init\nstruct D:\n    var id: Int\n    def __del__(deinit self):\n        print(\"drop\", self.id)\n\ndef main():\n    for i in range(3):\n        var outer: D = D(10 + i)\n        try:\n            var inner: D = D(20 + i)\n            print(\"use\", outer.id, inner.id)\n            if i == 1:\n                break\n        finally:\n            print(\"fin\", i)\n    print(\"done\")\n";
    let out = vm(src);
    // i=0: normal iteration — inner drops at its last use, outer after the try.
    // i=1: break — inner (body-local) and outer (loop-local) both drop, once each.
    assert_eq!(
        out.matches("drop 21").count(),
        1,
        "body-local of the break iteration dropped once"
    );
    assert_eq!(
        out.matches("drop 11").count(),
        1,
        "outer loop-local of the break iteration dropped once"
    );
    assert!(
        out.ends_with("fin 1\ndone\n"),
        "finally reads the loop var, then done:\n{out}"
    );
    // No iteration 2 (broke at i=1); its values never constructed.
    assert_eq!(out.matches("drop 12").count(), 0);
}
