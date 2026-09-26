use mojito::{Compiler, CompilerError, SemanticAdjustment, Value, ValueCategory};

#[test]
fn compiler_driver_runs_the_authoritative_pipeline() {
    let compiler = Compiler::default();
    let program = compiler
        .compile_unlinked("comptime n = 2 + 3\ndef main():\n    var x: Int = n\n    print(x)\n")
        .expect("compile");
    let execution = compiler.execute(&program).expect("execute");
    assert_eq!(execution.output, "5\n");
    assert!(execution.bindings.iter().any(|(name, value)| {
        name == "n" && matches!(value, Value::IntLiteral(value) if value.to_i64() == Some(5))
    }));
}

#[test]
fn compiler_materializes_only_closed_public_tuple_signatures() {
    let compiler = Compiler::default();
    let program = compiler
        .compile_unlinked(
            "def quotient_rem[T: DivModable](a: T, b: T) -> Tuple[T, T]:\n    return divmod(a, b)\n\ndef main():\n    var first: Tuple[Int, Int] = quotient_rem(7, 2)\n    var second: Tuple[Int, Int] = quotient_rem(-7, 2)\n    print(first[0], first[1])\n    print(second[0], second[1])\n",
        )
        .expect("a generic Tuple signature waits for concrete call-site substitution");
    let execution = compiler.execute(&program).expect("execute divmod tuples");
    assert_eq!(execution.output, "3 1\n-4 1\n");
}

#[test]
fn compiler_driver_reports_the_failing_stage() {
    let compiler = Compiler::default();
    let error = compiler
        .compile_unlinked("def bad() -> Int:\n    return missing\n")
        .expect_err("type error");
    assert!(matches!(error, CompilerError::Type(_)));

    let error = compiler
        .compile_unlinked(
            "@fieldwise_init\nstruct P:\n    var x: Int\ndef main():\n    var p: P = P(1)\n    var q: P = p^\n    print(p.x)\n",
        )
        .expect_err("ownership error");
    assert!(matches!(error, CompilerError::Ownership(_)));
}

#[test]
fn compiler_rejects_executable_file_scope() {
    let compiler = Compiler::default();
    let error = compiler
        .compile_unlinked("var x: Int = 1\nprint(x)\n")
        .expect_err("file-scope execution must be rejected");
    assert!(matches!(
        error,
        CompilerError::Type(mojito::TypeError::InvalidModuleScope(_))
    ));
}

#[test]
fn checked_boundary_carries_types_categories_edges_and_adjustments() {
    let program = Compiler::default()
        .compile_unlinked(
            "def choose(value: Int) -> Int:\n    return value\ndef choose(value: String) -> Int:\n    return value.byte_length()\ndef main():\n    var result: Int = choose(42)\n    print(result)\n",
        )
        .expect("compile");
    let expressions = program.checked().expressions();
    assert!(
        expressions
            .iter()
            .all(|node| node.id.0 < expressions.len() as u32)
    );
    assert!(
        expressions.iter().all(|node| {
            node.ty.is_some()
                || matches!(
                    node.category,
                    ValueCategory::Type | ValueCategory::CompileTime
                )
        }),
        "runtime checked expressions must carry types: {expressions:#?}"
    );
    assert!(expressions.iter().any(|node| {
        node.ty.as_ref().is_some_and(|ty| ty.to_string() == "Int")
            && node.category == ValueCategory::Place
    }));
    assert!(expressions.iter().any(|node| {
        node.adjustments
            .iter()
            .any(|adjustment| matches!(adjustment, SemanticAdjustment::ResolveCallable(_)))
    }));
    assert!(
        expressions
            .iter()
            .flat_map(|node| &node.children)
            .all(|child| (child.0 as usize) < expressions.len())
    );
}

#[test]
fn checked_hir_and_mir_retain_selected_trait_call_effects() {
    let program = Compiler::default()
        .compile_unlinked(
            "trait Fallible:\n    def run(self) raises -> Int: ...\n\n@fieldwise_init\nstruct Failure(Fallible):\n    var code: Int\n    def run(self) raises -> Int:\n        raise \"failed\"\n        return self.code\n\ndef invoke[T: Fallible](value: T) raises -> Int:\n    return value.run()\n\ndef main():\n    try:\n        var ignored = invoke(Failure(1))\n    except error:\n        pass\n",
        )
        .expect("compile trait effect program");

    let checked_call = program.checked().expressions().iter().find(|expression| {
        matches!(
            &expression.syntax.kind,
            mojito::ast::ExprKind::MethodCall { method, .. } if method == "run"
        )
    });
    assert_eq!(
        checked_call
            .and_then(|expression| expression.effects.raises.as_ref())
            .map(ToString::to_string),
        Some("Error".to_string())
    );

    let mir = mojito::mir::lower_checked_program(program.checked());
    assert!(mir.functions.iter().any(|(_, function)| {
        function.blocks.iter().any(|block| {
            block.instrs.iter().any(|instruction| {
                matches!(
                    instruction,
                    mojito::mir::MirInstr::MethodCall {
                        method,
                        raises: Some(error),
                        ..
                    } if method == "run" && error.to_string() == "Error"
                )
            })
        })
    }));
}

#[test]
fn generic_struct_instances_get_per_instantiation_method_clones() {
    // A closed application of an ordinary generic struct reached from user
    // code mints one clone per available method on the template, checked
    // with `self` bound to the instance: `_unqualified_type_name[Self]`
    // spells the instantiation and a `comptime if` on `Self.T` folds, while
    // the template keeps its erased pre-check and the runtime name stays the
    // template's. Calls on the instance retarget to the clone by exact name.
    let compiler = Compiler::default();
    let program = compiler
        .compile_source(
            "from std.reflection.type_info import _unqualified_type_name\n\nstruct Box[T: Copyable & Deinitable](Copyable, Movable):\n    var value: Self.T\n\n    def __init__(out self, var value: Self.T):\n        self.value = value^\n\n    def type_name(self) -> String:\n        return String(_unqualified_type_name[Self]())\n\n    def kind(self) -> String:\n        comptime if Self.T == Int:\n            return String(\"int box\")\n        else:\n            return String(\"other box\")\n\ndef main():\n    var a = Box[Int](7)\n    var b = Box(String(\"seven\"))\n    print(a.type_name(), b.type_name())\n    print(a.kind(), b.kind())\n",
            std::path::Path::new("/tmp/mojito_generic_struct_instances.mojo"),
        )
        .expect("compile the instance clones");
    let targets = program.checked().overload_targets();
    assert!(
        targets
            .values()
            .any(|target| target == "Box.type_name$y3:Int"),
        "the Int instance's call retargets to its clone: {targets:?}"
    );
    assert!(
        targets
            .values()
            .any(|target| target.starts_with("Box.kind$y") && target.contains("String")),
        "the String instance's call retargets to its clone: {targets:?}"
    );
    let mir = mojito::mir::lower_checked_program(program.checked());
    let names: Vec<&String> = mir.functions.iter().map(|(name, _)| name).collect();
    assert!(names.iter().any(|name| *name == "Box.type_name$y3:Int"));
    assert!(names.iter().any(|name| *name == "Box.type_name"));
    let output = compiler.execute(&program).expect("run the instance clones");
    assert_eq!(
        output.output,
        "Box[SIMD[DType.int, 1]] Box[String]\nint box other box\n"
    );
}

#[test]
fn overloaded_constructor_family_clones_as_one_overload_set() {
    // A generic struct's constructors clone together: each signature gets its
    // own member of the instance's `__init__$y3:Int` family, and the checker
    // names the member it selected — the clone family's `$ov$` suffixes key on
    // the substituted parameter types, so they do not correspond by name to
    // the template's.
    let compiler = Compiler::default();
    let program = compiler
        .compile_source(
            "struct Box[T: Copyable & Deinitable](Copyable, Movable):\n    var value: Self.T\n\n    def __init__(out self, var value: Self.T):\n        self.value = value^\n\n    def __init__(out self, var value: Self.T, twice: Bool):\n        self.value = value^\n\n    def kind(self) -> String:\n        comptime if Self.T == Int:\n            return String(\"int\")\n        else:\n            return String(\"other\")\n\ndef main():\n    var a = Box[Int](7)\n    var b = Box[Int](8, True)\n    print(a.kind(), b.kind())\n",
            std::path::Path::new("/tmp/mojito_constructor_family_clones.mojo"),
        )
        .expect("compile the constructor family");
    let clone_family = |name: &&String| name.starts_with("Box.__init__$y3:Int$ov$");
    let targets = program.checked().overload_targets();
    let selected: std::collections::BTreeSet<&String> = targets
        .values()
        .filter(|target| clone_family(target))
        .collect();
    assert_eq!(
        selected.len(),
        2,
        "each construction names its own clone: {targets:?}"
    );
    let mir = mojito::mir::lower_checked_program(program.checked());
    let names: Vec<&String> = mir.functions.iter().map(|(name, _)| name).collect();
    let defined: std::collections::BTreeSet<&&String> =
        names.iter().filter(|name| clone_family(name)).collect();
    assert_eq!(
        defined.len(),
        2,
        "the family lowers as an overload set: {names:?}"
    );
    for target in selected {
        assert!(
            names.contains(&target),
            "the selected clone '{target}' is lowered: {names:?}"
        );
    }
    let output = compiler
        .execute(&program)
        .expect("run the constructor family");
    assert_eq!(output.output, "int int\n");
}

#[test]
fn instance_clones_serve_operators_display_and_iteration() {
    // Every call shape on a closed generic-struct instance reaches the
    // instance's clone: the `==` dunder target, `len(x)`, `repr(x)` and
    // `print(x)` (the VM formats through the clone named by the argument's
    // checked static type), `List[Int]` subscript assignment, and the
    // `for` loop's `__iter__` prepare symbol.
    let compiler = Compiler::default();
    let program = compiler
        .compile_source(
            "from std.reflection.type_info import _unqualified_type_name\n\nstruct Box[T: Copyable & Deinitable](Copyable, Equatable where conforms_to(T, Equatable), Movable, Sized, Writable where conforms_to(T, Writable)):\n    var value: Self.T\n\n    def __init__(out self, var value: Self.T):\n        self.value = value^\n\n    def __eq__(self, other: Self) -> Bool where conforms_to(Self.T, Equatable):\n        return self.value == other.value\n\n    def __len__(self) -> Int:\n        comptime if Self.T == Int:\n            return 1\n        else:\n            return 2\n\n    def write_repr_to(self, mut writer: Some[Writer]) where conforms_to(Self.T, Writable):\n        writer.write(_unqualified_type_name[Self](), \"(\", repr(self.value), \")\")\n\ndef main():\n    var a = Box[Int](7)\n    print(a == Box[Int](7), len(a), repr(a))\n    var xs: List[Int] = [1, 2]\n    xs[0] = 5\n    var total = 0\n    for x in xs:\n        total += x\n    print(total)\n",
            std::path::Path::new("/tmp/mojito_instance_clone_dispatch.mojo"),
        )
        .expect("compile the instance dispatch program");
    let targets = program.checked().overload_targets();
    assert!(
        targets.values().any(|target| target == "Box.__eq__$y3:Int"),
        "the operator retargets to the instance clone: {targets:?}"
    );
    assert!(
        targets
            .values()
            .any(|target| target == "List.__setitem__$y3:Int"),
        "subscript assignment retargets to the instance clone: {targets:?}"
    );
    let iterates_through_clone = program.checked().expressions().iter().any(|expression| {
        expression.adjustments.iter().any(|adjustment| {
            matches!(
                adjustment,
                mojito::checked::SemanticAdjustment::Iterate(protocol)
                    if protocol.prepare.iter().any(|symbol| symbol.starts_with("List.__iter__$y3:Int"))
            )
        })
    });
    assert!(
        iterates_through_clone,
        "the for loop prepares through the instance clone"
    );
    let output = compiler
        .execute(&program)
        .expect("run the instance dispatch program");
    assert_eq!(output.output, "True 1 Box[SIMD[DType.int, 1]](Int(7))\n7\n");
}

#[test]
fn linked_std_utils_variant_constructs_tests_projects_and_sets() {
    let compiler = Compiler::default();
    let program = compiler
        .compile_source(
            "from std.utils import Variant\n\ndef main():\n    var numeric = Variant[Int, UInt](1)\n    print(numeric.isa[Int](), numeric.isa[UInt]())\n    var value: Variant[Int, String] = Variant[Int, String](7)\n    print(value.isa[Int]())\n    print(value[Int])\n    value.set[String](\"mojo\")\n    print(value.isa[String]())\n    print(value[String])\n",
            std::path::Path::new("/tmp/mojito_variant_completion.mojo"),
        )
        .expect("compile linked Variant");
    let execution = compiler.execute(&program).expect("execute Variant");
    assert_eq!(execution.output, "True False\nTrue\n7\nTrue\nmojo\n");
    assert!(program.checked().expressions().iter().any(|expression| {
        expression.adjustments.iter().any(|adjustment| {
            matches!(
                adjustment,
                SemanticAdjustment::ConstructVariant { index: 0, .. }
            )
        })
    }));
    assert!(program.checked().expressions().iter().any(|expression| {
        expression
            .adjustments
            .iter()
            .any(|adjustment| matches!(adjustment, SemanticAdjustment::VariantSet { index: 1, .. }))
    }));
}

#[test]
fn variant_pack_forwarding_through_a_generic_def_runs() {
    // `Variant` is an ordinary variadic struct applied over an enclosing
    // generic's own parameters: the pack-forwarding clone's expanded
    // signature (`-> Variant[Int, String]`) requests the concrete
    // specialization, and the retained bound-generic template checks
    // `Variant[T, String]` against the template's shell.
    let compiler = Compiler::default();
    let program = compiler
        .compile_source(
            "from std.utils import Variant\n\ndef first_variant[*Ts: Movable]() -> Variant[*Ts]:\n    return Variant[*Ts](3)\n\ndef wrap[T: Movable](var x: T) -> Variant[T, String]:\n    return Variant[T, String](x^)\n\ndef main():\n    var value = first_variant[Int, String]()\n    print(value.isa[Int]())\n    var wrapped = wrap(True)\n    print(wrapped.isa[Bool](), wrapped.isa[String]())\n",
            std::path::Path::new("/tmp/mojito_variant_type_pack.mojo"),
        )
        .expect("variadic-struct pack forwarding through generic defs");
    let output = compiler.execute(&program).expect("run the forwarded packs");
    assert_eq!(output.output, "True\nTrue False\n");
}

