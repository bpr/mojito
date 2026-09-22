//! Compile-time elaboration (`comptime if` / `comptime for`). Each test runs the
//! real pipeline stage order — parse → **validate** → **elaborate** → check → VM —
//! so it exercises the phase-distinction semantics: every arm is checked with
//! the declaration's parameters symbolic before elaboration selects one, and
//! `comptime for` unrolls with the loop variable substituted as a literal.

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
fn free_type_pack_def_takes_checked_arguments() {
    // A free `def show[*Ts: Writable](*args: *Ts)` specializes over any
    // checked argument — a local, a constructed value, an origin-bearing
    // temporary whose erased origin slot the clone spells as `_` — through
    // the checker-recorded instantiation, as inferred bound-generic calls do;
    // literal-only calls still specialize before checking.
    let src = "from std.format._utils import Named\n\ndef show[*Ts: Writable](*args: *Ts):\n    comptime for i in range(Ts.length):\n        print(args[i])\n\ndef main():\n    var w = 7\n    var x = 5\n    show(x)\n    show(\"lit\", 3)\n    show(Named(\"k\", w), x)\n    var n = Named(\"m\", w)\n    show(n)\n    w += 1\n    print(w)\n";
    assert_eq!(run(src).unwrap(), "5\nlit\n3\nk=7\n5\nm=7\n8\n");
    // A type-pack template that is never called still elaborates.
    let unused = "def show[*Ts: Writable](*args: *Ts):\n    comptime for i in range(Ts.length):\n        print(args[i])\n\ndef main():\n    print(1)\n";
    assert_eq!(run(unused).unwrap(), "1\n");
    // A bound violation is still reported.
    let bad = "struct Opaque:\n    var n: Int\n    def __init__(out self):\n        self.n = 0\n\ndef show[*Ts: Writable](*args: *Ts):\n    print(1)\n\ndef main():\n    var o = Opaque()\n    show(o)\n";
    let err = run(bad).unwrap_err();
    assert!(
        err.contains("does not conform to trait 'Writable'"),
        "{err}"
    );
}

#[test]
fn method_pack_elements_spell_erased_origin_slots_as_placeholders() {
    // `FormatStruct.params(Named("k", k))` (upstream's collection-repr
    // spelling): the method pack's element type renders its erased origin
    // slot as `_`, which the clone's parameter annotation accepts.
    let src = "from std.format._utils import FormatStruct, Named\n\nstruct S(Writable):\n    var x: Int\n    def __init__(out self, x: Int):\n        self.x = x\n    def write_to(self, mut writer: Some[Writer]):\n        var k = self.x + 1\n        FormatStruct(writer, \"S\").params(Named(\"k\", k), Named(\"x\", self.x)).fields(self.x)\n\ndef main():\n    print(S(3))\n";
    assert_eq!(run(src).unwrap(), "S[k=4, x=3](3)\n");
}

#[test]
fn inferred_generic_over_origin_erased_iterator_stays_abstract() {
    // A bound-generic call whose inferred type argument is an origin-slotted
    // struct (`next(it)` over a stored `_ListIter`, `ident(n)` over a
    // `Named` local) keeps the abstract path: a clone could not spell the
    // erased origin slot. The bundled List/Set iterators iterate themselves.
    let src = "from std.format._utils import Named\n\ndef ident[T: AnyType](x: T) -> Int:\n    return 1\n\ndef main() raises:\n    var xs: List[Int] = [1, 2, 3]\n    var it = xs.__iter__()\n    print(next(it), next(it))\n    for rest in it:\n        print(rest)\n    var st = Set[Int]()\n    st.add(9)\n    var si = st.__iter__()\n    print(next(si))\n    var w = 7\n    var n = Named(\"k\", w)\n    print(ident(n), ident(it))\n";
    assert_eq!(run(src).unwrap(), "1 2\n3\n9\n1 1\n");
    let mutation = "def main() raises:\n    var xs: List[Int] = [1, 2]\n    var it = xs.__iter__()\n    xs.append(9)\n    print(next(it))\n";
    let err = run(mutation).unwrap_err();
    assert!(err.contains("conflicts with live reference"), "{err}");
}

#[test]
fn comptime_if_selects_a_branch() {
    let src = "comptime N = 8\n\ndef main():\n    comptime if N > 4:\n        print(\"big\")\n    elif N > 0:\n        print(\"small\")\n    else:\n        print(\"zero\")\n";
    assert_eq!(run(src).unwrap(), "big\n");
}

#[test]
fn comptime_if_checks_the_unselected_branch_before_elaboration() {
    // Every arm is checked before elaboration selects one, as the pinned
    // Mojo does: a type error in the `else` arm rejects the program even
    // though `FLAG == 1` selects the `if` arm.
    let src = "comptime FLAG = 1\n\ndef main():\n    comptime if FLAG == 1:\n        print(\"ok\")\n    else:\n        var bad: Int = \"not an int\"\n        print(bad)\n";
    let err = run(src).unwrap_err();
    assert!(err.contains("expected Int, found StringLiteral"), "{err}");
    // The same program with a valid `else` arm still runs only the selected arm.
    let valid = "comptime FLAG = 1\n\ndef main():\n    comptime if FLAG == 1:\n        print(\"ok\")\n    else:\n        print(\"other\")\n";
    assert_eq!(run(valid).unwrap(), "ok\n");
}

#[test]
fn comptime_if_arms_of_a_template_check_with_symbolic_parameters() {
    // A member the bound does not declare is rejected in an untaken arm of a
    // generic template, and so is an unused template's invalid arm.
    let member = "def g[T: Copyable, flag: Bool](x: T) -> String:\n    comptime if flag:\n        return \"ok\"\n    else:\n        x.nonexistent()\n        return \"bad\"\n\ndef main():\n    print(g[Int, True](1))\n";
    let err = run(member).unwrap_err();
    assert!(
        err.contains("type 'T' has no method 'nonexistent'"),
        "{err}"
    );
    let unused = "def unused[n: Int]() -> Int:\n    comptime if n == 0:\n        return 1\n    else:\n        var x: Int = \"hello\"\n        return x\n\ndef main():\n    print(2)\n";
    let err = run(unused).unwrap_err();
    assert!(err.contains("expected Int, found StringLiteral"), "{err}");
    // A guard does not narrow the parameter: `T == Int` grants no `__add__`.
    let narrowing = "def f[T: Copyable](x: T) -> Int:\n    comptime if T == Int:\n        return x + 1\n    return 0\n\ndef main():\n    print(f[Int](3))\n";
    let err = run(narrowing).unwrap_err();
    assert!(err.contains("'+' is not defined for T"), "{err}");
}

#[test]
fn comptime_if_arms_are_block_scoped_and_dead_arms_have_no_effect() {
    // A binding declared in an arm is not visible after the conditional, as
    // upstream scopes it; a valid untaken arm produces no output.
    let scoped = "def main():\n    comptime if True:\n        var x = 1\n    print(x)\n";
    let err = run(scoped).unwrap_err();
    assert!(err.contains("Undefined variable 'x'"), "{err}");
    let dead = "def shout():\n    print(\"never\")\n\ndef f[n: Int]() -> Int:\n    comptime if n == 0:\n        return 1\n    else:\n        shout()\n        return 2\n\ndef main():\n    print(f[0]())\n";
    assert_eq!(run(dead).unwrap(), "1\n");
    // A dead arm's compile-time evaluation failure is not a type error.
    let dead_eval = "def f[n: Int]() -> Int:\n    comptime if n == 0:\n        return 1\n    else:\n        comptime k = 1 // 0\n        return k\n\ndef main():\n    print(f[0]())\n";
    assert_eq!(run(dead_eval).unwrap(), "1\n");
}

#[test]
fn comptime_if_arms_use_tuple_members_and_value_parameters() {
    // A `Tuple` over concrete types is an ordinary struct application under
    // validation: its constructor, subscript, `len`, `in`, and `==` type from
    // the template's signatures with the pack bound to the element list. A
    // value parameter is a compile-time binding in a default and in a nested
    // function, not a capture.
    let tuple = "def f[n: Int]():\n    comptime if n == 0:\n        var inferred = Tuple(1, \"one\")\n        var typed = Tuple[Float64, String](2, \"two\")\n        var pair = (3, True)\n        print(inferred[1], typed[0], pair[1], len(pair), 3 in pair, pair == (3, True))\n\ndef main():\n    f[0]()\n";
    assert_eq!(run(tuple).unwrap(), "one 2.0 True 2 True True\n");
    let nested = "def outer[n: Int]() -> Int:\n    comptime if n >= 0:\n        pass\n    def scaled() -> Int:\n        return n * 10\n    return scaled()\n\ndef main():\n    print(outer[2]())\n";
    assert_eq!(run(nested).unwrap(), "20\n");
}

