# Parametric MIR and One Elaborator: Staged Plan

**Status:** plan recorded 2026-09-30. P0's census has landed (§P0); no stage
after it has. The scheduled
tasks are [`docs/roadmap.md`](roadmap.md) §1; this document is their design
record and is updated as stages land. It was revised on 2026-09-30 after
[`docs/parametric-mir-plan-review.md`](parametric-mir-plan-review.md).

**Goal.** Mojito's flow should strongly resemble the flow in upstream's
`Mojo/docs/compiler/MojoCompilerWalkthrough.md`, with MIR standing where
upstream has MLIR dialects. Pliron is not part of this plan: the A1 shadow
core was rejected on overhead ([`docs/non-goals.md`](non-goals.md)), and
nothing below depends on reopening it.

## What Mojo does, and what Mojito does

Upstream's phases, from the walkthrough:

1. Parse and type-check each declaration once, with its parameters symbolic,
   and emit parametric IR.
2. Check lifetimes and insert destructors on that parametric IR.
3. Optimize the parametric IR (optional for resemblance).
4. Elaborate: one worklist instantiates every function and struct generator
   reachable from the entry, and an IR interpreter evaluates compile-time
   code.
5. Lower and optimize the concrete IR.
6. Emit LLVM.

A package is written after the semantic checks and before lowering: the
post-parse IR with its bodies, which an importer elaborates without checking
the source again.

Mojito today has the same phases in a different order, and three
instantiation mechanisms where upstream has one.

| Mechanism | Where | What it serves |
|---|---|---|
| AST cloner | `mojito-comptime` (`rewrite`, `specialize`, `mono`, `nested`; about 10.7k lines), before the executable check | Bodies keyed by `comptime if`/`for`, packs, `DType` and vector values, value-keyed and variadic structs, per-instantiation and per-call method clones, nested defs |
| Erased dispatch | the VM, at run time | Trait-bound generic bodies kept in MIR with their type parameters symbolic |
| MIR monomorphizer | `mojito-native` (`native/mono`; about 5.6k lines), native builds only | The same erased bodies, substituted over verified MIR |

Consequences of that arrangement:

- The driver re-elaborates and re-checks to a fixpoint, up to five discovery
  rounds, because only a check can discover the instantiations the next
  elaboration must clone (`src/compiler.rs:compile_linked`).
- A cloned body is checked again per instance. Checked templates
  ([`docs/notes/instantiation-from-template.md`](notes/instantiation-from-template.md))
  replace that check with derived facts for the bodies their certificates
  cover, and the rest keep the clone check.
- That derivation is `checker/template_facts.rs` and its submodules: 18,823
  lines in 24 files. It grows by one recipe per body shape and cannot reach
  every shape, because its carrier is still the clone.
- The VM and the native backend instantiate generics by different means, so
  each new generic feature is implemented twice.

Three facts make the target reachable from here.

- **MIR is already parametric for one class.** A trait-bound generic body
  reaches MIR with `Ty::Param` types, passes `mir::verify` and ownership
  analysis once, and `native::mono` substitutes types and compile-time values
  into it per instance. That is a generator and an elaborator in all but
  name.
- **Specialized MIR mostly runs on the VM already.** The A1 experiment
  specialized all 893 of its corpus inputs into MIR that verifies
  ([`docs/notes/pliron-a1.md`](notes/pliron-a1.md) §Coverage). Those rows
  were compilations, not runs. The `specialized_vm` corpus group measures the
  runs: 786 of 810 `assets/ok` fixtures behave as the erased program does,
  and the rest are filed. That is encouraging, not a parity proof.
- **The VM already executes compile-time code.** CTFE runs a helper on
  `VmBackend`. It reaches the VM through a synthesized AST subprogram and a
  checker run, so it does not yet consume generators (§P3).

## Target pipeline

```text
source -> lex -> parse -> link
       -> check            every body once, parameters symbolic
       -> HIR CFG -> MIR   generators: symbolic types, compile-time regions
       -> mir::verify (parametric) -> ownership/liveness -> drop elaboration
       -> elaborate        one worklist from the entries; substitution,
                           compile-time control flow, CTFE on the VM
       -> mir::verify (concrete)
       -> VM | native backend
```