#[test]
fn inferred_comptime_keyed_def_call_in_an_abstract_body_is_accepted() {
    // `show(x)` in `forward`'s own body infers `T := T`, which no discovery
    // round can serve. The call stays on `show`'s stub, but `forward`'s
    // abstract body runs only through an abstract reference, and there is
    // none. The closed call in `main` runs through its clone.
    let compiler = Compiler::default();
    let program = compiler
        .compile_source(
            "def show[T: Copyable](x: T):\n    comptime if T == Int:\n        print(\"int\")\n\ndef forward[T: Copyable](x: T):\n    show(x)\n\ndef main():\n    show(3)\n",
            std::path::Path::new("/tmp/mojito_abstract_comptime_keyed_call.mojo"),
        )
        .expect("an abstract inferred call to a compile-time-keyed def");
    let output = compiler.execute(&program).expect("run the served call");
    assert_eq!(output.output, "int\n");
}

#[test]
fn inferred_comptime_keyed_def_reached_from_a_function_value_is_rejected() {
    // Passing `forward` as a function value leaves it on its abstract path,
    // so its body's call to `show`'s stub could run: the fixpoint rejects
    // the function-value use rather than trap at run time.
    let compiler = Compiler::default();
    let error = compiler
        .compile_source(
            "def show[T: Copyable](x: T):\n    comptime if T == Int:\n        print(\"int\")\n\ndef forward[T: Copyable](x: T):\n    show(x)\n\ndef apply(f: def (Int) -> None, x: Int):\n    f(x)\n\ndef main():\n    apply(forward, 3)\n",
            std::path::Path::new("/tmp/mojito_unserved_comptime_keyed_call.mojo"),
        )
        .expect_err("a function-value use reaching a compile-time-keyed stub");
    let CompilerError::Comptime(error) = error else {
        panic!("expected a compile-time rejection, got {error}");
    };
    assert_eq!(
        error.to_string(),
        "compile-time call arity: generic 'forward' requires compile-time parameter 'T'"
    );
}

#[test]
fn a_generic_struct_method_calls_a_comptime_keyed_def_through_its_clones() {
    // `Box[T].f` keys nothing itself, but the `show` it calls does: the
    // erased template body keeps the call on `show`'s stub, and each closed
    // instance reaches its own clone of `f`, where `Self.T` is concrete.
    let compiler = Compiler::default();
    let program = compiler
        .compile_source(
            "def show[T: Copyable](x: T):\n    comptime if T == Int:\n        print(\"int\")\n    else:\n        print(\"other\")\n\nstruct Box[T: Copyable & Deinitable](Deinitable):\n    var x: Self.T\n\n    def __init__(out self, x: Self.T):\n        self.x = x.copy()\n\n    def f(self):\n        show(self.x)\n\ndef main():\n    Box[Int](3).f()\n    Box[Float64](1.5).f()\n",
            std::path::Path::new("/tmp/mojito_struct_method_comptime_keyed_call.mojo"),
        )
        .expect("a generic struct method calling a compile-time-keyed def");
    let output = compiler.execute(&program).expect("run both instances");
    assert_eq!(output.output, "int\nother\n");
}

#[test]
fn an_unavailable_method_does_not_reject_its_instance() {
    // `f` reaches `show`'s stub, and `Box[Plain]` mints no clone of it — but
    // only because its `where` clause is false there, so no call can reach
    // the erased body either. Withholding a method is not failing to serve
    // it, and the program the pin accepts must still compile.
    let compiler = Compiler::default();
    let program = compiler
        .compile_source(
            "def show[T: Copyable](x: T):\n    comptime if T == Int:\n        print(\"int\")\n    else:\n        print(\"other\")\n\n@fieldwise_init\nstruct Plain(Copyable, ImplicitlyCopyable, Movable, Deinitable):\n    var n: Int\n\nstruct Box[T: Copyable & Deinitable](Deinitable):\n    var x: Self.T\n\n    def __init__(out self, x: Self.T):\n        self.x = x.copy()\n\n    def f(self) where conforms_to(Self.T, Writable):\n        show(self.x)\n\ndef main():\n    var b = Box[Plain](Plain(1))\n    print(\"built\")\n",
            std::path::Path::new("/tmp/mojito_struct_method_unavailable_instance.mojo"),
        )
        .expect("an instance that withholds the stub-reaching method");
    let output = compiler.execute(&program).expect("run the program");
    assert_eq!(output.output, "built\n");
}

#[test]
fn a_nested_generic_def_reaches_a_comptime_keyed_clone() {
    // `inner` keys nothing itself, so nothing used to specialize it and its
    // `show(y)` stayed on the stub. Reaching a stub now makes it specialize
    // per call, and each instance's body reaches `show`'s own clone.
    let compiler = Compiler::default();
    let program = compiler
        .compile_source(
            "def show[T: Copyable](x: T):\n    comptime if T == Int:\n        print(\"int\")\n    else:\n        print(\"other\")\n\ndef main():\n    def inner[U: Copyable](y: U):\n        show(y)\n    inner(2)\n    inner(1.5)\n",
            std::path::Path::new("/tmp/mojito_nested_comptime_keyed_call.mojo"),
        )
        .expect("a nested generic def calling a compile-time-keyed def");
    let output = compiler.execute(&program).expect("run both instances");
    assert_eq!(output.output, "int\nother\n");
}

#[test]
fn an_inferred_nested_comptime_keyed_def_specializes_per_call() {
    // The lexical pass resolves a nested call from source syntax alone, so an
    // inferred `keyed(2)` had no arguments to select an arm with. The
    // template now stands for the discovery check, which records the
    // instantiation the next round mints.
    let compiler = Compiler::default();
    let program = compiler
        .compile_source(
            "def main():\n    def keyed[U: Copyable](y: U):\n        comptime if U == Int:\n            print(\"int\")\n        else:\n            print(\"other\")\n    keyed(2)\n    keyed(True)\n    keyed[Int](7)\n",
            std::path::Path::new("/tmp/mojito_nested_keyed_def.mojo"),
        )
        .expect("an inferred call to a compile-time-keyed nested def");
    let output = compiler.execute(&program).expect("run every instance");
    assert_eq!(output.output, "int\nother\nint\n");
}

#[test]
fn a_nested_generic_def_in_a_generic_body_reaches_the_clone() {
    // The enclosing body specializes first, so the nested `def` is registered
    // and scanned once per clone of `outer` — each with its own source, and
    // therefore its own recorded instantiation of `show`.
    let compiler = Compiler::default();
    let program = compiler
        .compile_source(
            "def show[T: Copyable](x: T):\n    comptime if T == Int:\n        print(\"int\")\n    else:\n        print(\"other\")\n\ndef outer[T: Copyable](x: T):\n    def inside[U: Copyable](y: U):\n        show(y)\n    inside(x)\n\ndef main():\n    outer(3)\n    outer(1.5)\n",
            std::path::Path::new("/tmp/mojito_nested_in_generic_body.mojo"),
        )
        .expect("a nested generic def inside a generic def");
    let output = compiler.execute(&program).expect("run both clones");
    assert_eq!(output.output, "int\nother\n");
}

#[test]
fn a_nested_generic_def_declared_but_never_called_is_accepted() {
    // Nothing runs the body, so nothing has to reach `show`'s clone: the
    // template is dead and disappears, as an uncalled top-level one does.
    let compiler = Compiler::default();
    let program = compiler
        .compile_source(
            "def show[T: Copyable](x: T):\n    comptime if T == Int:\n        print(\"int\")\n\ndef main():\n    def inner[U: Copyable](y: U):\n        show(y)\n    print(\"done\")\n",
            std::path::Path::new("/tmp/mojito_nested_uncalled.mojo"),
        )
        .expect("an uncalled nested generic def reaching a stub");
    let output = compiler.execute(&program).expect("run the program");
    assert_eq!(output.output, "done\n");
}

#[test]
fn view_temporaries_live_for_their_statement() {
    // A `ref[self]`-returning call chains on a temporary receiver (the
    // temporary is materialized for the statement), a discarded reference
    // result needs no copy, and a subscript view passed straight to a method
    // at its source's last use keeps the source alive through the call.
    let compiler = Compiler::default();
    let program = compiler
        .compile_source(
            "from std.format._utils import FormatStruct\n\n@fieldwise_init\nstruct P(Writable, Movable):\n    var x: Int\n    def write_to(self, mut writer: Some[Writer]):\n        FormatStruct(writer, \"P\").params(1).fields(self.x)\n\ndef main():\n    print(P(3))\n    var s = String(\"hello\")\n    var out = String()\n    out.write(s[byte=1:3])\n    print(out)\n",
            std::path::Path::new("/tmp/mojito_view_temporaries.mojo"),
        )
        .expect("view temporaries");
    let output = compiler
        .execute(&program)
        .expect("run the view temporaries");
    assert_eq!(output.output, "P[1](3)\nel\n");
}

#[test]
fn view_temporary_argument_conflicts_with_a_later_source_mutation() {
    // A subscript view temporary passed as an argument keeps a loan on its
    // source for the statement: a later argument mutating the source
    // conflicts with it. (Pinned here, on the linked pipeline, because the
    // `assets/ownership_error` seam group checks without the prelude, where
    // the nominal `String` is not a known type.)
    let compiler = Compiler::default();
    let error = compiler
        .compile_source(
            "def grow(mut s: String) -> Int:\n    s += \"zz\"\n    return 1\n\ndef main():\n    var s = String(\"hello\")\n    var out = String()\n    out.write(s[byte=1:3], grow(s))\n    print(out)\n",
            std::path::Path::new("/tmp/mojito_view_temporary_source_mutation.mojo"),
        )
        .expect_err("the view temporary's loan must conflict with the mutation");
    assert!(matches!(error, CompilerError::Ownership(_)), "{error:?}");
    assert!(
        error.to_string().contains("conflicts with live reference"),
        "{error}"
    );
}

#[test]
fn generic_methods_specialize_per_call_on_every_struct() {
    // A method-level type parameter on an ordinary generic instance folds
    // its `comptime if` per call (the clone bakes the instance's argument
    // before the call's), and a method-level pack on a non-generic struct
    // is inferred from the overflow arguments and expanded in its clone.
    let compiler = Compiler::default();
    let program = compiler
        .compile_source(
            "struct Box[T: Copyable & Deinitable](Copyable, Movable):\n    var value: Self.T\n\n    def __init__(out self, var value: Self.T):\n        self.value = value^\n\n    def kind[U: AnyType](self) -> Int:\n        comptime if U == Self.T:\n            return 2\n        comptime if U == Int:\n            return 1\n        return 0\n\nstruct Plain(Movable):\n    var n: Int\n\n    def __init__(out self):\n        self.n = 0\n\n    def fields[*Ts: Writable](mut self, *args: *Ts):\n        comptime for i in range(Ts.length):\n            self.n += 1\n\ndef main():\n    var b = Box[Bool](True)\n    print(b.kind[Int](), b.kind[Bool](), b.kind[String]())\n    var p = Plain()\n    p.fields(1, \"a\", True)\n    print(p.n)\n",
            std::path::Path::new("/tmp/mojito_per_call_clones.mojo"),
        )
        .expect("per-call method clones");
    let output = compiler.execute(&program).expect("run the per-call clones");
    assert_eq!(output.output, "1 2 0\n3\n");
}

#[test]
fn variant_requires_import_and_checks_projection_tags() {
    let compiler = Compiler::default();
    let unimported = compiler
        .compile_unlinked("def main():\n    var value = Variant[Int, String](7)\n")
        .expect_err("Variant is not a prelude type");
    assert!(matches!(
        unimported,
        CompilerError::Type(mojito::TypeError::UndefinedVariable(name)) if name == "Variant"
    ));

    let program = compiler
        .compile_source(
            "from std.utils import Variant\n\ndef main():\n    var value = Variant[Int, String](7)\n    print(value[String])\n",
            std::path::Path::new("/tmp/mojito_variant_wrong_tag.mojo"),
        )
        .expect("wrong active tag is a runtime check");
    let error = compiler
        .execute(&program)
        .expect_err("typed projection must check the active tag");
    assert!(matches!(
        error,
        CompilerError::Runtime(mojito::RuntimeError::TypeError(message))
            if message.contains("holds 'Int', not 'String'")
    ));
}

#[test]
fn variant_type_queries_take_and_replace_have_checked_ownership_semantics() {
    let compiler = Compiler::default();
    let program = compiler
        .compile_source(
            "from std.utils import Variant\n\ndef main():\n    var value = Variant[Int, String](7)\n    print(value.is_type_supported[Int](), value.is_type_supported[Float64]())\n    var old = value.replace[String, Int](\"seven\")\n    print(old, value[String])\n    var taken = value.unwrap[String]()\n    print(taken)\n    var unchecked = Variant[Int, String](9)\n    var unsafe_old = unchecked.unsafe_replace[String, Int](\"nine\")\n    var unsafe_taken = unchecked.unsafe_unwrap[String]()\n    print(unsafe_old, unsafe_taken)\n",
            std::path::Path::new("/tmp/mojito_variant_take_replace.mojo"),
        )
        .expect("compile Variant take/replace operations");
    let execution = compiler
        .execute(&program)
        .expect("execute Variant take/replace operations");
    assert_eq!(execution.output, "True False\n7 seven\nseven\n9 nine\n");

    let unsupported = compiler
        .compile_source(
            "from std.utils import Variant\n\ndef main():\n    var value = Variant[Int, String](7)\n    _ = value.unwrap[Float64]()\n",
            std::path::Path::new("/tmp/mojito_variant_unsupported_take.mojo"),
        )
        .expect_err("unsupported Variant operation arm must be rejected statically");
    assert!(matches!(unsupported, CompilerError::Type(_)));

    // `unwrap` takes `deinit self`: a plain-local receiver without `^` is an
    // implicit copy, legal only when the Variant is `ImplicitlyCopyable`
    // (every alternative is) — there is no last-use move, as upstream. A
    // transferred (`^`) receiver makes any later use an ownership error.
    let implicit_copy = compiler
        .compile_source(
            "from std.utils import Variant\n\ndef main():\n    var value = Variant[Int, List[Int]](7)\n    _ = value.unwrap[Int]()\n",
            std::path::Path::new("/tmp/mojito_variant_implicit_copy_take.mojo"),
        )
        .expect_err("a non-ImplicitlyCopyable Variant receiver must be transferred");
    assert!(matches!(
        implicit_copy,
        CompilerError::Type(mojito::TypeError::ImplicitCopy { .. })
    ));
    let moved = compiler
        .compile_source(
            "from std.utils import Variant\n\ndef main():\n    var value = Variant[Int, List[Int]](7)\n    _ = value^.unwrap[Int]()\n    print(value.isa[Int]())\n",
            std::path::Path::new("/tmp/mojito_variant_use_after_take.mojo"),
        )
        .expect_err("Variant.unwrap consumes its transferred receiver");
    assert!(matches!(moved, CompilerError::Ownership(_)));
    let copied = compiler
        .compile_source(
            "from std.utils import Variant\n\ndef main():\n    var value = Variant[Int, String](7)\n    print(value.unwrap[Int](), value.isa[Int]())\n",
            std::path::Path::new("/tmp/mojito_variant_copy_before_take.mojo"),
        )
        .expect("an ImplicitlyCopyable Variant copies its consumed receiver");
    assert_eq!(
        compiler.execute(&copied).expect("execute").output,
        "7 True\n"
    );

    let wrong_tag = compiler
        .compile_source(
            "from std.utils import Variant\n\ndef main():\n    var value = Variant[Int, String](7)\n    _ = value.unwrap[String]()\n",
            std::path::Path::new("/tmp/mojito_variant_wrong_take_tag.mojo"),
        )
        .expect("a checked take validates its dynamic tag at runtime");
    let wrong_tag = compiler
        .execute(&wrong_tag)
        .expect_err("checked Variant.take must trap on a tag mismatch");
    assert!(matches!(
        wrong_tag,
        CompilerError::Runtime(mojito::RuntimeError::TypeError(message))
            if message.contains("holds 'Int', not 'String'")
    ));

    let wrong_replace_tag = compiler
        .compile_source(
            "from std.utils import Variant\n\ndef main():\n    var value = Variant[Int, String](7)\n    _ = value.replace[String, String](\"replacement\")\n",
            std::path::Path::new("/tmp/mojito_variant_wrong_replace_tag.mojo"),
        )
        .expect("a checked replace validates its dynamic output tag at runtime");
    let wrong_replace_tag = compiler
        .execute(&wrong_replace_tag)
        .expect_err("checked Variant.replace must trap on a tag mismatch");
    assert!(matches!(
        wrong_replace_tag,
        CompilerError::Runtime(mojito::RuntimeError::TypeError(message))
            if message.contains("holds 'Int', not 'String'")
    ));
}