#[test]
fn pack_keyed_bodies_are_validated_with_the_element_opaque() {
    // An untaken arm of a pack-keyed body is judged from the template: the
    // element under a `comptime for` index has only the pack's bound.
    let def_pack = "def show[*Ts: Writable](*args: *Ts):\n    comptime for i in range(args.__len__()):\n        comptime if i > 100:\n            args[i].nonexistent()\n        print(args[i])\n\ndef main():\n    show(1, \"two\")\n";
    let err = run(def_pack).unwrap_err();
    assert!(
        err.contains("type 'Ts[i]' has no method 'nonexistent'"),
        "{err}"
    );
    // A struct's pack, never instantiated.
    let bag = "struct Bag[*Ts: Movable](\n    Deinitable where Ts.all_conforms_to[Deinitable](),\n    Movable,\n):\n    var storage: Tuple[*Self.Ts]\n\n    def __init__(out self, var *args: *Self.Ts):\n        self.storage = Tuple[*Self.Ts](*args^)\n\n";
    let main = "\ndef main():\n    print(1)\n";
    let untaken = format!(
        "{bag}    def poke(self):\n        comptime for i in range(Self.Ts.length):\n            comptime if i > 100:\n                self.storage[i].nonexistent()\n{main}"
    );
    let err = run(&untaken).unwrap_err();
    assert!(err.contains("has no method 'nonexistent'"), "{err}");
    // The declared bound and a conjunctive method `where` license a trait
    // use; nothing else does.
    let licensed = format!(
        "{bag}    def show(self) where conforms_to(Self.Ts.values, Writable):\n        comptime for i in range(Self.Ts.length):\n            print(self.storage[i])\n\n    def show_all(self) where Self.Ts.all_conforms_to[Writable]():\n        comptime for i in range(Self.Ts.length):\n            print(self.storage[i])\n{main}"
    );
    assert_eq!(run(&licensed).unwrap(), "1\n");
    for clause in [
        "",
        " where Self.Ts.all_conforms_to[Writable]() or Self.Ts.all_conforms_to[Hashable]()",
    ] {
        let unlicensed = format!(
            "{bag}    def show(self){clause}:\n        comptime for i in range(Self.Ts.length):\n            print(self.storage[i])\n{main}"
        );
        let err = run(&unlicensed).unwrap_err();
        assert!(
            err.contains("does not conform to trait 'Writable'"),
            "{clause}: {err}"
        );
    }
    // No guard narrows an element, a constant index keeps it dependent, and a
    // membership condition leaves both arms to check.
    let narrowed = format!(
        "{bag}    def count[T: Equatable](self, value: T) -> Int:\n        comptime for i in range(Self.Ts.length):\n            comptime if Self.Ts[i] == T:\n                if self.storage[i] == value:\n                    return 1\n        return 0\n{main}"
    );
    let err = run(&narrowed).unwrap_err();
    assert!(
        err.contains("operator '==' is not defined for Ts[i] and T"),
        "{err}"
    );
    let constant = format!(
        "{bag}    def first(self) -> Int:\n        return rebind[Int](self.storage[0]) + self.storage[0]\n{main}"
    );
    assert!(run(&constant).is_err());
    let membership = format!(
        "{bag}    def has[T: AnyType](self) -> Int:\n        comptime if Self.Ts.contains[T]():\n            return 1\n        else:\n            return \"no\"\n{main}"
    );
    let err = run(&membership).unwrap_err();
    assert!(err.contains("expected Int, found StringLiteral"), "{err}");
    let mixed = "struct Lead[*Ts: Movable & Deinitable](Movable):\n    var storage: Tuple[Int, *Self.Ts]\n\ndef main():\n    print(1)\n";
    let err = run(mixed).unwrap_err();
    assert!(
        err.contains("a variadic pack spread must be the only argument"),
        "{err}"
    );
}

#[test]
fn dtype_keyed_bodies_are_validated_with_the_lane_symbolic() {
    // An untaken arm of a `DType`- or width-keyed body is judged from the
    // template, with `Scalar[dt]` / `SIMD[dt, width]` symbolic: a def, a
    // struct method keyed on `Self.dt`, a struct holding a symbolic vector
    // field, and a validated body applying a `DType`-keyed struct.
    let def = "def kind[dt: DType](a: Scalar[dt]) -> Int:\n    comptime if dt == DType.float64:\n        return 1\n    else:\n        var s: String = a\n        return 2\n\ndef main():\n    print(kind[DType.float64](1.5))\n";
    let err = run(def).unwrap_err();
    assert!(err.contains("expected String, found Scalar[dt]"), "{err}");
    let width = "def total[dt: DType, width: Int](v: SIMD[dt, width]) -> Int:\n    comptime if width == 4:\n        return 4\n    else:\n        var s: String = v\n        return 0\n\ndef main():\n    print(total[DType.int32, 4](SIMD[DType.int32, 4](1, 2, 3, 4)))\n";
    let err = run(width).unwrap_err();
    assert!(
        err.contains("expected String, found SIMD[dt, width]"),
        "{err}"
    );
    let vec = "struct Vec[dt: DType](Movable):\n    var x: Scalar[Self.dt]\n\n    def __init__(out self, x: Scalar[Self.dt]):\n        self.x = x\n\n    def get(self) -> Scalar[Self.dt]:\n        return self.x\n\n";
    let method = format!(
        "{vec}    def tag(self) -> Int:\n        comptime if Self.dt == DType.float64:\n            return 1\n        else:\n            var s: String = self.x\n            return 2\n\ndef main():\n    print(Vec[DType.float64](1.5).tag())\n"
    );
    let err = run(&method).unwrap_err();
    assert!(err.contains("expected String, found Scalar[dt]"), "{err}");
    let field = "struct Buf[dt: DType, width: Int](Copyable, Movable):\n    var v: SIMD[Self.dt, Self.width]\n\n    def __init__(out self, v: SIMD[Self.dt, Self.width]):\n        self.v = v\n\n    def tag(self) -> Int:\n        comptime if Self.width == 4:\n            return 4\n        else:\n            var s: String = self.v\n            return 0\n\ndef main():\n    print(Buf[DType.int32, 4](SIMD[DType.int32, 4](1, 2, 3, 4)).tag())\n";
    let err = run(field).unwrap_err();
    assert!(
        err.contains("expected String, found SIMD[dt, width]"),
        "{err}"
    );
    // Applying the struct closes its lane: to `Float64` at a concrete dtype,
    // to the caller's own symbolic `dt` in a template.
    let applied = format!(
        "{vec}def build[T: Copyable](t: T) -> Float64:\n    comptime if T == Int:\n        return Vec[DType.float64](2.0).get()\n    else:\n        var s: String = Vec[DType.float64](2.0).get()\n        return 0.0\n\ndef main():\n    print(build(1))\n"
    );
    let err = run(&applied).unwrap_err();
    assert!(err.contains("expected String, found Float64"), "{err}");
    let symbolic = format!(
        "{vec}def make[dt: DType](a: Scalar[dt]) -> Vec[dt]:\n    var v = Vec[dt](a)\n    comptime if dt == DType.float64:\n        return v^\n    else:\n        var s: String = v.get()\n        return v^\n\ndef main():\n    print(make[DType.float64](2.5).get())\n"
    );
    let err = run(&symbolic).unwrap_err();
    assert!(err.contains("expected String, found Scalar[dt]"), "{err}");
    // A guard narrows nothing: `SIMD[dt, width]` stays its own type inside
    // the `width == 4` arm, and a concrete scalar never splats into it.
    let narrowed = "def narrowed[dt: DType, width: Int](v: SIMD[dt, width]):\n    comptime if width == 4:\n        var w: SIMD[dt, 4] = v\n\ndef main():\n    pass\n";
    let err = run(narrowed).unwrap_err();
    assert!(
        err.contains("expected SIMD[dt, 4], found SIMD[dt, width]"),
        "{err}"
    );
    let scalar = "def scaled[dt: DType](a: Scalar[dt]) -> Scalar[dt]:\n    comptime if dt == DType.int32:\n        return a * Int(2)\n    return a + 1.5\n\ndef main():\n    pass\n";
    let err = run(scalar).unwrap_err();
    assert!(
        err.contains("operator '*' is not defined for Scalar[dt] and Int"),
        "{err}"
    );
}

#[test]
fn rebind_retypes_its_operand_and_checks_the_instantiation() {
    // `rebind[Dest](value)` types as `Dest` while the operand is symbolic and
    // is an identity once instantiated; a mismatched instantiation and a
    // transferred operand are rejected.
    let src = "def f[T: Copyable](x: T) -> Int:\n    comptime if T == Int:\n        return rebind[Int](x) + 1\n    return 0\n\ndef main():\n    print(f[Int](3))\n";
    assert_eq!(run(src).unwrap(), "4\n");
    let reference = "def f[T: Copyable](ref x: T) -> Int:\n    comptime if T == Int:\n        ref y = rebind[Int](x)\n        return y + 1\n    return 0\n\ndef main():\n    var v = 3\n    print(f[Int](v))\n";
    assert_eq!(run(reference).unwrap(), "4\n");
    let wrong_use = "def f[T: Copyable](x: T) -> Int:\n    comptime if T == Int:\n        return rebind[String](x) + 1\n    return 0\n\ndef main():\n    print(f[Int](3))\n";
    let err = run(wrong_use).unwrap_err();
    assert!(err.contains("expected String, found Int"), "{err}");
    let mismatch =
        "def main():\n    var x: Int = 3\n    var y = rebind[Float64](x)\n    print(y)\n";
    let err = run(mismatch).unwrap_err();
    assert!(
        err.contains("rebind: the input type does not match the result type"),
        "{err}"
    );
    let transfer = "struct Tracked(Movable):\n    var n: Int\n    def __init__(out self, n: Int):\n        self.n = n\n\ndef take[T: Movable](var x: T):\n    comptime if T == Tracked:\n        var t = rebind[Tracked](x^)\n        print(t.n)\n\ndef main():\n    take[Tracked](Tracked(1))\n";
    let err = run(transfer).unwrap_err();
    assert!(
        err.contains("rebind takes its operand by reference"),
        "{err}"
    );
}

