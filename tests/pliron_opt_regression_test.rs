//! Optimization regression fixtures (Stage 6, S6.2.5): for each semantic
//! area the release pipeline could plausibly disturb — wrapping overflow,
//! runtime traps, references, tagged outcomes, `finally`, destructor order,
//! Variant payloads, SIMD lanes, aliasing collections, and runtime calls —
//! compare observable behavior across the VM (the oracle), `O0`, and
//! `release`. The full parity manifest covers the whole corpus; these
//! fixtures stay small, named, and runnable in seconds so an optimization
//! change gets a targeted verdict first.

#![cfg(feature = "backend-pliron")]

use std::path::Path;

use mojito::Compiler;
use mojito::backend::pliron as native;
use native::{CompileOptions, DebugInfo, NativeTarget, OptLevel};

const FIXTURE_NAME: &str = "pliron_opt_regression.mojo";

fn host_target() -> NativeTarget {
    NativeTarget::host().expect("pliron tests require a supported host target")
}

/// Compile `src` once, run it on the VM, then build and run executables at
/// both profiles; stdout must be byte-identical everywhere with exit 0.
fn assert_vm_o0_release_agree(name: &str, src: &str) {
    let compiler = Compiler::default();
    let compiled = compiler
        .compile_source(src, Path::new(FIXTURE_NAME))
        .unwrap_or_else(|error| panic!("{name}: must compile: {error}"));
    let execution = compiler
        .execute(&compiled)
        .unwrap_or_else(|error| panic!("{name}: must run on the VM: {error}"));
    let options = CompileOptions {
        entries: vec!["main".to_string()],
        sources: vec![(FIXTURE_NAME.to_string(), src.to_string())],
        target: host_target(),
        trace_lifecycle: false,
    };
    let mut module = native::compile(compiled.elaborated_mir(), &options)
        .unwrap_or_else(|error| panic!("{}", error.display_with_sources(&options.sources)));
    let dir = tempfile::tempdir().expect("tempdir");
    for (level, opt) in [("O0", OptLevel::O0), ("release", OptLevel::Release)] {
        let exe = dir.path().join(format!("{name}-{level}"));
        module
            .write_executable(&exe, opt, DebugInfo::Lines)
            .unwrap_or_else(|error| panic!("{name}: exe emission at {level}: {error}"));
        let run = std::process::Command::new(&exe)
            .output()
            .expect("regression executable runs");
        assert_eq!(
            run.status.code(),
            Some(0),
            "{name}: exit at {level}; stderr: {}",
            String::from_utf8_lossy(&run.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&run.stdout),
            execution.output,
            "{name}: stdout parity at {level}"
        );
    }
}

/// Compile `src`, confirm the VM rejects it at runtime, and confirm both
/// profiles produce the same nonzero exit code and identical stderr — a
/// release pipeline must not change which trap fires or how it reports.
fn assert_trap_parity(name: &str, src: &str) {
    let compiler = Compiler::default();
    let compiled = compiler
        .compile_source(src, Path::new(FIXTURE_NAME))
        .unwrap_or_else(|error| panic!("{name}: must compile: {error}"));
    compiler
        .execute(&compiled)
        .err()
        .unwrap_or_else(|| panic!("{name}: the VM must reject this fixture at runtime"));
    let options = CompileOptions {
        entries: vec!["main".to_string()],
        sources: vec![(FIXTURE_NAME.to_string(), src.to_string())],
        target: host_target(),
        trace_lifecycle: false,
    };
    let mut module = native::compile(compiled.elaborated_mir(), &options)
        .unwrap_or_else(|error| panic!("{}", error.display_with_sources(&options.sources)));
    let dir = tempfile::tempdir().expect("tempdir");
    let mut outcomes = Vec::new();
    for (level, opt) in [("O0", OptLevel::O0), ("release", OptLevel::Release)] {
        let exe = dir.path().join(format!("{name}-{level}"));
        module
            .write_executable(&exe, opt, DebugInfo::Lines)
            .unwrap_or_else(|error| panic!("{name}: exe emission at {level}: {error}"));
        let run = std::process::Command::new(&exe)
            .output()
            .expect("trap executable runs");
        let code = run.status.code().expect("trap exits, not signals");
        assert_ne!(code, 0, "{name}: must trap at {level}");
        outcomes.push((code, String::from_utf8_lossy(&run.stderr).into_owned()));
    }
    assert_eq!(
        outcomes[0], outcomes[1],
        "{name}: O0 and release must trap identically"
    );
}

