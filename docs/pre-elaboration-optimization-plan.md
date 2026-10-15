# Pre-Elaboration Optimization: Staged Plan

**Status:** plan recorded 2026-10-13. Stages 1 (R777), 2 (R778), 3
(R779), 4 (R780), 5 (R781), and 6 (R782, its intraprocedural core) have
landed, as has stage 11's `Mem2Reg` in its no-variant case (R787), and so
has the post-elaboration pipeline (R840). The scheduled
tasks are [`docs/roadmap.md`](roadmap.md) tracks `measure` (counting and
testing) and `pmir` (the passes); this document is their design record and
is updated as stages land.

**Goal.** Shrink each generic body once, before the elaborator copies it,
as upstream does, so that the cost of generic instantiation grows with what
a program uses rather than with the slack in every generic body it reaches.

## What Mojo does

Upstream's compiler walkthrough
(`Mojo/docs/compiler/MojoCompilerWalkthrough.md` in the `modular` repository,
"Phase 3: Pre-Elaboration Optimization") runs a pass pipeline on the
parametric KGEN IR after the semantic checks, the lifetime checks, and
destructor insertion, and before the elaborator instantiates anything. Its
stated reason is "fewer, smaller, simpler generators are faster to
instantiate (e.g. better for elaborator caching)". The passes it names
are below, as the walkthrough lists them. `Mojo/docs/compiler/manual/PassesAndIR.md`
lists each once, in the order `RemoveUnusedParams`, `EliminateDeadSymbols`,
`SROA`, `Mem2Reg`, `Canonicalizer`, `InlineParametric`, `SCCP`, and so on,
but the pinned compiler runs them in repeated rounds. `mojo build
--mlir-timing --mlir-timing-display=tree` on a small generic program shows:

```text
RemoveUnusedParams, EliminateDeadSymbols,
'kgen.generator' Pipeline: SROA, Mem2Reg, Canonicalizer
InlineParametric, RemoveUnusedParams, EliminateDeadSymbols,
'kgen.generator' Pipeline: SROA, Mem2Reg, SCCP, Canonicalizer
RemoveUnusedParams, EliminateDeadSymbols, InlineParametric,
'kgen.generator' Pipeline: SROA, Mem2Reg, Canonicalizer
RemoveUnusedParams, EliminateDeadSymbols,
'kgen.generator' Pipeline: CSEPass
ElaborateGenerators
'kgen.func' Pipelines: SROA, Mem2Reg, SCCP, Canonicalizer, CSEPass
```

So `SCCP` is always followed by a `Canonicalizer`: MLIR's `SCCP` replaces
values it proves constant but never rewrites a terminator, and the
`Canonicalizer` keeps the arm of an `hlcf.if` whose condition is now static
(`RemoveStaticCondition`). Upstream uses no `cf` dialect: its generators
hold run-time control flow as structured `hlcf` region ops until
`LowerControlFlow`, after elaboration.
Upstream also folds again on each concrete function after elaboration;
Mojito's post-elaboration pipeline runs `sccp` and `canonicalize` there
(R840, `docs/architecture.md` §Post-Elaboration Pipeline).

| Pass | Purpose, as the walkthrough states it |
|---|---|
| `SROA` | Scalar replacement of aggregates, on generators |
| `Mem2Reg` | Promote memory to registers, on generators |
| `Canonicalizer` | Apply rewrite patterns |
| `InlineParametric` | Inline `nodebug` functions and small functions before elaboration |
| `SCCP` | Sparse conditional constant propagation |
| `ApplyInliner` | Handle `apply` operator inlining |
| `EliminateDeadSymbols` | Remove unreferenced generators |
| `RemoveUnusedParams` | Clean up unused parameters |

Inlining at this stage is deliberately narrow. The walkthrough restricts it
to `always_inline_no_debug` functions and "certain small functions",
because inlining too much before elaboration "increases pressure on the
elaborator and reduces cache granularity".

## What Mojito does

Nothing. No pass transforms MIR between drop elaboration and the
elaborator, and none transforms it after. `native::mono` copies each generic
body exactly as lowering and drop elaboration left it, once per distinct
instance key (`InstanceKey`: template, owner, and every argument), for both
backends. `docs/parametric-mir-plan.md` named pre-elaboration optimization
among what it did not attempt; this plan is that attempt.

The arrangement upstream's order implies is already in place:

- every body is checked once, with its parameters symbolic;
- ownership is checked and drops are inserted on the parametric MIR, before
  the elaborator (`CompiledProgram::elaborated_mir`, `src/compiler.rs`);
