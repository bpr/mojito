//! Tests for the canonical overload-symbol module (`src/symbol.rs`): the one
//! owner of signature identity and `$ov$` lowered-name formatting. They pin the
//! external spellings, prove the checker-recorded callee names the exact MIR
//! function (no drift between the two manglings), and scan the source tree so a
//! hand-built overload symbol outside the module is caught.

use mojito::Ty;
use std::collections::HashSet;

use mojito::checker::resolve_overload_targets;
use mojito::mir::lower_program;
use mojito::parse;

/// `src` linked against the bundled standard library and elaborated, the
/// program the check takes: `Int`'s operators are `SIMD`'s library dunders.
fn linked_program(src: &str) -> Vec<mojito::Stmt> {
    let linked =
        mojito::link_source(src, std::path::Path::new("symbol_test.mojo")).expect("link error");
    mojito::elaborate(linked).expect("elaborate")
}

/// The lowered function names `lower_program` emits for `src`.
fn lowered_names(src: &str) -> HashSet<String> {
    lower_program(&linked_program(src))
        .expect("type error")
        .functions
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}

#[test]
fn free_function_overloads_get_signature_qualified_names() {
    let names = lowered_names(
        "def pick() -> Int:\n    return 0\n\
         def pick(x: Int) -> Int:\n    return x\n\
         def pick(s: StringLiteral) -> Int:\n    return 1\n",
    );
    assert!(names.contains("pick$ov$"), "zero-arg overload: {names:?}");
    assert!(names.contains("pick$ov$Int"), "{names:?}");
    assert!(names.contains("pick$ov$StringLiteral"), "{names:?}");
}

#[test]
fn keyword_variadic_role_is_part_of_callable_identity() {
    let source = "def route(value: Int) -> Int:\n    return 1\n\ndef route(var **options: Int) -> Int:\n    return 2\n\ndef main():\n    print(route(7))\n    print(route(answer=7))\n";
    let names = lowered_names(source);
    assert!(names.contains("route$ov$Int"), "{names:?}");
    assert!(names.contains("route$ov$$kwv$Int"), "{names:?}");

    let program = parse(source).expect("parse error");
    let targets = resolve_overload_targets(&program).expect("check error");
    for target in targets
        .values()
        .filter(|target| target.starts_with("route$ov$"))
    {
        assert!(
            names.contains(target),
            "missing lowered target {target}: {names:?}"
        );
    }
}

#[test]
fn generic_keyword_variadic_role_is_part_of_callable_identity() {
    let source = "def route[T: Copyable & Movable](value: T) -> Int:\n    return 1\n\ndef route[T: Copyable & Movable & Deinitable](var **options: T) -> Int:\n    return 2\n\ndef main():\n    print(route(7))\n    print(route(answer=7))\n";
    let names = lowered_names(source);
    let route_names: HashSet<_> = names
        .iter()
        .filter(|name| name.starts_with("route$ov$"))
        .cloned()
        .collect();
    assert_eq!(route_names.len(), 2, "{route_names:?}");
    assert!(
        route_names.iter().any(|name| name.contains("$kwv$")),
        "{route_names:?}"
    );

    let program = parse(source).expect("parse error");
    let targets = resolve_overload_targets(&program).expect("check error");
    let route_targets: HashSet<_> = targets
        .values()
        .filter(|target| target.starts_with("route$ov$"))
        .cloned()
        .collect();
    assert_eq!(route_targets.len(), 2, "{route_targets:?}");
    assert!(route_targets.iter().all(|target| names.contains(target)));
}

#[test]
fn parameterized_annotation_type_parameters_spell_their_bounds() {
    // `Box[T]` in argument position arrives from the parser as a value
    // identifier; a declared type parameter there mangles as the checker's
    // `TyKind::Param` does, so the overloaded generic member is the symbol its
    // call names.
    let source = "struct Box[T: Copyable & Movable & Deinitable]:\n    var item: Self.T\n\n    def __init__(out self, var item: Self.T):\n        self.item = item^\n\ndef pick[T: Copyable & Movable & Deinitable](b: Box[T]) -> Int:\n    return 1\n\ndef pick(x: Int) -> Int:\n    return x\n\ndef main():\n    print(pick(Box(7)))\n    print(pick(5))\n";
    let names = lowered_names(source);
    assert!(
        names.contains("pick$ov$Box$T$Copyable$Movable$Deinitable"),
        "{names:?}"
    );

    let program = parse(source).expect("parse error");
    let targets = resolve_overload_targets(&program).expect("check error");
    let pick_targets: HashSet<_> = targets
        .values()
        .filter(|target| target.starts_with("pick$ov$"))
        .cloned()
        .collect();
    assert_eq!(pick_targets.len(), 2, "{pick_targets:?}");
    assert!(
        pick_targets.iter().all(|target| names.contains(target)),
        "{pick_targets:?} vs {names:?}"
    );
}

#[test]
fn non_overloaded_def_keeps_its_source_name() {
    let names = lowered_names("def solo(x: Int) -> Int:\n    return x\n");
    assert!(names.contains("solo"), "{names:?}");
}