#[test]
fn pliron_opt_preserves_wrapping_overflow() {
    assert_vm_o0_release_agree(
        "overflow",
        "\
def main():
    var big: Int = 9223372036854775807
    print(big + 1)
    print(big * 3)
    var small: Int = -9223372036854775807 - 1
    print(small - 1)
    print(small // -1)
    var u: UInt = 0
    var one: UInt = 1
    print(u - one)
",
    );
}

#[test]
fn pliron_opt_preserves_division_trap() {
    assert_trap_parity(
        "div-trap",
        "\
def main():
    var d: Int = 0
    print(10 // d)
",
    );
}

#[test]
fn pliron_opt_preserves_reference_writes() {
    assert_vm_o0_release_agree(
        "references",
        "\
struct Box(Copyable, Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

def bump(mut b: Box, d: Int):
    b.v += d

def main():
    var b = Box(5)
    bump(b, 37)
    var xs: List[Int] = [1, 2, 3]
    xs[1] += 40
    print(b.v)
    print(xs[0] + xs[1] + xs[2])
",
    );
}

#[test]
fn pliron_opt_preserves_tagged_outcomes() {
    assert_vm_o0_release_agree(
        "tagged-outcomes",
        "\
def may(n: Int) raises -> Int:
    if n > 2:
        raise \"too big\"
    return n * 10

def main():
    try:
        print(may(1))
        print(may(5))
    except e:
        print(\"caught\")
    try:
        print(may(2))
    except e2:
        print(\"nope\")
",
    );
}

#[test]
fn pliron_opt_preserves_finally_ordering() {
    assert_vm_o0_release_agree(
        "finally",
        "\
def f(n: Int) raises -> Int:
    try:
        if n == 1:
            raise \"one\"
        return 10
    finally:
        print(\"fin\", n)

def main():
    try:
        print(f(0))
        print(f(1))
    except e:
        print(\"caught\")
",
    );
}

#[test]
fn pliron_opt_preserves_destructor_order() {
    assert_vm_o0_release_agree(
        "destructor-order",
        "\
struct D(Copyable, Movable):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

    def __deinit__(deinit self):
        print(\"drop\", self.id)

def consume(d: D) -> Int:
    return d.id * 2

def main():
    var a = D(1)
    var b = D(2)
    print(consume(D(3)))
    print(\"after\", a.id + b.id)
",
    );
}

#[test]
fn pliron_opt_preserves_variant_payloads() {
    assert_vm_o0_release_agree(
        "variant",
        "\
from std.utils import Variant

def main():
    var v: Variant[Int, String] = Variant[Int, String](5)
    print(\"isa Int\", v.isa[Int]())
    print(v.unwrap[Int]())
    var w: Variant[Int, String] = Variant[Int, String](String(\"hey\"))
    if w.isa[String]():
        print(w.unwrap[String]())
",
    );
}

#[test]
fn pliron_opt_preserves_simd_lane_semantics() {
    assert_vm_o0_release_agree(
        "simd-lanes",
        "\
def main():
    var v = SIMD[DType.int64, 4](1, 2, 3, 4)
    var w = v * v + v
    print(w[0] + w[1] + w[2] + w[3])
    var f = SIMD[DType.float64, 4](1.5, 2.5, 3.5, 4.5)
    print((f + f).reduce_add())
",
    );
}

#[test]
fn pliron_opt_preserves_aliasing_collections() {
    assert_vm_o0_release_agree(
        "aliasing-collections",
        "\
def main():
    var xs: List[Int] = [1, 2, 3]
    var total: Int = 0
    for i in range(len(xs)):
        xs[i] = xs[i] * 2
        total += xs[i]
    xs.append(9)
    var last = xs.pop()
    print(total, last, len(xs))
",
    );
}

#[test]
fn pliron_opt_preserves_runtime_calls() {
    assert_vm_o0_release_agree(
        "runtime-calls",
        "\
def main():
    var s = String(\"hello\")
    s += \" world\"
    print(len(s))
    print(s.find(\"wor\"))
    print(s)
",
    );
}

/// The instrumented verification lane (S6.2.2): with
/// `MOJITO_PLIRON_VERIFY_EACH_PASS` set, the pliron pass manager verifies
/// around every individual pass, not just the whole pipeline. Compiling a
/// call-and-branch-heavy fixture under it proves every intermediate state
/// verifies. nextest runs each test in its own process, so the env var
/// cannot leak.
#[test]
fn pliron_per_pass_verification_holds_across_the_pipeline() {
    unsafe { std::env::set_var("MOJITO_PLIRON_VERIFY_EACH_PASS", "1") };
    assert_vm_o0_release_agree(
        "per-pass-verify",
        "\
def fib(n: Int) -> Int:
    if n < 2:
        return n
    return fib(n - 1) + fib(n - 2)

def main():
    var acc: Int = 0
    for i in range(12):
        acc += fib(i)
    print(acc)
",
    );
}
