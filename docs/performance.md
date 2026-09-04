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
link plus initial check, not the rounds. The next step is to make
`Cfg::build_checked_fn` borrow the program-wide checked tables (or receive
a per-function slice of them) instead of cloning them per function, then
re-measure. Only after that does a precompiled-prelude cache become worth
designing; at 0.5 s per full check today, it would buy little.

**Tooling that exists now.** `--timings` on every command
(`docs/usage.md`); `scripts/bench-compile` (hyperfine matrix over
`benchmarks/compile/`, results under `benchmarks/compile/results/`,
gitignored); `scripts/sample-stacks` (gdb-based sampler, no sudo; `samply`
is installed but needs `kernel.perf_event_paranoid=1`); the `profiling`
Cargo profile (release + debug info).

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
