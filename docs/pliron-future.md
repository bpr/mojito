# Pliron as Mojito's Compiler IR Framework: Front-End Feasibility

**Status:** assessment recorded 2026-09-11. Not scheduled work. The scheduled
consequences are the checkboxes in [`docs/roadmap.md`](roadmap.md) §3.

**Companion document.**
[`docs/pliron-backend-pivot-plan.md`](pliron-backend-pivot-plan.md) decides how
Pliron could become the required IR framework *below* `CheckedProgram`, and
stages that migration (A0–A7, commitment point A4). This document asks the
question that plan deliberately leaves out: what would it take for Pliron to be
to Mojito what MLIR is to Mojo, *including above the waist*, and in what order
should the work happen. The two agree on keeping the AST and `CheckedProgram`;
they differ in where the remaining distance to Mojo actually lies.

## Bottom line

Rearchitecting is technically feasible, but Pliron is not the obstacle, and
adopting it does not by itself produce Mojo's architecture. What makes Mojo's
pipeline Mojo-like is that it type-checks parametric code *before* instantiating
it, and keeps that code as parametric IR. Mojito does the reverse: it elaborates
the AST first and then checks the concrete clones. That front-end change is
language work that has to happen with or without Pliron. Do it first, and treat
the move to Pliron as a later, optional step.

## What the source notes got wrong

The two notes in `~/markdown` that prompted this assessment have been corrected
in place. The corrections matter because three of them change the conclusion.

- **Mojo does not JIT-compile compile-time code.** The 2025 LLVM Dev Meeting
  talk states that meta-code is "Executed by an IR interpreter" and "supports
  ~arbitrary logic, including malloc".
- **Type checking does not happen during MLIR verification.** It happens in the
  parser, symbolically, before IR generation: "Type check + Generate IR before
  instantiating… Type checking is symbolic instead of concrete." Upstream's
  errors on the probes below even end with "failed to parse the provided Mojo
  source module".
- **Mojo's dialects are three, with distinct jobs.** `lit` carries declarations
  and calls (`lit.fn`, `lit.call`), `kgen` is the meta layer
  (`kgen.param.if`/`for`/`call`/`constant`, `#kgen.param.ref`), and `pop` holds
  base operations and types (`!pop.simd<size, dtype>`). Parameter expressions
  live as uniqued typed *attributes*, so type equality reduces to
  canonicalization plus pointer comparison.
- **Pliron is not rewrite-free.** It ships `irbuild::match_rewrite`, `rewriter`,
  and `dialect_conversion`. Per `docs/notes/pliron-stage0.md` finding 1, only
  MLIR's legality-declaration layer is missing.
- **Pliron has a JIT.** `pliron-llvm` ships an ORC LLJIT wrapper, which Mojito's
  differential harness already uses.
- **The "write a Rust interpreter" path already exists here.** Mojito's register
  VM runs all CTFE: `crates/mojito-comptime/src/comptime/ctfe.rs` builds a
  `VmBackend` and calls `run_function_value` at four sites. The third-party
  `plirun` interpreter is version 0.1.2 with 50 downloads and is not a
  foundation.

## The real obstacle: Mojito checks after it rewrites

Three probes, run against the pinned Mojo (`mojo-2026082605`) and against
Mojito at `a13af11`:

| Probe | Upstream | Mojito |
|---|---|---|
| `var x: Int = "hello"` in the untaken branch of a `comptime if` | rejected | **runs, prints 1** |
| `x.nonexistent()` on a generic `T` in an untaken `comptime if` branch | rejected | **runs, prints ok** |
| `x.nonexistent()` in a generic function that is never called | rejected | rejected |

The first probe:

```mojo
def f[n: Int]() -> Int:
    comptime if n == 0:
        return 1
    else:
        var x: Int = "hello"
        return x


def main():
    print(f[0]())
```

Upstream reports `cannot implicitly convert 'StringLiteral["hello"]' value to
'Int'`. The second probe replaces the body with `x.nonexistent()` on a
`T: Copyable` parameter and a `flag: Bool` that selects the other branch;
upstream reports `'T' value has no attribute 'nonexistent'`. Mojito runs both.

So Mojito accepts invalid Mojo today, which breaks `AGENTS.md` invariant 1. The
divergence is now recorded in [`docs/roadmap.md`](roadmap.md) §2 and its fix is
§3.

- **Cause.** `comptime::elaborate` runs before the checker, and dropping untaken
  branches is intended behavior: "a type error in a dropped branch is never
  seen" (`crates/mojito-comptime/src/comptime.rs`, module docs).
