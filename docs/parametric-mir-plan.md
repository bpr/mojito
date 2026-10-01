# Parametric MIR and One Elaborator: Staged Plan

**Status:** plan recorded 2026-09-30. Nothing here has landed. The scheduled
tasks are [`docs/roadmap.md`](roadmap.md) §1; this document is their design
record and is updated as stages land.

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

A package is the phase-2 output serialized, so an importer elaborates it
without checking it again.

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
- That derivation is `checker/template_facts.rs`: 16,138 lines, plus 2,390 in
  its submodules. It grows by one recipe per body shape and cannot reach
  every shape, because its carrier is still the clone.
- The VM and the native backend instantiate generics by different means, so
  each new generic feature is implemented twice.

Three facts make the target reachable from here.

- **MIR is already parametric for one class.** A trait-bound generic body
  reaches MIR with `Ty::Param` types, passes `mir::verify` and ownership
  analysis once, and `native::mono` substitutes types and compile-time values
  into it per instance. That is a generator and an elaborator in all but
  name.
- **Elaborated MIR already runs on the VM.** The A1 experiment's census
  specialized all 893 of its corpus inputs into MIR that verifies, and its
  census test asserted that specialization is transparent to the VM
  ([`docs/notes/pliron-a1.md`](notes/pliron-a1.md) §Coverage). The fixes that
  made that true stayed when the experiment was removed. The test that pinned
  it did not.
- **The VM is already the compile-time interpreter.** CTFE lowers a helper
  through HIR and MIR and runs it on `VmBackend`.

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

## The rule that replaces certificates

A checked template's MIR is valid for every instance when no fact in it
depends on the instance. Where a decision does depend on the instance, it is
an explicit MIR operation the elaborator resolves, never a second check.

- A call through a bound is already such an operation: its contract names the
  requirement, and the instance supplies the witness.
- Copying, moving, and destroying a value of a parameter type are resolved
  from the instance's lifecycle members.
- A `comptime if` or `comptime for` becomes a region the elaborator selects
  or unrolls (stage P3).
- A constraint an instance owes (a `where` clause, a conformance, a `rebind`
  equality) is recorded on the generator and checked when it is instantiated.
  The list in the instantiation note, §What an instance still owes, is the
  specification.

A body the symbolic check cannot type is a defect in the symbolic check, filed
against the pin's behavior. It is not a reason to check a clone.

## Stages

Each stage keeps the path it replaces alive behind a switch until the
nightly gate agrees on both, the way `MOJITO_VERIFY_TEMPLATE_FACTS` compares
derived facts with inferred ones today. A stage is not done until the code it
replaces is deleted.

### P0 — Census and freeze

- Count, per compilation, which mechanism serves each generic body, by class.
  The classes are the rows of the table above.
- Count how many instance bodies are inferred, how many derived, and how many
  the erased body serves with no clone.
- Freeze `template_facts.rs`: no new certificate class and no new recipe. A
  body the certificates do not cover waits for its stage.
- The counts order the classes inside P2 and P3. This document's order is the
  default until they exist.

### P1 — One elaborator below the waist, for both backends

- Restore a corpus check that elaborated MIR verifies and runs on the VM as
  the erased program does. It needs no LLVM, so it belongs in the default
  lane unless its memory peak says otherwise.
- Give `mir::verify` a concrete mode that rejects any symbolic type or
  compile-time parameter. `native/mono/symbolic.rs` enforces that today,
  outside the verifier.
- Run the VM on elaborated MIR by default. The erased path stays selectable
  as the differential oracle until P5.
- Exit: one instantiation mechanism below the waist. `native::mono` is the
  elaborator, and both backends consume its output.

### P2 — Stop cloning what the elaborator can already instantiate

- An ordinary generic struct's methods are cloned per instance only so the
  checker can check them concretely. With P1, the elaborator instantiates the
  template's MIR instead.
- The same holds for an explicit application of a trait-bound generic `def`.
- Bodies holding a compile-time construct keep the cloner until P3.
- The obligations those clone checks discharged move to the symbolic check
  where the pin rejects symbolically, and to an instantiation-time constraint
  otherwise.
- Exit: the method and function certificate classes that exist only to derive
  these clones are deleted from `template_facts.rs`.

### P3 — Compile-time parameters and control flow in MIR

One class at a time. For each: HIR and MIR gain the form, the verifier and
ownership analysis accept it, the elaborator resolves it, the cloner's branch
for the class is deleted, and so is its certificate class.