#[test]
fn method_symbol_splits_outside_baked_type_arguments() {
    use mojito::symbol::{retarget_method_symbol, split_method_symbol};
    let clone = "Box.__init__$y22:SIMD[DType.float32, 2]";
    assert_eq!(
        split_method_symbol(clone),
        Some(("Box", "__init__$y22:SIMD[DType.float32, 2]"))
    );
    assert_eq!(
        retarget_method_symbol(clone, "Box$mono$TSIMD$DType$float32$$2$").as_deref(),
        Some("Box$mono$TSIMD$DType$float32$$2$.__init__$y22:SIMD[DType.float32, 2]")
    );
    assert_eq!(
        split_method_symbol("List[Int].append"),
        Some(("List[Int]", "append"))
    );
    assert_eq!(split_method_symbol("SIMD[DType.float32, 2]"), None);
}

#[test]
fn method_and_constructor_overloads_get_qualified_names() {
    let names = lowered_names(
        "struct Box:\n    var n: Int\n\
         \n    def __init__(out self):\n        self.n = 0\n\
         \n    def __init__(out self, n: Int):\n        self.n = n\n\
         \n    def value(self) -> Int:\n        return self.n\n\
         \n    def value(self, add: Int) -> Int:\n        return self.n + add\n",
    );
    assert!(names.contains("Box.__init__$ov$"), "{names:?}");
    assert!(names.contains("Box.__init__$ov$Int"), "{names:?}");
    assert!(names.contains("Box.value$ov$"), "{names:?}");
    assert!(names.contains("Box.value$ov$Int"), "{names:?}");
}

#[test]
fn overloaded_trait_requirements_get_qualified_names() {
    // Each overloaded requirement owns its binders under its qualified name,
    // so two `pick[T]` requirements bind distinct `T`s.
    use mojito::ast::StmtKind;
    use mojito::symbol::{MethodShape, OverloadSets, lowered_method_name};
    let program = parse(
        "trait Picker:\n\
         \x20   def pick[T: Copyable](self, a: T) -> T: ...\n\
         \x20   def pick[T: Copyable](self, a: T, b: T) -> T: ...\n\
         \x20   def size(self) -> Int: ...\n",
    )
    .expect("parse error");
    let sets = OverloadSets::scan(&program);
    let owners: Vec<_> = program
        .iter()
        .filter_map(|stmt| match &stmt.kind {
            StmtKind::Trait { methods, .. } => Some(methods),
            _ => None,
        })
        .flatten()
        .map(|m| {
            lowered_method_name(
                &format!("Picker.{}", m.name),
                &[],
                MethodShape {
                    type_params: &[],
                    params: &m.params,
                    keyword_only: m.keyword_only,
                    has_self: true,
                    self_convention: m.self_convention,
                },
                &sets,
                &mojito::symbol::NoModuleNames,
            )
        })
        .collect();
    assert_eq!(
        owners,
        ["Picker.pick$ov$T", "Picker.pick$ov$T$T", "Picker.size"],
    );
}

#[test]
fn overloaded_struct_methods_own_distinct_binders_numbered_past_origins() {
    // The checker and the elaborator share one owner per method and one slot
    // per binder: two `pick[T]` overloads own distinct `T`s, and the origin
    // parameter before `U` takes no slot.
    use mojito::ast::StmtKind;
    use mojito::symbol::{MethodBinderOwners, OverloadSets};
    let program = parse(
        "struct Box:\n\
         \x20   def pick[T: Copyable](self, a: T) -> T:\n\
         \x20       return a.copy()\n\
         \x20   def pick[T: Copyable](self, a: T, b: Int) -> T:\n\
         \x20       return a.copy()\n\
         \x20   def view[o: Origin, U: Copyable](self, a: U) -> U:\n\
         \x20       return a.copy()\n",
    )
    .expect("parse error");
    let owners = MethodBinderOwners::scan(
        &program,
        &OverloadSets::scan(&program),
        &mojito::symbol::NoModuleNames,
    );
    let methods = program
        .iter()
        .find_map(|stmt| match &stmt.kind {
            StmtKind::Struct { methods, .. } => Some(methods),
            _ => None,
        })
        .expect("struct");
    let named: Vec<_> = methods.iter().map(|m| owners.owner("Box", m)).collect();
    assert_eq!(
        named,
        [
            "Box.pick$ov$T$Copyable",
            "Box.pick$ov$T$Copyable$Int",
            "Box.view"
        ]
    );
    assert_eq!(
        owners.owner("Box$y3:Int", &methods[1]),
        "Box.pick$ov$T$Copyable$Int"
    );
    let view = &methods[2].type_params;
    assert_eq!(view[1].binder_slot(view), 0);
}

#[test]
fn mojo_copy_constructor_counts_as_copyinit_not_an_init_overload() {
    // One ordinary `__init__` plus the `out self, *, copy: Self` form: the copy
    // constructor is modeled as `__copyinit__`, so neither is overloaded.
    let names = lowered_names(
        "struct Res:\n    var n: Int\n\
         \n    def __init__(out self, n: Int):\n        self.n = n\n\
         \n    def __init__(out self, *, copy: Self):\n        self.n = copy.n\n",
    );
    assert!(names.contains("Res.__init__"), "{names:?}");
    assert!(names.contains("Res.__copyinit__"), "{names:?}");
}

