//! Differential and emission tests for the supported Pliron backend
//! (feature `backend-pliron`; requires LLVM 23.1 — see scripts/check-pliron).
//!
//! Targeted cases only. The whole-corpus sweeps behind the generated
//! manifests are memory-heavy and live in `tests/heavy/main.rs`.

#![cfg(feature = "backend-pliron")]

use std::path::Path;

use expect_test::expect;
use mojito::Compiler;
use mojito::backend::pliron as native;
use native::{CompileOptions, DebugInfo, JitValue, NativeModule, OptLevel, TrapCategory};

const FIXTURE_NAME: &str = "pliron_fixture.mojo";

#[path = "pliron_support/mod.rs"]
mod support;

use support::{host_target, runtime_jit_symbols};

/// The JIT mapping is the whole runtime function contract, in its order.
#[test]
fn runtime_jit_symbols_cover_the_contract() {
    let mapped: Vec<&str> = runtime_jit_symbols()
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    let contract: Vec<&str> = mojito::native::rt_abi::RT_SYMBOLS
        .iter()
        .map(|signature| signature.symbol)
        .collect();
    assert_eq!(mapped, contract);
}

/// Compile `src` through the production pipeline and hand its cached
/// post-drop MIR to the Pliron backend with the given entries.
fn native_compile(src: &str, entries: &[&str]) -> NativeModule {
    let compiler = Compiler::default();
    let compiled = compiler
        .compile_source(src, Path::new(FIXTURE_NAME))
        .unwrap_or_else(|error| panic!("fixture must compile: {error}"));
    let options = CompileOptions {
        entries: entries.iter().map(ToString::to_string).collect(),
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

/// Every multi-lane SIMD form (construction, splat, elementwise, compare,
/// select, shuffle, lane access, casts, `to_bits`, reductions) — the
/// vector-typed pliron surface the canonical text must round-trip.
const SIMD_SURFACE: &str = "\
def compute() -> Int:
    var v = SIMD[DType.int32, 4](1, 2, 3, 4)
    var w = v * 3 + SIMD[DType.int32, 4](7)
    var m = w > 10
    var picked = m.select(w, -v)
    var f = picked.cast[DType.float64]() / 2.0
    var g = f.shuffle[3, 2, 1, 0]()
    var b = g.cast[DType.int8]().to_bits[DType.uint16]()
    var i = 2
    b[i] = b[i - 1] << 3
    var flags = SIMD[DType.bool, 8](True)
    var total: Int = SIMD[DType.int, 4](b[0].cast[DType.int](), 1, 2, 3).reduce_add()
    if m.reduce_and() and not flags.reduce_or():
        total += 1
    return total + Int(g.reduce_max()) + Int(w.reduce_mul()) + Int(f.reduce_add())

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

/// Vector kernels over run-time values (constants would fold away), for
/// inspecting the code the release pipeline actually selects.
const SIMD_KERNELS: &str = "\
def scale(v: SIMD[DType.int32, 8], k: Int32) -> SIMD[DType.int32, 8]:
    return v * k + v

def total(v: SIMD[DType.int32, 8]) -> Int32:
    return (v << 1).reduce_add()

def mask_sum(v: SIMD[DType.float32, 8], limit: Float32) -> Float32:
    var m = v > limit
    return m.select(v, 0.0).reduce_add()

def prefix(v: SIMD[DType.int32, 8]) -> SIMD[DType.int32, 8]:
    var out = v
    for i in range(1, 8):
        out[i] = out[i] + out[i - 1]
    return out

def carry(v: SIMD[DType.int32, 8], k: Int32) -> Int32:
    var a = v
    var b = a
    a = a + k
    return (a + b).reduce_add()

def truncate(v: SIMD[DType.float32, 8]) -> SIMD[DType.int32, 8]:
    return v.cast[DType.int32]()

def compute() -> Int:
    var v = SIMD[DType.int32, 8](1, 2, 3, 4, 5, 6, 7, 8)
    var acc = 0
    for i in range(3):
        acc += Int(total(scale(v, Int32(i))))
    var p = prefix(v)
    var f = v.cast[DType.float32]()
    return acc + Int(mask_sum(f, 3.5)) + Int(p.reduce_add()) + Int(carry(v, 2)) + Int(truncate(f).reduce_add())

def main():
    print(compute())
";

/// The vector lowering must survive the release pipeline *as vector code*:
/// legal vector IR proves nothing if LLVM scalarizes it. Release IR keeps
/// the `<8 x i32>` arithmetic, the `<8 x i1>` select, and the
/// `llvm.vector.reduce.*` calls, and the emitted object selects packed SSE
/// instructions for them (checked when the pinned toolchain's
/// `llvm-objdump` is present; the baseline x86-64 target has no SSE4.1, so
/// the i32 multiply is `pmuludq` pairs rather than `pmulld`).
///
/// A float→int cast is pinned in both arms: the release IR keeps the
/// whole-vector `fptosi` behind its range guard, and the object code selects
/// `cvttps2dq` for it.
///
/// Lane *writes* are pinned at `O0` instead: `insertelement` is what the
/// backend emits, and the release pipeline is free to fold an unrolled
/// insert chain into something else entirely. `O0` is also where promotion
/// is visible — every access to a multi-lane slot is a typed whole-vector
/// load or store, so mem2reg lifts the slot into vector SSA (`prefix`'s
/// lane-written `out` becomes a `phi <8 x i32>`, and `carry`, which only
/// moves whole vectors, keeps no storage at all).
#[test]
fn simd_lowering_emits_vector_code() {
    let mut module = native_compile(SIMD_KERNELS, &["compute", "main"]);
    let unoptimized = module.llvm_ir(OptLevel::O0).expect("LLVM conversion");
    let lane_writer = llvm_function(&unoptimized, "prefix");
    for needle in [
        "load <8 x i32>",
        "insertelement <8 x i32>",
        "store <8 x i32>",
        "phi <8 x i32>",
    ] {
        assert!(
            lane_writer.contains(needle),
            "a lane write lowers to no `{needle}`:\n{lane_writer}"
        );
    }
    // A whole-vector move is a typed load/store pair, not an `llvm.memcpy`
    // (a non-promotable use that would pin every slot it touches).
    let mover = llvm_function(&unoptimized, "carry");
    for needle in ["memcpy", "alloca"] {
        assert!(
            !mover.contains(needle),
            "a whole-vector move still emits `{needle}`:\n{mover}"
        );
    }
    // A float→int cast emits a range guard over the whole vector and, on
    // its fast path, one `fptosi`; the per-lane
    // `llvm.fptosi.sat.i128.f64` loop is the other arm of the branch,
    // reached only when a lane is out of range, infinite, or NaN.
    let caster = llvm_function(&unoptimized, "truncate");
    for needle in [
        "@llvm.fabs.v8f32(",
        "fcmp uge <8 x float>",
        "@llvm.vector.reduce.or.v8i1(",
        "fptosi <8 x float>",
        "@llvm.fptosi.sat.i128.f64(",
    ] {
        assert!(
            caster.contains(needle),
            "a float→int cast lacks `{needle}`:\n{caster}"
        );
    }
    let ir = module.llvm_ir(OptLevel::Release).expect("LLVM conversion");
    for needle in [
        "mul <8 x i32>",
        "shl <8 x i32>",
        "@llvm.vector.reduce.add.v8i32(",
        "fcmp ogt <8 x float>",
        "select <8 x i1>",
        "@llvm.vector.reduce.fadd.v8f32(float -0.000000e+00",
    ] {
        assert!(ir.contains(needle), "release IR lacks `{needle}`:\n{ir}");
    }
    // The cast's whole-vector conversion has to survive the release
    // pipeline as a vector; the guard's own shape is pinned at `O0` above,
    // where the optimizer has not yet rewritten the mask reduction.
    let caster = llvm_function(&ir, "truncate");
    assert!(
        caster.contains("fptosi <8 x float>"),
        "a float→int cast's fast path is no longer a whole-vector `fptosi`:\n{caster}"
    );
    let Some(objdump) = llvm_objdump() else {
        eprintln!("skipping object inspection: no llvm-objdump in the pinned toolchain");
        return;
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let object = dir.path().join("simd_kernels.o");
    module
        .write_object(&object, OptLevel::Release, DebugInfo::Lines)
        .expect("object emission");
    let listing = std::process::Command::new(objdump)
        .arg("-d")
        .arg(&object)
        .output()
        .expect("llvm-objdump runs");
    let text = String::from_utf8_lossy(&listing.stdout);
    for mnemonic in ["paddd", "pslld", "cmpltps", "cvttps2dq"] {
        assert!(
            text.contains(mnemonic),
            "object code selects no `{mnemonic}`:\n{text}"
        );
    }
}

/// Runtime arguments distinguish lane insertion from constructor insertion,
/// while mutable parameters keep the ABI's lane-aligned memory observable.
#[test]
fn simd_lane_writes_use_insertelement_at_o0() {
    let module = native_compile(
        "\
def write_int(mut v: SIMD[DType.int32, 4], i: Int, x: Int32):
    v[i] = x

def write_float(mut v: SIMD[DType.float32, 4], i: Int, x: Float32):
    v[i] = x

def write_bool(mut v: SIMD[DType.bool, 4], i: Int, x: Bool):
    v[i] = x
",
        &["write_int", "write_float", "write_bool"],
    );
    let ir = module.llvm_ir(OptLevel::O0).expect("LLVM conversion");
    for (name, lane, storage_lane, alignment) in [
        ("write_int", "i32", "i32", 4),
        ("write_float", "float", "float", 4),
        ("write_bool", "i1", "i8", 1),
    ] {
        let symbol = format!("@{}(", module.mangled_name(name).expect("entry symbol"));
        let body = llvm_function(&ir, &symbol);
        let insertion = format!("insertelement <4 x {lane}> %");
        let inserts: Vec<_> = body
            .lines()
            .filter(|line| line.contains("insertelement "))
            .collect();
        assert_eq!(inserts.len(), 1, "{name}: exactly one lane update:\n{body}");
        assert!(
            inserts[0].contains(&insertion)
                && inserts[0].contains(&format!(", {lane} %"))
                && inserts[0].contains(", i64 %"),
            "{name}: vector, value, and index must be runtime operands:\n{body}"
        );
        for operation in ["load", "store"] {
            let needle = format!("{operation} <4 x {storage_lane}>");
            let accesses: Vec<_> = body.lines().filter(|line| line.contains(&needle)).collect();
            assert!(!accesses.is_empty(), "{name}: missing {needle}:\n{body}");
            let align = format!(", align {alignment}");
            assert!(
                accesses
                    .iter()
                    .all(|line| { line.ends_with(&align) || line.contains(&format!("{align},")) }),
                "{name}: vector memory must use lane alignment:\n{body}"
            );
        }
        assert!(
            !body.contains(&format!("store {storage_lane} ")),
            "{name}: lane writes must store the updated vector:\n{body}"
        );
        if lane == "i1" {
            assert!(
                body.contains("icmp ne <4 x i8>")
                    && body.lines().any(|line| {
                        line.contains("zext <4 x i1>") && line.contains("to <4 x i8>")
                    }),
                "Bool masks must cross byte-per-lane storage explicitly:\n{body}"
            );
        }
    }
}

/// The body of the LLVM function whose name contains `name`, from `define`
/// to its closing brace.
fn llvm_function<'a>(ir: &'a str, name: &str) -> &'a str {
    let start = ir
        .match_indices("\ndefine ")
        .map(|(at, _)| at + 1)
        .find(|start| {
            ir[*start..]
                .lines()
                .next()
                .is_some_and(|line| line.contains(name))
        })
        .unwrap_or_else(|| panic!("no `define` for `{name}`:\n{ir}"));
    let end = ir[start..]
        .find("\n}")
        .map_or(ir.len(), |at| start + at + 2);
    &ir[start..end]
}

/// The pinned toolchain's `llvm-objdump`, if installed: beside the LLVM
/// prefix the backend links against, else a versioned or bare name on
/// `PATH`.
fn llvm_objdump() -> Option<std::path::PathBuf> {
    let prefix = std::env::var_os("LLVM_SYS_231_PREFIX").map_or_else(
        || std::path::PathBuf::from("/opt/llvm-23"),
        std::path::PathBuf::from,
    );
    [
        prefix.join("bin/llvm-objdump"),
        std::path::PathBuf::from("llvm-objdump-23"),
        std::path::PathBuf::from("llvm-objdump"),
    ]
    .into_iter()
    .find(|candidate| {
        std::process::Command::new(candidate)
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success())
    })
}

