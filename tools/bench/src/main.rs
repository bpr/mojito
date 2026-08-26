//! Native-benchmark driver for the Pliron backend (Stage 6, S6.1).
//!
//! Drives the feature-built `mojito` binary as a subprocess over the corpus
//! in `benchmarks/native/`: per fixture and profile it measures compile wall
//! time, per-phase compile timings (the CLI's `--timings` channel), peak
//! RSS (`wait4`/`ru_maxrss`), artifact size, executable runtime, and the VM
//! baseline. Raw samples stream as JSON Lines (one leading runner-metadata
//! record); the summary is a `median`/`MAD` TSV. `--check` compares a
//! summary against a committed baseline under the thresholds documented in
//! `benchmarks/native/noise-policy.md` and exits nonzero on regression.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

fn main() {
    let config = match Config::parse(std::env::args().skip(1)) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("mojito-bench: {error}");
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    };
    match run(&config) {
        Ok(true) => {}
        Ok(false) => std::process::exit(1),
        Err(error) => {
            eprintln!("mojito-bench: {error}");
            std::process::exit(2);
        }
    }
}

const USAGE: &str = "\
usage: mojito-bench --mojito PATH [options]
  --mojito PATH        the feature-built mojito binary (required)
  --root DIR           corpus root (default: benchmarks/native)
  --profiles LIST      comma-separated --native-opt profiles (default: 0,release)
  --samples N          compile samples per fixture/profile (default: 5)
  --exec-samples N     executable/VM run samples (default: 10)
  --warmups N          unrecorded warmup runs per lane (default: 2)
  --raw PATH           write raw samples as JSON Lines
  --summary PATH       write the median/MAD summary TSV
  --check BASELINE     compare the summary against a baseline TSV; exit 1 on
                       regression (thresholds: benchmarks/native/noise-policy.md)
  --smoke              machinery-validation subset: startup + arith_branch,
                       minimal samples; makes no performance claims
  --no-vm              skip the VM baseline lane";

struct Config {
    mojito: PathBuf,
    root: PathBuf,
    profiles: Vec<String>,
    samples: u32,
    exec_samples: u32,
    warmups: u32,
    raw: Option<PathBuf>,
    summary: Option<PathBuf>,
    check: Option<PathBuf>,
    smoke: bool,
    vm: bool,
}

impl Config {
    fn parse(args: impl Iterator<Item = String>) -> Result<Config, String> {
        let mut mojito = None;
        let mut root = PathBuf::from("benchmarks/native");
        let mut profiles = vec!["0".to_string(), "release".to_string()];
        let mut samples = 5;
        let mut exec_samples = 10;
        let mut warmups = 2;
        let mut raw = None;
        let mut summary = None;
        let mut check = None;
        let mut smoke = false;
        let mut vm = true;
        let mut iter = args;
        while let Some(arg) = iter.next() {
            let mut value = |name: &str| {
                iter.next()
                    .ok_or_else(|| format!("{name} requires a value"))
            };
            match arg.as_str() {
                "--mojito" => mojito = Some(PathBuf::from(value("--mojito")?)),
                "--root" => root = PathBuf::from(value("--root")?),
                "--profiles" => {
                    profiles = value("--profiles")?
                        .split(',')
                        .map(str::to_string)
                        .collect();
                }
                "--samples" => samples = parse_count(&value("--samples")?)?,
                "--exec-samples" => exec_samples = parse_count(&value("--exec-samples")?)?,
                "--warmups" => {
                    warmups = value("--warmups")?
                        .parse()
                        .map_err(|_| "--warmups expects a number".to_string())?;
                }
                "--raw" => raw = Some(PathBuf::from(value("--raw")?)),
                "--summary" => summary = Some(PathBuf::from(value("--summary")?)),
                "--check" => check = Some(PathBuf::from(value("--check")?)),
                "--smoke" => smoke = true,
                "--no-vm" => vm = false,
                other => return Err(format!("unknown option '{other}'")),
            }
        }
        if smoke {
            // Machinery validation only: minimal samples, no VM lane (the
            // debug-build VM's fixed startup floor would dominate the gate).
            samples = 1;
            exec_samples = 2;
            warmups = 0;
            vm = false;
        }
        Ok(Config {
            mojito: mojito.ok_or("--mojito PATH is required")?,
            root,
            profiles,
            samples,
            exec_samples,
            warmups,
            raw,
            summary,
            check,
            smoke,
            vm,
        })
    }
}