What stays:

- The AST and `CheckedProgram`. Upstream's parser emits IR directly; Mojito
  keeps its AST, and that divergence is accepted here.
- MIR as the waist. It gains a pre-elaboration form. Backends still consume
  only verified concrete MIR, so `AGENTS.md` invariant 5 holds as written.
- The VM as the executable oracle, the runtime ABI, and the Pliron-to-LLVM
  backend.

What goes, by the last stage:

- The AST cloner's core and the discovery fixpoint.
- Template fact derivation: realization, installation, and the certificate
  grammar in `template_facts.rs`.
- Erased dispatch in the VM, and the `mir::verify` tolerances that exist only
  for it.

What this plan does not attempt: a dialect stack, pre-elaboration
optimization, the lazy three-phase parser, parallel elaboration, and
parametric debug info.

## The generator contract

A checked template's MIR is valid for every instance when no fact in it
depends on the instance. Where a decision does depend on the instance, it is
an explicit MIR operation the elaborator resolves, never a second check.

That principle needs an inventory and a preservation argument before any
clone check is removed. Without them the certificate machinery would
reappear inside MIR under other names. The contract is its own note, written
before P2 (roadmap §1, the generator-contract entry).

**The guarantee.** Substituting arguments that satisfy a generator's recorded
obligations preserves typing, ownership, and effects. No source expression is
inferred again and no overload is ranked again.

**What a generator names.**

- Its binders, and the assumptions its bounds supply.
- The obligations an instance still owes.
- The calls and witnesses its check selected.
- Its lifecycle requirements: what copying, moving, and destroying a value of
  a parameter type demand.
- Its origin and transfer summaries.
- Source locations and instantiation provenance, which survive substitution.

It reuses `ParamRef`, `ParamExpr`, `ParamConstraint`, and the callable
identity in `mojito-symbol`. It adds no parallel vocabulary.

**The inventory.** The instantiation note's §What an instance still owes is
the migration inventory, not the specification. Each obligation there is
classified as one of:

- a proof made when the declaration is checked;
- an assumption a bound supplies;
- an operation or summary kept in MIR;
- a predicate checked when the generator is instantiated;
- clone bookkeeping that disappears (retargeting, local renumbering).

The list is wider than `where` clauses and `rebind` equalities. It holds
implicit copies, deletability, reference reads, loans in stored values,
closure effects, and replayed transfers, and some of those vary with the
substituted type. P2 includes lifecycle members, so none of this waits for
P3.

**Availability.** `native::mono` keeps a `speculative` set: instances its
eager constructor walk enqueued, dropped silently when they fail to
materialize, because a conditional member's `where` clause is absent from MIR
and the checker is trusted to have admitted no call. A general elaborator
needs the availability clause on the generator, and demanded reachability
told apart from speculative reachability, before the checks it trusts are
removed.

**"Check once"** means one semantic checking process per declaration, not one
traversal. Effect summaries and loop dataflow still iterate to fixed points
inside it.

A body the symbolic check cannot type is a defect in the symbolic check, filed
against the pin's behavior. It is not a reason to check a clone.

## Phases and their names

"Elaborated" already has a meaning in the code:
`CompiledProgram::elaborated_mir` is *drop-elaborated* MIR, which may still
be generic. This plan uses three names, and P1 makes the code use them.

| Name | What it is | Who may consume it |
|---|---|---|
| Parametric MIR | Checked generators that passed parametric verification | Ownership analysis |
| Drop-elaborated MIR | Parametric MIR with ownership verified and drops inserted | The elaborator; the serialized artifact |
| Concrete MIR | The elaborator's entry-rooted output, verified concrete | The VM and the native backend |

- Each is a distinct phase wrapper or an equally explicit API contract. They
  may share a representation. A caller never has to remember which verifier
  mode ran.
- **Parametric verification** accepts well-scoped, well-kinded binders and
  explicit residual obligations. It still rejects an unbound parameter, an
  unresolved inference hole, a malformed call contract, and a missing fact.
  It is not a "symbolic types allowed" switch.
