# Structured Control Flow Above the Waist: Staged Plan

**Status:** plan recorded 2026-10-14 (R859); stages 1 (R870), 2 (R871),
3 (R872), 4 (R873), and 5 (R874) landed 2026-10-14, and the `comptime for`
body's `has_next` test (R885) with them; stages 6 (R875), 7 (R876),
8 (R877), 9 (R878), 10 (R879, R1253, R1254), 11 (R880), 12 (R881), and
13 (R882) landed 2026-10-21.
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
  - `try` is `lit.try`, a `ControlFlowNode` like `hlcf`'s ops
    (`include/Mojo/LITDialect/LITOps.td`), so `LowerControlFlow` lowers it
    too; its `finally` region is gone by then, since `LowerSemanticCF`
    copies it before each exit.
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
  `processComptimeIfOp` elaborates only the arm taken and splices it where
  the op stood. `processComptimeForOp` makes one outer `hlcf.loop`
  labelled `param_for_outer` holding one unlabelled `hlcf.loop` per
  iterator, N+1 of them, since the final, exhausted iterator runs the
  body's stop arm. Each holds a clone of the body (`cloneInto` through an
  `IRMapping`, so only what the region defines is renumbered), whose
  arguments take the previous loop's results (`replaceArgs`); a
  `comptime.for.continue` breaks its iteration loop and a
  `comptime.for.break` the outer one. `simplifyParamForLoop` then folds
  the loops a clone leaves trivial.
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
  through a `ControlFlowTree` analysis: it splits the block at each root
  node, inlines each region's block, makes the op's results arguments of
  the block after it, and lowers each region terminator to a branch whose
  operands are its target's block arguments (`hlcf.unreachable` to
  `llvm.unreachable`). No `cf` dialect is used anywhere.

## What Mojito does

- **HIR** (`crates/mojito-hir/src/hir.rs`) reads structured checked
  statements (`CheckedStmtOp`) and, since stage 3, lowers them into a
  region tree: `if`/`while`/`for`/`comptime if`/`comptime for` are
  `HirInstr::If`/`Loop`/`ComptimeIf`/`ComptimeFor` region ops and
  `break`/`continue` labelled region terminators, and, since stage 5, a
  `try` is `HirInstr::Try` whose parts are regions of the same walk. No
  terminator jumps between blocks, and dead code after a sealed block is
  erased, so every region is one block.
- **MIR** (`crates/mojito-mir/src/mir/ir.rs`) has no CFG since stage 14c:
  `MirTerm` is `Return`, `ReturnWithCleanup`, `TryYield`, and the `hlcf`
  region terminators, and `mir::verify` requires every region, the body
  included, to be one block.
  - The region ops are `MirInstr::Try` and, since stage 1, `If`, `Loop`,
    `ComptimeIf`, and `ComptimeFor`. Each
    region is a `Vec<MirBlock>` of one block sharing the function's
    registers and slots, walked by
    `instruction_regions`/`instruction_regions_mut`. A structured `try`
    part completes by `TryYield` (`lit.try.yield`) and leaves to a loop by
    a labelled `Break`/`Continue`.
  - `lower_expr/ctrl.rs` builds `short_circuit`, `ternary`, `compare_chain`,
    and `comprehension_clauses` as `If`/`Loop` region ops (stage 2), and MIR
    lowering builds HIR's region ops as the same ops (stage 3,
    `Flatten::hir_region`). `mir::verify` checks each body's shape (stage
    5, in every mode since 14c); register typing,
    pre-drop verification, the ownership checks, drop elaboration, and
    every pass after it, the pre-elaboration pipeline included, read the
    structured body (stages 6–10), and so do the elaborator (stage 11)
    and the post-elaboration pipeline (stage 12). The VM steps region ops
    with each frame's region cursor (stage 13), and Pliron lowers region
    ops to LLVM blocks itself, as `LowerControlFlow` does (stage 14a).
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
  `inlined_cf_scope` loop (stage 10).
- **The post-elaboration passes read regions** (stage 12), with
  `simplify-cf` (`analysis/simplify_cf.rs`, upstream's `SimplifyCF`) after
  the first `mem-2-reg`, where upstream's `AutomaticInline` function
  pipeline starts. No pass rewrites a CFG, and since 14c none can meet
  one.
- **The elaborator works on regions** (stage 11). `mono/select.rs`
  decides a `comptime if` and splices its taken arm, and `mono/unroll.rs`
  makes `processComptimeForOp`'s loops (`LoopLabel::PARAM_FOR_OUTER`
  outside, one unlabelled loop per iterator) and folds them by
  `simplifyParamForLoop`'s two cases.