#[test]
fn rebind_keys_specialization_without_compile_time_control_flow() {
    // A `rebind` alone makes a parametric body specialize per instantiation:
    // the assertion is made on each clone, so an instantiation that satisfies
    // it runs, one that does not is rejected, and a template no call
    // instantiates is never judged.
    let src = "def bump[T: Copyable](mut x: T):\n    rebind[Int](x) += 1\n\ndef main():\n    var v = 3\n    bump(v)\n    print(v)\n";
    assert_eq!(run(src).unwrap(), "4\n");
    let mismatch = "def bump[T: Copyable](mut x: T):\n    rebind[Int](x) += 1\n\ndef main():\n    var s = String(\"a\")\n    bump(s)\n    print(s)\n";
    let err = run(mismatch).unwrap_err();
    assert!(
        err.contains("rebind: the input type does not match the result type"),
        "{err}"
    );
    let uninstantiated =
        "def bump[T: Copyable](mut x: T):\n    rebind[Int](x) += 1\n\ndef main():\n    print(1)\n";
    assert_eq!(run(uninstantiated).unwrap(), "1\n");
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
fn comptime_for_zero_step_range_unrolls_nothing() {
    let src = "def main():\n    print(\"before\")\n    comptime for i in range(0, 5, 0):\n        print(i)\n    print(\"after\")\n";
    assert_eq!(run(src).unwrap(), "before\nafter\n");
}

#[test]
fn specialized_where_erases_after_only_origin_metadata_remains() {
    let src = "def enabled[m: Bool, //, o: Origin[mut=m]](ref[o] value: Int) -> Int where (m == True, \"mutable only\"):\n    return value\n\ndef main():\n    var value = 1\n    print(enabled(value))\n";
    assert_eq!(run(src).unwrap(), "1\n");
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
fn comptime_for_iterates_a_heterogeneous_pack() {
    // The payoff: `args[i]` needs a compile-time-constant index (pack elements
    // are heterogeneous), which a runtime `for` can't provide — but `comptime
    // for` substitutes `i` with a literal, so each `args[i]` type-checks.
    let src = "def show[*Ts: Writable](*args: *Ts):\n    comptime for i in range(Ts.length):\n        print(args[i])\n\ndef main():\n    show(42, \"hi\", True)\n";
    assert_eq!(run(src).unwrap(), "42\nhi\nTrue\n");
}

#[test]
fn cloned_comptime_bodies_keep_distinct_checked_occurrence_facts() {
    let src = "def outer[*Ts: ImplicitlyCopyable & Writable & Deinitable](*args: *Ts):\n    comptime for i in range(Ts.length):\n        if True:\n            var x = args[i]\n            def show() {x}:\n                print(x)\n            show()\n\ndef main():\n    outer(1, True)\n";
    assert_eq!(run(src).unwrap(), "1\nTrue\n");
}

#[test]
fn comptime_for_over_a_list_of_strings() {
    // Iterate a compile-time list of strings; a compile-time Tuple has no
    // `__iter__` and rejects (`assets/type_error/comptime_for_tuple.mojo`).
    let src = "comptime states = [\"empty\", \"occupied\", \"deleted\"]\n\ndef main():\n    comptime for state in states:\n        print(state)\n";
    assert_eq!(run(src).unwrap(), "empty\noccupied\ndeleted\n");
}

#[test]
fn heterogeneous_type_pack_round_trips_through_tuple_spread() {
    // Mirrors current Mojo: a heterogeneous variadic pack can be transferred
    // into `Tuple[*Ts]`; this is not general fixed-arity call spreading.
    let src = "def repack[*Ts: Movable](var *args: *Ts) -> Tuple[*Ts]:\n    return Tuple[*Ts](*args^)\n\ndef main():\n    var values: Tuple[Int, StringLiteral, Bool] = repack(3, \"seven\", True)\n    print(values)\n";
    assert_eq!(run(src).unwrap(), "(3, seven, True)\n");
}

#[test]
fn runtime_pack_spread_rejects_shadowing_value_bindings() {
    let block = "def inspect[*Ts: Movable & Deinitable](var *args: *Ts):\n    if True:\n        var args = Tuple(9, 10)\n        var local = Tuple(*args^)\n        print(local)\n\ndef main():\n    inspect(1, True)\n";
    let nested = "def inspect[*Ts: Movable & Deinitable](var *args: *Ts):\n    def nested(var args: Tuple[Int, Int]):\n        print(Tuple(*args^))\n    nested(Tuple(9, 10))\n\ndef main():\n    inspect(1, True)\n";
    let loop_binding = "def inspect[*Ts: Movable & Deinitable](var *args: *Ts):\n    for args in [Tuple(9, 10)]:\n        print(Tuple(*args))\n\ndef main():\n    inspect(1, True)\n";
    let comprehension = "def inspect[*Ts: Movable & Deinitable](var *args: *Ts):\n    var lengths = [len(Tuple(*args)) for args in [Tuple(9, 10)]]\n    print(lengths[0])\n\ndef main():\n    inspect(1, True)\n";
    let sibling_method = "struct Pair[*Ts: Copyable & Movable](Copyable, Movable):\n    var storage: Tuple[*Self.Ts]\n    def __init__(out self, var *args: *Self.Ts):\n        self.storage = Tuple(*args^)\n    def shadow(self, var args: Tuple[Int, Int]):\n        print(Tuple(*args^))\n\ndef main():\n    var pair = Pair[Int](1)\n    pair.shadow(Tuple(9, 10))\n";

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
    let src = "def count[*Ts: Movable & Deinitable](var *args: *Ts) -> Int:\n    if True:\n        var args = Tuple(7, 8)\n        print(len(args))\n    for args in [Tuple(9, 10)]:\n        print(len(args))\n    var packed = Tuple(*args^)\n    return len(packed)\n\ndef main():\n    print(count(1, \"two\", True))\n";
    assert_eq!(run(src).unwrap(), "2\n2\n3\n");
}

#[test]
fn empty_runtime_pack_is_recognized_by_binding_presence() {
    let src = "def repack[*Ts: Movable](var *args: *Ts) -> Tuple[*Ts]:\n    return Tuple[*Ts](*args^)\n\ndef main():\n    var values = repack()\n    print(len(values))\n";
    assert_eq!(run(src).unwrap(), "0\n");
}

#[test]
fn type_pack_expansion_respects_nested_type_parameter_shadowing() {
    let src = "def inspect[*Ts: Copyable & Deinitable](*args: *Ts):\n    def nested[Ts: AnyType](value: Tuple[*Ts]) -> Int:\n        return len(value)\n    print(nested[Int](Tuple(1, True)))\n\ndef main():\n    inspect(9, False)\n";
    let error = run(src).unwrap_err();
    assert!(error.contains("unknown type '*Ts'"), "got: {error}");
}

#[test]
fn nested_heterogeneous_pack_specializes_at_its_lexical_declaration() {
    let src = "def count[*Ts: Movable & Deinitable](var *args: *Ts) -> Int:\n    def nested[*Ts: Movable & Deinitable](var *args: *Ts) -> Int:\n        return len(Tuple(*args^))\n    return nested(1, \"two\", True) + len(Tuple(*args^))\n\ndef main():\n    print(count(9, False))\n";
    assert_eq!(run(src).unwrap(), "5\n");
}

#[test]
fn nested_pack_supports_empty_and_distinct_specializations() {
    let src = "def outer():\n    def count[*Ts: Movable & Deinitable](var *args: *Ts) -> Int:\n        return len(args)\n    print(count())\n    print(count(1))\n    print(count(1, \"two\"))\n\ndef main():\n    outer()\n";
    assert_eq!(run(src).unwrap(), "0\n1\n2\n");
}

#[test]
fn nested_value_parameter_specialization_resolves_comptime_control_flow() {
    let src = "def outer():\n    def choose[flag: Bool]() -> Int:\n        comptime if flag:\n            return 41\n        else:\n            return 1\n    print(choose[True]() + choose[False]())\n\ndef main():\n    outer()\n";
    assert_eq!(run(src).unwrap(), "42\n");
}

#[test]
fn nested_pack_can_forward_to_an_earlier_pack_sibling() {
    let src = "def outer() -> Int:\n    def count[*Ts: Movable & Deinitable](var *args: *Ts) -> Int:\n        return len(args)\n    def relay[*Ts: Movable & Deinitable](var *args: *Ts) -> Int:\n        return count(*args^)\n    return relay(1, \"two\", True)\n\ndef main():\n    print(outer())\n";
    assert_eq!(run(src).unwrap(), "3\n");
}

#[test]
fn captured_outer_pack_forwarding_infers_only_the_variadic_overflow() {
    let src = "def outer[*Ts: Movable & Deinitable](var *args: *Ts) -> Int:\n    def score[*Us: Movable & Deinitable](head: Int, var *values: *Us) -> Int:\n        return head + len(values)\n    def relay() {args^} -> Int:\n        return score(40, *args^)\n    return relay()\n\ndef main():\n    print(outer(1, True))\n";
    assert_eq!(run(src).unwrap(), "42\n");
}

#[test]
fn nested_pack_reference_return_preserves_the_caller_handle() {
    let src = "def main():\n    var value = 40\n    def borrow[*Ts: Movable & Deinitable](ref item: Int, var *args: *Ts) -> ref[item] Int:\n        return item\n    ref borrowed = borrow(value, True)\n    borrowed += 2\n    print(value)\n";
    assert_eq!(run(src).unwrap(), "42\n");
}

#[test]
fn nested_pack_forwarding_transfers_move_only_elements() {
    let src = "struct Item(Movable):\n    var value: Int\n    def __init__(out self, value: Int):\n        self.value = value\n    def __init__(out self, *, deinit move: Self):\n        self.value = move.value\n    def __deinit__(deinit self):\n        print(\"drop\", self.value)\n\ndef outer() -> Int:\n    def first[*Ts: Movable & Deinitable](var *args: *Ts) -> Int:\n        return args[0].value\n    def relay[*Ts: Movable & Deinitable](var *args: *Ts) -> Int:\n        return first(*args^)\n    return relay(Item(42))\n\ndef main():\n    print(outer())\n";
    assert_eq!(run(src).unwrap(), "drop 42\n42\n");
}

#[test]
fn nested_whole_pack_forwarding_preserves_fixed_prefix_and_keyword_tail() {
    let src = "struct Item(Movable):\n    var value: Int\n    def __init__(out self, value: Int):\n        self.value = value\n    def __init__(out self, *, deinit move: Self):\n        self.value = move.value\n    def __deinit__(deinit self):\n        print(\"drop\", self.value)\n\ndef outer() -> Int:\n    def score[*Ts: Movable & Deinitable](out result: Int, head: Int, var *values: *Ts, scale: Int = 1):\n        result = (head + values[0].value) * scale\n    def relay[*Ts: Movable & Deinitable](var *values: *Ts) -> Int:\n        return score(1, *values^, scale=2)\n    return relay(Item(20))\n\ndef main():\n    print(outer())\n";
    assert_eq!(run(src).unwrap(), "drop 20\n42\n");
}

#[test]
fn top_level_whole_pack_forwarding_preserves_fixed_prefix_and_linear_values() {
    // A top-level pack body is validated from its template, so the element is
    // opaque and `rebind[Item]` is what licenses the field read. Verified on
    // the pin 2026-09-20: both print `drop 40` then `42`.
    let src = "struct Item(Movable):\n    var value: Int\n    def __init__(out self, value: Int):\n        self.value = value\n    def __init__(out self, *, deinit move: Self):\n        self.value = move.value\n    def __deinit__(deinit self):\n        print(\"drop\", self.value)\n\ndef score[*Ts: Movable & Deinitable](head: Int, var *values: *Ts) -> Int:\n    return head + rebind[Item](values[0]).value\n\ndef inner_relay[*Ts: Movable & Deinitable](var *values: *Ts) -> Int:\n    return score(2, *values^)\n\ndef relay[*Ts: Movable & Deinitable](var *values: *Ts) -> Int:\n    return inner_relay(*values^)\n\ndef main():\n    print(relay(Item(40)))\n";
    assert_eq!(run(src).unwrap(), "drop 40\n42\n");
}

#[test]
fn generic_comptime_aliases_survive_elaboration_unchanged() {
    // A generic alias cannot be folded eagerly (its body references its own
    // parameters); the elaborator passes it through untouched — repeated
    // where clauses included — and the checker owns expansion.
    let src = "comptime Pair[T: Copyable & Movable]: AnyType where (True, \"m1\") where (True, \"m2\") = Tuple[T, T]\n\ndef main():\n    print(1)\n";
    let program = elaborate(parse(src).expect("parse")).expect("elaborate");
    let alias = program
        .iter()
        .find_map(|stmt| match &stmt.kind {
            mojito::ast::StmtKind::Comptime {
                name,
                type_params,
                where_clauses,
                ..
            } if name == "Pair" => Some((type_params.len(), where_clauses.len())),
            _ => None,
        })
        .expect("the generic alias survives elaboration");
    assert_eq!(alias, (1, 2));
}

#[test]
fn generic_comptime_alias_expansion_runs_through_the_vm() {
    let src = "comptime Pair[T: Copyable & Movable]: AnyType = Tuple[T, T]\ncomptime Guard[n: Int]: AnyType where (n > 0, \"positive only\") = Int\n\ndef main():\n    var pair: Pair[Int] = (1, 2)\n    var guarded: Guard[3] = 7\n    print(pair[0] + guarded)\n";
    assert_eq!(run(src).unwrap(), "8\n");
}

#[test]
fn whole_pack_forwarding_reaches_mir_as_one_tuple_move() {
    let src = "def sink[*Ts: Movable & Deinitable](var *values: *Ts) -> Int:\n    return len(values)\n\ndef relay[*Ts: Movable & Deinitable](var *values: *Ts) -> Int:\n    return sink(*values^)\n\ndef main():\n    print(relay(42))\n";
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
    let multiple = "def outer() -> Int:\n    def count[*Ts: Movable & Deinitable](var *values: *Ts) -> Int:\n        return len(values)\n    def relay[*Ts: Movable & Deinitable](var *values: *Ts) -> Int:\n        return count(*values^, *values^)\n    return relay(1, True)\n\ndef main():\n    print(outer())\n";
    let mixed = "def outer() -> Int:\n    def count[*Ts: Movable & Deinitable](var *values: *Ts) -> Int:\n        return len(values)\n    def relay[*Ts: Movable & Deinitable](var *values: *Ts) -> Int:\n        return count(*values^, 9)\n    return relay(1, True)\n\ndef main():\n    print(outer())\n";
    let top_level_multiple = "def count[*Ts: Movable & Deinitable](var *values: *Ts) -> Int:\n    return len(values)\n\ndef relay[*Ts: Movable & Deinitable](var *values: *Ts) -> Int:\n    return count(*values^, *values^)\n\ndef main():\n    print(relay(1, True))\n";
    let top_level_mixed = "def count[*Ts: Movable & Deinitable](var *values: *Ts) -> Int:\n    return len(values)\n\ndef relay[*Ts: Movable & Deinitable](var *values: *Ts) -> Int:\n    return count(*values^, 9)\n\ndef main():\n    print(relay(1, True))\n";

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
    let src = "@fieldwise_init\nstruct Box:\n    var base: Int\n    def run(self) -> Int:\n        def count[*Ts: Movable & Deinitable](var *args: *Ts) {self} -> Int:\n            return self.base + len(args)\n        return count(1, True)\n\ndef main():\n    print(Box(40).run())\n";
    assert_eq!(run(src).unwrap(), "42\n");
}

#[test]
fn pack_forwarding_to_fixed_arity_remains_rejected() {
    let src = "def outer() -> Int:\n    def fixed(value: Int) -> Int:\n        return value\n    def relay[*Ts: Movable & Deinitable](var *args: *Ts) -> Int:\n        return fixed(*args^)\n    return relay(42)\n\ndef main():\n    print(outer())\n";
    let error = run(src).unwrap_err();
    assert!(
        error.contains("call spread outside a specialized type pack"),
        "got: {error}"
    );
}

#[test]
fn nested_pack_forwarding_preserves_nested_call_keywords_and_defaults() {
    let src = "def outer() -> Int:\n    def score[*Ts: Movable & Deinitable](head: Int, /, var *args: *Ts, scale: Int = 1) -> Int:\n        return (head + len(args)) * scale\n    return score[Int, Bool](10, 1, True, scale=3) + score[Int, Bool](10, 1, True)\n\ndef main():\n    print(outer())\n";
    assert_eq!(run(src).unwrap(), "48\n");
}

#[test]
fn heterogeneous_pack_inference_uses_only_variadic_overflow_arguments() {
    let top_level = "def count[*Ts: Movable & Deinitable](head: Int, var *args: *Ts) -> Int:\n    return head + len(args)\n\ndef main():\n    print(count(40, \"one\", True))\n";
    let nested = "def outer() -> Int:\n    def count[*Ts: Movable & Deinitable](head: Int, var *args: *Ts) -> Int:\n        return head + len(args)\n    return count(40, \"one\", True)\n\ndef main():\n    print(outer())\n";
    assert_eq!(run(top_level).unwrap(), "42\n");
    assert_eq!(run(nested).unwrap(), "42\n");
}

#[test]
fn nested_pack_named_result_is_not_part_of_the_call_abi() {
    let src = "def outer() -> Int:\n    def count[*Ts: Movable & Deinitable](out result: Int, var *args: *Ts):\n        result = len(args)\n    return count[Int, Bool](1, True)\n\ndef main():\n    print(outer())\n";
    assert_eq!(run(src).unwrap(), "2\n");
}

#[test]
fn nested_whole_pack_forwarding_can_chain_without_copying() {
    let src = "struct Item(Movable):\n    var value: Int\n    def __init__(out self, value: Int):\n        self.value = value\n    def __init__(out self, *, deinit move: Self):\n        self.value = move.value\n\ndef outer() -> Int:\n    def first[*Ts: Movable & Deinitable](var *args: *Ts) -> Int:\n        return args[0].value\n    def second[*Ts: Movable & Deinitable](var *args: *Ts) -> Int:\n        return first(*args^)\n    def third[*Ts: Movable & Deinitable](var *args: *Ts) -> Int:\n        return second(*args^)\n    return third(Item(42))\n\ndef main():\n    print(outer())\n";
    assert_eq!(run(src).unwrap(), "42\n");
}

#[test]
fn a_walrus_cannot_target_a_pack_template_name() {
    // A walrus updates a variable already in scope, so a pack template's name
    // is not a target: it never shadowed one, it is simply not a variable. The
    // pinned Mojo rejects the same programs with "expression must be mutable in
    // assignment".
    let top_level = "def choose[*Ts: Movable & Deinitable](var *args: *Ts) -> Int:\n    return len(args)\n\ndef main():\n    if True:\n        var ignored = (choose := 5)\n    print(choose(2))\n";
    let nested = "def outer():\n    def choose[*Ts: Movable & Deinitable](var *args: *Ts) -> Int:\n        return len(args)\n    if True:\n        var ignored = (choose := 5)\n    print(choose(2))\n\ndef main():\n    outer()\n";
    for source in [top_level, nested] {
        let error = run(source).unwrap_err();
        assert!(
            error.contains("cannot assign to undeclared variable 'choose'"),
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
    let src = "def outer[n: Int]() -> Int:\n    comptime if n >= 0:\n        pass\n    def nested[*InnerTypes: Movable & Deinitable](var *inner_args: *InnerTypes) -> Int:\n        return n * 10 + len(inner_args)\n    return nested(1, \"two\", True)\n\ndef main():\n    print(outer[2]())\n    print(outer[1]())\n";
    assert_eq!(run(src).unwrap(), "23\n13\n");
}

#[test]
fn nested_pack_specialization_preserves_explicit_captures() {
    let src = "def outer() -> Int:\n    var base = 40\n    def nested[*Ts: Movable & Deinitable](var *args: *Ts) {base} -> Int:\n        return base + len(args)\n    return nested(1, True)\n\ndef main():\n    print(outer())\n";
    assert_eq!(run(src).unwrap(), "42\n");
}

#[test]
fn same_spelled_nested_pack_templates_have_distinct_lexical_identities() {
    let src = "def outer():\n    if True:\n        def count[*Ts: Movable & Deinitable](var *args: *Ts) -> Int:\n            return len(args)\n        print(count(1))\n    if True:\n        def count[*Ts: Movable & Deinitable](var *args: *Ts) -> Int:\n            return 40 + len(args)\n        print(count(1, True))\n\ndef main():\n    outer()\n";
    assert_eq!(run(src).unwrap(), "1\n42\n");
}

#[test]
fn local_callable_shadows_a_top_level_pack_template_during_specialization() {
    let src = "def choose[*Ts: Movable & Deinitable](var *args: *Ts) -> Int:\n    return len(args)\n\ndef main():\n    def choose(value: Int) -> Int:\n        return value + 40\n    print(choose(2))\n";
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
fn comptime_for_variable_does_not_index_a_runtime_tuple() {
    // The body is checked once with the loop variable symbolic, as upstream
    // does, so `t[i]` is the dependent element at `i` rather than any one
    // element type, and `print` rejects it for the same reason the pin does:
    // "an element of 'values' with type 'Int, String, Bool[SIMDLength(
    // paramfor_next_value[_ZeroStartingRange](iter))]' does not conform to
    // trait 'Writable'" (observed 2026-09-20 against `dd957314`).
    let src = "def main():\n    var t: Tuple[Int, String, Bool] = (1, \"two\", True)\n    comptime for i in range(3):\n        print(t[i])\n";
    let err = run(src).unwrap_err();
    assert!(err.contains("an element of 'values'"), "{err}");
    assert!(
        err.contains("does not conform to trait 'Writable'"),
        "{err}"
    );
}

#[test]
fn non_comptime_binding_is_rejected_by_elaboration() {
    // `comptime NAME = <runtime value>` is rejected at compile-time elaboration.
    let program = parse("var x: Int = 3\ncomptime N = x\n").unwrap();
    assert!(elaborate(program).is_err());
}

#[test]
fn comptime_collection_displays_fold() {
    // Set and dictionary displays bind compile-time values; `len`, `in`,
    // and `comptime for` read them without the VM.
    let src = "comptime M = {\"a\": 1, \"b\": 2}\n\ndef main():\n    comptime S = {1, 2, 3}\n    comptime n = len(S)\n    comptime c = 2 in S\n    comptime has_b = \"b\" in M\n    print(n, c, has_b, comptime(len(M)))\n    comptime for k in M:\n        print(k)\n";
    assert_eq!(run(src).unwrap(), "3 True True 2\na\nb\n");
}

#[test]
fn comptime_collection_method_calls_run_through_the_vm() {
    // A chained method call over a compile-time dictionary is one VM-CTFE
    // entry whose result type comes from the typing probe.
    let src = "comptime M = {\"a\": 1, \"b\": 2}\n\ndef main():\n    comptime g = M.get(\"a\").value()\n    comptime d = M.get(\"zz\", 7)\n    print(g, d)\n";
    assert_eq!(run(src).unwrap(), "1 7\n");
}

#[test]
fn comptime_collection_runtime_use_is_rejected_by_elaboration() {
    // A compile-time collection is not implicitly copyable: a bare runtime
    // use needs `materialize[M]()`.
    let program = parse("comptime M = {1: 2}\n\ndef main():\n    print(len(M))\n").unwrap();
    let error = elaborate(program).unwrap_err().to_string();
    assert!(error.contains("not 'ImplicitlyCopyable'"), "got {error}");
    let program = parse("def main():\n    comptime l = [1, 2, 3]\n    print(len(l))\n").unwrap();
    let error = elaborate(program).unwrap_err().to_string();
    assert!(error.contains("Array[Int, Int(3)]"), "got {error}");
}

#[test]
fn ctfe_runs_a_pure_function_at_compile_time() {
    // A pure top-level function (loops + locals) executes at compile time.
    let src = "def next_pow2(n: Int) -> Int:\n    var p: Int = 1\n    while p < n:\n        p = p * 2\n    return p\n\ncomptime CAP = next_pow2(17)\n\ndef main():\n    comptime for i in range(CAP):\n        pass\n    print(CAP)\n";
    assert_eq!(run(src).unwrap(), "32\n");
}

#[test]
fn vm_backed_ctfe_zero_step_range_is_empty() {
    let src = "def count_iterations() -> Int:\n    var count = 0\n    for i in range(3, 0, 0):\n        count += 1\n    return count\n\ncomptime COUNT = count_iterations()\n\ndef main():\n    print(COUNT)\n";
    assert_eq!(run_compiled(src).unwrap(), "0\n");
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
    let src = "@fieldwise_init\nstruct Point:\n    var x: Int\n    var label: String\n\ndef main():\n    comptime r = reflect[Point]\n    comptime count = r.field_count()\n    comptime names = r.field_names()\n    comptime types = r.field_types()\n    print(count, names[0], names[1])\n    comptime if types[0] == Int:\n        print(\"int\")\n";
    assert_eq!(run(src).unwrap(), "2 x label\nint\n");
}

#[test]
fn reflection_supports_named_indexed_and_chainable_field_handles() {
    let src = "struct Coordinates:\n    var x: Int\n    var y: Float64\n\nstruct Point:\n    var coordinates: Coordinates\n\ndef main():\n    comptime r = reflect[Point]\n    comptime index = r.field_index[\"coordinates\"]()\n    comptime reflected = r.field[\"coordinates\"].field_at[1]\n    var value: reflected.T = 3.5\n    print(index, value)\n";
    assert_eq!(run(src).unwrap(), "0 3.5\n");
}

#[test]
fn reflection_field_handles_substitute_generic_struct_arguments() {
    let src = "@fieldwise_init\nstruct Boxed[T: Copyable & Movable & Deinitable]:\n    var value: Self.T\n\ndef main():\n    comptime reflected = reflect[Boxed[String]].field_at[0]\n    var value: reflected.T = \"generic\"\n    print(value)\n";
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
fn module_level_comptime_if_is_rejected() {
    // Upstream requires a `comptime if` inside a function, so conditionally
    // generating a declaration at module level — reflection-driven or not — is
    // not a form Mojito accepts either
    // (`assets/parse_error/comptime_if_module_level.mojo`).
    let src = "struct Unit:\n    var value: Int\n\ncomptime reflected = reflect[Unit]\ncomptime if reflected.field_count() == 1:\n    def generated() -> String:\n        return \"generated\"\nelse:\n    def generated() -> String:\n        return \"wrong\"\n\ndef main():\n    print(generated())\n";
    assert!(
        run(src)
            .unwrap_err()
            .contains("'comptime if' must be contained in a function")
    );
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
        error.contains("element 2 of type pack 'ArgTypes' has type 'StringLiteral'"),
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
    // A folded `comptime if Types[0] == Int` does not narrow the element: it
    // keeps its dependent type, and `rebind[Int]` is the explicit retyping
    // both compilers demand (`assets/ok/pack_element_rebind.mojo`). Verified
    // on the pin 2026-09-20: both print `5` then `0`.
    let src = "def first_plus_one[*Types: Copyable](*args: *Types) -> Int:\n    comptime if Types[0] == Int:\n        return rebind[Int](args[0]) + 1\n    else:\n        return 0\n\ndef main():\n    print(first_plus_one(4, \"tail\"))\n    print(first_plus_one(\"head\", 4))\n";
    assert_eq!(run(src).unwrap(), "5\n0\n");
}

#[test]
fn variadic_value_pack_specializes_and_unrolls() {
    let src = "def total[*values: Int]() -> Int:\n    var result = 0\n    comptime for value in values:\n        result = result + value\n    return result\n\ndef main():\n    print(total[1, 2, 3, 4]())\n";
    assert_eq!(run(src).unwrap(), "10\n");
}

#[test]
fn type_predicate_selects_comptime_branch() {
    // Upstream's type comparison (`T == Int`) lets a `comptime if` branch on a
    // type parameter — `name[Int]` takes the `int` branch, `name[String]` the
    // `other` branch (each a distinct specialization).
    let src = "def name[T: AnyType]() -> String:\n    comptime if T == Int:\n        return \"int\"\n    else:\n        return \"other\"\n\ndef main():\n    print(name[Int]())\n    print(name[String]())\n";
    assert_eq!(run(src).unwrap(), "int\nother\n");
}

#[test]
fn type_predicate_in_runtime_if_is_rejected() {
    // A type comparison has no runtime `Bool` form — used in a runtime `if` (not
    // a `comptime if`) it is not a resolvable value, so the program is rejected.
    let src = "def name[T: AnyType]() -> String:\n    if T == Int:\n        return \"int\"\n    else:\n        return \"other\"\n\ndef main():\n    print(name[Int]())\n";
    assert!(run(src).is_err());
}

#[test]
fn type_and_value_predicates_compose() {
    // A mixed type+value generic: the type comparison picks the outer branch and
    // the value-parameter predicate the inner one, each resolved per
    // instantiation.
    let src = "def tag[T: AnyType, n: Int]() -> String:\n    comptime if T == Int:\n        comptime if n == 0:\n            return \"int-zero\"\n        else:\n            return \"int-n\"\n    else:\n        return \"other\"\n\ndef main():\n    print(tag[Int, 0]())\n    print(tag[Int, 5]())\n    print(tag[String, 0]())\n";
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
    let src = "def choose[origin: Origin[mut=True], *Ts: Copyable & Deinitable](ref[origin] value: Int, var *args: *Ts) -> ref[origin] Int:\n    comptime for i in range(args.__len__()):\n        pass\n    return value\n\ndef main():\n    var value = 40\n    ref result = choose[origin_of(value), Int, Bool](value, 1, True)\n    result += 2\n    print(value)\n";
    assert_eq!(run(src).unwrap(), "42\n");
}

#[test]
fn type_pack_specialization_accepts_named_origin_after_pack() {
    let src = "def choose[*Ts: Copyable & Deinitable, origin: Origin[mut=True]](ref[origin] value: Int, var *args: *Ts) -> ref[origin] Int:\n    comptime for i in range(args.__len__()):\n        pass\n    return value\n\ndef main():\n    var value = 40\n    ref result = choose[Int, Bool, origin=origin_of(value)](value, 1, True)\n    result += 2\n    print(value)\n";
    assert_eq!(run(src).unwrap(), "42\n");
}

#[test]
fn type_pack_specialization_skips_infer_only_origin_before_pack() {
    let src = "def choose[origin: Origin[mut=True], //, *Ts: Copyable & Deinitable](ref[origin] value: Int, var *args: *Ts) -> ref[origin] Int:\n    comptime for i in range(args.__len__()):\n        pass\n    return value\n\ndef main():\n    var value = 40\n    ref result = choose[Int, Bool](value, 1, True)\n    result += 2\n    print(value)\n";
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
fn value_struct_specialization_validates_a_diagnostic_where_clause() {
    let template = "@fieldwise_init\nstruct Window[size: Int] where (size > 0, \"size must be positive\"):\n    var value: Int\n\n";
    assert_eq!(
        run(&format!(
            "{template}def main():\n    var value = Window[1](42)\n    print(value.value)\n"
        ))
        .unwrap(),
        "42\n"
    );

    let error = run(&format!(
        "{template}def main():\n    var value = Window[0](42)\n    print(value.value)\n"
    ))
    .unwrap_err();
    assert!(
        error.contains("constraint failed: size must be positive"),
        "got: {error}"
    );
}

#[test]
fn variadic_struct_specialization_folds_a_diagnostic_where_clause() {
    let template = "@fieldwise_init\nstruct CopyPack[*Ts: AnyType] where (conforms_to(Ts.values, Copyable), \"pack elements must be Copyable\"):\n    var values: Tuple[*Self.Ts]\n\n";
    assert_eq!(
        run(&format!(
            "{template}def main():\n    var value = CopyPack[Int, Bool]((1, True))\n    print(value.values[0])\n"
        ))
        .unwrap(),
        "1\n"
    );

    let error = run(&format!(
        "{template}struct Token(Movable):\n    var value: Int\n\ndef main():\n    var value: CopyPack[Token]\n"
    ))
    .unwrap_err();
    assert!(
        error.contains("constraint failed: pack elements must be Copyable"),
        "got: {error}"
    );
}

#[test]
fn variadic_struct_specializes_with_per_index_typed_storage() {
    // `p.storage[0]` has the exact element type (Int here), so it participates
    // in Int arithmetic; `p.storage[1]` is exactly Bool.
    let src = "@fieldwise_init\nstruct Pair[*Ts: Copyable & Movable](Copyable, Movable):\n    var storage: Tuple[*Self.Ts]\n\ndef main():\n    var p = Pair[Int, Bool]((1, True))\n    var n: Int = p.storage[0] + 41\n    var b: Bool = p.storage[1]\n    print(n)\n    print(b)\n";
    assert_eq!(run(src).unwrap(), "42\nTrue\n");
}

#[test]
fn variadic_struct_element_type_mismatch_is_rejected() {
    // Per-index typing is exact: reading the Int element into a Bool is a type
    // error, not a common-bound erasure.
    let src = "@fieldwise_init\nstruct Pair[*Ts: Copyable & Movable](Copyable, Movable):\n    var storage: Tuple[*Self.Ts]\n\ndef main():\n    var p = Pair[Int, Bool]((1, True))\n    var b: Bool = p.storage[0]\n    print(b)\n";
    let err = run(src).unwrap_err();
    assert!(err.contains("expected Bool, found Int"), "got: {err}");
}

#[test]
fn variadic_struct_distinct_instantiations_coexist() {
    // Two specializations of one template are distinct concrete structs with
    // independent field types (regression: annotation sites keyed by span
    // collided across specializations sharing the template's span).
    let src = "@fieldwise_init\nstruct Pair[*Ts: Copyable & Movable](Copyable, Movable):\n    var storage: Tuple[*Self.Ts]\n\ndef main():\n    var a = Pair[Int, Bool]((1, True))\n    var b = Pair[Int, Int]((2, 3))\n    var c = Pair[String]((\"solo\",))\n    print(a.storage[0] + b.storage[1])\n    print(c.storage[0])\n";
    assert_eq!(run(src).unwrap(), "4\nsolo\n");
}

#[test]
fn variadic_struct_annotations_and_methods_use_the_specialization() {
    // The struct type appears in a def parameter annotation (rewritten to the
    // specialized struct), and a concrete method runs against the expanded
    // storage.
    let src = "@fieldwise_init\nstruct Pair[*Ts: Copyable & Movable](Copyable, Movable):\n    var storage: Tuple[*Self.Ts]\n\n    def size(self) -> Int:\n        return len(self.storage)\n\ndef first_int(p: Pair[Int, Bool]) -> Int:\n    return p.storage[0]\n\ndef main():\n    var p: Pair[Int, Bool] = Pair[Int, Bool]((1, True))\n    var q = p.copy()\n    print(first_int(q))\n    print(q.size())\n";
    assert_eq!(run(src).unwrap(), "1\n2\n");
}

#[test]
fn variadic_struct_requires_explicit_type_arguments() {
    let src = "@fieldwise_init\nstruct Pair[*Ts: Copyable & Movable](Copyable, Movable):\n    var storage: Tuple[*Self.Ts]\n\ndef main():\n    var p = Pair((1, True))\n    print(p.storage[0])\n";
    let err = run(src).unwrap_err();
    assert!(
        err.contains("variadic struct 'Pair' requires explicit compile-time type arguments"),
        "got: {err}"
    );
}

#[test]
fn variadic_struct_bare_template_use_is_rejected() {
    let src = "@fieldwise_init\nstruct Pair[*Ts: Copyable & Movable](Copyable, Movable):\n    var storage: Tuple[*Self.Ts]\n\ndef main():\n    var x = Pair\n    print(\"unreachable\")\n";
    let err = run(src).unwrap_err();
    assert!(
        err.contains("variadic struct 'Pair' requires explicit compile-time type arguments"),
        "got: {err}"
    );
}

#[test]
fn variadic_struct_supports_exactly_one_pack() {
    // One trailing pack and no other compile-time parameters (current scope).
    let src = "@fieldwise_init\nstruct Bad[T: Copyable & Movable, *Ts: Copyable & Movable](Copyable, Movable):\n    var storage: Tuple[*Self.Ts]\n\ndef main():\n    var x = Bad[Int, Bool]((True,))\n    print(\"unreachable\")\n";
    let err = run(src).unwrap_err();
    assert!(
        err.contains("supports exactly one type-parameter pack"),
        "got: {err}"
    );
}

#[test]
fn variadic_struct_runtime_index_is_rejected() {
    let src = "@fieldwise_init\nstruct Pair[*Ts: Copyable & Movable](Copyable, Movable):\n    var storage: Tuple[*Self.Ts]\n\ndef main():\n    var p = Pair[Int, Bool]((1, True))\n    var i = 0\n    print(p.storage[i])\n";
    let err = run(src).unwrap_err();
    assert!(err.contains("compile-time Int index"), "got: {err}");
}

#[test]
fn variadic_struct_pack_init_constructs_per_position() {
    // Real Mojo's Tuple constructor shape: `var *args: *Ts` binds the
    // heterogeneous pack (each argument checked against its per-index element
    // type) and `Tuple(*args^)` transfers the elements into storage.
    let src = "struct Pair[*Ts: Copyable & Movable](Copyable, Movable):\n    var storage: Tuple[*Self.Ts]\n\n    def __init__(out self, var *args: *Self.Ts):\n        self.storage = Tuple(*args^)\n\n    def size(self) -> Int:\n        return len(self.storage)\n\ndef main():\n    var p = Pair[Int, String, Bool](7, \"x\", False)\n    print(p.size())\n    print(p.storage[0])\n    print(p.storage[1])\n    print(p.storage[2])\n";
    assert_eq!(run(src).unwrap(), "3\n7\nx\nFalse\n");
}

#[test]
fn variadic_struct_pack_init_rejects_wrong_arity_and_types() {
    let template = "struct Pair[*Ts: Copyable & Movable](Copyable, Movable):\n    var storage: Tuple[*Self.Ts]\n\n    def __init__(out self, var *args: *Self.Ts):\n        self.storage = Tuple(*args^)\n\n";
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
    let src = "struct Pair[*Ts: Copyable & Movable](Copyable, Movable):\n    var storage: Tuple[*Self.Ts]\n\n    def __init__(out self, var *args: *Self.Ts):\n        self.storage = Tuple(*args^)\n\n    def __getitem__[i: Int](self) -> Self.Ts[i]:\n        return self.storage[i].copy()\n\n    def __len__(self) -> Int:\n        return len(self.storage)\n\ndef main():\n    var p = Pair[Int, String, Bool](7, \"mid\", True)\n    var n: Int = p[0]\n    var s: String = p[1]\n    var b: Bool = p[2]\n    print(n)\n    print(s)\n    print(b)\n    print(len(p))\n";
    assert_eq!(run_compiled(src).unwrap(), "7\nmid\nTrue\n3\n");
}

#[test]
fn current_getitem_param_hook_handles_places_and_rvalues() {
    // Current Mojo spells a compile-time parameter subscript hook
    // `__getitem_param__`. A place preserves its reference result, while an
    // implicitly-copyable rvalue uses the generated value-returning twin.
    let src = "struct CurrentPair[*Ts: ImplicitlyCopyable & Copyable & Movable](Copyable, Movable):\n    var storage: Tuple[*Self.Ts]\n\n    def __init__(out self, var *args: *Self.Ts):\n        self.storage = Tuple(*args^)\n\n    def __getitem_param__[i: Int](ref self) -> ref[origin_of(self)] Self.Ts[i]:\n        return self.storage[i]\n\ndef main():\n    var pair = CurrentPair[Int, String](7, \"current\")\n    print(pair[0], pair[1])\n    print(CurrentPair[Int, String](9, \"rvalue\")[0])\n";
    assert_eq!(run_compiled(src).unwrap(), "7 current\n9\n");
}

#[test]
fn current_getitem_param_reference_result_can_bind_an_explicit_ref() {
    let src = "struct CurrentPair[*Ts: ImplicitlyCopyable & Copyable & Movable](Copyable, Movable):\n    var storage: Tuple[*Self.Ts]\n\n    def __init__(out self, var *args: *Self.Ts):\n        self.storage = Tuple(*args^)\n\n    def __getitem_param__[i: Int](ref self) -> ref[origin_of(self)] Self.Ts[i]:\n        return self.storage[i]\n\ndef main():\n    var pair = CurrentPair[Int, String](7, \"current\")\n    ref alias = pair[0]\n    alias += 5\n    print(alias)\n    print(pair[0])\n";
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
    let src = "struct Pair[*Ts: Copyable & Movable](Copyable, Movable):\n    var storage: Tuple[*Self.Ts]\n\n    def __init__(out self, var *args: *Self.Ts):\n        self.storage = Tuple(*args^)\n\n    def __getitem__[i: Int](self) -> Self.Ts[i]:\n        return self.storage[i].copy()\n\ndef main():\n    var a = Pair[Int, Bool](1, True)\n    var b = Pair[String, Int](\"s\", 5)\n    print(a[0] + b[1])\n    print(b[0])\n";
    assert_eq!(run_compiled(src).unwrap(), "6\ns\n");
}

#[test]
fn variadic_struct_dependent_getitem_rejects_bad_indices() {
    let template = "struct Pair[*Ts: Copyable & Movable](Copyable, Movable):\n    var storage: Tuple[*Self.Ts]\n\n    def __init__(out self, var *args: *Self.Ts):\n        self.storage = Tuple(*args^)\n\n    def __getitem__[i: Int](self) -> Self.Ts[i]:\n        return self.storage[i].copy()\n\n";
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
    let src = "struct NoCopy(Movable):\n    var x: Int\n\n    def __init__(out self, x: Int):\n        self.x = x\n\nstruct Pair[*Ts: Copyable & Movable](Copyable, Movable):\n    var storage: Tuple[*Self.Ts]\n\n    def __init__(out self, var *args: *Self.Ts):\n        self.storage = Tuple(*args^)\n\ndef main():\n    var p = Pair[NoCopy, Int](NoCopy(1), 2)\n    print(p.storage[1])\n";
    let err = run(src).unwrap_err();
    assert!(err.contains("not Copyable"), "got: {err}");
}

#[test]
fn associated_type_facts_request_nested_variadic_struct_specializations() {
    let src = "@fieldwise_init\nstruct Nested[*Ts: Movable](Movable):\n    pass\n\n@fieldwise_init\nstruct Family[*Ts: Movable](Movable):\n    comptime NestedType = Nested[*Self.Ts]\n    var marker: Int\n\ndef main():\n    var value = Family[Int, Bool](42)\n    print(value.marker)\n";
    assert_eq!(run(src).unwrap(), "42\n");
}

#[test]
fn explicit_type_argument_naming_a_non_generic_struct_specializes() {
    // Regression: the retained `[Plain]` argument on the rewritten call used to
    // be misresolved by the checker as an undefined value. A concrete type
    // argument is now baked into the clone and dropped from the call.
    let src = "@fieldwise_init\nstruct Plain(Copyable, Movable):\n    var n: Int\n\ndef pick[T: Movable](x: T) -> Int:\n    comptime if T == Plain:\n        return 1\n    else:\n        return 0\n\ndef main():\n    print(pick[Plain](Plain(3)))\n    print(pick[Int](4))\n";
    assert_eq!(run(src).unwrap(), "1\n0\n");
}

#[test]
fn explicit_type_argument_bound_violation_is_reported_at_the_call() {
    // A dropped type argument is never re-validated by the checker against the
    // residual signature, so the elaborator enforces the parameter's trait
    // bounds when the instantiation is requested.
    let src = "struct Pinned:\n    var n: Int\n    def __init__(out self, n: Int):\n        self.n = n\n\ndef pick[T: Copyable](x: T) -> Int:\n    comptime if T == Int:\n        return 1\n    else:\n        return 0\n\ndef main():\n    print(pick[Pinned](Pinned(3)))\n";
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
fn bound_generic_template_reports_abstract_body_errors() {
    // A plain trait-bound generic keeps its template alongside the clone the
    // explicit application mints, so a body the parameter's bounds cannot
    // support fails on `T` — upstream's pre-instantiation error — even though
    // the instantiation itself is concrete.
    let src = "@fieldwise_init\nstruct Plain(Copyable, Movable):\n    var n: Int\n\ndef broken[T: Movable](x: T) -> Int:\n    return x.definitely_missing_member\n\ndef main():\n    print(broken[Plain](Plain(3)))\n";
    let error = run(src).unwrap_err();
    assert!(
        error.contains("type 'T' has no field 'definitely_missing_member'"),
        "got: {error}"
    );
}

#[test]
fn bound_generic_template_survives_for_inferred_calls() {
    // Mixed usage of one bound generic: the explicit application monomorphizes
    // while the inferred call stays on the retained template's abstract
    // erased-dispatch path.
    let src = "def ident[T: ImplicitlyCopyable & Movable](x: T) -> T:\n    return x\n\ndef main():\n    print(ident[Int](1))\n    print(ident(2))\n";
    assert_eq!(run(src).unwrap(), "1\n2\n");
}

#[test]
fn conflicting_unrolled_inferred_calls_keep_the_abstract_path() {
    // Two `comptime for` copies of a pack walk share one source occurrence
    // with different inferred instantiations; both stay on the retained template's erased
    // dispatch and still run.
    let src = "def ident[T: ImplicitlyCopyable & Movable](x: T) -> T:\n    return x\n\ndef walk[*Ts: ImplicitlyCopyable & Writable & Deinitable](*args: *Ts):\n    comptime for i in range(Ts.length):\n        print(ident(args[i]))\n\ndef main():\n    walk(1, \"s\")\n";
    assert_eq!(run(src).unwrap(), "1\ns\n");
}

#[test]
fn trivially_predicates_fold_in_comptime_control() {
    let src = "struct Plain(Copyable):\n    var x: Int\n    def __init__(out self, x: Int):\n        self.x = x\n\nstruct Custom(Copyable):\n    var x: Int\n    def __init__(out self, x: Int):\n        self.x = x\n    def __init__(out self, *, copy: Self):\n        self.x = copy.x + 100\n\ndef main():\n    comptime if IsTriviallyCopyable[Int]:\n        print(\"int trivial\")\n    comptime if IsTriviallyCopyable[Plain]:\n        print(\"plain trivial\")\n    comptime if not IsTriviallyCopyable[Custom]:\n        print(\"custom user copy\")\n    comptime if IsTriviallyDeinitable[Plain]:\n        print(\"plain deinit trivial\")\n    comptime if not IsTriviallyDeinitable[String]:\n        print(\"string deinit nontrivial\")\n    comptime if IsTriviallyMovable[Plain]:\n        print(\"plain move trivial\")\n";
    assert_eq!(
        run(src).expect("run"),
        "int trivial\nplain trivial\ncustom user copy\nplain deinit trivial\nstring deinit nontrivial\nplain move trivial\n"
    );
}

#[test]
fn trivially_predicates_recurse_through_fields() {
    // A field whose type defines a user copy constructor defeats the outer
    // struct's triviality even though the outer struct synthesizes its copy.
    let src = "struct Custom(Copyable):\n    var x: Int\n    def __init__(out self, x: Int):\n        self.x = x\n    def __init__(out self, *, copy: Self):\n        self.x = copy.x\n\n@fieldwise_init\nstruct Outer(Copyable):\n    var inner: Custom\n\n@fieldwise_init\nstruct Simple(Copyable):\n    var a: Int\n    var b: Bool\n\ndef main():\n    comptime if not IsTriviallyCopyable[Outer]:\n        print(\"outer nontrivial\")\n    comptime if IsTriviallyCopyable[Simple]:\n        print(\"simple trivial\")\n";
    assert_eq!(run(src).expect("run"), "outer nontrivial\nsimple trivial\n");
}

#[test]
fn trivially_predicate_binds_as_comptime_value() {
    let src =
        "def main():\n    comptime trivially = IsTriviallyMovable[Int]\n    print(trivially)\n";
    assert_eq!(run(src).expect("run"), "True\n");
}

#[test]
fn trivial_register_passable_conformance_satisfies_the_predicates() {
    // The predicate is `conforms_to(T, TrivialRegisterPassable)` OR the
    // structural check: a declared conformance wins even when a user copy
    // constructor defeats the compiler-generated-lifecycle requirement.
    let src = "struct P(TrivialRegisterPassable, Copyable):\n    var x: Int\n    def __init__(out self, x: Int):\n        self.x = x\n    def __init__(out self, *, copy: Self):\n        self.x = copy.x + 1\n\ndef main():\n    comptime if IsTriviallyCopyable[P]:\n        print(\"trp trivial\")\n";
    assert_eq!(run(src).expect("run"), "trp trivial\n");
}

#[test]
fn pre_rename_trivially_spelling_no_longer_resolves() {
    // Upstream renamed the predicates without deprecated aliases, so the old
    // spelling is an ordinary unknown name.
    let src = "def main():\n    comptime if TriviallyCopyable[Int]:\n        print(\"trivial\")\n";
    let error = run(src).expect_err("old spelling must fail");
    assert!(
        error.contains("unknown type 'TriviallyCopyable'"),
        "unexpected error: {error}"
    );
}

#[test]
fn module_comptime_binding_does_not_shadow_specialized_type_parameters() {
    // A module-level `comptime T` must not substitute into a same-named
    // type parameter retained on a specialized generic def clone. std.memory.alloc's
    // `unsafe_alloc[T]` is prelude-linked, so before the specializer removed
    // its own compile-time parameter names from the materialization
    // substitution, any user constant named `T` corrupted the clone's
    // `Pointer[T, MutUntrackedOrigin]` annotation.
    let src = "def pure(a: Int) -> Int:\n    return a + 1\n\ncomptime T = pure(4)\n\ndef main():\n    print(T)\n";
    let output = run(src).expect("a module comptime binding named T must elaborate");
    assert!(output.starts_with("5\n"), "unexpected output: {output}");
}

#[test]
fn nested_tuple_type_arguments_resolve_in_every_position() {
    // A `Tuple[...]` application resolves as a nested type argument (the
    // nullary default over a nested element, a `T: Defaultable` bound), and
    // bare `Tuple(...)` calls nest without an annotation: the specialization
    // key spells a minted element canonically across discovery rounds.
    let src = "def make[T: Defaultable]() -> T:\n    return T()\n\ndef main():\n    var t = Tuple[Int, Tuple[Int, Bool]]()\n    print(t[0], t[1][0], t[1][1])\n    var x = Tuple(1, True)\n    var u = Tuple(x, 2)\n    print(u[0][0], u[0][1], u[1])\n    var v = Tuple(Tuple(3, False), 4)\n    print(v[1])\n    var m = make[Tuple[Int, Tuple[Int]]]()\n    print(m[1][0])\n";
    assert_eq!(run_compiled(src).unwrap(), "0 0 False\n1 True 2\n4\n0\n");
    let err = run_compiled("def main():\n    var t = Tuple[Int, Tuple[3]]()\n    print(t[0])\n")
        .unwrap_err();
    assert!(err.contains("expected a type, found a value"), "{err}");
}

#[test]
fn stored_array_and_span_iterators_iterate_themselves() {
    let src = "def main():\n    var a = [1, 2]\n    var ai = a.__iter__()\n    for x in ai:\n        print(x)\n    var xs: List[Int] = [5, 6]\n    var sp = Span(xs)\n    var it = sp.__iter__()\n    for y in it:\n        print(y)\n";
    assert_eq!(run(src).unwrap(), "1\n2\n5\n6\n");
    let mutation = "def main():\n    var xs: List[Int] = [1, 2]\n    var sp = Span(xs)\n    var it = sp.__iter__()\n    xs.append(9)\n    print(it.__len__())\n";
    let err = run(mutation).unwrap_err();
    assert!(err.contains("conflicts with live reference"), "{err}");
}

#[test]
fn conditional_conformance_refines_a_comptime_alias_body() {
    // A comptime alias a conditional trait requires (`Iterable where
    // conforms_to(T, Copyable)` → `IteratorType`) resolves under that
    // condition; without the conditional trait the alias body rejects.
    let iter = "from std.iter import Iterable, Iterator, StopIteration\n\nstruct MyIter[T: Copyable & Movable, o: Origin[mut=False]](Iterator, Copyable, Movable):\n    comptime Element = Self.T\n    var src: Pointer[Self.T, Self.o]\n    var done: Bool\n\n    def __init__(out self, ref[Self.o] src: Self.T):\n        self.src = Pointer(to=src)\n        self.done = False\n\n    def __next__(mut self) raises StopIteration -> Self.T:\n        if self.done:\n            raise StopIteration()\n        self.done = True\n        return self.src[].copy()\n\n";
    let alias = "    comptime IteratorType[\n        iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]\n    ] = MyIter[Self.T, iterable_origin]\n";
    let src = format!(
        "{iter}struct Bag[T: Movable & Deinitable](Iterable where conforms_to(T, Copyable), Movable):\n    comptime Element = Self.T\n{alias}    var v: Self.T\n\n    def __init__(out self, var v: Self.T):\n        self.v = v^\n\n    def __iter__(ref self) -> Self.IteratorType[origin_of(self.v)] where conforms_to(Self.T, Copyable):\n        return MyIter(self.v)\n\ndef main():\n    var b = Bag(7)\n    for x in b:\n        print(x)\n"
    );
    assert_eq!(run(&src).unwrap(), "7\n");
    let negative = format!(
        "{iter}struct Bag[T: Movable & Deinitable](Movable):\n{alias}    var v: Self.T\n\n    def __init__(out self, var v: Self.T):\n        self.v = v^\n\ndef main():\n    print(1)\n"
    );
    let err = run(&negative).unwrap_err();
    assert!(
        err.contains("does not conform to trait 'Copyable'"),
        "{err}"
    );
}

#[test]
fn variadic_struct_members_spell_the_struct_pack_through_self() {
    // Upstream's rule: inside a struct's members the struct's own pack is
    // `Self.Ts`; a bare `Ts` in a field type, a `comptime` member, a method
    // signature, an availability clause, or a body reports upstream's text.
    let header =
        "struct Bag[*Ts: Movable](Deinitable where Ts.all_conforms_to[Deinitable](), Movable):\n";
    let init = "    def __init__(out self, var *args: *Self.Ts):\n        self.storage = Tuple[*Self.Ts](*args^)\n";
    let main = "\ndef main():\n    var b = Bag[Int, Bool](1, True)\n    print(b.storage[0])\n";
    let bare = [
        format!("{header}    var storage: Tuple[*Ts]\n{init}{main}"),
        format!(
            "{header}    comptime element_types = Ts\n    var storage: Tuple[*Self.Ts]\n{init}{main}"
        ),
        format!(
            "{header}    var storage: Tuple[*Self.Ts]\n    def __init__(out self, var *args: *Ts):\n        self.storage = Tuple[*Self.Ts](*args^)\n{main}"
        ),
        format!(
            "{header}    var storage: Tuple[*Self.Ts]\n{init}    def first[i: Int](self) -> Ts[i]:\n        return self.storage[i]\n{main}"
        ),
        format!(
            "{header}    var storage: Tuple[*Self.Ts]\n{init}    def count(self) -> Int where Ts.all_conforms_to[Copyable]():\n        return Self.Ts.length\n{main}"
        ),
        format!(
            "{header}    var storage: Tuple[*Self.Ts]\n{init}    def count(self) -> Int:\n        comptime for i in range(len(Ts)):\n            pass\n        return Self.Ts.length\n{main}"
        ),
    ];
    for src in &bare {
        let err = run(src).unwrap_err();
        assert!(
            err.contains("unqualified access to struct parameter 'Ts'; use 'Self.Ts' instead"),
            "{src}\n{err}"
        );
    }
}

#[test]
fn struct_header_and_method_own_packs_keep_the_bare_name() {
    // The header has no `Self`: conformance clauses and the trailing `where`
    // name the pack bare. A method's own pack is its own binding, and
    // `conforms_to(Self.Ts.values, X)` on a method folds like
    // `Self.Ts.all_conforms_to[X]()`.
    let src = "struct Bag[*Ts: Movable](\n    Copyable where Ts.all_conforms_to[Copyable](),\n    Deinitable where Ts.all_conforms_to[Deinitable](),\n    Movable,\n) where (conforms_to(Ts.values, Movable), \"pack elements must be Movable\"):\n    var storage: Tuple[*Self.Ts]\n\n    def __init__(out self, var *args: *Self.Ts):\n        self.storage = Tuple[*Self.Ts](*args^)\n\n    def count(self) -> Int where conforms_to(Self.Ts.values, Copyable):\n        return Self.Ts.length\n\n    def tally[*Us: Movable](self, var *extra: *Us) -> Int:\n        var total = Self.Ts.length\n        comptime for i in range(Us.length):\n            total += 1\n        return total\n\ndef main():\n    var b = Bag[Int, Bool](1, True)\n    print(b.count())\n    print(b.tally(7, \"x\", False))\n";
    assert_eq!(run(src).unwrap(), "2\n5\n");
}

#[test]
fn callable_contract_binder_shadows_the_enclosing_binder() {
    // `apply`'s `T` and its contract's `T` are different parameters: the
    // clone substituting `apply`'s `T` leaves the contract generic, so the
    // call through `f` still infers the contract's own binder. The pinned
    // Mojo accepts and prints the same.
    let src = "def ident[T: Copyable & Deinitable](x: T) -> T:\n    return x.copy()\n\ndef apply[T: Copyable & Deinitable, F: def[T: Copyable & Deinitable](T) -> T](f: F, x: T) -> T:\n    return f(x)\n\ndef outer[T: Copyable & Deinitable](x: T) -> T:\n    return apply(ident, x)\n\ndef main():\n    print(outer(9))\n    print(outer(String(\"q\")))\n    print(apply(ident, 4))\n";
    assert_eq!(run_compiled(src).unwrap(), "9\nq\n4\n");
}
