//! Differential and emission tests for the experimental Pliron backend
//! (feature `backend-pliron`; requires LLVM 22 — see scripts/check-pliron).

#![cfg(feature = "backend-pliron")]

use std::path::Path;

use expect_test::expect;
use mojito::Compiler;
use mojito::backend::pliron as native;
use native::{CompileOptions, JitValue, NativeModule, OptLevel, TrapCategory};

const FIXTURE_NAME: &str = "pliron_fixture.mojo";

/// Compile `src` through the production pipeline and hand its cached
/// post-drop MIR to the Pliron backend with the given entries.
fn native_compile(src: &str, entries: &[&str]) -> NativeModule {
    let compiler = Compiler::default();
    let compiled = compiler
        .compile_source(src, Path::new(FIXTURE_NAME))
        .unwrap_or_else(|error| panic!("fixture must compile: {error}"));
    let options = CompileOptions {
        entries: entries.iter().map(|s| s.to_string()).collect(),
        sources: vec![(FIXTURE_NAME.to_string(), src.to_string())],
    };
    native::compile(compiled.elaborated_mir(), &options)
        .unwrap_or_else(|error| panic!("{}", error.display_with_sources(&options.sources)))
}

const FIB: &str = "\
def fib(n: Int) -> Int:
    if n < 2:
        return n
    return fib(n - 1) + fib(n - 2)

def compute() -> Int:
    return fib(10)

def main():
    print(compute())
";

