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

## Where to look first

These are investigation priorities, not conclusions from a profile.

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

### 1. Establish a reproducible external baseline

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

### 2. Extend `--timings` across the production pipeline

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

### 4. Profile the dominant measured phase

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
