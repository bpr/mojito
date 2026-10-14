# Structured Control Flow Above the Waist: Staged Plan

**Status:** plan recorded 2026-10-14 (R859); stages 1 (R870), 2 (R871),
3 (R872), 4 (R873), and 5 (R874) landed 2026-10-14, and the `comptime for`
body's `has_next` test (R885) with them. The scheduled tasks are on
[`docs/roadmap.md`](roadmap.md) track `mojo-shape`, one entry per stage below.
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
    (`ExprNodes.cpp`), unless the ternary's condition is a parameter
    value, which is `hlcf.comptime.if`.
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
  one labelled `hlcf.loop` per iterator, N+1 of them, since the final,
  exhausted iterator runs the body's stop arm, and `simplifyParamForLoop`
  folds the trivial ones away.
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
  statements (`CheckedStmtOp`) and, since stage 3, lowers them into a
  region tree: `if`/`while`/`for`/`comptime if`/`comptime for` are
  `HirInstr::If`/`Loop`/`ComptimeIf`/`ComptimeFor` region ops and
  `break`/`continue` labelled region terminators, and, since stage 5, a
  `try` is `HirInstr::Try` whose parts are regions of the same walk. No
  terminator jumps between blocks, and dead code after a sealed block is
  erased, so every region is one block.
- **MIR** (`crates/mojito-mir/src/mir/ir.rs`) below `lower-control-flow`
  is a CFG: `MirTerm` is `Jump`, `Branch`, `ComptimeBranch`, `ComptimeFor`,
  `Return`, `ReturnWithCleanup`, `TryYield`, and `EscapeJump`.
  - The region ops are `MirInstr::Try` and, since stage 1, `If`, `Loop`,
    `ComptimeIf`, and `ComptimeFor`. Each
    region is a `Vec<MirBlock>` sharing the function's registers and
    slots, walked by `instruction_regions`/`instruction_regions_mut` and
    `block_successors`. A structured `try` part completes by `TryYield`
    (`lit.try.yield`) and leaves to a loop by a labelled `Break`/`Continue`,
    which `lower-control-flow` turns into `EscapeJump`.
  - `lower_expr/ctrl.rs` builds `short_circuit`, `ternary`, `compare_chain`,
    and `comprehension_clauses` as `If`/`Loop` region ops (stage 2), MIR
    lowering builds HIR's region ops as the same ops (stage 3,
    `Flatten::hir_region`), and it ends each body with
    `lower-control-flow`, so every phase below it still sees blocks.
    Before that pass, `verify_structured_shape` checks the body, a finding
    being a MIR invariant error (stage 5).
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
- **Versions.** MIR text `1.54`, `PACKAGE_FORMAT` `"mojito-package 55"`
  since stage 1. Packaged bodies are MIR, so structured bodies change both.

## Target

- **One block per region.** A function body is one block. Every nested
  region (`if` arm, `elif` condition, loop body, `try` part, compile-time
  arm or body) is one `MirBlock`.
- **Region ops**, new `MirInstr` variants, one per upstream op:
  - `If { results, cond, then, elifs, orelse }` — `hlcf.if`. Each `elifs`
    entry is a condition region ending in `ElifCond(reg)` and an arm.
  - `Loop { label, body }` — `hlcf.loop`. Loop-carried values stay in slots
    until register promotion (R787) brings operands and results.
  - `ComptimeIf { results, conds, arms, orelse }` — `hlcf.comptime.if`:
    one arm per condition, then the else region.
  - `ComptimeFor { binder, source, body, orelse }` —
    `hlcf.comptime.for`. `orelse` is the shape `LowerSemanticCF` leaves
    and the elaborator asserts: one block holding `Unreachable`. MIR is
    that pass's output, so it has no `comptime.for.goto.else`: the body is
    one block holding a `ComptimeIf` on `has_next` over the binder, whose
    arm is the suite and whose else arm the stop arm, ending in
    `ComptimeBreak` (R885).
  - `Try` keeps its four parts, each one region — `lit.try`, which upstream
    keeps beside `hlcf`.
- **Region terminators**, new `MirTerm` variants: `Yield(regs)`,
  `ElifCond(reg)`, `Break { label }`, `Continue { label }`,
  `ComptimeYield(regs)` (`hlcf.comptime.yield`, which ends a
  `ComptimeIf` arm, where `hlcf.yield` may not), `ComptimeBreak`,
  `ComptimeContinue`, and `Unreachable`. A label is optional and need not
  be unique: a labelled `break`/`continue` names the nearest enclosing
  loop carrying it. `comptime.for.break`/`continue` carry no label and
  name the nearest `comptime for`. `Return`/`ReturnWithCleanup` stay and may end any region,
  as `hlcf.return` is `ReturnLike`. A `break` or `continue` crossing a
  `try` is a labelled `Break`/`Continue` naming its loop, replacing
  `EscapeJump`. A `try` part completes by `TryYield`, upstream's
  `lit.try.yield` (`LIT_TryYieldOp`, `HasParent<"TryOp">`), which stays,
  since `lit.try` stays a region op through the waist.