fn parse_count(text: &str) -> Result<u32, String> {
    let n: u32 = text.parse().map_err(|_| "expected a number".to_string())?;
    if n == 0 {
        return Err("sample counts must be positive".to_string());
    }
    Ok(n)
}

/// One fixture from `manifest.tsv`.
struct Fixture {
    file: String,
    category: String,
}

/// One measured subprocess run.
struct Sample {
    wall_us: u64,
    maxrss_kb: u64,
    exit_ok: bool,
    stderr: String,
}

/// Key for aggregation: fixture, profile ("-" for the VM lane), metric.
type MetricKey = (String, String, String);

fn run(config: &Config) -> Result<bool, String> {
    let fixtures = load_manifest(&config.root)?;
    let fixtures: Vec<&Fixture> = if config.smoke {
        fixtures
            .iter()
            .filter(|f| f.file.starts_with("startup") || f.file.starts_with("arith_branch"))
            .collect()
    } else {
        fixtures.iter().collect()
    };
    if fixtures.is_empty() {
        return Err("no fixtures selected".to_string());
    }

    let mut raw_lines = vec![runner_metadata(config)];
    let mut metrics: BTreeMap<MetricKey, Vec<f64>> = BTreeMap::new();
    let temp = TempDir::create()?;

    for fixture in &fixtures {
        let source = config.root.join("src").join(&fixture.file);
        let source = source
            .to_str()
            .ok_or_else(|| format!("non-UTF-8 fixture path {}", source.display()))?
            .to_string();
        for profile in &config.profiles {
            let exe = temp.path.join(format!(
                "{}-{profile}",
                fixture.file.trim_end_matches(".mojo")
            ));
            for _ in 0..config.warmups {
                compile_once(config, &source, profile, &exe)?;
            }
            for sample_index in 0..config.samples {
                let sample = compile_once(config, &source, profile, &exe)?;
                if !sample.exit_ok {
                    return Err(format!(
                        "{}: compile at {profile} failed:\n{}",
                        fixture.file, sample.stderr
                    ));
                }
                let phases = parse_timings(&sample.stderr);
                record(
                    &mut metrics,
                    &fixture.file,
                    profile,
                    "compile_wall_us",
                    sample.wall_us as f64,
                );
                record(
                    &mut metrics,
                    &fixture.file,
                    profile,
                    "compile_maxrss_kb",
                    sample.maxrss_kb as f64,
                );
                for (phase, micros) in &phases {
                    record(
                        &mut metrics,
                        &fixture.file,
                        profile,
                        &format!("phase:{phase}_us"),
                        *micros as f64,
                    );
                }
                raw_lines.push(raw_record(
                    "compile",
                    &fixture.file,
                    profile,
                    sample_index,
                    &sample,
                    &phases,
                    None,
                ));
            }
            let exe_bytes = std::fs::metadata(&exe)
                .map_err(|error| format!("{}: missing built exe: {error}", fixture.file))?
                .len();
            record(
                &mut metrics,
                &fixture.file,
                profile,
                "exe_bytes",
                exe_bytes as f64,
            );
            for _ in 0..config.warmups {
                run_measured(Command::new(&exe))?;
            }
            for sample_index in 0..config.exec_samples {
                let sample = run_measured(Command::new(&exe))?;
                if !sample.exit_ok {
                    return Err(format!(
                        "{}: exe at {profile} exited nonzero:\n{}",
                        fixture.file, sample.stderr
                    ));
                }
                record(
                    &mut metrics,
                    &fixture.file,
                    profile,
                    "exec_wall_us",
                    sample.wall_us as f64,
                );
                record(
                    &mut metrics,
                    &fixture.file,
                    profile,
                    "exec_maxrss_kb",
                    sample.maxrss_kb as f64,
                );
                raw_lines.push(raw_record(
                    "exec",
                    &fixture.file,
                    profile,
                    sample_index,
                    &sample,
                    &[],
                    Some(exe_bytes),
                ));
            }
        }
        if config.vm {
            for _ in 0..config.warmups.min(1) {
                vm_once(config, &source)?;
            }
            for sample_index in 0..config.exec_samples {
                let sample = vm_once(config, &source)?;
                if !sample.exit_ok {
                    return Err(format!(
                        "{}: VM run failed:\n{}",
                        fixture.file, sample.stderr
                    ));
                }
                record(
                    &mut metrics,
                    &fixture.file,
                    "-",
                    "vm_wall_us",
                    sample.wall_us as f64,
                );
                raw_lines.push(raw_record(
                    "vm",
                    &fixture.file,
                    "-",
                    sample_index,
                    &sample,
                    &[],
                    None,
                ));
            }
        }
        eprintln!("measured {} ({})", fixture.file, fixture.category);
    }

    if let Some(path) = &config.raw {
        std::fs::write(path, raw_lines.join("\n") + "\n")
            .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    }
    let summary = summarize(&metrics);
    match &config.summary {
        Some(path) => std::fs::write(path, &summary)
            .map_err(|error| format!("cannot write {}: {error}", path.display()))?,
        None => print!("{summary}"),
    }
    match &config.check {
        Some(baseline) => check_against(baseline, &summary),
        None => Ok(true),
    }
}

