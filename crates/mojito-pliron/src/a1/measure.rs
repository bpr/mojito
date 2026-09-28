//! Measurement: the shadow pipeline with each phase timed on its own, and
//! the summary that applies the predeclared budgets to paired samples.
//!
//! Timers are exclusive: each phase's clock covers that phase alone, and
//! `canonicalize` includes the fresh-context parse its definition needs.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::time::Instant;

use mojito_mir::mir::MirProgram;

use super::export::{Exported, export_program};
use super::import::import_program;
use super::inventory::Stage;
use super::ir_framework::new_context;
use super::opt::eliminate_dead_scalars;
use super::outcomes::{denormalize, normalize};
use super::text::{canonical_text, parse_text};
use super::verify::{verify_module, walk};
use super::{A1Error, A1ErrorKind};

/// The compile-time budget: the shadow's paired ratio stays strictly below.
pub const TIME_LIMIT: f64 = 1.20;
/// The peak-memory budget, likewise strict.
pub const MEMORY_LIMIT: f64 = 1.30;

/// What one shadow run built, and what each phase cost.
pub struct ShadowRun {
    pub exported: Exported,
    pub core_text: String,
    /// Each phase and its exclusive duration in nanoseconds, in order.
    pub phases: Vec<(&'static str, u128)>,
    pub functions: usize,
    pub blocks: usize,
    pub operations: usize,
    pub attributes: usize,
    /// Operations the dead-scalar pass removed.
    pub removed: usize,
}

/// Run the whole shadow boundary over `program`.
///
/// It specializes, builds core, verifies, normalizes, canonicalizes, parses
/// into a fresh context, and exports verified MIR; with `optimize`, the
/// dead-scalar pass runs on the executable core first.
pub fn shadow(
    program: &MirProgram,
    entries: &[String],
    optimize: bool,
) -> Result<ShadowRun, A1Error> {
    let mut phases = Vec::new();
    let mut timed = |phase: &'static str, started: Instant| {
        phases.push((phase, started.elapsed().as_nanos()));
    };

    let started = Instant::now();
    let specialized =
        mojito_native::native::mono::specialize(program, entries).map_err(|error| {
            A1Error::new(
                A1ErrorKind::UnsupportedForm,
                format!("specialization refuses the program: {}", error.construct),
            )
        })?;
    timed("specialize", started);
    let mut entry_map: Vec<(String, String)> = specialized.entries.into_iter().collect();
    entry_map.sort();

    let started = Instant::now();
    let mut ctx = new_context();
    let module = import_program(&mut ctx, &specialized.program, &entry_map)?;
    timed("construct", started);
    drop(specialized.program);

    let started = Instant::now();
    verify_module(&ctx, module, Stage::Bridge)?;
    timed("verify_bridge", started);

    let started = Instant::now();
    normalize(&mut ctx, module)?;
    timed("normalize", started);

    let started = Instant::now();
    verify_module(&ctx, module, Stage::ExecutableCore)?;
    timed("verify_core", started);

    let mut removed = 0;
    if optimize {
        let started = Instant::now();
        removed = eliminate_dead_scalars(&mut ctx, module)?.removed.len();
        timed("dce", started);
        let started = Instant::now();
        verify_module(&ctx, module, Stage::ExecutableCore)?;
        timed("verify_dce", started);
    }

    let started = Instant::now();
    let core_text = canonical_text(&mut ctx, module, Stage::ExecutableCore)?;
    timed("canonicalize", started);

    let started = Instant::now();
    let mut parsed = parse_text(&core_text, Stage::ExecutableCore)?;
    timed("parse", started);
    drop(ctx);

    let operations = walk(&parsed.ctx, parsed.module);
    let functions = super::outcomes::functions(&parsed.ctx, parsed.module);
    let blocks = functions
        .iter()
        .map(|func| super::outcomes::function_blocks(&parsed.ctx, *func).len())
        .sum();
    let attributes = operations
        .iter()
        .map(|op| op.deref(&parsed.ctx).attributes.0.len())
        .sum();
    let (functions, operations) = (functions.len(), operations.len());

    let started = Instant::now();
    denormalize(&mut parsed.ctx, parsed.module)?;
    verify_module(&parsed.ctx, parsed.module, Stage::Bridge)?;
    let exported = export_program(&parsed.ctx, parsed.module)?;
    timed("export", started);

