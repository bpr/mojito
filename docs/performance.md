# Compiler performance

Mojito currently has an excessive fixed cost for small programs. A Hello World
run has been observed at more than 25 seconds with a debug compiler and a little
over 12 seconds with a release compiler, versus about 0.7 seconds for Mojo. A
direct invocation of the already-built debug binary in the current checkout also
ran for more than 90 seconds before it was interrupted. That last result is a
single diagnostic observation, not a benchmark, but it rules out Cargo startup
as the whole explanation.

The Mojo comparison is useful as a product-level target, but it is not yet an
apples-to-apples compiler comparison: Mojo may be loading compiled standard
library artifacts or using persistent caches that Mojito does not have. The
first objective is therefore to account for Mojito's own time reliably.

## Measurements (2026-09-04)

The first step of the timing plan below has been carried out; the sections
after this one remain the plan, annotated with what now exists.

**Environment.** Commit `fc1692b` plus the timing instrumentation of this
change; rustc 1.96.1; Intel i7-10875H (8 cores, turbo on, `intel_pstate`
in `powersave`, package at ~77 °C — the numbers below are ~1.5–2× the
2026-09-03 observations in the roadmap and should be read as ratios, not
absolutes); Linux 7.1.1-76070101; VM backend; page cache warm; nothing else
running except where noted.

**Hello World is pure single-threaded CPU.** Under `/usr/bin/time -v` the
debug binary spends 50 s user + 3 s system, has zero major page faults, one
voluntary context switch, a 266 MB peak RSS, and 2.1 M minor page faults;
`strace -c` shows 27 file opens and 1139 `brk` calls. Startup, disk, and
stdlib I/O are not the problem.

**External baseline** (`scripts/bench-compile`; wall seconds, median with
min–max, 3 release / 2 debug runs after a warm-up; `parse` is 1–4 ms for
every fixture and omitted). Every program costs the same regardless of its
content — the fixed-cost signature:

| Fixture | Release `check` | Release `run` | Debug `check` | Debug `run` |
|---|---:|---:|---:|---:|
| `empty` | 18.1 (18.0–18.2) | 24.5 (24.0–24.6) | 43.0 (43.0–43.0) | 52.3 (52.3–52.4) |
| `hello` | 19.8 (17.7–20.4) | 29.5 (26.3–30.3) | 42.6 (42.6–42.7) | 52.6 (52.4–52.7) |
| `add` | 18.5 (18.3–19.4) | 25.0 (24.2–29.7) | 42.6 (42.5–42.7) | 52.7 (52.4–52.9) |
| `generic` | 18.3 (18.0–19.2) | 25.5 (24.6–25.6) | 43.0 (42.7–43.4) | 53.3 (53.1–53.5) |
| `tuple` | 21.0 (21.0–21.5) | 27.7 (27.5–28.3) | 52.1 (51.8–52.3) | 62.5 (62.1–62.8) |
| `tstring` | 18.1 (18.0–18.4) | 28.2 (28.1–29.0) | 43.1 (43.1–43.1) | 61.5 (60.6–62.4) |
| `stdlib_heavy` | 18.5 (18.2–18.6) | 24.6 (24.4–24.7) | 43.4 (43.4–43.5) | 53.1 (53.1–53.2) |

`check` (link + one checker pass + MIR + ownership, no elaboration) is 18 s
of the 25 s release `run`, and `run` adds elaboration, the extra discovery
rounds, drop elaboration, and execution. User CPU equals wall time within
3 s of system time (the 2 M minor page faults) in every row.

These rows are the pre-fix baseline; see "After the fix" below for the
current numbers.

**Where the time goes** (`mojito run --timings benchmarks/compile/hello.mojo`,
inclusive seconds; the full tree has ~90 records):

| Phase | Release | Debug |
|---|---:|---:|
| `total` | 24.3 | 51.5 |
| `frontend.link` (20 stdlib modules, 169 KB, 75 linked declarations) | 0.02 | 0.06 |
| `compile.discovery.initial.check` (2 transfer rounds) | 0.21 | 1.24 |
| `compile.discovery.round[0]` (elaborate + check, 2 transfer rounds) | 0.31 | 1.39 |
| `compile.mir.lower` | **23.6** | **48.2** |
| — `mir.lower.struct` (41 structs) | 22.1 | — |
| — `mir.lower.def` (28 functions) | 1.4 | — |
| — `mir.lower.verify.pre_drop` | 0.006 | — |
| `compile.ownership` (515 functions, 3 analyses) | 0.03 | 0.17 |
| `backend.prepare` (drop elaboration, post-drop verify, MIR clone) | 0.08 | 0.37 |
| `backend.vm` | 0.004 | 0.006 |

The inferred-generic fixture (`benchmarks/compile/generic.mojo`, 522
functions after specialization) shows the same shape: 27.3 s total, 24.9 s
in `mir.lower.struct` and 1.7 s in `mir.lower.def`, with link at 0.02 s and
the two discovery rounds at 0.26 s.

Each individual checker pass over the full prelude costs 0.08 s (release);
Hello World runs four of them (two discovery rounds × two transfer rounds),
so the nested fixpoints that the plan below feared cost 0.5 s in total. The
linker is negligible. **MIR lowering of the 515 stdlib functions is 97 % of
the run**, and the cost sits in struct methods.

**Why** (stack samples from `scripts/sample-stacks` over the `profiling`
build, 178 samples at 0.2 s): 43 % of samples are inside
`mir::nested::lower_fn_nested`, and the self time is almost entirely
`memmove`, `malloc`/`free`, and `HashMap<CheckedNodeId, CheckedExpr>` clone
and drop. The mechanism is in `hir::Cfg::build_fn_with_context`
(`crates/mojito-hir/src/hir.rs`): for **every** function it lowers, it
clones the checked-expression table of the **entire linked program** —
`checked.iter().cloned().map(|n| (n.id, n)).collect()`, each `CheckedExpr`
carrying its own AST `syntax: Expr` — plus a span index over the same table
(`checked_index`) and a clone of every checked declaration; `checked_var_types`
then scans that whole map once per variable, and `mir::lower_cfg_nested`
clones the map again into its `Flatten` state. With ~10 k checked
expressions and 515 functions that is ~5 M `CheckedExpr` deep clones per
compile: quadratic in program size, and the reason the stdlib dominates
Hello World.

**Decision.** The first decision point below resolves to "neither": not
link plus initial check, not the rounds. The fix was to share the tables
instead of copying them.

**After the fix** (same day): `CheckedProgram` now owns one
`Arc<CheckedTables>` (expressions, declarations, and their indexes,
built once at construction) that every `Cfg` and MIR `Flatten` borrows.
Hello World on the same machine:

| | Release | Debug |
|---|---:|---:|
| `total` before | 24.3 s | 51.5 s |
| `total` after | **0.82 s** | **4.6 s** |
| `compile.mir.lower` after | 0.12 s | 0.43 s |

The external matrix after the fix (same method as the baseline above; 5
release / 3 debug runs):

| Fixture | Release `check` | Release `run` | Debug `check` | Debug `run` |
|---|---:|---:|---:|---:|
| `empty` | 0.42 (0.41–0.42) | 0.79 (0.78–0.80) | 2.41 (2.41–2.45) | 4.78 (4.73–4.80) |
| `hello` | 0.41 (0.41–0.42) | 0.78 (0.69–0.80) | 2.35 (2.35–2.50) | 4.77 (4.72–4.80) |
| `add` | 0.41 (0.40–0.41) | 0.79 (0.78–0.79) | 2.42 (2.36–2.46) | 4.67 (4.66–4.68) |
| `generic` | 0.42 (0.41–0.44) | 0.80 (0.77–0.81) | 2.41 (2.40–2.42) | 4.72 (4.69–4.78) |
| `tuple` | 0.43 (0.41–0.45) | 0.81 (0.72–0.88) | 2.28 (2.24–2.29) | 4.49 (4.47–4.56) |
| `tstring` | 0.41 (0.41–0.42) | 1.09 (1.08–1.11) | 2.58 (2.25–2.58) | 6.11 (6.06–6.28) |
| `stdlib_heavy` | 0.41 (0.41–0.44) | 0.81 (0.79–0.82) | 2.57 (2.51–2.61) | 5.15 (5.08–5.19) |

The remaining release profile is flat: four checker passes at 0.09 s each,
`explicit_destroy` at 0.06 s per check, MIR lowering 0.12 s, drop
elaboration 0.07 s, link 0.02 s, VM 0.005 s. The next bucket to attack is
the nested discovery/transfer rounds (0.55 s of the 0.82 s), then a
precompiled-prelude cache if sub-0.3 s matters.

**Per-instantiation generic struct bodies (2026-09-04/05).** Ordinary
generic structs now mint per-instantiation method clones
(`docs/architecture.md`), and every call shape reaches them; the release
matrix (same method, 5 runs, `--cmds run`, medians) before and after, with
the counters `struct_requests`/`method_requests` now emitted
beside `def_requests`:

| Fixture | Before | After | Instances |
|---|---:|---:|---:|
| `empty` | 0.761 | 0.747 | 0 |
| `hello` | 0.757 | 0.749 | 0 |
| `add` | 0.756 | 0.765 | 0 |
| `generic` | 0.757 | 0.832 | 1 |
| `tuple` | 0.796 | 0.787 | 0 |
| `tstring` | 1.041 | 1.037 | 0 |
| `stdlib_heavy` | 0.763 | 1.888 | 24 |