#[test]
fn fib_lowers_verifies_and_prints_canonically() {
    let module = native_compile(FIB, &["compute"]);
    assert_eq!(module.mangled_name("fib"), Some("mj_fib"));
    expect![[r#"
        builtin.module @mojito 
        {
          ^block1v1():
            llvm.func @mj_fib: llvm.func <builtin.integer i64(builtin.integer i64) variadic = false>
              [] 
            {
              ^block2v1(v0: builtin.integer i64):
                v1 = llvm.constant <builtin.integer <1: i64>> : builtin.integer i64;
                llvm.br ^block3v1()

              ^block3v1():
                v4 = llvm.constant <builtin.integer <2: i64>> : builtin.integer i64;
                v5 = llvm.icmp v0 <SLT> v4 : builtin.integer i1 !0;
                llvm.cond_br if v5 ^block5v1() else ^block6v1() !1

              ^block4v1():
                v7 = llvm.constant <builtin.integer <1: i64>> : builtin.integer i64;
                v8 = llvm.sub v0, v7 <{nsw=false,nuw=false}>: builtin.integer i64 !2;
                v9 = llvm.call @mj_fib (v8) : llvm.func <builtin.integer i64(builtin.integer i64) variadic = false> !3;
                v11 = llvm.constant <builtin.integer <2: i64>> : builtin.integer i64;
                v12 = llvm.sub v0, v11 <{nsw=false,nuw=false}>: builtin.integer i64 !4;
                v13 = llvm.call @mj_fib (v12) : llvm.func <builtin.integer i64(builtin.integer i64) variadic = false> !5;
                v14 = llvm.add v9, v13 <{nsw=false,nuw=false}>: builtin.integer i64 !6;
                llvm.return v14 !7

              ^block5v1():
                llvm.return v0 !8

              ^block6v1():
                llvm.br ^block4v1()
            };
            llvm.func @mj_compute: llvm.func <builtin.integer i64() variadic = false>
              [] 
            {
              ^block7v1():
                v16 = llvm.constant <builtin.integer <1: i64>> : builtin.integer i64;
                llvm.br ^block8v1()

              ^block8v1():
                v17 = llvm.constant <builtin.integer <10: i64>> : builtin.integer i64;
                v18 = llvm.call @mj_fib (v17) : llvm.func <builtin.integer i64(builtin.integer i64) variadic = false> !9;
                llvm.return v18 !10
            }
        }

        outlined_attributes:
        !0 = @["pliron_fixture.mojo": line: 2, column: 8], []
        !1 = @["pliron_fixture.mojo": line: 2, column: 8], []
        !2 = @["pliron_fixture.mojo": line: 4, column: 16], []
        !3 = @["pliron_fixture.mojo": line: 4, column: 12], []
        !4 = @["pliron_fixture.mojo": line: 4, column: 29], []
        !5 = @["pliron_fixture.mojo": line: 4, column: 25], []
        !6 = @["pliron_fixture.mojo": line: 4, column: 12], []
        !7 = @["pliron_fixture.mojo": line: 4, column: 12], []
        !8 = @["pliron_fixture.mojo": line: 3, column: 16], []
        !9 = @["pliron_fixture.mojo": line: 7, column: 12], []
        !10 = @["pliron_fixture.mojo": line: 7, column: 12], []
    "#]].assert_eq(module.plir_text());
}

#[test]
fn compilation_is_deterministic() {
    let first = native_compile(FIB, &["compute"]);
    let second = native_compile(FIB, &["compute"]);
    assert_eq!(first.plir_text(), second.plir_text());
    assert_eq!(
        first.llvm_ir(OptLevel::O0).expect("LLVM conversion"),
        second.llvm_ir(OptLevel::O0).expect("LLVM conversion"),
        "repeated builds must produce byte-identical LLVM IR"
    );
}

/// Canonical text parses back and reprints byte-identically.
#[test]
fn canonical_text_round_trips() {
    use pliron::irfmt::parsers::spaced;
    use pliron::operation::Operation;
    use pliron::parsable::parse_from_str;
    use pliron::printable::Printable;
    use pliron::result::ExpectOk;

    // The first parse attaches `<in-memory>` locations to ops that carried
    // none, so byte stability is asserted from the first reparse onward
    // (the same policy the Stage 0 spike pinned; see docs/notes).
    let module = native_compile(FIB, &["compute"]);

    let ctx1 = &mut pliron::context::Context::new();
    let round1 = parse_from_str(
        spaced(Operation::top_level_parser()),
        ctx1,
        module.plir_text(),
    )
    .expect_ok(ctx1);
    pliron::operation::verify_operation(round1, ctx1).expect_ok(ctx1);
    pliron::debug_info::erase_given_names(ctx1, round1);
    let round1_text = round1.disp(ctx1).to_string();

    let ctx2 = &mut pliron::context::Context::new();
    let round2 =
        parse_from_str(spaced(Operation::top_level_parser()), ctx2, &round1_text).expect_ok(ctx2);
    pliron::operation::verify_operation(round2, ctx2).expect_ok(ctx2);
    pliron::debug_info::erase_given_names(ctx2, round2);
    let round2_text = round2.disp(ctx2).to_string();

    assert_eq!(
        round1_text, round2_text,
        "canonical text must be a parse -> print fixpoint from the first reparse"
    );
}

/// The `(relative path, source)` of every `.mojo` fixture in `dir`, sorted.
fn fixture_sources(dir: &str) -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join(dir);
    let mut fixtures: Vec<_> = std::fs::read_dir(&root)
        .unwrap_or_else(|error| panic!("{dir} exists: {error}"))
        .filter_map(|entry| {
            let path = entry.expect("readable dir entry").path();
            let name = path.file_name()?.to_str()?;
            name.ends_with(".mojo").then(|| name.to_string())
        })
        .collect();
    fixtures.sort();
    fixtures
        .into_iter()
        .map(|name| {
            let source = std::fs::read_to_string(root.join(&name)).expect("fixture source reads");
            (format!("{dir}/{name}"), source)
        })
        .collect()
}

/// The value-differential eligibility shape: a zero-argument, value-returning
/// `compute` entry (the printed value of `main` doubles as the VM oracle).
fn has_compute_entry(src: &str) -> bool {
    src.lines().any(|line| line.starts_with("def compute() ->"))
}

/// Run `work` over `items` in worker threads, preserving item order in the
/// results. A worker panic propagates when the scope joins, failing the
/// test. (The per-fixture production compile dominates the manifest pass;
/// fixtures are independent, and the JIT's global target initialization is
/// Once-guarded.)
fn parallel_map<T: Send, R: Send>(items: Vec<T>, work: impl Fn(T) -> R + Sync) -> Vec<R> {
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(items.len().max(1));
    let queue = std::sync::Mutex::new(items.into_iter().enumerate().rev().collect::<Vec<_>>());
    let results = std::sync::Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let Some((index, item)) = queue.lock().expect("queue lock").pop() else {
                        break;
                    };
                    let result = work(item);
                    results.lock().expect("results lock").push((index, result));
                }
            });
        }
    });
    let mut results = results.into_inner().expect("results lock");
    results.sort_by_key(|(index, _)| *index);
    results.into_iter().map(|(_, result)| result).collect()
}

