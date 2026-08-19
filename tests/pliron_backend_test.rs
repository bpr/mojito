//! Differential and emission tests for the experimental Pliron backend
//! (feature `backend-pliron`; requires LLVM 22 — see scripts/check-pliron).

#![cfg(feature = "backend-pliron")]

use std::path::Path;

use expect_test::expect;
use mojito::Compiler;
use mojito::backend::pliron as native;
use native::{CompileOptions, NativeModule};

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
        first.llvm_ir().expect("LLVM conversion"),
        second.llvm_ir().expect("LLVM conversion"),
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

/// Run a fixture through the VM and parse its single printed integer.
fn vm_value(src: &str) -> i64 {
    let compiler = Compiler::default();
    let compiled = compiler
        .compile_source(src, Path::new(FIXTURE_NAME))
        .unwrap_or_else(|error| panic!("fixture must compile: {error}"));
    let execution = compiler
        .execute(&compiled)
        .unwrap_or_else(|error| panic!("fixture must run on the VM: {error}"));
    execution.output.trim().parse().unwrap_or_else(|error| {
        panic!(
            "fixture must print one integer: {error}: {:?}",
            execution.output
        )
    })
}

/// Go/no-go B: every `assets/ok/pliron_*.mojo` fixture agrees between the VM
/// (printed value of `compute()`) and the JIT-executed native `compute`.
#[test]
fn differential_fixtures_match_vm() {
    let fixtures_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/ok");
    let mut fixtures: Vec<_> = std::fs::read_dir(&fixtures_dir)
        .expect("assets/ok exists")
        .filter_map(|entry| {
            let path = entry.expect("readable dir entry").path();
            let name = path.file_name()?.to_str()?;
            (name.starts_with("pliron_") && name.ends_with(".mojo")).then_some(path)
        })
        .collect();
    fixtures.sort();
    assert!(
        fixtures.len() >= 7,
        "differential corpus unexpectedly shrank: {fixtures:?}"
    );

    for path in fixtures {
        let src = std::fs::read_to_string(&path).expect("fixture reads");
        let expected = vm_value(&src);
        let module = native_compile(&src, &["compute"]);
        let actual = module
            .jit_i64("compute")
            .unwrap_or_else(|error| panic!("{}: JIT failed: {error}", path.display()));
        assert_eq!(
            actual,
            expected,
            "{}: native result diverges from the VM",
            path.display()
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
    module.write_object(&object_path).expect("object emission");
    let object_bytes = std::fs::read(&object_path).expect("object file exists");
    assert_eq!(&object_bytes[..4], b"\x7fELF", "object must be an ELF file");

    let exe_path = dir.path().join("main");
    module
        .write_executable(&exe_path)
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
            "def compute() -> Int:\n    var x = 1.5\n    return 1\n",
            &["compute"],
            &["in `compute`", "unsupported", "Float64"],
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
        (
            "def compute() -> Int:\n    return 2 ** 10\n",
            &["compute"],
            &["in `compute`", "operator `Pow`"],
        ),
        (
            "def compute() -> Int:\n    return 9223372036854775808\n",
            &["compute"],
            &[
                "in `compute`",
                "9223372036854775808 does not fit Int (i64)",
                "pliron_fixture.mojo:2:",
            ],
        ),
        (
            "def helper() raises -> Int:\n    raise Error(\"boom\")\n\ndef compute() -> Int:\n    try:\n        return helper()\n    except:\n        return 0\n",
            &["compute"],
            &["in `compute`", "unsupported"],
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