/// The manifest: `fixture <TAB> category`, `#` comments, schema-version 1.
fn load_manifest(root: &Path) -> Result<Vec<Fixture>, String> {
    let path = root.join("manifest.tsv");
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let mut fixtures = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut cells = line.split('\t');
        let file = cells.next().unwrap_or_default().to_string();
        let category = cells
            .next()
            .ok_or_else(|| format!("manifest row without category: {line}"))?
            .to_string();
        fixtures.push(Fixture { file, category });
    }
    Ok(fixtures)
}

fn compile_once(
    config: &Config,
    source: &str,
    profile: &str,
    exe: &Path,
) -> Result<Sample, String> {
    let mut command = Command::new(&config.mojito);
    command
        .args([
            "compile",
            "--backend",
            "pliron",
            "--emit",
            "exe",
            "--native-opt",
            profile,
            "--timings",
            "-o",
        ])
        .arg(exe)
        .arg(source);
    run_measured(command)
}

fn vm_once(config: &Config, source: &str) -> Result<Sample, String> {
    let mut command = Command::new(&config.mojito);
    command.args(["run", source]);
    run_measured(command)
}

/// Spawn, drain output, and reap through `wait4` so `ru_maxrss` (KiB on
/// Linux) rides along with the exit status. stderr drains on its own thread
/// while stdout drains here, so a child filling either pipe (a large compile
/// diagnostic easily exceeds pipe capacity) can never deadlock the drain.
fn run_measured(mut command: Command) -> Result<Sample, String> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let start = Instant::now();
    let mut child = command
        .spawn()
        .map_err(|error| format!("cannot spawn {:?}: {error}", command.get_program()))?;
    let mut stderr_pipe = child.stderr.take().expect("piped stderr");
    let stderr_reader = std::thread::spawn(move || {
        let mut stderr = String::new();
        stderr_pipe
            .read_to_string(&mut stderr)
            .map(|_| stderr)
            .map_err(|error| format!("reading child stderr: {error}"))
    });
    // stdout must still be drained (the child blocks on a full pipe), but
    // only its completion matters to the measurements.
    let mut stdout = String::new();
    child
        .stdout
        .take()
        .expect("piped stdout")
        .read_to_string(&mut stdout)
        .map_err(|error| format!("reading child stdout: {error}"))?;
    drop(stdout);
    let stderr = stderr_reader
        .join()
        .map_err(|_| "child stderr reader panicked".to_string())??;
    let pid = child.id() as libc::pid_t;
    let mut status: libc::c_int = 0;
    let mut rusage: libc::rusage = unsafe { std::mem::zeroed() };
    let reaped = unsafe { libc::wait4(pid, &mut status, 0, &mut rusage) };
    let wall_us = start.elapsed().as_micros() as u64;
    if reaped != pid {
        return Err(format!("wait4 failed: {}", std::io::Error::last_os_error()));
    }
    let exit_ok = libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0;
    Ok(Sample {
        wall_us,
        maxrss_kb: rusage.ru_maxrss as u64,
        exit_ok,
        stderr,
    })
}

/// `timing\t<phase>\t<micros>` lines from the CLI's `--timings` channel.
fn parse_timings(stderr: &str) -> Vec<(String, u64)> {
    stderr
        .lines()
        .filter_map(|line| {
            let mut cells = line.split('\t');
            if cells.next() != Some("timing") {
                return None;
            }
            let phase = cells.next()?.to_string();
            let micros = cells.next()?.parse().ok()?;
            Some((phase, micros))
        })
        .collect()
}

fn record(
    metrics: &mut BTreeMap<MetricKey, Vec<f64>>,
    fixture: &str,
    profile: &str,
    metric: &str,
    value: f64,
) {
    metrics
        .entry((fixture.to_string(), profile.to_string(), metric.to_string()))
        .or_default()
        .push(value);
}

