//! Phase 4 — ASAP destruction (`__del__`) tests.
//!
//! The VM elaborates drops (`analysis::elaborate_drops_program`) before executing,
//! splicing a `DropVar` at each variable's last use. A struct's `__del__` runs
//! there — *at last use*, not at scope end — and a value's struct fields are
//! destroyed in reverse declaration order. The private heterogeneous pack
//! storage behind nominal `Tuple` follows Mojo's distinct left-to-right element
//! order. These behaviors are asserted directly on VM output.
//!
//! `__del__` uses Mojo's `def __del__(deinit self)` signature. The checker
//! validates that lifecycle contract; the VM invokes the selected destructor.

use mojito::Compiler;
use std::path::Path;

fn vm(src: &str) -> String {
    let compiler = Compiler::default().with_snippet_module_scope();
    let program = compiler
        .compile_source(src, Path::new("drops_test.mojo"))
        .expect("compile error");
    compiler.execute(&program).expect("vm run failed").output
}

fn compile_error(src: &str) -> String {
    let compiler = Compiler::default().with_snippet_module_scope();
    compiler
        .compile_source(src, Path::new("drops_test.mojo"))
        .expect_err("expected compilation to fail")
        .to_string()
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
fn partial_aggregate_skips_its_whole_destructor_and_drops_residual_fields() {
    let src = "@fieldwise_init\nstruct Inner:\n    var id: Int\n    def __del__(deinit self):\n        print(\"drop inner\", self.id)\n\n@fieldwise_init\nstruct Outer:\n    var first: Inner\n    var second: Inner\n    def __del__(deinit self):\n        print(\"drop outer\")\n\ndef main():\n    var outer = Outer(Inner(1), Inner(2))\n    var first = outer.first^\n    print(\"use first\", first.id)\n    print(\"use second\", outer.second.id)\n";
    let output = vm(src);
    assert!(!output.contains("drop outer"), "{output}");
    assert_eq!(output.matches("drop inner 1").count(), 1, "{output}");
    assert_eq!(output.matches("drop inner 2").count(), 1, "{output}");
}

#[test]
fn fields_drop_in_reverse_declaration_order() {
    // Destroying a struct runs its `__del__`, then its fields' — in reverse order.
    let src = "@fieldwise_init\nstruct Inner:\n    var id: Int\n    def __del__(deinit self):\n        print(\"del inner\", self.id)\n\n@fieldwise_init\nstruct Outer:\n    var a: Inner\n    var b: Inner\n    def __del__(deinit self):\n        print(\"del outer\")\n\ndef main():\n    var o: Outer = Outer(Inner(1), Inner(2))\n    print(o.a.id)\n";
    // `del outer` first, then field `b` (Inner 2) before field `a` (Inner 1).
    assert_eq!(vm(src), "del outer\ndel inner 2\ndel inner 1\n1\n");
}

#[test]
fn nominal_tuple_field_elements_drop_once_in_mojo_order() {
    let src = format!(
        "{MOVABLE_NOISY}@fieldwise_init\nstruct Holder:\n    var storage: Tuple[Noisy, Noisy]\n\ndef main():\n    var holder = Holder((Noisy(1), Noisy(2)))\n    print(\"use\", holder.storage[0].id)\n    var keep_alive = holder.storage[1].id\n"
    );
    assert_eq!(vm(&src), "use 1\ndrop 1\ndrop 2\n");
}

#[test]
fn nested_nominal_tuple_fields_are_recursively_droppable() {
    let src = format!(
        "{MOVABLE_NOISY}@fieldwise_init\nstruct Nested:\n    var storage: Tuple[Int, Tuple[Noisy, Noisy], Noisy]\n\ndef main():\n    var value = Nested((0, (Noisy(3), Noisy(4)), Noisy(5)))\n    print(\"use\", value.storage[0])\n    ref last = value.storage[2]\n    var keep_alive = last.id\n"
    );
    assert_eq!(vm(&src), "use 0\ndrop 3\ndrop 4\ndrop 5\n");
}

#[test]
fn direct_nominal_tuple_elements_drop_left_to_right() {
    let src = format!(
        "{MOVABLE_NOISY}def main():\n    var values = (Noisy(11), Noisy(12))\n    ref first = values[0]\n    print(\"use\", first.id)\n    ref second = values[1]\n    var keep_alive = second.id\n"
    );
    assert_eq!(vm(&src), "use 11\ndrop 11\ndrop 12\n");
}

#[test]
fn noncopyable_nominal_tuple_element_transfer_is_rejected() {
    let src = format!(
        "{MOVABLE_NOISY}def main():\n    var values = (Noisy(13), Noisy(14))\n    var first = values[0]^\n    print(first.id)\n"
    );
    let error = compile_error(&src);
    assert!(
        error.contains("non-implicitly-copyable indexed value"),
        "got {error}"
    );
}

#[test]
fn noncopyable_field_below_nominal_tuple_index_is_rejected() {
    let src = format!(
        "{MOVABLE_NOISY}@fieldwise_init\nstruct Wrapped:\n    var item: Noisy\n\ndef main():\n    var values = (Wrapped(Noisy(15)),)\n    var item = values[0].item^\n    print(item.id)\n"
    );
    let error = compile_error(&src);
    assert!(
        error.contains("non-implicitly-copyable indexed value"),
        "got {error}"
    );
}

#[test]
fn copyable_nominal_tuple_element_transfer_remains_a_copy() {
    let src = "def main():\n    var values = (13, 14)\n    var first = values[0]^\n    print(first, values[0])\n";
    assert_eq!(vm(src), "13 13\n");
}

#[test]
fn moved_variadic_pack_becomes_tuple_storage_without_double_drop() {
    let src = format!(
        "{MOVABLE_NOISY}struct PackHolder[*Ts: Movable](Movable):\n    var storage: Tuple[*Ts]\n\n    def __init__(out self, var *args: *Ts):\n        self.storage = Tuple(*args^)\n\ndef main():\n    var value = PackHolder[Noisy, Int](Noisy(7), 9)\n    print(\"use\", value.storage[1])\n    var keep_alive = value.storage[1]\n"
    );
    let compiler = Compiler::default().with_snippet_module_scope();
    let compiled = compiler
        .compile_source(&src, Path::new("drops_test.mojo"))
        .expect("compile heterogeneous pack transfer");
    let mir = mojito::mir::lower_checked_program(compiled.checked());
    let moved_indices = mir
        .functions
        .iter()
        .flat_map(|(_, function)| &function.blocks)
        .flat_map(|block| &block.instrs)
        .filter_map(|instruction| match instruction {
            mojito::mir::MirInstr::MovePlace { place, .. } => place.proj.last(),
            _ => None,
        })
        .filter_map(|projection| match projection {
            mojito::mir::Proj::ConstIndex(index) => Some(*index),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(moved_indices.contains(&0), "{moved_indices:?}");
    assert!(moved_indices.contains(&1), "{moved_indices:?}");

    let output = compiler.execute(&compiled).expect("vm run failed").output;
    assert_eq!(output, "use 9\ndrop 7\n");
    assert_eq!(output.matches("drop 7").count(), 1);
}

#[test]
fn homogeneous_tuple_variadic_uses_private_runtime_pack_storage() {
    let src = "def show(*items: Tuple[Int, Int]):\n    print(len(items))\n    var first = items[0]\n    var second = items[1]\n    print(first[1])\n    print(second[0])\n\ndef main():\n    show((1, 2), (3, 4))\n";
    assert_eq!(vm(src), "2\n2\n3\n");
}

#[test]
fn tuple_field_droppable_is_cleaned_on_early_and_normal_edges() {
    let src = format!(
        "{MOVABLE_NOISY}@fieldwise_init\nstruct Holder:\n    var storage: Tuple[Noisy, Int]\n\ndef run(flag: Bool):\n    var value = Holder((Noisy(8), 0))\n    if flag:\n        print(\"early\")\n        return\n    print(\"normal\", value.storage[1])\n\ndef main():\n    run(True)\n    run(False)\n"
    );
    let output = vm(&src);
    assert_eq!(output, "drop 8\nearly\nnormal 0\ndrop 8\n");
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
fn rebinding_reference_aggregate_releases_each_owner_generation_once() {
    let src = "@fieldwise_init\nstruct Owner:\n    var n: Int\n    def __del__(deinit self):\n        print(\"drop\", self.n)\n\n@fieldwise_init\nstruct Borrowed[origin: Origin[mut=True]]:\n    var ptr: UnsafePointer[Owner, Self.origin]\n\ndef main():\n    var x = Owner(1)\n    var y = Owner(2)\n    var box = Borrowed(UnsafePointer(to=x))\n    print(box.ptr[0].n)\n    box = Borrowed(UnsafePointer(to=y))\n    print(\"rebound\")\n    print(box.ptr[0].n)\n";
    assert_eq!(vm(src), "1\ndrop 1\nrebound\n2\ndrop 2\n");
}

#[test]
fn runtime_reference_owner_drops_after_the_consuming_call() {
    let src = "@fieldwise_init\nstruct Owner:\n    var n: Int\n    def __del__(deinit self):\n        print(\"drop\", self.n)\n\ndef borrow(ref owner: Owner) -> ref[origin_of(owner.n)] Int:\n    return owner.n\n\ndef main():\n    var owner = Owner(1)\n    ref view = borrow(owner)\n    print(view)\n    print(\"after\")\n";
    assert_eq!(vm(src), "1\ndrop 1\nafter\n");
}

#[test]
fn branch_rebinding_reference_aggregate_does_not_double_drop_owners() {
    let src = "@fieldwise_init\nstruct Owner:\n    var n: Int\n    def __del__(deinit self):\n        print(\"drop\", self.n)\n\n@fieldwise_init\nstruct Borrowed[origin: Origin[mut=True]]:\n    var ptr: UnsafePointer[Owner, Self.origin]\n\ndef run(flag: Bool):\n    var x = Owner(1)\n    var y = Owner(2)\n    var box = Borrowed(UnsafePointer(to=x))\n    if flag:\n        box = Borrowed(UnsafePointer(to=y))\n    print(box.ptr[0].n)\n\ndef main():\n    run(True)\n    run(False)\n";
    let output = vm(src);
    assert_eq!(output, "drop 1\n2\ndrop 2\ndrop 2\n1\ndrop 1\n");
    assert_eq!(
        output.matches("drop 1").count(),
        2,
        "each owner 1 dropped once"
    );
    assert_eq!(
        output.matches("drop 2").count(),
        2,
        "each owner 2 dropped once"
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
fn return_from_owned_iteration_drops_the_current_and_residual_elements() {
    let src = "struct Item(Movable):\n    var value: Int\n    def __init__(out self, value: Int):\n        self.value = value\n    def __init__(out self, *, deinit move: Self):\n        self.value = move.value\n    def __del__(deinit self):\n        print(\"drop\", self.value)\n\ndef first_two() -> Int:\n    var items = [Item(1), Item(2), Item(3)]\n    for var item in items^:\n        print(\"take\", item.value)\n        if item.value == 2:\n            return item.value\n    return -1\n\ndef main():\n    print(\"returned\", first_two())\n    print(\"done\")\n";
    assert_eq!(
        vm(src),
        "take 1\ndrop 1\ntake 2\ndrop 2\ndrop 3\nreturned 2\ndone\n"
    );
}

#[test]
fn return_from_owned_iteration_runs_finally_before_iterator_cleanup() {
    let src = "struct Item(Movable):\n    var value: Int\n    def __init__(out self, value: Int):\n        self.value = value\n    def __init__(out self, *, deinit move: Self):\n        self.value = move.value\n    def __del__(deinit self):\n        print(\"drop\", self.value)\n\ndef first_two() -> Int:\n    var items = [Item(1), Item(2), Item(3)]\n    for var item in items^:\n        try:\n            print(\"take\", item.value)\n            if item.value == 2:\n                return item.value\n        finally:\n            print(\"finally\", item.value)\n    return -1\n\ndef main():\n    print(\"returned\", first_two())\n    print(\"done\")\n";
    assert_eq!(
        vm(src),
        "take 1\nfinally 1\ndrop 1\ntake 2\nfinally 2\ndrop 2\ndrop 3\nreturned 2\ndone\n"
    );
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

#[test]
fn deinit_move_source_consumes_residual_field_without_running_its_destructor() {
    // A `deinit` move-source parameter (the `move` in `__moveinit__`) is
    // *consumed*, not destroyed: its whole-value `__del__` is skipped (its
    // resources moved into the receiver, so running it would double-free), but
    // a residual field left live still receives its own destruction. Here the
    // Copyable `Handle` is *copied* by the move constructor rather than
    // transferred, so both the source's and the destination's handles are live
    // and each must be released — two "handle del 7", never a leak and never a
    // double `box del`.
    let src = "struct Handle(Copyable, Movable):\n    var id: Int\n    def __init__(out self, id: Int):\n        self.id = id\n    def __copyinit__(out self, existing: Self):\n        self.id = existing.id\n    def __del__(deinit self):\n        print(\"handle del\", self.id)\n\nstruct Box(Movable):\n    var h: Handle\n    def __init__(out self, h: Handle):\n        self.h = h\n    def __init__(out self, *, deinit move: Self):\n        self.h = move.h\n    def __del__(deinit self):\n        print(\"box del\")\n\ndef main():\n    var b1 = Box(Handle(7))\n    var b2 = b1^\n    print(\"mid\", b2.h.id)\n";
    assert_eq!(vm(src), "handle del 7\nbox del\nhandle del 7\nmid 7\n");
}

// Borrowed iteration source/iterator slot split: a temporary iterable that owns
// its storage is kept live in its own slot through the loop and destroyed
// exactly once, after the loop — in-place normalization would overwrite (leak)
// it. `Numbers` uses the bounded `__len__`/`__next__` protocol; `NumbersIter`
// borrows nothing observable so only the source has a `__del__`.
const ITERABLE_NUMBERS: &str = "@fieldwise_init\nstruct NumbersIter:\n    var cur: Int\n    var stop: Int\n    def __len__(self) -> Int:\n        return self.stop - self.cur\n    def __next__(mut self) -> Int:\n        var v = self.cur\n        self.cur = self.cur + 1\n        return v\n\nstruct Numbers(Movable):\n    var stop: Int\n    def __init__(out self, stop: Int):\n        self.stop = stop\n    def __init__(out self, *, deinit move: Self):\n        self.stop = move.stop\n    def __del__(deinit self):\n        print(\"drop numbers\", self.stop)\n    def __iter__(self) -> NumbersIter:\n        return NumbersIter(0, self.stop)\n\n";

#[test]
fn borrowed_iteration_over_a_temporary_drops_the_source_after_the_loop() {
    let src = format!(
        "{ITERABLE_NUMBERS}def main():\n    for x in Numbers(3):\n        print(\"x\", x)\n    print(\"after\")\n"
    );
    assert_eq!(vm(&src), "x 0\nx 1\nx 2\ndrop numbers 3\nafter\n");
}

#[test]
fn breaking_out_of_borrowed_temporary_iteration_still_drops_the_source_once() {
    let src = format!(
        "{ITERABLE_NUMBERS}def main():\n    for x in Numbers(5):\n        print(\"x\", x)\n        if x == 1:\n            break\n    print(\"after\")\n"
    );
    let out = vm(&src);
    assert_eq!(out, "x 0\nx 1\ndrop numbers 5\nafter\n");
    assert_eq!(out.matches("drop numbers").count(), 1);
}