#[test]
fn variant_protocols_are_conditioned_on_every_alternative() {
    let compiler = Compiler::default();
    let program = compiler
        .compile_source(
            "from std.utils import Variant\n\n@fieldwise_init\nstruct Styled(Writable):\n    var value: Int\n    def write_to(self, mut writer: Some[Writer]):\n        writer.write(\"styled=\", self.value)\n    def write_repr_to(self, mut writer: Some[Writer]):\n        writer.write(\"Styled[\", self.value, \"]\")\n\ndef main():\n    var left = Variant[Int, UInt](7)\n    var same = Variant[Int, UInt](7)\n    var other = Variant[Int, UInt](UInt(7))\n    print(hash(left) == hash(same), hash(left) == hash(other))\n    var styled = Variant[Styled, Int](Styled(4))\n    print(String(styled), repr(styled))\n    var copied = left\n    print(copied == left)\n",
            std::path::Path::new("/tmp/mojito_variant_protocols.mojo"),
        )
        .expect("all alternatives satisfy the requested Variant protocols");
    let execution = compiler
        .execute(&program)
        .expect("execute conditional Variant protocols");
    assert_eq!(
        execution.output,
        "True False\nstyled=4 Variant[Styled, SIMD[DType.int, 1]](Styled[4])\nTrue\n"
    );

    for (name, body, expected_trait) in [
        ("hash", "print(hash(value))", "Hashable"),
        ("write", "print(value)", "Writable"),
        ("equality", "print(value == value)", "Equatable"),
    ] {
        let source = format!(
            "from std.utils import Variant\n\n@fieldwise_init\nstruct Opaque:\n    var value: Int\n\ndef main():\n    var value = Variant[Int, Opaque](Opaque(1))\n    {body}\n"
        );
        let error = compiler
            .compile_source(
                &source,
                std::path::Path::new(&format!("/tmp/mojito_variant_non_{name}.mojo")),
            )
            .expect_err("one unsupported alternative must disable the protocol");
        match expected_trait {
            "Equatable" | "Writable" => assert!(matches!(error, CompilerError::Type(_))),
            trait_name => assert!(matches!(
                error,
                CompilerError::Type(mojito::TypeError::TraitNotSatisfied {
                    trait_name: found,
                    ..
                }) if found == trait_name
            )),
        }
    }

    let noncopyable = compiler
        .compile_source(
            "from std.utils import Variant\n\n@fieldwise_init\nstruct MoveOnly:\n    var value: Int\n\ndef main():\n    var value = Variant[Int, MoveOnly](MoveOnly(1))\n    var copied = value\n    print(copied.isa[MoveOnly]())\n",
            std::path::Path::new("/tmp/mojito_variant_noncopyable.mojo"),
        )
        .expect_err("a Variant is Copyable only when every alternative is Copyable");
    assert!(matches!(noncopyable, CompilerError::Type(_)));

    let nondeletable = compiler
        .compile_source(
            "from std.utils import Variant\n\nstruct Linear(Deinitable where False):\n    pass\n\ndef require_deletable[T: Deinitable]():\n    pass\n\ndef main():\n    require_deletable[Variant[Int, Linear]]()\n",
            std::path::Path::new("/tmp/mojito_variant_nondeletable.mojo"),
        )
        .expect_err("a Variant is deletable only when every alternative is deletable");
    assert!(matches!(nondeletable, CompilerError::Type(_)));
}

#[test]
fn pipeline_verifies_typed_mir_before_execution() {
    // The verification stage sits between checking and ownership: a healthy
    // program compiles, and the dedicated error variant renders findings as a
    // compiler invariant report rather than a user diagnostic.
    let compiler = Compiler::default();
    compiler
        .compile_source(
            "def main():\n    var x = 1\n    print(x)\n",
            std::path::Path::new("/tmp/mojito_verify_stage.mojo"),
        )
        .expect("a checked program passes MIR verification");
    let rendered = CompilerError::Verify(vec![
        "fn 'main': register r1 has no checked type".to_string(),
    ])
    .to_string();
    assert!(rendered.contains("invalid checked program"));
    assert!(rendered.contains("register r1"));
}

#[test]
fn inferred_polymorphic_recursion_reports_specialization_divergence() {
    // Each discovery round's clone records one deeper `List[…]` instantiation,
    // so the request set never stops growing; the round cap converts that into
    // a dedicated diagnostic instead of an endless compile.
    let compiler = Compiler::default();
    let error = compiler
        .compile_unlinked(
            "def wrap[T: Copyable & Movable](x: T, depth: Int) -> Int:\n    if depth <= 0:\n        return 0\n    return wrap([x.copy()], depth - 1)\n\ndef main():\n    print(wrap(1, 3))\n",
        )
        .expect_err("inferred polymorphic recursion cannot converge");
    assert!(
        matches!(error, CompilerError::SpecializationDivergence { .. }),
        "{error}"
    );
    let message = error.to_string();
    assert!(message.contains("'wrap'"), "{message}");
    assert!(message.contains("did not converge"), "{message}");
}

#[test]
fn callee_stores_transfer_loans_to_caller_bookkeeping() {
    // A callee's accepted store of a loan-carrying value into a `mut`
    // receiver or parameter records a transfer effect; call sites replay it,
    // so returning the destination while a transferred loan roots at a local
    // rejects at the return boundary. Runs through the compiler so the
    // linked stdlib's seeded `List.append` effect participates.
    let compiler = Compiler::default();
    for source in [
        // via the seeded stdlib effect
        "@fieldwise_init\nstruct RefBox[origin: Origin[mut=True]]:\n    var value: ref[origin] List[Int]\n\ndef make() -> List[RefBox[MutUnsafeAnyOrigin]]:\n    var sink = List[RefBox[MutUnsafeAnyOrigin]]()\n    var local: List[Int] = [9]\n    ref alias = local\n    sink.append(RefBox(alias))\n    return sink^\n\ndef main():\n    var got = make()\n",
        // via a transitively derived free-function effect
        "@fieldwise_init\nstruct RefBox[origin: Origin[mut=True]]:\n    var value: ref[origin] List[Int]\n\ndef stash(mut sink: List[RefBox[MutUnsafeAnyOrigin]], var box: RefBox[MutUnsafeAnyOrigin]):\n    sink.append(box^)\n\ndef collect() -> List[RefBox[MutUnsafeAnyOrigin]]:\n    var sink = List[RefBox[MutUnsafeAnyOrigin]]()\n    var local: List[Int] = [9]\n    ref alias = local\n    stash(sink, RefBox(alias))\n    return sink^\n\ndef main():\n    var got = collect()\n",
    ] {
        let error = compiler
            .compile_unlinked(source)
            .expect_err("transferred local-rooted loan must not escape");
        assert!(
            matches!(error, CompilerError::Type(_))
                && error.to_string().contains("escapes storage"),
            "{error}"
        );
    }
}

#[test]
fn overloaded_call_sites_replay_the_shared_effect_entry() {
    // Overloaded free functions share the bare-name effect entry; selecting
    // an overload replays it exactly like the single-callable path (this
    // was a silent pre-existing gap: the overload branch skipped effect
    // replay entirely). The seeded `List.append` chain requires the linked
    // compiler.
    let compiler = Compiler::default();
    let source = "@fieldwise_init\nstruct RefBox[origin: Origin[mut=True]]:\n    var value: ref[origin] List[Int]\n\ndef stash(mut sink: List[RefBox[MutUnsafeAnyOrigin]], var box: RefBox[MutUnsafeAnyOrigin]):\n    sink.append(box^)\n\ndef stash(x: Int):\n    print(x)\n\ndef collect() -> List[RefBox[MutUnsafeAnyOrigin]]:\n    var sink = List[RefBox[MutUnsafeAnyOrigin]]()\n    var local: List[Int] = [9]\n    ref alias = local\n    stash(sink, RefBox(alias))\n    return sink^\n\ndef main():\n    var got = collect()\n";
    let error = compiler
        .compile_unlinked(source)
        .expect_err("transferred local-rooted loan must not escape");
    assert!(
        matches!(error, CompilerError::Type(_)) && error.to_string().contains("escapes storage"),
        "{error}"
    );
}

#[test]
fn callable_struct_call_replays_transfer_effects() {
    // An indirect call through a callable-struct value replays the
    // `Struct.__call__` transfer effects: the seeded `List.append` inside
    // the body transfers the argument's loans onto the `mut` sink actual,
    // so mutating the loan source while the sink lives conflicts. Runs
    // through the compiler so the seeded stdlib effect participates.
    let compiler = Compiler::default();
    let conflict = "@fieldwise_init\nstruct RefBox[origin: Origin[mut=True]]:\n    var value: ref[origin] List[Int]\n\n@fieldwise_init\nstruct Stasher(def(mut List[RefBox[MutUnsafeAnyOrigin]], RefBox[MutUnsafeAnyOrigin])):\n    var count: Int\n    def __call__(mut self, mut sink: List[RefBox[MutUnsafeAnyOrigin]], box: RefBox[MutUnsafeAnyOrigin]):\n        self.count += 1\n        sink.append(box^)\n\ndef main():\n    var s = Stasher(0)\n    var sink = List[RefBox[MutUnsafeAnyOrigin]]()\n    var local: List[Int] = [9]\n    ref alias = local\n    s(sink, RefBox(alias))\n    local.append(1)\n    print(sink[0].value[0])\n";
    let error = compiler
        .compile_unlinked(conflict)
        .expect_err("mutating the transferred loan's source must conflict");
    assert!(
        matches!(error, CompilerError::Ownership(_))
            && error
                .to_string()
                .contains("conflicts with live reference 'sink'"),
        "{error}"
    );

    // The same program with the sink's last use before the mutation stays
    // accepted — no spurious rejection from the indirect replay.
    let after_last_use = "@fieldwise_init\nstruct RefBox[origin: Origin[mut=True]]:\n    var value: ref[origin] List[Int]\n\n@fieldwise_init\nstruct Stasher(def(mut List[RefBox[MutUnsafeAnyOrigin]], RefBox[MutUnsafeAnyOrigin])):\n    var count: Int\n    def __call__(mut self, mut sink: List[RefBox[MutUnsafeAnyOrigin]], box: RefBox[MutUnsafeAnyOrigin]):\n        self.count += 1\n        sink.append(box^)\n\ndef main():\n    var s = Stasher(0)\n    var sink = List[RefBox[MutUnsafeAnyOrigin]]()\n    var local: List[Int] = [9]\n    ref alias = local\n    s(sink, RefBox(alias))\n    print(sink[0].value[0])\n    local.append(1)\n    print(local[1])\n";
    compiler
        .compile_unlinked(after_last_use)
        .expect("carrier released before the mutation compiles");
}

#[test]
fn owned_iteration_requires_deinitable_elements() {
    // Current Mojo bounds owned iteration at `Movable & Deinitable` elements.
    // A linear List rejects at iterator selection (the bundled
    // `__iter__(var self)` where clause fails for the specialization), a user
    // iterator yielding linear elements rejects at the element gate, and both
    // reject regardless of exhaustion — the pre-alignment linear-element
    // extension is gone.
    let compiler = Compiler::default();
    let exhaustive = "@explicit_destroy(\"close Conn\")\nstruct Conn(Movable, Deinitable where False):\n    var id: Int\n\n    def __init__(out self, id: Int):\n        self.id = id\n\n    def close(deinit self):\n        print(\"close\", self.id)\n\ndef main():\n    var conns: List[Conn] = [Conn(1), Conn(2)]\n    for var item in conns^:\n        item^.close()\n";
    let escaping = format!("{exhaustive}        break\n");
    for source in [exhaustive.to_string(), escaping] {
        let error = compiler
            .compile_unlinked(&source)
            .expect_err("linear owned iteration");
        let CompilerError::Type(mojito::TypeError::Unsupported(message)) = error else {
            panic!("expected the owned-iteration bound rejection, got {error:?}");
        };
        assert!(
            message.contains("requires 'Movable & Deinitable' elements"),
            "{message}"
        );
    }

    let user_iterator = "@explicit_destroy(\"close Conn\")\nstruct Conn(Movable, Deinitable where False):\n    var id: Int\n\n    def __init__(out self, id: Int):\n        self.id = id\n\n    def close(deinit self):\n        print(\"close\", self.id)\n\nstruct Drain(Iterator, Movable):\n    comptime Element = Conn\n    var remaining: Int\n\n    def __init__(out self, remaining: Int):\n        self.remaining = remaining\n\n    def __next__(mut self) raises StopIteration -> Conn:\n        if self.remaining == 0:\n            raise StopIteration()\n        self.remaining -= 1\n        return Conn(self.remaining)\n\nstruct Bucket(Movable):\n    var count: Int\n\n    def __init__(out self, count: Int):\n        self.count = count\n\n    def __iter__(var self) -> Drain:\n        return Drain(self.count)\n\ndef main():\n    var bucket = Bucket(2)\n    for var item in bucket^:\n        item^.close()\n";
    let error = compiler
        .compile_unlinked(user_iterator)
        .expect_err("linear user iterator");
    let CompilerError::Type(mojito::TypeError::Unsupported(message)) = error else {
        panic!("expected the owned-iteration element gate, got {error:?}");
    };
    assert!(
        message.contains("non-Deinitable 'Conn' cannot be consumed implicitly"),
        "{message}"
    );
    // The rejection names the element's declared obligation.
    assert!(message.contains("(close Conn)"), "{message}");
}