- **One CFG pass.** `lower-control-flow` (upstream's `LowerControlFlow`) is
  the only code that produces `Jump`/`Branch`. It starts as a transitional
  pass right after MIR lowering, moves down one consumer at a time, and
  ends inside the Pliron lowering. The VM then executes region ops, as
  upstream's interpreter does, and `Jump`, `Branch`, `ComptimeBranch`, the
  `ComptimeFor` terminator, and `EscapeJump` leave `MirTerm`.
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
  iterator, N+1 of them, each deciding its leading `has_next` test before
  its arms, with `ComptimeBreak`/`ComptimeContinue` retargeted to that
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

1. **Vocabulary.** *Landed (R870).* Region ops and terminators, MIR text
   read/write (1.54, package 55), `verify_structured_function_in`, walkers
   generalized over every region op (`instruction_regions`,
   `block_successors`, `prune_block_list`, `renumber_escapes`, and the
   analysis duplicates), and `mir::lower_control_flow`, registered as the
   pass `lower-control-flow`, with cases under
   `tests/mir_passes/lower-control-flow/`. No producer yet.
   - The transitional pass lowers `Unreachable` to `Return(None)`, MIR's
     unreachable-scaffolding placeholder, until stage 14 emits LLVM's
     `unreachable`. Results go through one slot per result, stored at each
     `yield` and forwarded at the join, the shape a ternary has today.
   - A `break`/`continue` leaving a `try` part to a function-level loop
     lowers to `EscapeJump`; one whose loop sits in an enclosing `try`
     part lowers to today's unsupported boundary.
2. **Expressions.** *Landed (R871).* `short_circuit`, `ternary`,
   `compare_chain`, and `comprehension_clauses` emit `If` (a comprehension
   also a `Loop`) inside today's blocks, each region one block built by
   `Flatten::region`, so MIR lowering creates no block of its own.
   - `and`/`or` yield `Bool` (Mojito accepts no other operand). A ternary
     the checker proved register-passable (`RegisterPassableResult`)
     yields its value; any other result is stored by each arm into a
     result slot forwarded after a result-less `If`, upstream's
     destination buffer.
   - A comparison chain nests one `If` per link inside the previous link's
     then arm, its else arm yielding the previous comparison, as
     `ChainedCmpOpNode::emitNextCmp` does.
   - A comprehension `for` clause is a `Loop` whose body tests
     `__has_next__`, breaks in the `If`'s else arm, calls `__next__`, runs
     the rest of the clauses, and continues; an `if` clause is a
     result-less `If` (`emitForStmt`, `emitIfClause`).
   - `lower_cfg_nested` ends with `lower_control_flow`, before
     `close_register_types` types the result registers through the pass's
     `$yield` slots; `mir::lower_cfg_structured` returns the structured
     form for phase tests.
3. **Statements.** *Landed (R872).* HIR's `Cfg` is a region tree
   (`Cfg::root`, `HirRegion`): `if`/`elif` are `If`, each `elif` a
   condition region; `while`/`for` are a `Loop`, labelled uniquely per
   function, whose body opens with an `If` on the condition or has-next
   flag, its else arm holding the loop `else` (lowered outside the loop's
   frame) and then `Break` (`lowerLITLoop`); `break`/`continue` are
   labelled. `Flatten::hir_region` lowers each region to a block list,
   and a comprehension loop takes its label from the same counter
   (`Flatten::loop_label`), so every loop of a function is labelled.
   - `comptime for` moved here from stage 4: a `comptime for` body's
     `break` inside a run-time `if` region cannot jump to a block outside
     that region, so it is `ComptimeBreak`, which only a `ComptimeFor`
     region op can resolve. HIR builds `HirInstr::ComptimeFor`, MIR
     lowering the `ComptimeFor` region op.
   - A `break`/`continue` in a `try` part names its loop's label, and
     `lower-control-flow` makes the `EscapeJump`; HIR has no escape
     terminator, and MIR lowering threads no outer block map.
   - Dead code after a sealed block is erased; stage 5 records how.
4. **Compile-time forms.** *Landed (R873).* HIR builds `comptime if`/
   `elif`/`else` as one `HirInstr::ComptimeIf`, every condition first and
   then one arm region per condition plus the else region, each arm that
   completes ended by `Terminator::ComptimeYield`, as `ParserStmts.cpp`
   builds `hlcf.comptime.if`. MIR lowering builds `MirInstr::ComptimeIf`,
   which the transitional pass lowers to today's `ComptimeBranch` chain.
   HIR's `Jump` and `ComptimeBranch` terminators are gone, so `Cfg::g` has
   no edge. `comptime for` landed with stage 3. Its body opens with the
   `ComptimeIf` on `has_next` (R885), and the elaborator already makes
   upstream's N+1 copies, each deciding that test first and keeping its
   taken arm alone (`mono/unroll.rs`). The CFG `ComptimeFor` terminator
   keeps its `exit` edge, which no executed path now takes, until stages
   11 and 14 delete the terminator.
5. **`try`.** *Landed (R874).* HIR lowers a `try`'s parts as regions of
   the function's own walk (`Lower::try_statement`, `HirExcept`), as the
   parser builds `lit.try`, where each part was a separately built HIR CFG;
   MIR lowers them through `Flatten::try_part`, which scopes the part's
   loans and aliases as before. A region op after a sealed block is erased,
   as `eraseOpToEndOfBlock` does, so every region is one block. The
   erasure happens on the checked statement list, before lowering
   (`CheckedTables::live_statements`, R889), and covers the statements
   after a `raise` too. A part
   completes by `TryYield` (`try.yield {}`, MIR text 1.59), the renamed
   `FallOff`; a `break`/`continue` in a part was already a labelled
   `Break`/`Continue` (stage 3). The single-block rule runs in production:
   `lower_cfg_nested` records `verify_structured_shape`'s findings as
   invariant errors before `lower-control-flow`.
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

- The one-session sizes of stages 6–11 are estimates (HIR is about
  1,400 lines; the ownership and drop analyses about 9,500). Each entry may
  be split by its planning session.
- Where upstream lowers `lit.try` was not traced. `SimplifyCF` still names
  `LIT::TryOp`, so the plan keeps Mojito's `Try` a region op through the
  waist.
- Whether the pin accepts `comptime for … else` was not run here.
  Upstream's parser does parse it (`ParserStmts.cpp`), and R797 records
  the pin running it.