#[test]
fn struct_and_generic_parameter_types_mangle_from_their_annotations() {
    let names = lowered_names(
        "@fieldwise_init\nstruct Point:\n    var x: Int\n\
         @fieldwise_init\nstruct Pair[T: Movable & Deinitable]:\n    var a: Self.T\n    var b: Self.T\n\
         def pick(p: Point) -> Int:\n    return p.x\n\
         def pick(n: Int) -> Int:\n    return n\n\
         def pick(q: Pair[Int]) -> Int:\n    return q.a\n",
    );
    assert!(names.contains("pick$ov$Point"), "{names:?}");
    assert!(names.contains("pick$ov$Int"), "{names:?}");
    assert!(names.contains("pick$ov$Pair$Int"), "{names:?}");
}

#[test]
fn nested_defs_lift_to_dollar_joined_names() {
    let names = lowered_names(
        "def outer(x: Int) -> Int:\n\
         \x20   def inner(y: Int) {imm x} -> Int:\n\
         \x20       return y + x\n\
         \x20   return inner(1)\n",
    );
    assert!(names.contains("outer$inner$__storage.inner"), "{names:?}");
}

#[test]
fn deeply_nested_defs_use_the_full_lexical_symbol_path() {
    let names = lowered_names(
        "def outer(x: Int) -> Int:\n\
         \x20   def middle() {x} -> Int:\n\
         \x20       def inner() {x} -> Int:\n\
         \x20           return x\n\
         \x20       return inner()\n\
         \x20   return middle()\n",
    );
    assert!(names.contains("outer$middle$__storage.middle"), "{names:?}");
    assert!(
        names.contains("outer$middle$__storage.middle$inner$__storage.inner"),
        "{names:?}"
    );
}

#[test]
fn same_named_nested_defs_in_distinct_blocks_receive_distinct_lifted_symbols() {
    let names = lowered_names(
        "def outer(flag: Bool) -> Int:\n\
         \x20   if flag:\n\
         \x20       def choose(value: Int) -> Int:\n\
         \x20           return value\n\
         \x20       return choose(1)\n\
         \x20   else:\n\
         \x20       def choose(value: Int) -> Int:\n\
         \x20           return value + 1\n\
         \x20       return choose(1)\n",
    );
    let lifted = names
        .iter()
        .filter(|name| name.starts_with("outer$choose$decl"))
        .count();
    assert_eq!(lifted, 2, "{names:?}");
}

#[test]
fn nested_declaration_symbols_are_derived_from_checked_ids() {
    assert_eq!(
        mojito::symbol::nested_lifted_declaration_name("outer", "choose", mojito::CheckedDeclId(7),),
        "outer$choose$decl7"
    );
}

/// The drift regression: every callee the checker records for an overloaded
/// call must name a function the MIR actually emits — including struct-typed,
/// generic, and `Self.T`-typed parameters, which previously mangled differently
/// on the two sides (`pick$ov$Struct$Point` vs `pick$ov$Point`).
#[test]
fn checker_recorded_callees_name_real_mir_functions() {
    let src = "@fieldwise_init\nstruct Point:\n    var x: Int\n\
         @fieldwise_init\nstruct Pair[T: Movable & Deinitable]:\n    var a: Self.T\n    var b: Self.T\n\
         struct Box:\n    var n: Int\n\
         \n    def __init__(out self):\n        self.n = 0\n\
         \n    def __init__(out self, n: Int):\n        self.n = n\n\
         \n    def get(self) -> Int:\n        return self.n\n\
         \n    def get(self, p: Point) -> Int:\n        return self.n + p.x\n\
         def pick(p: Point) -> Int:\n    return p.x\n\
         def pick(n: Int) -> Int:\n    return n + 1\n\
         def pick(q: Pair[Int]) -> Int:\n    return q.a\n\
         def main():\n\
         \x20   print(pick(Point(7)))\n\
         \x20   print(pick(1))\n\
         \x20   print(pick(Pair(1, 2)))\n\
         \x20   var b: Box = Box(5)\n\
         \x20   print(b.get(), b.get(Point(3)))\n";
    let program = linked_program(src);
    let targets = resolve_overload_targets(&program).expect("check error");
    let names: HashSet<String> = lower_program(&program)
        .expect("type error")
        .functions
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert!(!targets.is_empty(), "expected recorded overload targets");
    // The program's own calls: the bundled library's are its package's.
    let own = targets
        .iter()
        .filter(|(site, _)| {
            !site
                .source
                .as_deref()
                .is_some_and(|source| source.contains("/stdlib/std/"))
        })
        .map(|(_, target)| target);
    for target in own {
        assert!(
            names.contains(target),
            "checker target '{target}' names no MIR function; emitted: {names:?}"
        );
    }
}

/// The bundled Mojo bodies the compiler calls by symbol reach every linked
/// program under exactly the spellings `mojito_symbol::symbol` names. Renaming
/// `stdlib/std/_intrinsics.mojo` or one of its `def`s would otherwise drop
/// integer display back to the VM's Rust rendering with nothing failing.
#[test]
fn compiler_called_intrinsic_bodies_keep_their_lowered_symbols() {
    let linked = mojito::link_source(
        "def main():\n    print(1)\n",
        std::path::Path::new("symbol_test.mojo"),
    )
    .expect("link error");
    let program = mojito::elaborate(linked).expect("comptime error");
    let names: HashSet<String> = lower_program(&program)
        .expect("type error")
        .functions
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    for symbol in [
        mojito::symbol::INT_DIGITS_SYMBOL,
        mojito::symbol::UINT_DIGITS_SYMBOL,
    ] {
        assert!(
            names.contains(symbol),
            "the bundled `{symbol}` body is not linked into every program"
        );
    }
}

