//! Helpers shared by the Pliron test targets: the host target, the runtime
//! JIT symbol mapping, fixture enumeration, and the corpus-sweep worker pool
//! with its memory budget.
//!
//! Included with `#[path]` by `tests/pliron_backend_test.rs` and
//! `tests/heavy/main.rs`; it is not a test target of its own (Cargo
//! discovers only `tests/*.rs` and `tests/*/main.rs`).

// Each including target uses a different subset: the module is compiled once
// per target, so the rest is dead code there.
#![allow(dead_code, reason = "shared helpers; each target uses a subset")]

use std::path::Path;

use mojito::backend::pliron as native;
use native::{JitValue, NativeTarget};

/// The host as a native target; every Pliron test compiles for (and JITs or
/// runs on) the host.
pub fn host_target() -> NativeTarget {
    NativeTarget::host().expect("pliron tests require a supported host target")
}

/// The linked `mojito-runtime` exports as an explicit JIT symbol mapping, so
/// a JIT'd module referencing runtime-contract functions (`mjrt_trap` from
/// trap guards) resolves them deterministically instead of relying on
/// process-symbol resolution. Every function in `rt_abi::RT_SYMBOLS` is
/// mapped: a module reaches the JIT already lowered, so a symbol missing
/// here surfaces as an opaque `Symbols not found` materialization failure on
/// whichever fixture first calls it. `runtime_jit_symbols_cover_the_contract`
/// keeps the two lists in step.
pub fn runtime_jit_symbols() -> Vec<(&'static str, u64)> {
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
        address!(mjrt_free),
        address!(mjrt_pointer_status),
        address!(mjrt_dealloc),
        address!(mjrt_write_stdout),
        address!(mjrt_fmt_i64),
        address!(mjrt_fmt_u64),
        address!(mjrt_fmt_f64),
        address!(mjrt_repr_string),
        address!(mjrt_trap),
        address!(mjrt_unhandled_error),
        address!(mjrt_abort),
        address!(mjrt_trace),
        address!(mjrt_read_line),
    ]
}

/// The `(relative path, source)` of every `.mojo` fixture in `dir`, sorted.
pub fn fixture_sources(dir: &str) -> Vec<(String, String)> {
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
            std::path::Path::new(name)
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("mojo"))
                .then(|| name.to_string())
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
pub fn has_compute_entry(src: &str) -> bool {
    src.lines().any(|line| line.starts_with("def compute() ->"))
}

/// How many fixtures a corpus sweep compiles at once.
///
/// A sweep worker holds a whole `Compiler` — the bundled standard library,
/// parsed and checked — plus an LLVM module and a linker, which is around
/// 300 MB in a debug build. The fan-out is therefore a memory budget, not a
/// core count: taking every core regardless of free memory is what the OOM
/// killer ended on the 2026-09-12 gate run, where several sweeps ran beside
/// each other as sibling nextest processes. Set
/// `MOJITO_TEST_COMPILE_JOBS` to pin the fan-out instead, on a machine whose
/// free memory is about to change under the sweep.
pub fn compile_jobs() -> usize {
    /// Resident set a single sweep worker is assumed to need.
    const WORKER_MIB: u64 = 400;
    /// Headroom left to the rest of the machine. Without it a sweep sizes
    /// itself to consume every free page, which is not the same as fitting.
    const RESERVE_MIB: u64 = 1024;

    if let Some(jobs) = std::env::var("MOJITO_TEST_COMPILE_JOBS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|jobs| *jobs > 0)
    {
        return jobs;
    }
    let cores = std::thread::available_parallelism().map_or(4, std::num::NonZero::get);
    let Some(available) = available_mib() else {
        return cores;
    };
    let budget = available.saturating_sub(RESERVE_MIB) / WORKER_MIB;
    usize::try_from(budget).map_or(cores, |budget| budget.clamp(1, cores))
}

/// `MemAvailable` from `/proc/meminfo`, in MiB (`None` off Linux).
fn available_mib() -> Option<u64> {
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    let field = meminfo
        .lines()
        .find_map(|line| line.strip_prefix("MemAvailable:"))?;
    field
        .split_whitespace()
        .next()?
        .parse::<u64>()
        .ok()
        .map(|kib| kib / 1024)
}

/// Run `work` over `items` in [`compile_jobs`] worker threads, preserving
/// item order in the results. A worker panic propagates when the scope
/// joins, failing the test. (The per-fixture production compile dominates
/// the manifest pass; fixtures are independent, and the JIT's global target
/// initialization is Once-guarded.)
pub fn parallel_map<T: Send, R: Send>(items: Vec<T>, work: impl Fn(T) -> R + Sync) -> Vec<R> {
    let workers = compile_jobs().min(items.len().max(1));
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
pub const fn ret_kind_name(value: JitValue) -> &'static str {
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
pub fn assert_jit_matches(fixture: &str, level: &str, native: JitValue, printed: &str) {
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

/// Stdin bytes for fixtures that call `input()`. The same bytes feed the
/// in-process VM (via `VmBackend::set_input_override`) and every native
/// executable's piped stdin, so `input()` rows are true exe differentials.
/// Doubles as the "reads stdin" predicate: a hit must never run with
/// inherited stdin or the manifest test blocks under Cargo.
pub fn fixture_stdin(rel: &str) -> Option<&'static [u8]> {
    match rel.rsplit('/').next()? {
        "input.mojo" => Some(b"World\n"),
        "pliron_input_echo.mojo" => Some(b"echoed line\n"),
        _ => None,
    }
}

/// Run a parity executable, piping `stdin` bytes when present (an absent
/// entry inherits the test runner's stdin, which never blocks because such
/// fixtures don't read it).
pub fn run_executable(
    exe: &Path,
    stdin: Option<&[u8]>,
    envs: &[(&str, &str)],
) -> std::process::Output {
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