- **Stale claim.** `docs/features.md` described clone-and-re-check as "matching
  real Mojo's per-instantiation model". The slides and the third probe show
  upstream checks generic bodies without instantiating them. That sentence has
  been corrected.
- **What Mojo's model requires.** The checker must type-check generic bodies,
  including `comptime if`/`for`, with parameters left symbolic. That needs a
  decision procedure for parameter-expression equality, so that `SIMD[dt, n+1]`
  and `SIMD[dt, 1+n]` are the same type. That problem is the fourth section of
  the 2025 talk. Mojito avoids it today by making everything concrete first.
- **How far Mojito already is.** Trait-bound generics already get an abstract
  pre-check and run through erased dispatch, so the machinery is not absent, it
  is bypassed whenever elaboration can specialize.

## Do you need a high-level dialect?

Yes. Without one, Pliron stays what it is in Mojito today: an LLVM emitter. That
is the current design, and it works.

The three layers needed are the ones the pivot plan already names:
`mojito.semantic` (declarations, calls, places, origins, argument conventions,
the `lit` analogue), a parametric layer (`kgen`'s analogue: parametric
`if`/`for`/`call` plus parameter-expression attributes, which the pivot plan
does not yet model), and `mojito.core` (base operations, the `pop` analogue).
The parametric layer is the piece that is genuinely missing from both the
current compiler and the existing plan, and it is exactly what the front-end
work below produces.

Adopting it reverses decisions the repository has written down. Each has been
marked as a current rule under review rather than a permanent one:

- The architecture doc's dialect policy: "Do not reproduce the MIR schema as a
  second operation set."
- `AGENTS.md` invariant 5, which makes MIR the stable waist, and the Start Here
  rule that no backend IR is a required internal layer.
- `docs/architecture.md` Design Goals: "mojito is not trying to reproduce Mojo's
  production architecture."
- The default-lane isolation test, which forbids Pliron in the default build.
  This one is only policy: Pliron core is pure Rust (slotmap, downcast-rs,
  combine), and only `pliron-llvm` needs LLVM.

A cheaper middle path has a precedent. cuda-oxide models rustc MIR as a
`dialect-mir` in Pliron and lowers it through the llvm and nvvm dialects.

## What survives a rearchitecture

Rust line counts are `src` only, excluding tests.

| Component | kLOC | Fate |
|---|---|---|
| lexer, parser, ast, common, module | 11.6 | Reused as is |
| types, symbol | 6.3 | Mostly reused; parameters gain a symbolic expression form |
| checker (+ checked handoff) | 37.5 (+2.3) | Name resolution, overloads, trait conformance, and diagnostics carry over. Its output changes from span-keyed side tables to emitted dialect ops, and it gains symbolic checking |
| comptime | 17.4 | **Mostly not reusable as code.** The AST-cloning core (`rewrite`/`specialize`/`mono`/`nested`, about 8.7k lines) becomes an elaborator over IR. CTFE-safety rules, the fuel budget, value crossing, and pack/SIMD-width logic carry over, and its tests become the spec |
| hir | 1.6 | Dropped |
| mir | 24.8 | Lowering logic (places, partial moves, try regions, calls) is ported. About 6.6k of text writer/parser and 3.9k of verifier are replaced by Pliron op definitions, printer, and verifiers |
| analysis | 5.9 | Algorithms (loans, partial-move tree, drop order) are rewritten over Pliron ops |
| native mono | 4.8 | Replaced by the elaborator; mangling is reused |
| pliron backend | 20.7 | The most reusable large crate: ABI, emission, toolchain, pass pipeline, debug info, and SIMD lowering stay. `lower/*` becomes conversion patterns from the mid-level dialect |
| native-core, runtime | 2.0 | Reused as is |
| stdlib (7.6k lines of Mojo) + fixture corpus | — | Fully reused, and the corpus is what makes a rewrite survivable |

That is roughly 60k of about 149k lines rewritten. These are estimates, not
measurements.

**The VM is not the part to discard.** Mojo still needs an IR interpreter for
compile-time code, and the VM is that component, reading the wrong IR. It is
also the reference implementation every VM-versus-native parity test depends on.
Its runtime value model and libc/builtin semantics carry over; the execution
loop changes.

## Other obstacles

- **Interpreter speed.** Pliron ops are trait objects downcast at runtime, and
  values are arena indices. An interpreter walking ops directly pays that per
  step. The fast answer is to translate the dialect into a register form before
  interpreting, which is MIR again.
- **Artifact text.** `docs/notes/pliron-stage0.md` finding 2 records that
  Pliron's parse-then-print is not a fixpoint without name erasure. Mojito's
  lossless `.mir` round trips and the `exec` verb depend on a stable text form.