/// The summary TSV: schema header plus one row per (fixture, profile,
/// metric) with sample count, median, and median absolute deviation.
fn summarize(metrics: &BTreeMap<MetricKey, Vec<f64>>) -> String {
    let mut out = String::from(
        "# mojito-bench summary; schema-version 1\n\
         # fixture <TAB> profile <TAB> metric <TAB> n <TAB> median <TAB> mad\n",
    );
    for ((fixture, profile, metric), values) in metrics {
        let median = median(values);
        let mad = mad(values, median);
        out.push_str(&format!(
            "{fixture}\t{profile}\t{metric}\t{}\t{median:.1}\t{mad:.1}\n",
            values.len()
        ));
    }
    out
}

fn median(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).expect("finite samples"));
    let mid = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    } else {
        sorted[mid]
    }
}

fn mad(values: &[f64], median_value: f64) -> f64 {
    let deviations: Vec<f64> = values
        .iter()
        .map(|value| (value - median_value).abs())
        .collect();
    median(&deviations)
}

/// Regression thresholds per metric, mirroring
/// `benchmarks/native/noise-policy.md` (the authority; change both
/// together). `(relative allowance, absolute noise floor)`.
const CHECK_THRESHOLDS: &[(&str, f64, f64)] = &[
    ("exec_wall_us", 0.10, 1000.0),
    ("compile_wall_us", 0.15, 10_000.0),
    ("exec_maxrss_kb", 0.10, 1024.0),
    ("compile_maxrss_kb", 0.10, 10_240.0),
    ("exe_bytes", 0.05, 4096.0),
];

/// Compare a fresh summary against a baseline TSV. A metric regresses when
/// it exceeds the baseline by more than its relative allowance AND the
/// delta clears both the absolute noise floor and the combined MAD spread.
fn check_against(baseline_path: &Path, summary: &str) -> Result<bool, String> {
    let baseline_text = std::fs::read_to_string(baseline_path)
        .map_err(|error| format!("cannot read {}: {error}", baseline_path.display()))?;
    let baseline = parse_summary(&baseline_text);
    let current = parse_summary(summary);
    let mut ok = true;
    for (key, (_, base_median, base_mad)) in &baseline {
        let (fixture, profile, metric) = key;
        let Some((threshold, floor)) = CHECK_THRESHOLDS
            .iter()
            .find(|(name, _, _)| name == metric)
            .map(|(_, threshold, floor)| (*threshold, *floor))
        else {
            continue;
        };
        let Some((_, current_median, current_mad)) = current.get(key) else {
            eprintln!("CHECK MISSING {fixture} {profile} {metric}: not in current run");
            ok = false;
            continue;
        };
        let delta = current_median - base_median;
        let regressed = delta > base_median * threshold
            && delta > floor
            && delta > 3.0 * (base_mad + current_mad);
        if regressed {
            eprintln!(
                "CHECK FAIL {fixture} {profile} {metric}: {current_median:.0} vs baseline \
                 {base_median:.0} (+{:.1}%)",
                delta / base_median * 100.0
            );
            ok = false;
        }
    }
    if ok {
        eprintln!(
            "CHECK PASS: no regressions against {}",
            baseline_path.display()
        );
    }
    Ok(ok)
}

/// Rows of a summary TSV as `(key) -> (n, median, mad)`.
fn parse_summary(text: &str) -> BTreeMap<MetricKey, (u64, f64, f64)> {
    let mut rows = BTreeMap::new();
    for line in text.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let cells: Vec<&str> = line.split('\t').collect();
        if cells.len() != 6 {
            continue;
        }
        let (Ok(n), Ok(median), Ok(mad)) = (cells[3].parse(), cells[4].parse(), cells[5].parse())
        else {
            continue;
        };
        rows.insert(
            (
                cells[0].to_string(),
                cells[1].to_string(),
                cells[2].to_string(),
            ),
            (n, median, mad),
        );
    }
    rows
}