#[test]
fn self_typed_overload_keys_agree_between_declaration_and_call() {
    // A same-arity overload whose parameter is the enclosing struct type keys as
    // `$ov$Self` on both sides, not `$ov$Pair`: the declaration mangles the bare
    // `Self` annotation, and the call side canonicalizes the resolved receiver
    // type back to `Self`. Before the fix these diverged (`$ov$Self` defined,
    // `$ov$Pair` recorded) and the VM raised "unknown method". (The generic
    // spelling — `List[Self.T]` respelled to `Self` — is covered end-to-end by
    // the standard library's consuming `List.extend` in the evaluator suite; a
    // `T`-typed user struct cannot be checked in this raw parse+check seam.)
    let src = "struct Pair:\n    var a: Int\n\
         \n    def __init__(out self, a: Int):\n        self.a = a\n\
         \n    def merge(self, other: Self) -> Int:\n        return self.a + other.a\n\
         \n    def merge(self, n: Int) -> Int:\n        return self.a + n\n\
         def main():\n\
         \x20   var p: Pair = Pair(1)\n\
         \x20   var q: Pair = Pair(2)\n\
         \x20   print(p.merge(q), p.merge(5))\n";
    let program = linked_program(src);
    let targets = resolve_overload_targets(&program).expect("check error");
    let names: HashSet<String> = lower_program(&program)
        .expect("type error")
        .functions
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    // Declaration side names the self-typed overload `$ov$Self`.
    assert!(names.contains("Pair.merge$ov$Self"), "{names:?}");
    assert!(names.contains("Pair.merge$ov$Int"), "{names:?}");
    // Call side records the same `$ov$Self` callee, and every recorded callee
    // names a real MIR function (no declaration/call-site drift).
    assert!(
        targets.values().any(|t| t == "Pair.merge$ov$Self"),
        "expected a recorded `$ov$Self` call target; got: {targets:?}"
    );
    // The program's own calls: the bundled library's are its package's.
    let own = targets
        .iter()
        .filter(|(site, _)| {
            !site
                .source
                .as_deref()
                .is_some_and(|source| source.contains("/stdlib/std/"))
        })
        .map(|(_, target)| target);
    for target in own {
        assert!(
            names.contains(target),
            "checker target '{target}' names no MIR function; emitted: {names:?}"
        );
    }
}

#[test]
fn mlir_typed_overload_keys_agree_between_declaration_and_call() {
    // An overload over `__mlir_type.index` keys by the type the annotation
    // resolves to on both sides, so the recorded callee names the declared
    // MIR function.
    let src = "struct Index:\n\
         \x20   var _mlir_value: __mlir_type.index\n\
         \n    def __init__(out self, *, mlir_value: __mlir_type.index):\n\
         \x20       self._mlir_value = mlir_value\n\
         \n    def __init__(out self, value: Int, /):\n\
         \x20       self._mlir_value = value.__mlir_index__()\n\
         def main():\n\
         \x20   var a = Index(mlir_value=Int(3).__mlir_index__())\n\
         \x20   print(Int(mlir_value=a._mlir_value), Int(mlir_value=Index(5)._mlir_value))\n";
    let program = linked_program(src);
    let targets = resolve_overload_targets(&program).expect("check error");
    let names: HashSet<String> = lower_program(&program)
        .expect("type error")
        .functions
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert!(
        targets
            .values()
            .any(|t| t.starts_with("Index.__init__$ov$") && t.ends_with("$kw$mlir_value")),
        "expected a recorded `mlir_value` constructor target; got: {targets:?}"
    );
    // The program's own calls: the bundled library's are its package's.
    let own = targets
        .iter()
        .filter(|(site, _)| {
            !site
                .source
                .as_deref()
                .is_some_and(|source| source.contains("/stdlib/std/"))
        })
        .map(|(_, target)| target);
    for target in own {
        assert!(
            names.contains(target),
            "checker target '{target}' names no MIR function; emitted: {names:?}"
        );
    }
}

#[test]
fn type_of_parameter_overload_keys_agree_between_declaration_and_call() {
    // A parameter annotated `type_of(x)` over an earlier parameter keys as
    // `x`'s own annotation, the type the call side resolves it to.
    let src = "def pick(x: Int, y: type_of(x)) -> type_of(x):\n    return y\n\
         def pick(x: Bool, y: Bool) -> Bool:\n    return x\n\
         def main():\n\
         \x20   print(pick(1, 2), pick(True, False))\n";
    let program = parse(src).expect("parse error");
    let targets = resolve_overload_targets(&program).expect("check error");
    let names: HashSet<String> = lower_program(&program)
        .expect("type error")
        .functions
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert!(names.contains("pick$ov$Int$Int"), "{names:?}");
    // The program's own calls: the bundled library's are its package's.
    let own = targets
        .iter()
        .filter(|(site, _)| {
            !site
                .source
                .as_deref()
                .is_some_and(|source| source.contains("/stdlib/std/"))
        })
        .map(|(_, target)| target);
    for target in own {
        assert!(
            names.contains(target),
            "checker target '{target}' names no MIR function; emitted: {names:?}"
        );
    }
}