- **Concrete verification** inspects more than register types: declaration
  fields, signatures, instruction payloads, constants, nested regions,
  parameter applications, witnesses, and obligations. It says which metadata
  may survive with no executable meaning. Erased generic dispatch is gone
  from concrete MIR. Indirect calls and runtime closures are not.
- **Entry roots** are defined once for `main`, top-level execution, artifact
  execution, CTFE, and an externally requested native entry.
- The native backend receives concrete MIR and does not specialize it again.

## Stages

Each stage keeps the path it replaces alive behind a switch until the
nightly gate agrees on both, the way `MOJITO_VERIFY_TEMPLATE_FACTS` compares
derived facts with inferred ones today. That comparison path is temporary: a
stage is not done until the code it replaces is deleted. The erased VM is the
one exception, kept as an oracle until P5.

A stage that changes the production pipeline or a public contract updates
`docs/architecture.md` and the contract documents in the same change.

### P0 — Census and freeze

- Count, per compilation, which mechanism serves each generic body, by class.
  The classes are the rows of the table above.
- Count how many instance bodies are inferred, how many derived, and how many
  the erased body serves with no clone.
- Freeze `template_facts.rs`: no new certificate class and no new recipe. A
  body the certificates do not cover waits for its stage.
- The counts order the classes inside P2 and P3. This document's order is the
  default until they exist.

#### The census (2026-09-30)

`mojito run --instantiation-census FILE` prints the counts, and `--timings`
prints the cloned and checked rows as `instantiation.*` counters beside the
template statistics. `CompiledProgram::instantiation_census` is the same
record. Taken at `86a25b67` plus the census itself, on the three
`benchmarks/compile` programs:

| Bodies | Stage | `hello` | `generic` | `stdlib_heavy` |
|---|---|---:|---:|---:|
| **Minted by the AST cloner** | | **244** | **329** | **666** |
| `def`, type arguments only | P2 | 12 | 17 | 17 |
| `def` holding a `comptime if` | P3a | 0 | 0 | 0 |
| `def` holding a `comptime for` | P3b | 0 | 0 | 0 |
| `def` expanding a type pack | P3b | 0 | 0 | 0 |
| `def` keyed by a `DType` or vector value | P3c | 0 | 0 | 0 |
| `def` keyed by another value | P3c | 0 | 0 | 0 |
| Member of a `DType`- or vector-keyed struct | P3c, P3d | 43 | 43 | 43 |
| Member of another value-keyed struct | P3d | 0 | 0 | 0 |
| Member of a variadic struct | P3d | 175 | 175 | 175 |
| Per-instantiation method clone, no compile-time control flow | P2 | 0 | 80 | 417 |
| Per-instantiation method clone holding a `comptime if` or `for` | P3 | 0 | 0 | 0 |
| Per-call method clone | P3e | 14 | 14 | 14 |
| Nested `def` clone | P3e | 0 | 0 | 0 |
| Minted for a compile-time evaluation's subprogram | P3e | 0 | 0 | 0 |
| **Checked**: inferred | | 21 | 26 | 48 |
| **Checked**: derived from a checked template | | 223 | 303 | 618 |
| **Parametric bodies left in MIR** | | 345 | 357 | 385 |
| … of which `main` reaches | P1 | 0 | 10 | 36 |
| Instances those serve with no clone | P1 | 0 | 10 | 98 |

How to read it:

- A cloned body has one class. Where several describe it, it takes the one
  whose stage lands last, since the cloner keeps it until then
  (`mojito_checked::census::CloneClass`).
- The cloned rows count the converged program. The checked rows count
  distinct bodies over every discovery round, and they sum to the cloned
  total in all three programs.
- The instance rows come from running `native::mono` from `main`. A
  parametric body left in MIR is a function whose types still name a
  parameter. A clone that keeps one of its own parameters is among them
  (roadmap §1, the census entry).
- The 244 bodies of Hello World are the fixed cost every program pays: the
  bundled `Tuple` specializations, the `DType`-keyed ranges, the hasher, and
  the `os` and `path` applications.

What the counts say about the order:

- **P2 is the largest stage for a program with its own instantiations.**
  Per-instantiation method clones are 417 of `stdlib_heavy`'s 666 bodies, and
  none of them holds compile-time control flow, so P2 moves all of them.