- **Versions.** MIR text `1.54`, `PACKAGE_FORMAT` `"mojito-package 55"`
  since stage 1. A package holds checked units, never MIR, so moving the
  lowering point changes neither.

## Target

- **One block per region.** A function body is one block. Every nested
  region (`if` arm, `elif` condition, loop body, `try` part, compile-time
  arm or body) is one `MirBlock`.
- **Region ops**, new `MirInstr` variants, one per upstream op:
  - `If { results, cond, then, elifs, else_args, orelse }` — `hlcf.if`.
    Each `elifs` entry is a condition region ending in `ElifCond { cond,
    values }` and an arm, each with its arguments (`cond_args`,
    `then_args`), which the values of the `ElifCond` entering it define,
    as `else_args` are the last one's.
  - `Loop { label, operands, args, results, body }` — `hlcf.loop`, its
    body's arguments defined by its operands and each `Continue`'s values,
    its results by each `Break`'s. `mem-2-reg` produces them for a slot
    stored in the body (R927).
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
  `ElifCond { cond, values }`, `Break { label, values }`, `Continue { label, values }`,
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
- **The verifier enforces it.** `mir::verify` requires every region to be
  one block in every mode, as `SizedRegion<1>` is an op verifier's
  constraint upstream; MIR text cannot spell a CFG terminator.
- **`hlcf` canonicalizations replace R836's CFG rewrites.**
  `RemoveStaticCondition`, `HoistYieldResults`, `RemoveUnusedResults`,
  `HoistUnconditionalReturn`, and `HoistConditionalReturn` on `If`;
  `RemoveDeadLoop` on `Loop`; `checkTrivialBlocks` on `ComptimeIf`. The
  threading, identical-successor, and single-predecessor patterns are
  deleted, since the blocks they remove no longer exist. `SimplifyCF`'s
  trivial-loop removal joins the post-elaboration pipeline, where upstream
  runs it.
- **The elaborator works on regions.** A `ComptimeIf` is replaced by its
  taken arm. A `ComptimeFor` unrolls into an outer `Loop` labelled
  `param_for_outer` holding one unlabelled `Loop` per iterator, N+1 of
  them, each deciding its leading `has_next` test before its arms, with
  `ComptimeContinue` breaking its iteration loop and `ComptimeBreak` the
  outer one, and the trivial loops are folded away. The dominator
  recovery (`outermost_loop`, `mir::ComptimeForBodies`) is deleted.

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
   taken arm alone (`mono/unroll.rs`). Since stage 11 only a phase test's
   or `mojito opt`'s lowering makes the CFG `ComptimeFor` terminator,
   which stage 14 deletes.
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
11. **Elaborator.** *Landed (R880).* Templates are fetched structured
    (`Bodies::get`; `Templates` and `PackageBodies::template` are gone),
    and every elaborator walk enters every region op.
    - `select_comptime_ifs` (`mono/select.rs`) decides each `comptime
      if` in order and splices its taken arm, the op's results read as
      the arm's `comptime.yield` values; an arm that leaves ends the
      block, so nothing after it is elaborated.
    - `unroll_op` (`mono/unroll.rs`) is `processComptimeForOp`: each
      clone renumbers only the registers it defines (and the operands a
      call keeps for a removed argument, which nothing defines), takes
      the previous iteration loop's results as its arguments, and
      decides its `has_next` test before anything else.
      `simplify_param_for_loop` folds a loop its one break ends and a
      breakless iteration loop, counting breaks by `loop_breaks_mut`.
    - The lowering point was the end of elaboration
      (`lower_control_flow_program` before `ConcreteMir::verified`), and
      `verified_fragment` lowered each clone it collected, until stage 12
      moved it into the backends. The concrete
      check splits: `concrete_function_findings` accepts runtime region
      ops in an instance, and a backend's program refuses them.
    - `mir::ComptimeForBodies` and `DominatorRoots` are gone, and so is
      `BreakSite::in_comptime_for`: `lower-control-flow` folds a result
      a break in a `comptime for` body carries, and `mem-2-reg` promotes
      nothing in a list holding a lowered `comptime for`.
    - `lower_whole_drops` memoizes each region op's flow within one
      list's walk, since a structured instance's loops nest as deep as
      its inlined bodies.