/// The manifest spelling of a JIT value's kind.
fn ret_kind_name(value: JitValue) -> &'static str {
    match value {
        JitValue::Int(_) => "Int",
        JitValue::UInt(_) => "UInt",
        JitValue::Float64(_) => "Float64",
        JitValue::Bool(_) => "Bool",
    }
}

/// Assert a JIT result equals the VM's printed value, parsed at the JIT
/// result's kind. Floats compare by bits with NaN-class equality (the VM
/// prints shortest-round-trip text, so the parse-back is exact).
fn assert_jit_matches(fixture: &str, level: &str, native: JitValue, printed: &str) {
    match native {
        JitValue::Int(actual) => {
            let expected: i64 = printed
                .parse()
                .unwrap_or_else(|e| panic!("{fixture}: VM must print an Int: {e}: {printed:?}"));
            assert_eq!(
                actual, expected,
                "{fixture}: native Int diverges at {level}"
            );
        }
        JitValue::UInt(actual) => {
            let expected: u64 = printed
                .parse()
                .unwrap_or_else(|e| panic!("{fixture}: VM must print a UInt: {e}: {printed:?}"));
            assert_eq!(
                actual, expected,
                "{fixture}: native UInt diverges at {level}"
            );
        }
        JitValue::Float64(actual) => {
            let expected: f64 = printed
                .parse()
                .unwrap_or_else(|e| panic!("{fixture}: VM must print a Float64: {e}: {printed:?}"));
            let matches =
                (actual.is_nan() && expected.is_nan()) || actual.to_bits() == expected.to_bits();
            assert!(
                matches,
                "{fixture}: native Float64 {actual:?} diverges from VM {expected:?} at {level}"
            );
        }
        JitValue::Bool(actual) => {
            let expected = match printed {
                "True" => true,
                "False" => false,
                other => panic!("{fixture}: VM must print a Bool: {other:?}"),
            };
            assert_eq!(
                actual, expected,
                "{fixture}: native Bool diverges at {level}"
            );
        }
    }
}