- **P3d is the largest stage for the fixed cost.** Variadic struct members
  are 175 of the 244 bodies every program mints. The `DType`- and
  vector-keyed struct members are 43 more, and they need P3c before P3d.
- **P3a and P3b move no body in these programs.** No `def` clone here holds a
  `comptime if`, a `comptime for`, or a pack. They stay first inside P3
  because P3c and P3d depend on the forms they add, not because of what they
  move. The counts are not costs: they say how many bodies a class holds, not
  what each costs to check. A program that exercises them is roadmap §1's benchmark entry.
- **Derivation already serves nine clones in ten.** 223 of 244, 303 of 329,
  and 618 of 666 bodies are derived, so a stage's saving is mostly the clone
  and its derivation, and only seldom a second inference.
- **Erased dispatch serves few bodies.** `main` reaches 36 parametric bodies
  in `stdlib_heavy` and none in Hello World, so P1 changes little of what
  these programs run.

The budget baseline for decision D4 is `total` from `--timings`, debug
profile, `run` on the VM, median of three, same commit and machine as the
census (rustc 1.96.1):

| Program | Debug `run` |
|---|---:|
| `hello.mojo` | 11.56 s (10.82–11.66) |
| `generic.mojo` | 11.54 s (11.50–11.72) |
| `stdlib_heavy.mojo` | 15.55 s (15.32–15.82) |

The release rows of [`docs/performance.md`](performance.md) predate checked
templates. That document records this baseline too, and a stage is measured
against whichever profile it reruns, interleaved with the commit before it.

### P1 — One elaborator below the waist, for both backends

- Done: the corpus binary's `specialized_vm` group specializes each
  `assets/ok` program, verifies it, and compares its VM run with the erased
  run. 786 of 810 agreed when it landed. The rest are an expected-failure
  list, filed by kind in the roadmap.
- Done: `mir::verify::verify_concrete` rejects any symbolic type or
  compile-time parameter, and `native::mono` verifies its output with it.
  The compile-time argument slots a resolved call keeps are the residue
  (roadmap 1.2).
- Give the three phases of §Phases and their names their wrappers and their
  entry roots, and cache the concrete graph so the native backend does not
  specialize twice.
- Run the VM on concrete MIR by default. The erased path stays selectable as
  the differential oracle until P5.
- Parity is more than output on accepted programs. The gate compares results,
  output, error categories, and ordered lifecycle events, with controlled
  inputs for programs that read files or stdin. It includes artifact round
  trips and the ownership and type-error folders, since execution parity
  cannot see a checker that accepts more.
- The erased VM is a migration comparator. Where it and the pin disagree, the
  pin decides.
- Exit: one instantiation mechanism below the waist. `native::mono` is the
  elaborator, and both backends consume its output.

### P2 — Stop cloning what the elaborator can already instantiate

- Gate: the generator contract is written and every existing obligation is
  classified (§The generator contract).
- An ordinary generic struct's methods are cloned per instance only so the
  checker can check them concretely. With P1, the elaborator instantiates the
  template's MIR instead.
- The same holds for an explicit application of a trait-bound generic `def`.
- Bodies holding a compile-time construct keep the cloner until P3.
- Each obligation the clone checks discharged goes where the contract
  classified it.
- Conditional members carry their availability clause, and the elaborator's
  `speculative` set goes.
- Exit: the method and function certificate classes that exist only to derive
  these clones are deleted from `template_facts.rs`.

### P3 — Compile-time parameters and control flow in MIR

Three prerequisites come first. None of them moves a body.

- **Ownership on a compile-time region.** "Ownership analyses every arm" does
  not say what the join rule is, where a last use falls, what an arm may
  assume, or what is cleaned up on a return or a raise. For a loop it leaves
  open loop-carried ownership, zero iterations, heterogeneous elements, and
  compile-time `break` and `continue`. Upstream's
  `Mojo/lib/LowerLIT/CheckLifetimes.cpp` unifies consume sets across the arms
  of a compile-time conditional and computes a stable consume set for a
  compile-time loop, so the question is how to express those facts in
  Mojito's ownership model, not whether it can be done. A narrow vertical
  experiment answers it: a move-only value, a reference-bearing value, a
  conditional use followed by a use after the join, and destruction across
  an early exit; then zero, one, and several iterations and a heterogeneous
  pack. Each probe records the pin's verdict and destructor order, negative
  cases included.
