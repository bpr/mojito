//! The A1 measurement tool: one input, one mode, one phase per process, so
//! a wrapper can read each process's own wall time and peak memory.
//!
//! ```text
//! pliron_a1 --input PATH --mode baseline|shadow|shadow-opt
//!           --phase compile|artifact|execute --metrics PATH
//!           [--emit-v1 PATH] [--emit-core PATH] [--emit-bridge PATH]
//!           [--emit-specialized PATH]
//! pliron_a1 --summarize SAMPLES.tsv --shadow-mode shadow|shadow-opt
//! ```
//!
//! `compile` starts from source; `artifact` starts from a frozen v1 file
//! and times the boundary alone; `execute` also runs the result on the VM,
//! timed apart. A failure writes an error row and exits nonzero: an input
//! the shadow cannot cover never reports a fast baseline in its place.
//! `--emit-core` writes the canonical core text of a shadow run,
//! `--emit-bridge` the unverified bridge module as imported (and then
//! verifies each function on its own, naming it first), and
//! `--emit-specialized` the v1 text of the specialized input, for reading
//! a diagnostic against the text it names.

use std::fmt::Write as _;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use mojito::Compiler;
use mojito::backend::pliron::a1::measure::{self, Sample, ShadowRun};
use mojito::mir::MirProgram;
use sha2::Digest;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Baseline,
    Shadow,
    ShadowOpt,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Compile,
    Artifact,
    Execute,
}

struct Options {
    input: PathBuf,
    mode: Mode,
    phase: Phase,
    metrics: PathBuf,
    emit_v1: Option<PathBuf>,
    emit_core: Option<PathBuf>,
    emit_bridge: Option<PathBuf>,
    emit_specialized: Option<PathBuf>,
}

/// What one run measured, as the fields of its metrics row.
#[derive(Default)]
struct Metrics {
    phases: Vec<(&'static str, u128)>,
    counts: Vec<(&'static str, usize)>,
    v1_bytes: usize,
    core_bytes: usize,
    output_sha256: String,
}

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let outcome = match value_of(&arguments, "--summarize") {
        Some(samples) => summarize(Path::new(&samples), &arguments),
        None => parse(&arguments).and_then(|options| run(&options)),
    };
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("pliron_a1: {message}");
            ExitCode::FAILURE
        }
    }
}

fn value_of(arguments: &[String], flag: &str) -> Option<String> {
    arguments
        .iter()
        .position(|argument| argument == flag)
        .and_then(|position| arguments.get(position + 1))
        .cloned()
}

fn parse(arguments: &[String]) -> Result<Options, String> {
    let required =
        |flag: &str| value_of(arguments, flag).ok_or_else(|| format!("{flag} is required"));
    let mode = match required("--mode")?.as_str() {
        "baseline" => Mode::Baseline,
        "shadow" => Mode::Shadow,
        "shadow-opt" => Mode::ShadowOpt,
        other => return Err(format!("unknown mode `{other}`")),
    };
    let phase = match required("--phase")?.as_str() {
        "compile" => Phase::Compile,
        "artifact" => Phase::Artifact,
        "execute" => Phase::Execute,
        other => return Err(format!("unknown phase `{other}`")),
    };
    Ok(Options {
        input: required("--input")?.into(),
        mode,
        phase,
        metrics: required("--metrics")?.into(),
        emit_v1: value_of(arguments, "--emit-v1").map(PathBuf::from),
        emit_core: value_of(arguments, "--emit-core").map(PathBuf::from),
        emit_bridge: value_of(arguments, "--emit-bridge").map(PathBuf::from),
        emit_specialized: value_of(arguments, "--emit-specialized").map(PathBuf::from),
    })
}

fn run(options: &Options) -> Result<(), String> {
    let input = std::fs::read(&options.input)
        .map_err(|error| format!("{}: {error}", options.input.display()))?;
    let measured = measure_input(options);
    let row = metrics_row(options, &sha256(&input), &measured);
    std::fs::write(&options.metrics, row)
        .map_err(|error| format!("{}: {error}", options.metrics.display()))?;
    measured.map(drop)
}