/// The Stage 2 acceptance gate in one pass (one production compile per
/// eligible fixture — the compile dominates the cost, so manifest generation,
/// the O0/O1 value differential, and the trap-category differential share it):
///
/// - Every `assets/ok` fixture with the compute-entry shape either compiles
///   natively and must match the VM at `O0` and `O1`, or is recorded as
///   `excluded` with its first rejection diagnostic.
/// - Every `assets/runtime_error/pliron_trap_*` fixture must trap in the VM
///   with a recognized [`TrapCategory`] message and exit a native executable
///   with that category's exit code (`64 + code`) at both levels, printing
///   nothing. (Traps run only as subprocesses — an in-process JIT trap would
///   `exit` the test runner.)
/// - Everything else is recorded `ineligible`, so
///   `conformance/pliron-scalar.tsv` names every fixture exactly once.
///
/// The checked-in manifest must match regeneration byte-exactly
/// (`UPDATE_EXPECT=1` with `CARGO_WORKSPACE_DIR=$PWD` refreshes it), and the
/// trailing guards fail if eligible coverage unexpectedly shrinks.
#[test]
fn scalar_capability_manifest_and_differential() {
    let ok_rows = parallel_map(fixture_sources("assets/ok"), |(rel, src)| {
        if !has_compute_entry(&src) {
            return (
                rel,
                "-".into(),
                "ineligible".to_string(),
                "no-scalar-entry-shape".into(),
            );
        }
        let compiler = Compiler::default();
        let compiled = compiler
            .compile_source(&src, Path::new(&rel))
            .unwrap_or_else(|error| panic!("{rel}: ok fixture must compile: {error}"));
        let options = CompileOptions {
            entries: vec!["compute".to_string()],
            sources: vec![(rel.clone(), src.clone())],
        };
        match native::compile(compiled.elaborated_mir(), &options) {
            Err(error) => {
                let detail = error.display_with_sources(&options.sources);
                (rel, "compute".into(), "excluded".to_string(), detail)
            }
            Ok(module) => {
                let execution = compiler
                    .execute(&compiled)
                    .unwrap_or_else(|error| panic!("{rel}: fixture must run on the VM: {error}"));
                let printed = execution.output.trim().to_string();
                let at_o0 = module
                    .jit_value("compute", OptLevel::O0)
                    .unwrap_or_else(|error| panic!("{rel}: JIT at O0 failed: {error}"));
                let at_o1 = module
                    .jit_value("compute", OptLevel::O1)
                    .unwrap_or_else(|error| panic!("{rel}: JIT at O1 failed: {error}"));
                assert_jit_matches(&rel, "O0", at_o0, &printed);
                assert_jit_matches(&rel, "O1", at_o1, &printed);
                let detail = format!("ret={}", ret_kind_name(at_o0));
                (rel, "compute".into(), "differential".to_string(), detail)
            }
        }
    });

    let trap_rows = parallel_map(fixture_sources("assets/runtime_error"), |(rel, src)| {
        let is_trap_fixture = rel
            .rsplit('/')
            .next()
            .is_some_and(|name| name.starts_with("pliron_trap_"));
        if !is_trap_fixture {
            return (
                rel,
                "-".into(),
                "ineligible".to_string(),
                "no-scalar-entry-shape".into(),
            );
        }
        let compiler = Compiler::default();
        let compiled = compiler
            .compile_source(&src, Path::new(&rel))
            .unwrap_or_else(|error| panic!("{rel}: trap fixture must compile: {error}"));
        let vm_error = compiler
            .execute(&compiled)
            .expect_err("trap fixture must fail on the VM")
            .to_string();
        let category = TrapCategory::from_vm_message(&vm_error).unwrap_or_else(|| {
            panic!("{rel}: VM error carries no recognized trap category: {vm_error}")
        });
        let options = CompileOptions {
            entries: vec!["main".to_string()],
            sources: vec![(rel.clone(), src.clone())],
        };
        let mut module = native::compile(compiled.elaborated_mir(), &options)
            .unwrap_or_else(|error| panic!("{}", error.display_with_sources(&options.sources)));
        let dir = tempfile::tempdir().expect("tempdir");
        for (level, opt) in [("O0", OptLevel::O0), ("O1", OptLevel::O1)] {
            let exe = dir.path().join(format!("trap-{level}"));
            module
                .write_executable(&exe, opt)
                .unwrap_or_else(|error| panic!("{rel}: exe emission at {level}: {error}"));
            let run = std::process::Command::new(&exe)
                .output()
                .expect("trap executable runs");
            assert_eq!(
                run.status.code(),
                Some(i32::from(category.exit_code())),
                "{rel}: native trap exit status diverges at {level} (VM: {vm_error})"
            );
            assert!(
                run.stdout.is_empty(),
                "{rel}: trapping executable must print nothing at {level}"
            );
        }
        let detail = format!("category={category:?}");
        (rel, "main".into(), "trap-differential".to_string(), detail)
    });
    let rows: Vec<(String, String, String, String)> =
        ok_rows.into_iter().chain(trap_rows).collect();

    let mut manifest = String::from(
        "# Pliron scalar capability manifest (generated; schema-version 1).\n\
         # One row per assets/ok and assets/runtime_error fixture:\n\
         #   fixture <TAB> entry <TAB> status <TAB> detail\n\
         # status: differential (VM/native value oracle, O0+O1) |\n\
         #         trap-differential (VM/native trap-category oracle, O0+O1) |\n\
         #         excluded (native rejection diagnostic) |\n\
         #         ineligible (no zero-arg value-returning `compute` entry)\n\
         # Regenerate: UPDATE_EXPECT=1 CARGO_WORKSPACE_DIR=$PWD \\\n\
         #   cargo nextest run --features backend-pliron scalar_capability_manifest\n",
    );
    for (fixture, entry, status, detail) in &rows {
        manifest.push_str(&format!("{fixture}\t{entry}\t{status}\t{detail}\n"));
    }
    expect_test::expect_file!["../conformance/pliron-scalar.tsv"].assert_eq(&manifest);

    // Coverage guards: the eligible sets must never silently shrink, and a
    // pliron-named fixture must never regress from differential to excluded.
    let count = |status: &str| rows.iter().filter(|(_, _, s, _)| s == status).count();
    let differential = count("differential");
    let traps = count("trap-differential");
    assert!(
        differential >= 20,
        "differential coverage unexpectedly shrank: {differential} < 20"
    );
    assert!(
        traps >= 4,
        "trap-differential coverage unexpectedly shrank: {traps} < 4"
    );
    for (fixture, _, status, detail) in &rows {
        let name = fixture.rsplit('/').next().unwrap_or(fixture);
        assert!(
            !(name.starts_with("pliron_") && status == "excluded"),
            "{fixture}: pliron fixture regressed to excluded: {detail}"
        );
    }
}