/// Repository hygiene: the `$ov$` spelling may exist only in the canonical
/// symbol module. A new hand-built overload symbol anywhere else in the
/// compiler sources (the facade's `src/` or any workspace crate under
/// `crates/`) reintroduces the checker/MIR/VM drift this module exists to
/// prevent. Comments are exempt: naming an overload symbol in prose documents
/// what `mojito::symbol` produces, it does not build one.
#[test]
fn ov_spelling_appears_only_in_the_symbol_module() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut offenders = Vec::new();
    scan_rs_files(&manifest.join("src"), &mut offenders);
    for entry in std::fs::read_dir(manifest.join("crates")).expect("read crates dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            scan_rs_files(&path.join("src"), &mut offenders);
        }
    }
    assert!(
        offenders.is_empty(),
        "'$ov$' outside the symbol module — route it through mojito::symbol: {offenders:?}"
    );
}

fn scan_rs_files(dir: &std::path::Path, offenders: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).expect("read src dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            scan_rs_files(&path, offenders);
        } else if path.extension().is_some_and(|e| e == "rs")
            && path
                .file_name()
                .is_some_and(|f| f != "symbol.rs" && f != "tests.rs")
            && std::fs::read_to_string(&path)
                .expect("read source file")
                .lines()
                // Production code only: a file's test module comes last, and
                // a crate below `mojito-symbol` cannot name its constants.
                .take_while(|line| line.trim() != "#[cfg(test)]")
                .any(|line| {
                    line.split_once("//")
                        .map_or(line, |(code, _)| code)
                        .contains("$ov$")
                })
        {
            offenders.push(path.display().to_string());
        }
    }
}

#[test]
fn collector_position_is_part_of_callable_identity() {
    let source = "def route(a: Int, *rest: Int) -> Int:\n    return 1\n\ndef route(*rest: Int, a: Int) -> Int:\n    return 2\n\ndef main():\n    print(route(1, 2))\n    print(route(2, a=1))\n";
    let names = lowered_names(source);
    let program = parse(source).expect("parse error");
    let targets = resolve_overload_targets(&program).expect("check error");
    let route_targets: HashSet<_> = targets
        .values()
        .filter(|target| target.starts_with("route$ov$"))
        .cloned()
        .collect();
    assert_eq!(route_targets.len(), 2, "{route_targets:?}");
    assert!(
        route_targets.iter().all(|target| names.contains(target)),
        "{route_targets:?} vs {names:?}"
    );
}

#[test]
fn variadic_key_spells_a_collectors_position_and_pack_bounds() {
    use mojito::ast::StmtKind;
    use mojito::symbol::VariadicKey;

    let program = parse(
        "def f[*Ts: Writable](a: Int, *rest: *Ts):\n    pass\n\n\
         def f[*Ts: Writable](*rest: *Ts, a: Int):\n    pass\n\n\
         def f[*Ts: Intable](a: Int, *rest: *Ts):\n    pass\n\n\
         def f[*Ts: Writable](b: Int, var *others: *Ts):\n    pass\n\n\
         def f(a: Int):\n    pass\n",
    )
    .expect("parse error");
    let keys: Vec<_> = program
        .iter()
        .filter_map(|statement| match &statement.kind {
            StmtKind::Def {
                params,
                type_params,
                ..
            } => Some(VariadicKey::from_ast_params(params, type_params)),
            _ => None,
        })
        .collect();
    assert_eq!(keys[0].as_ref().map(VariadicKey::position), Some(1));
    assert_eq!(keys[1].as_ref().map(VariadicKey::position), Some(0));
    assert_ne!(keys[0], keys[1], "position");
    assert_ne!(keys[0], keys[2], "pack bounds");
    assert_eq!(keys[0], keys[3], "name and convention are not identity");
    assert_eq!(keys[4], None);
}