- **API churn.** Pliron is at 0.17 behind a git pin. Today that churn is
  confined to one feature-gated crate; afterwards it touches everything.
- **No stdlib compatibility gain.** Upstream's stdlib is written against the
  closed `pop`/`kgen`/`lit` ops through `__mlir_op`. Pliron clones of those
  dialects still would not let Mojito compile upstream stdlib source. Mojito
  accepts only `__mlir_type.index`.
- **Compile time.** The effect of an end-to-end Pliron pipeline on compile time
  is unmeasured. Hello World is currently 0.8 s release.

## Corpus sweep for untaken-branch type errors

Run 2026-09-11 over every `assets/`, `stdlib/`, and `conformance/` Mojo file
containing `comptime if` (68 occurrences across 25 files), with the pinned Mojo
as the oracle. Files without that spelling were not swept.

Four sites break once the check order is fixed:

- `assets/ok/generic_ctfe_value_param.mojo` and
  `assets/ok/generic_ctfe_associated_value.mojo` place a deliberate type error
  (`var wrong: Int = "…"`) in the untaken branch as a compile-time assertion.
  Upstream already rejects both, though first for a different reason: it
  requires `comptime if` inside a function.
- `assets/ok/variadic_method_type_params.mojo` (`get`, `count_matching`) and
  `stdlib/std/collections/tuple.mojo` (`__contains__`) rely on the guard
  narrowing the method's `T` to the element type `Ts[i]`, which upstream does
  not do.

The narrowing rule was probed directly:

| Program | Upstream | Mojito |
|---|---|---|
| `comptime if T == Int: return x + 1` | rejected, `'T' does not implement the '__add__' method` | runs, prints 4 |
| `comptime if T == Int: return rebind[Int](x) + 1` | runs, prints 4 | rejected, `Undefined variable 'rebind'` |

So upstream's fix for the shape is `rebind`, which Mojito does not implement,
and upstream's own `Tuple.__contains__` uses it.

Everything else is safe. The remaining branches return same-typed literals,
write to a `Writer`, or are the `comptime if …: pass` specialization markers in
`stdlib/std/utils/variant.mojo`.

The sweep also found eight `assets/ok` fixtures that the pinned Mojo rejects
for reasons unrelated to branches, none of them listed in
`conformance/cases.tsv`. They are their own roadmap entry in §2.

## Recommendation: incremental, and not first

Existing defects come first. The native-backend and parity work in
[`docs/roadmap.md`](roadmap.md) §1 and §2 is a prerequisite for any of this:
a pivot evaluated against a backend with known miscompiles cannot be judged.

1. **Fix the check order inside the current architecture.** Type-check
   `comptime if`/`for` bodies symbolically and reject what upstream rejects.
   Conformance needs this regardless of Pliron, and it is where the Mojo-like
   behavior users actually notice lives: errors before instantiation, rather
   than template stack traces.
2. **Give parameter expressions a symbolic, normalized form.** Store them
   uniqued and canonicalized, shaped like Pliron attributes, so a later move
   costs little.
3. **Only then decide on hosting MIR as a Pliron dialect**, via the pivot plan's
   Stage A1 falsifiable slice. The full Mojo-style version, with an elaborator
   and an interpreter over parametric IR, rewrites about 40% of the compiler.

## Relationship to the pivot plan

- The pivot plan is right that parser-direct-to-Pliron would discard useful
  seams, and this assessment does not reopen it.
- The pivot plan's commitment point (A4) is below `CheckedProgram`, so it can
  proceed without the front-end change, and the front-end change can proceed
  without it. They are independent, and the front-end change is cheaper.
- The parametric layer is missing from the pivot plan's dialect design. If the
  pivot lands first, `mojito.semantic` will need it added rather than designed
  in, because monomorphization currently happens above the waist in elaboration
  and below it in `native::mono`.

## Sources

- Lattner and Zhu, "Building Modern Language Frontends with MLIR: Lessons from
  Mojo's Compile-Time Meta-Programming", LLVM Dev Meeting 2025:
  <https://llvm.org/devmtg/2025-10/slides/technical_talks/lattner_zhu.pdf>
- Modular, Mojo at LLVM 2023: <https://www.modular.com/blog/mojo-llvm-2023>
- cuda-oxide Pliron chapter:
  <https://nvlabs.github.io/cuda-oxide/compiler/pliron.html>
- `plirun` on crates.io: <https://crates.io/crates/plirun>
- Local evidence: `docs/notes/pliron-stage0.md` (facility matrix and findings),
  `crates/mojito-comptime/src/comptime/ctfe.rs` (VM-backed CTFE),
  the pinned Pliron checkout at revision `477e6b0e`.