/// A pure-scalar `main`: the emitted executable runs, exits 0, and prints
/// nothing — matching the VM run of the same program.
const EXE_MAIN: &str = "\
def fib(n: Int) -> Int:
    if n < 2:
        return n
    return fib(n - 1) + fib(n - 2)

def main():
    var r = fib(12)
";

#[test]
fn executable_and_object_emission() {
    let compiler = Compiler::default();
    let compiled = compiler
        .compile_source(EXE_MAIN, Path::new(FIXTURE_NAME))
        .expect("fixture compiles");
    let execution = compiler.execute(&compiled).expect("VM run succeeds");
    assert_eq!(execution.output, "", "VM run must print nothing");

    let options = CompileOptions {
        entries: vec!["main".to_string()],
        sources: vec![(FIXTURE_NAME.to_string(), EXE_MAIN.to_string())],
    };
    let mut module = native::compile(compiled.elaborated_mir(), &options)
        .unwrap_or_else(|error| panic!("{}", error.display_with_sources(&options.sources)));

    let dir = tempfile::tempdir().expect("tempdir");
    let object_path = dir.path().join("main.o");
    module
        .write_object(&object_path, OptLevel::O0)
        .expect("object emission");
    let object_bytes = std::fs::read(&object_path).expect("object file exists");
    assert_eq!(&object_bytes[..4], b"\x7fELF", "object must be an ELF file");

    let exe_path = dir.path().join("main");
    module
        .write_executable(&exe_path, OptLevel::O0)
        .expect("executable emission");
    let run = std::process::Command::new(&exe_path)
        .output()
        .expect("executable runs");
    assert_eq!(run.status.code(), Some(0), "executable must exit 0");
    assert!(run.stdout.is_empty(), "executable must print nothing");
}

/// Compile a fixture expecting a backend diagnostic; return its rendering.
fn native_error(src: &str, entries: &[&str]) -> String {
    let compiler = Compiler::default();
    let compiled = compiler
        .compile_source(src, Path::new(FIXTURE_NAME))
        .unwrap_or_else(|error| panic!("fixture must reach the backend: {error}"));
    let options = CompileOptions {
        entries: entries.iter().map(|s| s.to_string()).collect(),
        sources: vec![(FIXTURE_NAME.to_string(), src.to_string())],
    };
    let error = native::compile(compiled.elaborated_mir(), &options)
        .err()
        .expect("the backend must reject this fixture");
    error.display_with_sources(&options.sources)
}