#[test]
fn owned_pack_iteration_still_forwards_linear_elements() {
    // Variadic packs are not library iterators: linear whole-pack forwarding
    // stays supported under guaranteed exhaustion, and the escape guard still
    // rejects an abandoning exit with the element's obligation named.
    let compiler = Compiler::default();
    let exhaustive = "@explicit_destroy(\"close Conn\")\nstruct Conn(Movable, Deinitable where False):\n    var id: Int\n\n    def __init__(out self, id: Int):\n        self.id = id\n\n    def close(deinit self):\n        print(\"close\", self.id)\n\ndef consume(var *conns: Conn):\n    for var item in conns^:\n        item^.close()\n\ndef main():\n    consume(Conn(1), Conn(2))\n";
    let program = compiler
        .compile_unlinked(exhaustive)
        .expect("linear pack exhaustive");
    let execution = compiler.execute(&program).expect("execute");
    assert_eq!(execution.output, "close 1\nclose 2\n");

    let escaping = exhaustive.replace(
        "        item^.close()\n",
        "        item^.close()\n        break\n",
    );
    let error = compiler
        .compile_unlinked(&escaping)
        .expect_err("linear pack escaping");
    let CompilerError::Type(mojito::TypeError::Unsupported(message)) = error else {
        panic!("expected the residual-escape guard, got {error:?}");
    };
    assert!(message.contains("residual elements"), "{message}");
    assert!(message.contains("(close Conn)"), "{message}");
}

#[test]
fn nested_def_captured_self_store_faces_the_escape_guard() {
    // The diagnosed nested-def routing gap, closed: a nested `def` capturing
    // `mut self` stores a frame-local loan into a field of the enclosing
    // receiver. The nested Def frame's allowed-owner set now includes the
    // capture-reachable outer owners, so the store-outward guard fires and
    // rejects what previously slipped through to a stale-reference crash at
    // runtime.
    let compiler = Compiler::default();
    let source = "@fieldwise_init\nstruct RefBox[origin: Origin[mut=True]]:\n    var value: ref[origin] List[Int]\n\n@fieldwise_init\nstruct Holder[origin: Origin[mut=True]]:\n    var slot: RefBox[Self.origin]\n\n    def stash_local(mut self):\n        def install() {mut self}:\n            var local: List[Int] = [7]\n            ref alias = local\n            self.slot = RefBox(alias)\n        install()\n\ndef main():\n    var keep: List[Int] = [1]\n    ref whole = keep\n    var holder = Holder(RefBox(whole))\n    holder.stash_local()\n    print(holder.slot.value[0])\n";
    let error = compiler
        .compile_unlinked(source)
        .expect_err("nested-def store");
    assert!(matches!(
        error,
        CompilerError::Type(mojito::TypeError::StoredReferenceEscapesOrigin)
    ));

    // The parameter-rooted twin does not escape the frame, but a box over
    // the parameter's origin is not a `RefBox[Self.origin]`: it rejects on
    // origin identity, as at the pin, before the escape guard is consulted.
    let param_rooted = "@fieldwise_init\nstruct RefBox[origin: Origin[mut=True]]:\n    var value: ref[origin] List[Int]\n\n@fieldwise_init\nstruct Holder[origin: Origin[mut=True]]:\n    var slot: RefBox[Self.origin]\n\n    def stash_param(mut self, mut source: List[Int]):\n        def install() {mut self, ref source}:\n            ref alias = source\n            self.slot = RefBox(alias)\n        install()\n\ndef main():\n    var keep: List[Int] = [1]\n    ref whole = keep\n    var holder = Holder(RefBox(whole))\n    var other: List[Int] = [5]\n    holder.stash_param(other)\n";
    let error = compiler
        .compile_unlinked(param_rooted)
        .expect_err("param-rooted nested-def store");
    assert!(
        matches!(&error, CompilerError::Type(mojito::TypeError::OriginIdentityMismatch { found, expected })
            if found == "RefBox[origin_of(source)]" && expected == "RefBox[origin]"),
        "{error}"
    );

    // Frame balance: a store BESIDE (after) a nested def, in the method's own
    // body, still faces the guard — the nested frame pushes and pops without
    // disturbing the method's escape context.
    let adjacent = "@fieldwise_init\nstruct RefBox[origin: Origin[mut=True]]:\n    var value: ref[origin] List[Int]\n\n@fieldwise_init\nstruct Holder[origin: Origin[mut=True]]:\n    var slot: RefBox[Self.origin]\n\n    def stash_local(mut self):\n        def helper(x: Int) -> Int:\n            return x\n        var local: List[Int] = [helper(7)]\n        ref alias = local\n        self.slot = RefBox(alias)\n\ndef main():\n    var keep: List[Int] = [1]\n    ref whole = keep\n    var holder = Holder(RefBox(whole))\n    holder.stash_local()\n";
    let error = compiler
        .compile_unlinked(adjacent)
        .expect_err("adjacent store");
    assert!(matches!(
        error,
        CompilerError::Type(mojito::TypeError::StoredReferenceEscapesOrigin)
    ));
}

#[test]
fn compiled_program_retains_and_emits_its_verified_mir() {
    let compiler = Compiler::default();
    let compiled = compiler
        .compile_unlinked("def main():\n    print(42)\n")
        .expect("compile");
    assert!(compiled.mir().invariant_errors.is_empty());
    let first = compiled.emit_mir().expect("emit MIR");
    let second = compiled.emit_mir().expect("repeat emission");
    assert_eq!(first, second);
    assert!(first.ends_with('\n'));
    assert!(!first.ends_with("\n\n"));
}

#[test]
fn compiled_program_caches_one_elaborated_backend_artifact() {
    let compiler = Compiler::default();
    let compiled = compiler
        .compile_unlinked("def main():\n    print(42)\n")
        .expect("compile");
    assert!(std::ptr::eq(
        compiled.elaborated_mir(),
        compiled.elaborated_mir()
    ));
    assert!(compiled.elaborated_mir().invariant_errors.is_empty());
    let emitted = compiled.emit_mir().expect("emit MIR");
    let execution = compiler.execute(&compiled).expect("execute");
    assert_eq!(execution.output, "42\n");
    assert!(emitted.starts_with("mojito-mir"));
}

#[test]
fn type_names_accept_applied_pack_elements_and_nested_spellings() {
    // A `Tuple[...]` or `SIMD[...]` application as a type-pack element, and
    // `_unqualified_type_name` spelling a minted value specialization at
    // every nesting level with `True`/`False` value arguments.
    let compiler = Compiler::default();
    let program = compiler
        .compile_source(
            "from std.format._utils import TypeNames\nfrom std.reflection.type_info import _unqualified_type_name\n\nstruct Flag[b: Bool](Copyable, Movable):\n    var v: Int\n\n    def __init__(out self):\n        self.v = 0\n\ndef main():\n    print(TypeNames[Tuple[Int, Bool]]())\n    print(TypeNames[SIMD[DType.int, 4]]())\n    print(_unqualified_type_name[Dict[String, Int]]())\n    print(_unqualified_type_name[Optional[Set[Int]]]())\n    print(_unqualified_type_name[List[Flag[True]]]())\n",
            std::path::Path::new("/tmp/mojito_type_names_applied.mojo"),
        )
        .expect("compile the applied pack elements");
    let output = compiler
        .execute(&program)
        .expect("run the applied pack elements");
    assert_eq!(
        output.output,
        "Tuple[SIMD[DType.int, 1], Bool]\nSIMD[DType.int, 4]\nDict[String, SIMD[DType.int, 1], AHasher[[0, 0, 0, 0] : SIMD[DType.uint64, 4]]]\nOptional[Set[SIMD[DType.int, 1], AHasher[[0, 0, 0, 0] : SIMD[DType.uint64, 4]]]]\nList[Flag[True]]\n"
    );
}

#[test]
fn assigned_call_over_owned_interior_view_argument_rejects() {
    // `s.rstrip()` borrows `s`'s owned bytes, so a call assigned straight
    // back to `s` would replace the storage its argument still reads.
    let error = Compiler::default()
        .compile_unlinked(
            "def takes(v: StringSpan) -> String:\n    return String(v)\n\ndef main():\n    var s = String(\"abc  \")\n    s = takes(s.rstrip())\n    print(s)\n",
        )
        .expect_err("the call result aliases its view argument");
    assert_eq!(
        error.to_string(),
        "aliasing values passed immutably to 'v' argument and constructed as a result in 'takes' call"
    );
}

#[test]
fn assigned_call_over_plain_origin_view_argument_is_accepted() {
    // `StringSpan(s)` borrows `s` itself rather than an owned interior of it,
    // and the temporary's loan ends when the call returns.
    let compiler = Compiler::default();
    let program = compiler
        .compile_unlinked("def main():\n    var s = String(\"abc\")\n    s = String(StringSpan(s))\n    print(s)\n")
        .expect("a plain-origin view temporary does not alias the destination");
    let execution = compiler.execute(&program).expect("execute");
    assert_eq!(execution.output, "abc\n");
}

#[test]
fn a_type_pack_overload_declared_first_does_not_capture_a_keyed_call() {
    // The name-keyed template registry keeps one declaration per name, so
    // resolving every call of `kind` against it answered 99 here: the pack
    // clone ran for a call the checker gave to the keyed overload. Which
    // declaration serves a call is the request's to say, not the registry's.
    let compiler = Compiler::default();
    let program = compiler
        .compile_source(
            "def kind[*Ts: Copyable](*xs: *Ts) -> Int:\n    return 99\n\ndef kind[T: Copyable](a: T) -> Int:\n    comptime if T == Int:\n        return 1\n    else:\n        return 10\n\ndef main():\n    print(kind(3))\n    print(kind(True))\n",
            std::path::Path::new("/tmp/mojito_pack_overload_declared_first.mojo"),
        )
        .expect("a keyed overload declared after a type-pack one");
    let output = compiler.execute(&program).expect("run the keyed clones");
    assert_eq!(output.output, "1\n10\n");
}

#[test]
fn a_type_pack_overload_of_a_keyed_name_is_served_by_its_own_class() {
    // One name, two specialization classes. The keyed member bakes its
    // `comptime if` per argument type and the pack member specializes over
    // its element types; each call reaches the clone of the declaration the
    // checker selected.
    let compiler = Compiler::default();
    let program = compiler
        .compile_source(
            "def kind[T: Copyable](a: T) -> Int:\n    comptime if T == Int:\n        return 1\n    else:\n        return 10\n\ndef kind[*Ts: Copyable](*xs: *Ts) -> Int:\n    var n = 0\n    comptime for i in range(Ts.length):\n        n = n + 1\n    return n\n\ndef main():\n    print(kind(3))\n    print(kind(True))\n    print(kind(1, 2))\n    print(kind(1, True, 3))\n",
            std::path::Path::new("/tmp/mojito_mixed_overload_family.mojo"),
        )
        .expect("a compile-time-keyed def overloaded with a type-pack one");
    let output = compiler
        .execute(&program)
        .expect("run both classes' clones");
    assert_eq!(output.output, "1\n10\n2\n3\n");
}

#[test]
fn a_nullary_overload_beside_a_type_pack_one_is_told_apart_by_its_arguments() {
    // A variadic parameter is caller-visible but is spelled by neither the
    // request's parameter names nor its parameter types, so the pack member
    // and the nullary one both claim the empty list. Only whether the
    // request's arguments bind a declaration's parameters separates them.
    let compiler = Compiler::default();
    let program = compiler
        .compile_source(
            "def kind[T: Copyable](a: T) -> Int:\n    comptime if T == Int:\n        return 1\n    else:\n        return 10\n\ndef kind[*Ts: Copyable](*xs: *Ts) -> Int:\n    var n = 0\n    comptime for i in range(Ts.length):\n        n = n + 1\n    return n\n\ndef kind() -> Int:\n    return 77\n\ndef main():\n    print(kind(3))\n    print(kind(1, 2))\n    print(kind())\n",
            std::path::Path::new("/tmp/mojito_nullary_beside_pack_overload.mojo"),
        )
        .expect("a nullary overload sharing the family's empty parameter key");
    let output = compiler.execute(&program).expect("run each selected clone");
    assert_eq!(output.output, "1\n2\n77\n");
}

const TEMPLATE_TWO_TYPES: &str = "def tag[T: Copyable](x: T) -> Int:\n    return 7\n\ndef main():\n    print(tag(3))\n    print(tag(True))\n";

/// The `return` expressions of every generated clone of `template`.
fn clone_return_values<'a>(
    program: &'a mojito::compiler::CompiledProgram,
    template: &str,
) -> Vec<&'a mojito::ast::Expr> {
    let prefix = format!("{template}$");
    program
        .checked()
        .statements()
        .iter()
        .filter_map(|statement| match &statement.kind {
            mojito::ast::StmtKind::Def { name, body, .. } if name.starts_with(&prefix) => {
                Some(body)
            }
            _ => None,
        })
        .flat_map(|body| body.iter())
        .filter_map(|statement| match &statement.kind {
            mojito::ast::StmtKind::Return(Some(value)) => Some(value),
            _ => None,
        })
        .collect()
}

/// How many times `name` was inferred and retained as a checked template.
/// Bundled method templates are certified too, so a test counts its own.
fn certified_count(stats: &mojito::templates::TemplateStats, name: &str) -> usize {
    stats
        .certified
        .iter()
        .filter(|certified| *certified == name)
        .count()
}

#[test]
fn template_two_types_infers_the_template_once() {
    let compiler = Compiler::default();
    let program = compiler
        .compile_unlinked(TEMPLATE_TWO_TYPES)
        .expect("compile");
    let stats = program.template_stats();
    assert_eq!(certified_count(stats, "tag"), 1, "one template inference");
    assert!(
        stats.reused.iter().any(|name| name == "tag"),
        "later passes reuse the template's own facts: {stats:?}"
    );
    let derived: std::collections::HashSet<&str> = stats
        .derived
        .iter()
        .map(String::as_str)
        .filter(|name| name.starts_with("tag$"))
        .collect();
    assert_eq!(derived.len(), 2, "both instances derive: {stats:?}");
    assert!(
        stats
            .inferred_clones
            .iter()
            .all(|name| !name.starts_with("tag$")),
        "no clone of the certified template is inferred: {stats:?}"
    );
    let execution = compiler.execute(&program).expect("execute");
    assert_eq!(execution.output, "7\n7\n");
}

#[test]
fn template_instance_ids_are_disjoint() {
    let program = Compiler::default()
        .compile_unlinked(TEMPLATE_TWO_TYPES)
        .expect("compile");
    let values = clone_return_values(&program, "tag");
    assert_eq!(values.len(), 2, "two clones, one return each");
    let nodes: Vec<_> = values
        .iter()
        .map(|value| {
            let ids = program.checked().expression_ids_at(&value.source_span());
            assert_eq!(ids.len(), 1, "one checked node per clone occurrence");
            program.checked().expression(ids[0]).expect("checked node")
        })
        .collect();
    assert_ne!(nodes[0].id, nodes[1].id, "instances share no checked node");
    assert_ne!(
        values[0].source_span(),
        values[1].source_span(),
        "instances share no occurrence"
    );
    for node in nodes {
        assert_eq!(node.ty, Some(mojito::Ty::IntLiteral));
        assert!(
            node.adjustments
                .contains(&SemanticAdjustment::MaterializeLiteral(mojito::Ty::Int)),
            "the derived literal keeps its materialization: {:?}",
            node.adjustments
        );
    }
}