- an imported package body is lowered when the elaborator first reaches it,
  and its drops are inserted then, through the package's finishing hook
  (`PackageBodies::finishing`, set in `src/package.rs`), once per body and
  not once per instance;
- the elaborator instantiates only what a worklist from the entry roots
  reaches.

So the pipeline has two seams and they are both per generator: the
program's own bodies after `elaborate_drops_program`, and each package body
in the finishing hook after `elaborate_drops_function`.

## Measure first

A pass is worth landing only if it shrinks what the elaborator produces, or
the work it does, on real programs, and only if it changes no program's
behavior. The `measure` track makes both visible:

1. **Expansion counters** *(done, R771)*. Under `--timings`, `elaborate`
   reports `generators`, `generator_instances`, `generator_instructions`
   (the reached generic bodies), `generator_instance_instructions`, and
   `concrete_instructions` (the whole concrete output), and the same three
   sizes in blocks (`generator_blocks`, `generator_instance_blocks`,
   `concrete_blocks`, R836); `docs/performance.md` §3 defines them. The ratio of `generator_instance_instructions` to
   `generator_instructions` is the expansion the passes attack.
2. **A corpus report and a committed baseline** *(done, R772)*.
   `scripts/instantiation-report` runs the counters over `assets/ok` and
   `benchmarks/compile` and writes one row per fixture, plus a `total` row,
   to the committed `benchmarks/instantiation.tsv`, so a pass's effect is a
   diff of that file; `docs/performance.md` §3 records the first numbers.
3. **A ratchet** *(done, R773)*. `scripts/instantiation-report --check`,
   the overnight gate's last step, fails when the concrete instruction total
   over the fixtures elaborated in both the baseline and the fresh run
   differs from the committed one, or when a fixture stops elaborating, so a
   later change cannot give back what a pass won without committing it.
4. **Benchmarks that instantiate a lot** *(done, R774)*.
   `benchmarks/compile/collections_many.mojo` reaches many instances of the
   bundled collections, `nested_generics.mojo` nests generic types deeply,
   and `trait_defaults.mojo` calls trait defaults through many conformers;
   `docs/performance.md` §3 records their counters.
5. **A switch and a differential.** The pre-elaboration pipeline can be
   turned off (*done, R777*: `MOJITO_PRE_ELABORATION=0`, or
   `Compiler::with_pre_elaboration(false)`), and a corpus test checks that every
   `assets/ok`, `assets/extensions/ok`, and `assets/runtime_error` fixture
   prints and traps the same with it on and off (*done, R775*: the
   `pre_elaboration::*` group). The VM stays the oracle: a pass is correct
   only if the VM cannot tell it ran.
6. **Pass tests.** *Done, R776.* A pass is tested as MIR text in, MIR text
   out, through the existing textual round trip (`mir::text`), beside a
   focused fixture that runs: a pass adds its cases under
   `tests/mir_passes/<pass-name>/` (`tests/mir_passes/README.md`), and
   `mojito opt --pass NAME` runs it by hand.

## The passes

Each pass runs on parametric MIR, must keep it verifying
(`mir::verify` runs after each pass under debug builds), and must keep
every instruction that does not declare itself free of effects, as MLIR
does (`MirInstr::would_be_trivially_dead`): no drop, consume, or ownership
mark defines a result, and each counts as an effect. The passes live in `mojito-analysis`, beside drop
elaboration, which already rewrites MIR; the root crate wires them into both
seams, so no new crate edge is needed.

In order:

1. **The seam** *(done, R777)*. An empty pipeline (`analysis::PIPELINE`)
   at both seams, with the switch, so the later passes are additions to a
   list; `docs/architecture.md` §Pre-Elaboration Pipeline describes it.
2. **Dead generators** (`EliminateDeadSymbols`) *(done, R778)*. Drop the
   program's own generic bodies that no root reaches before the other
   passes run. The elaborator already ignores them, so this saves pass
   time, not output size. It never applies to a package build, which never
   reaches the program seam. Dead concrete functions and struct
   declarations go too, rooted at the entry roots and the compilation's
   exports (R834).
3. **Cleanup** (`Canonicalizer`, first part) *(done, R779)*. Remove
   unreachable blocks and instructions whose results nothing reads and that
   have no effect, in the `canonicalize` pass, which the later
   `Canonicalizer` stages extend with patterns rather than replace. Lowering
   leaves few of either: generator-instance instructions fall by about 1 %
   (`docs/performance.md`).
