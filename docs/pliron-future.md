# Pliron as Mojito's Compiler IR Framework: Front-End Feasibility

**Status:** assessment recorded 2026-09-11; the dialect-definition section
added 2026-09-16. The goal it serves — an implementation that resembles Mojo's
own — is settled (`docs/architecture.md`); what stays unscheduled is this
document's particular staging of it. The scheduled consequences are the
checkboxes in [`docs/roadmap.md`](roadmap.md) §1.

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
- **Mojo has many dialects, each with a distinct job.** Among them, `lit`
  carries declarations and calls (`lit.fn`, `lit.call`), `kgen` is the meta
  layer (`kgen.param.if`/`for`/`constant`, `#kgen.param.expr`), `pop` holds
  base operations, and `hlcf` holds structured control flow; `co`, an
  interpreter dialect, and upstream MLIR dialects sit beside them. The value types
  (`!kgen.simd<size, dtype>`, `!kgen.pointer`, `!kgen.dtype`) belong to `kgen`,
  not `pop`. Parameter expressions live as uniqued typed *attributes*, so type
  equality reduces to canonicalization plus pointer comparison. See
  [Defining the dialects](#defining-the-dialects).
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

So Mojito accepted invalid Mojo, which broke `AGENTS.md` invariant 1. Resolved
2026-09-16 by source validation (`checker/comptime_validation.rs`,
`docs/architecture.md` §Stage 2): both probes now reject with the
diagnostics above; the pack-keyed remainder is [`docs/roadmap.md`](roadmap.md)
§1.

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

Adopting it reverses decisions the repository has written down. Each is
recorded as the arrangement the code follows today, not as a permanent one —
the goal is to resemble Mojo's implementation, so these are the rules a stage
is expected to move:

- The architecture doc's dialect policy: "Do not reproduce the MIR schema as a
  second operation set."
- `AGENTS.md` invariant 5, which makes MIR the stable waist, and the Start Here
  rule that no backend IR is a required internal layer.
- `docs/architecture.md` Design Goals, which now record the opposite intent:
  not reproducing Mojo's production architecture "is now regarded as a mistake
  that must be corrected".
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
  interpreting, which is MIR again. Mojo does the same: its elaborator's
  interpreter compiles functions to `FunctionIRBytecode` before evaluating
  them (see [How Mojo's pipeline uses its dialects](#how-mojos-pipeline-uses-its-dialects)).
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

Four sites broke once the check order is fixed. Two are gone as of the
2026-09-12 respelling pass: `assets/ok/generic_ctfe_value_param.mojo` and
`assets/ok/generic_ctfe_associated_value.mojo` placed a deliberate type error
(`var wrong: Int = "…"`) in the untaken branch as a compile-time assertion, and
now fold the assertion into a second `comptime` alias instead. Two remain,
both the same narrowing rule:

- `conformance/fixtures/pack_element_type_narrowing.mojo` (`get`,
  `count_matching`, moved out of `assets/ok/variadic_method_type_params.mojo`)
  and `stdlib/std/builtin/tuple.mojo` (`__contains__`) rely on the guard
  narrowing the method's `T` to the element type `Self.Ts[i]`, which upstream
  does not do.

The narrowing rule was probed directly:

| Program | Upstream | Mojito |
|---|---|---|
| `comptime if T == Int: return x + 1` | rejected, `'T' does not implement the '__add__' method` | runs, prints 4 |
| `comptime if T == Int: return rebind[Int](x) + 1` | runs, prints 4 | rejected, `Undefined variable 'rebind'` |

So upstream's fix for the shape is `rebind`, which Mojito now implements
(`checker/rebind.rs`); the bundled `Tuple.__contains__` and
`assets/ok/pack_element_rebind.mojo` use it. The implicit narrowing itself is
still accepted, because a variadic template's bodies are not yet validated
symbolically (`docs/roadmap.md` §1).

Everything else is safe. The remaining branches return same-typed literals,
write to a `Writer`, or are the `comptime if …: pass` specialization markers in
`stdlib/std/utils/variant.mojo`.

The sweep also found eight `assets/ok` fixtures that the pinned Mojo rejected
for reasons unrelated to branches, none of them listed in
`conformance/cases.tsv`. All eight were respelled on 2026-09-12 and now run on
both compilers as `cases.tsv` `run` rows; the unswept remainder of `assets/` is
its own roadmap entry in §3.

## Recommendation: incremental, and not first

Existing lifecycle and place defects come first, but not all of
[`docs/roadmap.md`](roadmap.md) §2 and §3, since §3 reopens at every re-pin.
§1 opens with a numbered list of the entries that gate it: the
drop-elaboration and MIR place-shape defects the Stage A1 slice must model,
and the native miscompile in that slice's own shape. A pivot evaluated against a backend with
known miscompiles cannot be judged. The rest of §2 and §3 is orthogonal to
steps 1 and 2 below and can interleave with them.

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

## Defining the dialects

Recorded 2026-09-16 from a check of `~/markdown/pliron-tablegen.md` (a note on
building an MLIR TableGen equivalent for Pliron, and Mojo-like dialects on top
of it). The note was checked against Pliron at the pin `477e6b0` and at master
`8e0edee`, against Modular's own dialect definitions
(`Mojo/include/Mojo/<X>Dialect/*.td` at `modular` `dafad186fb`), and against
the pinned toolchain. Its architecture holds. Its Pliron inventory is out of
date, and most of its Mojo op names beyond the compiler walkthrough's short
tables are invented.

Dialects are not the next step: the recommendation above still puts the
front-end work first. But a Mojo-shaped Mojito will need them, and probably
quite a few. Mojo has many, and the pivot plan plus this assessment already
name four for Mojito: `mojito.semantic`, a parametric layer, `mojito.core`, and
`mojito.abi`. Across that many dialects the definitions are a large mechanical
surface. Mojo writes its dialects in MLIR ODS (TableGen). Pliron has no ODS.

### What Mojo's dialects contain

The table covers the four dialects the front end is built around, not every
dialect Mojo has. Counts are top-level `def` lines in the `.td` files, so
approximate.

| Dialect | Job | Ops / types / attrs | Real names |
|---|---|---|---|
| `lit` | Parser output: declarations, calls, places, origins. There is no AST. | 56 / 17 / 26 | `lit.fn`, `lit.call`, `lit.var.decl`, `lit.ref.load`/`store`, `lit.ownership.mark_initialized`/`mark_destroyed`; `!lit.ref<T, origin>`, `!lit.origin`; `#lit.origin.union`/`field`/`subtree`/`mutcast` |
| `kgen` | Program structure and the parametric layer; the canonical IR after checking | 35 / 22 / 86 | `kgen.generator`, `kgen.func`, `kgen.call`, `kgen.param.constant`/`if`/`for`/`apply`/`yield`, `kgen.struct.extract`; `!kgen.simd`, `!kgen.pointer`, `!kgen.dtype`, `!kgen.string`, `!kgen.param`; `#kgen.param.expr`, `#kgen.dtype.constant` |
| `pop` | Base operations, parametric before elaboration and concrete after | 73 / 4 / 36 | `pop.add`, `pop.cmp`, `pop.cast`, `pop.load`/`store`, `pop.simd.splat`, `pop.array.gep`, `pop.stack_allocation`; its only types are `!pop.array`, `!pop.union`, `!pop.int_literal`, `!pop.float_literal` |
| `hlcf` | Structured run-time control flow; non-parametric, present before and after elaboration | 10 / 0 / 1 | `hlcf.if`/`elif`/`switch`/`for`/`loop`/`break`/`continue`/`yield`; `#hlcf.unroll_level` |

They are not the whole set.
- `Mojo/lib` holds a `<Name>Dialect` directory for each of these four plus
  `co` (coroutines, 14 ops).
- The walkthrough's summary table adds `debuginfo` and `interp`.
- The pipeline also uses upstream MLIR's `index`, `llvm`, `nvvm`, and `rocdl`.

The count moves as Mojo grows; the point is that a Mojo-style compiler owns
several dialects, not one or two.
`mojo build --mlir-timing` shows the passes by name: `LowerSemanticCF`,
`CheckLifetimes`, `LowerLIT`, `ElaborateGenerators`, `RaiseForLoops`,
`LowerControlFlow`, `LegalizePOPOperations`, `LowerPOPToLLVM`,
`LowerKGENToLLVM`.

Three facts bear on Mojito's planned dialects:

- **Value types live in the parametric layer.** A scalar is a size-one
  `!kgen.simd`, and SIMD, pointer, and dtype types are all `kgen` types,
  because their widths and dtypes are parameter attributes. The pivot plan's
  `mojito.core` is monomorphic, so its value types are concrete. A Mojito
  parametric layer would have to own a parametric form of them, or
  `mojito.core`'s types would have to accept parameter attributes.
- **The parametric layer is mostly attributes.** `kgen` defines more than
  twice as many attributes as ops. That is the §1 roadmap entry "Parameter
  expressions have no symbolic form" at framework scale.
- **Compile-time and run-time control flow are separate dialects.**
  - `kgen.param.if` and `kgen.param.for` are the compile-time branch and loop.
    The elaborator's interpreter instantiates their bodies.
  - Run-time branches and loops stay structured in `hlcf` until
    `LowerControlFlow`. `hlcf` is non-parametric.
  - `#hlcf.unroll_level` steers a separate loop-unrolling transform on
    run-time loops.
  - Mojito expands `comptime if`/`for` in AST elaboration and builds a CFG for
    everything else in HIR. So a Mojito parametric layer needs `param.if` and
    `param.for` analogues from the start. An `hlcf` analogue is needed only if
    some pass wants loops before the CFG.

### What Pliron already provides

The same at the pin and at master:

- **Definitions.**
  - `#[pliron_op]` takes `name`, `format`, `interfaces`, `attributes`,
    `operands`, `results`, and `verifier = "succ"`.
  - `#[pliron_type]` and `#[pliron_attr]` define types and attributes.
  - `operands`/`results` generate `get_operand_<name>` getters and the
    `OperandNOfType`/`ResultNOfType` interfaces.
  - `derive_attr_get_set` generates attribute accessors.
- **Text form.** `format_op` directives:
  - Entity references: `$attr`, `$i`.
  - Types: `type($i)`, `typesig`.
  - Regions and successors: `region($i)`, `succ($i)`.
  - Lists: `operands(sep)`.
  - Attributes: `attr(...)`, `opt_attr(...)`, `attr_dict`.
  - Escape to the generic form: `canonical`.
- **Interfaces.**
  - `op_interface`, `type_interface`, and `attr_interface`, with impl macros.
    Queries go through `op_cast::<dyn I>` and `op_impls`.
  - Builtin interfaces cover MLIR's common traits: terminators, operand and
    result arity, `OperandSegmentInterface`, `SingleBlockRegionInterface`,
    `IsolatedFromAboveInterface`, `SymbolOpInterface`,
    `SameOperandsAndResultType`, `CallOpInterface`.
  - Interface `verify` functions run automatically.
- **Rewriting and passes.**
  - Rewrite and conversion: `irbuild::{match_rewrite, rewriter,
    dialect_conversion}`.
  - Pass infrastructure: `PassManager` and `AnalysisManager`.
  - Folding and optimizations: `ConstFoldInterface`, mem2reg, DCE, and CFG
    simplification.
- **Registration.** `Dialect::register` and `OpId`. The proc macros register
  each item into distributed slices.
- **Shipped dialects.** Only `builtin` and `llvm`. There is no structured
  control flow and no `func`/`cf` dialect.

Mojito uses none of the definition macros yet: `mojito-pliron` builds
`builtin` and `llvm` ops and runs mem2reg and DCE.

### What an ODS layer would add

Missing at the pin and at master:

- variadic operand and result groups declared in the definition (today:
  `OperandSegmentInterface` plus hand-written accessors);
- generated builders;
- generated verifiers beyond the successor check, and declarative type
  constraints beyond the builtin `*OfType` interfaces;
- assembly-format optional groups, custom directives, `functional-type`, and
  type inference from the format (parsed operand types are ignored);
- a runtime descriptor of each op's operands, attributes, and interfaces;
- documentation generated from the definitions;
- declarative rewrite patterns, and MLIR-style legality targets (already
  recorded in `docs/notes/pliron-stage0.md` finding 1).

Where the note goes wrong:

- **It treats existing Pliron facilities as future work.** Rewrite patterns,
  dialect conversion, a pass manager, and folding already exist. Only the
  declarative forms are missing.
- **Several API and trait names are wrong.** `op.get_interface::<dyn I>()` is
  `op_cast`, and Pliron has no `Pure` or `Commutative` traits.
- **Its own crate layout fits Pliron poorly.** It proposes a separate
  `pliron-tblgen` binary and schema crate. Pliron registers per-item from
  proc macros and keeps no global schema. A generator that emits
  `#[pliron_op]` items, or a schema grown inside `pliron-derive`, would work
  with that model instead of beside it.
- **Most Mojo names beyond the walkthrough are invented.**
  - The note lists `lit.yield`, `lit.destroy`, `kgen.param.bind`/`get`,
    `kgen.field.*`, `kgen.alloc`, `pop.broadcast`, `pop.extract`/`insert`,
    `pop.array.set`, and `hlcf.while`. None exists.
  - `hlcf.if` takes a `!kgen.scalar<bool>`, not an `i1`.

The note's split between generated and handwritten code is right, and it maps
onto code Mojito already has:

| Dialect | Handwritten subsystem | Mojito's existing counterpart |
|---|---|---|
| `lit` | Origin algebra, lifetime checking | checker origins, ownership analysis, drop elaboration |
| `kgen` | Elaboration, compile-time interpreter | comptime elaborator, VM-backed CTFE |
| `pop` | Parametric type rules, lowering | SIMD and scalar rules, `mojito-pliron` lowering |
| `hlcf` | Structured-to-CFG lowering | HIR CFG lowering |

### Recommendation for dialect definitions

1. **Write the first dialects by hand.** Use `#[pliron_op]` for the pivot
   plan's Stage A1 shadow `mojito.core` and any first parametric-layer slice.
   A generator pays off in proportion to op count, summed across dialects.
   The five in `Mojo/lib` hold about 190 ops and 200 types and attributes,
   far more than a first slice. So build it only once Mojito's dialects repeat
   accessors, builders, and verifiers often enough to show the shape the
   generator must take. Expect that point to come: Mojito is likely to own
   several dialects, as Mojo does. Until then, keep hand-written definitions
   uniform (one `#[pliron_op]` style, operand segments and verifiers done
   the same way everywhere), so a later generator replaces them mechanically.
2. **Let the front-end work order the dialects, not the framework.** The note
   suggests `hlcf`, then `pop`, then `kgen`, then `lit`, to exercise Pliron
   feature by feature. For Mojito, the first Pliron-shaped artifact is the
   symbolic parameter-expression form (§1). It is an attribute: a
   `#[pliron_attr]` with canonicalization, whether or not it lives in Pliron
   yet. Next comes the parametric layer with its compile-time `if` and `for`,
   then `mojito.core`.
3. **Grow any declarative layer upstream.** Build it as an extension of
   `pliron-derive`, proposed to Pliron, in line with the pivot plan's "upstream
   narrow additions" stance. The first pieces worth having are the ones
   missing above that every dialect repeats: variadic groups, builders, and
   declared type constraints. Documentation and descriptors come after.

## How Mojo's pipeline uses its dialects

Recorded 2026-09-16 from Modular's compiler walkthrough
(`Mojo/docs/compiler/MojoCompilerWalkthrough.md`). smolnero's essay "What a
Compiler Chooses to Remember" (2026-08-30) prompted it; the essay retells the
walkthrough and cites it as its only source. The essay's one imprecision is
that a `.mojoc` package holds IR from "before elaboration". It holds post-parse
`lit` IR, as below.

These points bear on Mojito, and none is visible from the dialect definitions
alone.

- **Dialects are split by concern, not by phase, and they mix.**
  - `kgen`, `pop`, and `hlcf` ops already appear beside `lit` before
    `LowerLIT`. A stage is defined by which ops are legal: no `lit` after
    `LowerLIT`, no generators after `ElaborateGenerators`, and only `llvm`
    at the end.
  - This matches the pivot plan's "avoid a dialect per frontend phase" rule,
    and says how to keep it with many dialects: phase legality belongs in
    verifiers and conversion targets, not in dialect boundaries.
- **Generators are optimized before they are instantiated.**
  - Between `LowerLIT` and elaboration, `SROA`, `Mem2Reg`, `Canonicalizer`,
    `SCCP`, `EliminateDeadSymbols`, and `RemoveUnusedParams` run on the
    parametric generators.
  - Two restricted inliners run there too: `InlineParametric` for
    `always_inline_no_debug` and small functions, and `ApplyInliner`. The
    restriction is deliberate: more inlining would weaken the elaborator's
    caching.
  - The work is done once per template rather than once per instantiation.
    Mojito cannot do the same while it clones the AST before checking.
    `docs/performance.md` measures the nested discovery and transfer rounds
    at 0.55 s of Hello World's 0.82 s.
- **Elaboration is a cached, parallel expansion graph.**
  - Each node is a generator plus its parameter values.
  - Independent instantiations are elaborated concurrently. Compile-time
    evaluation is a synchronization point.
  - Mojito's counterparts are the discovery rounds and the specialization
    key (`mojito-symbol` mangling).
- **The compile-time interpreter is a bytecode VM, not an op walker.**
  - It compiles functions to `FunctionIRBytecode`.
  - It evaluates ops through dedicated interpreter hooks or their fold hooks.
  - It keeps an emulated address space for loads and stores, and needs no
    JIT.
  - This is Mojito's VM-backed CTFE design, and it supports keeping the VM
    as the interpreter in any Pliron-hosted future.
- **Precompiled packages cache checked templates, not binaries.**
  - `mojo precompile` writes a `.mojoc` file: the `lit.package` op with its
    function bodies, as MLIR bytecode.
  - Importers load bodies lazily and elaborate them with their own
    parameter values.
  - The file is tied to the compiler version, and the walkthrough calls it a
    cache, not a distribution format.
  - Mojito's counterpart is the precompiled-prelude cache
    `docs/performance.md` proposes. Caching the prelude as checked templates
    needs the template checking in §1. Caching elaborated clones would have
    to be redone for every new instantiation.
- **Parsing is lazy in three passes.**
  - Name resolution registers declarations.
  - Signature resolution types them.
  - Body resolution parses a body only when it is needed.
  - Source and binary packages both materialize bodies on first use. Mojito
    parses and checks its linked stdlib again in every compiler process.
- **Debug info is parametric.**
  - A `debuginfo` type can wrap a parameter
    (`!debuginfo.unresolved<!kgen.param<T>>`).
  - Elaboration concretizes it by the same substitution that concretizes
    code, with no special handling.
  - A Mojito parametric layer should carry debug types as ordinary attributes
    for the same reason.
- **Calling conventions are lowered before concrete optimization.**
  - `LowerArgConventions` and `LowerCallingConventions` rewrite the
    `byref_result`, `byref_error`, pack, and variant forms first.
  - `SROA`, `SCCP`, `AutomaticInline`, `LoopUnrolling`, and
    `DeadArgumentElimination` then run on concrete `kgen.func`.
  - That is the job the pivot plan gives `mojito.abi`.
- **Lowering is not one-way.** `RaiseForLoops` runs on concrete `kgen.func`
  and turns a simple `hlcf.loop` with no early exits into an `hlcf.for`, the
  form `LoopUnrolling` works on. A higher level can be recovered when a later
  pass wants it back.

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
- smolnero, "First-Principle Mojo: KGEN, MLIR, and What a Compiler Chooses to
  Remember" (2026-08-30):
  <https://smolnero.com/posts/first-principle-mojo-kgen-mlir-and-what-a-compiler-chooses-to-remember>
- Modular's dialect definitions and compiler docs (local clone
  `~/src/mojo/repos/modular`): `Mojo/include/Mojo/{LIT,KGEN,POP,HLCF,CO}Dialect/*.td`,
  `Mojo/docs/compiler/MojoCompilerWalkthrough.md`,
  `Mojo/docs/compiler/manual/PassesAndIR.md`, `Mojo/proposals/origin-design.md`,
  `Mojo/docs/stdlib/internal/pop_dialect.md`
- Pliron derive macros and interfaces: `pliron-derive/src/lib.rs`,
  `src/builtin/op_interfaces.rs`, `src/irbuild/`, `src/pass.rs` at the pin
- Local evidence: `docs/notes/pliron-stage0.md` (facility matrix and findings),
  `crates/mojito-comptime/src/comptime/ctfe.rs` (VM-backed CTFE),
  the pinned Pliron checkout at revision `477e6b0e`.