fn measure_input(options: &Options) -> Result<Metrics, String> {
    let mut metrics = Metrics::default();
    let started = Instant::now();
    let program = match options.phase {
        Phase::Artifact => {
            let text = std::fs::read(&options.input).map_err(|error| error.to_string())?;
            mojito::mir::text::load_artifact(&text, options.input.display().to_string())
                .map_err(|report| format!("{report:?}"))?
                .program
        }
        Phase::Compile | Phase::Execute => Compiler::default()
            .compile_path(&options.input)
            .map_err(|error| error.to_string())?
            .elaborated_mir()
            .clone(),
    };
    metrics
        .phases
        .push(("front_end", started.elapsed().as_nanos()));

    let result = match options.mode {
        Mode::Baseline => program,
        Mode::Shadow | Mode::ShadowOpt => {
            let entries = entries(&program);
            if let Some(path) = &options.emit_specialized {
                emit_specialized(&program, &entries, path)?;
            }
            if let Some(path) = &options.emit_bridge {
                let text =
                    measure::bridge_print(&program, &entries).map_err(|error| error.to_string())?;
                std::fs::write(path, text)
                    .map_err(|error| format!("{}: {error}", path.display()))?;
                let normalized_path = path.with_extension("normalized.txt");
                let verdicts = measure::verify_each_function(
                    &program,
                    &entries,
                    |symbol| eprintln!("verifying {symbol}"),
                    |normalized| {
                        if let Err(error) = std::fs::write(&normalized_path, normalized) {
                            eprintln!("{}: {error}", normalized_path.display());
                        }
                    },
                )
                .map_err(|error| error.to_string())?;
                for (symbol, verdict) in verdicts {
                    if let Err(error) = verdict {
                        eprintln!("`{symbol}`: {error}");
                    }
                }
            }
            let shadow = measure::shadow(&program, &entries, options.mode == Mode::ShadowOpt);
            if let Some(path) = &options.emit_core {
                let text = match &shadow {
                    Ok(shadow) => Ok(shadow.core_text.clone()),
                    Err(_) => measure::first_print(&program, &entries),
                };
                if let Ok(text) = text {
                    std::fs::write(path, text)
                        .map_err(|error| format!("{}: {error}", path.display()))?;
                }
            }
            let shadow =
                shadow.map_err(|error| format!("{error}{}", uncovered(&program, &entries)))?;
            drop(program);
            record(&mut metrics, shadow)
        }
    };
    let text = mojito::mir::text::disassemble(&result).map_err(|error| format!("{error:?}"))?;
    metrics.v1_bytes = text.len();
    metrics.output_sha256 = sha256(text.as_bytes());
    black_box((result.functions.len(), &metrics.output_sha256));
    if let Some(path) = &options.emit_v1 {
        std::fs::write(path, &text).map_err(|error| format!("{}: {error}", path.display()))?;
    }
    if options.phase == Phase::Execute {
        let started = Instant::now();
        let mut vm = mojito::backend::VmBackend::new();
        vm.run_elaborated(result)
            .map_err(|error| error.to_string())?;
        black_box(vm.output().len());
        metrics
            .phases
            .push(("execute", started.elapsed().as_nanos()));
    }
    Ok(metrics)
}

/// The entries the native backend would compile: `main`, and module
/// initialization when the program has any.
fn entries(program: &MirProgram) -> Vec<String> {
    ["main", "__toplevel__"]
        .into_iter()
        .filter(|entry| program.functions.iter().any(|(name, _)| name == entry))
        .map(str::to_string)
        .collect()
}

/// The v1 text of the specialized closure, written to `path`.
fn emit_specialized(program: &MirProgram, entries: &[String], path: &Path) -> Result<(), String> {
    let specialized = mojito::native::mono::specialize(program, entries)
        .map_err(|error| format!("specialize: {error:?}"))?;
    let text = mojito::mir::text::disassemble(&specialized.program)
        .map_err(|error| format!("{error:?}"))?;
    std::fs::write(path, text).map_err(|error| format!("{}: {error}", path.display()))
}

/// Every refusal the importer has for the specialized closure, counted,
/// so a refused input says how far outside the inventory it is.
fn uncovered(program: &MirProgram, entries: &[String]) -> String {
    let Ok(specialized) = mojito::native::mono::specialize(program, entries) else {
        return String::new();
    };
    let refusals = mojito::backend::pliron::a1::import::refusals(&specialized.program);
    let listed: Vec<String> = refusals
        .iter()
        .map(|(what, count)| format!("{what} x{count}"))
        .collect();
    format!("; refusals: {}", listed.join(", "))
}