4. **Copy propagation** (`Canonicalizer`, second part) *(done, R780)*.
   Forward an identity copy's operand to its readers, so lowering's
   temporaries do not survive into every instance: a `value.copy` of a
   scalar, pointer, `None`, or SIMD lane folds (`MirInstr::forwarded_operand`).
   A copy of a register-passable trivial aggregate (vector SIMD,
   `StringLiteral`, a `TrivialRegisterPassable` struct) folds when it is its
   operand's sole reader (`MirDeclarations::copy_is_identity`), as
   upstream's loads of such values are SSA values; any other struct copy,
   a tuple's included, or a type-parameter copy may run `__copyinit__` and
   stays; a type-parameter copy that an instance makes trivial is forwarded by the
   post-elaboration `canonicalize` (R840; R839 records the question).
   *Pass-through blocks (R836).* `canonicalize` also threads empty blocks
   that only jump on, folds a branch whose arms meet, and merges a block
   into its single jump predecessor: corpus concrete blocks fall by 10.4 %,
   generator-instance blocks by 13 %. These are MLIR's CFG patterns
   standing in for upstream's structure, where a join or latch is no
   block; the structural gap is its own `pmir` entry.
5. **Constant propagation** (`SCCP`) *(done, R781)*. The `sccp` pass folds
   what is constant without the parameters: built-in operators,
   literal materializations to `Int` and `Float64`, and identity copies over
   scalar constants (`MirInstr::folded_constant`), and nothing where the
   compile-time folder and the VM could disagree. A second `canonicalize`
   after it folds the branches it decides, as upstream's second round
   runs `SCCP, Canonicalizer`. A parameter read is left to the elaborator,
   which folds the read per instance, and the post-elaboration pipeline then
folds the operators over it (R840);
   a materialization to `UInt` or a one-lane SIMD type folds to a typed
   `Const::Value` (R841), and so does a literal splatted to a SIMD type
   of any width (R847). Generator-instance
   instructions fall by 6.5 % (`docs/performance.md`), almost all of it
   literal materializations.