- **P3a. `comptime if` on a value parameter.** MIR gains a structured
  compile-time conditional, as `Try` is a structured instruction today.
  Ownership analyses every arm. The elaborator keeps the taken one.
- **P3b. `comptime for` and type packs.** A structured compile-time loop
  whose body is analysed once with the index symbolic and unrolled at
  elaboration.
- **P3c. `DType`, vector, and other value-dependent types.** Registers carry
  types built over parameter expressions. Layout is asked only of concrete
  MIR.
- **P3d. Struct generators.** Value-keyed and variadic structs, `Tuple` and
  `TString` included, are declared once and instantiated by the elaborator.
- **P3e. A method's own compile-time parameters, nested defs, and clones
  minted during CTFE.**

The MIR text schema is bumped once, at P3a, for generators and compile-time
regions.

### P4 — Check once, then elaborate

- With no clone left to check, the executable check runs once on the linked
  source, and source validation and the executable check become one pass.
- The discovery fixpoint becomes the elaborator's worklist: a call in a
  generator names its callee and its parameter expressions, and the
  elaborator finds the instances transitively.
- Module-scope `comptime` values are the open question of this stage
  (decision D3).

### P5 — Delete

- The AST cloner's core, the request plumbing in the driver, and what is left
  of template derivation.
- Erased dispatch in the VM and its verifier tolerances.
- `docs/architecture.md` is rewritten to the target pipeline in the same
  change.

### P6 — Parametric MIR as the package artifact

- Serialize verified, drop-elaborated generators, which is what upstream's
  `.mojoc` holds.
- The first consumer is the bundled standard library, checked once per
  compiler build rather than once per compilation.

## Decisions for the owner

- **D1. The VM runs only elaborated MIR.** Recommended: yes, from P1, with the
  erased path kept as an oracle until P5. Upstream's interpreter never runs a
  generator.
- **D2. Where the elaborator lives.** `native::mono` sits in `mojito-native`,
  after `mojito-vm` in the crate order, and the root driver can call it for
  both backends with no new edge. Recommended: leave it there through P4 and
  rename the crate at P5. Any stage that seems to need a new dependency edge
  stops and asks, per `AGENTS.md`.
- **D3. Module-scope `comptime` values.** Upstream keeps them symbolic until
  elaboration. Recommended: keep folding them before the check through P4 and
  decide then, since the checker reads them in types today.
- **D4. The compile-time budget.** Proposed: no stage may leave a
  `docs/performance.md` row slower than 1.20 times its P0 baseline, and P4
  must leave every row faster than the baseline, since it removes the
  discovery rounds.
- **D5. The MIR text schema bump** at P3a, and whether pre-P3 artifacts stay
  readable.

## What would stop the plan

- P1 fails if elaborated MIR cannot be made to verify and run on the whole
  corpus, or if D4's budget fails with no fix. The plan then stops with the
  two-mechanism arrangement, and that is recorded in `docs/non-goals.md`.
- P3a fails if ownership cannot be decided on a compile-time region without
  knowing the taken arm. The fallback is to run ownership after elaboration
  for such generators, a recorded divergence from upstream's order, not the
  end of the plan.

## `template_facts.rs`

The file is 16,138 lines: one `impl Checker` block of about 7,000 lines, a
band of free helpers, and the `BodyShape` certificate grammar of about 6,000.
Two things reduce it.

- **Now, behavior-preserving** (roadmap §6): split it along its own seams —
  capture, certificate grammar, realization, installation, verification — so
  no file is over 3,000 lines and each later deletion removes whole files.
- **By stage**: P2 deletes the method and function classes, each P3 step
  deletes its class, and P5 deletes the mechanism. From P0 on the tree's line
  count only goes down.

The line shares per class are not measured. P0's census and each stage's plan
size them.

## Sizes

Line counts are `wc -l` on 2026-09-30. The split between kept and deleted is
an estimate.

| Component | Lines | Fate |
|---|---|---|
| `comptime/{rewrite,specialize,mono,nested}.rs` | 10,698 | Deleted at P5; CTFE, fuel, and value crossing stay |
| `checker/template_facts.rs` and submodules | 18,528 | Deleted by P5 |
| `mojito-checked/src/templates.rs` | 2,681 | Mostly deleted; binder and obligation vocabulary moves to the generator |
| `checker/comptime_validation.rs` | 1,623 | Merges into the one check at P4 |
| `native/mono` | 5,621 | Kept and extended; becomes the elaborator |
| VM erased dispatch | not measured | Deleted at P5 |
