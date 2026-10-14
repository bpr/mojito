# Structured Control Flow Above the Waist: Staged Plan

**Status:** plan recorded 2026-10-14 (R859). The scheduled tasks are on
[`docs/roadmap.md`](roadmap.md) track `pmir`, one entry per stage below.
This document is their design record and is updated as stages land. It is
the structured-control part of
[`docs/parametric-mir-plan.md`](parametric-mir-plan.md).

**Goal.** Above the MIR waist, a body's control flow is a tree of
single-block regions, as upstream's generators hold it in the `hlcf`
dialect. A CFG first appears where upstream's first appears: in the lowering
to the native backend.

## What Mojo does

Checked in `~/src/mojo/repos/modular/Mojo`.

- **The parser emits structured ops** (`lib/MojoParser`).
  - `if` is `hlcf.if` (`ParserStmts.cpp`, `IREmitter.cpp`).
  - `while` and `for` are `lit.loop` with an `hlcf.if` on the condition or
    the has-next flag.
  - Short-circuit and ternary expressions are `hlcf.if` with results
    (`ExprNodes.cpp`).
  - `comptime if` and `comptime for` are `hlcf.comptime.if` and
    `hlcf.comptime.for`. No `kgen` op carries control flow.
  - `try` is `lit.try`.
- **`LowerSemanticCF`** (`lib/LowerLIT/LowerSemanticCF.cpp`) runs before
  `CheckLifetimes`, and its output is still structured.
  - `lit.loop` becomes `hlcf.loop` with a unique label, and `lit.break`/
    `lit.continue` become labelled `hlcf.break`/`hlcf.continue`.
  - A loop's `else` goes on the loop's normal exit inside its region,
    followed by a `break` (`lowerLITLoop`).
  - `lowerTryFinally` copies a `finally` body before each terminator that
    leaves the `try`.
  - An infinite loop ends in `hlcf.unreachable`.
- **The `hlcf` dialect** (`include/Mojo/HLCFDialect/HLCFOps.td`) has 20 op
  definitions: `for`, `loop`, `switch`, `continue`, `break`, `yield`,
  `for.yield`, `if.elifcond.yield`, `if`, `match.next`, `match.complete`,
  `match`, `comptime.for`, `comptime.for.break`, `comptime.for.continue`,
  `comptime.for.goto.else`, `comptime.if`, `comptime.yield`, `return`, and
  `unreachable`.
  - Every region is `SizedRegion<1>`: one block, never a CFG.
  - `hlcf.if` has results, supplied by its arms' `hlcf.yield`. Each `elif`
    condition is computed in its own region, ended by `if.elifcond.yield`.
  - `hlcf.loop` has an optional label, loop-carried operands, and results.
  - Every parent op implements `InterpreterOpInterface`, so the elaborator's
    interpreter runs structured control flow directly.
- **`CheckLifetimes` walks the structure** (`lib/LowerLIT/CheckLifetimes.cpp`).
  `checkIfOp`/`checkMatchOp` join arms, `checkLoopOp` iterates a loop (and a
  `ComptimeForOp`) to a fixpoint, and a labelled `break`/`continue` is
  resolved through its enclosing op. `DestructorInsertion` works the same
  way.
- **The elaborator works on regions** (`lib/Elaborator/Elaborator.cpp`).
  `processComptimeIfOp` keeps the arm taken. `processComptimeForOp` makes
  one labelled `hlcf.loop` per iteration, and `simplifyParamForLoop` folds
  the trivial ones away.
- **Canonicalization is structural**
  (`lib/HLCFDialect/HLCFOpsCanonicalizers.cpp`).
  - On `IfOp`: `HoistYieldResults`, `RemoveStaticCondition`,
    `HoistUnconditionalReturn`, `HoistConditionalReturn`,
    `RemoveUnusedResults`, and elif promotion.
  - On `LoopOp`: `RemoveDeadLoop`, `RemoveUnusedLoopResults`,
    `RemoveUnusedLoopArgs`. On `ForOp`: `RemoveNoopLoop`. On `ComptimeIfOp`:
    `checkTrivialBlocks`.
- **Pass order** (`lib/Compiler/Pipeline/Pipeline.cpp`).
  `LowerSemanticCF` → `CheckLifetimes` → `LowerLIT` → `Canonicalizer`/`SCCP`
  on each generator → `ElaborateGenerators` → the function pipelines.
  `SimplifyCF` (`lib/Transforms/SimplifyCF.cpp`, trivial-loop removal) runs
  only there, after elaboration, as does `LowerLoops`.
  `lib/KGENToLLVM/LowerControlFlow.cpp` lowers `hlcf` to LLVM blocks
  through a `ControlFlowTree` analysis. No `cf` dialect is used anywhere.