`stdlib_heavy` (six user instantiations of `Box`/`List`/`Dict`, 24 with the
storage they reach, 919 MIR functions instead of 538; 2.5x) pays for two
things:
every discovery round re-checks ~380 clone bodies (each check pass 0.45 s
instead of 0.27 s, the two transfer rounds included), and one extra round
(0.55 s) comes from one inferred generic-def call inside a clone body
(`hash[String, AHasher]` in `Dict[String, Int]`'s clone) that is a genuinely
new instantiation — the erased template never requested it. An occurrence
whose def clone already exists now retargets in the checker without a round
(`existing_def_clone`), so only new instantiations discovered inside clones
still cost one. The remaining levers, in order: reachability-pruned clone
lowering and the §1/§2 round-structure items below. Programs without their own
instantiations are unchanged: instances reached only inside bundled stdlib
bodies keep the erased path and mint nothing.

**Tooling that exists now.** `--timings` on every command
(`docs/usage.md`); `scripts/bench-compile` (hyperfine matrix over
`benchmarks/compile/`, results under `benchmarks/compile/results/`,
gitignored); `scripts/sample-stacks` (gdb-based sampler, no sudo; `samply`
is installed but needs `kernel.perf_event_paranoid=1`); the `profiling`
Cargo profile (release + debug info).

## Redundant-work audit after the HIR fix

The program-wide checked-table clone was the dominant problem and is now fixed.
A subsequent source audit found the following remaining repeated work. These
items are ordered by expected value given the post-fix profile. Only the first
bucket is already supported by timings; the rest are candidates to measure,
not claims about current wall-clock impact.

### 1. Discovery produces complete programs that are discarded

`Compiler::compile_linked` performs an initial elaboration and check, extracts
specialization requests, then repeats full elaboration and checking whenever
the request set grows. Every temporary check also performs work needed only for
the final program:

- trait-default expansion and syntax rekeying;
- the complete transfer-effect fixpoint and reference-result validation;
- explicit-destroy analysis;
- construction of all `CheckedProgram` fact tables; and
- reconstruction of checked expressions and declarations by walking the AST.

The post-fix Hello World profile attributes about 0.55 s of its 0.82 s release
time to the nested discovery/checker rounds. The first experiment should be a
dedicated discovery result or checker mode that produces only the facts needed
to request specializations. Final-only validation and complete
`CheckedProgram` assembly should run once after specialization converges.

**Landed 2026-09-20, in part.** A round's check now returns a
`DiscoveryResult` and the checked arena is assembled once, for the round that
converges (see *Checked templates and the discovery result* below). Trait
defaults and syntax rekeying are not per-round waste: `prepare` already moves
invariant normalization outside the loop, and the rekey is what identifies a
clone's occurrences. The transfer fixpoint and the rejecting checks still run
every round, because a program they reject must be rejected.

### 2. Transfer effects recheck every declaration and body

Within each outer discovery check, transfer and call-through effects start with
an incomplete seed. If a call site observed an effect summary that later grew,
the checker constructs a fresh `Checker` and checks the entire expanded program
again. Hello World used to take two such rounds for every outer check.

Resolved (2026-09-26) by carrying facts between passes rather than by a
worklist: every fact store logs its writes, each body site records the log
ranges it wrote and the effect entries it read, and the next pass — a
transfer pass or the first pass of the next discovery round, which now
starts from the previous round's committed effect maps — copies a body's
entries when its record is clean and every read is still current
(`docs/architecture.md`, `checker/body_carry.rs`). Hello World on the
reference machine: 2.3 s to 1.5 s release and 15 s to 7.9 s debug; the six
full body passes become one full pass (0.31 s release) and three carried
ones (about 0.09 s each, `check_program.bodies` under `--timings`), with
6 bodies re-inferred in the second transfer pass and 100–250 in each later
round (new clones, rewritten calls, and served requests). What a carried
pass still costs is the copy itself and the per-site syntax hash
(`docs/roadmap.md` R148).

### 3. Comptime elaboration rebuilds invariant indexes each round

Every `elaborate_with_requests` invocation starts again from a clone of the
linked program. It resynthesizes lifecycle methods, rebuilds a
`ConformanceOracle`, recollects bound-generic templates, and separately indexes
functions, structs, and specializable declarations before re-elaborating the
whole program and restamping provenance. The compiler has already scanned the
linked program for bound-generic template names before entering this loop.

A persistent elaboration session could own the synthesized base AST,
declaration indexes, conformance information, and template catalog, then apply
only newly discovered requests in later rounds.

**Measured 2026-09-20: not worth it yet.** `prepare` already resynthesizes
lifecycle methods once. What `elaborate_prepared` still rebuilds per round —
the conformance oracle and the declaration indexes — costs about 5 ms
(`discovery.*.elaborate.indexes` in `--timings`), and a whole elaboration is
about 0.13 s of a 9.9 s debug Hello World. Native monomorphization consumes
`MirProgram`, not the elaborator's output, so nothing downstream shares this
state either.

### 4. Request discovery makes overlapping full-program scans

Each discovery round separately walks checked expressions/declarations for
Tuple requests, checked expressions for template strings, and the generic
instantiation map for both ordinary generics and scalar ranges. Recursive Tuple
type collection may revisit the same types, while request accumulation uses
linear `Vec::contains`/`Vec::position` deduplication.

Prefer one checker-produced request index, or at least one combined collection
pass, with canonical request keys in sets/maps. This is currently a scaling
concern rather than a measured Hello World bottleneck.

### 5. Checked facts still contain duplicate compatibility indexes

Neither `CheckedTables` nor `CheckedProgram` keeps an expression location
index any more; a node is reached by its id alone. The program still retains
compatibility overload-target and implicit-conversion maps while the newer
checked-expression/call representations carry much of the same semantic
information. MIR still reads some compatibility data, so this is an
incomplete migration rather than dead state. Finish that migration.

### 6. Ownership facts are discarded and partly recomputed for drops

The pre-drop ownership gate runs move, interior-origin, and loan analyses over
every MIR function but returns only success or failure. Drop elaboration later
recomputes related loan-generation destinations and entries, register-loan
states and uses, definition/move sets, CFG relationships, liveness, and
per-instruction state sequences.

The analyses are not interchangeable, so do not merely delete the second set.
Instead, determine which successful ownership facts can form a reusable
analysis artifact consumed by drop elaboration. Current timings make this a
scaling improvement, not the next latency fix.

### 7. MIR and VM ownership APIs force whole-program copies

Drop elaboration clones the complete pre-drop MIR so `CompiledProgram` can
retain both representations. VM execution then clones the cached elaborated MIR
again because `run_elaborated` consumes it. The first copy may be justified by
the public pre-drop view; the second is primarily an API ownership cost.

Candidate designs are a borrowing VM, shared post-drop MIR, a consuming
execution path, or retaining pre-drop MIR only when explicitly requested. VM
startup also clones declaration metadata into separate struct and signature
registries while retaining the original declarations. A single indexed program
representation could keep the metadata once.

One concrete runtime case should be fixed independently: every executed
`SizeOf` instruction rebuilds the struct-field index for the entire MIR program.
Build that index once in `Prog`, or resolve the size during compilation.

### 8. Native layout and type lowering are uncached

`LayoutCx::layout_of` recursively recomputes layouts for structs, tuples,
variants, and their fields. The Pliron backend calls it, aggregate layout, and
`lower_ty` repeatedly during declaration and instruction lowering. Cache layout
by target and canonical type; consider caching lowered backend types in the
per-module lowering context as well. Measure this on native fixtures before
choosing the cache representation.

### 9. Pliron eagerly constructs optional artifacts

Every Pliron compilation renders and stores canonical text for the complete
module and collects its complete debug correlation table, including callers
that proceed directly to LLVM/JIT and never request the text. Canonical text
rendering should be lazy if native phase timings show it matters.

Target stamping currently requires LLVM conversion, verification, printing,
reparsing, and another verification because the pinned Pliron LLVM layer does
not expose the raw module needed to set target metadata. This repeated work is
documented and intentional today, but should disappear if the backend API makes
in-place target stamping possible.

### 10. Native specialization and Pliron repeat reachability work

Backend monomorphization starts at the requested entries and produces an
entry-rooted concrete graph. Pliron then performs another reachability walk,
including lifecycle dependencies, before lowering. These policies are not
proven equivalent, so neither walk should be removed blindly. Investigate
having specialization return the authoritative reachable functions and
lifecycle edges so Pliron can consume that result rather than rediscover it.

The implementation order is therefore: eliminate final-only work in discarded
discovery checks; replace global transfer-effect replay; retain elaboration
indexes across rounds; combine request scans; then address duplicate checked
indexes, reusable ownership results, MIR/VM copies, and native-only caches or
lazy artifacts according to their measured phase costs.

## Checked templates and the discovery result (2026-09-20)

One debug-profile run each, on `ceda943` plus uncommitted work, rustc 1.96.1,
verification mode and timing notes off. These are single observations for
comparing counts, not medians, and are not comparable with the release
figures above.

| Program | Total before | With one arena build | Arena builds |
|---|---:|---:|---:|
| `benchmarks/compile/hello.mojo` | 10.58 s | 9.90 s | 3 to 1 |
| `benchmarks/compile/stdlib_heavy.mojo` | 17.83 s | 16.68 s | 3 to 1 |
| `benchmarks/compile/generic.mojo` | 11.10 s | 10.55 s | 3 to 1 |
| `benchmarks/compile/tuple.mojo` | 10.76 s | 10.02 s | 3 to 1 |
| `benchmarks/compile/tstring.mojo` | 10.78 s | 10.08 s | 3 to 1 |

The saving is the arena. `CheckedProgram::new` cost about 0.49 s per round,
0.43 s of it building checked expressions, and two of three builds are gone.

### Body inference by kind

Summed over every check pass of one compilation (three discovery rounds of two
transfer passes each). A body is *generated* when the elaborator lists it
(`GeneratedDeclarations`) or it carries an explicit receiver type. The first
version of this counter tested for a `$` in the name, which a module-qualified
source name such as `__module$std$string$String` carries too. It therefore
reported 3828 "clone" inferences for Hello World, most of them methods of
ordinary bundled structs. The figures below replace it.

| Program | Plain | Template | Generated | Instance-clone checks | Derived | Template bodies reused |
|---|---:|---:|---:|---:|---:|---:|
| `hello.mojo` | 2100 | 1540 | 1320 | 0 | 0 | 375 |
| `stdlib_heavy.mojo` | 2100 | 1530 | 2742 | 2254 | 860 | 415 |
| `generic.mojo` | 2118 | 1517 | 1542 | 474 | 274 | 410 |

- Hello World mints no per-instantiation method clones: a program without its
  own instantiations has none. When measured, its generated bodies were
  members of structs the elaborator then specialized whole (`Tuple$…`, the
  `DType`-keyed ranges, the hasher) and per-call clones, all from
  concrete-only templates; no struct is specialized whole since 2026-10-06.
- `stdlib_heavy.mojo` checks an instance clone 2254 times and derives 860 of
  them, 38%; `generic.mojo` derives 274 of 474, 58%. With scalar getters alone
  (`MethodScalarBody`) those were 242 and 70, and wall time did not move.
  Before `ref self` accessors and the `_mojito_abort` statement they were 586
  and 178, before place arguments 698 and 220, before whole-value
  arguments, vanishing transfers, comparisons, and bound copies 710 and 226,
  and before bound dispatches with arguments, the `hasher.update` and
  `writer.write` builtins, and the method's own hasher binder 802 and 256.
- The `MethodBody` class moves it. Three interleaved runs each against the
  preceding commit, debug profile, no `--timings`: `stdlib_heavy.mojo` 17.78 to
  17.98 s before and 16.91 to 17.12 s after; `generic.mojo` 11.30 to 11.57 s
  before and 10.53 to 10.82 s after. `--timings` itself costs about four
  seconds on `stdlib_heavy.mojo`, before and after, most of it the census.
- `ref self` accessors and reference results move it again, measured the same
  way against c08b3aa: `stdlib_heavy.mojo` 17.64 to 17.74 s before and 16.88
  to 17.07 s after; `generic.mojo` 11.21 to 11.46 s before and 10.53 to
  10.64 s after. Absolute times drift between sessions, so only the
  interleaved pairs compare.
- `ref` locals, receivers reached through a reference, and `mut`/`ref`
  parameters move neither count, so wall time was not measured again. Every
  bundled body that holds one also constructs a struct (the 16 `ref source =
  self` iterator makers), passes a non-scalar, dispatches through a bound, or
  holds a `for` (the 12 `Dict`/`Set` entry loops and `List.extend(Span)`).
  The shapes derive in user structs.
- Whole-value arguments, vanishing transfers, comparisons, and bound copies
  move it again, measured the same way against 5f3d610: `stdlib_heavy.mojo`
  18.66 to 19.07 s before and 17.69 to 18.08 s after; `generic.mojo` 11.81 to
  12.20 s before and 11.05 to 11.32 s after.
- Bound dispatches with arguments and the two builtins add ten derived clone
  bodies per pass to `stdlib_heavy.mojo` (58 of 2742 clone inferences) and
  three to `generic.mojo`, too few to show above session drift, so no
  interleaved pair was taken: three runs after read 17.60 to 17.93 s and
  11.10 to 11.54 s.
- Constructions add 26 derived clone bodies per pass to `stdlib_heavy.mojo`
  (144 of 2742 clone inferences: 860 to 1004 derived) and 36 to
  `generic.mojo` (274 to 310). No interleaved pair was taken; three plain
  runs after read 18.18 to 18.47 s and 11.83 to 12.17 s, in a session whose
  drift is larger than the change, so no wall-time claim is made.
- Whole-value element stores and stores through a mutable-reference getter
  (2026-09-23) move neither count: `stdlib_heavy.mojo` reads 1096 derived
  before and after, `generic.mojo` 340. No bundled body holds `x[i] += …`,
  `Dict._append_new` is kept out by three other constructs, and `Set.insert`,
  the one bundled body the store alone kept out, is not instantiated by
  either benchmark. The shapes derive in user structs
  (`template_method_subscript_store.mojo`).

### Instantiation census and the P0 baseline (2026-09-30)

`mojito run --instantiation-census FILE` counts which mechanism instantiates
each generic body. The full table and its reading are in
[`docs/parametric-mir-plan.md`](parametric-mir-plan.md) §P0. Taken at
`86a25b67` plus the census itself:

| Program | Cloned | Inferred | Derived | Parametric in MIR | Reached from `main` | Instances served |
|---|---:|---:|---:|---:|---:|---:|
| `hello.mojo` | 244 | 21 | 223 | 345 | 0 | 0 |
| `generic.mojo` | 329 | 26 | 303 | 357 | 10 | 10 |
| `stdlib_heavy.mojo` | 666 | 48 | 618 | 385 | 36 | 98 |

These count distinct bodies. The table under "Body inference by kind" counts
visits, one per check pass, so the two do not compare.

The plan's decision D4 measures each stage against this baseline: `total`
from `--timings`, debug profile, `run` on the VM, median of three with the
range, rustc 1.96.1.

| Program | Debug `run` |
|---|---:|
| `hello.mojo` | 11.56 s (10.82–11.66) |
| `generic.mojo` | 11.54 s (11.50–11.72) |
| `stdlib_heavy.mojo` | 15.55 s (15.32–15.82) |

Absolute times drift between sessions, so a stage reruns the commit before it
interleaved with its own, as the measurements above do.

The P3 benchmark, `benchmarks/compile/keyed.mojo`, joins the workload set on
2026-10-02: it mints the `comptime if`, `comptime for`, and type-pack bodies
the three programs above never do. Its census is in the plan's §P0. Taken at
`444f4e03` plus the benchmark, interleaved with Hello World on the same
binary, same measure:

| Program | Cloned | Inferred | Derived | Debug `run` |
|---|---:|---:|---:|---:|
| `hello.mojo` | 232 | 20 | 212 | 8.72 s (8.63–8.74) |
| `keyed.mojo` | 254 | 30 | 224 | 8.85 s (8.85–8.90) |

### The VM on concrete MIR (2026-10-01)

Since stage P1 the VM runs the elaborator's concrete graph instead of erased
generic bodies. Same binary (`d840d827` plus the change), `--erased` against
the default, interleaved, debug profile, `total` from `--timings`, median of
three with the range:

| Program | Erased | Concrete | Ratio |
|---|---:|---:|---:|
| `hello.mojo` | 8.662 s (8.646–8.776) | 8.628 s (8.606–8.685) | 0.996 |
| `generic.mojo` | 9.200 s (9.191–9.325) | 9.143 s (9.142–9.334) | 0.994 |
| `stdlib_heavy.mojo` | 12.081 s (11.979–12.085) | 12.165 s (12.163–12.277) | 1.007 |

The backend phases of `stdlib_heavy.mojo`, the program that instantiates
most:

| Phase | Erased | Concrete |
|---|---:|---:|
| `backend` | 1.294 s | 1.469 s |
| `backend.prepare.drops.elaborate` | 1.078 s | 1.079 s |
| `backend.prepare.elaborate` | — | 0.273 s |
| `backend.prepare.mir_clone` | 0.060 s | 0.008 s |
| `backend.vm` | 0.095 s | 0.041 s |

- Elaboration is the only new cost: 0.004 s, 0.016 s, and 0.273 s for the
  three programs, which reach 5, 29, and 182 concrete functions out of 911,
  1,001, and 1,338 in MIR (`concrete_functions` beside `mir_functions` in
  `--timings`).
- The VM clones and indexes only the reachable graph, so `mir_clone` and
  `vm` shrink.
- Peak RSS falls: 293 → 232 MB, 311 → 248 MB, and 407 → 348 MB.
- The instance row of the census for `stdlib_heavy.mojo` is now 96, not 98: a
  lone static called on a spelled instance names that instance's clone.

### Parameter-expression attributes (2026-09-20)

Value arguments, value defaults, and dependent types became typed canonical
nodes interned per compilation (`docs/notes/param-expr-attributes.md`), and
the checker, elaborator, and native monomorphization now share one compile-time
folder. Release build, `rustc 1.96.1`, Intel i7-10875H, `hyperfine --warmup 1
--runs 5 -N`, baseline `74a3d40`:

| Program | Baseline | After | Peak RSS |
|---|---|---|---|
| `hello.mojo` | 2.032 s ± 0.037 | 1.947 s ± 0.023 | 283.9 MB → 273.2 MB |
| `generic.mojo` | 2.246 s ± 0.059 | 2.136 s ± 0.010 | |
| `tuple.mojo` | 2.065 s ± 0.028 | 2.010 s ± 0.028 | |

- The gain is the elaborator's `eval_infix`, which evaluated each operand up
  to three times before choosing a domain and now evaluates it once.
- `--timings` reports `param_expr.{interned, intern_hits, constant_folds,
  replacements, contexts}`. `generic.mojo` interns 21 nodes over 744
  replacements and 17,231 intern hits in one context; a CTFE subprogram is a
  standalone checker boundary and adds a context of its own.
- These are current-build numbers. The 0.8 s figure earlier in this document
  is historical and is not the baseline.

### Census of method bodies (`stdlib_heavy.mojo`, last discovery round, one pass)

`--timings` reports, per generic body, everything that keeps it from being
captured (`template_census.*`; `MOJITO_TIMING_NOTES=1` names each body).

| | Bodies inferred | Capturable with today's recipes |
|---|---:|---:|
| Generic method templates | 223 | 136 |
| Per-instantiation clones | 260 | 173 |

A derived body is not inferred, so it is not in the census. Of the 411
per-instantiation clone bodies of that pass, 176 derive; the census rows are
the generated generic bodies that were inferred.

What blocks the clone bodies, by how many bodies each reason appears in:

| Reason | Bodies | Sole blocker of |
|---|---:|---:|
| a call-through residue | 39 | 27 |
| `ExplicitDestroyCalls` | 13 | 4 |
| adjustment `TypeName` | 11 | 4 |
| adjustment `EraseCompileTimeArgument` | 8 | 0 |
| `ImplicitConversions` | 7 | 0 |
| adjustment `PointerOriginCast` | 7 | 5 |
| adjustment `BorrowViewResult` | 6 | 6 |

The four reference facts (`ReferenceValueUses`, the `ReferenceResult`
adjustment, `InteriorReferences`, `CopyableReferenceResultReads`),
`SubscriptDescriptors`, `CallPlaceUses`, a transfer summary that vanishes
for plain data, `ConstructionImmutableBinders`, and the return annotation's
re-resolution (the facts once keyed outside the body) have recipes and left
the table; what `effects` still counts is a body that calls its own callable
parameter, or reads such a body's residue. The best single recipe left is
that residue, 27 more clone bodies. Capturable is necessary, not sufficient:
173 bodies are capturable already and are inferred because no class admits
their syntax.

What those bodies are made of is the census's second half
(`template_census.<class>.grammar.<construct>`): every construct a method's
declaration and body hold, whatever class admits it. Among the 260 clone
bodies, the constructs no class admits in any form are the method's own
binders (37) and `raises` with `raise` (21). The rest are admitted only in
part: a `mut` parameter (admitted with no origin clause, though every bundled
one is a writer or a hasher the body calls through its bound), a `ref`
declaration (admitted over a place of `self`, a parameter, a local, or a
reference call), a method call with arguments (150, admitted when every
argument is a closed scalar, a kept place, or a whole value of exactly its
parameter's type), a direct call with arguments (145, admitted as a struct
construction over closed scalars, whole values, or `copy:` of a named
place), a subscript (107, admitted on a pointer slot or as a reference call
on a field), a `^` transfer (115), a non-scalar closed result (90), a string
literal (35, admitted as the `_mojito_abort` message), a `ref self` (admitted
with no receiver origin), and a reference result (admitted when every
`return` hands out a place of `self`).

The remaining cost is body inference of uncovered bodies, about one second per
transfer pass. The `pmir` track of `docs/roadmap.md` carries the entries that attack it,
and `docs/notes/instantiation-from-template.md` is the design record.

The counters are `body_inference.{plain,template,clone}`,
`body_sites.instance_clone`,
`templates.{surviving_trait_bound,validated_keyed,concrete_only}`,
`template_facts_recorded`, `template_capture_incomplete.*`,
`template_derivations.{installed,ineligible,verified,overload_rebinding}`,
`template_bodies.reused`, `template_census.*`, and `arena_builds`.
`MOJITO_TIMING_NOTES=1` names each declaration.

## Where to look first

These were the investigation priorities before the measurement above; the
profile answered them (item 5's MIR lowering, via the HIR clone, dominates;
items 1–3 are cheap today).

1. **Rebuilding the implicit standard library for every command.** The linker
   unconditionally loads `std.prelude`, parses its transitive imports, flattens
   them into the entry program, and rewrites their ASTs. The current `stdlib/`
   contains about 182 KiB of Mojo source; `std/string.mojo` alone is about 70
   KiB. Hello World consequently appears to pay for a substantial source
   program before its own few tokens reach checking. Measure module counts,
   bytes, declarations, and time before considering parsed-module caches,
   checked prelude artifacts, demand-driven imports, or another design. A cache
   must include the compiler version, target-relevant configuration, stdlib
   content, and module search roots in its validity key.

2. **Nested whole-program fixpoints.** `Compiler::compile_linked` first checks an
   elaborated clone of the entire linked AST, then may re-elaborate the original
   linked AST and re-run the checker whenever Tuple, template-string,
   bound-generic, or scalar-range specialization requests grow. There are up to
   six discovery checks. Within every one of those checks, transfer and
   call-through effect discovery can rerun the whole checker up to five times.
   The worst-case structure is therefore roughly thirty full checks before MIR,
   even though normal programs should converge much sooner. Record both round
   counts and per-round time. If this dominates, replace global replay with a
   worklist/incremental scheme, or separate stable declaration registration
   from the bodies that actually require another pass.

3. **Checker cost per pass.** A single checker pass makes several complete
   declaration walks before checking bodies, synthesizes trait-default wrappers
   and rekeys a cloned AST, builds many occurrence-indexed fact maps, and performs overload,
   conformance, substitution, and effect comparisons across a large prelude.
   Some deduplication and stale-effect checks are linear scans of vectors. First
   subdivide declaration registration, body checking, effect validation, and
   explicit-destroy checking; then profile the slow subdivision. Likely remedies
   include indexed candidate sets, interned names/types, memoized conformance and
   substitution queries, and work queues instead of repeated global scans, but
   none should be selected without samples and counters.

4. **Whole-program cloning and allocation traffic.** Linking clones binding and
   namespace maps while recursively rewriting ASTs. Specialization repeatedly
   clones the linked program. Checked-to-HIR lowering retains or clones syntax
   and semantic facts, while ownership dataflow repeatedly clones set-valued
   states at control-flow joins. This can make debug builds particularly painful
   and can amplify every fixpoint above. Use allocation and sampling profiles to
   identify the responsible types before introducing `Arc`, interning, arenas,
   or ownership-oriented API changes.

5. **Post-check fixed costs.** MIR lowering, three ownership analyses, drop
   elaboration, post-drop verification, the final MIR clone into the VM, and VM
   registry construction all operate on the entire linked program. These are
   less likely than the source/checker multiplication to explain a double-digit
   Hello World compile time, but they are easy to isolate. In particular,
   `CompiledProgram::drop_elaborated_mir` clones pre-drop MIR once and
   `Compiler::execute` clones the elaborated MIR again. Remove or share those
   copies only if their measured cost matters.

The modest debug-to-release improvement is consistent with excessive work,
allocation, or poor algorithmic scaling rather than only slow debug VM dispatch.
It is not enough evidence to choose among those causes.

## Timing plan

### 1. Establish a reproducible external baseline *(driver: `scripts/bench-compile`)*

Benchmark the already-built executable, never `cargo run`. Build debug and
release once, then run each case in a fresh process. Report the median and p90
of at least ten measured runs after one unmeasured warm-up, plus minimum,
maximum, user CPU time, system CPU time, and peak RSS. Keep cold-filesystem-cache
measurements separate rather than mixing them into the normal result.

Start with this small matrix:

- an empty `main` and Hello World, to expose fixed cost;
- a small program with no specialization requests;
- one Tuple/template-string program and one inferred generic program, to expose
  discovery rounds;
- a medium stdlib-heavy program, to expose scaling;
- compile-only and compile-plus-VM-execution variants.

Record the commit, dirty-worktree marker, Rust version, build profile, target,
CPU, operating system, backend, input hash, and whether caches were warm. Mojo
numbers should record the same environment and explicitly state whether its
stdlib/compiler caches were warm.

### 2. Extend `--timings` across the production pipeline *(done)*

`--timings` currently reports the frontend as one aggregate for the Pliron path
and then reports native phases. Make it available to ordinary VM `run` and to
compile-only commands, with no output or measurable work when disabled. Keep
human-readable diagnostics off stdout. Emit stable, machine-readable records to
stderr; the existing `timing\t<phase>\t<micros>` convention can be extended with
a hierarchical phase name, round number, and counters.

Use an optional collector owned by the compiler invocation rather than reading
an environment variable at every span. A small RAII span based on
`std::time::Instant` is sufficient initially. It should record inclusive wall
time and invocation count; nested spans let the reporting layer derive self
time. Time failed compilations too, marking the final outcome. Do not put a lock,
global map, formatting, or allocation on the disabled path.

Instrument these boundaries:

```text
total
  input.read
  link
    entry.parse
    prelude.resolve
    module.read_parse              (aggregate and per module in verbose mode)
    import.resolve
    ast.rewrite_flatten
  compile
    template.scan
    discovery.initial.elaborate
    discovery.round[N].request_scan
      tuple | tstring | generic | scalar_range
    discovery.round[N].elaborate
    discovery.round[N].check
      syntax_rekey
      transfer.round[M].declarations
      transfer.round[M].bodies
      transfer.round[M].reference_validation
      explicit_destroy
    hir.lower
    mir.lower
    mir.verify.pre_drop
    ownership.lifetimes
    ownership.loans
  backend.prepare
    drops.elaborate
    pre_elaboration
    mir.verify.post_drop
    vm.mir_clone
    vm.registry_build
  backend.execute
```

The exact nesting may follow crate boundaries, but the report must distinguish
repeated work by round. Avoid a single `checker` timer that conceals five checks.
Native backends should retain their existing detailed phases under
`backend.compile` and use the same collector/output format.

### 3. Add explanatory counters

Time alone cannot distinguish a slow operation from too much input. Collect
cheap counters beside the spans:

- source modules and bytes read, parse tokens, linked declarations, AST nodes,
  and expressions;
- specialization discovery rounds, inner transfer-effect rounds, requests
  found/new/conflicted by kind, and generated declarations;
- structs, traits, functions, checked bodies, overload candidates considered,
  conformance queries and cache hits, and type substitutions;
- HIR blocks/instructions, MIR functions/blocks/instructions/registers, and
  verifier findings;
- ownership functions/blocks/instructions, iterations to each dataflow
  fixpoint, and state-set sizes;
- VM functions/declarations indexed and executed MIR instructions.

Counters must be defined once and tested on tiny fixtures so changes in their
meaning do not silently invalidate historical results.

The elaborator's counters (R771) are its pass statistics, as upstream's
elaborator pass keeps them: `native::mono` returns them in
`SpecializedProgram::counts` (`ElaborationCounts`), and the driver reports
them under `elaborate`, because the elaborator runs on its own thread and
the timing log is thread-local. An *instruction* is one `MirInstr` in a
block at any depth, a region op counting once beside its regions' own,
terminators not counted (`MirFunction::instruction_count`, the same unit
as `mir.lower.instructions`). Before R881 (2026-10-21) the counts read a
body's function-level blocks only; the measurements below dated earlier
use that unit.

- `concrete_instructions`: the instructions of the whole concrete program.
- `generators`: the templates the elaboration instantiated under a name
  other than the template's own (`twice` → `twice$mono$TInt`, `Box.get` →
  `Box$mono$TInt.get`). A body emitted under its own name — `main`, a
  method of a non-generic struct, a body generic only over origins, which
  erase from the instance key — is a plain function, not a generator.
  Unreached generic bodies are not counted.