6. **Unused parameters** (`RemoveUnusedParams`) *(done, R782)*. The
   `remove-unused-params` pass records on each generic body
   (`MirFunction::removed_params`) its own compile-time parameters that
   neither its body nor its signature reads, and the elaborator leaves
   their arguments out of the instance key and the instance symbol
   (`f$mono` for `f[n]` whose every argument went). The read analysis
   (`mir::unread_parameters`) is a structural walk of the body's and the
   signature's serialized form, as MLIR walks every attribute and type,
   plus a value parameter's slot reads; a parameter read only by another
   parameter's default is unread, as on the pin, and a drop of a scalar
   slot is no read. It runs in each of `PIPELINE`'s rounds, as upstream's
   does, and each run records the unread set afresh: a parameter only
   grows removable as the cleanup passes erase reads, so the last round's
   run decides (R869). Upstream also
   rewrites the generator's signature and its callers' arguments; Mojito
   keeps both and binds the argument the body never reads. The pass walks
   the call graph callees first, as upstream's does, so a parameter only
   forwarded to a removed callee parameter is removed too (R907); a
   forward around a cycle stays a read, as on the pin. A nested body
   removes the enclosing parameters it never reads, and its enclosing
   function reads through it only the rest (R908). A static method
   removes the struct parameters it never reads, and a static call's
   receiver forwards them (R909). The pin goes further than this core: it
   removes a nested generic `def`'s own unread parameters, a `comptime
   for` nested body's, a struct parameter a method with a receiver
   ignores or that only spells a by-address argument's type, and a few
   forwards Mojito still counts; each is a `pmir` entry.
   Corpus generator instances fall by 16 of 30 543
   (`docs/performance.md`): the bundled library has almost no unread
   parameter, so the stage's effect is the program's own and the
   interprocedural residues'.
7. **`@always_inline` reaches MIR.** A function's MIR declaration records
   the inline level its decorator names, as upstream's function op does:
   `always`, `nodebug`, `builtin` (`@always_inline("builtin")`), or `never`
   (`@no_inline`), with `automatic` for none (`InlineLevel`, MIR text 1.61).
   The checker rejects the operands and conflicting pairs the pin rejects.
   `@inline(level)`, which the pinned std spells far more often, is not yet
   read, so the bundled library's markings wait on it.
8. **Restricted inlining** (`InlineParametric`, landed for closed
   callees). `inline-parametric` opens the second round, after the first
   `canonicalize`, runs again in the third, before its `canonicalize`, as
   upstream's does (R869), and runs at both seams: over the program's own bodies
   callees first (the call graph's strongly connected components
   bottom-up), and over each package body as it is finished. It replaces a
   call to an `@always_inline("nodebug")` or `@always_inline("builtin")`
   function with the callee's finished body, so the callee's drops come
   with it, and every inlined register takes the call site's location. A
   callee on a cycle of such calls stays a call. The pin's restriction,
   observed through elaboration diagnostics (a `comptime assert` in the
   callee fails inside the caller's instantiation once it is inlined):
   - `nodebug` and `builtin` callees inline at any size (640 statements
     still inline), with loops, branches, early returns, `raises`, `mut`
     and owned parameters, `ref` results, keyword and default arguments,
     variadics, value parameters, struct methods and constructors, nested
     `def`s, transitive chains, and calls inside the caller's `try`; a
     `nodebug` destructor is inlined at the value's death.
   - Plain `@always_inline` callees inline only below a size bound (16
     `s += i` statements inline, 24 do not, nor does a `for range` loop).
   - Nothing else inlines: a plain `def`, `@no_inline`, a trait-dispatched
     call, a callee holding a `try`, and a few `String`-literal shapes stay
     calls; a recursive `@always_inline` function is an error.

   Mojito inlines the callees that name none of their own compile-time
   parameters (a concrete function or a method of a non-parametric struct),
   called positionally into `read` or owned parameters, that do not raise,
   return a value rather than a reference, and hold no `try`. An owned
   parameter, or a `read` one of a scalar type, is a fresh slot the argument
   initializes; any other `read` parameter is the caller's place the
   argument was loaded from. The remaining shapes are `pmir` entries R915
   through R923; the pin's refusals Mojito lacks are R924 (shapes it keeps
   as calls) and R925 (a recursive `@always_inline` function, an error on
   the pin). The
   corpus holds no `nodebug` or `builtin` callee outside
   `assets/ok/always_inline_nodebug_calls.mojo`, so the stage's corpus
   effect waits on the bundled library's markings (stage 10), as
   `EliminateDeadSymbols`' waits on the passes after it.
9. **Trait default wrappers** (landed, R785). A trait default is its
   trait's member `Trait.method`, one function over `_Self`, reached
   through a forwarding wrapper per conformer. Upstream's
   `synthesizeDefaultTraitMethodWrapper` marks every wrapper
   `@always_inline("nodebug")`, and Mojito's wrapper declaration carries the
   same level (`CheckedDefaultWrapper::INLINE`), so `inline-parametric`
   replaces a statically resolved call with the wrapper's forwarding call
   and the wrapper instance drops out of the concrete program.
   - A call reached by trait dispatch keeps the wrapper instance, on the
     pin too: the callee is unknown before elaboration, and upstream's
     post-elaboration pipeline has no inliner.
   - The pin also inlines a parametric conformer's wrapper (`B[k].m`) and
     the `mut self`, raising, generic, and keyword-call shapes, which
     Mojito inlines too (R916–R920).
10. **The bundled library marks what upstream marks.** Upstream's std puts
    `@always_inline` on most small methods. Mojito's bundled std has none, so
    the inliner has nothing to act on until the library carries upstream's
    markings.
11. **Memory traffic** (`Mem2Reg`, `SROA`). Promote a generic body's
    variable slots to registers, and split aggregates.
    - `Mem2Reg` landed as `mem-2-reg` (R787) for its no-variant case, at
      both seams: in the first three rounds over generators' concrete-sized
      locals, and after elaboration behind `lower-arg-conventions`, which
      stores each register-passable argument into a local first, as
      upstream's `LowerArgConventions` does. Once drops are elaborated, a
      slot of a type with no destructor and an identity copy carries no
      ownership effect, so no ownership state is rewritten.
    - A slot whose reads merge two stores waits on structured bodies and
      region values (R927).
    - `SROA` waits on a value-level aggregate construction (R928).

`ApplyInliner` has no counterpart: Mojito has no `apply` operator.

## What would stop a stage

- A pass that changes any `assets/ok` output under the differential. The
  pass is wrong, whatever the counters say.
- A pass whose corpus effect is within noise. It is not landed, and the
  reason goes to `docs/non-goals.md`. A pass that only removes work for the
  passes after it, as `EliminateDeadSymbols` does, is judged with them.
- The native debug-correlation premise (`tests/heavy/`) failing because an
  instruction lost its span. A pass keeps each surviving instruction's span
  and gives an inlined instruction its call site's.

## Documents touched as stages land

- `docs/architecture.md`: the pipeline gains its pre-elaboration stage at
  the seam's landing.
- `docs/performance.md`: each pass records its corpus numbers.
- `docs/mir-text-format.md`: the `@always_inline` attribute.
- `docs/features.md`: only where a program's behavior is visible, which for
  an optimization pass should be nowhere.
