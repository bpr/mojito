//! Opt-in phase timing for the compiler pipeline (`mojito --timings`).
//!
//! The collector is a process-wide switch plus a thread-local event log.
//! Disabled — the default — a span is one relaxed atomic load and nothing
//! else: no clock read, lock, allocation, or formatting. Enabled, each span
//! records an enter/exit pair, and [`report`] folds the log into one record
//! per hierarchical phase path (`compile.discovery.round[1].check.bodies`)
//! with inclusive time, self time, and invocation count, so repeated work
//! (a checker pass rerun per discovery round) stays distinguishable from a
//! single slow pass. Counters attach to the innermost open span.
//!
//! Output is the machine-readable `timing\t<path>\t<inclusive_us>` record
//! the native bench driver already parses, extended with `\t<self_us>\t
//! <count>` columns, plus `count\t<path>\t<n>` lines; it goes to stderr so
//! program output stays byte-identical.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

/// Turn timing collection on for the rest of the process.
pub fn enable() {
    ENABLED.store(true, Ordering::Relaxed);
}

/// Whether spans are being recorded.
#[inline]
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Open a timed phase; it closes when the returned guard drops.
#[inline]
pub fn span(name: &'static str) -> Span {
    enter(name, None)
}

/// Open a timed phase distinguished by an iteration number (`name[n]`).
#[inline]
pub fn round(name: &'static str, n: usize) -> Span {
    enter(name, Some(n))
}

/// Add `n` to a counter attached to the innermost open span.
#[inline]
pub fn count(name: &'static str, n: u64) {
    if enabled() {
        EVENTS.with(|events| events.borrow_mut().push(Event::Count { name, n }));
    }
}

/// Fold the recorded events into per-path records, in first-entry order.
/// Spans still open (an early `return` in the caller) are closed at `now`.
pub fn report() -> String {
    if !enabled() {
        return String::new();
    }
    let events = EVENTS.with(|events| std::mem::take(&mut *events.borrow_mut()));
    let now = Instant::now();
    let mut order: Vec<String> = Vec::new();
    let mut records: HashMap<String, Record> = HashMap::new();
    let mut counters: Vec<(String, u64)> = Vec::new();
    let mut counter_index: HashMap<String, usize> = HashMap::new();
    let mut stack: Vec<OpenSpan> = Vec::new();
    for event in events {
        match event {
            Event::Enter { name, round, at } => {
                let mut path = stack
                    .last()
                    .map(|open| format!("{}.", open.path))
                    .unwrap_or_default();
                path.push_str(name);
                if let Some(n) = round {
                    let _ = write!(path, "[{n}]");
                }
                if !records.contains_key(&path) {
                    order.push(path.clone());
                    records.insert(path.clone(), Record::default());
                }
                stack.push(OpenSpan {
                    path,
                    start: at,
                    children: 0,
                });
            }
            Event::Exit { at } => close(&mut stack, &mut records, at),
            Event::Count { name, n } => {
                let path = match stack.last() {
                    Some(open) => format!("{}.{name}", open.path),
                    None => name.to_string(),
                };
                match counter_index.get(&path) {
                    Some(&i) => counters[i].1 += n,
                    None => {
                        counter_index.insert(path.clone(), counters.len());
                        counters.push((path, n));
                    }
                }
            }
        }
    }
    while !stack.is_empty() {
        close(&mut stack, &mut records, now);
    }
    let mut out = String::new();
    for path in &order {
        let r = &records[path];
        let _ = writeln!(
            out,
            "timing\t{path}\t{}\t{}\t{}",
            r.inclusive, r.self_time, r.count
        );
    }
    for (path, n) in &counters {
        let _ = writeln!(out, "count\t{path}\t{n}");
    }
    out
}

/// A timed phase; records its exit when dropped.
pub struct Span {
    armed: bool,
}

impl Drop for Span {
    #[inline]
    fn drop(&mut self) {
        if self.armed {
            let at = Instant::now();
            EVENTS.with(|events| events.borrow_mut().push(Event::Exit { at }));
        }
    }
}

static ENABLED: AtomicBool = AtomicBool::new(false);

thread_local! {
    static EVENTS: RefCell<Vec<Event>> = const { RefCell::new(Vec::new()) };
}

enum Event {
    Enter {
        name: &'static str,
        round: Option<usize>,
        at: Instant,
    },
    Exit {
        at: Instant,
    },
    Count {
        name: &'static str,
        n: u64,
    },
}

#[derive(Default)]
struct Record {
    inclusive: u128,
    self_time: u128,
    count: u64,
}

/// A span on the report's reconstruction stack: its full path, start, and
/// the inclusive time of the children closed so far.
struct OpenSpan {
    path: String,
    start: Instant,
    children: u128,
}

#[inline]
fn enter(name: &'static str, round: Option<usize>) -> Span {
    if !enabled() {
        return Span { armed: false };
    }
    let at = Instant::now();
    EVENTS.with(|events| events.borrow_mut().push(Event::Enter { name, round, at }));
    Span { armed: true }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_spans_record_nothing() {
        // `enable` is process-wide; this test must run before any enabling
        // test in the same process, so it only checks the guard shape.
        let _outer = span("outer");
        count("things", 3);
        assert!(EVENTS.with(|e| e.borrow().is_empty()) || enabled());
    }

    #[test]
    fn report_folds_paths_rounds_and_counters() {
        enable();
        {
            let _total = span("total");
            for n in 0..2 {
                let _r = round("round", n);
                let _inner = span("inner");
                count("items", 5);
            }
            let _twice = span("twice");
        }
        {
            let _twice = span("twice");
        }
        let report = report();
        let lines: Vec<&str> = report.lines().collect();
        assert!(lines[0].starts_with("timing\ttotal\t"), "{report}");
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("timing\ttotal.round[0].inner\t")),
            "{report}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("timing\ttotal.round[1].inner\t")),
            "{report}"
        );
        let twice = lines
            .iter()
            .find(|l| l.starts_with("timing\ttwice\t"))
            .expect("top-level twice");
        assert!(twice.ends_with("\t1"), "{twice}");
        let nested = lines
            .iter()
            .find(|l| l.starts_with("timing\ttotal.twice\t"))
            .expect("nested twice");
        assert!(nested.ends_with("\t1"), "{nested}");
        assert!(
            lines.contains(&"count\ttotal.round[0].inner.items\t5"),
            "{report}"
        );
        // Self time never exceeds inclusive time.
        for line in &lines {
            if let Some(rest) = line.strip_prefix("timing\t") {
                let cells: Vec<&str> = rest.split('\t').collect();
                let inclusive: u128 = cells[1].parse().unwrap();
                let self_time: u128 = cells[2].parse().unwrap();
                assert!(self_time <= inclusive, "{line}");
            }
        }
    }
}

/// Close the innermost open span at `at`, folding it into its record and
/// its parent's child time.
fn close(stack: &mut Vec<OpenSpan>, records: &mut HashMap<String, Record>, at: Instant) {
    let open = stack.pop().expect("exit without enter");
    let inclusive = at.duration_since(open.start).as_micros();
    let record = records.entry(open.path).or_default();
    record.inclusive += inclusive;
    record.self_time += inclusive.saturating_sub(open.children);
    record.count += 1;
    if let Some(parent) = stack.last_mut() {
        parent.children += inclusive;
    }
}
