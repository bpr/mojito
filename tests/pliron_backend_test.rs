//! Differential and emission tests for the experimental Pliron backend
//! (feature `backend-pliron`; requires LLVM 22 — see scripts/check-pliron).

#![cfg(feature = "backend-pliron")]

use std::path::Path;

use expect_test::expect;
use mojito::backend::pliron as native;
use mojito::{Compiler, CompilerError, RuntimeError};
use native::{
    CompileOptions, DebugInfo, JitValue, NativeModule, NativeTarget, OptLevel, TrapCategory,
};

const FIXTURE_NAME: &str = "pliron_fixture.mojo";

/// The host as a native target; every test in this binary compiles for (and
/// JITs or runs on) the host.
fn host_target() -> NativeTarget {
    NativeTarget::host().expect("pliron tests require a supported host target")
}

/// The linked `mojito-runtime` exports as an explicit JIT symbol mapping, so
/// a JIT'd module referencing runtime-contract functions (`mjrt_trap` from
/// trap guards) resolves them deterministically instead of relying on
/// process-symbol resolution.
fn runtime_jit_symbols() -> Vec<(&'static str, u64)> {
    macro_rules! address {
        ($symbol:ident) => {
            (
                stringify!($symbol),
                mojito_runtime::$symbol as *const () as u64,
            )
        };
    }
    vec![
        address!(mjrt_version),
        address!(mjrt_alloc),
        address!(mjrt_dealloc),
        address!(mjrt_write_stdout),
        address!(mjrt_fmt_i64),
        address!(mjrt_fmt_u64),
        address!(mjrt_fmt_f64),
        address!(mjrt_trap),
        address!(mjrt_read_line),
    ]
}

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
        target: host_target(),
        trace_lifecycle: false,
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
fn backend_monomorphizes_one_generic_function_at_multiple_types() {
    let source = "def identity[T: Copyable & Movable](value: T) -> T:\n    return value.copy()\n\ndef int_value() -> Int:\n    return identity(42)\n\ndef bool_value() -> Bool:\n    return identity(True)\n";
    let module = native_compile(source, &["int_value", "bool_value"]);

    assert_eq!(module.jit_i64("int_value").unwrap(), 42);
    assert_eq!(
        module.jit_value("bool_value", OptLevel::O0).unwrap(),
        JitValue::Bool(true)
    );
    let text = module.plir_text();
    assert!(text.contains("identity_24y3_3aInt"), "{text}");
    assert!(text.contains("identity_24y4_3aBool"), "{text}");
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
    // Debugging filter for one or more comma-separated fixture substrings and
    // their differential/sanitizer lanes. The
    // parity gate skips its generated-file assertion and coverage ratchets in
    // this mode; UPDATE_EXPECT remains forbidden so a focused run can never
    // truncate the checked-in manifest.
    let only = std::env::var("MOJITO_PARITY_ONLY").ok();
    let filters = only.as_deref().map(|value| {
        value
            .split(',')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
    });
    let mut fixtures: Vec<_> = std::fs::read_dir(&root)
        .unwrap_or_else(|error| panic!("{dir} exists: {error}"))
        .filter_map(|entry| {
            let path = entry.expect("readable dir entry").path();
            let name = path.file_name()?.to_str()?;
            name.ends_with(".mojo").then(|| name.to_string())
        })
        .filter(|name| {
            filters
                .as_ref()
                .is_none_or(|filters| filters.iter().any(|filter| name.contains(filter)))
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
///   nothing on stdout while the runtime reports the category on stderr.
///   (Traps run only as subprocesses — an in-process JIT trap would exit the
///   test runner.)
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
            target: host_target(),
            trace_lifecycle: false,
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
                let symbols = runtime_jit_symbols();
                let at_o0 = module
                    .jit_value_with_symbols("compute", OptLevel::O0, &symbols)
                    .unwrap_or_else(|error| panic!("{rel}: JIT at O0 failed: {error}"));
                let at_o1 = module
                    .jit_value_with_symbols("compute", OptLevel::Release, &symbols)
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
            target: host_target(),
            trace_lifecycle: false,
        };
        let mut module = native::compile(compiled.elaborated_mir(), &options)
            .unwrap_or_else(|error| panic!("{}", error.display_with_sources(&options.sources)));
        let dir = tempfile::tempdir().expect("tempdir");
        for (level, opt) in [("O0", OptLevel::O0), ("release", OptLevel::Release)] {
            let exe = dir.path().join(format!("trap-{level}"));
            module
                .write_executable(&exe, opt, DebugInfo::Lines)
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
                "{rel}: trapping executable must print nothing on stdout at {level}"
            );
            let stderr = String::from_utf8_lossy(&run.stderr);
            assert!(
                stderr.contains(category.runtime_message()),
                "{rel}: trap stderr lacks the runtime message at {level}: {stderr}"
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
        target: host_target(),
        trace_lifecycle: false,
    };
    let mut module = native::compile(compiled.elaborated_mir(), &options)
        .unwrap_or_else(|error| panic!("{}", error.display_with_sources(&options.sources)));

    let dir = tempfile::tempdir().expect("tempdir");
    let object_path = dir.path().join("main.o");
    module
        .write_object(&object_path, OptLevel::O0, DebugInfo::Lines)
        .expect("object emission");
    let object_bytes = std::fs::read(&object_path).expect("object file exists");
    assert_eq!(&object_bytes[..4], b"\x7fELF", "object must be an ELF file");

    let exe_path = dir.path().join("main");
    module
        .write_executable(&exe_path, OptLevel::O0, DebugInfo::Lines)
        .expect("executable emission");
    let run = std::process::Command::new(&exe_path)
        .output()
        .expect("executable runs");
    assert_eq!(run.status.code(), Some(0), "executable must exit 0");
    assert!(run.stdout.is_empty(), "executable must print nothing");
}

/// Native executables compose the VM's exact bytes: for each print-capable
/// fixture — scalars, string literals, and the aggregate surface (fieldwise
/// and `__init__` construction, methods with `mut self` write-back, copies,
/// observable destructor order) — the emitted executable's stdout at `O0`
/// and `O1` equals the VM's execution output byte-for-byte, exiting 0 with
/// an empty stderr.
#[test]
fn print_fixture_exes_match_vm_output() {
    for fixture in [
        "assets/ok/pliron_print_scalars.mojo",
        "assets/ok/pliron_print_str_literal.mojo",
        "assets/ok/pliron_struct_fieldwise.mojo",
        "assets/ok/pliron_struct_init_method.mojo",
        "assets/ok/pliron_struct_drop_order.mojo",
        // Exercises the checked consuming Array-literal -> List parameter
        // conversion. O1 previously exposed the missing List `cap` field as
        // allocator corruption during the second Bank getter.
        "assets/ok/callable_element_call_dispatch.mojo",
    ] {
        let src = std::fs::read_to_string(fixture).expect("fixture exists");
        let compiler = Compiler::default();
        let compiled = compiler
            .compile_source(&src, Path::new(fixture))
            .unwrap_or_else(|error| panic!("{fixture}: must compile: {error}"));
        let execution = compiler
            .execute(&compiled)
            .unwrap_or_else(|error| panic!("{fixture}: must run on the VM: {error}"));
        let options = CompileOptions {
            entries: vec!["main".to_string()],
            sources: vec![(fixture.to_string(), src.clone())],
            target: host_target(),
            trace_lifecycle: false,
        };
        let mut module = native::compile(compiled.elaborated_mir(), &options)
            .unwrap_or_else(|error| panic!("{}", error.display_with_sources(&options.sources)));
        let dir = tempfile::tempdir().expect("tempdir");
        for (level, opt) in [("O0", OptLevel::O0), ("release", OptLevel::Release)] {
            let exe = dir.path().join(format!("print-{level}"));
            module
                .write_executable(&exe, opt, DebugInfo::Lines)
                .unwrap_or_else(|error| panic!("{fixture}: exe emission at {level}: {error}"));
            let run = std::process::Command::new(&exe)
                .output()
                .expect("print executable runs");
            assert_eq!(run.status.code(), Some(0), "{fixture}: exit at {level}");
            assert_eq!(
                String::from_utf8_lossy(&run.stdout),
                execution.output,
                "{fixture}: stdout bytes diverge from the VM at {level}"
            );
            assert!(
                run.stderr.is_empty(),
                "{fixture}: stderr must be empty at {level}"
            );
        }
    }
}

/// String literals intern into private `mjstr_<n>` constant-pool globals on
/// use — deduplicated by content in deterministic first-use order — and the
/// literal-only `+`/`==` operators fold at compile time (fold-only literals
/// never emit a global).
#[test]
fn string_constant_pool_interns_and_dedups() {
    let src = "\
def main():
    print(\"hello\")
    print(\"hello\", \"a\" + \"b\")
    print(\"ab\" == \"a\" + \"b\")
";
    let module = native_compile(src, &["main"]);
    let text = module.plir_text();
    for needle in [
        "llvm.global @mjstr_0",
        "llvm.linkage PrivateLinkage",
        "llvm.addressof @mjstr_0",
        "llvm.call @mjrt_write_stdout",
    ] {
        assert!(
            text.contains(needle),
            "canonical text lacks `{needle}`:\n{text}"
        );
    }
    // Pool contents in first-use order: "hello", "\n", " ", "ab" (the folded
    // concatenation), "True", "False" — the repeated "hello" and the folded
    // equality share entries, and the fold-only "a"/"b" never intern.
    let globals = text.matches("llvm.global @mjstr_").count();
    assert_eq!(globals, 6, "constant pool size changed:\n{text}");
}

/// The aggregate memory model prints canonically: layout-engine offsets
/// realize as `i8` GEPs, aggregate copies as `llvm.memcpy` intrinsic calls,
/// and struct storage as aligned byte allocas.
#[test]
fn aggregate_surface_prints_canonically() {
    let src =
        std::fs::read_to_string("assets/ok/pliron_struct_fieldwise.mojo").expect("fixture exists");
    let module = native_compile(&src, &["main"]);
    let text = module.plir_text();
    for needle in [
        "llvm.gep",
        "llvm.call_intrinsic",
        "llvm.memcpy.p0.p0.i64",
        "llvm.alloca",
        "[align : 8]",
    ] {
        assert!(
            text.contains(needle),
            "canonical text lacks `{needle}`:\n{text}"
        );
    }
}

/// The Stage 4 acceptance gate in one pass, one production compile per
/// fixture:
///
/// - Every `assets/ok` and `assets/ownership_ok` fixture with a `main` entry
///   either compiles natively — then its executable's stdout at `O0` and
///   `O1` must equal the VM's execution output byte-for-byte with exit 0 and
///   empty stderr (handled raises and `finally` paths included), and its
///   `O0` AddressSanitizer/LeakSanitizer build must run equally clean (no
///   leak, double free, or invalid access anywhere in the run) — or is
///   recorded `excluded` with its first rejection diagnostic.
/// - Every `assets/runtime_error/pliron_raise_*` fixture must raise in the VM
///   and exit natively with the unhandled-error category (69), reporting
///   `unhandled error: <message>` on stderr at both levels. (The VM's `run`
///   discards buffered partial stdout on an error while native executables
///   stream it — a recorded CLI-level divergence, so stdout is not compared
///   on the raise rows.)
///
/// The checked-in `conformance/pliron-parity.tsv` manifest must match
/// regeneration byte-exactly, and the trailing guards fail if eligible
/// coverage unexpectedly shrinks or exclusions grow.
/// Stdin bytes for fixtures that call `input()`. The same bytes feed the
/// in-process VM (via `VmBackend::set_input_override`) and every native
/// executable's piped stdin, so `input()` rows are true exe differentials.
/// Doubles as the "reads stdin" predicate: a hit must never run with
/// inherited stdin or the manifest test blocks under Cargo.
fn fixture_stdin(rel: &str) -> Option<&'static [u8]> {
    match rel.rsplit('/').next()? {
        "input.mojo" => Some(b"World\n"),
        "pliron_input_echo.mojo" => Some(b"echoed line\n"),
        _ => None,
    }
}

/// Run a parity executable, piping `stdin` bytes when present (an absent
/// entry inherits the test runner's stdin, which never blocks because such
/// fixtures don't read it).
fn run_executable(exe: &Path, stdin: Option<&[u8]>, envs: &[(&str, &str)]) -> std::process::Output {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut command = Command::new(exe);
    command.envs(envs.iter().copied());
    let Some(bytes) = stdin else {
        return command.output().expect("parity executable runs");
    };
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("parity executable spawns");
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(bytes)
        .expect("stdin bytes reach the executable");
    child.wait_with_output().expect("parity executable runs")
}

#[test]
fn parity_exe_manifest_and_differential() {
    let mut runnable = fixture_sources("assets/ok");
    runnable.extend(fixture_sources("assets/ownership_ok"));
    let ok_rows = parallel_map(runnable, |(rel, src)| {
        let compiler = Compiler::default();
        let Ok(compiled) = compiler.compile_source(&src, Path::new(&rel)) else {
            // Historical module-scope snippets compile only through the test
            // suite's non-conforming snippet mode.
            return (
                rel,
                "-".into(),
                "ineligible".to_string(),
                "non-conforming-snippet".into(),
            );
        };
        if !compiled
            .elaborated_mir()
            .functions
            .iter()
            .any(|(name, _)| name == "main")
        {
            return (
                rel,
                "-".into(),
                "ineligible".to_string(),
                "no-main-entry".into(),
            );
        }
        let mut entries = vec!["main".to_string()];
        if compiled
            .elaborated_mir()
            .functions
            .iter()
            .any(|(name, _)| name == "__toplevel__")
        {
            entries.push("__toplevel__".to_string());
        }
        let options = CompileOptions {
            entries,
            sources: vec![(rel.clone(), src.clone())],
            target: host_target(),
            trace_lifecycle: false,
        };
        match native::compile(compiled.elaborated_mir(), &options) {
            Err(error) => {
                let detail = error.display_with_sources(&options.sources);
                (rel, "main".into(), "excluded".to_string(), detail)
            }
            Ok(mut module) => {
                let stdin = fixture_stdin(&rel);
                let vm_output = match stdin {
                    // `input()` fixtures run on a VM with the same bytes the
                    // executables get piped, prompts captured in the output.
                    Some(bytes) => {
                        let mut vm = mojito::backend::VmBackend::new();
                        vm.set_input_override(bytes.to_vec());
                        vm.run_elaborated(compiled.elaborated_mir().clone())
                            .unwrap_or_else(|error| {
                                panic!("{rel}: fixture must run on the VM: {error}")
                            });
                        vm.output()
                    }
                    None => {
                        compiler
                            .execute(&compiled)
                            .unwrap_or_else(|error| {
                                panic!("{rel}: fixture must run on the VM: {error}")
                            })
                            .output
                    }
                };
                let dir = tempfile::tempdir().expect("tempdir");
                let asan_exe = dir.path().join("parity-asan");
                module
                    .write_executable_sanitized(&asan_exe, OptLevel::O0, DebugInfo::Lines)
                    .unwrap_or_else(|error| panic!("{rel}: sanitized emission: {error}"));
                let run = run_executable(&asan_exe, stdin, &[("ASAN_OPTIONS", "detect_leaks=1")]);
                assert_eq!(
                    run.status.code(),
                    Some(0),
                    "{rel}: sanitizer run failed:\n{}",
                    String::from_utf8_lossy(&run.stderr)
                );
                assert!(
                    run.stderr.is_empty(),
                    "{rel}: sanitizer diagnostics:\n{}",
                    String::from_utf8_lossy(&run.stderr)
                );
                for (level, opt) in [("O0", OptLevel::O0), ("release", OptLevel::Release)] {
                    let exe = dir.path().join(format!("parity-{level}"));
                    module
                        .write_executable(&exe, opt, DebugInfo::Lines)
                        .unwrap_or_else(|error| panic!("{rel}: exe emission at {level}: {error}"));
                    let run = run_executable(&exe, stdin, &[]);
                    assert_eq!(
                        run.status.code(),
                        Some(0),
                        "{rel}: exit at {level}: {:?}\nstdout:\n{}\nstderr:\n{}",
                        run.status,
                        String::from_utf8_lossy(&run.stdout),
                        String::from_utf8_lossy(&run.stderr)
                    );
                    assert_eq!(
                        String::from_utf8_lossy(&run.stdout),
                        vm_output,
                        "{rel}: stdout bytes diverge from the VM at {level}"
                    );
                    assert!(
                        run.stderr.is_empty(),
                        "{rel}: stderr must be empty at {level}: {}",
                        String::from_utf8_lossy(&run.stderr)
                    );
                }
                (
                    rel,
                    "main".into(),
                    "exe-differential".to_string(),
                    "sanitized".into(),
                )
            }
        }
    });

    let error_rows = parallel_map(fixture_sources("assets/runtime_error"), |(rel, src)| {
        let compiler = Compiler::default();
        let compiled = match compiler.compile_source(&src, Path::new(&rel)) {
            Ok(compiled) => compiled,
            Err(_) => {
                return (
                    rel,
                    "-".into(),
                    "ineligible".into(),
                    "non-conforming-snippet".into(),
                );
            }
        };
        let vm_error = match compiler
            .execute(&compiled)
            .expect_err("runtime-error fixture must fail on the VM")
        {
            CompilerError::Runtime(error) => error,
            error => panic!("{rel}: expected a runtime error, got {error}"),
        };
        let category = match &vm_error {
            RuntimeError::Raised(_) => TrapCategory::UnhandledError,
            RuntimeError::Abort(_) => TrapCategory::Abort,
            RuntimeError::TypeError(message) => TrapCategory::from_vm_message(message)
                .unwrap_or_else(|| panic!("{rel}: unmapped VM runtime error: {vm_error}")),
            _ => panic!("{rel}: unmapped VM runtime error: {vm_error}"),
        };
        let vm_error = vm_error.to_string();
        let mut entries = Vec::new();
        if compiled
            .elaborated_mir()
            .functions
            .iter()
            .any(|(name, _)| name == "main")
        {
            entries.push("main".to_string());
        }
        if compiled
            .elaborated_mir()
            .functions
            .iter()
            .any(|(name, _)| name == "__toplevel__")
        {
            entries.push("__toplevel__".to_string());
        }
        let entry_detail = entries.join(",");
        let options = CompileOptions {
            entries,
            sources: vec![(rel.clone(), src.clone())],
            target: host_target(),
            trace_lifecycle: false,
        };
        let mut module = native::compile(compiled.elaborated_mir(), &options)
            .unwrap_or_else(|error| panic!("{}", error.display_with_sources(&options.sources)));
        let dir = tempfile::tempdir().expect("tempdir");
        for (level, opt) in [("O0", OptLevel::O0), ("release", OptLevel::Release)] {
            let exe = dir.path().join(format!("error-{level}"));
            module
                .write_executable(&exe, opt, DebugInfo::Lines)
                .unwrap_or_else(|error| panic!("{rel}: exe emission at {level}: {error}"));
            let run = run_executable(&exe, fixture_stdin(&rel), &[]);
            assert_eq!(
                run.status.code(),
                Some(i32::from(category.exit_code())),
                "{rel}: runtime-error category diverges at {level}"
            );
            let expected_stderr = match category {
                TrapCategory::UnhandledError | TrapCategory::Abort => format!("{vm_error}\n"),
                _ => format!("mojito runtime trap: {}\n", category.runtime_message()),
            };
            assert_eq!(
                String::from_utf8_lossy(&run.stderr),
                expected_stderr,
                "{rel}: runtime-error stderr diverges at {level}"
            );
        }
        let sanitizer = dir.path().join("error-asan");
        module
            .write_executable_sanitized(&sanitizer, OptLevel::O0, DebugInfo::Lines)
            .unwrap_or_else(|error| panic!("{rel}: sanitized error emission: {error}"));
        let run = run_executable(
            &sanitizer,
            fixture_stdin(&rel),
            &[("ASAN_OPTIONS", "detect_leaks=0")],
        );
        assert_eq!(
            run.status.code(),
            Some(i32::from(category.exit_code())),
            "{rel}: sanitized runtime-error category diverges:\n{}",
            String::from_utf8_lossy(&run.stderr)
        );
        assert!(
            !String::from_utf8_lossy(&run.stderr).contains("AddressSanitizer"),
            "{rel}: sanitizer diagnosed native memory misuse:\n{}",
            String::from_utf8_lossy(&run.stderr)
        );
        (
            rel,
            entry_detail,
            "error-differential".to_string(),
            format!("category={category:?}"),
        )
    });
    let rows: Vec<(String, String, String, String)> =
        ok_rows.into_iter().chain(error_rows).collect();

    let mut manifest = String::from(
        "# Pliron native-parity manifest (generated; schema-version 1).\n\
         # One row per assets/ok, assets/ownership_ok, and assets/runtime_error fixture:\n\
         #   fixture <TAB> entry <TAB> status <TAB> detail\n\
         # status: exe-differential (VM/native stdout-byte oracle, O0+O1, ASan/LSan-clean) |\n\
         #         error-differential (VM/native runtime-error category oracle, O0+O1+ASan) |\n\
         #         excluded (native rejection diagnostic) |\n\
         #         ineligible (no runnable `main` shape for this gate)\n\
         # Regenerate: UPDATE_EXPECT=1 CARGO_WORKSPACE_DIR=$PWD \\\n\
         #   cargo nextest run --features backend-pliron parity_exe_manifest\n",
    );
    for (fixture, entry, status, detail) in &rows {
        manifest.push_str(&format!("{fixture}\t{entry}\t{status}\t{detail}\n"));
    }
    let focused = std::env::var_os("MOJITO_PARITY_ONLY").is_some();
    assert!(
        !(focused && std::env::var_os("UPDATE_EXPECT").is_some()),
        "MOJITO_PARITY_ONLY cannot be combined with UPDATE_EXPECT"
    );
    if !focused {
        expect_test::expect_file!["../conformance/pliron-parity.tsv"].assert_eq(&manifest);
    }

    // Coverage guards: the eligible sets must never silently shrink, the
    // exclusion count only ratchets down toward the Stage 5 zero-exclusion
    // target, and a pliron-named fixture must never regress to excluded.
    let count = |status: &str| rows.iter().filter(|(_, _, s, _)| s == status).count();
    let differential = count("exe-differential");
    let errors = count("error-differential");
    let excluded = count("excluded");
    if !focused {
        assert!(
            differential == 387,
            "exe-differential coverage must cover the complete runnable inventory: {differential} != 361"
        );
        assert!(
            errors == 30,
            "error-differential coverage must cover every runnable runtime-error fixture: {errors} != 29"
        );
        assert!(
            excluded == 0,
            "native exclusions remain after the zero-exclusion burn-down: {excluded}"
        );
    }
    for (fixture, _, status, detail) in &rows {
        let name = fixture.rsplit('/').next().unwrap_or(fixture);
        assert!(
            !(name.starts_with("pliron_") && status == "excluded"),
            "{fixture}: pliron fixture regressed to excluded: {detail}"
        );
    }
}

/// The generated capability matrix pins the advertised support surface per
/// MIR instruction mnemonic, checked-type constructor, and runtime symbol.
/// Module tests pin the instruction and type rows against the canonical
/// schema vocabulary, so a new MIR form forces a capability decision here.
#[test]
fn capability_matrix_is_current() {
    expect_test::expect_file!["../conformance/pliron-capability.tsv"]
        .assert_eq(&native::capability::matrix());
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
        target: host_target(),
        trace_lifecycle: false,
    };
    let error = native::compile(compiled.elaborated_mir(), &options)
        .err()
        .expect("the backend must reject this fixture");
    error.display_with_sources(&options.sources)
}

/// Every construct outside the currently advertised native subset produces a contextual
/// diagnostic naming the function and construct — no panics, no fallbacks.
#[test]
fn unsupported_constructs_produce_contextual_diagnostics() {
    // (source, entries, phrases the diagnostic must contain)
    let cases: &[(&str, &[&str], &[&str])] = &[
        // Pointer element arithmetic lowers `+` only (the `unsafe_offset`
        // form); `-` keeps a contextual rejection.
        (
            "from std.memory import unsafe_alloc\n\ndef compute() -> Int:\n    var p = unsafe_alloc[Int](2)\n    var q = p + 1\n    var d = q - 1\n    p.unsafe_free()\n    return 1\n",
            &["compute"],
            &["operator `Sub` on Pointer operands"],
        ),
        // An IntLiteral constant that exceeds i64 storage rejects instead of
        // wrapping — the VM keeps arbitrary precision in literal-typed slots
        // (materialization to concrete scalar widths wraps VM-exactly).
        (
            "comptime BIG = 2 ** 80\n\ndef main():\n    print(Int(BIG))\n",
            &["main", "__toplevel__"],
            &["integer literal 1208925819614629174706176 does not fit IntLiteral storage (i64)"],
        ),
        // A move capture whose type runs a user `__moveinit__` still rejects
        // before the backend could silently substitute a byte relocation for
        // the user-observable constructor.
        (
            "struct Loud(Movable):\n    var v: Int\n    def __init__(out self, v: Int):\n        self.v = v\n    def __moveinit__(out self, deinit other: Self):\n        self.v = other.v\n\ndef compute() -> Int:\n    var l = Loud(3)\n    var peek: def() capturing[_] -> Int = lambda {var l^} -> Int: l.v\n    return peek()\n",
            &["compute"],
            &["owned closure capture of `Loud` with a user `__moveinit__`"],
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

/// Negative ownership cases fail in the front end, before any backend runs:
/// the production pipeline rejects them during ownership analysis, so
/// `run --backend pliron` (which compiles through the same pipeline) can
/// never hand them to the native backend.
#[test]
fn negative_ownership_fixtures_fail_before_the_backend() {
    parallel_map(fixture_sources("assets/ownership_error"), |(rel, src)| {
        let compiler = Compiler::default();
        let error = compiler
            .compile_source(&src, Path::new(&rel))
            .err()
            .unwrap_or_else(|| panic!("{rel}: ownership-error fixture must be rejected"));
        // The rejection is a front-end diagnostic, not a backend one.
        let message = error.to_string();
        assert!(
            !message.contains("pliron"),
            "{rel}: rejection unexpectedly reached the native backend: {message}"
        );
    });
}

/// Ordered lifecycle traces — destructor dispatches, consumes, raises,
/// catches — match between the VM's test-only event log and a
/// trace-instrumented native executable: the Stage 4 ordered
/// create/drop/error acceptance beyond stdout byte parity.
#[test]
fn lifecycle_event_traces_match_the_vm() {
    for fixture in [
        "assets/ok/exceptions.mojo",
        "assets/ok/explicit_destroy_raising.mojo",
        "assets/ok/inplace_raises_try.mojo",
        "assets/ok/pliron_pointer_lifecycle.mojo",
        "assets/ok/try_return.mojo",
        "assets/ok/pliron_struct_drop_order.mojo",
        "assets/ok/pliron_iter_drop_order.mojo",
        "assets/ok/pliron_partial_move_drop.mojo",
        "assets/ok/pliron_closure_drop_order.mojo",
        // pliron_list_core stays out of this lane even with instance names
        // normalized to bare templates: generic-collection copy/consume
        // events differ structurally (compiled destructor chains trace
        // element drops the VM's arena never performs).
    ] {
        let src = std::fs::read_to_string(fixture).expect("fixture exists");
        let compiler = Compiler::default();
        let compiled = compiler
            .compile_source(&src, Path::new(fixture))
            .unwrap_or_else(|error| panic!("{fixture}: must compile: {error}"));
        let mut vm = mojito::backend::VmBackend::new();
        vm.enable_lifecycle_log();
        vm.run_elaborated(compiled.elaborated_mir().clone())
            .unwrap_or_else(|error| panic!("{fixture}: must run on the VM: {error}"));
        let vm_events: Vec<String> = vm.lifecycle_log().expect("log enabled").to_vec();

        let mut entries = vec!["main".to_string()];
        if compiled
            .elaborated_mir()
            .functions
            .iter()
            .any(|(name, _)| name == "__toplevel__")
        {
            entries.push("__toplevel__".to_string());
        }
        let options = CompileOptions {
            entries,
            sources: vec![(fixture.to_string(), src.clone())],
            target: host_target(),
            trace_lifecycle: true,
        };
        let mut module = native::compile(compiled.elaborated_mir(), &options)
            .unwrap_or_else(|error| panic!("{}", error.display_with_sources(&options.sources)));
        let dir = tempfile::tempdir().expect("tempdir");
        let exe = dir.path().join("traced");
        module
            .write_executable(&exe, OptLevel::O0, DebugInfo::Lines)
            .unwrap_or_else(|error| panic!("{fixture}: exe emission: {error}"));
        let run = std::process::Command::new(&exe)
            .output()
            .expect("traced executable runs");
        assert_eq!(run.status.code(), Some(0), "{fixture}: exit");
        let native_events: Vec<String> = String::from_utf8_lossy(&run.stderr)
            .lines()
            .filter_map(|line| line.strip_prefix("mjtrace ").map(str::to_string))
            .collect();
        assert_eq!(
            native_events, vm_events,
            "{fixture}: ordered lifecycle traces diverge"
        );
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
/// and the shared `mjrt_pow`/`mjrt_trap` scaffolding all print canonically.
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
        "llvm.func @mjrt_trap",
        "llvm.call @mjrt_trap",
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
    // Trap blocks pass the category codes to `mjrt_trap`.
    for category in [TrapCategory::DivModZero, TrapCategory::PowExponent] {
        let code = category.code();
        assert!(
            text.contains(&format!("<{code}: i32>")),
            "canonical text lacks trap category code {code}:\n{text}"
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

/// LLVM-side mechanical cross checks of the shared native ABI
/// (`mojito::native`): target-data layout agreement, the contract table's
/// LLVM declaration rendering, the pinned data-layout string against the
/// installed toolchain, and the runtime symbols of produced executables.
/// Nothing in this module executes generated code — these are the
/// "target-only cross checks" of the ABI milestone's acceptance.
mod native_abi_cross_checks {
    use std::ffi::CString;
    use std::process::Command;

    use expect_test::expect;
    use llvm_sys::core::{
        LLVMContextCreate, LLVMContextDispose, LLVMDoubleTypeInContext, LLVMInt1TypeInContext,
        LLVMInt32TypeInContext, LLVMInt64TypeInContext, LLVMPointerTypeInContext,
        LLVMStructTypeInContext,
    };
    use llvm_sys::prelude::{LLVMContextRef, LLVMTypeRef};
    use llvm_sys::target::{
        LLVMABIAlignmentOfType, LLVMABISizeOfType, LLVMCreateTargetData, LLVMDisposeTargetData,
        LLVMOffsetOfElement, LLVMTargetDataRef,
    };
    use mojito::Compiler;
    use mojito::native::layout::{LayoutCx, StructFieldIndex, StructLayout};
    use mojito::native::rt_abi::{self, CAbiTy, RtFieldTy};
    use mojito::native::target::{NativeTarget, Triple};
    use mojito::types::Ty;

    use super::{EXE_MAIN, FIXTURE_NAME, host_target};

    /// LLVM target data built from the pinned data-layout string alone — no
    /// module, no code generation, no execution.
    struct TargetData {
        ctx: LLVMContextRef,
        td: LLVMTargetDataRef,
    }

    impl TargetData {
        fn new(triple: Triple) -> TargetData {
            let layout = CString::new(triple.data_layout()).expect("no NUL in data layout");
            unsafe {
                TargetData {
                    ctx: LLVMContextCreate(),
                    td: LLVMCreateTargetData(layout.as_ptr()),
                }
            }
        }

        fn prim(&self, ty: CAbiTy) -> LLVMTypeRef {
            unsafe {
                match ty {
                    CAbiTy::U32 => LLVMInt32TypeInContext(self.ctx),
                    CAbiTy::U64 | CAbiTy::I64 => LLVMInt64TypeInContext(self.ctx),
                    CAbiTy::F64 => LLVMDoubleTypeInContext(self.ctx),
                    CAbiTy::PtrConstU8 | CAbiTy::PtrMutU8 => LLVMPointerTypeInContext(self.ctx, 0),
                }
            }
        }

        fn strukt(&self, fields: &[LLVMTypeRef]) -> LLVMTypeRef {
            let mut fields = fields.to_vec();
            unsafe {
                LLVMStructTypeInContext(self.ctx, fields.as_mut_ptr(), fields.len() as u32, 0)
            }
        }

        fn rt_type(&self, spec: &rt_abi::RtTypeSpec) -> LLVMTypeRef {
            let fields: Vec<LLVMTypeRef> = spec
                .fields
                .iter()
                .map(|field| match field.ty {
                    RtFieldTy::Prim(prim) => self.prim(prim),
                    RtFieldTy::Named(name) => {
                        self.rt_type(rt_abi::find_type(name).expect("known type"))
                    }
                })
                .collect();
            self.strukt(&fields)
        }

        /// The LLVM realization of a checked type under the shared layout
        /// rules (scalars, pointers, descriptor/error structs, tuples, and
        /// nominal structs; Variant's overlay is realized only in Stage 4).
        fn ty(&self, structs: &StructFieldIndex, ty: &Ty) -> LLVMTypeRef {
            unsafe {
                match ty {
                    Ty::Int | Ty::UInt => LLVMInt64TypeInContext(self.ctx),
                    Ty::Bool => LLVMInt1TypeInContext(self.ctx),
                    Ty::Float64 => LLVMDoubleTypeInContext(self.ctx),
                    Ty::None => self.strukt(&[]),
                    Ty::Pointer { .. } | Ty::Ref(_) => LLVMPointerTypeInContext(self.ctx, 0),
                    Ty::StringLiteral => {
                        let ptr = LLVMPointerTypeInContext(self.ctx, 0);
                        let len = LLVMInt64TypeInContext(self.ctx);
                        self.strukt(&[ptr, len])
                    }
                    Ty::Error => {
                        let string = self.rt_type(rt_abi::find_type("MjString").expect("known"));
                        self.strukt(&[string])
                    }
                    Ty::Tuple(elements) | Ty::RuntimePack(elements) => {
                        let fields: Vec<LLVMTypeRef> = elements
                            .iter()
                            .map(|element| self.ty(structs, element))
                            .collect();
                        self.strukt(&fields)
                    }
                    Ty::Struct(name, _) => {
                        let fields: Vec<LLVMTypeRef> = structs
                            .get(name)
                            .expect("struct in index")
                            .iter()
                            .map(|field| self.ty(structs, field))
                            .collect();
                        self.strukt(&fields)
                    }
                    other => panic!("no LLVM realization in this test for {other}"),
                }
            }
        }

        fn assert_agrees(&self, label: &str, llvm_ty: LLVMTypeRef, expected: &StructLayout) {
            unsafe {
                assert_eq!(
                    LLVMABISizeOfType(self.td, llvm_ty),
                    expected.layout.size,
                    "{label}: size"
                );
                assert_eq!(
                    u64::from(LLVMABIAlignmentOfType(self.td, llvm_ty)),
                    expected.layout.align,
                    "{label}: align"
                );
                for (index, offset) in expected.offsets.iter().enumerate() {
                    assert_eq!(
                        LLVMOffsetOfElement(self.td, llvm_ty, index as u32),
                        *offset,
                        "{label}: offset of field {index}"
                    );
                }
            }
        }
    }

    impl Drop for TargetData {
        fn drop(&mut self) {
            unsafe {
                LLVMDisposeTargetData(self.td);
                LLVMContextDispose(self.ctx);
            }
        }
    }

    /// Every exported runtime type and a representative checked-type matrix
    /// lay out identically under the shared layout engine and LLVM's own
    /// target data for the pinned data-layout string.
    #[test]
    fn pliron_layout_engine_agrees_with_llvm_target_data() {
        let triple = Triple::X86_64UnknownLinuxGnu;
        let target = NativeTarget::new(triple);
        let td = TargetData::new(triple);

        for spec in rt_abi::RT_TYPES {
            let expected = rt_abi::type_layout(spec, &target);
            td.assert_agrees(spec.name, td.rt_type(spec), &expected);
        }

        let mut structs = StructFieldIndex::default();
        structs.insert("Pair".to_string(), vec![Ty::Int, Ty::Bool]);
        structs.insert(
            "Outer".to_string(),
            vec![
                Ty::Struct("Pair".to_string(), vec![]),
                Ty::Bool,
                Ty::Float64,
            ],
        );
        let cx = LayoutCx {
            target: &target,
            structs: &structs,
        };
        let aggregate_cases: &[(&str, Vec<Ty>)] = &[
            ("bool_int_bool", vec![Ty::Bool, Ty::Int, Ty::Bool]),
            ("bool_bool_int", vec![Ty::Bool, Ty::Bool, Ty::Int]),
            ("empty", vec![]),
            (
                "mixed",
                vec![
                    Ty::Float64,
                    Ty::Bool,
                    Ty::StringLiteral,
                    Ty::Struct("Outer".to_string(), vec![]),
                ],
            ),
        ];
        for (label, fields) in aggregate_cases {
            let expected = cx.struct_layout(fields).expect("layout");
            let llvm_ty = td.ty(&structs, &Ty::Tuple(fields.clone()));
            td.assert_agrees(label, llvm_ty, &expected);
        }
        let scalar_cases = [
            Ty::Int,
            Ty::UInt,
            Ty::Bool,
            Ty::Float64,
            Ty::StringLiteral,
            Ty::Error,
            Ty::Struct("Outer".to_string(), vec![]),
        ];
        for ty in scalar_cases {
            let expected = cx.layout_of(&ty).expect("layout");
            let llvm_ty = td.ty(&structs, &ty);
            unsafe {
                assert_eq!(
                    LLVMABISizeOfType(td.td, llvm_ty),
                    expected.size,
                    "{ty}: size"
                );
                assert_eq!(
                    u64::from(LLVMABIAlignmentOfType(td.td, llvm_ty)),
                    expected.align,
                    "{ty}: align"
                );
            }
        }
    }

    /// The mechanical LLVM rendering of the runtime contract table.
    #[test]
    fn pliron_runtime_declarations_render_the_contract_table() {
        expect![[r#"
            @mjrt_abi_version = external global i32
            declare i32 @mjrt_version()
            declare ptr @mjrt_alloc(i64, i64)
            declare void @mjrt_free(ptr)
            declare i32 @mjrt_pointer_status(ptr)
            declare void @mjrt_dealloc(ptr, i64, i64)
            declare void @mjrt_write_stdout(ptr, i64)
            declare i64 @mjrt_fmt_i64(i64, ptr)
            declare i64 @mjrt_fmt_u64(i64, ptr)
            declare i64 @mjrt_fmt_f64(double, ptr)
            declare void @mjrt_trap(i32) noreturn
            declare void @mjrt_unhandled_error(ptr, i64) noreturn
            declare void @mjrt_abort(ptr, i64) noreturn
            declare void @mjrt_trace(i32, ptr, i64)
            declare void @mjrt_read_line(ptr)
        "#]]
        .assert_eq(&mojito::backend::pliron::runtime_declarations());
    }

    /// The pinned data-layout string equals what the installed clang produces
    /// for the same triple, so `opt`, `clang`, and the layout engine agree.
    #[test]
    fn pliron_pinned_data_layout_matches_installed_clang() {
        let triple = Triple::X86_64UnknownLinuxGnu;
        let clang = ["clang-22", "clang"]
            .into_iter()
            .find(|candidate| {
                Command::new(candidate)
                    .arg("--version")
                    .output()
                    .is_ok_and(|out| out.status.success())
            })
            .expect("clang available (the pliron lane requires it)");
        let output = Command::new(clang)
            .args([
                &format!("--target={}", triple.name()),
                "-x",
                "c",
                "/dev/null",
                "-S",
                "-emit-llvm",
                "-o",
                "-",
            ])
            .output()
            .expect("clang runs");
        assert!(output.status.success());
        let text = String::from_utf8_lossy(&output.stdout);
        let line = text
            .lines()
            .find(|line| line.starts_with("target datalayout = "))
            .expect("clang output has a data-layout line");
        assert_eq!(
            line,
            format!("target datalayout = \"{}\"", triple.data_layout())
        );
    }

    /// Produced executables expose the inspectable ABI version symbol and no
    /// runtime symbols outside the contract table (plus the backend-emitted
    /// `mjrt_pow` helper) — checked over both a scalar executable and a
    /// Stage 3 executable that prints, allocates, and carries constant-pool
    /// strings (whose `mjstr_*` globals must stay private). Inspection only —
    /// the executables never run.
    #[test]
    fn pliron_executables_expose_only_specified_runtime_symbols() {
        const STAGE3_MAIN: &str = "\
def main():
    var s = \"symbols\"
    print(s, 42)
";
        for source in [EXE_MAIN, STAGE3_MAIN] {
            let compiler = Compiler::default();
            let compiled = compiler
                .compile_source(source, std::path::Path::new(FIXTURE_NAME))
                .expect("fixture compiles");
            let options = super::CompileOptions {
                entries: vec!["main".to_string()],
                sources: vec![(FIXTURE_NAME.to_string(), source.to_string())],
                target: host_target(),
                trace_lifecycle: false,
            };
            let mut module = mojito::backend::pliron::compile(compiled.elaborated_mir(), &options)
                .unwrap_or_else(|error| panic!("{}", error.display_with_sources(&options.sources)));
            let dir = tempfile::tempdir().expect("tempdir");
            let exe = dir.path().join("abi_inspect");
            module
                .write_executable(&exe, super::OptLevel::O0, super::DebugInfo::Lines)
                .expect("exe emission");

            let nm = ["llvm-nm-22", "llvm-nm", "nm"]
                .into_iter()
                .find(|candidate| {
                    Command::new(candidate)
                        .arg("--version")
                        .output()
                        .is_ok_and(|out| out.status.success())
                })
                .expect("an nm tool available");
            let output = Command::new(nm).arg(&exe).output().expect("nm runs");
            assert!(output.status.success());
            let text = String::from_utf8_lossy(&output.stdout);
            let exported: Vec<(&str, &str)> = text
                .lines()
                .filter_map(|line| {
                    let mut parts = line.split_whitespace().rev();
                    let name = parts.next()?;
                    let kind = parts.next()?;
                    (kind.len() == 1 && kind.chars().all(|c| c.is_ascii_uppercase()))
                        .then_some((name, kind))
                })
                .collect();
            let globals: Vec<&str> = exported
                .iter()
                .filter(|(name, _)| name.starts_with("mjrt_"))
                .map(|(name, _)| *name)
                .collect();
            assert!(
                globals.contains(&rt_abi::ABI_VERSION_SYMBOL),
                "executable must expose {}: {globals:?}",
                rt_abi::ABI_VERSION_SYMBOL
            );
            assert!(globals.contains(&"mjrt_version"), "{globals:?}");
            let allowed: Vec<&str> = rt_abi::RT_SYMBOLS
                .iter()
                .map(|sig| sig.symbol)
                .chain(rt_abi::RT_DATA_SYMBOLS.iter().map(|(name, _)| *name))
                .chain(["mjrt_pow"])
                .collect();
            for symbol in &globals {
                assert!(
                    allowed.contains(symbol),
                    "unexpected runtime symbol {symbol} in the executable (contract table: {allowed:?})"
                );
            }
            // The constant pool is private linkage — never exported.
            for (name, _) in &exported {
                assert!(
                    !name.starts_with("mjstr_"),
                    "constant-pool global {name} leaked into the export surface"
                );
            }
        }
    }
}