- `generator_instances`: the instances of those generators.
- `generator_instructions`: each generator's template body as the
  elaborator read it (drop-elaborated), counted once.
- `generator_instance_instructions`: the instructions of every generator
  instance in the concrete output. Its ratio to `generator_instructions` is
  the expansion the pre-elaboration passes attack.
- `concrete_blocks`, `generator_blocks`, `generator_instance_blocks`: the
  same three sizes in blocks, every region's counted
  (`MirFunction::block_count`). A control-flow rewrite that removes blocks
  moves no instruction, so only these show it.

`MOJITO_TIMING_NOTES=1` adds one `generator` note per row:
`<template> <instances> <template instructions> <instance instructions>`.
On a `Box[T]` with `__init__`/`get` and a `twice[T]` called at `Int` and
`String` (2026-10-13, compiled std package): 37 concrete functions of 810
instructions, 17 generators of 191 instructions, 18 instances of 193;
`twice 2 2 4`, `Box.get 1 2 2`, the other 15 rows std's (`List`, `Span`,
`_RawAlloc`, `unsafe_alloc`). The VM and the native lane report the same
numbers. `tests/compiler_test.rs::elaboration_counts_name_generators_and_their_instances`
pins the definitions on the program's own rows, never on std totals.

Three compile benchmarks make instances most of the program (R774,
2026-10-13, compiled std package, debug build). `collections_many.mojo`
drives `List`, `Dict`, `Set`, and `Optional` at a dozen element types,
`nested_generics.mojo` wraps a user `Box` six deep and nests user generics
inside std collections, and `trait_defaults.mojo` calls a two-level trait's
defaults through twelve conformers:

| Fixture | concrete fns | concrete instrs | generators | instances | generator → instance instrs |
|---|---:|---:|---:|---:|---|
| `collections_many` | 834 | 17 499 | 111 | 803 | 2 129 → 16 077 (7.6×) |
| `nested_generics` | 217 | 4 098 | 59 | 194 | 1 136 → 3 117 (2.7×) |
| `trait_defaults` | 185 | 2 058 | 29 | 130 | 323 → 1 165 (3.6×) |

Each compiles at the pin with the same output: the overnight gate's
`benchmarks-mojo-sweep` step runs `scripts/sweep-assets-mojo --benchmarks`,
which fails on any `benchmarks/compile` fixture the pin rejects, Mojito
rejects, or whose stdout differs. Benchmarks keep no burn-down ledger: one
the pin rejects is respelled or deleted.

