# Structured Control Flow Above the Waist: Staged Plan

**Status:** plan recorded 2026-10-14 (R859); stages 1 (R870), 2 (R871),
3 (R872), 4 (R873), and 5 (R874) landed 2026-10-14, and the `comptime for`
body's `has_next` test (R885) with them; stages 6 (R875), 7 (R876),
8 (R877), and 9 (R878) landed 2026-10-21.
The scheduled tasks are on [`docs/roadmap.md`](roadmap.md) track `pmir`,
one entry per stage below.
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
    and `comprehension_clauses` as `If`/`Loop` region ops (stage 2), and MIR
    lowering builds HIR's region ops as the same ops (stage 3,
    `Flatten::hir_region`). `verify_structured_shape` checks each body, a
    finding being a MIR invariant error (stage 5); register typing,
    pre-drop verification, the ownership checks, drop elaboration, and
    every pass after it, the pre-elaboration pipeline included, read the
    structured body (stages 6–10); the elaborator runs
    `lower-control-flow` on each template where it first fetches it
    (`Bodies::get`), so it and every phase after it still see blocks.
- **The ownership checks and drop elaboration walk regions.** The move and
  initialization scan, carrying interior origins, walks the structure as
  `CheckLifetimes` does (stages 6–7), and so does the loan check. Drop
  elaboration places its drops in the regions as `DestructorInsertion`
  does (stage 8), and field drops, copy elision, destructor and
  constructor naming, and the copy/move lowering read the same regions
  after it (stage 9).
- **The pre-elaboration passes read regions.** `sccp` walks a structured
  body as `SCCP` walks `hlcf`, `canonicalize` applies the `hlcf` patterns
  (`analysis/canonicalize/hlcf.rs`), `mem-2-reg` solves reaching stores
  over regions, and `inline-parametric` wraps every inlined body in an
  `inlined_cf_scope` loop (stage 10). `canonicalize`'s R836 CFG rewrites
  (pass-through threading, identical-successor folds, single-predecessor
  merges) run only on the post-elaboration pipeline's lowered bodies.
- **The elaborator recovers structure.** `mono/unroll.rs`'s `loop_body`
  finds a `comptime for` body as the blocks its `body` target dominates, and
  `outermost_loop` finds the enclosing loop. A region op would hold both.
- **Versions.** MIR text `1.54`, `PACKAGE_FORMAT` `"mojito-package 55"`
  since stage 1. A package holds checked units, never MIR, so moving the
  lowering point changes neither.

## Target

- **One block per region.** A function body is one block. Every nested
  region (`if` arm, `elif` condition, loop body, `try` part, compile-time
  arm or body) is one `MirBlock`.