## What Mojito does

- **HIR** (`crates/mojito-hir/src/hir.rs`) reads structured checked
  statements (`CheckedStmtOp`) and lowers `if`/`while`/`for`/`break`/
  `continue`/`comptime if`/`comptime for` into a `Cfg`
  (`StableGraph<BasicBlock, ()>`) of blocks and jumps. Each `try` part is
  its own region CFG (`build_seeded_checked_region`).
- **MIR** (`crates/mojito-mir/src/mir/ir.rs`) is a CFG: `MirTerm` is `Jump`,
  `Branch`, `ComptimeBranch`, `ComptimeFor`, `Return`, `ReturnWithCleanup`,
  `FallOff`, and `EscapeJump`.
  - One region op exists already: `MirInstr::Try`. Its parts are each a
    `Vec<MirBlock>` mini-CFG sharing the function's registers and slots,
    walked by `try_regions`/`try_regions_mut` and `block_successors`, and
    left through `EscapeJump`/`FallOff`.
  - `lower_expr/ctrl.rs` builds `short_circuit`, `ternary`, `compare_chain`,
    and `comprehension_clauses` as blocks.
- **The analyses work on edges.** Ownership, borrow, and initialization
  checks are dataflow over blocks; `analysis/drops.rs` puts drops on edges
  and splits critical edges.
- **`canonicalize`** (`analysis/canonicalize.rs`, R836) threads
  pass-through blocks, folds identical-successor branches, and merges
  single-predecessor blocks: MLIR `cf` patterns standing in for structure
  MIR does not have.
- **The elaborator recovers structure.** `mono/unroll.rs`'s `loop_body`
  finds a `comptime for` body as the blocks its `body` target dominates, and
  `outermost_loop` finds the enclosing loop. A region op would hold both.
- **Versions.** MIR text `1.53`, `PACKAGE_FORMAT` `"mojito-package 54"`.
  Packaged bodies are MIR, so structured bodies change both.

## Target

- **One block per region.** A function body is one block. Every nested
  region (`if` arm, `elif` condition, loop body, `try` part, compile-time
  arm or body) is one `MirBlock`.
- **Region ops**, new `MirInstr` variants, one per upstream op:
  - `If { results, cond, then, elifs, orelse }` — `hlcf.if`. Each `elifs`
    entry is a condition region ending in `ElifCond(reg)` and an arm.
  - `Loop { label, body }` — `hlcf.loop`. Loop-carried values stay in slots
    until register promotion (R787) brings operands and results.
  - `ComptimeIf { cond, then, orelse }` — `hlcf.comptime.if`.
  - `ComptimeFor { label, binder, slot, source, body }` —
    `hlcf.comptime.for`. No `else` until the checker accepts
    `comptime for … else`, so no `comptime.for.goto.else`.
  - `Try` keeps its four parts, each one region — `lit.try`, which upstream
    keeps beside `hlcf`.
- **Region terminators**, new `MirTerm` variants: `Yield(regs)`,
  `ElifCond(reg)`, `Break { label }`, `Continue { label }`,
  `ComptimeBreak { label }`, `ComptimeContinue { label }`, and
  `Unreachable`. `Return`/`ReturnWithCleanup` stay and may end any region,
  as `hlcf.return` is `ReturnLike`. A `break` or `continue` crossing a
  `try` is a labelled `Break`/`Continue` naming its loop, replacing
  `EscapeJump`; `FallOff` becomes `Yield([])`.
