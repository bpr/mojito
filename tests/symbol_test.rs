//! Tests for the canonical overload-symbol module (`src/symbol.rs`): the one
//! owner of signature identity and `$ov$` lowered-name formatting. They pin the
//! external spellings, prove the checker-recorded callee names the exact MIR
//! function (no drift between the two manglings), and scan the source tree so a
//! hand-built overload symbol outside the module is caught.

use std::collections::HashSet;

use mojito::checker::resolve_overload_targets;
use mojito::mir::lower_program;
use mojito::parse;

/// The lowered function names `lower_program` emits for `src`.
fn lowered_names(src: &str) -> HashSet<String> {
    lower_program(&parse(src).expect("parse error"))
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
         def pick(s: StringLiteral) -> StringLiteral:\n    return s\n",
    );
    assert!(names.contains("pick$ov$"), "zero-arg overload: {names:?}");
    assert!(names.contains("pick$ov$Int"), "{names:?}");
    assert!(names.contains("pick$ov$String"), "{names:?}");
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
    let source = "def route[T: Copyable & Movable](value: T) -> Int:\n    return 1\n\ndef route[T: Copyable & Movable](var **options: T) -> Int:\n    return 2\n\ndef main():\n    print(route(7))\n    print(route(answer=7))\n";
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
fn non_overloaded_def_keeps_its_source_name() {
    let names = lowered_names("def solo(x: Int) -> Int:\n    return x\n");
    assert!(names.contains("solo"), "{names:?}");
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
         @fieldwise_init\nstruct Pair[T: AnyType]:\n    var a: Self.T\n    var b: Self.T\n\
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
    assert!(names.contains("outer$inner"), "{names:?}");
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
    assert!(names.contains("outer$middle"), "{names:?}");
    assert!(names.contains("outer$middle$inner"), "{names:?}");
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
    let lifted: Vec<_> = names
        .iter()
        .filter(|name| name.starts_with("outer$choose$decl"))
        .collect();
    assert_eq!(lifted.len(), 2, "{names:?}");
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
         @fieldwise_init\nstruct Pair[T: AnyType]:\n    var a: Self.T\n    var b: Self.T\n\
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
    let program = parse(src).expect("parse error");
    let targets = resolve_overload_targets(&program).expect("check error");
    let names: HashSet<String> = lower_program(&program)
        .expect("type error")
        .functions
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert!(!targets.is_empty(), "expected recorded overload targets");
    for target in targets.values() {
        assert!(
            names.contains(target),
            "checker target '{target}' names no MIR function; emitted: {names:?}"
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
    let program = parse(src).expect("parse error");
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
    for target in targets.values() {
        assert!(
            names.contains(target),
            "checker target '{target}' names no MIR function; emitted: {names:?}"
        );
    }
}

/// Repository hygiene: the `$ov$` spelling may exist only in the canonical
/// symbol module. A new hand-built overload symbol anywhere else in `src/`
/// reintroduces the checker/MIR/VM drift this module exists to prevent.
#[test]
fn ov_spelling_appears_only_in_the_symbol_module() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();
    scan_rs_files(&root, &mut offenders);
    assert!(
        offenders.is_empty(),
        "'$ov$' outside src/symbol.rs — route it through mojito::symbol: {offenders:?}"
    );
}

fn scan_rs_files(dir: &std::path::Path, offenders: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).expect("read src dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            scan_rs_files(&path, offenders);
        } else if path.extension().is_some_and(|e| e == "rs")
            && path.file_name().is_some_and(|f| f != "symbol.rs")
            && std::fs::read_to_string(&path)
                .expect("read source file")
                .contains("$ov$")
        {
            offenders.push(path.display().to_string());
        }
    }
}
