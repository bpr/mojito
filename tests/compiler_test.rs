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