    let started = Instant::now();
    let findings = mojito_mir::mir::verify::verify(&exported.program);
    timed("verify_mir", started);
    if !findings.is_empty() {
        let input = mojito_native::native::mono::specialize(program, entries)
            .map(|specialized| mojito_mir::mir::verify::verify(&specialized.program))
            .unwrap_or_default();
        let (kind, what) = if input == findings {
            (
                A1ErrorKind::UnsupportedForm,
                "the specialized input does not verify, and the export reproduces it",
            )
        } else {
            (A1ErrorKind::Export, "the exported MIR does not verify")
        };
        return Err(A1Error::new(
            kind,
            format!("{what}: {}", findings.join("; ")),
        ));
    }
    Ok(ShadowRun {
        exported,
        core_text,
        phases,
        functions,
        blocks,
        operations,
        attributes,
        removed,
    })
}

/// One measured child process.
#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    pub input: String,
    /// `baseline`, `shadow`, or `shadow-opt`.
    pub mode: String,
    pub run: usize,
    pub wall_ns: f64,
    pub peak_kib: f64,
    /// Whether the process succeeded with full coverage of its input.
    pub ok: bool,
}

/// How one budget fared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The conservative upper bound is strictly below the limit.
    Pass,
    /// The lower bound is above the limit.
    Fail,
    /// The bounds straddle the limit: repeat a quiet batch once.
    Inconclusive,
    /// A sample failed or is missing: never a pass, never a rejection.
    Incomplete,
}

/// Median, spread, and extremes of one series.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spread {
    pub median: f64,
    pub p90: f64,
    pub min: f64,
    pub max: f64,
    /// The median absolute deviation.
    pub mad: f64,
}

impl Spread {
    pub fn of(values: &[f64]) -> Option<Self> {
        let mut sorted = values.to_vec();
        sorted.sort_by(f64::total_cmp);
        let median = middle(&sorted)?;
        let mut deviations: Vec<f64> = sorted.iter().map(|value| (value - median).abs()).collect();
        deviations.sort_by(f64::total_cmp);
        let rank = (sorted.len() * 9).div_ceil(10).max(1) - 1;
        Some(Self {
            median,
            p90: sorted[rank],
            min: sorted[0],
            max: sorted[sorted.len() - 1],
            mad: middle(&deviations)?,
        })
    }

    /// The median plus three median absolute deviations.
    pub const fn upper(&self) -> f64 {
        3.0f64.mul_add(self.mad, self.median)
    }

    pub const fn lower(&self) -> f64 {
        (-3.0f64).mul_add(self.mad, self.median)
    }

    pub fn verdict(&self, limit: f64) -> Verdict {
        if self.upper() < limit {
            Verdict::Pass
        } else if self.lower() > limit {
            Verdict::Fail
        } else {
            Verdict::Inconclusive
        }
    }
}

/// One row of the summary: an input, or the aggregate of all of them.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub input: String,
    pub baseline: Spread,
    pub shadow: Spread,
    /// The paired shadow-to-baseline wall-time ratio, run by run.
    pub time_ratio: Spread,
    pub baseline_peak_kib: f64,
    pub shadow_peak_kib: f64,
    /// The paired shadow-to-baseline peak-memory ratio, run by run.
    pub memory_ratio: Spread,
    pub time: Verdict,
    pub memory: Verdict,
}

/// The name of the aggregate row.
pub const AGGREGATE: &str = "(all inputs)";

/// Apply the budgets to the paired samples of `shadow_mode` against
/// `baseline`. Every input has a row; the last row sums each run's
/// compile times over all inputs and compares the largest peaks.
pub fn summarize(samples: &[Sample], shadow_mode: &str) -> Vec<Row> {
    let mut inputs: BTreeMap<&str, (Vec<&Sample>, Vec<&Sample>)> = BTreeMap::new();
    for sample in samples {
        let entry = inputs.entry(sample.input.as_str()).or_default();
        if sample.mode == "baseline" {
            entry.0.push(sample);
        } else if sample.mode == shadow_mode {
            entry.1.push(sample);
        }
    }
    let mut rows = Vec::new();
    let mut totals: BTreeMap<usize, (f64, f64, f64, f64)> = BTreeMap::new();
    let mut complete = true;
    for (input, (baseline, shadow)) in &inputs {
        let paired = pairs(baseline, shadow);
        let covered = paired.is_some();
        complete &= covered;
        let pairs = paired.unwrap_or_default();
        for (run, (base, shade)) in pairs.iter().enumerate() {
            let total = totals.entry(run).or_insert((0.0, 0.0, 0.0, 0.0));
            total.0 += base.wall_ns;
            total.1 += shade.wall_ns;
            total.2 = total.2.max(base.peak_kib);
            total.3 = total.3.max(shade.peak_kib);
        }
        let series: Vec<(f64, f64, f64, f64)> = pairs
            .iter()
            .map(|(base, shade)| (base.wall_ns, shade.wall_ns, base.peak_kib, shade.peak_kib))
            .collect();
        rows.push(row(input, &series, covered));
    }
    let series: Vec<(f64, f64, f64, f64)> = totals.into_values().collect();
    rows.push(row(AGGREGATE, &series, complete && !inputs.is_empty()));
    rows
}