12. **Post-elaboration pipeline.** *Landed (R881).* The elaborator emits
    structured instances, and the post-elaboration pipeline reads them.
    - The lowering point was each backend's entry: the VM's
      `ConcreteProgram::new` (each CTFE fragment) and
      `VmBackend::run_concrete`, until stage 13, and Pliron's `compile`,
      which lowers a clone of each reachable function. Every driver, artifact, and test
      path ends in one of them. Concrete verification no longer refuses
      runtime region ops.
    - `simplify-cf` ports `SimplifyCF` whole: trivial-loop removal (which
      removes every `inlined_cf_scope` loop an inlined body's one final
      `break` ends), trivial-`try` removal, and implied-condition
      propagation. It runs after the first `mem-2-reg`, before `sroa`, as
      upstream's `AutomaticInline` function pipeline begins; `cse` regains
      the merges region scoping hid across sibling loops.
    - `lower-arg-conventions` decides its stored arguments by the
      region promotion's `variance`.
    - R836's CFG rewrites in `canonicalize`, its constant-branch fold, and
      the CFG paths of `sccp` and `mem-2-reg` are deleted with their pass
      cases: Mojo has no `cf` dialect. Since 14c `mojito opt` refuses a
      lowered body at load, when it verifies.
    - `MirFunction::instruction_count`/`block_count`, and so every
      elaboration counter, count every region.
13. **VM.** *Landed (R882).* It executes region ops, as upstream's
    interpreter does, and the VM path and CTFE stop lowering.
    - Each frame holds a `RegionCursor` (`backend/vm/regions.rs`): its
      position as a path through the region ops it is inside, which each
      op's entry and each region terminator moves, as
      `transferControlFlowTo` moves the interpreter's `pc`. A call inside
      any region stays a pushed frame, so neither nesting nor recursion
      grows the host stack. Exits resolve through `mir::region_exit` and
      `RegionFrame::names`, the resolvers `lower-control-flow` uses.
    - A region exit naming an op around a `try` leaves the part as
      `Flow::Leave`, running each `finally` on the way, until the cursor of
      a list inside that op resolves it; a `break` across two `try`s now
      runs (R905's VM half).
    - Reaching `hlcf.unreachable` is a runtime error, as upstream gives it
      no `interpret`, where the lowering made it a return.
    - The `try` itself still runs its parts on the host stack
      (`exec_try`/`run_region`), filed as R1272. The CFG terminator arms
      went with stage 14c.
14. **Pliron.** Split in three:
    - **14a, landed (R883).** `lower/regions.rs` and `try_flow.rs` lower
      region ops to LLVM blocks, as `LowerControlFlow` does: an op's
      results and a region's arguments are block arguments (aggregates
      through staging storage), and a region exit leaving a `try` part
      crosses each `finally` on the way, so R905's shape runs natively.
      Owned temporaries release on the region tree.
    - **14b, landed (R1273).** The transitional `lower-control-flow` pass
      went, with its `opt` registration and pass cases: the phase entry
      points return the structured body (one `lower_checked_program`, one
      `lower_cfg`), and the slot-ends check and `cut_try_cleanups` walk
      regions as `CheckLifetimes` does, an `if` or `match` joining its
      arms, a loop iterated to a fixpoint with its break set as live-out,
      and region exits resolved through `RegionFrame` and `region_exit`
      across each `finally` they cross.
    - **14c, landed (R1274).** `Jump`, `Branch`, `ComptimeBranch`, the
      `ComptimeFor` terminator, and `EscapeJump` left `MirTerm`, with
      their MIR text spellings (1.88), the VM's arms and `Flow::Jump`, and
      Pliron's refusal. `mir::verify` itself requires one block per
      region, so the separate shape checks, the `is_structured_shape`
      guards, and `opt`'s `STRUCTURED_ONLY` went too, and every walker
      reads one block per list: the block-list fixpoints of the ownership
      walks, drop lowering, and the slot-ends check, their escape
      channels, `cse`'s dominator tree, and `canonicalize`'s
      unreachable-block erasure are gone. A region is still a
      `Vec<MirBlock>` the verifier holds to one block, where upstream's is
      `SizedRegion<1>`: filed on the roadmap's `pmir` track.

Each stage moves Mojito strictly closer to upstream; no stage adopts a
Mojito-only shape.

## Gaps kept open

- **`finally` placement.** Closed by R884 (2026-10-22): MIR lowering copies
  each `finally` before every terminator leaving its `try`
  (`mir/try_finally.rs`, upstream's `lowerTryFinally`), each raise naming
  its `try` by label as `lit.try.raise` does, so the VM no longer runs a
  `finally` dynamically and the `try` holds none.
- **HIR as a layer.** Upstream has no HIR between its parser's ops and
  `CheckLifetimes`, only `LowerSemanticCF`. Whether HIR's remaining job
  (name-to-slot resolution) folds into MIR lowering is a later question,
  not filed until an entry can name its lever.

## Unverified

- The one-session sizes of stages 6–11 are estimates (HIR is about
  1,400 lines; the ownership and drop analyses about 9,500). Each entry may
  be split by its planning session.
- Whether the pin accepts `comptime for … else` was not run here.
  Upstream's parser does parse it (`ParserStmts.cpp`), and R797 records
  the pin running it.