fn record(metrics: &mut Metrics, shadow: ShadowRun) -> MirProgram {
    metrics.phases.extend(shadow.phases);
    metrics.counts = vec![
        ("functions", shadow.functions),
        ("blocks", shadow.blocks),
        ("operations", shadow.operations),
        ("attributes", shadow.attributes),
        ("removed", shadow.removed),
    ];
    metrics.core_bytes = shadow.core_text.len();
    black_box(sha256(shadow.core_text.as_bytes()));
    shadow.exported.program
}

fn metrics_row(
    options: &Options,
    input_sha256: &str,
    measured: &Result<Metrics, String>,
) -> String {
    let mode = match options.mode {
        Mode::Baseline => "baseline",
        Mode::Shadow => "shadow",
        Mode::ShadowOpt => "shadow-opt",
    };
    let phase = match options.phase {
        Phase::Compile => "compile",
        Phase::Artifact => "artifact",
        Phase::Execute => "execute",
    };
    let mut row = format!(
        "{{\"input\":{},\"input_sha256\":\"{input_sha256}\",\"mode\":\"{mode}\",\"phase\":\"{phase}\"",
        quoted(&options.input.display().to_string())
    );
    match measured {
        Ok(metrics) => {
            let _ = write!(row, ",\"status\":\"ok\",\"phases_ns\":{{");
            for (index, (name, nanoseconds)) in metrics.phases.iter().enumerate() {
                let comma = if index == 0 { "" } else { "," };
                let _ = write!(row, "{comma}\"{name}\":{nanoseconds}");
            }
            let _ = write!(row, "}},\"counts\":{{");
            for (index, (name, count)) in metrics.counts.iter().enumerate() {
                let comma = if index == 0 { "" } else { "," };
                let _ = write!(row, "{comma}\"{name}\":{count}");
            }
            let _ = write!(
                row,
                "}},\"v1_bytes\":{},\"core_bytes\":{},\"output_sha256\":\"{}\"",
                metrics.v1_bytes, metrics.core_bytes, metrics.output_sha256
            );
        }
        Err(error) => {
            let _ = write!(row, ",\"status\":\"error\",\"error\":{}", quoted(error));
        }
    }
    row.push_str("}\n");
    row
}

fn quoted(text: &str) -> String {
    let mut out = String::from("\"");
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            control if control.is_control() => {
                let _ = write!(out, "\\u{:04x}", control as u32);
            }
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

fn sha256(bytes: &[u8]) -> String {
    sha2::Sha256::digest(bytes)
        .iter()
        .fold(String::new(), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

/// Summarize raw samples: tab-separated `input`, `mode`, `run`,
/// `wall_ns`, `peak_kib`, `status`, under one header line.
fn summarize(samples: &Path, arguments: &[String]) -> Result<(), String> {
    let shadow_mode = value_of(arguments, "--shadow-mode").unwrap_or_else(|| "shadow".to_string());
    let text = std::fs::read_to_string(samples)
        .map_err(|error| format!("{}: {error}", samples.display()))?;
    let parsed = text
        .lines()
        .skip(1)
        .filter(|line| !line.is_empty())
        .map(sample)
        .collect::<Result<Vec<_>, _>>()?;
    print!(
        "{}",
        measure::summary_tsv(&measure::summarize(&parsed, &shadow_mode))
    );
    Ok(())
}

fn sample(line: &str) -> Result<Sample, String> {
    let fields: Vec<&str> = line.split('\t').collect();
    let [input, mode, run, wall_ns, peak_kib, status] = fields.as_slice() else {
        return Err(format!("a sample has six fields: `{line}`"));
    };
    let number = |field: &str| {
        field
            .parse::<f64>()
            .map_err(|error| format!("`{field}`: {error}"))
    };
    Ok(Sample {
        input: (*input).to_string(),
        mode: (*mode).to_string(),
        run: run.parse().map_err(|error| format!("`{run}`: {error}"))?,
        wall_ns: number(wall_ns)?,
        peak_kib: number(peak_kib)?,
        ok: *status == "ok",
    })
}