/// Every construct outside the scalar subset produces a contextual
/// diagnostic naming the function and construct — no panics, no fallbacks.
#[test]
fn unsupported_constructs_produce_contextual_diagnostics() {
    // (source, entries, phrases the diagnostic must contain)
    let cases: &[(&str, &[&str], &[&str])] = &[
        (
            "def compute() -> Int:\n    var s = \"hi\"\n    return 1\n",
            &["compute"],
            &["in `compute`", "unsupported"],
        ),
        (
            "def compute() -> Int:\n    print(1)\n    return 1\n",
            &["compute"],
            &[
                "in `compute`",
                "call to unknown or builtin function `print`",
                "pliron_fixture.mojo:2:",
            ],
        ),
        // Production `range(...)` iterates the self-hosted stdlib range
        // structs (methods, raising `__next__`) — a manifest-recorded
        // exclusion until the struct/method stage; the rejection surfaces in
        // the deepest unsupported stdlib callee.
        (
            "def compute() -> Int:\n    var s = 0\n    for i in range(4):\n        s = s + i\n    return s\n",
            &["compute"],
            &["pliron backend:", "unsupported"],
        ),
        // Width-1 SIMD scalar aliases (Int32, UInt8, ...) stay excluded.
        (
            "def compute() -> Int:\n    var x: Int32 = 5\n    return 1\n",
            &["compute"],
            &["pliron backend:", "unsupported"],
        ),
        (
            "def helper() raises -> Int:\n    raise Error(\"boom\")\n\ndef compute() -> Int:\n    try:\n        return helper()\n    except:\n        return 0\n",
            &["compute"],
            &["in `compute`", "unsupported"],
        ),
        // Variadic calls stay outside the bound-call contract.
        (
            "def total(*xs: Int) -> Int:\n    var s = 0\n    for x in xs:\n        s = s + x\n    return s\n\ndef compute() -> Int:\n    return total(1, 2, 3)\n",
            &["compute"],
            &["pliron backend:", "unsupported"],
        ),
        // Collections drag stdlib helpers into the reachable closure; the
        // rejection may surface in the deepest unsupported callee.
        (
            "def compute() -> Int:\n    var xs = List[Int]()\n    return 1\n",
            &["compute"],
            &["pliron backend:", "unsupported"],
        ),
    ];
    for (src, entries, phrases) in cases {
        let message = native_error(src, entries);
        for phrase in *phrases {
            assert!(
                message.contains(phrase),
                "diagnostic missing `{phrase}`:\n{message}\nfor source:\n{src}"
            );
        }
    }
}

/// Missing entries are rejected by name.
#[test]
fn unknown_entry_is_rejected() {
    let message = native_error(FIB, &["nonexistent"]);
    assert!(
        message.contains("entry function `nonexistent`"),
        "{message}"
    );
}

/// One compact function per Stage 2 surface: float arithmetic and compares,
/// UInt operators, conversions, the div-by-zero and pow-exponent trap blocks,
/// and the shared `mjrt_pow`/`exit` scaffolding all print canonically.
const STAGE2_SURFACE: &str = "\
def mix(a: Int, b: UInt, x: Float64) -> Float64:
    var q = a // 3
    var p = a ** 2
    var u = b // UInt(3) + (b >> UInt(1))
    var f = x * 2.5 + Float64(a) + Float64(u)
    if f != f or x < 0.5:
        f = f ** 2.0
    return f + Float64(Int(x)) + Float64(Bool(a))

def compute() -> Float64:
    return mix(7, UInt(9), 1.25)
";

#[test]
fn stage2_surface_prints_canonically() {
    let module = native_compile(STAGE2_SURFACE, &["compute"]);
    let text = module.plir_text();
    // The full snapshot lives in the FIB test; here the load-bearing Stage 2
    // shapes are pinned structurally so unrelated numbering churn does not
    // invalidate the pin.
    for needle in [
        "llvm.func @exit",
        "llvm.func @mjrt_pow",
        "llvm.call @mjrt_pow",
        "llvm.call_intrinsic",
        "llvm.udiv",
        "llvm.lshr",
        "llvm.fadd",
        "llvm.fmul",
        "llvm.fcmp",
        "llvm.sitofp",
        "llvm.unreachable",
        "builtin.fp64",
    ] {
        assert!(
            text.contains(needle),
            "canonical text lacks `{needle}`:\n{text}"
        );
    }
    // Trap blocks call exit with the category exit codes.
    for code in [
        TrapCategory::DivModZero.exit_code(),
        TrapCategory::PowExponent.exit_code(),
    ] {
        assert!(
            text.contains(&format!("<{code}: i32>")),
            "canonical text lacks trap exit code {code}:\n{text}"
        );
    }
}

/// Source locations propagate into the canonical Pliron text as
/// file/line/column outlined attributes.
#[test]
fn locations_render_in_canonical_text() {
    let module = native_compile(FIB, &["compute"]);
    assert!(
        module
            .plir_text()
            .contains("@[\"pliron_fixture.mojo\": line: 2, column: 8]"),
        "{}",
        module.plir_text()
    );
}