fn run_source(source: &str) -> (String, mojito::templates::TemplateStats) {
    let compiler = Compiler::default();
    let program = compiler.compile_unlinked(source).expect("compile");
    let output = compiler.execute(&program).expect("execute").output;
    (output, program.template_stats().clone())
}

#[test]
fn template_overload_binding_keeps_the_symbolic_choice() {
    // The pinned Mojo binds `pick(x)` once, while it checks `outer` with `T`
    // symbolic: the generic overload is the only candidate a `T` argument
    // fits, and every instance inherits it. Re-checking the `Int` clone used
    // to rank the set again and pick `pick(x: Int)`.
    let generic_last = "def pick(x: Int) -> Int:\n    return 1\n\ndef pick[T: Copyable](x: T) -> Int:\n    return 2\n\ndef outer[T: Copyable](x: T) -> Int:\n    return pick(x)\n\ndef main():\n    print(outer(3))\n    print(outer(True))\n    print(pick(5))\n";
    let (output, stats) = run_source(generic_last);
    assert_eq!(output, "2\n2\n1\n");
    assert!(
        stats.derived.iter().any(|name| name.starts_with("outer$")),
        "the instances derive from the checked template: {stats:?}"
    );
    assert!(
        stats
            .inferred_clones
            .iter()
            .all(|name| !name.starts_with("outer$")),
        "no instance re-ranks the overload set: {stats:?}"
    );

    let generic_first = "def pick[T: Copyable](x: T) -> Int:\n    return 2\n\ndef pick(x: Int) -> Int:\n    return 1\n\ndef outer[T: Copyable](x: T) -> Int:\n    return pick(x)\n\ndef main():\n    print(outer[Int](3))\n    print(outer[Bool](True))\n    print(outer(4))\n    print(pick(5))\n";
    assert_eq!(run_source(generic_first).0, "2\n2\n2\n1\n");
}

#[test]
fn template_inner_request_needs_no_outer_clone_inference() {
    let source = "def helper[T: Copyable](x: T) -> Int:\n    return 5\n\ndef outer[T: Copyable](x: T) -> Int:\n    return helper(x)\n\ndef main():\n    print(outer(3))\n    print(outer(True))\n";
    let (output, stats) = run_source(source);
    assert_eq!(output, "5\n5\n");
    for clone in ["outer$", "helper$"] {
        assert!(
            stats.derived.iter().any(|name| name.starts_with(clone)),
            "{clone} instances derive, so the inner request came from retained facts: {stats:?}"
        );
        assert!(
            stats
                .inferred_clones
                .iter()
                .all(|name| !name.starts_with(clone)),
            "{clone} instances are never inferred: {stats:?}"
        );
    }
}

#[test]
fn template_bounded_len_realizes_per_instance() {
    // `len(x)` is proved once through `T: Sized`. Each instance takes the
    // concrete witness and the read-in-place fact the built-in records for a
    // nominal struct, without its body being inferred again.
    let source = "def is_empty[T: Sized](x: T) -> Bool:\n    return len(x) == 0\n\nstruct Bag(Sized):\n    var items: List[Int]\n\n    def __init__(out self):\n        self.items = List[Int]()\n\n    def __len__(self) -> Int:\n        return len(self.items)\n\ndef main():\n    var xs: List[Int] = [1, 2, 3]\n    print(is_empty(xs))\n    var b: Bag = Bag()\n    print(is_empty(b))\n";
    let (output, stats) = run_source(source);
    assert_eq!(output, "False\nTrue\n");
    assert_eq!(certified_count(&stats, "is_empty"), 1);
    assert!(
        stats
            .inferred_clones
            .iter()
            .all(|name| !name.starts_with("is_empty$")),
        "{stats:?}"
    );
    assert!(
        stats
            .derived
            .iter()
            .filter(|name| name.starts_with("is_empty$"))
            .collect::<std::collections::HashSet<_>>()
            .len()
            == 2,
        "{stats:?}"
    );
}

#[test]
fn template_two_arms_select_checked_facts() {
    // Source validation checks both arms once. Each instance keeps the
    // occurrences of the arm the elaborator selected and inherits their
    // facts; the untaken arm contributes nothing executable.
    let source = "def choose[flag: Bool]() -> Int:\n    comptime if flag:\n        return 11\n    else:\n        return 22\n\ndef main():\n    print(choose[True]())\n    print(choose[False]())\n";
    let compiler = Compiler::default();
    let program = compiler.compile_unlinked(source).expect("compile");
    let stats = program.template_stats();
    assert_eq!(certified_count(stats, "choose"), 1);
    assert!(
        stats
            .inferred_clones
            .iter()
            .all(|name| !name.starts_with("choose$")),
        "{stats:?}"
    );
    let returned: Vec<String> = clone_return_values(&program, "choose")
        .iter()
        .map(|value| format!("{:?}", value.kind))
        .collect();
    assert_eq!(returned.len(), 2, "one selected arm per instance");
    assert_ne!(returned[0], returned[1], "distinct selected-arm syntax");
    assert_eq!(
        compiler.execute(&program).expect("execute").output,
        "11\n22\n"
    );

    // An invalid untaken arm is rejected from the template, before any
    // instance exists, even when nothing instantiates it.
    let invalid = "def choose[flag: Bool]() -> Int:\n    comptime if flag:\n        return 11\n    else:\n        return \"twenty-two\"\n\ndef main():\n    print(1)\n";
    assert!(matches!(
        compiler.compile_unlinked(invalid),
        Err(CompilerError::Type(_))
    ));
}

#[test]
fn template_rebind_and_where_are_instance_obligations() {
    let rebind = |argument: &str| {
        format!(
            "def as_int[T: Copyable](x: T) -> Int:\n    return rebind[Int](x)\n\ndef main():\n    print(as_int({argument}))\n"
        )
    };
    let (output, stats) = run_source(&rebind("3"));
    assert_eq!(output, "3\n");
    assert!(
        stats.derived.iter().any(|name| name.starts_with("as_int$")),
        "a true assertion derives: {stats:?}"
    );
    // A false assertion is a type error in the rebind's own words, never a
    // permissive fallback.
    let error = Compiler::default()
        .compile_unlinked(&rebind("True"))
        .expect_err("a false rebind assertion rejects");
    assert!(
        error
            .to_string()
            .contains("rebind: the input type does not match"),
        "{error}"
    );

    let constrained = |argument: &str| {
        format!(
            "def small[n: Int]() -> Int where (n < 4, \"n must stay below four\"):\n    comptime if n == 0:\n        return 100\n    else:\n        return 200\n\ndef main():\n    print(small[{argument}]())\n"
        )
    };
    let (output, stats) = run_source(&constrained("3"));
    assert_eq!(output, "200\n");
    assert!(
        stats.derived.iter().any(|name| name.starts_with("small$")),
        "{stats:?}"
    );
    let error = Compiler::default()
        .compile_unlinked(&constrained("7"))
        .expect_err("a violated where clause rejects");
    assert!(
        matches!(error, CompilerError::Comptime(_))
            && error.to_string().contains("n must stay below four"),
        "a sourced constraint failure, not a clone body type error: {error}"
    );
}

#[test]
fn template_loop_instances_remap_owners() {
    // One template occurrence becomes several instance occurrences when a
    // `comptime for` unrolls. Every copy must name the instance's own `sum`,
    // and two instances must not share a binding identity.
    let source = "def total[n: Int]() -> Int:\n    var sum = 0\n    comptime for i in range(n):\n        comptime if i == 1:\n            sum += 10\n        else:\n            sum += 1\n    return sum\n\ndef main():\n    print(total[0]())\n    print(total[1]())\n    print(total[3]())\n";
    let compiler = Compiler::default();
    let program = compiler.compile_unlinked(source).expect("compile");
    let stats = program.template_stats();
    assert!(
        stats
            .inferred_clones
            .iter()
            .all(|name| !name.starts_with("total$")),
        "every trip count derives: {stats:?}"
    );
    let checked = program.checked();
    let mut declared = Vec::new();
    let mut updates_per_instance = Vec::new();
    for statement in checked.statements() {
        let mojito::ast::StmtKind::Def { name, body, .. } = &statement.kind else {
            continue;
        };
        if !name.starts_with("total$") {
            continue;
        }
        let owner = body
            .iter()
            .find(|statement| matches!(statement.kind, mojito::ast::StmtKind::VarDecl { .. }))
            .and_then(|statement| checked.tables().declaration_at(&statement.source_span()))
            .and_then(|declaration| declaration.binding)
            .expect("the instance declares its own 'sum'");
        let updates: Vec<_> = body
            .iter()
            .filter_map(|statement| match &statement.kind {
                mojito::ast::StmtKind::AugAssign { place, .. } => Some(place),
                _ => None,
            })
            .map(|place| {
                let ids = checked.expression_ids_at(&place.source_span());
                assert_eq!(ids.len(), 1, "each unrolled copy is its own occurrence");
                checked.expression(ids[0]).expect("checked place").binding
            })
            .collect();
        assert!(
            updates.iter().all(|binding| *binding == Some(owner)),
            "every copy of '{name}' updates that instance's 'sum'"
        );
        updates_per_instance.push(updates.len());
        declared.push(owner);
    }
    updates_per_instance.sort_unstable();
    assert_eq!(
        updates_per_instance,
        [0, 1, 3],
        "zero, one, and three trips"
    );
    declared.sort_unstable();
    declared.dedup();
    assert_eq!(declared.len(), 3, "instances share no binding identity");
    assert_eq!(
        compiler.execute(&program).expect("execute").output,
        "0\n1\n12\n"
    );
}

#[test]
fn template_pack_instances_derive() {
    // A pack-keyed template is checked once at the dependent element `Ts[i]`;
    // each instance takes every unrolled copy at the element the folded index
    // fixed, and the derived facts agree with the clone's own check.
    let source = "def show[*Ts: Writable](*values: *Ts):\n    comptime for i in range(values.__len__()):\n        print(values[i])\n\ndef main():\n    show(1, \"two\", 3.5)\n    show(True)\n    show()\n";
    let expected = "1\ntwo\n3.5\nTrue\n";
    for verify in [false, true] {
        let compiler = Compiler::default().with_template_verification(verify);
        let program = compiler.compile_unlinked(source).expect("compile");
        let stats = program.template_stats();
        assert_eq!(certified_count(stats, "show"), 1, "one template inference");
        // A verifying run infers each derived body as well and files it
        // under `verified` once the two bundles agree.
        let served = if verify {
            &stats.verified
        } else {
            &stats.derived
        };
        let derived: std::collections::HashSet<&str> = served
            .iter()
            .map(String::as_str)
            .filter(|name| name.starts_with("show$"))
            .collect();
        assert_eq!(derived.len(), 3, "every instance derives: {stats:?}");
        assert!(
            verify
                || stats
                    .inferred_clones
                    .iter()
                    .all(|name| !name.starts_with("show$")),
            "no clone of the certified template is inferred: {stats:?}"
        );
        assert_eq!(
            compiler.execute(&program).expect("execute").output,
            expected
        );
    }
}

#[test]
fn template_folded_values_derive() {
    // A loop variable and a value parameter read as runtime values fold to
    // each instance's literal. The literal keeps the name's identity and
    // takes its own type, its materialization to the template's `Int`, and
    // the temporary a read argument makes of it; the derived facts agree
    // with the clone's own check.
    let source = "def bump(x: Int) -> Int:\n    return x + 1\n\ndef mixed[n: Int]() -> Int:\n    var acc = n\n    comptime for i in range(n):\n        acc = acc * 2 + i\n        acc += bump(i)\n    comptime if n > 2:\n        return acc\n    return n\n\ndef main():\n    print(mixed[0]())\n    print(mixed[3]())\n";
    for verify in [false, true] {
        let compiler = Compiler::default().with_template_verification(verify);
        let program = compiler.compile_unlinked(source).expect("compile");
        let stats = program.template_stats();
        assert_eq!(certified_count(stats, "mixed"), 1, "one template inference");
        let served = if verify {
            &stats.verified
        } else {
            &stats.derived
        };
        let derived: std::collections::HashSet<&str> = served
            .iter()
            .map(String::as_str)
            .filter(|name| name.starts_with("mixed$"))
            .collect();
        assert_eq!(derived.len(), 2, "every instance derives: {stats:?}");
        assert_eq!(
            compiler.execute(&program).expect("execute").output,
            "0\n39\n"
        );
    }
    // Over two literals an operator folds, so a folded operand beside a
    // literal keeps the clone check.
    let folding = "def scaled[n: Int]() -> Int:\n    var acc = 0\n    comptime for i in range(n):\n        acc += i * 10\n    return acc\n\ndef main():\n    print(scaled[3]())\n";
    let (output, stats) = run_source(folding);
    assert_eq!(output, "30\n");
    assert!(
        stats
            .derived
            .iter()
            .all(|name| !name.starts_with("scaled$")),
        "{stats:?}"
    );
}

#[test]
fn discovery_scan_matches_the_checked_arena() {
    // Request discovery reads a `DiscoveryResult` instead of the assembled
    // arena. Every request kind is a function of the expressions visited,
    // their three recorded types, the declaration types, and the recorded
    // instantiations — so those must be exactly the arena's, in its order.
    for benchmark in ["tuple", "tstring", "generic", "stdlib_heavy"] {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("benchmarks/compile")
            .join(format!("{benchmark}.mojo"));
        let linked = mojito::link(&path).expect("link");
        let program = mojito::elaborate(linked).expect("elaborate");
        let discovery = mojito::checker::check_program_for_discovery(
            &program,
            &std::collections::HashMap::new(),
            &mut mojito::templates::TemplateCatalog::default(),
        )
        .expect("check");
        let mut scanned = Vec::new();
        discovery.scan_expressions(&mut |expression| {
            scanned.push((
                expression.source_span(),
                discovery.expression_type(expression).cloned(),
                discovery.expression_place_type(expression).cloned(),
                discovery.expression_binding_type(expression).cloned(),
            ));
        });
        let declaration_types = discovery.declaration_types();
        let generic = discovery.generic_instantiations().clone();
        let methods = discovery.method_instantiations().clone();
        let structs = discovery.struct_instantiations().to_vec();
        let leaves = discovery.hash_leaf_types().to_vec();

        let checked = discovery.finalize();
        let arena: Vec<_> = checked
            .expressions()
            .iter()
            .map(|node| {
                (
                    node.syntax.source_span(),
                    node.ty.clone(),
                    node.place_ty.clone(),
                    node.binding_ty.clone(),
                )
            })
            .collect();
        assert!(!arena.is_empty(), "{benchmark}");
        assert_eq!(scanned, arena, "{benchmark}: expressions and their types");
        let arena_declarations: Vec<_> = checked
            .declarations()
            .iter()
            .filter_map(|declaration| declaration.ty.clone())
            .collect();
        assert_eq!(declaration_types, arena_declarations, "{benchmark}");
        assert_eq!(&generic, checked.generic_instantiations(), "{benchmark}");
        assert_eq!(&methods, checked.method_instantiations(), "{benchmark}");
        assert_eq!(structs, checked.struct_instantiations(), "{benchmark}");
        assert_eq!(leaves, checked.hash_leaf_types(), "{benchmark}");
    }
}