/// The leading raw-record: everything needed to interpret the run.
fn runner_metadata(config: &Config) -> String {
    let cpu =
        read_first_match("/proc/cpuinfo", "model name").unwrap_or_else(|| "unknown".to_string());
    let governor = std::fs::read_to_string("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    let git_rev = command_line(Command::new("git").args(["rev-parse", "HEAD"]))
        .unwrap_or_else(|| "unknown".to_string());
    let toolchain = command_stdout(Command::new(&config.mojito).args([
        "compile",
        "--backend",
        "pliron",
        "--print-toolchain",
    ]))
    .unwrap_or_default();
    let epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!(
        "{{\"schema\":1,\"kind\":\"runner\",\"epoch\":{epoch},\"cpu\":{},\"governor\":{},\
         \"git_rev\":{},\"smoke\":{},\"toolchain\":{}}}",
        json_string(&cpu),
        json_string(&governor),
        json_string(&git_rev),
        config.smoke,
        json_string(toolchain.trim()),
    )
}

fn raw_record(
    kind: &str,
    fixture: &str,
    profile: &str,
    sample: u32,
    measured: &Sample,
    phases: &[(String, u64)],
    exe_bytes: Option<u64>,
) -> String {
    let mut record = format!(
        "{{\"schema\":1,\"kind\":{},\"fixture\":{},\"profile\":{},\"sample\":{sample},\
         \"wall_us\":{},\"maxrss_kb\":{}",
        json_string(kind),
        json_string(fixture),
        json_string(profile),
        measured.wall_us,
        measured.maxrss_kb,
    );
    if !phases.is_empty() {
        let inner: Vec<String> = phases
            .iter()
            .map(|(phase, micros)| format!("{}:{micros}", json_string(phase)))
            .collect();
        record.push_str(&format!(",\"phases\":{{{}}}", inner.join(",")));
    }
    if let Some(bytes) = exe_bytes {
        record.push_str(&format!(",\"exe_bytes\":{bytes}"));
    }
    record.push('}');
    record
}

fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn read_first_match(path: &str, prefix: &str) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()?
        .lines()
        .find(|line| line.starts_with(prefix))
        .and_then(|line| line.split(':').nth(1))
        .map(|value| value.trim().to_string())
}

fn command_line(command: &mut Command) -> Option<String> {
    command_stdout(command).map(|out| out.trim().to_string())
}

fn command_stdout(command: &mut Command) -> Option<String> {
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

/// Minimal self-cleaning temp dir (no tempfile dependency).
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn create() -> Result<TempDir, String> {
        let path = std::env::temp_dir().join(format!("mojito-bench.{}", std::process::id()));
        std::fs::create_dir_all(&path)
            .map_err(|error| format!("cannot create {}: {error}", path.display()))?;
        Ok(TempDir { path })
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_and_mad_are_robust() {
        let values = [10.0, 12.0, 11.0, 100.0, 11.5];
        let m = median(&values);
        assert_eq!(m, 11.5);
        assert_eq!(mad(&values, m), 0.5);
    }

    #[test]
    fn timings_parse_only_timing_lines() {
        let stderr = "noise\ntiming\tfrontend\t123\ntiming\tclang\t42\nother\tstuff\n";
        assert_eq!(
            parse_timings(stderr),
            vec![("frontend".to_string(), 123), ("clang".to_string(), 42)]
        );
    }

    #[test]
    fn summary_round_trips_through_parse() {
        let mut metrics = BTreeMap::new();
        for value in [10.0, 11.0, 12.0] {
            super::record(&mut metrics, "f.mojo", "0", "exec_wall_us", value);
        }
        let text = summarize(&metrics);
        let parsed = parse_summary(&text);
        let row = parsed
            .get(&(
                "f.mojo".to_string(),
                "0".to_string(),
                "exec_wall_us".to_string(),
            ))
            .expect("row survives");
        assert_eq!(*row, (3, 11.0, 1.0));
    }

    #[test]
    fn seeded_regression_fails_check() {
        // A halved-baseline exec median is a seeded 100% slowdown; --check
        // must reject it and must pass the unmodified summary.
        let summary = "\
# mojito-bench summary; schema-version 1
f.mojo\t0\texec_wall_us\t10\t80000.0\t100.0
f.mojo\t0\texe_bytes\t1\t50000.0\t0.0
";
        let seeded = summary.replace("80000.0", "40000.0");
        let dir = std::env::temp_dir().join(format!("mojito-bench-test.{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("tempdir");
        let baseline = dir.join("baseline.tsv");
        std::fs::write(&baseline, &seeded).expect("write baseline");
        assert_eq!(check_against(&baseline, summary), Ok(false));
        std::fs::write(&baseline, summary).expect("write identical baseline");
        assert_eq!(check_against(&baseline, summary), Ok(true));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn json_strings_escape_specials() {
        assert_eq!(json_string("a\"b\\c\nd"), "\"a\\\"b\\\\c\\nd\"");
    }
}