/// Every concrete specialization value keeps its encoding byte for byte, the
/// erased-origin marker is an explicit key part, and a residual or deferred
/// value anywhere in a key's input refuses the key instead of being dropped
/// or spelled as a parameter name.
#[test]
fn param_expr_closed_specialization_keys() {
    use mojito::ast::Dtype;
    use mojito::ct::CtLane;
    use mojito::param_expr::{MetaTy, ParamContext, ParamId};
    use mojito::symbol::{SpecializationKeyPart, mangle, mangle_parts};
    use mojito::{CtValue, TyArg, TyKind};

    let float_literal = mojito::literal::FloatLiteral::parse_exact("157/50").expect("exact");
    let rendered = float_literal.to_string();
    let concrete = [
        (CtValue::Int(-3), "i-3;".to_string()),
        (CtValue::UInt(4), "u4;".to_string()),
        (
            CtValue::Float(1.0_f64.to_bits()),
            "f3ff0000000000000;".to_string(),
        ),
        (
            CtValue::IntLiteral(mojito::literal::IntLiteral::from(7_i64)),
            "I1:7".to_string(),
        ),
        (
            CtValue::FloatLiteral(float_literal),
            format!("F{}:{rendered}", rendered.len()),
        ),
        (CtValue::Bool(true), "b1;".to_string()),
        (CtValue::Str("hi".into()), "s2:hi".to_string()),
        (
            CtValue::Tuple(vec![CtValue::Int(1), CtValue::Bool(false)]),
            "t2[i1;b0;]".to_string(),
        ),
        (CtValue::List(vec![CtValue::Int(1)]), "l1[i1;]".to_string()),
        (
            CtValue::dict(None, vec![(CtValue::Str("k".into()), CtValue::Int(1))]),
            "m1[s1:ki1;]".to_string(),
        ),
        (
            CtValue::set(None, vec![CtValue::Int(9)]),
            "h1[i9;]".to_string(),
        ),
        (CtValue::Dtype(Dtype::Float32), "dfloat32;".to_string()),
        (
            CtValue::Simd {
                dtype: Dtype::UInt8,
                lanes: vec![CtLane::Int(1), CtLane::Int(2)],
            },
            "vuint8:2;[i1;i2;]".to_string(),
        ),
        (
            CtValue::Struct {
                name: "Extent".into(),
                fields: vec![("w".into(), CtValue::Int(2)), ("h".into(), CtValue::Int(3))],
            },
            "S6:Extent{i2;i3;}".to_string(),
        ),
        (CtValue::Type(Ty::int()), "y3:Int".to_string()),
    ];
    assert_eq!(concrete.len(), 15);
    for (value, encoding) in &concrete {
        assert_eq!(
            mangle("f", std::slice::from_ref(value)),
            Ok(format!("f${encoding}")),
            "{value:?}"
        );
    }
    // The owner key of a method on an origin-parameterized struct.
    assert_eq!(
        mangle_parts(
            "Holder",
            &[
                SpecializationKeyPart::Value(&CtValue::Type(Ty::int())),
                SpecializationKeyPart::ErasedOrigin,
            ],
        ),
        Ok("Holder$y3:Int$p6:origin".to_string())
    );

    // A residual: direct, nested in an aggregate, and inside a type payload.
    let context = ParamContext::new();
    let n = CtValue::Expr(context.decl_ref(ParamId::new("f", 0), "n", MetaTy::int()));
    let open = [
        n.clone(),
        CtValue::Deferred(mojito::param_expr::ParamRef::unbound("callback")),
        CtValue::Tuple(vec![CtValue::Int(1), n.clone()]),
        CtValue::Struct {
            name: "Extent".into(),
            fields: vec![("w".into(), n.clone())],
        },
        CtValue::Type(TyKind::Struct("Buf".into(), vec![TyArg::Val(n)]).into()),
    ];
    for value in &open {
        let error = mangle("f", &[CtValue::Int(1), value.clone()]).expect_err("a residual key");
        assert_eq!(error.template, "f");
    }
}

#[test]
fn a_template_name_is_recovered_from_a_key_whose_values_are_text_only() {
    use mojito::ct::CtValue;
    use mojito::symbol::{demangle_specialization, mangle, specialization_template};

    // A value key is rebuilt, so both answers agree.
    let value_keyed = mangle("Holder.take", &[CtValue::Int(3)]).expect("a constant key");
    assert_eq!(
        demangle_specialization(&value_keyed).map(|(template, _)| template),
        Some("Holder.take")
    );
    assert_eq!(specialization_template(&value_keyed), Some("Holder.take"));

    // A string or a literal key reads back exactly.
    let int_literal = mojito::literal::IntLiteral::from(-7_i64);
    let float_literal = mojito::literal::FloatLiteral::parse_exact("157/50").expect("exact");
    let literal_keyed = mangle(
        "Named",
        &[
            CtValue::Str("a:b$c".into()),
            CtValue::IntLiteral(int_literal.clone()),
            CtValue::FloatLiteral(float_literal.clone()),
        ],
    )
    .expect("a constant key");
    assert_eq!(
        demangle_specialization(&literal_keyed),
        Some((
            "Named",
            vec![
                CtValue::Str("a:b$c".into()),
                CtValue::IntLiteral(int_literal),
                CtValue::FloatLiteral(float_literal),
            ]
        ))
    );

    // A type key renders a `Ty` no parser here reads back, so only the
    // template name is recoverable — and a binder belongs to the template.
    let type_keyed = mangle("List.__hash__", &[CtValue::Type(Ty::int())]).expect("a constant key");
    assert_eq!(type_keyed, "List.__hash__$y3:Int");
    assert_eq!(demangle_specialization(&type_keyed), None);
    assert_eq!(specialization_template(&type_keyed), Some("List.__hash__"));

    // A container holding one is text-only too, and still delimits.
    let nested = mangle(
        "f",
        &[CtValue::Tuple(vec![
            CtValue::Int(1),
            CtValue::Type(Ty::bool()),
        ])],
    )
    .expect("a constant key");
    assert_eq!(demangle_specialization(&nested), None);
    assert_eq!(specialization_template(&nested), Some("f"));

    // An unspecialized name carries no suffix either way.
    assert_eq!(specialization_template("List.__hash__"), None);
    assert_eq!(demangle_specialization("List.__hash__"), None);
}