/// Compile `source` as a linked entry module. An unlinked source has no
/// module path, and an instance seen only there mints no method clones.
fn compile_entry(compiler: &Compiler, source: &str) -> mojito::compiler::CompiledProgram {
    compiler
        .compile_source(source, std::path::Path::new("template_methods.mojo"))
        .expect("compile")
}

const METHOD_GETTERS: &str = "@fieldwise_init\nstruct Counter[T: Copyable & Movable & Deinitable](Copyable):\n    var item: Self.T\n    var count: Int\n    var active: Bool\n\n    def size(self) -> Int:\n        return self.count\n\n    def doubled(self, extra: Int) -> Int:\n        return self.count + self.count + extra\n\n    def is_active(self) -> Bool:\n        return self.active\n\ndef main():\n    var a = Counter(7, 3, True)\n    var b = Counter(String(\"x\"), 5, False)\n    print(a.size(), a.doubled(1), a.is_active())\n    print(b.size(), b.doubled(2), b.is_active())\n";

#[test]
fn template_method_instances_derive() {
    // A generic struct's method is inferred once, with the struct's
    // parameters symbolic. Each per-instantiation clone (`size$y3:Int`)
    // inherits the checked template's facts instead of being inferred.
    let compiler = Compiler::default();
    let program = compile_entry(&compiler, METHOD_GETTERS);
    let stats = program.template_stats();
    assert_eq!(
        compiler.execute(&program).expect("execute").output,
        "3 7 True\n5 12 False\n"
    );
    for method in ["Counter.size", "Counter.doubled", "Counter.is_active"] {
        assert!(
            stats.certified.iter().any(|name| name == method),
            "{method} is a checked template: {stats:?}"
        );
        let clone = format!("{method}$");
        let derived: std::collections::HashSet<&String> = stats
            .derived
            .iter()
            .filter(|name| name.starts_with(&clone))
            .collect();
        assert_eq!(
            derived.len(),
            2,
            "{method} derives for both instances; refused: {:?}",
            stats.refused
        );
        assert!(
            stats
                .inferred_clones
                .iter()
                .all(|name| !name.starts_with(&clone)),
            "no clone of {method} is inferred: {stats:?}"
        );
    }
}

/// Compile `source` derived and under fact verification, which fails the
/// compilation if a derived bundle differs from the body's own check. Both
/// must run to `expected`, and each of `methods` must derive for `instances`
/// instances with no clone of it inferred.
fn assert_methods_derive(source: &str, expected: &str, methods: &[(&str, usize)]) {
    let compiler = Compiler::default();
    let derived = compile_entry(&compiler.clone().with_template_verification(false), source);
    let verified = compile_entry(&compiler.clone().with_template_verification(true), source);
    assert_eq!(
        compiler.execute(&derived).expect("execute").output,
        expected
    );
    assert_eq!(
        compiler.execute(&verified).expect("execute").output,
        expected
    );
    assert_eq!(
        clones_and_requests(&derived),
        clones_and_requests(&verified)
    );
    let stats = derived.template_stats();
    for (method, instances) in methods {
        let clone = format!("{method}$");
        let names: std::collections::HashSet<&String> = stats
            .derived
            .iter()
            .filter(|name| name.starts_with(&clone))
            .collect();
        assert_eq!(
            names.len(),
            *instances,
            "{method} derives for every instance; refused: {:?}",
            stats.refused
        );
        assert!(
            stats
                .inferred_clones
                .iter()
                .all(|name| !name.starts_with(&clone)),
            "no clone of {method} is inferred: {:?}",
            stats.inferred_clones
        );
    }
}

#[test]
fn template_method_statements_derive() {
    // Receivers beyond a read `self`, a `where` clause, scalar locals and
    // field writes, `if`, `while`, a discarded call, and a sibling call that
    // passes a scalar.
    assert_methods_derive(
        include_str!("../assets/ok/template_method_statements.mojo"),
        "4 51\n0 12\n10 20\n10 55\n9 20\n64 64\n9 39\n",
        &[
            ("Tally.size", 2),
            ("Tally.bump", 2),
            ("Tally.reset", 2),
            ("Tally.fill", 2),
            ("Tally.bump_by", 2),
            ("Tally.triangle", 2),
            ("Tally.clamped", 2),
            ("Tally.spend", 2),
            ("Tally.width", 2),
        ],
    );
}

#[test]
fn template_method_moves_derive() {
    // A whole value of a parameter type moved between a `var` parameter, a
    // local, a field, and the result. `Keep`'s local is linear in the
    // template and deletable in each instance; `Pair`'s copies are owed per
    // instance.
    assert_methods_derive(
        include_str!("../assets/ok/template_method_moves.mojo"),
        "1 3\n1 z\n1 9\n0 4 0 1 x\n4 q 2\n2 1 2\nr l r\n",
        &[
            ("Cell.__init__", 2),
            ("Cell.count", 2),
            ("Slot.replace", 3),
            ("Slot.cycle", 3),
            ("Slot.take", 3),
            ("Keep.pass_through", 2),
            ("Pair.left", 2),
            ("Pair.pick", 2),
            ("Pair.flip", 2),
        ],
    );
}

#[test]
fn template_method_reference_results_derive() {
    // A `ref self` accessor returning a field or a pointer slot as a handle,
    // behind a scalar guard or the bundled bounds check that aborts.
    assert_methods_derive(
        include_str!("../assets/ok/template_method_reference_result.mojo"),
        "1 0 1\na 0 a\n3 0 3\nz y\n4 4\n",
        &[
            ("Slot.peek", 3),
            ("Slot.counter", 3),
            ("Slot.guarded", 3),
            ("List.unsafe_get", 1),
            ("Optional.value", 1),
            ("Optional.unsafe_value", 1),
        ],
    );
}

#[test]
fn template_method_reference_calls_derive() {
    // A reference-returning call on a field of `self`, forwarded as the
    // method's own reference result or read by value. `Rack`'s template marks
    // no copyable read; its `Int` instance does and its `Token` one does not.
    assert_methods_derive(
        include_str!("../assets/ok/template_method_reference_call.mojo"),
        "4\n3 4\n8\n9\ny\nx y\n",
        &[
            ("Rack.at", 2),
            ("Shelf.at", 2),
            ("Shelf.first", 2),
            ("Shelf.second", 2),
        ],
    );
}

#[test]
fn template_method_subscript_stores_derive() {
    // A store through a subscript of `self` or of one of its fields: a scalar
    // field of a reference getter's element, a scalar or a whole value
    // through a declared setter, a scalar element stored whole or augmented
    // through the mutable reference its getter yields or through a value
    // getter and a setter, and a struct element's in-place dunder.
    assert_methods_derive(
        include_str!("../assets/ok/template_method_subscript_store.mojo"),
        "8 0 31\n6 3 7 11\n0 51\ny 1 z 5\n2 3 4 8\n1 3 8 w\n",
        &[
            ("Shelf.reset", 2),
            ("Shelf.bump", 2),
            ("Shelf.count", 2),
            ("Shelf.bump_count", 2),
            ("Shelf.put_item", 2),
            ("Shelf.put_bucket", 2),
            ("Shelf.put_entry", 2),
            ("Shelf.set_cell", 2),
            ("Shelf.bump_cell", 2),
            ("Shelf.bump_table", 2),
            ("Shelf.bump_counter", 2),
            ("Shelf.bump_tally", 2),
            ("Shelf.put", 2),
        ],
    );
}

#[test]
fn template_method_parameter_built_stores_derive() {
    // An augmented element store whose value getter reads a subscripted
    // value built over the parameter, or whose element is the parameter
    // itself and updates through its bound's `__iadd__`: the getter is
    // realized on the instance's receiver and the dunder re-selected on the
    // instance's element type.
    assert_methods_derive(
        include_str!("../assets/ok/template_method_parameter_built_store.mojo"),
        "2 2 6 9\n1 4 32 41\n3 21\n",
        &[
            ("Rack.bump_box", 2),
            ("Rack.bump_self", 2),
            ("Rack.accumulate", 2),
            ("Rack.fold", 2),
        ],
    );
}

#[test]
fn template_method_borrowed_parameters_derive() {
    // A `mut` or bare `ref` parameter is bound from its convention alone: a
    // `mut` one may be stored to, scalar or whole, and a `ref` one is read.
    assert_methods_derive(
        include_str!("../assets/ok/template_method_borrowed_parameter.mojo"),
        "5\n1 mine\n7 held\nTrue True\n0\n",
        &[
            ("Shelf.tally", 2),
            ("Shelf.reset", 2),
            ("Shelf.put", 2),
            ("Shelf.same", 2),
            ("Shelf.larger", 2),
        ],
    );
}

#[test]
fn template_method_reference_locals_derive() {
    // A `ref` declaration over `self`, a field, a parameter, a local, or a
    // reference call, then read, copied out, stored through, or forwarded as
    // the method's own reference result.
    assert_methods_derive(
        include_str!("../assets/ok/template_method_reference_local.mojo"),
        "3 0 3\n5 x\n4 y\n2 2 2 2\n4 4\n9 q\n5 x\n",
        &[
            ("Shelf.hits_at", 2),
            ("Shelf.value_at", 2),
            ("Shelf.item_at", 2),
            ("Shelf.touch", 2),
            ("Shelf.size", 2),
            ("Shelf.mine", 2),
            ("Shelf.doubled", 2),
            ("Shelf.echo", 2),
            ("Shelf.peek", 2),
        ],
    );
}

#[test]
fn template_method_reference_receivers_derive() {
    // A field read or a closed method call through a reference call's result
    // or a `ref` local, over a closed referent and a generic one.
    assert_methods_derive(
        include_str!("../assets/ok/template_method_reference_receiver.mojo"),
        "1 0 3 2\n4 y\n10 0 4 3\n",
        &[
            ("Shelf.key_at", 2),
            ("Shelf.total_at", 2),
            ("Shelf.value_at", 2),
            ("Shelf.bump_at", 2),
            ("Shelf.seen_at", 2),
            ("Shelf.touch", 2),
        ],
    );
}

#[test]
fn template_method_reference_arguments_derive() {
    // A `ref` local, a field reached through a reference, and a reference
    // call's result handed to a read, a `var`, and a `mut` parameter, and a
    // `ref` local lent to a hand-written constructor's `ref` parameter.
    assert_methods_derive(
        include_str!("../assets/ok/template_method_reference_argument.mojo"),
        "2 1 2 1\n2 1 2 1\n2 1 2 1\n2 2 2 1\n1 1 2 2\n",
        &[
            ("Shelf.local_entry", 2),
            ("Shelf.call_entry", 2),
            ("Shelf.call_item", 2),
            ("Shelf.local_field", 2),
            ("Shelf.call_field", 2),
            ("Shelf.taken", 2),
            ("Shelf.kept", 2),
            ("Shelf.counted", 2),
            ("Shelf.counted_mut", 2),
        ],
    );
}

#[test]
fn template_method_sibling_views_derive() {
    // A sibling call whose result is a view over `self`: returned as is,
    // wrapped by a fieldwise or a hand-written constructor, spelled through a
    // `comptime` alias as `Dict.keys` is, and bound to a local. The call's
    // loan and the origin its result binds name the receiver.
    assert_methods_derive(
        include_str!("../assets/ok/template_method_sibling_view.mojo"),
        "2 1 2 1\n2 1 12 11\n2 1\n",
        &[
            ("Shelf.view", 2),
            ("Shelf.keys", 2),
            ("Shelf.aliased", 2),
            ("Shelf.counted", 2),
            ("Shelf.bound", 2),
        ],
    );
}

#[test]
fn template_method_raises_derive() {
    // A method that raises: `Error("…")` under a bare `raises`, and a
    // construction of the declared error type, built over the struct's
    // parameter or closed, under a typed one, one of them from a method
    // returning a reference.
    assert_methods_derive(
        include_str!("../assets/ok/template_method_raises.mojo"),
        "7\nseven\n7 seven\nerror: empty slot\nerror: SlotError\n1 2 1\nerror: Exhausted\n",
        &[("Slot.take", 2), ("Slot.peek", 2), ("Slot.use", 2)],
    );
}

#[test]
fn template_method_binder_construction_derives() {
    // A construction of the method's own `[H: Hasher]` binder, kept symbolic
    // by every clone, and the fresh hasher consumed through `finish`.
    assert_methods_derive(
        include_str!("../assets/ok/template_method_binder_construction.mojo"),
        "True False\nTrue False\nTrue False\n",
        &[("Sealed.__hash__", 3)],
    );
}

#[test]
fn template_method_hash_leaf_derives() {
    // A multi-lane vector handed to a hasher, which no other body hashed:
    // the leaf is closed, so every instance records the template's again.
    assert_methods_derive(
        include_str!("../assets/ok/template_method_hash_leaf.mojo"),
        "True False\nFalse\nTrue False\nTrue False\n",
        &[("Lanes.__hash__", 3)],
    );
}

#[test]
fn template_method_vector_hash_leaf_derives() {
    // A bound `__hash__` on a sized scalar or a multi-lane vector instance:
    // derivation's hashed leaf is the one the instance's clone check accepts.
    assert_methods_derive(
        include_str!("../assets/ok/template_method_vector_hash_leaf.mojo"),
        "5089976597503910097\nTrue True\n",
        &[("Box.digest", 2)],
    );
    assert_methods_derive(
        include_str!("../conformance/probes/native_simd_instance_mangle.mojo"),
        "1547189026303444902\n",
        &[("Box.digest", 1)],
    );
}

#[test]
fn template_method_struct_binder_construction_keeps_the_clone_check() {
    // A struct binder's construction (`Self.H()`) builds another type under
    // each instance, so the body stays outside the class.
    let source = "from std.hashlib import Hasher\nfrom std.hashlib.hasher import default_hasher\n\n\nstruct Mixer[H: Hasher](Movable):\n    var seed: Int\n\n    def __init__(out self, seed: Int):\n        self.seed = seed\n\n    def mix(self) -> UInt64:\n        var inner = Self.H()\n        inner.update(self.seed)\n        return inner^.finish()\n\n\ndef main():\n    var a = Mixer[default_hasher](3)\n    var b = Mixer[default_hasher](3)\n    print(a.mix() == b.mix())\n";
    let compiler = Compiler::default();
    let program = compile_entry(&compiler, source);
    let stats = program.template_stats();
    assert_eq!(
        compiler.execute(&program).expect("execute").output,
        "True\n"
    );
    assert!(
        stats.refused.iter().any(|(name, reason)| {
            name.starts_with("Mixer.mix$") && reason == "its template is not certified"
        }),
        "the struct binder's construction leaves the body outside the class: {:?}",
        stats.refused
    );
}