- **Legality is not lifecycle glue.** Symbolic ownership decides legality and
  last use once. Substitution may resolve a destructor witness or expand
  aggregate cleanup, and it never recomputes a last use. A development-only
  concrete ownership comparison checks that, because `mir::verify` alone does
  not establish ownership preservation.
- **CTFE requests instances from the worklist.** Today
  `comptime/ctfe.rs` builds an AST subprogram, checks a synthesized typing
  probe, turns the result type back into source syntax, and runs a second
  synthesized helper. Kept, that would put a checker run and AST
  reconstruction inside the elaborator. The target path: a typed compile-time
  application demands a concrete callable from the shared worklist, runs
  verified MIR on the VM, and returns a validated compile-time value. An
  evaluation may demand further instances without restarting the source
  pipeline. The effect restrictions, value-crossing rules, and shared fuel
  stay.
- **The worklist has keys and states.** Requests have canonical keys and are
  pending, active, completed, or failed. A recursive function reference is
  valid. A cycle that demands an unfinished constant or layout is an error.
  Expanding polymorphic recursion has its own bound, apart from VM
  instruction fuel. `native::mono`'s instance identity and budget are the
  starting point.
- **A common type vocabulary.** A heterogeneous pack already needs a type
  that depends on a symbolic index, and P2's methods already need receiver
  applications, conditional members, and lifecycle witnesses. So register
  types over parameter expressions are specified before P3b and P3c, not in
  P3c.

Then one class at a time. For each: HIR and MIR gain the form, the verifiers
and ownership analysis accept it, the elaborator resolves it, the cloner's
branch for the class is deleted, and so is its certificate class.

- **P3a. `comptime if` on a value parameter.** MIR gains a structured
  compile-time conditional, as `Try` is a structured instruction today. The
  elaborator keeps the taken arm.
- **P3b. `comptime for` over a value index**, then **heterogeneous pack
  expansion**. They share the loop form and differ in their correctness
  conditions, so they are two steps.
- **P3c. `DType`, vector, and other value-dependent types.** Layout is asked
  only of a concrete type, and a compile-time layout query names the target
  it is answered for. A concrete type does not make layout independent of
  the target.
- **P3d. Struct generators.** Value-keyed and variadic structs, `Tuple` and
  `TString` included, are declared once and instantiated by the elaborator.
- **P3e.** Three steps: a method's own compile-time parameters, nested
  definitions and their captures, and clones minted during CTFE.

One mixed-feature probe is carried through every P3 step, so migrations that
pass alone also compose.

The MIR text schema is versioned at P3a for generators and compile-time
regions. Later forms may bump it again (decision D5).

### P4 — Check once, then elaborate

- With no clone left to check, the executable check runs once on the linked
  source, and source validation and the executable check become one process.
- The discovery fixpoint becomes the elaborator's worklist: a call in a
  generator names its callee and its parameter expressions, and the
  elaborator finds the instances transitively.
- CTFE's AST route is deleted. The request path designed before P3 is the
  only one.
- Module-scope `comptime` values follow the boundary decision D3 set before
  P3.

### P5 — Delete

- The AST cloner's core, the request plumbing in the driver, and what is left
  of template derivation.
- Erased dispatch in the VM and its verifier tolerances. This is the one
  comparison path that outlives its stage, and it goes here.
- A last pass over `docs/architecture.md`. Each earlier stage already updated
  the pipeline it changed.

### P6 — Parametric MIR as the package artifact

- Serialize verified, drop-elaborated generators. This is Mojito's chosen
  boundary. Upstream's `.mojoc` holds something earlier: the post-parse
  `lit.package` with its bodies, written after the semantic checks and before
  lowering.
- The first consumer is the bundled standard library, checked once per
  compiler build rather than once per compilation.
- A package keeps every exported template, whether or not the producing
  program's entry graph reaches it. An entry-pruned executable artifact is
  not a package.
- Importing skips the source check. It still validates the artifact and
  checks each instance's obligations.
