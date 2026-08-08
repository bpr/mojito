//! Self-hosting proof (Phase 6, first installment): the `stdlib/` collection types
//! are written **in mojito itself** — ordinary *generic* structs (`List[T]`,
//! `Optional[T]`, `Set[T]`, `Dict[K, V]`), no compiler intrinsic. Each test writes
//! a small entry program that imports through the bundled `stdlib/std/...` search
//! root and runs on the VM.

use mojito::{BackendKind, Compiler, elaborate, link};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let id = N.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("mojito_selfhost_{}_{}", std::process::id(), id));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        TempDir(dir)
    }
    fn write(&self, rel: &str, contents: &str) -> PathBuf {
        let path = self.0.join(rel);
        std::fs::write(&path, contents).expect("write entry");
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Exercise the library and VM at the explicit link/elaborate/check boundary.
/// Authoritative executable coverage lives in `assets/ok`; the regression case
/// below also pins chained reference-returning subscript receivers.
fn run(entry: &Path) -> Result<String, String> {
    let program = link(entry).map_err(|e| e.to_string())?;
    let program = elaborate(program).map_err(|e| format!("comptime error: {e}"))?;
    let checked = mojito::check_program(&program).map_err(|e| format!("type error: {e:?}"))?;
    let mut backend = BackendKind::make("vm").expect("the register VM is implemented");
    backend
        .run(&checked)
        .map_err(|e| format!("runtime error: {e:?}"))?;
    Ok(backend.output())
}

/// Run through the authoritative whole-program `Compiler` pipeline
/// (discovery/specialization plus the ownership phase the raw boundary above
/// intentionally skips); rejection pins use this runner.
fn run_compiled(entry: &Path) -> Result<String, String> {
    let compiler = Compiler::default();
    let program = compiler
        .compile_path(entry)
        .map_err(|error| error.to_string())?;
    let execution = compiler
        .execute(&program)
        .map_err(|error| error.to_string())?;
    Ok(execution.output)
}

#[test]
fn self_hosted_generic_optional() {
    let d = TempDir::new();
    let main = d.write(
        "main.mojo",
        "from std.optional import Optional\n\ndef main():\n    var a: Optional[Int] = Optional[Int](42, True)\n    var b: Optional[Int] = Optional[Int](0, False)\n    print(a.is_some(), a.or_else(-1))\n    print(b.is_some(), b.or_else(-1))\n",
    );
    assert_eq!(run(&main).unwrap(), "True 42\nFalse -1\n");
}

#[test]
fn self_hosted_generic_list_grows_indexes_iterates() {
    let d = TempDir::new();
    let main = d.write(
        "main.mojo",
        "from std.collections.list import List\n\ndef main():\n    var xs: List[Int] = List[Int]()\n    var i: Int = 0\n    while i < 10:\n        xs.append(i * i)\n        i = i + 1\n    print(len(xs))\n    print(xs[0], xs[9])\n    xs[0] = 100\n    var total: Int = 0\n    for x in xs:\n        total = total + x\n    print(total)\n",
    );
    // 10 elements (grew past cap 4); 0²=0, 9²=81; sum = 100 + (1+4+…+81) = 385.
    assert_eq!(run(&main).unwrap(), "10\n0 81\n385\n");
}

#[test]
fn self_hosted_generic_list_has_value_semantics() {
    let d = TempDir::new();
    let main = d.write(
        "main.mojo",
        "from std.collections.list import List\n\ndef main():\n    var a: List[Int] = List[Int]()\n    a.append(1)\n    a.append(2)\n    var b: List[Int] = a\n    b.append(99)\n    b[0] = 555\n    print(len(a), len(b))\n    print(a[0], b[0])\n",
    );
    // `var b = a` deep-copies via __copyinit__ — b's mutations don't touch a.
    assert_eq!(run(&main).unwrap(), "2 3\n1 555\n");
}

#[test]
fn self_hosted_generic_set_deduplicates_contains_iterates() {
    let d = TempDir::new();
    let main = d.write(
        "main.mojo",
        "from std.collections.set import Set\n\ndef main():\n    var s: Set[Int] = Set[Int]()\n    s.add(3)\n    s.add(3)\n    s.add(5)\n    print(len(s))\n    print(3 in s, 4 in s)\n    var total: Int = 0\n    for x in s:\n        total = total + x\n    print(total)\n",
    );
    assert_eq!(run(&main).unwrap(), "2\nTrue False\n8\n");
}

#[test]
fn self_hosted_generic_dict_sets_gets_updates_iterates() {
    let d = TempDir::new();
    let main = d.write(
        "main.mojo",
        "from std.collections.dict import Dict\n\ndef main() raises:\n    var d: Dict[String, Int] = Dict[String, Int]()\n    d[\"a\"] = 10\n    d[\"b\"] = 20\n    d[\"a\"] = 15\n    print(len(d))\n    print(\"a\" in d, \"z\" in d)\n    print(d[\"a\"], d.get(\"z\", -1))\n    var total: Int = 0\n    for key in d:\n        total = total + d[key]\n    print(total)\n",
    );
    assert_eq!(run(&main).unwrap(), "2\nTrue False\n15 -1\n35\n");
}

#[test]
fn hashdict_mutation_during_iteration_is_rejected() {
    // HashDict iteration now retains the derived interior "element" loan, so
    // a during-loop setitem lazily invalidates the borrowed key iterator.
    let directory = TempDir::new();
    let main = directory.write(
        "main.mojo",
        "from std.collections.hashdict import HashDict\n\ndef main():\n    var mapping = HashDict[Int, String]()\n    mapping[1] = \"one\"\n    mapping[2] = \"two\"\n    for key in mapping:\n        mapping[3] = \"three\"\n        print(key)\n",
    );
    let error =
        run_compiled(&main).expect_err("hashdict mutation during iteration must be rejected");
    assert!(
        error.contains("invalidated interior reference") && error.contains("[\"element\"]"),
        "got {error}"
    );
}

#[test]
fn kwargs_mutation_during_iteration_is_rejected() {
    // The callee owns its **kwargs StringDict, but the borrowed key iterator
    // still loans its "element" generation: mutating during the loop is
    // rejected while mutation after the loop stays legal (pinned elsewhere).
    let directory = TempDir::new();
    let main = directory.write(
        "main.mojo",
        "def tally(**options: Int) -> Int:\n    var count = 0\n    for key in options:\n        options[\"extra\"] = 1\n        count += 1\n    return count\n\ndef main():\n    print(tally(alpha=1, beta=2))\n",
    );
    let error = run_compiled(&main).expect_err("kwargs mutation during iteration must be rejected");
    assert!(
        error.contains("invalidated interior reference") && error.contains("[\"element\"]"),
        "got {error}"
    );
}

#[test]
fn dict_iteration_reads_live_values_and_comprehensions_share_the_loan() {
    // The borrowing key iterator observes values as they are at read time
    // (updated before the loop), and the comprehension path shares the same
    // derived loan and sibling "value"-generation rules.
    let directory = TempDir::new();
    let main = directory.write(
        "main.mojo",
        "from std.collections.dict import Dict\nfrom std.collections.hashdict import HashDict\n\ndef main() raises:\n    var mapping = Dict[String, Int]()\n    mapping[\"a\"] = 1\n    mapping[\"b\"] = 2\n    mapping[\"a\"] = 10\n    var total = 0\n    for key in mapping:\n        total += mapping[key]\n    print(total)\n    var keys = [key for key in mapping]\n    print(len(keys))\n    var hashed = HashDict[Int, Int]()\n    hashed[1] = 5\n    hashed[2] = 6\n    var values = [hashed[key] for key in hashed]\n    print(values[0] + values[1])\n",
    );
    assert_eq!(run(&main).unwrap(), "12\n2\n11\n");
}

#[test]
fn borrowed_set_and_dict_iterators_retain_their_collection_owners() {
    // Neither loop body touches its source collection. The iterable expression
    // is therefore the collection's apparent last source use, but the checked
    // borrowed-origin loan must keep its pointer-owning storage alive until the
    // synthetic iterator is exhausted and destroyed.
    let directory = TempDir::new();
    let main = directory.write(
        "main.mojo",
        "from std.collections.set import Set\nfrom std.collections.dict import Dict\n\ndef main():\n    var values = Set[Int]()\n    values.add(3)\n    values.add(5)\n    var total = 0\n    for value in values:\n        total += value\n    print(total)\n    var mapping = Dict[String, Int]()\n    mapping[\"first\"] = 1\n    mapping[\"second\"] = 2\n    var keys = \"\"\n    for key in mapping:\n        keys += key\n    print(keys)\n",
    );
    assert_eq!(run(&main).unwrap(), "8\nfirstsecond\n");
}

#[test]
fn self_hosted_hash_backed_set() {
    // Phase 6: a hash-backed `HashSet[T]` (buckets chosen via `key.__hash__()`)
    // works for two key types — `Int` (intrinsic scalar hash) and `String`.
    let d = TempDir::new();
    let main = d.write(
        "main.mojo",
        "from std.collections.hashset import HashSet\n\ndef main():\n    var s: HashSet[Int] = HashSet[Int]()\n    s.add(3)\n    s.add(3)\n    s.add(11)\n    s.add(19)\n    print(len(s))\n    print(s.contains(11), s.contains(4))\n    var w: HashSet[String] = HashSet[String]()\n    w.add(\"mojo\")\n    w.add(\"lite\")\n    w.add(\"mojo\")\n    print(len(w))\n    print(w.contains(\"lite\"), w.contains(\"rust\"))\n",
    );
    assert_eq!(run(&main).unwrap(), "3\nTrue False\n2\nTrue False\n");
}

#[test]
fn incremental_hasher_accumulates_multiple_hash_parts() {
    let directory = TempDir::new();
    let main = directory.write(
        "main.mojo",
        "from std.hashing import IncrementalHasher\n\ndef main():\n    var first = IncrementalHasher.create()\n    first.update(UInt(3))\n    first.update(UInt(7))\n    var second = IncrementalHasher.create()\n    second.update(UInt(3))\n    second.update(UInt(8))\n    print(first.finish() == first.finish())\n    print(first.finish() == second.finish())\n",
    );
    assert_eq!(run(&main).unwrap(), "True\nFalse\n");
}

// --- Nested self-hosted lists (roadmap §2: the hash-set bucket-array shape) ---
//
// Characterization matrix for `List[List[T]]` where `List` is the self-hosted
// `std.collections.list` struct. Positive cases use the public copy/assignment
// surface; one regression sentinel records the deferred chained-ref contract.

#[test]
fn nested_list_builds_and_reads_via_explicit_rows() {
    let d = TempDir::new();
    let main = d.write(
        "main.mojo",
        "from std.collections.list import List\n\ndef main():\n    var m: List[List[Int]] = List[List[Int]]()\n    var r0: List[Int] = List[Int]()\n    r0.append(1)\n    r0.append(2)\n    var r1: List[Int] = List[Int]()\n    r1.append(3)\n    m.append(r0)\n    m.append(r1)\n    var first: List[Int] = m[0]\n    var second: List[Int] = m[1]\n    print(len(m))\n    print(first[0], first[1], second[0])\n    print(len(second))\n    var total: Int = 0\n    for x in first:\n        total = total + x\n    print(total)\n",
    );
    assert_eq!(run(&main).unwrap(), "2\n1 2 3\n1\n3\n");
}

#[test]
fn nested_list_chained_reference_receiver_uses_subscript_contracts() {
    let d = TempDir::new();
    let main = d.write(
        "main.mojo",
        "from std.collections.list import List\n\ndef main():\n    var rows = List[List[Int]]()\n    var row = List[Int]()\n    row.append(7)\n    rows.append(row)\n    rows[0].append(8)\n    rows[0][0] = 9\n    print(rows[0][0], rows[0][1])\n",
    );
    assert_eq!(run(&main).unwrap(), "9 8\n");
}

#[test]
fn nested_list_append_copies_row() {
    // Passing a row to `append` copies it (by-value argument): later mutation of
    // the original row must not reach the stored one.
    let d = TempDir::new();
    let main = d.write(
        "main.mojo",
        "from std.collections.list import List\n\ndef main():\n    var m: List[List[Int]] = List[List[Int]]()\n    var row: List[Int] = List[Int]()\n    row.append(1)\n    m.append(row)\n    row[0] = 42\n    row.append(7)\n    var stored: List[Int] = m[0]\n    print(stored[0], len(stored))\n",
    );
    assert_eq!(run(&main).unwrap(), "1 1\n");
}

#[test]
fn nested_list_copy_is_deep() {
    // `var n = m` must deep-copy the rows: mutating a row read out of the copy
    // (and stored back into the copy) must not reach the original.
    let d = TempDir::new();
    let main = d.write(
        "main.mojo",
        "from std.collections.list import List\n\ndef main():\n    var m: List[List[Int]] = List[List[Int]]()\n    var row: List[Int] = List[Int]()\n    row.append(1)\n    m.append(row)\n    var n: List[List[Int]] = m\n    var changed: List[Int] = n[0]\n    changed[0] = 99\n    n[0] = changed^\n    var original: List[Int] = m[0]\n    var updated: List[Int] = n[0]\n    print(original[0], updated[0])\n",
    );
    assert_eq!(run(&main).unwrap(), "1 99\n");
}

#[test]
fn nested_list_getitem_returns_a_copy() {
    // Binding the ref-returning `m[0]` as an owned `List[Int]` copies the row;
    // mutating that owned value does not change the original element.
    let d = TempDir::new();
    let main = d.write(
        "main.mojo",
        "from std.collections.list import List\n\ndef main():\n    var m: List[List[Int]] = List[List[Int]]()\n    var row: List[Int] = List[Int]()\n    row.append(1)\n    m.append(row)\n    var copied: List[Int] = m[0]\n    copied[0] = 77\n    var original: List[Int] = m[0]\n    print(original[0], copied[0])\n",
    );
    assert_eq!(run(&main).unwrap(), "1 77\n");
}

#[test]
fn nested_list_explicit_row_writeback() {
    // The explicit value-copy and assignment form remains useful alongside
    // direct chained reference mutation.
    let d = TempDir::new();
    let main = d.write(
        "main.mojo",
        "from std.collections.list import List\n\ndef main():\n    var m: List[List[Int]] = List[List[Int]]()\n    var row: List[Int] = List[Int]()\n    row.append(1)\n    m.append(row)\n    var updated: List[Int] = m[0]\n    updated.append(5)\n    m[0] = updated^\n    var stored: List[Int] = m[0]\n    print(len(stored), stored[1])\n",
    );
    assert_eq!(run(&main).unwrap(), "2 5\n");
}

#[test]
fn nested_list_as_struct_field_bucket_shape() {
    // The exact hash-set shape, using explicit public copy/mutate/write-back.
    let d = TempDir::new();
    let main = d.write(
        "main.mojo",
        "from std.collections.list import List\n\nstruct Grid:\n    var buckets: List[List[Int]]\n\n    def __init__(out self):\n        self.buckets = List[List[Int]]()\n        self.buckets.append(List[Int]())\n        self.buckets.append(List[Int]())\n\n    def add(mut self, i: Int, v: Int):\n        var bucket: List[Int] = self.buckets[i]\n        bucket.append(v)\n        self.buckets[i] = bucket^\n\n    def total(self, i: Int) -> Int:\n        var bucket: List[Int] = self.buckets[i]\n        var t: Int = 0\n        for x in bucket:\n            t = t + x\n        return t\n\ndef main():\n    var g: Grid = Grid()\n    g.add(0, 3)\n    g.add(1, 4)\n    g.add(0, 5)\n    print(g.total(0), g.total(1))\n",
    );
    assert_eq!(run(&main).unwrap(), "8 4\n");
}

#[test]
fn self_hosted_hashset_copy_and_list_shadowing() {
    let d = TempDir::new();
    let main = d.write(
        "main.mojo",
        "from std.collections.hashset import HashSet\nfrom std.collections.list import List\n\ndef main():\n    var s: HashSet[Int] = HashSet[Int]()\n    s.add(1)\n    var t: HashSet[Int] = s.copy()\n    t.add(9)\n    print(len(s), len(t), s.contains(9), t.contains(9))\n    var xs: List[Int] = List[Int]()\n    xs.append(7)\n    print(xs[0])\n",
    );
    assert_eq!(run(&main).unwrap(), "1 2 False True\n7\n");
}

#[test]
fn self_hosted_dict_views_get_and_snapshots() {
    let d = TempDir::new();
    let main = d.write(
        "main.mojo",
        "from std.collections.dict import Dict\n\ndef main() raises:\n    var d: Dict[String, Int] = Dict[String, Int]()\n    d[\"a\"] = 1\n    d[\"b\"] = 2\n    var keys = d.keys()\n    var values = d.values()\n    var items = d.items()\n    d[\"c\"] = 3\n    print(len(keys), len(values), len(items), len(d))\n    print(keys[0], keys[1], values[0], values[1])\n    print(items[0].key, items[0].value)\n    print(d.get(\"a\").is_some(), d.get(\"z\").is_some())\n    print(d.get(\"z\", 99))\n    for key in d:\n        print(key, d[key])\n",
    );
    assert_eq!(
        run(&main).unwrap(),
        "2 2 2 3\na b 1 2\na 1\nTrue False\n99\na 1\nb 2\nc 3\n"
    );
}

#[test]
fn hash_dict_matches_list_dict_and_preserves_order() {
    let d = TempDir::new();
    let main = d.write(
        "main.mojo",
        "from std.collections.dict import Dict\nfrom std.collections.hashdict import HashDict\n\ndef main() raises:\n    var a: Dict[Int, Int] = Dict[Int, Int]()\n    var b: HashDict[Int, Int] = HashDict[Int, Int]()\n    var i: Int = 0\n    while i < 20:\n        a[i] = i * 10\n        b[i] = i * 10\n        i = i + 1\n    a[3] = 333\n    b[3] = 333\n    print(len(a), len(b), b.bucket_count())\n    for key in a:\n        print(key, a[key])\n    print(\"---\")\n    for key in b:\n        print(key, b[key])\n    print(b.get(100).is_some(), b.get(100, -1))\n",
    );
    let output = run(&main).unwrap();
    let mut lines = output.lines();
    assert_eq!(lines.next(), Some("20 20 32"));
    let rest: Vec<&str> = lines.collect();
    let divider = rest.iter().position(|line| *line == "---").unwrap();
    assert_eq!(&rest[..divider], &rest[divider + 1..divider * 2 + 1]);
    assert_eq!(rest.last(), Some(&"False -1"));
}

#[test]
fn hash_dict_copy_has_value_semantics() {
    let d = TempDir::new();
    let main = d.write(
        "main.mojo",
        "from std.collections.hashdict import HashDict\n\ndef main() raises:\n    var a: HashDict[String, Int] = HashDict[String, Int]()\n    a[\"x\"] = 1\n    var b = a.copy()\n    b[\"x\"] = 9\n    b[\"y\"] = 2\n    print(len(a), a[\"x\"], \"y\" in a)\n    print(len(b), b[\"x\"], \"y\" in b)\n",
    );
    assert_eq!(run(&main).unwrap(), "1 1 False\n2 9 True\n");
}

#[test]
fn hash_dict_missing_subscript_raises() {
    let d = TempDir::new();
    let main = d.write(
        "main.mojo",
        "from std.collections.hashdict import HashDict\n\ndef main():\n    var d: HashDict[String, Int] = HashDict[String, Int]()\n    try:\n        print(d[\"missing\"])\n    except e:\n        print(e)\n",
    );
    assert_eq!(run(&main).unwrap(), "Error(\"missing key\")\n");
}

#[test]
fn kwargs_are_owned_self_hosted_string_dicts() {
    let d = TempDir::new();
    let main = d.write(
        "main.mojo",
        "def show(prefix: Int, **options: Int) raises:\n    print(prefix, len(options))\n    for key in options:\n        print(key, options[key])\n    options[\"local\"] = 9\n    print(options.get(\"missing\", -1), len(options))\n\ndef main() raises:\n    show(7, first=1, second=2)\n    show(8)\n",
    );
    assert_eq!(
        run(&main).unwrap(),
        "7 2\nfirst 1\nsecond 2\n-1 3\n8 0\n-1 1\n"
    );
}

#[test]
fn transferred_string_dict_forwards_keywords_in_order() {
    let d = TempDir::new();
    let main = d.write(
        "main.mojo",
        "def show(prefix: Int, **options: Int) raises:\n    print(prefix, len(options))\n    for key in options:\n        print(key, options[key])\n\ndef relay(**options: Int) raises:\n    show(prefix=7, **options^)\n\ndef main() raises:\n    relay(left=20, right=22)\n",
    );
    assert_eq!(run(&main).unwrap(), "7 2\nleft 20\nright 22\n");
}

#[test]
fn generic_and_method_kwargs_execute_through_string_dict() {
    let d = TempDir::new();
    let main = d.write(
        "main.mojo",
        "def generic_size[T: Copyable & Movable](**options: T) -> Int:\n    return len(options)\n\n@fieldwise_init\nstruct Counter:\n    var bias: Int\n    def size[T: Copyable & Movable](self, **options: T) -> Int:\n        return self.bias + len(options)\n    def relay(self, **options: Int) -> Int:\n        return self.size(**options^)\n    @staticmethod\n    def static_size[T: Copyable & Movable](**options: T) -> Int:\n        return len(options)\n\ndef main():\n    var counter = Counter(10)\n    print(generic_size(first=1, second=2))\n    print(counter.size(left=\"a\", right=\"b\"))\n    print(counter.relay(one=1, two=2, three=3))\n    print(Counter.static_size(a=1, b=2, c=3, d=4))\n",
    );
    assert_eq!(run(&main).unwrap(), "2\n12\n13\n4\n");
}

#[test]
fn bounded_trait_method_kwargs_execute_through_the_selected_method() {
    let d = TempDir::new();
    let main = d.write(
        "main.mojo",
        "trait Counts:\n    def count[Element: Copyable & Movable](self, **options: Element) -> Int: ...\n\n@fieldwise_init\nstruct Counter(Counts):\n    var bias: Int\n    def count[Element: Copyable & Movable](self, **options: Element) -> Int:\n        return self.bias + len(options)\n\ndef count_through_bound[Target: Counts](target: Target, **options: Int) -> Int:\n    return target.count(**options^)\n\ndef main():\n    var counter = Counter(10)\n    print(count_through_bound(counter, left=1, right=2))\n",
    );
    assert_eq!(run(&main).unwrap(), "12\n");
}

#[test]
fn bounded_keyword_overloads_keep_the_checker_selected_signature() {
    let d = TempDir::new();
    let main = d.write(
        "main.mojo",
        "trait Picks:\n    def pick(self, **options: Int) -> Int: ...\n    def pick(self, **options: String) -> Int: ...\n\n@fieldwise_init\nstruct Picker(Picks):\n    var marker: Int\n    def pick(self, **options: Int) -> Int:\n        return 1\n    def pick(self, **options: String) -> Int:\n        return 2\n\ndef through_bound[Target: Picks](target: Target) -> Int:\n    return target.pick(value=\"selected\")\n\ndef main():\n    var picker = Picker(0)\n    print(through_bound(picker))\n",
    );
    assert_eq!(run(&main).unwrap(), "2\n");
}

#[test]
fn keyword_collectors_follow_named_out_results_in_the_runtime_frame() {
    let d = TempDir::new();
    let main = d.write(
        "main.mojo",
        "def count(out result: Int, **options: Int):\n    result = len(options)\n\ndef main():\n    print(count(first=1, second=2))\n",
    );
    assert_eq!(run(&main).unwrap(), "2\n");
}

#[test]
fn keyword_overflow_records_implicit_conversions_for_every_call_kind() {
    let d = TempDir::new();
    let main = d.write(
        "main.mojo",
        "struct Box(Copyable):\n    var value: Int\n    @implicit\n    def __init__(out self, value: Int):\n        self.value = value\n\ndef free(**options: Box) raises -> Int:\n    return options[\"item\"].value\n\n@fieldwise_init\nstruct Collector:\n    var bias: Int\n    def method(self, **options: Box) raises -> Int:\n        return self.bias + options[\"item\"].value\n    @staticmethod\n    def static(**options: Box) raises -> Int:\n        return options[\"item\"].value\n\ntrait Collects:\n    def bounded(self, **options: Box) raises -> Int: ...\n\n@fieldwise_init\nstruct BoundedCollector(Collects):\n    var bias: Int\n    def bounded(self, **options: Box) raises -> Int:\n        return self.bias + options[\"item\"].value\n\ndef through_bound[Target: Collects](target: Target) raises -> Int:\n    return target.bounded(item=4)\n\ndef main() raises:\n    var collector = Collector(10)\n    var bounded = BoundedCollector(20)\n    print(free(item=1))\n    print(collector.method(item=2))\n    print(Collector.static(item=3))\n    print(through_bound(bounded))\n",
    );
    assert_eq!(run(&main).unwrap(), "1\n12\n3\n24\n");
}

#[test]
fn method_overloads_distinguish_fixed_and_keyword_collector_shapes() {
    let d = TempDir::new();
    let main = d.write(
        "main.mojo",
        "@fieldwise_init\nstruct Selector:\n    var marker: Int\n    def choose(self, value: Int) -> Int:\n        return 1\n    def choose(self, value: Int, **options: Int) -> Int:\n        return 2 + len(options)\n\ndef main():\n    var selector = Selector(0)\n    print(selector.choose(7))\n    print(selector.choose(7, extra=9))\n",
    );
    assert_eq!(run(&main).unwrap(), "1\n3\n");
}

#[test]
fn forwarded_method_kwargs_preserve_duplicate_detection() {
    let d = TempDir::new();
    let main = d.write(
        "main.mojo",
        "@fieldwise_init\nstruct Relay:\n    var marker: Int\n    def target(self, **options: Int):\n        pass\n    def forward(self, **options: Int):\n        self.target(first=0, **options^)\n\ndef main():\n    var relay = Relay(0)\n    relay.forward(first=1)\n",
    );
    let error = run(&main).expect_err("forwarded duplicate must fail at runtime binding");
    assert!(
        error.contains("more than once"),
        "unexpected error: {error}"
    );
}

#[test]
fn self_hosted_math_rounding_helpers() {
    // Phase 7: the self-hosted `math` module (not prelude — must be imported)
    // exposes `floor`/`ceil`/`trunc`/`ceildiv`, generic over their trait bounds;
    // built-in `Int`/`Float64` supply the dunders intrinsically after erasure.
    let d = TempDir::new();
    let main = d.write(
        "main.mojo",
        "from std.math import floor, ceil, trunc, ceildiv\n\ndef main():\n    print(floor(3.7), ceil(3.2), trunc(-3.7))\n    print(floor(5), ceil(5))\n    print(ceildiv(7, 2), ceildiv(-7, 2))\n    print(ceildiv(7.0, 2.0))\n",
    );
    assert_eq!(run(&main).unwrap(), "3.0 4.0 -3.0\n5 5\n4 -3\n4.0\n");
}

#[test]
fn self_hosted_algorithms_use_comptime_facts() {
    let main = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join("ok")
        .join("self_hosted_algorithms.mojo");
    assert_eq!(
        run(&main).unwrap(),
        "1 2 0\n8 24\n4 17\n42\nfallback\n7\nalpha\n11\nbeta\n3\n"
    );
}

#[test]
fn linked_vm_ctfe_keeps_nominal_helpers_without_unrelated_templates() {
    let d = TempDir::new();
    d.write(
        "library.mojo",
        "def _increment(value: Int) -> Int:\n    return value + 1\n\ndef _trait_default() -> Int:\n    return 7\n\n@fieldwise_init\nstruct Box:\n    var value: Int\n    def incremented(self) -> Int:\n        return _increment(self.value)\n\ntrait HasDefault:\n    def default_value(self) -> Int:\n        return _trait_default()\n\ndef compile_answer() -> Int:\n    return 6 * 7\n\ndef uninstantiated[T: AnyType]() -> Int:\n    comptime if is_same_type[T, Int]():\n        return 1\n    else:\n        return \"this template must not cross the CTFE boundary\"\n",
    );
    let main = d.write(
        "main.mojo",
        "from library import Box, compile_answer\n\ncomptime ANSWER = compile_answer()\n\ndef main():\n    print(ANSWER)\n    print(Box(8).incremented())\n",
    );

    // VM CTFE checks retained nominal method bodies. Their linked, module-qualified
    // free helpers therefore belong to the declaration closure, while the unused
    // generic `comptime if` template must stay outside that checked subprogram.
    assert_eq!(run(&main).unwrap(), "42\n9\n");
}

#[test]
fn self_hosted_pack_tuple_constructs_and_indexes_with_exact_types() {
    // The variadic-generic prototype for a future self-hosted Tuple: pack
    // construction, exact per-index element typing at compile-time-constant
    // indices, length, and value-semantic copy/move.
    let d = TempDir::new();
    let main = d.write(
        "main.mojo",
        "from std.collections.pack_tuple import PackTuple\n\ndef main():\n    var t = PackTuple[Int, String, Bool](3, \"mid\", True)\n    var n: Int = t[0]\n    print(n + 4)\n    print(t[1])\n    print(t[2])\n    print(len(t))\n    var copy = t\n    print(copy[0])\n    var moved = t^\n    print(moved[2])\n",
    );
    assert_eq!(run(&main).unwrap(), "7\nmid\nTrue\n3\n3\nTrue\n");
}

#[test]
fn self_hosted_pack_tuple_preserves_tuple_restrictions() {
    // Native-tuple parity: immutable (no `__setitem__`), non-iterable (no
    // `__iter__`), and a compile-time-constant index is required.
    let d = TempDir::new();
    let imports = "from std.collections.pack_tuple import PackTuple\n\n";
    let write = d.write(
        "write.mojo",
        &format!(
            "{imports}def main():\n    var t = PackTuple[Int, Bool](1, True)\n    t[0] = 9\n    print(t[0])\n"
        ),
    );
    assert!(run(&write).is_err(), "element writes must be rejected");
    let iterate = d.write(
        "iterate.mojo",
        &format!(
            "{imports}def main():\n    var t = PackTuple[Int, Bool](1, True)\n    for x in t:\n        print(x)\n"
        ),
    );
    assert!(run(&iterate).is_err(), "iteration must be rejected");
    let runtime_index = d.write(
        "runtime_index.mojo",
        &format!(
            "{imports}def main():\n    var t = PackTuple[Int, Bool](1, True)\n    var i = 0\n    print(t[i])\n"
        ),
    );
    let err = run(&runtime_index).unwrap_err();
    assert!(err.contains("compile-time Int index"), "got: {err}");
}

#[test]
fn self_hosted_string_codepoint_type() {
    // `s[codepoint=i]` yields a prelude-exported `Codepoint`: Writable as the
    // character it decoded, `Intable` for the scalar, scalar-ordered.
    let d = TempDir::new();
    let main = d.write(
        "main.mojo",
        "def main():\n    var s = String(\"h\\u00e9llo\\U0001f642\")\n    try:\n        var first: Codepoint = s[codepoint=0]\n        var accent = s[codepoint=1]\n        var face = s[codepoint=5]\n        print(first, accent, face)\n        print(Int(first), Int(accent), Int(face))\n        print(first.is_ascii(), accent.is_ascii())\n        print(first.utf8_byte_length(), accent.utf8_byte_length(), face.utf8_byte_length())\n        print(first < accent, accent == accent, face > accent)\n    except:\n        print(\"unexpected\")\n",
    );
    assert_eq!(
        run_compiled(&main).unwrap(),
        "h é 🙂\n104 233 128578\nTrue False\n1 2 4\nTrue True True\n"
    );
}

#[test]
fn self_hosted_string_grapheme_segmentation() {
    // The documented UAX #29 subset: combining marks join (GB9), decomposed
    // Hangul jamo compose (GB6-GB8), regional indicators pair (GB12/GB13),
    // ZWJ sequences and skin tones join (simplified GB11, GB9), CR LF stays
    // one cluster (GB3), and controls break (GB4/GB5).
    let d = TempDir::new();
    let main = d.write(
        "main.mojo",
        "def main() raises:\n    var accent = String(\"e\\u0301\")\n    print(accent.codepoint_count(), accent.grapheme_count(), accent[grapheme=0])\n    var jamo = String(\"\\u1112\\u1161\\u11ab\")\n    print(jamo.codepoint_count(), jamo.grapheme_count(), jamo[grapheme=0])\n    var flags = String(\"\\U0001f1fa\\U0001f1f8\\U0001f1eb\\U0001f1f7\")\n    print(flags.grapheme_count(), flags[grapheme=1])\n    var family = String(\"\\U0001f468\\u200d\\U0001f469\\u200d\\U0001f467\")\n    print(family.grapheme_count(), family[grapheme=0])\n    var thumb = String(\"\\U0001f44d\\U0001f3fd\")\n    print(thumb.grapheme_count())\n    var crlf = String(\"a\\r\\nb\")\n    print(crlf.grapheme_count())\n    print(String(\"\").grapheme_count())\n",
    );
    assert_eq!(
        run_compiled(&main).unwrap(),
        "2 1 e\u{301}\n3 1 \u{1112}\u{1161}\u{11ab}\n2 🇫🇷\n1 👨\u{200d}👩\u{200d}👧\n1\n3\n0\n"
    );
}