/// The summary as tab-separated text, times in milliseconds.
pub fn summary_tsv(rows: &[Row]) -> String {
    let mut out = String::from(
        "input\tbaseline_median_ms\tbaseline_p90_ms\tshadow_median_ms\tshadow_p90_ms\ttime_ratio\ttime_ratio_upper\ttime_ratio_lower\ttime\tbaseline_peak_kib\tshadow_peak_kib\tmemory_ratio\tmemory_ratio_upper\tmemory\n",
    );
    for row in rows {
        let ms = |ns: f64| ns / 1_000_000.0;
        let _ = writeln!(
            out,
            "{}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\t{:.4}\t{:.4}\t{:.4}\t{:?}\t{:.0}\t{:.0}\t{:.4}\t{:.4}\t{:?}",
            row.input,
            ms(row.baseline.median),
            ms(row.baseline.p90),
            ms(row.shadow.median),
            ms(row.shadow.p90),
            row.time_ratio.median,
            row.time_ratio.upper(),
            row.time_ratio.lower(),
            row.time,
            row.baseline_peak_kib,
            row.shadow_peak_kib,
            row.memory_ratio.median,
            row.memory_ratio.upper(),
            row.memory,
        );
    }
    out
}

/// The samples of one input paired run by run, or `None` when a run
/// failed, is missing, or has no partner.
fn pairs<'a>(
    baseline: &[&'a Sample],
    shadow: &[&'a Sample],
) -> Option<Vec<(&'a Sample, &'a Sample)>> {
    if baseline.is_empty() || baseline.len() != shadow.len() {
        return None;
    }
    let mut paired = Vec::new();
    for base in baseline {
        let shade = shadow.iter().find(|shade| shade.run == base.run)?;
        if !base.ok || !shade.ok {
            return None;
        }
        paired.push((*base, *shade));
    }
    Some(paired)
}

fn row(input: &str, series: &[(f64, f64, f64, f64)], complete: bool) -> Row {
    let column = |select: fn(&(f64, f64, f64, f64)) -> f64| -> Vec<f64> {
        series.iter().map(select).collect()
    };
    let empty = Spread {
        median: 0.0,
        p90: 0.0,
        min: 0.0,
        max: 0.0,
        mad: 0.0,
    };
    let spread = |values: Vec<f64>| Spread::of(&values).unwrap_or(empty);
    let time_ratio = spread(column(|sample| sample.1 / sample.0));
    let memory_ratio = spread(column(|sample| sample.3 / sample.2));
    let peak = |values: Vec<f64>| values.into_iter().fold(0.0, f64::max);
    let verdict = |spread: &Spread, limit: f64| {
        if complete {
            spread.verdict(limit)
        } else {
            Verdict::Incomplete
        }
    };
    Row {
        input: input.to_string(),
        baseline: spread(column(|sample| sample.0)),
        shadow: spread(column(|sample| sample.1)),
        time: verdict(&time_ratio, TIME_LIMIT),
        memory: verdict(&memory_ratio, MEMORY_LIMIT),
        time_ratio,
        baseline_peak_kib: peak(column(|sample| sample.2)),
        shadow_peak_kib: peak(column(|sample| sample.3)),
        memory_ratio,
    }
}

fn middle(sorted: &[f64]) -> Option<f64> {
    let count = sorted.len();
    match count {
        0 => None,
        _ if count % 2 == 1 => Some(sorted[count / 2]),
        _ => Some(f64::midpoint(sorted[count / 2 - 1], sorted[count / 2])),
    }
}
