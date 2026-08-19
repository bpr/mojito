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
            "def choose(value: Int) -> Int:\n    return value\ndef choose(value: String) -> Int:\n    return len(value)\ndef main():\n    var result: Int = choose(42)\n    print(result)\n",
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
fn explicit_type_pack_specializes_variant_annotation_and_construction() {
    let compiler = Compiler::default();
    let program = compiler
        .compile_source(
            "from std.utils import Variant\n\ndef first_variant[*Ts: Movable]() -> Variant[*Ts]:\n    return Variant[*Ts](3)\n\ndef main():\n    var value = first_variant[Int, String]()\n    print(value.isa[Int]())\n    print(value[Int])\n",
            std::path::Path::new("/tmp/mojito_variant_type_pack.mojo"),
        )
        .expect("specialize a Variant type pack");
    let execution = compiler
        .execute(&program)
        .expect("execute specialized Variant construction");
    assert_eq!(execution.output, "True\n3\n");

    let unsupported_alternative = compiler
        .compile_source(
            "from std.utils import Variant\n\ndef first_variant[*Ts: Movable]() -> Variant[*Ts]:\n    return Variant[*Ts](True)\n\ndef main():\n    _ = first_variant[Int, String]()\n",
            std::path::Path::new("/tmp/mojito_variant_type_pack_bad_arm.mojo"),
        )
        .expect_err("the specialized constructor value must match an alternative");
    assert!(matches!(unsupported_alternative, CompilerError::Type(_)));
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

    let moved = compiler
        .compile_source(
            "from std.utils import Variant\n\ndef main():\n    var value = Variant[Int, String](7)\n    _ = value.unwrap[Int]()\n    print(value.isa[Int]())\n",
            std::path::Path::new("/tmp/mojito_variant_use_after_take.mojo"),
        )
        .expect_err("Variant.take consumes its receiver");
    assert!(matches!(moved, CompilerError::Ownership(_)));

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
    assert_eq!(execution.output, "True False\nstyled=4 Styled[4]\nTrue\n");

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
            "def wrap[T: Copyable & Movable](x: T, depth: Int) -> Int:\n    if depth <= 0:\n        return 0\n    return wrap([x], depth - 1)\n\ndef main():\n    print(wrap(1, 3))\n",
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
        "@fieldwise_init\nstruct RefBox[origin: Origin[mut=True]]:\n    var value: ref[origin] List[Int]\n\ndef make() -> List[RefBox]:\n    var sink: List[RefBox] = List[RefBox]()\n    var local: List[Int] = [9]\n    ref alias = local\n    sink.append(RefBox(alias))\n    return sink^\n\ndef main():\n    var got = make()\n",
        // via a transitively derived free-function effect
        "@fieldwise_init\nstruct RefBox[origin: Origin[mut=True]]:\n    var value: ref[origin] List[Int]\n\ndef stash(mut sink: List[RefBox], var box: RefBox):\n    sink.append(box^)\n\ndef collect() -> List[RefBox]:\n    var sink: List[RefBox] = List[RefBox]()\n    var local: List[Int] = [9]\n    ref alias = local\n    stash(sink, RefBox(alias))\n    return sink^\n\ndef main():\n    var got = collect()\n",
        // via a body-inferred user-method effect
        "@fieldwise_init\nstruct RefBox[origin: Origin[mut=True]]:\n    var value: ref[origin] List[Int]\n\n@fieldwise_init\nstruct Holder:\n    var slot: RefBox\n    def rebind_to(mut self, mut source: List[Int]):\n        ref alias = source\n        self.slot = RefBox(alias)\n\ndef steal() -> Holder:\n    var keep: List[Int] = [1]\n    ref whole = keep\n    var holder = Holder(RefBox(whole))\n    var local: List[Int] = [5]\n    holder.rebind_to(local)\n    return holder^\n\ndef main():\n    var got = steal()\n",
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
fn call_through_residues_resolve_at_call_sites() {
    // A body calling through its own callable parameter records a
    // higher-order call-through residue; each call site translates the
    // CONCRETE callable's effects through the recorded argument mapping and
    // replays them. Covers the compile-time value-param spelling, the
    // runtime `def(...)` parameter, and the two-level composed forwarding
    // chain — all through the seeded `List.append` effect, so the linked
    // compiler is required.
    let compiler = Compiler::default();
    let prologue = "@fieldwise_init\nstruct RefBox[origin: Origin[mut=True]]:\n    var value: ref[origin] List[Int]\n\ndef stash(mut sink: List[RefBox], box: RefBox):\n    sink.append(box^)\n\n";
    let value_param = format!(
        "{prologue}def feed[callback: def(mut List[RefBox], RefBox) thin](mut sink: List[RefBox], box: RefBox):\n    callback(sink, box^)\n\ndef main():\n    var sink: List[RefBox] = List[RefBox]()\n    var local: List[Int] = [9]\n    ref alias = local\n    feed[stash](sink, RefBox(alias))\n    local.append(1)\n    print(sink[0].value[0])\n"
    );
    let runtime_param = format!(
        "{prologue}def feed(f: def(mut List[RefBox], RefBox), mut sink: List[RefBox], box: RefBox):\n    f(sink, box^)\n\ndef main():\n    var sink: List[RefBox] = List[RefBox]()\n    var local: List[Int] = [9]\n    ref alias = local\n    feed(stash, sink, RefBox(alias))\n    local.append(1)\n    print(sink[0].value[0])\n"
    );
    let composed = format!(
        "{prologue}def feed[callback: def(mut List[RefBox], RefBox) thin](mut sink: List[RefBox], box: RefBox):\n    callback(sink, box^)\n\ndef outer[callback: def(mut List[RefBox], RefBox) thin](mut sink: List[RefBox], box: RefBox):\n    feed[callback](sink, box^)\n\ndef main():\n    var sink: List[RefBox] = List[RefBox]()\n    var local: List[Int] = [9]\n    ref alias = local\n    outer[stash](sink, RefBox(alias))\n    local.append(1)\n    print(sink[0].value[0])\n"
    );
    for source in [&value_param, &runtime_param, &composed] {
        let error = compiler
            .compile_unlinked(source)
            .expect_err("mutating the transferred loan's source must conflict");
        assert!(
            matches!(error, CompilerError::Ownership(_))
                && error
                    .to_string()
                    .contains("conflicts with live reference 'sink'"),
            "{error}"
        );
    }
}

#[test]
fn call_through_visibility_is_declaration_order_independent() {
    // The call-through map shares the two-phase pass: an earlier method
    // calling a LATER same-struct method that forwards through a callable
    // value parameter still resolves the residue, in both orders.
    let compiler = Compiler::default();
    let late = "@fieldwise_init\nstruct RefBox[origin: Origin[mut=True]]:\n    var value: ref[origin] List[Int]\n\ndef stash(mut sink: Sink, box: RefBox):\n    sink.slot = box^\n\n@fieldwise_init\nstruct Sink:\n    var slot: RefBox\n\n    def via(mut self, var box: RefBox):\n        self.feed[stash](box^)\n\n    def feed[callback: def(mut Sink, RefBox) thin](mut self, var box: RefBox):\n        callback(self, box^)\n\ndef make(mut keep: List[Int]) -> Sink:\n    ref whole = keep\n    var sink = Sink(RefBox(whole))\n    var local: List[Int] = [9]\n    ref alias = local\n    sink.via(RefBox(alias))\n    return sink^\n\ndef main():\n    var keep: List[Int] = [1]\n    var got = make(keep)\n";
    let early = late.replace(
        "    def via(mut self, var box: RefBox):\n        self.feed[stash](box^)\n\n    def feed[callback: def(mut Sink, RefBox) thin](mut self, var box: RefBox):\n        callback(self, box^)",
        "    def feed[callback: def(mut Sink, RefBox) thin](mut self, var box: RefBox):\n        callback(self, box^)\n\n    def via(mut self, var box: RefBox):\n        self.feed[stash](box^)",
    );
    for source in [late, early.as_str()] {
        let error = compiler
            .compile_unlinked(source)
            .expect_err("transferred local-rooted loan must not escape at the return");
        assert!(
            matches!(
                error,
                CompilerError::Type(
                    mojito::TypeError::ReturnsReferenceToLocal
                        | mojito::TypeError::StoredReferenceEscapesOrigin
                )
            ),
            "{error}"
        );
    }
}

#[test]
fn captured_store_effects_replay_at_closure_invocations() {
    // A store through a CAPTURED enclosing owner inside a nested def records
    // a concrete `Bound`-destination effect (the capture-channel residue is
    // closed): the invocation replays it — a local-rooted actual escapes —
    // and the enclosing method re-abstracts it so ITS callers replay too.
    // The seeded `List.append` chain requires the linked compiler.
    let compiler = Compiler::default();
    let escape = "@fieldwise_init\nstruct RefBox[origin: Origin[mut=True]]:\n    var value: ref[origin] List[Int]\n\n@fieldwise_init\nstruct Keeper:\n    var items: List[RefBox]\n\n    def add_local(mut self):\n        var local: List[Int] = [9]\n        ref alias = local\n        def push(var box: RefBox) {mut self}:\n            self.items.append(box^)\n        push(RefBox(alias))\n\ndef main():\n    var items: List[RefBox] = List[RefBox]()\n    var k = Keeper(items^)\n    k.add_local()\n";
    let error = compiler
        .compile_unlinked(escape)
        .expect_err("local-rooted loan through the captured store must escape");
    assert!(
        matches!(error, CompilerError::Type(_)) && error.to_string().contains("escapes storage"),
        "{error}"
    );

    let transitive = "@fieldwise_init\nstruct RefBox[origin: Origin[mut=True]]:\n    var value: ref[origin] List[Int]\n\n@fieldwise_init\nstruct Keeper:\n    var items: List[RefBox]\n\n    def add_param(mut self, var box: RefBox):\n        def push(var b: RefBox) {mut self}:\n            self.items.append(b^)\n        push(box^)\n\ndef main():\n    var items: List[RefBox] = List[RefBox]()\n    var k = Keeper(items^)\n    var local: List[Int] = [9]\n    ref alias = local\n    k.add_param(RefBox(alias))\n    local.append(1)\n    print(k.items[0].value[0])\n";
    let error = compiler
        .compile_unlinked(transitive)
        .expect_err("the re-abstracted effect must reach the method's caller");
    assert!(
        matches!(error, CompilerError::Ownership(_))
            && error
                .to_string()
                .contains("conflicts with live reference 'k'"),
        "{error}"
    );
}

#[test]
fn abstract_dispatch_replays_the_conformer_effect_union() {
    // A trait-method call on a bounded type parameter has no concrete body;
    // the checker replays the union of transfer effects over every
    // conforming implementation (the whole-program dispatch set). The
    // overloaded sibling keeps `feed` on the abstract erased-dispatch path.
    // Both conformer declaration orders converge through the two-phase pass.
    let compiler = Compiler::default();
    let conformer_first = "@fieldwise_init\nstruct RefBox[origin: Origin[mut=True]]:\n    var value: ref[origin] List[Int]\n\ntrait Sink:\n    def put(mut self, var box: RefBox): ...\n\n@fieldwise_init\nstruct Bag(Sink):\n    var slot: RefBox\n\n    def put(mut self, var box: RefBox):\n        self.slot = box^\n\ndef feed[T: Sink](mut sink: T, var box: RefBox):\n    sink.put(box^)\n\ndef feed(x: Int):\n    print(x)\n\ndef main():\n    var keep: List[Int] = [1]\n    ref whole = keep\n    var bag = Bag(RefBox(whole))\n    var local: List[Int] = [9]\n    ref alias = local\n    feed(bag, RefBox(alias))\n    local.append(1)\n    print(bag.slot.value[0])\n";
    let conformer_last = "@fieldwise_init\nstruct RefBox[origin: Origin[mut=True]]:\n    var value: ref[origin] List[Int]\n\ntrait Sink:\n    def put(mut self, var box: RefBox): ...\n\ndef feed[T: Sink](mut sink: T, var box: RefBox):\n    sink.put(box^)\n\ndef feed(x: Int):\n    print(x)\n\n@fieldwise_init\nstruct Bag(Sink):\n    var slot: RefBox\n\n    def put(mut self, var box: RefBox):\n        self.slot = box^\n\ndef main():\n    var keep: List[Int] = [1]\n    ref whole = keep\n    var bag = Bag(RefBox(whole))\n    var local: List[Int] = [9]\n    ref alias = local\n    feed(bag, RefBox(alias))\n    local.append(1)\n    print(bag.slot.value[0])\n";
    for source in [conformer_first, conformer_last] {
        let error = compiler
            .compile_unlinked(source)
            .expect_err("mutating the transferred loan's source must conflict");
        assert!(
            matches!(error, CompilerError::Ownership(_))
                && error
                    .to_string()
                    .contains("conflicts with live reference 'bag'"),
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
    let source = "@fieldwise_init\nstruct RefBox[origin: Origin[mut=True]]:\n    var value: ref[origin] List[Int]\n\ndef stash(mut sink: List[RefBox], var box: RefBox):\n    sink.append(box^)\n\ndef stash(x: Int):\n    print(x)\n\ndef collect() -> List[RefBox]:\n    var sink: List[RefBox] = List[RefBox]()\n    var local: List[Int] = [9]\n    ref alias = local\n    stash(sink, RefBox(alias))\n    return sink^\n\ndef main():\n    var got = collect()\n";
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
    let conflict = "@fieldwise_init\nstruct RefBox[origin: Origin[mut=True]]:\n    var value: ref[origin] List[Int]\n\n@fieldwise_init\nstruct Stasher(def(mut List[RefBox], RefBox)):\n    var count: Int\n    def __call__(mut self, mut sink: List[RefBox], box: RefBox):\n        self.count += 1\n        sink.append(box^)\n\ndef main():\n    var s = Stasher(0)\n    var sink: List[RefBox] = List[RefBox]()\n    var local: List[Int] = [9]\n    ref alias = local\n    s(sink, RefBox(alias))\n    local.append(1)\n    print(sink[0].value[0])\n";
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
    let after_last_use = "@fieldwise_init\nstruct RefBox[origin: Origin[mut=True]]:\n    var value: ref[origin] List[Int]\n\n@fieldwise_init\nstruct Stasher(def(mut List[RefBox], RefBox)):\n    var count: Int\n    def __call__(mut self, mut sink: List[RefBox], box: RefBox):\n        self.count += 1\n        sink.append(box^)\n\ndef main():\n    var s = Stasher(0)\n    var sink: List[RefBox] = List[RefBox]()\n    var local: List[Int] = [9]\n    ref alias = local\n    s(sink, RefBox(alias))\n    print(sink[0].value[0])\n    local.append(1)\n    print(local[1])\n";
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
    let escaping = format!("{}        break\n", exhaustive);
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

    let user_iterator = "@fieldwise_init\nstruct StopIteration:\n    pass\n\n@explicit_destroy(\"close Conn\")\nstruct Conn(Movable, Deinitable where False):\n    var id: Int\n\n    def __init__(out self, id: Int):\n        self.id = id\n\n    def close(deinit self):\n        print(\"close\", self.id)\n\nstruct Drain(Iterator, Movable):\n    comptime Element = Conn\n    var remaining: Int\n\n    def __init__(out self, remaining: Int):\n        self.remaining = remaining\n\n    def __next__(mut self) raises StopIteration -> Conn:\n        if self.remaining == 0:\n            raise StopIteration()\n        self.remaining -= 1\n        return Conn(self.remaining)\n\nstruct Bucket(Movable):\n    var count: Int\n\n    def __init__(out self, count: Int):\n        self.count = count\n\n    def __iter__(var self) -> Drain:\n        return Drain(self.count)\n\ndef main():\n    var bucket = Bucket(2)\n    for var item in bucket^:\n        item^.close()\n";
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
    let source = "@fieldwise_init\nstruct RefBox[origin: Origin[mut=True]]:\n    var value: ref[origin] List[Int]\n\n@fieldwise_init\nstruct Holder:\n    var slot: RefBox\n\n    def stash_local(mut self):\n        def install() {mut self}:\n            var local: List[Int] = [7]\n            ref alias = local\n            self.slot = RefBox(alias)\n        install()\n\ndef main():\n    var keep: List[Int] = [1]\n    ref whole = keep\n    var holder = Holder(RefBox(whole))\n    holder.stash_local()\n    print(holder.slot.value[0])\n";
    let error = compiler
        .compile_unlinked(source)
        .expect_err("nested-def store");
    assert!(matches!(
        error,
        CompilerError::Type(mojito::TypeError::StoredReferenceEscapesOrigin)
    ));

    // The parameter-rooted twin stays accepted: the loan the nested def
    // installs roots at the enclosing method's `mut` parameter, which is
    // caller-visible storage. (End-to-end execution of capture-installed
    // reference reads is a recorded capture-channel residue.)
    let param_rooted = "@fieldwise_init\nstruct RefBox[origin: Origin[mut=True]]:\n    var value: ref[origin] List[Int]\n\n@fieldwise_init\nstruct Holder:\n    var slot: RefBox\n\n    def stash_param(mut self, mut source: List[Int]):\n        def install() {mut self, ref source}:\n            ref alias = source\n            self.slot = RefBox(alias)\n        install()\n\ndef main():\n    var keep: List[Int] = [1]\n    ref whole = keep\n    var holder = Holder(RefBox(whole))\n    var other: List[Int] = [5]\n    holder.stash_param(other)\n";
    compiler
        .compile_unlinked(param_rooted)
        .expect("param-rooted nested-def store stays accepted");

    // Frame balance: a store BESIDE (after) a nested def, in the method's own
    // body, still faces the guard — the nested frame pushes and pops without
    // disturbing the method's escape context.
    let adjacent = "@fieldwise_init\nstruct RefBox[origin: Origin[mut=True]]:\n    var value: ref[origin] List[Int]\n\n@fieldwise_init\nstruct Holder:\n    var slot: RefBox\n\n    def stash_local(mut self):\n        def helper(x: Int) -> Int:\n            return x\n        var local: List[Int] = [helper(7)]\n        ref alias = local\n        self.slot = RefBox(alias)\n\ndef main():\n    var keep: List[Int] = [1]\n    ref whole = keep\n    var holder = Holder(RefBox(whole))\n    holder.stash_local()\n";
    let error = compiler
        .compile_unlinked(adjacent)
        .expect_err("adjacent store");
    assert!(matches!(
        error,
        CompilerError::Type(mojito::TypeError::StoredReferenceEscapesOrigin)
    ));
}

#[test]
fn transfer_effect_visibility_is_declaration_order_independent() {
    // The two-phase effects pass: an earlier method calling a LATER
    // same-struct storing method now carries the callee's transfer effect —
    // the check reruns, seeded with the first round's committed effects,
    // whenever a call site observed a stale callee entry — so both
    // declaration orders reject identically.
    let compiler = Compiler::default();
    let late = "@fieldwise_init\nstruct RefBox[origin: Origin[mut=True]]:\n    var value: ref[origin] List[Int]\n\n@fieldwise_init\nstruct Sink:\n    var slot: RefBox\n\n    def via(mut self, var box: RefBox):\n        self.stash(box^)\n\n    def stash(mut self, var box: RefBox):\n        self.slot = box^\n\ndef make(mut keep: List[Int]) -> Sink:\n    ref whole = keep\n    var sink = Sink(RefBox(whole))\n    var local: List[Int] = [9]\n    ref alias = local\n    sink.via(RefBox(alias))\n    return sink^\n\ndef main():\n    var keep: List[Int] = [1]\n    var got = make(keep)\n";
    let error = compiler.compile_unlinked(late).expect_err("late order");
    assert!(matches!(
        error,
        CompilerError::Type(mojito::TypeError::ReturnsReferenceToLocal)
    ));
    let early = late.replace(
        "    def via(mut self, var box: RefBox):\n        self.stash(box^)\n\n    def stash(mut self, var box: RefBox):\n        self.slot = box^",
        "    def stash(mut self, var box: RefBox):\n        self.slot = box^\n\n    def via(mut self, var box: RefBox):\n        self.stash(box^)",
    );
    let error = compiler.compile_unlinked(&early).expect_err("early order");
    assert!(matches!(
        error,
        CompilerError::Type(mojito::TypeError::ReturnsReferenceToLocal)
    ));
}

#[test]
fn recursion_only_transfer_effects_reach_the_fixpoint() {
    // Mutually recursive methods where the store is reachable only through
    // the recursive partner: round one leaves the earlier method effect-free
    // (its callee's body is uncommitted at the call site), the rerun closes
    // the cycle, and the caller's return-escape rejects.
    let compiler = Compiler::default();
    let source = "@fieldwise_init\nstruct RefBox[origin: Origin[mut=True]]:\n    var value: ref[origin] List[Int]\n\n@fieldwise_init\nstruct Sink:\n    var slot: RefBox\n\n    def ping(mut self, var box: RefBox, n: Int):\n        if n > 0:\n            self.pong(box^, n - 1)\n        else:\n            self.slot = box^\n\n    def pong(mut self, var box: RefBox, n: Int):\n        self.ping(box^, n)\n\ndef make(mut keep: List[Int]) -> Sink:\n    ref whole = keep\n    var sink = Sink(RefBox(whole))\n    var local: List[Int] = [9]\n    ref alias = local\n    sink.pong(RefBox(alias), 1)\n    return sink^\n\ndef main():\n    var keep: List[Int] = [1]\n    var got = make(keep)\n";
    let error = compiler
        .compile_unlinked(source)
        .expect_err("mutual recursion");
    assert!(matches!(
        error,
        CompilerError::Type(mojito::TypeError::ReturnsReferenceToLocal)
    ));
}

#[test]
fn augmented_assignment_replays_the_dunders_transfer_effects() {
    // The in-place dunder goes through ordinary method selection, so a user
    // `__iadd__` that stashes its loan-carrying argument into `self` already
    // installs the transfer at the `sink += carrier` site: returning the
    // receiver with a local-rooted transferred loan rejects.
    let compiler = Compiler::default();
    let source = "@fieldwise_init\nstruct RefBox[origin: Origin[mut=True]]:\n    var value: ref[origin] List[Int]\n\n@fieldwise_init\nstruct Sink:\n    var slot: RefBox\n\n    def __iadd__(mut self, var box: RefBox):\n        self.slot = box^\n\ndef make(mut keep: List[Int]) -> Sink:\n    ref whole = keep\n    var sink = Sink(RefBox(whole))\n    var local: List[Int] = [9]\n    ref alias = local\n    sink += RefBox(alias)\n    return sink^\n\ndef main():\n    var keep: List[Int] = [1]\n    var got = make(keep)\n";
    let error = compiler.compile_unlinked(source).expect_err("augassign");
    assert!(matches!(
        error,
        CompilerError::Type(mojito::TypeError::ReturnsReferenceToLocal)
    ));
}

#[test]
fn unpack_into_place_faces_the_store_outward_guard() {
    // Unpacking into fields of `self` runs the same store-outward rule as
    // ordinary place assignment: a tuple element carrying a frame-local loan
    // rejects with the escape diagnostic (previously this shape surfaced an
    // incidental non-Copyable unpack error), while the parameter-rooted twin
    // passes the guard and still lands on the pre-existing
    // implicitly-copyable rvalue-unpack requirement.
    let compiler = Compiler::default();
    let escaping = "@fieldwise_init\nstruct RefBox[origin: Origin[mut=True]]:\n    var value: ref[origin] List[Int]\n\n@fieldwise_init\nstruct Pair:\n    var a: RefBox\n    var b: Int\n\n    def fill(mut self):\n        var local: List[Int] = [9]\n        ref alias = local\n        var pack = (RefBox(alias), 5)\n        self.a, self.b = pack^\n\ndef main():\n    var keep: List[Int] = [1]\n    ref whole = keep\n    var pair = Pair(RefBox(whole), 0)\n    pair.fill()\n";
    let error = compiler
        .compile_unlinked(escaping)
        .expect_err("escaping unpack");
    assert!(matches!(
        error,
        CompilerError::Type(mojito::TypeError::StoredReferenceEscapesOrigin)
    ));

    let param_rooted = escaping.replace(
        "    def fill(mut self):\n        var local: List[Int] = [9]\n        ref alias = local\n",
        "    def fill(mut self, mut source: List[Int]):\n        ref alias = source\n",
    );
    let param_rooted = param_rooted.replace(
        "pair.fill()",
        "var src: List[Int] = [9]\n    pair.fill(src)",
    );
    let error = compiler
        .compile_unlinked(&param_rooted)
        .expect_err("copyable wall");
    let message = format!("{error}");
    assert!(message.contains("implicitly copyable"), "{message}");
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