- **Region ops**, new `MirInstr` variants, one per upstream op:
  - `If { results, cond, then, elifs, orelse }` — `hlcf.if`. Each `elifs`
    entry is a condition region ending in `ElifCond(reg)` and an arm.
  - `Loop { label, operands, args, results, body }` — `hlcf.loop`, its
    body's arguments defined by its operands and each `Continue`'s values,
    its results by each `Break`'s. Loop-carried values stay in slots until
    register promotion of merged slots (R927) produces them.
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
  `ElifCond(reg)`, `Break { label, values }`, `Continue { label, values }`,
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
6. **Move and initialization checks on regions**, as `CheckLifetimes`.
   *Landed (R875).* `analysis/moves.rs` walks a structured body as
   `UninitializedValueScan` does: an `If` joins its arms (`checkIfOp`, an
   elif's condition region starting from the state the previous one left),
   a `ComptimeIf` joins arms that each start from the entry state
   (`checkIfLikeOp`), a `Match` case starts from the join of the previous
   case's `MatchNext` states and its live-out joins every `MatchComplete`
   and the else region (`checkMatchOp`), and a `Loop` or `ComptimeFor`
   re-walks its body until its continue set stops changing, its break set
   being its live-out (`checkLoopOp`).
   - A frame stack (now `region_walk.rs`'s `Frame`) resolves each `Break`/`Continue`,
     `ComptimeBreak`/`ComptimeContinue`, and `MatchNext`/`MatchComplete` by
     `lower-control-flow`'s rules, so the state at a `break` out of a `try`
     reaches its loop, where it reached the function's exit before.
   - A `try` with a `finally` holds back the states its parts hand a frame
     and walks each through the `finally` first, as `lowerTryFinally`'s
     copy before each exiting terminator would. `Unreachable` checks
     nothing (`checkTerminatorOp` on `hlcf.unreachable`).
   - `lower_cfg_nested` no longer lowers: `analysis::check_ownership_program`
     runs `lower-control-flow` on each body between its move scan and the
     borrow checks, and `lower_checked_program` lowers every body for
     phase tests. Register typing types an `If`'s results from its arms'
     yields, and named-result materialization reaches the region ops' exits.
   - The CFG walker joins an `EscapeJump`'s state into its target block, so
     a lowered body's store after the loop sees the state a `break` out of
     a `try` left; store drops read structured bodies since stage 8.
7. **Borrow checks on regions** (loans, interior). *Landed (R876).* The
   lowering point moved below the checks: `check_ownership_program` runs
   `lower-control-flow` on each body after its loan check.
   - The forward walk is generic (`analysis/region_walk.rs`'s
     `ForwardScan`), and the interior-origin check joined the move scan as
     upstream's `TrackedAndInteriorLiveness`: one walk
     (`moves.rs`'s `analyze_lifetimes`) carries both states and reports
     the first complaint in walk order. The state a container mutation
     leaves at a `break`/`continue` out of a `try` now reaches the loop, as
     the move state did in stage 6. A use through a reference whose
     origin's root was moved reports the use of the uninitialized root, as
     upstream does.
   - The loan check has no upstream counterpart (it is the kept
     strictness of `docs/non-goals.md`), so it only reads regions: a
     backward walk shaped as `DestructorInsertion`'s (`BackwardScan`)
     gives reference liveness and the forward walk the loan generations.
     A `try` is one instruction to it, using every variable its parts
     read; nothing inside a `try` is loan-checked.
   - Register loans are not a check: they feed drop elaboration, so they
     moved to stage 8.
8. **Drop elaboration on regions**, as `DestructorInsertion`. *Landed
   (R877).* `elaborate_drops` reads the structured body and
   `elaborate_drops_function` runs `lower-control-flow` right after it;
   store drops, before it, splice into every region.
   - Two walks give the facts. `DropFacts` (`register_loans.rs`) is one
     forward walk (`walk_forward`, entering every region op) recording the
     loan generations reaching each instruction and block and the roots
     each instruction consumes through its operand registers; the CFG
     register-loan fixpoint and its register liveness are gone. The
     backward walk (`walk_backward`, `DropLiveness`) gives variable
     liveness and reports each block, region op, and `try` to a
     `BackwardRecorder`.
   - The backward walk enters `try` as `checkTryOp` walks it, keeping
     Mojito's `finally`: the `finally` from the `try`'s live-out, then the
     handler and `else` from its live-in, then the body. A raising
     instruction keeps the raise channel live (the handler's live-in, or
     the `finally`'s and the enclosing raise's), a `return` the `finally`
     walked from the enclosing return set, and a `break`/`continue`
     leaving a part the `finally` walked from its frame's set, as
     `lowerTryFinally`'s copy before the exit would. `ReferenceLiveness`
     still treats a `try` as one instruction (`BackwardScan::enters`).
   - A splice pass places each drop after its last use, at the entry of an
     `if` or `comptime if` arm that leaves out a value a sibling keeps
     (`destroyValuesAtEntryIfNeeded`, unified per elif step as `checkIfOp`
     does), or before a block's terminator when its yielded values were
     the last use. No block is added, so critical-edge splitting is gone.
   - Escape cleanup lists are gone: what dies on a `break` out of a `try`
     dies before it, in the part. A `try`'s raise-edge list now destroys
     what a raise in the body holds that the raise's observers do not read,
     as the entry destructors of upstream's error arm do; `return` cleanup
     lists are computed as before (R829, R884).
   - Pass tests feed structured bodies; `analysis::opt` refuses
     `elaborate-drops` on a lowered one.
9. **Field drops and copy elision on regions.** *Landed (R878).* Every
   pass between drop elaboration and the pre-elaboration pipeline reads
   the structured body, and both seams run `lower-control-flow` after
   `lower-lit`, before `PIPELINE` (a seam step, not a pipeline pass: the
   pipeline can be switched off and skips `__toplevel__`).
   - `lower-lit`, constructor naming, the copy/move lowering, and
     destructor naming already walked every region.
   - `lower_whole_drops`'s definite-initialization fold
     (`mir/drop_calls.rs`) follows every `hlcf` op as `CheckLifetimes`
     walks it, with a frame stack resolving `break`/`continue`/`match`
     exits by the rule `lower-control-flow` uses (`mir::RegionFrame`,
     `mir::region_exit`). A region exit leaving a `try` part arrives as it
     leaves, or past a `finally` holding no whole value, as an escape
     from a lowered part does.
   - Copy elision counts a region terminator's registers
     (`mir::terminator_regs`), and its forward scans stop at a region op
     other than `try`, where lowering would have ended the block.
   - Field drops ride the shared walks: `FieldFacts` (forward register
     provenance), `FieldLiveness` (backward), and a splice placing each
     field's drop after its last use, at an `if`/`comptime if` arm's entry
     (`destroyValuesAtEntryIfNeeded`), and at a handler's entry. A raise
     keeps live only what its handler reads, at real raise points, so a
     field a handler reads now dies right after the body's last raising
     call, as on the pin. Edge drops and critical-edge splitting are
     gone. They stay a second walk after drop elaboration, where upstream
     tracks fields in `DestructorInsertion`'s one walk (filed on the
     roadmap).
   - `elaborate-drops` pass output is structured.
10. **Pre-elaboration pipeline.** Three stages, one session each:
    - **10a (landed, R879).** The walkers that entered only `try`
      (`fold::replace_folded`, `callable_values.rs`, `cut_try_cleanups`)
      read every region; `sccp` walks regions as `SCCP` does;
      `canonicalize` gains `RemoveStaticCondition`, `ConditionPropagation`,
      both hoists, `HoistYieldResults`, `RemoveUnusedResults`,
      `RemoveDeadLoop`, and `ComptimeIfOp::canonicalize` with
      `checkTrivialBlocks`; `mem-2-reg` promotes by structured dominance.
      R836's CFG rewrites stay for lowered bodies until stage 12.
    - **10b (R1253, landed).** `inline-parametric` inlines structured
      callees, wrapping every inlined body in one loop labelled
      `inlined_cf_scope`.
    - **10c (R1254, landed).** The seams stop lowering, and the elaborator
      lowers each template where it first fetches it (`Bodies::get`). An
      inlined result is forwarded, so no move initializer runs, and the
      elaborator's `comptime for` body recovery reads one dominator tree
      per step (`mir::ComptimeForBodies`).
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
