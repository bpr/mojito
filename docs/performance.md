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
`Arc<CheckedTables>` (expressions, declarations, and their span indexes,
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
again. Hello World currently takes two such rounds for every outer check.

Investigate registering callable signatures once and propagating effect
summaries through a call-graph SCC/worklist. Only callers whose callee summary
changed should need reconsideration. Whether expression checking can wait for
stable summaries, or must be updated incrementally with them, is a semantic
design question to resolve before implementation.

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

`CheckedTables` now owns `expressions_by_span`, but `CheckedProgram` separately
owns an `expression_index` of the same shape, and constructs both. The program
also retains compatibility overload-target and implicit-conversion maps while
the newer checked-expression/call representations carry much of the same
semantic information. MIR still reads some compatibility data, so this is an
incomplete migration rather than dead state. Finish that migration and keep one
authoritative occurrence index.

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
| `hello.mojo` | 2100 | 1571 | 1320 | 0 | 0 | 325 |
| `stdlib_heavy.mojo` | 2100 | 1581 | 2892 | 2254 | 710 | 345 |
| `generic.mojo` | 2118 | 1568 | 1590 | 474 | 226 | 340 |

- Hello World mints no per-instantiation method clones: a program without its
  own instantiations has none. Its generated bodies are members of structs the
  elaborator specialized whole (`Tuple$…`, the `DType`-keyed ranges, the
  hasher) and per-call clones, all from concrete-only templates.
- `stdlib_heavy.mojo` checks an instance clone 2254 times and derives 710 of
  them, 31%; `generic.mojo` derives 226 of 474, 48%. With scalar getters alone
  (`MethodScalarBody`) those were 242 and 70, and wall time did not move.
  Before `ref self` accessors and the `_mojito_abort` statement they were 586
  and 178, and before place arguments 698 and 220.
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
| Generic method templates | 252 | 111 |
| Per-instantiation clones | 312 | 147 |

A derived body is not inferred, so it is not in the census. Of the 411
per-instantiation clone bodies of that pass, 124 derive; the census rows are
the generated generic bodies that were inferred.

What blocks the clone bodies, by how many bodies each reason appears in:

| Reason | Bodies | Sole blocker of |
|---|---:|---:|
| `ConstructionImmutableBinders` | 103 | 62 |
| a callee effect summary that is not empty | 39 | 27 |
| a fact keyed outside the body's occurrences | 24 | 0 |
| `ExplicitDestroyCalls` | 13 | 4 |
| adjustment `TypeName` | 11 | 3 |

The four reference facts (`ReferenceValueUses`, the `ReferenceResult`
adjustment, `InteriorReferences`, `CopyableReferenceResultReads`),
`SubscriptDescriptors`, and `CallPlaceUses` have recipes and left the table.
The best recipe sets by how many more clone bodies they would make capturable:
one, 62 (`ConstructionImmutableBinders`); two, 89 (plus effect summaries).
Capturable is necessary, not sufficient: 147 bodies are
capturable already and are inferred because no class admits their syntax.

What those bodies are made of is the census's second half
(`template_census.<class>.grammar.<construct>`): every construct a method's
declaration and body hold, whatever class admits it. Among the 314 clone
bodies, the constructs no class admits in any form are the method's own
binders (40) and `raises` with `raise` (21). The rest are admitted only in
part: a `mut` parameter (37, admitted with no origin clause, though every one
of these is a writer or a hasher the body calls through its bound), a `ref`
declaration (28, admitted over a place of `self`, a parameter, a local, or a
reference call), a method call with arguments (175, admitted when every
argument is a closed scalar), a direct call with arguments (174), a subscript (129, admitted on a pointer slot or as
a reference call on a field), a `^` transfer (121), a non-scalar closed result
(117), a string literal (42, admitted as the `_mojito_abort` message), a
`ref self` (41, admitted with no receiver origin), and a reference result (16,
admitted when every `return` hands out a place of `self`).

The remaining cost is body inference of uncovered bodies, about one second per
transfer pass. `docs/roadmap.md` section 1 carries the entries that attack it,
and `docs/notes/instantiation-from-template.md` is the design record.

The counters are `body_inference.{plain,template,clone}`,
`body_sites.instance_clone`,
`templates.{surviving_trait_bound,validated_keyed,concrete_only,validation_aborted}`,
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
   declaration walks before checking bodies, expands trait defaults and rekeys a
   cloned AST, builds many occurrence-indexed fact maps, and performs overload,
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
   `CompiledProgram::elaborated_mir` clones pre-drop MIR once and
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
      trait_defaults_expand
      syntax_rekey
      transfer.round[M].declarations
      transfer.round[M].bodies
      transfer.round[M].reference_validation
      explicit_destroy
    hir.lower
    mir.lower
    mir.verify.pre_drop
    ownership.moves
    ownership.interior_origins
    ownership.loans
  backend.prepare
    drops.elaborate
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