#[test]
fn compilation_is_deterministic() {
    for program in [FIB, SIMD_SURFACE] {
        let first = native_compile(program, &["compute"]);
        let second = native_compile(program, &["compute"]);
        assert_eq!(first.plir_text(), second.plir_text());
        assert_eq!(
            first.llvm_ir(OptLevel::O0).expect("LLVM conversion"),
            second.llvm_ir(OptLevel::O0).expect("LLVM conversion"),
            "repeated builds must produce byte-identical LLVM IR"
        );
    }
}

/// Canonical text parses back and reprints byte-identically.
#[test]
fn canonical_text_round_trips() {
    // The first parse attaches `<in-memory>` locations to ops that carried
    // none, so byte stability is asserted from the first reparse onward
    // (the same policy the Stage 0 spike pinned; see docs/notes).
    for program in [FIB, SIMD_SURFACE] {
        round_trip_module(&native_compile(program, &["compute"]));
    }
}

fn round_trip_module(module: &NativeModule) {
    use pliron::irfmt::parsers::spaced;
    use pliron::operation::Operation;
    use pliron::parsable::parse_from_str;
    use pliron::printable::Printable;
    use pliron::result::ExpectOk;

    let ctx1 = &mut pliron::context::Context::new();
    let round1 = parse_from_str(
        spaced(Operation::top_level_parser()),
        ctx1,
        module.plir_text(),
    )
    .expect_ok(ctx1);
    pliron::operation::verify_operation(round1, ctx1).expect_ok(ctx1);
    pliron::builtin::given_names::erase_given_names(ctx1, round1);
    let round1_text = round1.disp(ctx1).to_string();

    let ctx2 = &mut pliron::context::Context::new();
    let round2 =
        parse_from_str(spaced(Operation::top_level_parser()), ctx2, &round1_text).expect_ok(ctx2);
    pliron::operation::verify_operation(round2, ctx2).expect_ok(ctx2);
    pliron::builtin::given_names::erase_given_names(ctx2, round2);
    let round2_text = round2.disp(ctx2).to_string();

    assert_eq!(
        round1_text, round2_text,
        "canonical text must be a parse -> print fixpoint from the first reparse"
    );
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
        entries: entries.iter().map(ToString::to_string).collect(),
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
            "from std.memory.alloc import unsafe_alloc\n\ndef compute() -> Int:\n    var p = unsafe_alloc[Int](2)\n    var q = p + 1\n    var d = q - 1\n    p.unsafe_free()\n    return 1\n",
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

/// Ordered lifecycle traces — destructor dispatches, consumes, raises,
/// catches — match between the VM's test-only event log and a
/// trace-instrumented native executable: the Stage 4 ordered
/// create/drop/error acceptance beyond stdout byte parity.
#[test]
fn lifecycle_event_traces_match_the_vm() {
    for fixture in [
        "assets/ok/exceptions.mojo",
        "assets/ok/deinit_param_destruction_timing.mojo",
        "assets/ok/deinit_body_field_last_use.mojo",
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
/// `UInt` operators, conversions, the div-by-zero and pow-exponent trap blocks,
/// the call into the bundled `_pow_int` body, and the shared `mjrt_trap`
/// scaffolding all print canonically.
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
        "llvm.func @mj__u_umodule_24std_24_24intrinsics_24_upow_uint",
        "llvm.call @mj__u_umodule_24std_24_24intrinsics_24_upow_uint",
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
        LLVMArrayType2, LLVMContextCreate, LLVMContextDispose, LLVMDoubleTypeInContext,
        LLVMFloatTypeInContext, LLVMInt1TypeInContext, LLVMInt8TypeInContext,
        LLVMInt16TypeInContext, LLVMInt32TypeInContext, LLVMInt64TypeInContext,
        LLVMPointerTypeInContext, LLVMStructTypeInContext,
    };
    use llvm_sys::prelude::{LLVMContextRef, LLVMTypeRef};
    use llvm_sys::target::{
        LLVMABIAlignmentOfType, LLVMABISizeOfType, LLVMCreateTargetData, LLVMDisposeTargetData,
        LLVMOffsetOfElement, LLVMTargetDataRef,
    };
    use mojito::Compiler;
    use mojito::ast::Dtype;
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
        fn new(triple: Triple) -> Self {
            let layout = CString::new(triple.data_layout()).expect("no NUL in data layout");
            unsafe {
                Self {
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
                    // Multi-lane SIMD storage is an *array* of lanes (lane
                    // alignment, byte-per-lane Bool), never an LLVM vector
                    // type: the backend computes in vectors but stores in
                    // this shape.
                    Ty::Simd { dtype, width } => {
                        let lane = match dtype {
                            Dtype::Bool if *width == 1 => LLVMInt1TypeInContext(self.ctx),
                            Dtype::Bool | Dtype::Int8 | Dtype::UInt8 => {
                                LLVMInt8TypeInContext(self.ctx)
                            }
                            Dtype::Int16 | Dtype::UInt16 => LLVMInt16TypeInContext(self.ctx),
                            Dtype::Int32 | Dtype::UInt32 => LLVMInt32TypeInContext(self.ctx),
                            Dtype::Float32 => LLVMFloatTypeInContext(self.ctx),
                            Dtype::Int | Dtype::Int64 | Dtype::UInt64 => {
                                LLVMInt64TypeInContext(self.ctx)
                            }
                            Dtype::Float64 => LLVMDoubleTypeInContext(self.ctx),
                        };
                        if *width == 1 {
                            lane
                        } else {
                            LLVMArrayType2(lane, *width as u64)
                        }
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
            (
                "simd_fields_lane_aligned",
                vec![
                    Ty::Bool,
                    Ty::Simd {
                        dtype: Dtype::Int32,
                        width: 4,
                    },
                    Ty::Bool,
                    Ty::Simd {
                        dtype: Dtype::Float64,
                        width: 2,
                    },
                    Ty::Simd {
                        dtype: Dtype::Bool,
                        width: 8,
                    },
                    Ty::Simd {
                        dtype: Dtype::UInt8,
                        width: 16,
                    },
                    Ty::Simd {
                        dtype: Dtype::Int16,
                        width: 1,
                    },
                ],
            ),
        ];
        for (label, fields) in aggregate_cases {
            let expected = cx.struct_layout(fields).expect("layout");
            let llvm_ty = td.ty(&structs, &Ty::Tuple(fields.clone()));
            td.assert_agrees(label, llvm_ty, &expected);
        }
        let scalar_cases = [
            Ty::Simd {
                dtype: Dtype::Int32,
                width: 4,
            },
            Ty::Simd {
                dtype: Dtype::Bool,
                width: 8,
            },
            Ty::Simd {
                dtype: Dtype::Float32,
                width: 16,
            },
            Ty::Simd {
                dtype: Dtype::Int,
                width: 2,
            },
            Ty::Simd {
                dtype: Dtype::Bool,
                width: 1,
            },
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
        expect![[r"
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
            declare i64 @mjrt_repr_string(ptr, i64, ptr)
            declare void @mjrt_trap(i32) noreturn
            declare void @mjrt_unhandled_error(ptr, i64) noreturn
            declare void @mjrt_abort(ptr, i64) noreturn
            declare void @mjrt_trace(i32, ptr, i64)
            declare void @mjrt_read_line(ptr)
        "]]
        .assert_eq(&mojito::backend::pliron::runtime_declarations());
    }

    /// The pinned data-layout string equals what the installed clang produces
    /// for the same triple, so `opt`, `clang`, and the layout engine agree.
    #[test]
    fn pliron_pinned_data_layout_matches_installed_clang() {
        let triple = Triple::X86_64UnknownLinuxGnu;
        let clang = ["clang-23", "clang"]
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
    /// runtime symbols outside the contract table — checked over both a
    /// scalar executable and a Stage 3 executable that prints, allocates, and
    /// carries constant-pool strings (whose `mjstr_*` globals must stay
    /// private). Inspection only — the executables never run.
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

            let nm = ["llvm-nm-23", "llvm-nm", "nm"]
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