#[test]
fn a_nested_instance_argument_spells_as_its_own_symbol() {
    use mojito::symbol::{InstanceArg, instance_symbol};
    use mojito::{TyArg, TyKind};
    // Each level spells the level below once, as the symbol minted for it,
    // so the name of `W[W[…]]` grows linearly with its nesting.
    let mut ty = Ty::int();
    let mut previous = String::new();
    for _ in 0..4 {
        let name = instance_symbol("W", &[InstanceArg::Ty(ty.clone())]);
        assert!(
            previous.is_empty() || name == format!("W$mono$T{previous}"),
            "{name}"
        );
        previous.clone_from(&name);
        ty = TyKind::Struct(name, vec![TyArg::Ty(ty)]).into();
    }
    assert_eq!(previous, "W$mono$TW$mono$TW$mono$TW$mono$TInt");
}

#[test]
fn overload_sets_scan_over_a_package() {
    // An importer scans its own statements alone; the package's names stay
    // the package's.
    use mojito::symbol::{OverloadLookup, OverloadSets};
    let library = parse(
        "def pick() -> Int:\n    return 0\n\
         def pick(x: Int) -> Int:\n    return x\n\
         def solo() -> Int:\n    return 1\n",
    )
    .expect("parse error");
    let own = parse(
        "def mine(x: Int) -> Int:\n    return x\n\
         def mine(x: Int, y: Int) -> Int:\n    return y\n",
    )
    .expect("parse error");
    let sets = OverloadSets::scan(&library);
    assert!(sets.function_is_overloaded("pick", 1));
    assert!(sets.is_function("solo"));
    let mut mine = OverloadSets::default();
    mine.scan_more(&own);
    assert!(mine.function_is_overloaded("mine", 2));
    assert!(!mine.is_function("pick"));
}

#[test]
fn program_overloads_ask_the_own_sets_then_the_owning_package_declaration() {
    // An importer's sets answer an own name from its own scan and a package
    // name by looking up the reached declarations under it, decoding each
    // record only when a query names it. A package constant is answered
    // from its declaration's record likewise.
    use mojito::checked_package::{
        DeclarationRecords, DeclaringModules, PackageDeclarations, PackageScope, PackagedModule,
        ProgramConstants, ProgramOverloads,
    };
    use mojito::symbol::OverloadLookup;
    use std::sync::Arc;
    let mut library = parse(
        "comptime N = 2\n\
         def pick() -> Int:\n    return 0\n\
         def pick(x: Int) -> Int:\n    return x\n",
    )
    .expect("parse error");
    mojito::ast::stamp_source(&mut library, "/library/a.mojo");
    let mut unreached = parse("def solo() -> Int:\n    return 1\n").expect("parse error");
    mojito::ast::stamp_source(&mut unreached, "/library/b.mojo");
    library.extend(unreached);
    let declaring = DeclaringModules::of(&library).expect("one module per name");
    let program = lower_program(&library).expect("type error");
    let mut records = DeclarationRecords {
        modules: ["a", "b"]
            .map(|name| PackagedModule {
                name: name.to_string(),
                canonical: format!("/library/{name}.mojo"),
                path: format!("/library/{name}.mojo"),
                link: Vec::new(),
            })
            .into(),
        declaring,
        ..DeclarationRecords::default()
    };
    for declaration in program.declarations.functions {
        if declaration.lowered_name.starts_with("pick") || declaration.lowered_name == "solo" {
            records.place_function(declaration).expect("placed");
        }
    }
    records
        .place_constant("N", mojito::IntLiteral::from(2_i64))
        .expect("a library declaration");
    let package = Arc::new(PackageDeclarations::of_records(records).expect("the records encode"));
    let scope = PackageScope::new(
        Arc::clone(&package),
        Arc::from([true, false]),
        mojito::checked_package::ReachedFailures::default(),
    );
    let mut overloads = ProgramOverloads::importing(Some(scope.clone()));
    assert_eq!(package.decoded(), 0);
    overloads.scan_more(
        &parse(
            "def mine(x: Int) -> Int:\n    return x\n\
             def mine(x: Int, y: Int) -> Int:\n    return y\n",
        )
        .expect("parse error"),
    );
    assert_eq!(
        ProgramConstants::importing(Some(scope)).get("N"),
        Some(mojito::IntLiteral::from(2_i64))
    );
    assert_eq!(package.decoded(), 1, "only `N`'s record decoded");
    assert!(overloads.function_is_overloaded("mine", 2));
    assert!(overloads.function_is_overloaded("pick", 1));
    assert!(!overloads.is_function("solo"), "its module was not reached");
    assert!(!overloads.is_function("absent"));
}