- The implementation is last, but its metadata shapes the generator contract
  now: exported signatures, generic bodies, conformance and effect summaries,
  dependency identities, source provenance, compiler and schema
  compatibility, target assumptions, and cache invalidation.

## Decisions for the owner

- **D1. The VM runs only concrete MIR.** Recommended: yes, from P1, with the
  erased path kept as an oracle until P5. Upstream's interpreter never runs a
  generator.
- **D2. Where the elaborator lives.** For P1, `native::mono` stays in
  `mojito-native` and the root driver calls it for both backends, with no new
  edge. Connecting CTFE is different: `mojito-comptime` does not depend on
  `mojito-native` today. The CTFE protocol entry draws the call and crate
  graphs and brings the edge or the extraction to the owner as a design
  question, per `AGENTS.md`. Renaming the crate at P5 does not settle it.
- **D3. Module-scope `comptime` values.** Decided before P3, not at P4. A
  module constant can depend on a generic compile-time call whose result
  shapes another declaration's signature, so this is about types, not module
  initialization. The decision says which closed constants are folded before
  the check and which dependencies stay typed requests. Early folding may
  stay as an implementation for a while; the boundary may not stay undefined.
- **D4. The budget.** Proposed, for the owner to set:
  - A named workload set: the three P0 programs, the P3 benchmark (roadmap
    §1), and one many-instantiation program.
  - Debug and release baselines, with the P0 revision and machine recorded.
    Compilation is measured apart from execution, repeated and interleaved,
    with a stated noise tolerance, and with differential verification off.
  - Peak memory, instance counts, and concrete-MIR size are tracked beside
    time. Eager specialization can add work and code even as it removes
    rounds.
  - 1.20 times baseline is a regression alarm on any row, not a target.
  - P4 owes a stated improvement on the workloads repeated checking
    dominates, and bounded regressions elsewhere. It does not owe a faster
    time on every row.
- **D5. The MIR text schema.** Either reserve the complete generator format
  at P3a or allow further versioned bumps through P3. Recommended: allow the
  bumps. Whether pre-P3 artifacts stay readable is decided at P3a.

## What would stop the plan

- P1 does not switch the default while its parity gate has unexplained
  failures.
- If P1 misses the budget, the erased default stays while reachability,
  copying, and instance caching are investigated. A slow first implementation
  does not make the goal a non-goal. Only a measured floor above the budget
  after that investigation stops the plan, and the owner makes that call.
- If ownership cannot be decided on a compile-time region, running ownership
  after elaboration is acceptable only as a temporary strategy, and only if
  it still rejects an invalid untaken arm and an invalid unused declaration.
  A fallback that checks only the selected instance accepts more programs,
  which is a language change, not a phase-order divergence.

## `template_facts.rs`

The tree is 18,823 lines. It is split along its own seams — capture,
certificate, the `BodyShape` grammar by class, realization, installation,
verification — one file each under `checker/template_facts/`, none over
3,000 lines, so each later deletion removes whole files
(`docs/symbol-map.md` lists the owners).

- **By stage**: P2 deletes the method and function classes, each P3 step
  deletes its class, and P5 deletes the mechanism.

The freeze forbids a new certificate class or recipe. It allows a correctness
fix to existing behavior.

Two measures track progress. The tree's line count goes down from P0 on. The
census says which semantic decisions still need a clone or an inference, and
that is the measure a stage is judged by: a migrated class is complete when
its clone path is deleted.

The line shares per class are not measured. Each stage's plan sizes them.

## Sizes

Line counts are `wc -l` on 2026-09-30. The split between kept and deleted is
an estimate.

| Component | Lines | Fate |
|---|---|---|
| `comptime/{rewrite,specialize,mono,nested}.rs` | 10,698 | Deleted at P5; CTFE, fuel, and value crossing stay |
| `checker/template_facts.rs` and submodules | 18,823 | Deleted by P5 |
| `mojito-checked/src/templates.rs` | 2,681 | Mostly deleted; binder and obligation vocabulary moves to the generator |
| `checker/comptime_validation.rs` | 1,623 | Merges into the one check at P4 |
| `native/mono` | 5,621 | Kept and extended; becomes the elaborator |
| VM erased dispatch | not measured | Deleted at P5 |