#[test]
fn template_method_value_parameter_templates_reuse() {
    // A struct with a scalar value parameter (`Grid[T, rows: Int]`, as
    // `Array`) or an origin parameter (`Span`) is never cloned whole, so its
    // methods are certified as templates and their own facts reused in every
    // later pass instead of being inferred again. A receiver origin naming
    // the method's own binder (`ref [o] self`) derives per instance.
    let source = include_str!("../assets/ok/template_method_value_parameter.mojo");
    let expected = "2 3\na 2\n5 5 3\nz 2\n4 8 False 2 6 True\n2 2\n";
    let compiler = Compiler::default();
    let program = compile_entry(&compiler.clone().with_template_verification(false), source);
    let verified = compile_entry(&compiler.clone().with_template_verification(true), source);
    assert_eq!(
        compiler.execute(&program).expect("execute").output,
        expected
    );
    assert_eq!(
        compiler.execute(&verified).expect("execute").output,
        expected
    );
    let stats = program.template_stats();
    for name in [
        "Grid.count",
        "Grid.total",
        "Grid.full",
        "Array.__len__",
        "Array.__getitem__",
        "Array.unsafe_get",
        "Span.__len__",
        "Span.__getitem__",
        "Cell.hits_of",
    ] {
        assert_eq!(
            certified_count(stats, name),
            1,
            "{name}: one template inference"
        );
        assert!(
            stats.reused.iter().any(|reused| reused == name),
            "{name}: later passes reuse the template's own facts: {stats:?}"
        );
    }
    let derived: std::collections::HashSet<&str> = stats
        .derived
        .iter()
        .map(String::as_str)
        .filter(|name| name.starts_with("Cell.hits_of$"))
        .collect();
    assert_eq!(
        derived.len(),
        2,
        "both receiver-origin clones derive: {stats:?}"
    );
}

#[test]
fn template_method_raised_string_keeps_the_clone_check() {
    // A raised string literal is converted to `Error` by its type, which no
    // recipe keeps yet, so the body stays outside the class and still runs.
    let source = "struct Slot[T: Copyable & Deinitable](Movable):\n    var item: Self.T\n    var full: Bool\n\n    def __init__(out self, var item: Self.T):\n        self.item = item^\n        self.full = False\n\n    def take(self) raises -> Self.T:\n        if not self.full:\n            raise \"empty\"\n        return self.item.copy()\n\n\ndef main():\n    try:\n        print(Slot[Int](1).take())\n    except e:\n        print(e)\n";
    let compiler = Compiler::default();
    let program = compile_entry(&compiler, source);
    let stats = program.template_stats();
    assert_eq!(
        compiler.execute(&program).expect("execute").output,
        "empty\n"
    );
    assert!(
        stats.refused.iter().any(|(name, reason)| {
            name.starts_with("Slot.take$") && reason == "its template is not certified"
        }),
        "the raised string leaves the body outside the class: {:?}",
        stats.refused
    );
}

#[test]
fn template_method_explicit_destroy_call_derives() {
    // A consuming call on a `^` transfer: a named `deinit self` destructor on
    // a local or on a field of a consumed `self`, a `var self` method, and a
    // `deinit self` requirement through a bound, whose instance's struct is
    // asked again whether it names a destructor.
    assert_methods_derive(
        include_str!("../assets/ok/template_method_explicit_destroy_call.mojo"),
        "2 3 5\n6 7\n8 9\n10 22 1\n",
        &[
            ("Desk.spend", 2),
            ("Desk.lease", 2),
            ("Ledger.close", 2),
            ("Clerk.handle", 2),
        ],
    );
}

#[test]
fn template_method_copied_consuming_receiver_derives() {
    // A consuming call on a named place the call copies first: `or_else` on
    // a field of a read parameter, of `self`, on a local, and on a parameter
    // built over the struct's parameter, and a `var self` requirement through
    // a bound. A direct call of a scalar module function derives beside them.
    assert_methods_derive(
        include_str!("../assets/ok/template_method_copied_consuming_receiver.mojo"),
        "3 2\n3 6\n7 9\nx z\n4 60\n",
        &[
            ("Shelf.count", 2),
            ("Shelf.capped", 2),
            ("Shelf.pick", 2),
            ("Purse.total", 2),
        ],
    );
}

#[test]
fn template_method_copied_receivers_derive() {
    // A read method copying `self` into a `var` local, then storing a scalar,
    // a whole value, or an in-place update to the local's fields and reading
    // them back. `Span`'s contiguous slice stores a pointer offset the same way.
    assert_methods_derive(
        include_str!("../assets/ok/template_method_copied_receiver.mojo"),
        "1 2 1 1\n5 q\n2 1\n2 2 3 2 b\n",
        &[
            ("Window.shifted", 2),
            ("Window.replaced", 2),
            ("Window.trimmed", 2),
        ],
    );
}

#[test]
fn template_method_tuple_elements_derive() {
    // Elements of a tuple-typed local read at literal indices, from a closed
    // tuple, one built over the struct's parameter, and `slice.indices(n)`,
    // as `List`'s strided slice reads its bounds.
    assert_methods_derive(
        include_str!("../assets/ok/template_method_tuple_element.mojo"),
        "3 5\n40 60\n1 2\n3 1 5 4 d a\n",
        &[("Holder.span", 2), ("Holder.tag", 2), ("Holder.width", 2)],
    );
}

#[test]
fn template_method_whole_tuple_element_keeps_the_clone_check() {
    // The grammar reads a tuple element only as a scalar; an element of the
    // struct's parameter type, read as a whole value, stays outside it.
    let source = "@fieldwise_init\nstruct Holder[T: ImplicitlyCopyable & Deinitable](Deinitable, ImplicitlyCopyable, Movable):\n    var value: Self.T\n    var size: Int\n\n    def tagged(self) -> Tuple[Self.T, Int]:\n        return (self.value, self.size)\n\n    def first(self) -> Self.T:\n        var entry = self.tagged()\n        var head = entry[0]\n        return head\n\n\ndef main():\n    print(Holder[Int](7, 3).first(), Holder[String](\"w\", 5).first())\n";
    let compiler = Compiler::default();
    let program = compile_entry(&compiler, source);
    let stats = program.template_stats();
    assert_eq!(compiler.execute(&program).expect("execute").output, "7 w\n");
    assert!(
        stats.refused.iter().any(|(name, reason)| {
            name.starts_with("Holder.first$") && reason == "its template is not certified"
        }),
        "a whole-value tuple element leaves the body outside the class: {:?}",
        stats.refused
    );
}

#[test]
fn template_method_defaulted_destructor_keeps_the_clone_check() {
    // A destructor's defaulted argument is evaluated in the callee's scope,
    // which no recipe keeps yet, so the body stays outside the class.
    let source = "@explicit_destroy(\"finish the ticket\")\nstruct Ticket(Movable, Deinitable where False):\n    var id: Int\n\n    def __init__(out self, id: Int):\n        self.id = id\n\n    def finish(deinit self, bonus: Int = 1) -> Int:\n        return self.id + bonus\n\n\nstruct Desk[T: Copyable & Deinitable](Movable):\n    var count: Int\n\n    def __init__(out self):\n        self.count = 0\n\n    def spend(self, n: Int) -> Int:\n        var ticket = Ticket(n)\n        return ticket^.finish()\n\n\ndef main():\n    print(Desk[Int]().spend(1))\n";
    let compiler = Compiler::default();
    let program = compile_entry(&compiler, source);
    let stats = program.template_stats();
    assert_eq!(compiler.execute(&program).expect("execute").output, "2\n");
    assert!(
        stats.refused.iter().any(|(name, reason)| {
            name.starts_with("Desk.spend$") && reason == "its template is not certified"
        }),
        "the defaulted argument leaves the body outside the class: {:?}",
        stats.refused
    );
}

#[test]
fn template_method_struct_argument_derives() {
    // An instance argument that is a struct declaring fields of its own
    // parameter types is judged with those fields at its own arguments, so
    // `Pair[Int, String]` is plain data as `Int` is. Native lowering of the
    // shape is roadmap 2.4's collision, so it is no `assets/ok` fixture.
    assert_methods_derive(
        "@fieldwise_init\nstruct Pair[K: Copyable & Deinitable, V: Copyable & Deinitable](Copyable):\n    var key: Self.K\n    var value: Self.V\n\n\nstruct Shelf[T: Copyable & Deinitable](Movable):\n    var item: Self.T\n    var uses: Int\n\n    def __init__(out self, var item: Self.T):\n        self.item = item^\n        self.uses = 0\n\n    def bump(mut self) -> Int:\n        self.uses += 1\n        return self.uses\n\n    def replace(mut self, var item: Self.T) -> Int:\n        self.item = item^\n        return self.bump()\n\n\ndef main():\n    var a = Shelf[Pair[Int, String]](Pair[Int, String](1, \"one\"))\n    var b = Shelf[Int](7)\n    print(a.bump(), b.bump())\n    print(a.replace(Pair[Int, String](2, \"two\")), b.replace(8))\n    print(a.item.key, a.item.value, b.item)\n",
        "1 1\n2 2\n2 two 8\n",
        &[("Shelf.bump", 2), ("Shelf.replace", 2)],
    );
}

#[test]
fn template_method_built_over_locals_derive() {
    // A local whose type is built over the struct's parameter: a bundled
    // collection moved out, handed on, or left unused, a hand-written struct
    // named or inferred at its construction, and a view a constructor's `ref`
    // parameter infers. Each binding's deletability is judged again at the
    // instance's type. Such a local is also a receiver and `len`'s operand.
    assert_methods_derive(
        include_str!("../assets/ok/template_method_built_over_local.mojo"),
        "0 0 3 3\n2 y 5 z\n4 4 1 1\n0 0 4 x 0 0\n",
        &[
            ("Shelf.made", 2),
            ("Shelf.fresh", 2),
            ("Shelf.popped", 2),
            ("Shelf.cleared", 2),
            ("Shelf.handed", 2),
            ("Shelf.wrapped", 2),
            ("Shelf.inferred", 2),
            ("Shelf.unused", 2),
            ("Shelf.viewed", 2),
        ],
    );
}

#[test]
fn template_method_constructions_derive() {
    // A `copy:` construction of the struct's own type, a fieldwise
    // construction, a bundled collection over the struct's parameter, and a
    // hand-written constructor family retargeted to the instance's own
    // `__init__` clone, as a result, a field store, and a local.
    assert_methods_derive(
        include_str!("../assets/ok/template_method_construction.mojo"),
        "1 2 x 3 0 0\n1 x 1 2 x\n1 0 x 7\n1 1 5 y\n0 0\n",
        &[
            ("Box.copy", 2),
            ("Box.empty", 2),
            ("Box.some", 2),
            ("Box.pair", 2),
            ("Box.tagged", 2),
            ("Box.tagged_as", 2),
            ("Box.reset", 2),
            ("Box.keep", 2),
            ("Box.__init__", 2),
            ("Tagged.__init__", 2),
        ],
    );
}

#[test]
fn template_method_converting_call_arguments_derive() {
    // A literal and a whole value of the struct's parameter type, each
    // converted at a sibling call's argument. The conversion is recorded
    // twice — at the argument and in the call's boundary — so the verified
    // half of this assertion is what pins the boundary: a derived bundle
    // that kept the template's `Wrapper` there differs from the clone's own
    // `Wrapper.__init__$y3:Int`.
    assert_methods_derive(
        include_str!("../assets/ok/template_method_converting_call_argument.mojo"),
        "4 2 7\n4 2 7\n",
        &[
            ("Holder.numbered", 2),
            ("Holder.flagged", 2),
            ("Holder.boxed", 2),
            ("Holder.keep", 2),
        ],
    );
}

#[test]
fn template_method_converting_closed_bindings_derive() {
    // A literal converted into a closed struct at an annotated binding: the
    // local holds the struct, not the scalar it was converted from.
    assert_methods_derive(
        include_str!("../assets/ok/template_method_converting_argument.mojo"),
        "count 4 flag True 5\ncount 4 flag True hi\n",
        &[("Holder.write_to", 2)],
    );
}

#[test]
fn template_method_converting_bindings_derive() {
    // An annotated `var` whose value converts to a declared type built over
    // the struct's parameter: a literal and a whole value of the parameter
    // type, each reaching the instance's own constructor clone; an
    // annotation equal to the value's type, which converts nothing; and a
    // scalar annotation, whose local stays a scalar.
    assert_methods_derive(
        include_str!("../assets/ok/template_method_converting_binding.mojo"),
        "4 6 5 0\n4 6 hi 0\n3 m 4.0 4.0\n",
        &[
            ("Holder.labeled", 2),
            ("Holder.counted", 2),
            ("Holder.boxed", 2),
            ("Holder.listed", 2),
            ("Holder.moved", 2),
            ("Holder.floated", 2),
        ],
    );
}

#[test]
fn template_method_operator_dispatch_derives() {
    // Equality, `!=`, and an ordering over two places of the struct's
    // parameter type; `!=` served by the instance's `__eq__` and a negation
    // (`Tag` declares no `__ne__`); and `+` over two places whose type is
    // built over the parameter, whose result is a temporary of that type
    // rather than a `Bool`.
    assert_methods_derive(
        include_str!("../assets/ok/template_method_operator_dispatch.mojo"),
        "False True False True\n1 2\nTrue True True False\nFalse False 1\n3 7\n",
        &[
            ("Pair.same", 3),
            ("Pair.differs", 3),
            ("Pair.matches", 3),
            ("Sorted.ordered", 2),
            ("Sorted.bounds", 2),
            ("Totals.merged", 2),
        ],
    );
}

#[test]
fn template_method_operator_operands_derives() {
    // A sibling call's result, a nested operator, and a right-hand literal
    // as operands: a temporary records nothing in either check, a place
    // under a consuming dunder is copied in the template already, and a
    // literal reaches `Meter[Self.T]` through the template's own conversion.
    assert_methods_derive(
        include_str!("../assets/ok/template_method_operator_operands.mojo"),
        "True False True False\n1 5 6 8\nTrue False True False\n3 9 4 12\nFalse False True True\n",
        &[
            ("Pair.starts", 2),
            ("Gauge.lowered", 2),
            ("Gauge.bumped", 2),
            ("Gauge.is_two", 2),
            ("Gauge.below", 2),
            ("Gauge.spanned", 2),
            ("Gauge.chained", 2),
            ("Gauge.balanced", 2),
            ("Gauge.matches_sum", 2),
        ],
    );
}