#[test]
fn a_package_record_keys_its_constant_and_effect_summaries_by_its_directory() {
    // A record's constant is its declaration's own, and an effect summary
    // names its function by its index in the record, so a build refuses a
    // constant of another declaration, a summary of no function of its
    // record, and a function placed after a summary would shift the
    // indexes.
    use mojito::checked_package::{
        DeclarationRecords, DeclaringModules, PackageDeclarations, PackagedModule,
    };
    let mut library = parse(
        "comptime N = 2\n\
         def pick() -> Int:\n    return 0\n\
         def pick(x: Int) -> Int:\n    return x\n\
         struct S:\n    var x: Int\n    def get(self) -> Int:\n        return self.x\n",
    )
    .expect("parse error");
    mojito::ast::stamp_source(&mut library, "/library/a.mojo");
    let mut records = DeclarationRecords {
        modules: vec![PackagedModule {
            name: "a".to_string(),
            canonical: "/library/a.mojo".to_string(),
            path: "/library/a.mojo".to_string(),
            link: Vec::new(),
        }],
        declaring: DeclaringModules::of(&library).expect("one module per name"),
        ..DeclarationRecords::default()
    };
    let program = lower_program(&library).expect("type error");
    let mut functions: Vec<_> = program
        .declarations
        .functions
        .into_iter()
        .filter(|declaration| declaration.lowered_name.starts_with("pick"))
        .collect();
    let last = functions.pop().expect("two overloads");
    let picked = functions[0].lowered_name.clone();
    for declaration in functions {
        records.place_function(declaration).expect("placed");
    }
    let one = mojito::IntLiteral::from(1_i64);
    assert!(records.place_constant("S.get", one.clone()).is_err());
    records.place_constant("N", one.clone()).expect("its own");
    assert!(
        records.place_constant("N", one).is_err(),
        "a second constant"
    );
    assert!(records.place_transfer_effects("N", Vec::new()).is_err());
    assert!(
        records
            .place_call_through_effects("pick", Vec::new())
            .is_err()
    );
    records
        .place_transfer_effects(&picked, Vec::new())
        .expect("a function of its record");
    assert!(records.place_function(last).is_err(), "after a summary");
    let package = PackageDeclarations::of_records(records).expect("the records encode");
    let lowered = package.lowered_owning(&picked).expect("a record");
    assert!(lowered.transfer_effects(&picked).is_some());
    assert!(lowered.call_through_effects(&picked).is_none());
    let constant = package.lowered_owning("N").expect("a record");
    assert_eq!(
        constant.constant("N"),
        Some(&mojito::IntLiteral::from(1_i64))
    );
    assert_eq!(constant.constant("M"), None);
}

#[test]
fn a_package_answers_a_function_name_prefix_from_its_by_name_table() {
    // An overload set is one run of the package's by-name function table,
    // read in place: the base's `$ov$` overloads, and nothing beside them.
    use mojito::checked_package::{
        DeclarationRecords, DeclaringModules, PackageDeclarations, PackagedModule,
    };
    let mut library = parse(
        "def f() -> Int:\n    return 0\n\
         def f(x: Int) -> Int:\n    return x\n\
         def f(x: Bool) -> Int:\n    return 1\n\
         def fa() -> Int:\n    return 2\n\
         def g() -> Int:\n    return 3\n\
         struct fs:\n    var x: Int\n    def get(self) -> Int:\n        return self.x\n",
    )
    .expect("parse error");
    mojito::ast::stamp_source(&mut library, "/library/a.mojo");
    let mut records = DeclarationRecords {
        modules: vec![PackagedModule {
            name: "a".to_string(),
            canonical: "/library/a.mojo".to_string(),
            path: "/library/a.mojo".to_string(),
            link: Vec::new(),
        }],
        declaring: DeclaringModules::of(&library).expect("one module per name"),
        ..DeclarationRecords::default()
    };
    let program = lower_program(&library).expect("type error");
    let mut names = Vec::new();
    for declaration in program.declarations.functions {
        if declaration.lowered_name.starts_with(['f', 'g']) {
            names.push(declaration.lowered_name.clone());
            records.place_function(declaration).expect("placed");
        }
    }
    names.sort();
    let package = PackageDeclarations::of_records(records).expect("the records encode");
    let prefixed = |prefix: &str| -> Vec<String> {
        package
            .functions_prefixed(prefix)
            .map(|(name, _)| name.to_string())
            .collect()
    };
    let expected = |prefix: &str| -> Vec<String> {
        names
            .iter()
            .filter(|name| name.starts_with(prefix))
            .cloned()
            .collect()
    };
    let overloads = prefixed("f$ov$");
    assert_eq!(overloads.len(), 3, "{names:?}");
    assert_eq!(overloads, expected("f$ov$"));
    assert!(prefixed("f").contains(&"fa".to_string()));
    assert!(prefixed("f").contains(&"fs.get".to_string()));
    assert_eq!(prefixed("f"), expected("f"));
    assert_eq!(prefixed("g"), ["g"]);
    assert_eq!(prefixed("fs."), ["fs.get"]);
    assert!(prefixed("h").is_empty());
    assert!(prefixed("f$ov$Z").is_empty());
    assert_eq!(package.struct_count(), 0);
}

#[test]
fn an_applied_type_operand_mangles_as_the_type_it_spells() {
    // A bracket operand parses as an expression; `List[Pair[Int]]` keys the
    // same as the type argument it spells.
    use mojito::ast::{Expr, ExprKind, ParamArg, Type};
    use mojito::symbol::TypeKey;
    let pair = || ParamArg::Type(Type::Named("Pair".into(), vec![ParamArg::Type(Type::Int)]));
    let spelled = Type::Named("List".into(), vec![pair()]);
    let parsed = Type::Named(
        "List".into(),
        vec![ParamArg::Value(Expr::from(ExprKind::TypeApply {
            name: "Pair".into(),
            args: vec![ParamArg::Type(Type::Int)],
        }))],
    );
    assert_eq!(TypeKey::from_ast(&parsed), TypeKey::from_ast(&spelled));
    assert_eq!(TypeKey::from_ast(&parsed).as_str(), "List$Pair$Int");
}