The corpus record (R772, 2026-10-13) is `benchmarks/instantiation.tsv`,
written by `scripts/instantiation-report`: one row per `assets/ok` and
`benchmarks/compile` fixture with the six size counters above, then a
`total` row over the elaborated fixtures. The file holds no date, hash, or
binary path, so a pass's effect on expansion is `git diff
benchmarks/instantiation.tsv` after rerunning the script, and a rerun on an
unchanged tree diffs empty. The baseline covers 1 018 fixtures, all
elaborated:

| Total | concrete fns | concrete instrs | generators | instances | generator → instance instrs |
|---|---:|---:|---:|---:|---|
| all 1 018 | 42 459 | 893 343 | 16 574 | 26 899 | 241 278 → 396 614 (1.64×) |

`assets/ok` holds 862 289 of the concrete instructions and the eleven
benchmarks 31 054. The ratio of the totals'
`generator_instance_instructions` to `generator_instructions` is the corpus
expansion the `pmir` passes attack. Caveats: the counters come from the
compiled std package (the script drops `MOJITO_STD_PACKAGE` from the child
environment) and the debug binary, the script's default; each fixture is
executed with `run`, the command that reaches the elaborator, with stdin
from `/dev/null`, and a runtime trap keeps its row; and a summed
`generators` is not a count of distinct templates, since every fixture
recounts the std generators it reaches. The rows are the record. The whole
sweep takes about 15 s at the default 8 jobs.

The ratchet (R773) is `scripts/instantiation-report --check`, the overnight
gate's last step (`scripts/run-overnight-gates`, after conformance restores
the featureless debug binary the baseline was recorded with). It writes the
fresh report to `target/instantiation.tsv` and compares the
`concrete_instructions` total over the fixtures **elaborated in both** runs
with the committed baseline's. It fails on *growth* (the shared total went
up), on a *stale* baseline (it went down and the lower number was not
committed, so a later change could give the win back), and on a *lost*
fixture (elaborated in the baseline, `failed` or `timeout` now). Fixtures on
one side only, or newly elaborated, are an inventory note and do not fail.
The raw `total` row is not the compared quantity: it grows with every added
fixture or fixed failure, and shrinks whenever a fixture breaks, so it would
both fail on harmless changes and let a regression pay for real growth.
Per-fixture moves that leave the shared total equal pass, so a pass may shift
instructions between fixtures. The check prints all six shared totals and the
ten largest per-fixture deltas; a change that means to move the total reruns
the script without `--check` and commits `benchmarks/instantiation.tsv`.

### 4. Profile the dominant measured phase *(done for Hello World: `scripts/sample-stacks`)*

After phase timing identifies the large bucket, build release with debug info
and collect a sampling flame graph (`perf`/`samply` on Linux). If clone/drop or
hashing frames dominate, add an allocation profile with heaptrack, DHAT, or an
equivalent allocator profiler. Profile both Hello World and the medium case:
the former finds fixed cost, while the latter exposes poor scaling. Do not begin
with micro-optimizations inferred from source inspection.

### 5. Turn results into budgets and regression tracking

Check in a small benchmark driver that parses the timing stream into one row per
sample and stores results outside correctness-test output. Establish budgets
only after variance is known. Track at least total wall time, link, all checker
rounds combined, MIR/ownership, VM preparation, VM execution, peak RSS, and the
round/corpus-size counters. A change is successful only if repeated external
wall-clock measurements improve without merely moving time between spans.

The first decision point is simple: if link plus initial checking consumes most
of Hello World, prototype a reusable/precompiled prelude boundary. If repeated
rounds consume most of it, attack the nested fixpoints first. If neither does,
the phase report tells us which later component deserves the profiler.

## P4: the driver checks once (2026-10-08)

**Environment.** Baseline commit `b1e6226f` (release binary built at that
commit) against the same tree with the R9 driver change; rustc 1.96.1;
the machine of the sections above; VM backend; `--timings` rows in
microseconds, medians of 5 interleaved runs per binary (`/usr/bin/time`
for peak RSS). `check` is the one check without its arena build in both
columns (the baseline's `discovery.initial.check`; the new span nests the
arena, subtracted here). `round[0]` is the baseline's second elaboration
plus carried check, which no longer exists.

| Program | compile (base → new) | validation | elaborate | check | round[0] | arena | mir.lower | mono elaborate | concrete fns | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| hello | 1126 → 938 (−16.7%) | 51 → 50 | 37 → 37 | 538 → 525 | 169 → 0 | 78 → 77 | 196 → 202 | 145 → 143 | 5 → 5 | 196 → 194 |
| generic | 1118 → 954 (−14.7%) | 50 → 50 | 37 → 38 | 542 → 530 | 166 → 0 | 77 → 77 | 194 → 201 | 145 → 143 | 25 → 25 | 197 → 196 |
| keyed | 1111 → 950 (−14.5%) | 52 → 52 | 37 → 39 | 533 → 528 | 165 → 0 | 76 → 77 | 194 → 204 | 144 → 145 | 58 → 58 | 204 → 201 |
| stdlib_heavy | 1133 → 950 (−16.2%) | 50 → 50 | 37 → 38 | 540 → 523 | 164 → 0 | 77 → 77 | 195 → 203 | 146 → 144 | 165 → 165 | 218 → 215 |

The second elaboration and the carried check are gone, the one check costs
no more than the old initial check, concrete-MIR instance counts are
unchanged, and peak RSS is not above the baseline on any row. The
remaining frontend is the initial check of the bundled library (R11) and
MIR lowering.
## P4: one check, regions kept, constants bound (2026-10-09)

**Environment.** Baseline `target/release/mojito` built at `1200b270`
(the driver half of R9) against the same tree with the rest of R9 (slices
3–6: plain-body regions kept to MIR, module constants as parameter
expressions, validation merged into the one check, the AST CTFE route
deleted); rustc 1.96.1; the machine of the sections above; VM backend;
`--timings` rows in microseconds, 3 interleaved runs per binary (the
middle run; `/usr/bin/time -f %M` for peak RSS). `check` includes the
arena build in both columns.

| Program | compile (base → new) | validation | elaborate | check | arena | mir.lower | mono elaborate | concrete fns | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| hello | 935 → 911 (−2.5%) | 50 → 0 | 38 → 38 | 597 → 621 | 71 → 75 | 196 → 200 | 144 → 144 | 5 → 5 | 199 → 197 |
| generic | 942 → 901 (−4.4%) | 50 → 0 | 37 → 37 | 604 → 615 | 74 → 74 | 200 → 198 | 143 → 141 | 25 → 25 | 201 → 199 |
| keyed | 972 → 918 (−5.5%) | 52 → 0 | 39 → 38 | 624 → 621 | 79 → 75 | 206 → 206 | 144 → 144 | 58 → 58 | 206 → 204 |
| stdlib_heavy | 953 → 905 (−5.0%) | 52 → 0 | 39 → 38 | 610 → 614 | 74 → 77 | 201 → 202 | 145 → 146 | 165 → 165 | 221 → 219 |

The validation pass is gone, the one check costs at most 4% more than
before (every plain body's `comptime if` and `comptime for` is now typed
once and every module constant read is a checked fact), concrete-MIR
instance counts are unchanged, and peak RSS is below the baseline on
every row; D4's 1.20× alarm is not approached.

## P6: the bundled library is a compiled package (2026-10-09)

**Environment.** One release binary built from the R11 tree, run with the
library compiled from source (`MOJITO_STD_PACKAGE=0`, the old pipeline) and
imported from its cached package; rustc 1.96.1; the machine of the sections
above; VM backend; `--timings` rows in microseconds, the middle of 3 runs
per mode (`/usr/bin/time -f %M` for peak RSS). `elaborate` is the backend's
mono elaboration, which now also decodes, drop-elaborates, and verifies each
package body it reaches.

| Program | total (source → package) | compile | check | mir.lower | elaborate | bodies decoded | concrete fns | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| hello | 936 → 140 (−85%) | 707 → 73 | 432 → 32 | 201 → 2 | 2 → 3 | 3 | 5 → 5 | 193 → 75 |
| generic | 940 → 150 (−84%) | 706 → 76 | 430 → 35 | 202 → 3 | 4 → 8 | 16 | 25 → 25 | 195 → 78 |
| keyed | 947 → 172 (−82%) | 705 → 79 | 427 → 36 | 203 → 4 | 10 → 22 | 32 | 58 → 58 | 201 → 83 |
| stdlib_heavy | 973 → 209 (−79%) | 709 → 77 | 430 → 35 | 200 → 3 | 27 → 56 | 70 | 165 → 165 | 216 → 94 |

The check and lowering now cover the program's own bodies only, and the
concrete-MIR instance counts are unchanged. What remains of Hello World's
140 ms is mostly the library's declarations, still read from source: the
prelude's link and parse (33), comptime elaboration (26), the declaration
phases of the check (about 25), and reading the package itself (about 27).
Building the package costs one source-mode compilation of every library
module, once per compiler build (about 1.3 s release, 6 s debug).

### The importer reads no library source (2026-10-09, R545)

Same environment and method as the table above, on the R545 tree. The
importer's link reads each library module's record from the package
(`module::package`) instead of reading, parsing, and resolving its source,
and no package body reaches `prepare`, comptime elaboration, or the check.
The package grows by its 61 module records, about 140 KB of bytecode, to
8.5 MB.

| Program | total (R11 package → now) | link | prepare | comptime elaborate | check | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|
| hello | 140 → 76 (−46%) | 33 → 2.2 | 6.7 → 1.6 | 26 → 9.2 | 32 → 25 | 75 → 54 |
| generic | 150 → 85 (−43%) | 32 → 2.2 | 6.8 → 1.5 | 26 → 9.4 | 35 → 27 | 78 → 56 |
| keyed | 172 → 107 (−38%) | 32 → 2.4 | 6.6 → 1.5 | 26 → 9.9 | 36 → 29 | 83 → 62 |
| stdlib_heavy | 209 → 148 (−29%) | 32 → 2.3 | 6.6 → 1.6 | 26 → 9.7 | 35 → 29 | 94 → 78 |

Linking decodes the 39 records Hello World reaches in about 1.5 ms. Of
Hello World's 76 ms, what scales with the library is now reading the
package (22: the MIR `declarations` section's text decode is 18, R554),
the check's registration of the library's declaration headers (about 20 of
its 25: method signatures 7, the module walk 9, the arena 4; R552), and
preparing and elaborating those headers (11, R553).

### The importer installs the library's declarations (2026-10-10, R552)

Same environment and method as the tables above, on the R545 tree against
the R552 tree, both importing the package; RSS in MB. The package build's
check exports each library module's resolved declarations as a
declaration interface (`checker::interface`), and an importer's check
installs those of the modules its link reached instead of registering the
library's declaration headers, then checks the program's own statements
alone. The package grows by 41 interfaces, 164 KB of bytecode, to 8.6 MB.

| Program | total (R545 → now) | compile | check | check arena | peak RSS MB |
|---|---:|---:|---:|---:|---:|
| hello | 76 → 51 (−33%) | 41 → 16 | 25 → 3.0 | 3.6 → 0.0 | 56 → 48 |
| generic | 89 → 61 (−31%) | 44 → 19 | 28 → 4.1 | 4.9 → 0.2 | 58 → 51 |
| keyed | 106 → 82 (−23%) | 46 → 21 | 29 → 5.1 | 5.1 → 0.5 | 64 → 56 |
| stdlib_heavy | 146 → 122 (−16%) | 45 → 20 | 28 → 5.0 | 4.8 → 0.3 | 81 → 74 |

Of Hello World's 3 ms check, installing the 39 reached interfaces into the
one transfer pass is 1.0 ms (R555) and answering every installed struct's
conformances again 1.3 ms (`check.conformances`, R556); decoding the
interfaces is 1.5 ms more, before the check (`package.interfaces`, R555).
What scales with the library is now reading the package (R554) and
preparing and elaborating the library's headers (R553).

### The importer resolves a package declaration on first lookup (2026-10-10, R554)

Same environment and method as the tables above, on the R552 tree against
the R554 tree, both importing the package; medians of 5 interleaved runs,
milliseconds, RSS in MB. The package stores each MIR struct and function
declaration as its own bytecode record instead of one MIR text artifact
(893 KB of text become 164 KB), and the importer's lowering attaches the
package's declarations instead of copying all of them into its program:
every phase looks a declaration up by name and decodes it the first time.
Hello World decodes 3 of the 942 declarations. The package shrinks from
8.6 MB to 8.0 MB.

| Program | total (R552 → now) | declarations read | mir.lower | drops | mono elaborate | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|
| hello | 52.0 → 32.9 (−37%) | 18.4 → 0.0 | 1.4 → 0.2 | 1.0 → 0.5 | 2.4 → 1.9 | 47.8 → 38.2 |
| generic | 60.5 → 41.4 (−32%) | 18.0 → 0.0 | 2.1 → 0.9 | 1.3 → 0.7 | 8.2 → 7.7 | 50.2 → 41.0 |
| keyed | 81.1 → 60.9 (−25%) | 17.9 → 0.0 | 3.0 → 1.7 | 1.7 → 1.2 | 22.2 → 21.8 | 55.2 → 45.8 |
| stdlib_heavy | 122.7 → 101.1 (−18%) | 17.9 → 0.0 | 2.6 → 1.4 | 1.5 → 0.9 | 55.6 → 55.4 | 72.4 → 61.6 |

Reading the package was then the whole-file read (about 3.5 ms for 8 MB,
of which 7.4 MB was body text that mostly never decodes) and its text
interface record (0.3 ms); R557 below closed both. What scaled with the
library was otherwise preparing and elaborating the library's headers
(R553, since landed).

### The importer maps a bytecode package (2026-10-10, R557)

Same environment and method as the tables above, on the R553 tree against
the R557 tree, both importing the package; medians of 5 interleaved runs,
milliseconds, RSS in MB. The package is now a directory followed by
bytecode records: each body is `postcard` of its `MirFunction` and its
file-id locations instead of MIR text, and the interface is `postcard`
too. The importer maps the file (`memmap2`) and decodes the directory, the
interface, and nothing else until a lookup asks. The package shrinks from
8.0 MB to 1.7 MB.

| Program | total (R553 → now) | package read → map | interface | mono elaborate | peak RSS MB |
|---|---:|---:|---:|---:|---:|
| hello | 19.5 → 16.5 (−15%) | 3.29 → 0.01 | 0.28 → 0.12 | 1.9 → 1.5 | 38.1 → 30.7 |
| generic | 31.1 → 24.1 (−23%) | 3.46 → 0.01 | 0.30 → 0.12 | 8.4 → 5.7 | 40.7 → 33.5 |
| keyed | 49.2 → 40.7 (−17%) | 3.29 → 0.02 | 0.29 → 0.12 | 21.8 → 17.1 | 45.9 → 38.7 |
| stdlib_heavy | 91.2 → 75.4 (−17%) | 3.15 → 0.01 | 0.28 → 0.12 | 55.8 → 44.0 | 63.5 → 55.7 |

Decoding the directory (`package.directory`) is 0.23 ms: its 1,900 record
names and 874 body entries, which replaces the line-framed reader's
untimed scan and index building (`frontend` self time falls from 3.3 to
2.9 ms with the directory in it). The rest of the gain is the body decode
inside mono elaboration, which no longer parses MIR text (about 8× faster
per body). Of what remains untimed in `frontend`, about 1.2 ms was
digesting the library's source to key the package cache (R562, below).

### The importer resolves a declaration interface on first lookup (2026-10-10, R555)

Same environment and method as the tables above, on the R557 tree (with
R555's overload-set fix) against the R555 tree, both importing the
package; medians of 5 interleaved runs, milliseconds, RSS in MB. The
package build exports one declaration interface per library declaration
under a directory of every table key each holds, beside a small record per
module (overload sets, module-scope compile-time facts, elaborated
constants). The process reads the directory once; a compilation decodes
the module records of the modules its link reached, and a declaration's
record the first time a lookup its own tables miss names one of its keys.
Nothing is installed: every checker pass and the conformance oracle read
the same decoded records. The package grows from 1.71 to 1.74 MB.

| Program | total (R557 → now) | package.interfaces | elaborate.indexes | check | check.conformances | peak RSS MB |
|---|---:|---:|---:|---:|---:|---:|
| hello | 17.0 → 14.1 (−17%) | 2.27 → 0.22 | 1.61 → 0.16 | 3.15 → 3.86 | 1.50 → 3.58 | 29.5 → 28.2 |
| generic | 23.6 → 21.1 (−11%) | 2.14 → 0.20 | 1.53 → 0.19 | 3.88 → 4.70 | 1.44 → 3.20 | 32.0 → 31.2 |
| keyed | 40.9 → 38.6 (−6%) | 2.07 → 0.20 | 1.53 → 0.22 | 5.06 → 5.83 | 1.41 → 3.13 | 37.4 → 36.9 |
| stdlib_heavy | 75.8 → 73.6 (−3%) | 2.05 → 0.20 | 1.55 → 0.22 | 5.01 → 5.91 | 1.43 → 2.88 | 54.1 → 54.7 |

The check's install span (0.8 ms) and the oracle's (1.4 ms) are gone, and
`package.interfaces` now decodes only the reached module records. Each
program decodes 63 or 64 declaration records, almost all of them the
reached structs: the conformance rows still walk every struct
(`check.conformances`, R556), which is where their decoding now lands.
With R556, a compilation would decode only the declarations it names.

### The importer answers only its own structs' conformances (2026-10-10, R556)

Same environment and method as the tables above, on the R558 tree against
the R556 tree, both importing the package; medians of 5 interleaved runs,
milliseconds. An importer's check answers conformance rows for its own
structs only: a package struct's rows for the traits the package asked
are in its MIR declaration, and `mir::link` joins an empty row for each
trait the importer declares afresh. The walk over every reached struct,
and the decoding of every struct's interface it caused, are gone.

| Program | total (R558 → now) | check | check.conformances | interfaces decoded |
|---|---:|---:|---:|---:|
| hello | 12.4 → 8.8 (−29%) | 3.57 → 0.27 | 3.32 → 0.01 | 63 → 0 |
| generic | 20.6 → 16.7 (−19%) | 4.69 → 1.48 | 3.25 → 0.04 | 63 → 3 |
| keyed | 38.5 → 34.8 (−10%) | 6.00 → 2.80 | 3.20 → 0.03 | 64 → 4 |
| stdlib_heavy | 72.6 → 69.9 (−4%) | 5.83 → 2.94 | 2.85 → 0.03 | 63 → 6 |

A program whose clause names a builtin or package trait the package never
asked (`Absable`) still answers it for every reached package struct,
about 1.7 ms (`check.conformances.package_rows`); R564 removed it.

### The importer reads no library source to find its package (2026-10-10, R562)

Same environment as the tables above, on the R562 tree against the R556
tree, both importing the package; medians of 9 runs, milliseconds. The
compiler build's identity now digests the library's source (root
`build.rs`), so the package cache is keyed by that identity and the
library's root, and the importer no longer walks and hashes the library's
61 files to find its package: `strace` shows 2 library-path accesses (the
prelude probe) where there were 157.

| Program | total (R556 → now) | `frontend` self | hyperfine wall |
|---|---:|---:|---:|
| hello | 9.0 → 7.8 (−13%) | 2.84 → 1.68 | 10.0 → 9.4 |
| generic | — | — | 18.0 → 16.9 |

### The importer reads a package struct or function resolved (2026-10-10, R559)

Same environment as the tables above, on the R559 tree against the R565
tree (`a9e27aaf`), both importing the package; `hyperfine` means of 40
runs and `--timings` medians of 9, milliseconds. An importer's
elaboration reads a package struct, function, and `def` parameter list
from the declaration interfaces, and its `prepare` reads the package's
SIMD aliases from its elaborated constants, so a module record keeps only
its trait declarations (6 of the 220 Hello World's link replayed). The
package shrinks from 1.74 to 1.64 MB.

| Program | hyperfine wall | package.module | elaborate | elaborate.indexes |
|---|---:|---:|---:|---:|
| hello | 8.2 → 6.1 (−26%) | 1.26 → 0.11 | 0.32 → 0.23 | 0.15 → 0.07 |
| generic | 16.0 → 15.3 (−4%) | 1.32 → 0.11 | 0.60 → 0.50 | 0.18 → 0.08 |
| keyed | 34.2 → 32.9 (−4%) | 1.24 → 0.10 | 0.80 → 0.70 | 0.18 → 0.09 |
| stdlib_heavy | 71.0 → 69.7 (−2%) | 1.27 → 0.10 | 0.65 → 0.55 | 0.20 → 0.10 |

The trait declarations stayed in the records until a conformer's wrapper
was built from the trait's resolved signature (R568, below).

### The importer's link replays no declaration (2026-10-10, R568)

Same environment as the tables above, on the R568 tree against the R559
tree (`a2676e53`), both importing the package; `hyperfine` means of 3 × 60
interleaved runs, and `--timings` over 15 interleaved runs. A conformer's
wrapper for a trait default is built by the check from the default's
resolved signature, which the trait's declaration interface records, so a
module record holds no declaration and Hello World's link replays none (6
before). The change is below the noise: the six bodiless traits it no
longer decodes, stamps, or partitions cost about 20 µs of
`package.module`, which the check's wrapper planning spends again. The
package grows from 1.632 to 1.640 MB, the defaults' recorded signatures.

| Program | hyperfine wall | package.module |
|---|---:|---:|
| hello | 6.4 / 6.9 / 6.2 → 6.4 / 6.7 / 6.2 | 0.12 → 0.10 |
| generic | 15.8 → 14.7 | — |
| keyed | 33.2 → 33.6 | — |
| stdlib_heavy | 69.6 → 70.6 | — |

### A package records each library declaration once (2026-10-10, R560)

Same environment as the tables above, on the R560 tree against the R568
tree (`2428929e`), both importing the package from their own cache;
`hyperfine` means of 3 × 60 interleaved runs, and `--timings` medians of
9, milliseconds. Each library declaration is one record — its MIR
declarations and effect summaries, then the checker rows they do not say —
under one directory, where the package held a MIR declaration record per
struct and function and, apart, a checker interface blob with its own
directory. The checker rows shrink from 157 to 79 KB (a struct's
parameters, fields, destructors, and obligation, every method signature,
and every `def` type now come from the MIR declaration), and the package
from 1.640 to 1.560 MB. `package.interface` no longer decodes the effect
summaries; `package.directory` now also decodes the declaration
directory the check used to decode on its own, outside any span.

| Program | hyperfine wall | package.directory | package.interface |
|---|---:|---:|---:|
| hello | 6.7 / 7.3 / 7.1 → 6.5 / 6.3 / 6.9 | 0.37 → 0.55 | 0.17 → 0.09 |
| generic | 15.8 → 15.3 | — | — |
| keyed | 34.2 → 33.2 | — | — |
| stdlib_heavy | 70.2 → 69.7 | — | — |

### An importer copies nothing in when a compilation starts (2026-10-10, R563)

Same environment as the tables above, on the R563 tree against the R560
tree (`83809dfd`), both importing the package from their own cache;
`--timings` medians of 15 interleaved runs, milliseconds. A declaration's
record now holds the overload facts of its names, its module constant as
the build's elaboration resolved it, and its module-scope compile-time
facts, and the check, the elaborator, and MIR answer a name their own
tables miss from the reached declaration owning it, decoded on first
lookup. `package.interfaces` used to decode every reached module's
checker record (31 for Hello World) and merge their overload sets; it now
only marks the reached modules. The elaborator no longer copies every
reached constant into its environment, so `elaborate` loses that copy and
`elaborate.indexes` the oracle's merged-set clone. A program that reads a
package constant decodes that constant's record (one more for the probe
and `hashlib_keyed_ahasher`). The package grows from 1.551 to 1.566 MB:
each declaration's record carries its own overload facts.

| Program | total | package.interfaces | elaborate | elaborate.indexes |
|---|---:|---:|---:|---:|
| hello | 4.40 → 4.02 | 0.19 → 0.02 | 0.27 → 0.11 | 0.08 → 0.05 |
| package_constant_in_own_signature | 19.5 → 19.1 | 0.19 → 0.02 | 0.46 → 0.25 | 0.10 → 0.07 |
| hashlib_keyed_ahasher | 28.1 → 27.9 | 0.18 → 0.02 | 0.29 → 0.18 | 0.07 → 0.04 |
| path_operations | 185.3 → 185.5 | 0.19 → 0.02 | 0.39 → 0.27 | 0.07 → 0.05 |

### A struct's conformance is decided when a clause asks it (2026-10-10, R564)

Same environment as the tables above, on the R564 tree against the R563
tree (`5bcfc9ed`), both importing the package from their own cache;
`--timings` medians of 15 interleaved runs, milliseconds. A struct's
declaration carries its declared conformances and its lifecycle rows, and
a trait's its parents; the elaborator decides any other conformance from
them when a clause asks it. No compilation answers a struct for a trait
ahead of use, so the importer's walk over every reached package struct for
a trait the package never asked (`check.conformances.package_rows`), and
the decoding of every reached struct's interface it caused (60 interfaces
for the fixture below, now 3), are gone. The package grows from 1.566 to
1.568 MB: every library struct carries its declared list and seven
lifecycle rows instead of a row per trait the library asked.

| Program | total | check | check.conformances |
|---|---:|---:|---:|
| hello | 3.97 → 3.79 | 0.15 → 0.13 | 0.01 → 0.00 |
| conforms_to_user_and_unasked_traits | 24.4 → 20.8 | 5.43 → 1.20 | 4.21 → 0.02 |

## Dead generators are removed before the pipeline (2026-10-14, R778)

`eliminate-dead-symbols`, upstream's `EliminateDeadSymbols`, is the first
pre-elaboration pass: it removes the program's own parametric bodies that no
concrete function or entry root reaches (`docs/architecture.md`
§Pre-Elaboration Pipeline). Release binary, 16 cores, `--timings` medians
of interleaved runs with `MOJITO_PRE_ELABORATION` on and off, milliseconds.

**The corpus report is unchanged by design.** `scripts/instantiation-report`
over all 1 020 fixtures writes the same file with the pipeline on and off:
every counter measures what the elaborator reached, and the pass removes
only bodies it never reaches. In the default package mode the program's own
functions are all a fixture has, and a fixture uses nearly all of them: the
pass removes 13 bodies (144 instructions) of 7 068 across the corpus, in 7
fixtures, and none in any compile benchmark. There it costs 0.01–0.17 ms
per program, within noise of every total.

**Source mode** (`MOJITO_STD_PACKAGE=0`, three runs) is where dead bodies
exist: std's generics a program does not reach. The pass removes a quarter
of the program's functions, and the post-drop verify no longer walks them.
The walk (`mir::SymbolTable`, which visits every string a body serializes)
costs about what the verify saves; the gain is the time every later
per-body pass (R779 onward) would spend on these bodies.

| Benchmark | own functions | removed (instructions) | pass | post-drop verify on / off | total on / off |
|---|---:|---:|---:|---:|---:|
| `hello` | 763 | 195 (3 300) | 5.3 | 17.5 / 20.9 | 595 / 593 |
| `generic` | 768 | 195 (3 300) | 5.4 | 17.4 / 20.7 | 603 / 593 |
| `collections_many` | 767 | 86 (1 454) | 6.7 | 20.2 / 21.7 | 797 / 785 |
| `nested_generics` | 768 | 136 (2 150) | 6.3 | 19.2 / 21.1 | 657 / 645 |
| `stdlib_heavy` | 768 | 136 (2 150) | 6.0 | 18.6 / 21.2 | 629 / 623 |
| `trait_defaults` | 811 | 195 (3 300) | 5.6 | 17.8 / 21.3 | 624 / 617 |

The totals are within the spread of three-run medians (with the pass on,
`empty` and `keyed` ran faster). R835 would make the walk visit only typed
symbol references.

## Symbol uses follow typed references (2026-10-14, R835)

`mir::SymbolTable` now looks up only the strings a field encodes as a
symbol reference (`mojito_types::symbol_ref`), rather than every string a
body serializes. The walk still visits the whole serialized value, as
MLIR's `getSymbolUses` walks every attribute; it skips the hash lookups for
locals, fields, keywords, and literals. Source mode (`MOJITO_STD_PACKAGE=0`),
release binary, `--timings` medians of five interleaved runs against the
same tree looking up every string, milliseconds; the pass column sums its
four rounds.

| Benchmark | removed functions old / new | pass old / new | total old / new |
|---|---:|---:|---:|
| `hello` | 190 / 204 | 19.6 / 16.8 | 673 / 674 |
| `generic` | 190 / 204 | 19.5 / 16.7 | 680 / 674 |
| `collections_many` | 81 / 95 | 25.3 / 20.9 | 925 / 922 |
| `nested_generics` | 131 / 145 | 23.7 / 19.8 | 742 / 740 |
| `stdlib_heavy` | 131 / 145 | 23.0 / 19.0 | 724 / 712 |
| `trait_defaults` | 190 / 204 | 20.2 / 17.4 | 709 / 702 |

The 14 more bodies are `TString`'s and `ParameterList`'s methods and
`_ParameterListIter`'s, which a parameter binder or a field spelled by name
kept live. Every executable fixture prints, fails, and exits the same in
source mode; in package mode the live sets are identical.

## Canonicalize removes unreachable blocks and dead instructions (2026-10-14, R779)

`canonicalize`, upstream's `Canonicalizer`, is the second pre-elaboration
pass: it erases blocks the entry does not reach and instructions with no
effect but reading whose results nothing reads (`docs/architecture.md`
§Pre-Elaboration Pipeline). `scripts/instantiation-report` over all 1 020
fixtures, debug binary, with `MOJITO_PRE_ELABORATION=0` for *before*:

| | before | after | change |
|---|---:|---:|---:|
| corpus `concrete_instructions` | 892 673 | 888 654 | −4 019 (−0.45 %) |
| corpus `generator_instructions` | 241 048 | 239 094 | −1 954 (−0.81 %) |
| corpus `generator_instance_instructions` | 396 159 | 392 282 | −3 877 (−0.98 %) |
| `collections_many` instance instructions | 16 043 | 15 982 | −61 |
| `nested_generics` instance instructions | 3 113 | 3 092 | −21 |
| `stdlib_heavy` instance instructions | 2 779 | 2 761 | −18 |
| `trait_defaults` instance instructions | 1 165 | 1 161 | −4 |

**The effect is small, and that is what lowering leaves.** In the std
bodies `empty.mojo` reaches (3 612 blocks, 19 805 instructions), only 2
blocks are unreachable, both empty, and 21 instructions are dead: 20
`const none` left by statement-position intrinsics and one `place.load`.
An unreachable block holds no counted instruction (terminators are not
counted), so only the dead instructions move the counters. Nearly every
unread result belongs to an instruction with an effect the pass keeps: 970
`interiors.invalidate` and 250 `loans.establish` markers, and 372 call
results. Drops define no result and are never unread. The pass is mainly
the frame R780's copy patterns and R781's decided branches join.

The committed baseline had drifted from HEAD before this change (it held
1 018 fixtures and 893 343 concrete instructions); the rerun committed here
absorbs that. The pass's program-seam counters are
`pre_elaboration.canonicalize.erased_blocks` and `.erased_instructions`;
package bodies are finished on the elaborator's thread, so their counts do
not reach `--timings`.

## Canonicalize forwards identity copies (2026-10-14, R780)

`canonicalize` now folds a `value.copy` whose source and destination are
typed alike by a scalar, pointer, `None`, or SIMD-lane type
(`MirInstr::forwarded_operand`, `types::copy_is_identity`), so its readers
name the source and the copy is gone from every instance.
`scripts/instantiation-report` over all 1 020 fixtures, debug binary;
*before* is R779's committed baseline:

| | before | after | change |
|---|---:|---:|---:|
| corpus `concrete_instructions` | 888 654 | 872 283 | −16 371 (−1.84 %) |
| corpus `generator_instructions` | 239 094 | 233 791 | −5 303 (−2.22 %) |
| corpus `generator_instance_instructions` | 392 282 | 383 656 | −8 626 (−2.20 %) |
| `collections_many` instance instructions | 15 982 | 15 784 | −198 |
| `nested_generics` instance instructions | 3 092 | 3 029 | −63 |
| `stdlib_heavy` instance instructions | 2 761 | 2 718 | −43 |
| `trait_defaults` instance instructions | 1 161 | 1 143 | −18 |

**Copies were the commonest slack lowering leaves.** Nearly every value
read (`place.load`, `ref.read`, `field.get`, `index.get`) is followed by a
`value.copy` at the copy-lifecycle boundary, and every one of them has a
single reader. `collections_many`'s concrete MIR (`emit-mir`, source-mode
std) holds 190 copies with the pass off and 35 with it on. The concrete
total falls further than the instance total because the pass also runs on
the program's and the package's concrete bodies.

**What stays.** A copy of a struct or of a type parameter may run
`__copyinit__`, so it stays on the generator; a type-parameter copy an
instance makes trivial folds in that instance (R840), and so does a
dispatched `copy()` (R839). A copy of a register-passable trivial
aggregate (vector SIMD, `StringLiteral`, a `TrivialRegisterPassable`
struct) now folds too (R838, below); a tuple copy stays, as upstream's
`Tuple.__init__(copy=)` call does. A widening copy
(`IntLiteral` into `Int`) stays because forwarding it would change its
readers' operand type.

The program-seam counter is `pre_elaboration.canonicalize.forwarded_copies`
(4 on a small probe); package bodies are finished on the elaborator's
thread, so their counts do not reach `--timings`.

## SCCP folds constants that do not depend on a parameter (2026-10-14, R781)

`sccp`, upstream's `SCCP`, runs after the first `canonicalize`, and a
second `canonicalize` after it folds each `branch` it decides, as the pin's
second pre-elaboration round runs `SCCP, Canonicalizer`
(`docs/architecture.md` §Pre-Elaboration Pipeline).
`scripts/instantiation-report` over all 1 020 fixtures, debug binary;
*before* is R780's committed baseline:

| | before | after | change |
|---|---:|---:|---:|
| corpus `concrete_instructions` | 872 283 | 802 472 | −69 811 (−8.00 %) |
| corpus `generator_instructions` | 233 791 | 218 153 | −15 638 (−6.69 %) |
| corpus `generator_instance_instructions` | 383 656 | 358 548 | −25 108 (−6.54 %) |
| `collections_many` instance instructions | 15 784 | 14 725 | −1 059 |
| `nested_generics` instance instructions | 3 029 | 2 806 | −223 |
| `stdlib_heavy` instance instructions | 2 718 | 2 528 | −190 |
| `trait_defaults` instance instructions | 1 143 | 1 094 | −49 |

**Almost all of it is literal materialization.** Lowering writes every
integer or float literal as an `IntLiteral` or `FloatLiteral` `const`
followed by a `literal.materialize` to its machine type. `sccp` folds the
pair to one `const int(…)` or `const float(…)`, and `canonicalize` then
erases the literal. In `collections_many`'s MIR (`emit-mir`, pipeline off
and on) the pass takes `literal.materialize` from 1 395 to 24, `unary`
from 98 to 62, `binary` from 1 686 to 1 674, and `branch` from 949 to 946.
Real arithmetic and decided branches are rare because values cross blocks
through variable slots, not registers; `Mem2Reg` would widen what the
lattice sees. Of the 24 materializations left, none targets `UInt`: 17
target a concrete scalar SIMD type (folded by R841, below), 4 a SIMD type
whose dtype is a parameter (`_StridedRange`/`_ZeroStartingRange`'s
`dtype`), and 3 `Int` over a `var.use` operand, which needs `Mem2Reg`.
Operators over a parameter read are folded in each instance by the
post-elaboration pipeline (R840, below).

The program-seam counters are `pre_elaboration.sccp.folded_constants` and
`pre_elaboration.canonicalize.folded_branches` (9 and 3 on a small probe);
both `canonicalize` runs count into the one span.

## The elaborator's output is folded per instance (2026-10-14, R840)

`sccp` then `canonicalize` now run over every concrete function the
elaborator emits, as upstream's per-function `'kgen.func'` pipeline runs
`SCCP` and `Canonicalizer` after `ElaborateGenerators`
(`docs/architecture.md` §Post-Elaboration Pipeline).
`scripts/instantiation-report` over all 1 020 fixtures, debug binary;
*before* is R781's committed baseline:

| | before | after | change |
|---|---:|---:|---:|
| corpus `concrete_instructions` | 802 472 | 794 792 | −7 680 (−0.96 %) |
| corpus `generator_instance_instructions` | 358 548 | 351 199 | −7 349 (−2.05 %) |
| `collections_many` concrete instructions | 16 035 | 15 905 | −130 |
| `stdlib_heavy` concrete instructions | 3 311 | 3 275 | −36 |
| `nested_generics` concrete instructions | 3 701 | 3 697 | −4 |
| `trait_defaults` concrete instructions | 1 888 | 1 884 | −4 |

679 fixtures shrank and none grew; the largest drops are
`hashlib_values` (−248), `comptime_display_binding_reads` (−185), and
`template_keyed_runtime_if` (−171). Generator counts and template sizes do
not move: the pipeline rewrites instances only.

**Most of it is copies.** `collections_many`'s counters are
`folded_constants` 3, `folded_branches` 3, `erased_blocks` 12,
`forwarded_copies` 36, and `erased_instructions` 19. The pre-elaboration
`canonicalize` already forwarded every copy whose generator type is an
identity copy, so each copy forwarded here is one whose type substitution
made trivial: a `T`-typed `value.copy` instantiated at a scalar. A
dispatched `x.copy()` stayed a method call until R839. Parameter arithmetic does fold (a probe's
`f[n: Int]` instance drops its `mul`, `add`, `gt`, and `branch` to one
`const` and a `jump`), but in the corpus values cross blocks through
variable slots, which `sccp` does not see until `Mem2Reg` exists.

**Cost.** On `collections_many`, release binary, the `post_elaboration`
span took about 26 ms: `sccp` 2.5 ms, `canonicalize` 4.9 ms, and the
whole-program re-admission (`ConcreteMir::verified`) the remaining
18 ms, against about 121 ms of elaboration. Debug: 142 ms, of which the
passes are 40 ms, against about 930 ms. R844 re-verifies only the
functions a pass changed; see the next section.

The counters are `elaborate.post_elaboration.sccp.folded_constants` and
`elaborate.post_elaboration.canonicalize.{folded_branches, erased_blocks,
forwarded_copies, erased_instructions}`. A folded producer's in-place
binding (`var.store … in_place: true`) is cleared by `sccp` itself, since
a `const` is no call-family producer; instances hit that case constantly,
and generators could have.

## Post-elaboration re-admission verifies changed bodies only (2026-10-14, R844)

Each `BodyPass` now reports whether it changed its body, and
`ConcreteMir::rewrite_bodies` re-verifies only those bodies in a release
build. On `collections_many`, release binary, three runs each, same
machine and session:

| | before | after |
|---|---|---|
| `post_elaboration` inclusive | 31.0–32.3 ms | 16.4–17.1 ms |
| `post_elaboration` self (re-admission, recount) | 18.4–19.4 ms | 3.8–4.0 ms |
| `sccp` | 3.4–3.5 ms | 3.2–3.6 ms |
| `canonicalize` | 9.2–9.5 ms | 9.3–9.5 ms |

759 of the 844 concrete functions are unchanged
(`elaborate.post_elaboration.unchanged_functions`). The passes themselves
now cost three times what re-admission does.

## Scalar SIMD and `UInt` literal materializations fold (2026-10-14, R841)

`sccp` now folds a literal materialized to `UInt` or a one-lane SIMD type
to a typed `Const::Value` (`docs/architecture.md` §Pre-Elaboration
Pipeline). In `collections_many`'s MIR (`emit-mir`, pipeline on)
`literal.materialize` falls from 24 to 7: the 4 over a parameter dtype and
the 3 over a `var.use`. The 17 folded sites sit in bundled-library bodies
(14 in `_c_stat.__init__`), which are finished on the elaborator's thread,
so `pre_elaboration.sccp.folded_constants` stays 23; the per-instance
`elaborate.post_elaboration.sccp.folded_constants` goes from 3 to 4, and
`concrete_instructions` from 15 905 to 15 904 (debug binary, `--timings`).
The corpus-wide `scripts/instantiation-report` rerun is left to the
nightly gate.

## Operators over `UInt` and scalar SIMD constants fold (2026-10-14, R846)

`sccp` now folds operators over `UInt` and one-lane SIMD constants, by the
lane rule the VM's `simd_binop` shares, and a one-lane `simd.make` of a
literal (`docs/architecture.md` §Pre-Elaboration Pipeline). On a probe of
twelve scalar operator expressions (`UInt8(250) + 10`, `Float32(0.1) +
Float32(0.2)`, `UInt8(3) < UInt8(4)`, …) `pre_elaboration.sccp.folded_constants`
rises from 6 to 44, and `main` keeps no `binary`, `unary`, or `simd.make`
but the three over `UInt(7)`, a constructor call. On `collections_many` it
rises from 23 to 51, the post-elaboration count stays 4, and
`concrete_instructions` falls from 15 904 (R841's figure) to 15 857 (debug
binary, `--timings`). The corpus-wide `scripts/instantiation-report` rerun is
left to the nightly gate.

## A scalar construction from a literal folds (2026-10-14, R854)

`UInt(7)`, `Int(7)`, `Float64(2.5)`, and `UInt()` now lower as the literal's
materialization rather than a builtin `call`, so `sccp` folds them and the
operators over them. On `print(UInt(7) // 2, UInt(7) > 3)`, `main` keeps
only two constants and the `print`: none of the three `UInt(7)`
instructions R846's section names survives. On `collections_many`,
`pre_elaboration.sccp.folded_constants` rises from 51 to 54, the
post-elaboration count stays 4, and `concrete_instructions` falls from
15 857 to 15 852 (debug binary, `--timings`). The corpus-wide
`scripts/instantiation-report` rerun is left to the nightly gate.

## Canonicalize threads pass-through blocks (2026-10-14, R836)

`canonicalize` now threads each edge into an empty block that only jumps
on, folds a branch whose arms name one block to a jump, and merges a block
a jump alone reaches into its predecessor (`docs/architecture.md`
§Pre-Elaboration Pipeline). These stand in for upstream's structured
control flow: its generators hold `if` and loops as `hlcf` region ops, so
a join or a latch is no block there. The elaborator now counts blocks
(§3). `scripts/instantiation-report` over all 1 021 fixtures, debug
binary; *before* is the tree without the rewrites, with the counters:

| | before | after | change |
|---|---:|---:|---:|
| corpus `concrete_blocks` | 139 119 | 124 655 | −14 464 (−10.4 %) |
| corpus `generator_blocks` | 39 885 | 37 321 | −2 564 (−6.4 %) |
| corpus `generator_instance_blocks` | 65 194 | 56 712 | −8 482 (−13.0 %) |
| corpus `concrete_instructions` | 787 273 | 787 273 | 0 |
| `collections_many` concrete blocks | 2 452 | 2 228 | −224 |
| `stdlib_heavy` concrete blocks | 515 | 475 | −40 |
| `nested_generics` concrete blocks | 604 | 566 | −38 |
| `trait_defaults` concrete blocks | 311 | 302 | −9 |

1 019 fixtures lost blocks and none gained any; no instruction count
moved. The std package bodies `emit-mir benchmarks/compile/empty.mojo`
prints fall from 3 608 blocks to 3 085, and of their 505 empty blocks
ending in a `jump` 19 are left: 18 entry blocks and one `comptime for`
body target, which the rewrites leave alone. The committed baseline's
instruction totals also move from R840's 794 792 to 787 273, the
unrecorded effect of R841–R856.

The counters are `canonicalize.threaded_branches`,
`canonicalize.folded_successors`, and `canonicalize.merged_blocks`, beside
`erased_blocks`, which counts the blocks they leave unreachable. On
`collections_many` the post-elaboration run threads 6 edges and merges 26
blocks; the pre-elaboration work lands in std package bodies, finished on
the elaborator's thread, so its counts do not reach `--timings`.

## Canonicalize forwards copies of register-passable trivial values (2026-10-14, R838)

`canonicalize` now forwards a `value.copy` of a vector SIMD, a
`StringLiteral`, or a struct that declares `TrivialRegisterPassable`
unconditionally (`MirDeclarations::copy_is_identity`), as upstream's parser
loads such a value into an SSA value its copies are. Such a copy is
forwarded only when it is its operand's sole reader, and Pliron never
aliases a load of such a type to its place, so the forwarded register is
the value read, not a view of the place. `scripts/instantiation-report`
over the 1 021 fixtures that predate the change, debug binary:

| | before | after | change |
|---|---:|---:|---:|
| corpus `concrete_instructions` | 787 273 | 787 247 | −26 |
| corpus `generator_instructions` | 216 720 | 216 714 | −6 |
| corpus `generator_instance_instructions` | 349 092 | 349 075 | −17 |

Ten fixtures moved, all SIMD-heavy (`simd_parameter_default_construction`
−6 concrete instructions); none grew. The effect is small because lowering
already makes no copy for `var q = p` or a read argument: the copies left
are a field read returned, stored, or passed owned. On
`assets/ok/trivial_register_passable_copy_forwarded.mojo` the
pre-elaboration `canonicalize.forwarded_copies` rises from 1 to 5.

## A dispatched `copy()` at a built-in folds per instance (2026-10-14, R839)

A generic body's `x.copy()` on a `T`-typed receiver lowers to a `copy`
method call through `__trait_dispatch.copy`. In an instance where `T` is a
built-in, the elaborator used to clear the target and leave the call for
both backends to answer as "the copy is the value". It now writes the call
as `value.copy` (`native::mono::substitute::copy_builtin_receivers`), as
upstream's `Copyable.copy` body `Self(copy=self)` is at such a `Self`, so
the post-elaboration `canonicalize` forwards it. The pin agrees: at `Int`,
`x.copy()`, `T(copy=x)`, and `var y = x` each compile to `ret i64 %0` in
its *unoptimized* LLVM (`mojo build --emit llvm`), so the fold happens in
its MLIR pipeline. On `collections_many`, debug binary:

| | before | after | change |
|---|---:|---:|---:|
| post-elaboration `forwarded_copies` | 36 | 139 | +103 |
| `concrete_instructions` | 15 840 | 15 737 | −103 |

The 103 receivers are all scalars or one-lane SIMD: `Int` 20,
`SIMD[int64, 1]` 16, `Float64` 12, `Bool` 11, `UInt8` 9, `Int32` 7,
`UInt16` 7, `UInt32` 5, and `Int8`, `Int16`, `UInt64`, `Float32` 4 each.
A receiver read through a `ref` binding (`for value in values:
value.copy()`) folds too since R864: the instance types a whole-root load
of a reference root by its referent
(`native::mono::substitute::repair_storage_result_types`), as lowering and
both backends read it, so post-elaboration `forwarded_copies` rises from 4
to 5 on `template_method_variadic_initializer` and from 4 to 6 on
`instance_constructor_symbols`. A struct
receiver's `copy` is its copy constructor, a call upstream keeps too. The
corpus-wide `scripts/instantiation-report` rerun is left to the nightly
gate.

## Canonicalize folds constant operators (2026-10-14, R842)

`canonicalize` now folds an instruction over operands defined once by a
folded `const`, as MLIR's greedy driver applies a `fold`, through the
replacement `sccp` uses (`analysis/fold.rs`). The first `canonicalize`
runs before `sccp`, so the program-seam folds move between counters and
the MIR does not change. On `collections_many`, debug binary,
`pre_elaboration.sccp.folded_constants` falls from 54 to 0 and
`pre_elaboration.canonicalize.folded_constants` reads 54; the
post-elaboration counts (6 and 0) and `concrete_instructions` (15 737) do
not move. `emit-mir` matched the previous binary on 126 `assets/ok`
fixtures (every eighth). Every fold `sccp` made there was straight-line;
its executable-block reasoning will matter once `Mem2Reg` (R787) puts
values in registers across blocks.

## Unused parameters leave the instance key (2026-10-14, R782)

`remove-unused-params`, upstream's `RemoveUnusedParams`, runs last in the
pre-elaboration pipeline and records each generic body's own compile-time
parameters that neither its body nor its signature reads; the elaborator
leaves their arguments out of the instance key
(`docs/architecture.md` §Pre-Elaboration Pipeline).
`scripts/instantiation-report` over the 1 031 fixtures that elaborate,
debug binary; *before* is a build of the parent commit, since the committed
baseline had drifted:

| | before | after | change |
|---|---:|---:|---:|
| corpus `generator_instances` | 30 543 | 30 527 | −16 |
| corpus `generator_instance_instructions` | 372 485 | 372 442 | −43 |
| corpus `concrete_functions` | 46 605 | 46 589 | −16 |
| corpus `concrete_instructions` | 806 443 | 806 400 | −43 |

Four fixtures change, and no fixture's output, native included:
`unused_parameter_instances` (16 → 10 instances, the stage's own fixture),
`trait_requirement_parameter_default` (14 → 8),
`trait_requirement_default_from_caller_parameter` (7 → 4), and
`comptime_application_argument` (113 → 112). The bundled library's
generators read their parameters, so no library instance merges; the
compile benchmarks do not move. The pin removes more: a parameter only
forwarded to a removed one (R907), one only a nested function's scope
carries (R908), and a static method's unread owner parameter (R909).

## Forwarded parameters leave the instance key (2026-10-14, R907)

`remove-unused-params` now walks the call graph callees first, as upstream's
`RemoveUnusedParams` does, so a parameter only forwarded, by a call or a
resolved method call, to a parameter the callee removed is removed too
(`docs/architecture.md` §Pre-Elaboration Pipeline). Its stage fixture,
`assets/ok/forwarded_parameter_instances.mojo`, debug binary, before
(the parent commit) and after:

| | before | after | change |
|---|---:|---:|---:|
| `concrete_functions` | 26 | 21 | −5 |
| `generator_instances` | 21 | 16 | −5 |
| `generator_instance_instructions` | 47 | 37 | −10 |

`mid`, `two_hop`, `expr_fwd`, `tmid`, and `method_fwd` each fall from two
instances to one, as on the pin; `mixed` (which reads `n` itself),
`to_reader` (whose callee reads it), and `rec` (which forwards it around a
self-call) keep two, as the pin does. On the pin a cycle member's calls out
of the cycle are not rewritten either, so `rec[2]` reaches a second `leaf`
instance there; Mojito's `leaf` removes its own `n`, so it keeps one, with
no observable difference (R912 adopts upstream's rewrite).
`scripts/instantiation-report` over the rest of the corpus moves no
fixture: no bundled-library generator only forwards a parameter to one its
callee drops, and a program-to-package forward still counts, as upstream
treats a package generator as external.

## `nodebug` callees inline before elaboration (2026-10-14, R784)

`inline-parametric`, upstream's `InlineParametric`, opens the
pre-elaboration pipeline's second round and replaces a call to an
`@always_inline("nodebug")` or `@always_inline("builtin")` callee that names
none of its own compile-time parameters with the callee's finished body
(`docs/architecture.md` §Pre-Elaboration Pipeline). Its stage fixture,
`assets/ok/always_inline_nodebug_calls.mojo`, debug binary, with the pass
and without it (the rest of the pipeline on in both):

| | without | with | change |
|---|---:|---:|---:|
| `concrete_functions` | 88 | 77 | −11 |
| `concrete_instructions` | 1 935 | 1 969 | +34 |
| `generator_instances` | 36 | 36 | 0 |
| `generator_instance_instructions` | 445 | 457 | +12 |

The eleven inlined callees leave the concrete program, since nothing calls
them any more; each inlined copy costs a few parameter bindings, and the
two instances of `generic_sum` each hold an inlined `add_one` and `square`.
No instance count falls: a generic callee is not yet inlined (R916).

The corpus does not move. No fixture under `assets/` or `benchmarks/`, and
nothing in the bundled library, marks a function `nodebug` or `builtin`
apart from the stage fixture, so the pass rewrites no other body; the
corpus `scripts/instantiation-report` row waits for the nightly report. The
pass's effect on the corpus arrives with the bundled library's markings
(R786, R913).

## Small `@always_inline` callees inline before elaboration (2026-10-16, R923)

`inline-parametric` now follows a plain `@always_inline` callee too and
inlines it while its finished body's `inline_cost` is below the pin's bound
(`docs/architecture.md` §Pre-Elaboration Pipeline). The bundled library's
`@inline(.always)` markings start to act: the stage fixture,
`assets/ok/always_inline_small_calls.mojo`, and three library-heavy
fixtures, debug binary, before and after (output identical in every row):

| fixture | `concrete_functions` | `concrete_instructions` | `generator_instances` |
|---|---:|---:|---:|
| `always_inline_small_calls.mojo` | 23 → 13 | 319 → 258 | 3 → 2 |
| `comptime_for_string_aggregates.mojo` | 34 → 31 | 944 → 982 | 5 → 4 |
| `comptime_struct_string_field.mojo` | 14 → 13 | 273 → 273 | 3 → 2 |
| `ctfe_dict_set_result.mojo` | 77 → 68 | 1 061 → 1 093 | 62 → 55 |

Every fixture under `assets/ok`, `assets/ownership_ok`, and
`assets/runtime_error` prints the same output and exits the same way as
before; the corpus `scripts/instantiation-report` row waits for the
nightly report.

## Trait default wrappers inline before elaboration (2026-10-14, R785)

A trait default's forwarding wrapper is `nodebug`, as upstream's
`synthesizeDefaultTraitMethodWrapper` marks it, so `inline-parametric`
replaces a statically resolved call with the wrapper's one forwarding call
(`docs/architecture.md` §Stage 3: Semantic Checking). The fixtures that move, debug
binary, before and after (output identical in every row):

| fixture | `concrete_functions` | `concrete_instructions` |
|---|---:|---:|
| `trait_default_method` | 7 → 6 | 96 → 94 |
| `trait_default_refinement` | 20 → 17 | 168 → 162 |
| `trait_default_calls_later_function` | 38 → 37 | 753 → 751 |
| `trait_bound_argument_drop_order` | 19 → 18 | 192 → 190 |
| `trait_default_overloads` | 16 → 16 | 145 → 149 |

A wrapper reached only by inlined calls leaves the concrete program. In
`trait_default_overloads`, `through[X: T]` still reaches both wrappers by
dispatch, so they stay, and each inlined copy in `main` adds a block split.
No `generator_instances` count moves. The shared corpus total of
`scripts/instantiation-report` falls by 8 concrete instructions, and
`benchmarks/instantiation.tsv` is regenerated with this change. The
bundled library's own `write` wrappers are variadic forwards, which the
inliner can bind since collectors inline (R1079), though neither
`Writer.write` nor `String.write` carries an inline marking (R1101).

## Mem2Reg promotes trivially register-passable slots (2026-10-14, R787)

`mem-2-reg`, upstream's `Mem2Reg`, runs before each of the first three
pre-elaboration rounds' `canonicalize`, and after elaboration behind
`lower-arg-conventions`, which stores each eligible `read` or owned
argument into a local it can promote (`docs/architecture.md`
§Pre-Elaboration Pipeline and §Post-Elaboration Pipeline). A slot each of
whose reads sees one reaching store becomes registers; a slot whose reads
merge two stores kept its slot until region values arrived (R927, below). Debug
binary, before and after (output identical):

| fixture | `concrete_instructions` | `generator_instance_instructions` | promoted slots (pre / post) |
|---|---:|---:|---:|
| `collections_many` | 15,730 → 14,157 | 14,482 → 13,162 | 13 / 671 |
| `nested_generics` | 3,628 → 3,367 | 2,760 → 2,604 | 0 / 121 |

The pre-elaboration counters cover the program's own bodies; package
bodies are finished on the elaborator's thread, whose counters the report
does not hold. Over the corpus (`scripts/instantiation-report`, regenerated
with this change), concrete instructions fall 808,361 → 735,053 (−9.1 %),
generator-instance instructions 372,899 → 348,661 (−6.5 %), and concrete
blocks 135,912 → 135,782. Two fixtures lose a concrete function
(`pliron_int_pow`, `pliron_uint_pow`): once `b` and `e` are registers,
`canonicalize` folds `b ** e`, and the power function is no longer reached.

## Lifetime-only markers end at `lower-lit` (2026-10-14, R837)

`lower-lit`, the part of upstream's `LowerLIT` that MIR has, erases every
`lifetime.keep_alive` and `interiors.invalidate` right after drop
elaboration, at both seams, so neither reaches the pre-elaboration
pipeline, the elaborator, or an instance (`docs/architecture.md`
§Pre-Elaboration Pipeline). The std bodies `empty.mojo` reaches held
1,037 of them; they now hold none, and `loans.establish` stays at 253.
Over the corpus (`scripts/instantiation-report`, regenerated with this
change; every fixture still elaborates, output identical):

| corpus total | before | after | change |
|---|---:|---:|---:|
| `concrete_instructions` | 710,775 | 647,616 | −63,159 (−8.9 %) |
| `generator_instructions` | 228,153 | 206,725 | −21,428 (−9.4 %) |
| `generator_instance_instructions` | 340,240 | 305,643 | −34,597 (−10.2 %) |
| blocks (all three counters) | unchanged | unchanged | 0 |

Per benchmark, concrete instructions: `collections_many` 13,740 → 12,238,
`nested_generics` 3,274 → 2,929, `stdlib_heavy` 2,902 → 2,592,
`trait_defaults` 1,763 → 1,671. `mem-2-reg` promotes the same slots.

Erasing only the instructions is not enough: an erased invalidation's
marker register keeps a span whose origin names a slot, and when the
unroller retires that slot (`mir::retire_slots`) its guard, which reads no
span origin, lets `renumber_slots` hit a retired slot. `lower-lit` drops
the erased markers' types and spans, as `canonicalize` does (R933 files
the guard's gap).

## Generic `nodebug` callees inline before elaboration (2026-10-14, R916)

`inline-parametric` now inlines a `nodebug` or `builtin` callee that has
its own compile-time parameters, binding them to the call's arguments
(`docs/architecture.md` §Pre-Elaboration Pipeline), so such a callee is no
longer instantiated per binding. That is the instance count R784's section
above still lacked. Debug binary, `scripts/instantiation-report` rows, the
binary before this change against the binary after it:

| fixture | `concrete_functions` | `generator_instances` | `generator_instance_instructions` |
|---|---:|---:|---:|
| `assets/ok/always_inline_nodebug_generic_calls.mojo` | 52 → 38 | 33 → 19 | 198 → 147 |
| `assets/ok/always_inline_nodebug_calls.mojo` | 76 → 75 | 36 → 35 | 365 → 362 |
| `benchmarks/compile/keyed.mojo` | 71 → 68 | 52 → 49 | 403 → 386 |
| `assets/ok/trait_default_wrapper_conventions.mojo` | 125 → 124 | 96 → 95 | 1,131 → 1,129 |

No other fixture moves. `keyed.mojo`'s three `rotate_bits_left` instances
are gone, the bundled library's only generic free `nodebug` function the
corpus reaches; the library's other `nodebug` functions are methods of
parametric structs (R917). The corpus total (output identical):

| corpus total | before | after | change |
|---|---:|---:|---:|
| `concrete_functions` | 46,214 | 46,195 | −19 |
| `concrete_instructions` | 649,778 | 649,728 | −50 |
| `generators` | 19,241 | 19,231 | −10 |
| `generator_instances` | 30,596 | 30,577 | −19 |
| `generator_instance_instructions` | 305,841 | 305,768 | −73 |

`benchmarks/instantiation.tsv` had not been regenerated since R913 let the
bundled library's `nodebug` callees reach the pass, which moved about 490
fixtures by one inlined `_is_valid_utf8` each. It is regenerated with this
change, and the "before" column above is the previous binary's report, so
that shift is not counted here.

## Static methods drop unread struct parameters (2026-10-15, R909)

`remove-unused-params` now treats a method as a generator over its struct's
parameters too, and a static call's spelled receiver `S[n]` as a forward
of `n` (`docs/architecture.md` §Pre-Elaboration Pipeline). Its stage
fixture, `assets/ok/static_method_unread_owner_instances.mojo`, debug
binary, before (the parent commit) and after:

| | before | after | change |
|---|---:|---:|---:|
| `concrete_functions` | 16 | 12 | −4 |
| `generator_instances` | 11 | 7 | −4 |
| `generator_instance_instructions` | 15 | 9 | −6 |

`S.st`, `S.chained`, and `caller` each fall from two instances to one, and
`P.first` from three to two, as on the pin. Over every fixture under
`assets/`, twelve move, all through static methods of parametric structs,
for 394 → 364 generator instances in total, and none changes its output or
diagnostics. A method with a receiver keeps its struct's parameters, where
the pin removes them (R978).

## Unread callable parameters are removed (2026-10-15, R910)

`remove-unused-params` now considers a callable value parameter, and the
elaborator binds no runtime closure or environment for one a callee
removed (`docs/architecture.md` §Pre-Elaboration Pipeline). The stage
fixture, `assets/ok/unread_callable_parameter_instances.mojo`, was
rejected before (forwarding a capturing closure to a callee that ignores
it); its subset without that forward and `forward_and_call`, debug binary,
before (the parent commit) and after:

| | before | after | change |
|---|---:|---:|---:|
| `concrete_functions` | 23 | 18 | −5 |
| `generator_instances` | 14 | 9 | −5 |
| `generator_instance_instructions` | 50 | 33 | −17 |

`ignore`, `ignore_capturing`, `ignore_generic`, `forward_ignored`, and
`defaulted` each fall to one instance, as on the pin; `call` and
`forward_called` keep one per binding. No other fixture under `assets/`
changes its instance count, output, or diagnostics.

## A struct type reads only what its layout reads (2026-10-15, R977)

`remove-unused-params` now reads a struct type's arguments only where the
struct's lowered layout reads them, and whole where a member runs on it
(`docs/architecture.md` §Pre-Elaboration Pipeline). The stage fixture,
`assets/ok/struct_layout_unread_parameter_instances.mojo`, debug binary,
before (the parent commit) and after:

| | before | after | change |
|---|---:|---:|---:|
| `concrete_functions` | 117 | 108 | −9 |
| `generator_instances` | 103 | 94 | −9 |
| `generator_instance_instructions` | 708 | 686 | −22 |

`fb`, `fwd`, `fhb`, `fpb`, `tag_of`, `boxed_tag`, `fp`, `fv`, and
`fm` each fall to one instance, as on the pin; `fh`, `fs`, `pass_`, and
`fn_` keep one per argument, as on the pin, and `fg` keeps two where the
pin keeps one (R998). No other fixture under `assets/ok/` changes its
instance count, output, or diagnostics, so `benchmarks/instantiation.tsv`
gains only the fixture's row.

## A method's receiver reads only what its struct's layout reads (2026-10-15, R978)

`remove-unused-params` now reads a method's `Self` (its receiver, `out
self`, or `Self` result) as the struct spelled at its own parameters, and
an owner-less method instance takes its struct with the removed arguments
erased (`docs/architecture.md` §Pre-Elaboration Pipeline). The stage
fixture, `assets/ok/receiver_unread_struct_parameter_instances.mojo`,
debug binary, before (the parent commit) and after:

| | before | after | change |
|---|---:|---:|---:|
| `concrete_functions` | 31 | 27 | −4 |
| `generator_instances` | 26 | 22 | −4 |
| `generator_instance_instructions` | 74 | 66 | −8 |

`Box.get`, `Box.take`, `Box.bump`, and `Wrap.get` each fall to one
instance, as on the pin. `Box.reads` and `Arr.first` keep one per
argument, as on the pin. `Box.twice` (R998), `Wrap.take` (R999), and the
constructors and destructors (R1000, R1001) keep one per argument where the
pin keeps one. Over the corpus, `benchmarks/instantiation.tsv`'s shared
`concrete_functions` total falls from 40418 to 40054 and no fixture grows:
bundled methods such as `List.__len__` merge too. Two fixtures that failed
at the parent commit elaborate again (a static call's erased receiver,
`static_method_unread_owner_instances.mojo`, `static_call_through_self.mojo`).

## Unused arguments leave the generator a direct call names (2026-10-15, R997)

`remove-unused-params` now removes first the runtime arguments a free
function's body never uses but to drop a value whose drop runs no code,
upstream's `identifyUnusedArguments`, and reads the rest of the body without
them; the elaborator instantiates that reduced generator for each direct
call and the full one for a function value (`docs/architecture.md`
§Pre-Elaboration Pipeline). The stage fixture,
`assets/ok/unused_argument_instances.mojo` without its `fside` call,
which the parent commit rejects, debug binary, before (the parent commit)
and after:

| | before | after | change |
|---|---:|---:|---:|
| `concrete_functions` | 37 | 34 | −3 |
| `generator_instances` | 17 | 15 | −2 |
| `generator_instance_instructions` | 43 | 39 | −4 |

`fa`, `fav`, `fmut`, `fkw`, and `fbor` each fall to one instance, as on the
pin; `fown`, whose `NBox` destructor prints, keeps one per argument, as on
the pin. The non-generic `g` and `fdef` gain a `$REMOVED$ARG` clone, and a
function value of `g` keeps `g` itself. `fside()`, whose omitted default
prints, now runs without evaluating it, as on the pin, where the parent
commit refused to run the default at compile time (a used argument's such
default still fails, R1015). No other fixture under `assets/ok/`
reaches a reduced generator or changes its output, so
`benchmarks/instantiation.tsv` gains only the fixture's row. A method's
argument (R1011) and receiver (R1012), a forward of an argument to a
removed one (R1013), and nested, lifted, and variadic functions (R1014)
keep their arguments where the pin removes them.

## A member call reads only the struct arguments the member kept (2026-10-15, R998)

`remove-unused-params` now reads a call of a struct's member (a method
call, a subscript, an operator, a prefix `-` or `~`, an iteration step)
through the member's removals, as upstream's call passes the member's
parameters as values (`docs/architecture.md` §Pre-Elaboration Pipeline).
The stage fixture, `assets/ok/member_call_unread_struct_parameter_instances.mojo`
less its `fneg`, which fails elaboration at the parent commit, debug
binary, before (the parent commit) and after:

| | before | after | change |
|---|---:|---:|---:|
| `concrete_functions` | 36 | 27 | −9 |
| `generator_instances` | 29 | 20 | −9 |
| `generator_instance_instructions` | 105 | 65 | −40 |

`fg`, `NBox.twice`, `fsub`, `fop`, `finv`, `fput`, `fk`, `fit`, and `ft`
each fall to one instance, as on the pin; `fneg` keeps one per argument,
as on the pin. A construction (R1020) and `len` (R1021) still read their
struct whole. The corpus effect on `benchmarks/instantiation.tsv` is left
to its next regeneration.

## A method's unused argument leaves the generator a method call names (2026-10-15, R1011)

`remove-unused-params` now removes a method's runtime argument its body
never uses, past the receiver, and the elaborator's resolved `MethodCall`
arm names the reduced generator and drops the argument, as upstream's
`RemoveUnusedParams` clones every generator (`_REMOVED_ARG`). Over the
1,049 `assets/ok` and benchmark fixtures elaborated before and after
(`benchmarks/instantiation.tsv`, debug binary, the parent commit before):

| | before | after | change |
|---|---:|---:|---:|
| `concrete_functions` | 28,376 | 28,260 | −116 |
| `concrete_instructions` | 389,429 | 387,662 | −1,767 |
| `generator_instances` | 16,481 | 16,401 | −80 |
| `generator_instance_instructions` | 171,516 | 170,423 | −1,093 |

No fixture's concrete program grows, and no program's output changes. A
few fixtures gain a generator: a non-generic method that loses its
argument becomes a reduced clone, as R997's non-generic free function
does (`S.g` in `assets/ok/overloaded_signature_module_constant.mojo`). The new
fixture, `assets/ok/method_unused_argument_instances.mojo`, has one
instance of each of `Holder.m`, `Holder.mk`, and `PBox.m` for `Arr[1]`
and `Arr[2]`, as on the pin. A static method (R1032), a dunder or table
member (R1033), and the receiver (R1012) still keep their arguments.

## A method's unused receiver leaves the generator a method call names (2026-10-15, R1012)

`remove-unused-params` now removes a method's receiver its body never
uses, as upstream's `RemoveUnusedParams` does (`k3(q::Holder)_REMOVED_ARG()`),
and the elaborator's resolved `MethodCall` arm becomes a `Call` of the
reduced generator with no receiver. Over the 1,050 `assets/ok` and
benchmark fixtures elaborated before and after
(`benchmarks/instantiation.tsv`, debug binary, the parent commit before;
the new fixture's row aside):

| | before | after | change |
|---|---:|---:|---:|
| `concrete_functions` | 28,272 | 28,207 | −65 |
| `concrete_instructions` | 387,807 | 386,697 | −1,110 |
| `generator_instances` | 16,407 | 16,470 | +63 |
| `generator_instance_instructions` | 170,442 | 175,370 | +4,928 |

No fixture's concrete program grows, and no program's output changes.
The generator counters grow because a non-generic method that loses its
receiver becomes a reduced clone, a generator with one instance, as
R1011's non-generic methods did (`StringSpan._lead_width$REMOVED$ARG` in
the string fixtures). The new fixture, `assets/ok/method_unused_receiver.mojo`,
has one instance of `Grid.k3` for `Grid[1]` and `Grid[2]`, as on the pin.

## An argument only forwarded to a removed argument is unused (2026-10-15, R1013)

`remove-unused-params` now reads a body as upstream's `RemoveUnusedParams`
leaves it once it has rewritten each call of a callee it removed an
argument from (`mir::without_removed_argument_forwards`), and the
elaborator builds every instance through the same view
(`mir::with_removed_argument_forwards_erased`). Over the 1,051 `assets/ok`
and benchmark fixtures elaborated before and after
(`benchmarks/instantiation.tsv`, debug binary, the parent commit before;
the new fixture's row aside):

| | before | after | change |
|---|---:|---:|---:|
| `concrete_functions` | 28,228 | 28,211 | −17 |
| `concrete_instructions` | 386,911 | 386,767 | −144 |
| `generator_instances` | 16,478 | 16,462 | −16 |
| `generator_instance_instructions` | 175,393 | 175,304 | −89 |

No program's output changes over the 1,089 `assets/ok`, `ownership_ok`,
and `runtime_error` fixtures (HEAD worktree differential). The new
fixture, `assets/ok/forwarded_argument_instances.mojo`, has one instance
of each of `fwd`, `fkw`, `fchain`, `fmove`, and `frecv` for `n = 1, 2`,
and keeps two of `fcopy`, `fboth`, and `fwdk`, as on the pin. An argument
a function passes only to itself (R1034) still keeps its instances.

## Every binder kind inlines (2026-10-16, R1078)

`inline-parametric` now binds a callee's callable binders, its owner's
origin-carrying type, its constrained binders and `where` clauses, and a
`comptime for` sequence, and binds a `read` argument passed from a place,
read through a block-made reference, or trivially copied, where each kept
the call before (`docs/architecture.md`, §Pre-Elaboration Pipeline). The
whole-corpus refusal trace that planned the change ranked the owner's
origin argument first (every `Span.__getitem__`, `Span.unsafe_ptr`,
`_ListIter.__deinit__`, and iterator `__copyinit__` call), the
conditional-member availability second, and the `ref.read`-of-a-made-
reference argument (`_ZeroStartingRange.__copyinit__` in `__iter__`,
`String.__copyinit__` in `copy`) third; all three now inline. No
program's output changes over the 1,059 `assets/ok` and `ownership_ok`
fixtures (HEAD worktree differential), and two that stopped at HEAD now
run (`pack_inferred_through_tuple.mojo`, `tuple_template_served.mojo`,
R1093). The instantiation counters are the nightly report's to record;
expect `generator_instances` to fall by the iterator and span members the
bundled library no longer instantiates per element type.

## Collectors and spreads inline (2026-10-16, R1079)

A call collecting into `*xs`, spreading the caller's pack, or forwarding
`**kwargs^` into a `nodebug` callee inlines, the call's block building the
collector as a `tuple.make`, and the consuming `Tuple` members inline on a
forwarded, loaded, or copied receiver, their `comptime for` bound applied
to the bound element list. The in-session gate measured no counts; the
nightly `scripts/instantiation-report` refreshes `benchmarks/instantiation.tsv`,
and `generator_instances` should fall by the `Tuple.reverse`,
`Tuple.concat`, and collector-taking `nodebug` instances the corpus no
longer names.

## Mem2Reg carries slots through region values (2026-10-21, R927)

On a structured body `mem-2-reg` is now upstream's `processRegion`: a slot
assigned in an arm, an elif condition, a loop or `comptime for` body, or a
`match` case and read after the op travels through the op's region
arguments, init operands, and results instead of keeping its slot
(`docs/architecture.md`, §Pre-Elaboration Pipeline). Summed `--timings`
counters over the `assets/ok` fixtures, debug binary, HEAD against the
change (every fixture's output identical on the VM, the whole asset tree
diffed):

| counter | before | after |
|---|---:|---:|
| pre-elaboration promoted slots, program bodies | 2,597 | 2,872 |
| pre-elaboration promoted slots, package bodies | 1,560 | 2,205 |
| post-elaboration promoted slots | 15,041 | 14,422 |
| concrete instructions | 414,023 | 382,714 (−7.6 %) |
| generator-instance instructions | 163,107 | 154,382 (−5.3 %) |

Fewer slots are left for the post-elaboration round, and every instance
of a generic body starts from the smaller generator. The nightly
`scripts/instantiation-report` refreshes `benchmarks/instantiation.tsv`.

## An inlined call through a bound names the witness (2026-10-22, R1052)

`inline-parametric` now folds a call through a trait bound in an inlined
body to the member the receiver's struct records in its witness table,
once the substitution made the receiver a struct, so
`remove-unused-params` reads only the struct arguments that member kept
(`docs/architecture.md` §Pre-Elaboration Pipeline). On four probes, each a
caller `f[n](b: Box[n])` reaching a `@no_inline` member
through an `@always_inline("nodebug")` helper bounded by a trait, debug
binary, the parent commit before:

| probe (call through the bound) | `generator_instances` before | after |
|---|---:|---:|
| method (`value.__len__()`) | 3 | 2 |
| comparison and subscript (`a < b`, `a[1]`) | 6 | 4 |
| trait default (`value.one()`) | 4 | 2 |
| generic requirement (`a.pick[2]()`) | 3 | 3 |

The generic requirement's call folds, but its bracket argument keeps a
run-time register, since a call through a bound records no instantiation
(R394). Over the 582 `assets/ok` and `assets/runtime_error` fixtures that
declare a bounded parameter, every output is unchanged on the VM and no
other fixture's `generator_instances` moves; the stage fixture,
`assets/ok/member_call_unread_struct_parameter_instances.mojo`, gains
`fbound`, one instance for every `n`. The nightly
`scripts/instantiation-report` refreshes `benchmarks/instantiation.tsv`.