#[test]
fn template_method_bounded_arithmetic_derives() {
    // `a + b` and `a / b` under arithmetic bounds: the template's result is
    // the operand's own type (`Float64` for `/`), so the operator is a
    // temporary where the body puts it, and each instance owes that its
    // scalar has the operation and gives the type the bound promised.
    let source = r"struct Acc[T: Addable & Copyable & Deinitable & Divisible & Movable](Movable):
    var first: Self.T
    var second: Self.T

    def __init__(out self, var first: Self.T, var second: Self.T):
        self.first = first^
        self.second = second^

    def total(self) -> Self.T:
        return self.first + self.second

    def bound(self) -> Self.T:
        var sum = self.first + self.second
        return sum^

    def ratio(self) -> Float64:
        return self.first / self.second

def main():
    var a = Acc[Int](1, 2)
    var b = Acc[Float64](1.5, 0.5)
    print(a.total(), b.total())
    print(a.bound(), b.bound())
    print(a.ratio(), b.ratio())
";
    assert_methods_derive(
        source,
        "3 2.0\n3 2.0\n0.5 3.0\n",
        &[("Acc.total", 2), ("Acc.bound", 2), ("Acc.ratio", 2)],
    );
}

#[test]
fn template_method_consuming_operand_derives() {
    // A dunder that takes its operand by value copies the place the operator
    // reads, which the symbolic template never recorded. The instance owes
    // the copy and that its own type is implicitly copyable.
    let source = r"@fieldwise_init
struct Coin(Copyable, Deinitable, Equatable, ImplicitlyCopyable, Movable):
    var value: Int

    def __eq__(self, var other: Coin) -> Bool:
        return self.value == other.value

    def __ne__(self, var other: Coin) -> Bool:
        return self.value != other.value

struct Pair[T: Copyable & Equatable & Deinitable](Movable):
    var first: Self.T
    var second: Self.T

    def __init__(out self, var first: Self.T, var second: Self.T):
        self.first = first^
        self.second = second^

    def same(self) -> Bool:
        return self.first == self.second

def main():
    var a = Pair[Int](1, 2)
    var b = Pair[Coin](Coin(3), Coin(3))
    print(a.same(), b.same())
";
    assert_methods_derive(source, "False True\n", &[("Pair.same", 2)]);
}

#[test]
fn template_method_converting_operand_derives() {
    // No overload of the instance's `__eq__` takes its own type, so the
    // operand reaches the declared one through an `@implicit` constructor.
    // The conversion is selected at the instance's types by the same recipe a
    // converted call argument uses, and `Bill` pins the two adaptations
    // composed: the copy is owed at the operand's own type, the conversion at
    // the declared one.
    let source = r"@fieldwise_init
struct Money(Copyable, Deinitable, Equatable, ImplicitlyCopyable, Movable):
    var cents: Int

    def __eq__(self, other: Cents) -> Bool:
        return self.cents == other.amount

@fieldwise_init
struct Bill(Copyable, Deinitable, Equatable, ImplicitlyCopyable, Movable):
    var notes: Int

    def __eq__(self, var other: Cents) -> Bool:
        return self.notes == other.amount

struct Cents(Copyable, Deinitable, ImplicitlyCopyable, Movable):
    var amount: Int

    def __init__(out self, amount: Int):
        self.amount = amount

    @implicit
    def __init__(out self, m: Money):
        self.amount = m.cents

    @implicit
    def __init__(out self, b: Bill):
        self.amount = b.notes

struct Pair[T: Copyable & Equatable & Deinitable](Movable):
    var first: Self.T
    var second: Self.T

    def __init__(out self, var first: Self.T, var second: Self.T):
        self.first = first^
        self.second = second^

    def same(self) -> Bool:
        return self.first == self.second

def main():
    var a = Pair[Int](1, 2)
    var b = Pair[Money](Money(4), Money(4))
    var c = Pair[Bill](Bill(1), Bill(2))
    print(a.same(), b.same(), c.same())
";
    assert_methods_derive(source, "False True False\n", &[("Pair.same", 3)]);
}

#[test]
fn template_method_consuming_conversion_keeps_the_clone_check() {
    // The recipe re-selects the constructor at the instance's types, and a
    // constructor that consumes its source records an implicit copy the
    // template never did. The clone check takes the body back, and the
    // program still runs.
    let source = "struct Wrapper[T: Copyable & Deinitable](Copyable, Deinitable, Movable):\n    var value: Self.T\n\n    @implicit\n    def __init__(out self, var value: Self.T):\n        self.value = value^\n\nstruct Holder[T: Copyable & Deinitable](Deinitable, Movable):\n    var item: Self.T\n\n    def __init__(out self, var item: Self.T):\n        self.item = item^\n\n    def keep(self, box: Wrapper[Self.T]) -> Int:\n        return 7\n\n    def boxed(self) -> Int:\n        return self.keep(self.item)\n\ndef main():\n    var number = Holder[Int](5)\n    print(number.boxed())\n";
    let compiler = Compiler::default();
    let program = compile_entry(&compiler, source);
    let stats = program.template_stats();
    assert_eq!(compiler.execute(&program).expect("execute").output, "7\n");
    assert!(
        stats.refused.iter().any(|(name, reason)| {
            name.starts_with("Holder.boxed$")
                && reason == "an implicit conversion consumes, raises, or borrows for the instance"
        }),
        "the consuming conversion refuses the derivation: {:?}",
        stats.refused
    );
}

#[test]
fn template_method_view_binding_keeps_the_clone_check() {
    // An annotation left to inference takes the value's own type, and a
    // view converted from its source borrows it: neither is a relation an
    // instance derives, so the body stays outside the class and still runs.
    let source = "struct Holder[T: Copyable & Deinitable](Deinitable, Movable):\n    var items: List[Self.T]\n\n    def __init__(out self, var item: Self.T):\n        self.items = List[Self.T]()\n        self.items.append(item^)\n\n    def viewed(self) -> Int:\n        var span: Span[Self.T, _] = self.items\n        return len(span)\n\ndef main():\n    var number = Holder[Int](5)\n    print(number.viewed())\n";
    let compiler = Compiler::default();
    let program = compile_entry(&compiler, source);
    let stats = program.template_stats();
    assert_eq!(compiler.execute(&program).expect("execute").output, "1\n");
    assert!(
        stats.refused.iter().any(|(name, reason)| {
            name.starts_with("Holder.viewed$") && reason == "its template is not certified"
        }),
        "the view binding leaves the body outside the class: {:?}",
        stats.refused
    );
}

#[test]
fn template_method_bound_witness_shapes_derive() {
    // Each instance re-selects the witness a call through the bound names:
    // an overload member the arity picks, a `[H: Hasher]` binder a concrete
    // hasher bakes into the per-call clone, a `mut self` requirement, and a
    // `var self` one the receiver's `^` transfer consumes.
    assert_methods_derive(
        include_str!("../assets/ok/template_method_bound_witness_shapes.mojo"),
        "T3 B3 3 s\nT3 B3 3 s\nTrue True\nTrue True\nFalse\n15\n",
        &[
            ("Holder.__hash__", 4),
            ("Holder.write_to", 4),
            ("Holder.feed", 4),
            ("Holder.show", 4),
            ("Ledger.step", 1),
            ("Ledger.settle", 1),
        ],
    );
}

#[test]
fn template_method_same_arity_witness_overloads_keep_the_clone_check() {
    // Two `total` members take one argument each: only a ranking on the
    // argument's type would choose, so the instance is checked as a clone.
    let source = "trait Tally:\n    def total(self, by: Int) -> Int:\n        ...\n\n\nstruct Counter(Copyable, Deinitable, Movable, Tally):\n    var count: Int\n\n    def __init__(out self, count: Int):\n        self.count = count\n\n    def total(self, by: Int) -> Int:\n        return self.count + by\n\n    def total(self, by: String) -> Int:\n        return self.count\n\n\nstruct Ledger[T: Copyable & Deinitable & Tally](Movable):\n    var entry: Self.T\n\n    def __init__(out self, var entry: Self.T):\n        self.entry = entry^\n\n    def sum(self) -> Int:\n        return self.entry.total(1)\n\n\ndef main():\n    print(Ledger[Counter](Counter(2)).sum())\n";
    let compiler = Compiler::default();
    let program = compile_entry(&compiler, source);
    let stats = program.template_stats();
    assert_eq!(compiler.execute(&program).expect("execute").output, "3\n");
    assert!(
        stats.refused.iter().any(|(name, reason)| {
            name.starts_with("Ledger.sum$")
                && reason == "the instance's overloaded witness needs ranking by type"
        }),
        "the same-arity overload set refuses the derivation: {:?}",
        stats.refused
    );
}

#[test]
fn template_method_origin_bearing_constructions_derive() {
    // A view over the receiver, constructed from a `ref` local into a
    // fieldwise struct's reference field: the constructed type names the
    // receiver in an origin argument, kept by binding and rebound per
    // instance, and the `return` re-resolves the annotation's
    // `origin_of(self)`.
    assert_methods_derive(
        include_str!("../assets/extensions/ok/ref_field_template_method_construction.mojo"),
        "4 q 2 2\n5 r 1 1\n",
        &[("Store.view", 2), ("Store.rest", 2), ("Store.__init__", 2)],
    );
}

#[test]
fn template_def_with_a_scalar_local_keeps_the_symbolic_choice() {
    // A surviving trait-bound `def` with a scalar local derives, so its
    // instances inherit the overload the template bound rather than ranking
    // the set again on a concrete argument.
    let (output, stats) = run_source(include_str!(
        "../assets/ok/template_overload_binding_local.mojo"
    ));
    assert_eq!(output, "2\n2\n");
    assert_eq!(
        stats
            .derived
            .iter()
            .filter(|name| name.starts_with("outer$"))
            .collect::<std::collections::HashSet<_>>()
            .len(),
        2,
        "both instances derive; refused: {:?}",
        stats.refused
    );
}

#[test]
fn template_def_converting_argument_derives() {
    // A direct call whose argument converts through an `@implicit`
    // constructor: the template's selection of the callee stands for every
    // instance, and only the conversion beneath it is chosen again, in the
    // `comptime if` arm a keyed instance kept as well.
    let (output, stats) = run_source(include_str!(
        "../assets/ok/template_def_converting_argument.mojo"
    ));
    assert_eq!(output, "6 6\n4 2\n");
    for template in ["counted$", "keyed$"] {
        assert_eq!(
            stats
                .derived
                .iter()
                .filter(|name| name.starts_with(template))
                .collect::<std::collections::HashSet<_>>()
                .len(),
            2,
            "both {template} instances derive; refused: {:?}",
            stats.refused
        );
    }
}

/// The per-instantiation method clones of a compiled program, as
/// `Owner.clone` names, and the instances its checked facts request.
fn clones_and_requests(program: &mojito::compiler::CompiledProgram) -> (Vec<String>, Vec<String>) {
    let mut clones: Vec<String> = program
        .checked()
        .statements()
        .iter()
        .filter_map(|statement| match &statement.kind {
            mojito::ast::StmtKind::Struct { name, methods, .. } => Some((name, methods)),
            _ => None,
        })
        .flat_map(|(owner, methods)| {
            methods
                .iter()
                .filter(|method| method.self_ty.is_some())
                .map(move |method| format!("{owner}.{}", method.name))
        })
        .collect();
    clones.sort();
    let mut requests: Vec<String> = program
        .checked()
        .struct_instantiations()
        .iter()
        .map(|instantiation| format!("{instantiation:?}"))
        .collect();
    requests.sort();
    (clones, requests)
}

#[test]
fn template_method_requests_match_an_inferred_run() {
    // A derived clone must request exactly what an inferred clone requests,
    // or discovery converges on a different program. `Outer[Int].size`
    // reaches `Inner[Int].size` through a field, and `twice` reaches
    // `Outer[Int].size` through `self`: both calls are retargeted per
    // instance, and both receivers are recorded as applications.
    let source = "@fieldwise_init\nstruct Inner[T: Copyable & Movable & Deinitable](Copyable):\n    var item: Self.T\n    var count: Int\n\n    def size(self) -> Int:\n        return self.count\n\n@fieldwise_init\nstruct Outer[T: Copyable & Movable & Deinitable](Copyable):\n    var inner: Inner[Self.T]\n\n    def size(self) -> Int:\n        return self.inner.size()\n\n    def twice(self) -> Int:\n        return self.size() * 2\n\ndef main():\n    var a = Outer(Inner(7, 3))\n    var b = Outer(Inner(String(\"x\"), 5))\n    print(a.twice(), b.twice())\n";
    let derived = compile_entry(
        &Compiler::default().with_template_verification(false),
        source,
    );
    // Verification infers every derivable body and keeps the inferred facts;
    // it fails the compilation if they differ from the derived ones.
    let inferred = compile_entry(
        &Compiler::default().with_template_verification(true),
        source,
    );
    assert!(
        derived
            .template_stats()
            .derived
            .iter()
            .any(|name| name.starts_with("Outer.twice$")),
        "the retargeted call derives: {:?}",
        derived.template_stats()
    );
    assert_eq!(
        clones_and_requests(&derived),
        clones_and_requests(&inferred)
    );
    let compiler = Compiler::default();
    assert_eq!(
        compiler.execute(&derived).expect("execute").output,
        "6 10\n"
    );
    assert_eq!(
        compiler.execute(&inferred).expect("execute").output,
        "6 10\n"
    );
}

#[test]
fn template_transfer_replay_reuses_the_template_and_derives_its_instances() {
    // A method whose body replays a callee's transfer summary is certified
    // once, served from its own facts in every later transfer round, and
    // derived for each plain-data instance.
    let compiler = Compiler::default();
    let program = compiler
        .compile_source(
            &std::fs::read_to_string("assets/ok/template_method_transfer_replay.mojo")
                .expect("read the fixture"),
            std::path::Path::new("assets/ok/template_method_transfer_replay.mojo"),
        )
        .expect("compile");
    let stats = program.template_stats();
    for name in ["Bag.push", "Bag.push_held"] {
        assert_eq!(
            certified_count(stats, name),
            1,
            "{name}: one template inference"
        );
        assert!(
            stats.reused.iter().any(|reused| reused == name),
            "{name}: later rounds reuse the template's own facts: {stats:?}"
        );
        let derived: std::collections::HashSet<&str> = stats
            .derived
            .iter()
            .map(String::as_str)
            .filter(|derived| derived.starts_with(&format!("{name}$")))
            .collect();
        assert_eq!(derived.len(), 2, "{name}: both instances derive: {stats:?}");
        assert!(
            stats
                .inferred_clones
                .iter()
                .all(|clone| !clone.starts_with(&format!("{name}$"))),
            "{name}: no clone of the certified template is inferred: {stats:?}"
        );
    }
    let execution = compiler.execute(&program).expect("execute");
    assert_eq!(execution.output, "2 2 2 x\n");
}