- **One CFG pass.** `lower-control-flow` (upstream's `LowerControlFlow`) is
  the only code that produces `Jump`/`Branch`. It starts as a transitional
  pass right after MIR lowering, moves down one consumer at a time, and
  ends inside the Pliron lowering. The VM then executes region ops, as
  upstream's interpreter does, and `Jump`, `Branch`, `ComptimeBranch`, the
  `ComptimeFor` terminator, `FallOff`, and `EscapeJump` leave `MirTerm`.
- **The verifier enforces it.** `mir::verify` gains a structured mode —
  every region one block, only region terminators — applied to a body until
  the point where it is lowered.
- **`hlcf` canonicalizations replace R836's CFG rewrites.**
  `RemoveStaticCondition`, `HoistYieldResults`, `RemoveUnusedResults`,
  `HoistUnconditionalReturn`, and `HoistConditionalReturn` on `If`;
  `RemoveDeadLoop` on `Loop`; `checkTrivialBlocks` on `ComptimeIf`. The
  threading, identical-successor, and single-predecessor patterns are
  deleted, since the blocks they remove no longer exist. `SimplifyCF`'s
  trivial-loop removal joins the post-elaboration pipeline, where upstream
  runs it.
- **The elaborator works on regions.** A `ComptimeIf` is replaced by its
  taken arm. A `ComptimeFor` unrolls into one labelled `Loop` per
  iteration, with `ComptimeBreak`/`ComptimeContinue` retargeted to that
  loop, and the trivial wrappers are folded away. `loop_body` and
  `outermost_loop` are deleted.

## Stages

The rule for every stage: **producers first, then the lowering point moves
down.** A stage either makes one more construct structured, with the
transitional `lower-control-flow` right after MIR lowering, or moves the
lowering point below one more consumer. While producers land, a region may
still hold several blocks, as `Try`'s do today, so expression region ops
can sit inside a statement CFG; the single-block rule switches on with the
last producer. Every stage keeps program output unchanged; the
`pre_elaboration` differential and the corpus are the oracle.

1. **Vocabulary.** Region ops and terminators, MIR text read/write (version
   bump), the structured verifier mode, walkers generalized over every
   region op (`try_regions`, `block_successors`, `prune_block_list`,
   `renumber_escapes`), and `lower-control-flow` with pass tests under
   `tests/mir_passes/lower-control-flow/`. No producer yet.
2. **Expressions.** `short_circuit`, `ternary`, `compare_chain`, and
   `comprehension_clauses` emit `If` with results (a comprehension also a
   `Loop`) inside today's blocks.
3. **Statements.** HIR's `Cfg` becomes a region tree: `if`/`elif` are `If`;
   `while`/`for` are a `Loop` with an `If` on the condition or has-next and
   the loop `else` on the normal exit; `break`/`continue` are labelled.
   `mir.rs::lower_cfg` walks the tree.
4. **Compile-time forms.** `comptime if`/`for` become `ComptimeIf`/
   `ComptimeFor`, lowered by the transitional pass to today's terminators.
5. **`try`.** Each part is one region; `EscapeJump`/`FallOff` become
   `Break`/`Continue`/`Yield`. The single-block rule switches on.
6. **Move and initialization checks on regions**, as `CheckLifetimes`:
   an `If` joins its arms, a `Loop` iterates to a fixpoint, and the state at
   a `Break`/`Continue` flows to its labelled loop.
7. **Borrow checks on regions** (loans, register loans, interior). The
   lowering point moves below the checks.
8. **Drop elaboration on regions**, as `DestructorInsertion`: a drop goes
   before a region terminator, at a region's entry, or after the op.
   Critical-edge splitting is deleted.
9. **Field drops, store drops, and copy elision on regions.** The lowering
   point moves below them.
10. **Pre-elaboration pipeline and package.** `canonicalize` gains the
    `hlcf` patterns and loses R836's; `sccp` walks regions; package bodies
    are structured (`PACKAGE_FORMAT` bump). The lowering point moves to the
    elaborator's entry.
11. **Elaborator.** Instantiation, substitution, and `ComptimeIf`/
    `ComptimeFor` work on regions; the dominator code is deleted; a CTFE
    fragment is lowered before `call_concrete`. The lowering point moves to
    the end of elaboration.
12. **Post-elaboration pipeline.** `sccp`/`canonicalize` on regions, plus
    `SimplifyCF`'s trivial-loop removal. The lowering point moves to the
    backend boundary (`src/compiler.rs`, `src/artifact.rs`, the Pliron
    entry).
13. **VM.** It executes region ops, generalizing `run_region`/`exec_try`'s
    `Flow`. The VM path and CTFE stop lowering.
14. **Pliron.** `lower/term.rs` and `try_flow.rs` lower region ops to LLVM
    blocks, as `LowerControlFlow` does. The transitional pass and the CFG
    terminators are deleted.

Each stage moves Mojito strictly closer to upstream; no stage adopts a
Mojito-only shape.

## Gaps kept open

- **`finally` placement.** The VM runs a `finally` dynamically on each exit,
  where upstream copies it before each exiting terminator
  (`lowerTryFinally`). Filed on the roadmap beside the stages.
- **HIR as a layer.** Upstream has no HIR between its parser's ops and
  `CheckLifetimes`, only `LowerSemanticCF`. Whether HIR's remaining job
  (name-to-slot resolution) folds into MIR lowering is a later question,
  not filed until an entry can name its lever.

## Unverified

- The one-session sizes of stages 3 and 6–11 are estimates (HIR is about
  1,400 lines; the ownership and drop analyses about 9,500). Each entry may
  be split by its planning session.
- Where upstream lowers `lit.try` was not traced. `SimplifyCF` still names
  `LIT::TryOp`, so the plan keeps Mojito's `Try` a region op through the
  waist.
- Whether the pin accepts `comptime for … else` was not checked.
