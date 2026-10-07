# Parametric MIR and One Elaborator: Staged Plan

**Status:** plan recorded 2026-09-30. P0's census (§P0) and P1 have landed,
P2's gate, the generator contract, is written, and P2's first step landed
2026-10-01 (§P2). The scheduled
tasks are [`docs/roadmap.md`](roadmap.md) track `pmir`; this document is their design
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
  were compilations, not runs. The corpus group that became `erased_vm`
  measured the runs: 786 of 810 `assets/ok` fixtures behaved as the erased
  program did when it landed, and the rest were filed and closed before P1
  switched the default.
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
reappear inside MIR under other names. The contract is its own note,
[`docs/notes/generator-contract.md`](notes/generator-contract.md), written
2026-10-01. What follows is its outline.

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

**Availability.** A conditional member's `where` clause is on the generator,
with the conformance rows that decide it, and `native::mono` evaluates it
under each instance's bindings. A demanded member its instance disproves is
an error, and a discovered one is skipped. The contract note's §Availability
has the rules.

**"Check once"** means one semantic checking process per declaration, not one
traversal. Effect summaries and loop dataflow still iterate to fixed points
inside it.

A body the symbolic check cannot type is a defect in the symbolic check, filed
against the pin's behavior. It is not a reason to check a clone.

## Phases and their names

This plan uses three names, and since P1 the code uses them:
`CompiledProgram::{mir, drop_elaborated_mir, concrete_mir}`.

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

The last three rows counted a clone that keeps a parameter of its own among
the erased bodies. The census now reports the two apart
(`instantiation.erased.*` and `instantiation.cloned_parametric.*`). At
2026-10-02, after P2's first steps:

| Parametric bodies in MIR | `hello` | `generic` | `stdlib_heavy` |
|---|---:|---:|---:|
| Templates no clone replaces | 303 | 305 | 306 |
| … of which `main` reaches | 0 | 11 | 42 |
| Instances those serve with no clone | 0 | 11 | 120 |
| Clones that keep a parameter | 42 | 43 | 53 |
| … of which `main` reaches | 0 | 0 | 1 |
| Instances those serve | 0 | 0 | 3 |

How to read it:

- A cloned body has one class. Where several describe it, it takes the one
  whose stage lands last, since the cloner keeps it until then
  (`mojito_checked::census::CloneClass`).
- The cloned rows count the converged program. The checked rows count
  distinct bodies over every discovery round, and they sum to the cloned
  total in all three programs.
- The instance rows come from running `native::mono` from `main`. A
  parametric body left in MIR is a function whose types still name a
  parameter. In the 2026-09-30 table a clone that keeps one of its own
  parameters is among them (`Tuple$t2[…].write_to`, generic in its writer).
  The cloner now reports the names its clones lower under
  (`CloneCensus::minted`), and the later table splits them out.
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
  what each costs to check. `benchmarks/compile/keyed.mojo`, below, is the
  program that exercises them.
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

#### The P3 benchmark (2026-10-02)

`benchmarks/compile/keyed.mojo` mints the bodies P3a and P3b move, which the
three programs above do not: user `def`s folding a `comptime if` on a value
parameter (`scale[n]`, four instances) and on a type parameter (`describe`,
four), unrolling a `comptime for` (`unrolled_sum[n]`, three), and expanding
a type pack (`show` and `count`, five); the bundled `rotate_bits_left`
(three); and a generic struct's method holding a `comptime if` on `Self.T`
(`Cell.label`, three). The census at `444f4e03` plus the benchmark, beside
Hello World at the same revision; rows that read zero in both are left out:

| Bodies | Stage | `hello` | `keyed` |
|---|---|---:|---:|
| **Minted by the AST cloner** | | **232** | **254** |
| `def` holding a `comptime if` | P3a | 0 | 11 |
| `def` holding a `comptime for` | P3b | 0 | 3 |
| `def` expanding a type pack | P3b | 0 | 5 |
| Member of a `DType`- or vector-keyed struct | P3c, P3d | 43 | 43 |
| Member of a variadic struct | P3d | 175 | 175 |
| Per-instantiation method clone holding a `comptime if` or `for` | P3 | 0 | 3 (0 since 2026-10-05) |
| Per-call method clone | P3e | 14 | 14 |
| **Checked**: inferred | | 20 | 30 |
| **Checked**: derived from a checked template | | 212 | 224 |
| Templates no clone replaces | | 303 | 304 |
| … of which `main` reaches | | 0 | 14 |
| Instances those serve with no clone | | 0 | 16 |
| Clones that keep a parameter | | 42 | 42 |

- The 22 bodies over Hello World are the 22 the program asks for; the
  keyed `def`s add no fixed cost. At P3a (2026-10-03) the `def holding a
  comptime if` row reads 0 and the cloned total 243: `scale`, `describe`,
  and the bundled `rotate_bits_left` are served by their templates.
- Ten of the 22 are inferred and twelve derived, where Hello World derives
  eleven bodies in twelve: a keyed class pays a full check per instance far
  more often than the fixed cost does.
- `total` from `--timings`, debug profile, `run` on the VM, three runs
  interleaved with Hello World on the same binary: `keyed.mojo` 8.85 s
  (8.85–8.90) against `hello.mojo` 8.72 s (8.63–8.74). Absolute times have
  drifted from the P0 rows above, so a P3 stage reruns both, interleaved
  with the commit before it.

### P1 — One elaborator below the waist, for both backends

Done 2026-10-01. `native::mono` is the elaborator, and both backends consume
its output.

- `mir::verify::verify_concrete` rejects any symbolic type or compile-time
  parameter. `mir::ConcreteMir` is built only by passing it, and
  `native::mono` returns one. A resolved call keeps no compile-time
  argument slot, and a slot that still forwards a binder or computes an
  expression over one is rejected too.
- The three phases of §Phases and their names are
  `CompiledProgram::{mir, drop_elaborated_mir, concrete_mir}`. The entry
  roots are `native::mono::entry_roots`. The concrete graph is cached, and
  the native backend takes it and elaborates nothing.
- The VM runs concrete MIR by default, for source programs and for loaded
  artifacts. The erased path is selectable as the differential oracle until
  P5: `--erased`, `MOJITO_VM_ERASED=1`, or
  `Compiler::with_vm_instantiation`.
- CTFE and the stage-composed `Backend::run(&CheckedProgram)` seam still run
  erased bodies. `mojito-vm` sits above `mojito-native` and cannot call the
  elaborator. CTFE moves at P4.
- The parity gate is the corpus binary's `erased_vm` group: for each
  `assets/ok` and `assets/runtime_error` program, the concrete run and the
  erased run agree on output or error text and on the ordered lifecycle
  events, with fixed stdin for the programs that read it, and the serialized
  artifact, loaded and elaborated, reaches the same outcome. The type-error
  and ownership-error folders keep their own groups: P1 changes nothing
  above the waist, so no program is accepted that was rejected.
- The erased VM is a migration comparator. Where it and the pin disagree, the
  pin decides.

Measured against decision D4 on the same binary, erased and concrete
interleaved, debug profile, `total` from `--timings`, median of three:

| Program | Erased | Concrete | Ratio | Elaboration | Concrete functions | Peak RSS, erased → concrete |
|---|---:|---:|---:|---:|---:|---:|
| `hello.mojo` | 8.662 s | 8.628 s | 0.996 | 0.004 s | 5 of 911 | 293 MB → 232 MB |
| `generic.mojo` | 9.200 s | 9.143 s | 0.994 | 0.016 s | 29 of 1,001 | 311 MB → 248 MB |
| `stdlib_heavy.mojo` | 12.081 s | 12.165 s | 1.007 | 0.273 s | 182 of 1,338 | 407 MB → 348 MB |

- Every row is inside the 1.20 alarm, so concrete is the default.
- Elaboration costs what it instantiates. The VM then builds its registries
  from, and clones, only the reachable graph, which is where the time and the
  memory come back.

### P2 — Stop cloning what the elaborator can already instantiate

- Gate, met 2026-10-01: the generator contract is written and every existing
  obligation is classified
  ([`docs/notes/generator-contract.md`](notes/generator-contract.md)). Two
  obligations turned out to be checks the pin makes on the declaration and
  Mojito does not. They were roadmap entries (row 20's is still R112), and P2 depends on them.
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

#### The first step (2026-10-01)

A method of an ordinary generic struct with no compile-time construct in its
body mints no clone on a plain-data instance. The elaborator instantiates
its template's MIR, lifecycle members and constructors included.

- **What still clones.** A method whose body holds or reaches a compile-time
  construct, and every method of an instance whose argument carries a loan or
  a callable. Roadmap track `pmir` has one entry for each reason.
- **Discovery.** No clone check walks a template-served body at an instance's
  arguments. The driver reads from the template's checked types which
  instances the body reaches and which methods only an instance's own check
  can serve (`src/compiler/template_reach.rs`), and the elaborator hands a
  template body's call on a closed receiver to that instance's clone where
  one exists.
- **The declaration checks the contract named.** A `^` transfer of a
  parameter type needs a bound proving `Movable` (row 10). An `@implicit`
  conversion is selected once where the template serves the method (row 20);
  a method that still clones repeats it, and that is roadmap R112.
- **The serialized `Call`.** A static call records its spelled receiver type,
  from which the elaborator binds the struct's parameters (text schema 1.8).
- **The erased oracle.** It agrees on every fixture but the
  `ERASED_VM_RESIDUE` rows (`tests/corpus_test.rs`). An erased value carries
  no type arguments, so the oracle cannot hand a template-served body's call
  to the receiver instance's clone as the elaborator does, and it cannot
  tell a place pointer bound to a parameter from a reference.
- **Availability.** A member's `where` clause and each struct's conformance
  rows reach MIR, and the elaborator decides the clause. Its `speculative`
  set and the rollback are gone (text schema 1.9).
- **Loan-carrying instances (2026-10-02).** An instance over a loan-carrying
  argument clones no more than a plain-data one. A template's store of a
  symbolic value publishes the stored type as the summary's source
  (`SigOrigin::Carried`, text schema 1.10), and a call closes it with the
  receiver's arguments
  ([`notes/generator-contract.md`](notes/generator-contract.md) §Carried
  sources). `assets/ok/bundled_instance_loan_carrying_argument.mojo` mints
  248 clones where it minted 413. The three benchmark programs hold no such
  instance and are unchanged.
- **A `def` at a loan-carrying argument (2026-10-02).** Such a call keeps
  its template: the call closes a carried source with the types it binds to
  the `def`'s own binders, and a method's own binders close the same way.
  The driver reads what the served body reaches at those types. The
  elaborator resolves an adapted iterator step on the way: a step that
  returns the element drops the adapter, and one that returns a reference
  keeps it as a copy-out. A `def` whose body holds or reaches a compile-time
  construct keeps its clone. `assets/ok/unspelled_loan_carrying_type_argument.mojo`
  mints 245 clones where it minted 247. The three benchmark programs hold
  no such call and are unchanged.
- **Every closed call of a plain `def` (2026-10-02).** `show(n)` and
  `show[Int](n)` keep the template: a uniquely named `def` with type
  parameters only, no compile-time construct in or reached from its body,
  and no struct specialized whole over its parameters is never cloned. The
  call records the arguments the checker solved
  (`MirInstr::Call::instantiated_args`, text schema 1.13), so the elaborator
  binds a parameter no runtime parameter or result spells (`bytes[Int]()`),
  and the erased oracle reifies a struct's name from them. The driver reads
  what the served body reaches at every closed call. On the way the
  elaborator selects the receiver's own overload for a bound dispatch of an
  overloaded method with binders of its own, a comparison through a bound
  borrows a named operand of the parameter type as the requirement's read
  dunder does, and a loop over a bound reads its exhaustion type from the
  bound's associated iterator. `def_type_arguments` reads 0 on all three
  benchmark programs (12, 16 and 12 before). The `BoundedOperations`
  certificate class served nothing afterwards and is deleted; `FixedCalls`
  admits the built-in `len` instead.
- **Not done.** The clause forms the elaborator leaves undecided, the
  `def`s that still clone (a value parameter beside the type parameters, a
  compile-time construct held or reached, a struct specialized whole over
  the parameters, an overload family), and the exit: clone-symbol
  retargeting and the remaining certificate classes still serve the clones
  that remain — `ClosedScalarBody`, `FixedCalls` and `FunctionBody` now
  only for a stub-reaching, value-keyed, or overloaded `def` and for a
  compile-time evaluation's own subprogram.

The census on the three `benchmarks/compile` programs, before and after:

| Bodies | `hello` | `generic` | `stdlib_heavy` |
|---|---:|---:|---:|
| Minted by the AST cloner | 244 → 244 | 329 → 249 | 666 → 255 |
| Per-instantiation method clone, no compile-time control flow | 0 → 0 | 80 → 1 | 417 → 11 |
| Checked: inferred | 21 → 21 | 26 → 21 | 48 → 21 |
| Checked: derived | 223 → 223 | 303 → 228 | 618 → 234 |
| Parametric bodies `main` reaches | 0 → 0 | 10 → 11 | 36 → 43 |
| Instances those serve with no clone | 0 → 0 | 10 → 11 | 96 → 123 |

A method that spells a type name (`write_repr_to` on `List`, `Dict`, and
`Optional`) no longer clones: the template carries the type in
`MirInstr::TypeName` (text schema 1.11), and the elaborator writes the name
from the substituted type. The table predates that change.

A `def` that names an associated type only in its body (`for item in items`
over `C: Iterable`) no longer clones at a loan-carrying argument: a struct's
MIR declaration carries its associated types (text schema 1.12), and the
elaborator reads `C.Element` off the struct `C` is bound to.

Every closed call of a plain trait-bound `def` keeps its template since
2026-10-02, so the `def`, type arguments only row reads 0 → 0 on all three
programs where the table's baseline read 12, 16 and 12 (`hello`'s fixed
cost was the `os`, `path` and `stat` module `def`s at `String` and `Int`;
`generic`'s added `has_two_or_more` and `is_empty` at `Bag` and
`List[Int]`). Minted by the AST cloner reads 232 on all three.

Measured against decision D4 on the same machine, release profile, `total`
from `--timings`, the commit before and this one interleaved, median of five:

| Program | Before | After | Ratio | Peak RSS |
|---|---:|---:|---:|---:|
| `hello.mojo` | 1.813 s | 1.778 s | 0.981 | 224 MB → 220 MB |
| `generic.mojo` | 1.936 s | 1.782 s | 0.920 | 241 MB → 223 MB |
| `stdlib_heavy.mojo` | 2.637 s | 1.863 s | 0.707 | 339 MB → 247 MB |

### P3 — Compile-time parameters and control flow in MIR

Three prerequisites come first. None of them moves a body.

- **Ownership on a compile-time region.** Decided
  ([`docs/notes/comptime-region-ownership.md`](notes/comptime-region-ownership.md),
  2026-10-02): a compile-time region is decided as the runtime region of the
  same shape, with the condition opaque and the trip count unknown, which is
  what upstream's `Mojo/lib/LowerLIT/CheckLifetimes.cpp` does on its
  structured `ComptimeIfOp` and `ComptimeForOp`. Nineteen pin probes
  (`conformance/probes/comptime_region_*.mojo`) observe it, and the
  `--comptime-regions keep` experiment (since deleted) reproduced every
  verdict and every destructor order on the analysis of the day. P3a and P3b
  therefore add no join rule: a parameter-expression condition is opaque to
  the ownership analysis, and the elaborator keeps the taken arm with its
  drops — which is how P3a landed.
- **Legality is not lifecycle glue.** Symbolic ownership decides legality and
  last use once. Substitution may resolve a destructor witness or expand
  aggregate cleanup, and it never recomputes a last use. A development-only
  concrete ownership comparison checks that, because `mir::verify` alone does
  not establish ownership preservation.
- **CTFE requests instances from the worklist.** Designed
  ([`docs/notes/ctfe-request-path.md`](notes/ctfe-request-path.md),
  2026-10-02). Today `comptime/ctfe.rs` builds an AST subprogram, checks a
  synthesized typing probe, turns the result type back into source syntax,
  and runs a second synthesized helper on the erased body, once per
  discovery round, with no cache. The target path: a compile-time
  application is `ParamKind::Apply` over a lowered thunk; the elaborator
  demands the thunk's instance, drains its reference closure on a nested
  worklist, runs the verified concrete fragment on the VM
  (`VmBackend::call_concrete`), validates the result at the declared type,
  and caches it by the instance key. The effect restrictions,
  value-crossing rules, and shared fuel stay; the fuel becomes one counter
  per compilation.
- **The worklist has keys and states.** Designed in the same note.
  `InstanceKey` stays the key, indexed; an instance is pending, active,
  completed, or failed. A reference edge (a call in a body) never waits, so
  recursion is valid; a demand edge (an application, a layout, a module
  constant) materializes now, and a demand on an active key is upstream's
  "function instantiation in parameter domain that recursively requires
  itself". No depth bound, as upstream: the instance budget is the one
  elaboration bound, apart from VM fuel, and is reached in seconds.
- **A common type vocabulary.** Specified
  ([`docs/notes/param-expr-attributes.md`](notes/param-expr-attributes.md)
  §Register types, 2026-10-03): a register type names a parameter expression
  through the forms that exist — a type binder, a dependent type (a pack
  element included), a symbolic vector lane or width, a symbolic struct
  argument, and the new `ParamKind::Apply` — and the parametric verifier
  accepts one whose binders are in scope and whose expressions are
  well-kinded (`verify/scope.rs`), while the concrete verifier rejects it.
  Layout is asked only of a concrete type, and the elaborator answers every
  layout query under the compilation's native target. A `def` with a value
  parameter beside its type parameters keeps its clone until the elaborator
  binds the value from the call (P3c); every other plain `def` is
  template-served.

Then one class at a time. For each: HIR and MIR gain the form, the verifiers
and ownership analysis accept it, the elaborator resolves it, the cloner's
branch for the class is deleted, and so is its certificate class.

- **P3a. `comptime if` on a value parameter.** Done 2026-10-03: MIR carries
  the region as the `ComptimeBranch` terminator — the `if` diamond with a
  constraint over the body's binders as its condition, which the ownership
  analysis decides as a `Branch` — and the elaborator decides it under the
  instance's bindings and prunes the untaken arm. The request path's first
  code landed with it (`docs/notes/ctfe-request-path.md`). The cloner's
  `comptime if` class is gone for every top-level `def`, uniquely named or an
  overload family's member; a nested `def`'s stays with its entry. A generic
  struct's method followed 2026-10-05: its body is elaborated with the
  struct's parameters open as `Self.`-qualified binders, so a `comptime if`
  or a `range` loop over `Self.T` or `Self.n` stays in the template, and
  `keyed.mojo`'s per-instantiation `comptime` row reads 0 (was 3; the
  cloned total 211). A local `comptime` binding over the struct's
  parameters still keys a clone (R312). The same step made `rebind` a
  template form: MIR carries it as a value rebind (`MirInstr::Rebind`,
  schema 1.23) or a place retyped to its target, and the elaborator asserts
  the equality per instance after deciding its branches, as upstream's
  `processRebindOp` does, so a `def` or a method holding one is served too.
- **P3b. `comptime for` over a value index**, then **heterogeneous pack
  expansion**. They share the loop form and differ in their correctness
  conditions, so they are two steps. The first landed 2026-10-03: MIR
  carries a `range` loop as the `ComptimeFor` header — the loop's own index
  binder and three parameter-expression bounds — the ownership analysis
  decides it as a loop with its trip count unknown, and the elaborator
  unrolls it under each instance's bindings with the nested loops and the
  `comptime if`s over the index decided per copy, before substitution. The
  cloner's class keeps only the loops the template does not serve: over a
  list, a pack, a reflection query, or with a local `comptime` binding in
  the body. Schema 1.16. On 2026-10-05 the header became a sequence
  (`ComptimeSequence`, schema 1.25), as upstream's `kgen.param.for` iterates
  one, and a loop over a list, set, or dictionary display of literals is
  served too. On 2026-10-06 the remaining sequences followed with no schema
  change: a named closed collection, a `def`'s own value pack, a reflected
  field-name list and a reflection bound, a body `comptime` binding over
  the index, and a display over the binders, which MIR lifts as a thunk the
  elaborator runs per instance. A local `comptime` binding of such a
  display followed the same day: MIR lifts it once, where it is declared,
  and every loop over the name iterates that one application. The other
  reads of such a binding followed the same day, with no schema change: a
  `comptime if` condition, a `range` bound, and an `Int` or `Bool` read by
  another binding or crossing are each lifted as a thunk that begins by
  binding the display, which the elaborator demands in a condition, a loop
  header, or a parameter constant, and `materialize[L]()` builds the display
  where it crosses. An element or the length of the binding in a type or a
  parameter argument is a parameter expression over the binding's sequence,
  which the check names and the elaborator runs when a substitution meets
  it (2026-10-06). An argument that applies a function to a binding, or
  compares one, is the application of a function the check names and MIR
  lifts (2026-10-06), and an alias of a binding is another name of it
  (2026-10-06). An element that applies a function and a loop constructing
  a reflected field type still key a clone (R363, R364). The second landed 2026-10-03 for a module-level
  `def` whose body neither spreads the pack nor binds an element's type
  under the loop, its collector read or owned: the pack binder, the
  `VariadicPack` collector, the dependent element `Ts[i]`, and the pack's
  length as a loop bound cross the waist, and the elaborator binds the pack
  from the call's recorded elements; an owned pack is destroyed last to
  first after its last element use. The third landed 2026-10-03: a pack
  spread whole into `print` or another served `def` crosses the waist as
  the call's spread argument (`MirInstr::Call::spread`, schema 1.17), which
  the elaborator expands into the bound pack's element places, and a
  spreading `def` is served when every callee it spreads into is. On
  2026-10-06 a collector-less signature spread (`-> Variant[*Ts]`) and a
  call of an overloaded method with binders of its own were served too:
  the driver no longer keys a body for the overload. Later that day a
  spread into a method with a type-pack collector was served too, judged
  by method name, and a spread into `Writer.write` once `Writer` became the
  bundled trait with upstream's default `write`; a spread into
  `String.format` still keys a clone (R421), the element-typed binding keeps
  the clone (R252), and the certificate class waits on them and on the
  methods (R253).
- **P3c. `DType`, vector, and other value-dependent types.** Layout is asked
  only of a concrete type, and a compile-time layout query names the target
  it is answered for. A concrete type does not make layout independent of
  the target. The first step landed 2026-10-03 for a uniquely named `def`
  keyed on a `DType` binder, a lane width, or a layout operand: the checker
  records every lane fact as its slots, MIR carries the slots in its
  register types and in `MakeSimd`, `SimdCast`, and `SimdBitcast` (schema
  1.18), a symbolic lane count or dtype read is the evaluation of its
  parameter expression, and the elaborator binds the lane binders from the
  call's recorded arguments or a vector argument's slots and closes the slots
  per instance. The cloner keeps a `def` whose body shuffles, slices, joins,
  or hashes a lane value, queries a float format over its binder, binds a
  local `comptime`, or holds a nested `def`, and every overloaded one
  (R257–R261); a method's own lane binder is served since P3e's first
  step.
- **P3d. Struct generators.** Value-keyed and variadic structs, `Tuple` and
  `TString` included, are declared once and instantiated by the elaborator.
  The first step landed 2026-10-05 for a struct keyed on a `DType` or on a
  lane width (the range family, a user `Walker[dt: DType]`, `Lanes[w: Int]`
  holding `SIMD[DType.int32, Self.w]`): it is an ordinary generic struct,
  checked once with the binder symbolic, and `native::mono` mints its
  instances. A scalar `range(...)` constructs the range struct at its dtype,
  and the erased oracle closes a member's lane from its frame's and
  receiver's reified binders. Hello World's census reads
  `struct_dtype_vector 22` (was 43, all `AHasher` now) and
  `template_method_struct_lane.mojo`'s `def_dtype_vector 0` (was 5). The
  second step landed the same day for a struct keyed on a vector value
  (`AHasher[key: U256]`) or on a struct-typed value (`Tagged[e: Extent]`):
  a closed binder read is a parameter-value constant (`Const::Value`, schema
  1.27), a field of a struct-typed value a `ParamKind::Field` node, and the
  elaborator freezes a computed struct argument to its fieldwise
  construction. A variadic struct whose source names no struct still
  specialized whole followed (`served_variadic_structs`), `native::mono`
  binding its pack per instance. Hello World read `struct_dtype_vector 0`
  and total 175; its `struct_variadic 128` was `Tuple`, `TString`, and
  their unrolled members. `Tuple` and `TString` followed on 2026-10-06:
  both are templates `native::mono` instantiates, the tuple requests and
  the `Tuple$t…` spellings are gone, and Hello World's census reads 0
  clones. `Variant` followed the same day, and with it every variadic
  struct: its storage operations carry upstream's
  `_get_type_index[T, *Ts]()` as a symbolic `VariantIndex` (schema 1.31),
  which `native::mono` closes per instance, failing a non-member. The
  driver's variadic-struct requests, `generate_struct_spec`, the per-index
  accessor clones, the variadic shells, and the census's three
  struct-member classes are gone; `Variant[Int, Bool](1)` with `isa[Int]`
  reads 0 clones (was `struct_variadic 21`).
- **P3e.** Three steps: a method's own compile-time parameters, nested
  definitions and their captures, and clones minted during CTFE. The first
  landed 2026-10-05: a method with compile-time parameters of its own is a
  generator, its call carrying the arguments the checker solved
  (`MirInstr::MethodCall::instantiated_args`, schema 1.22) and
  `native::mono` binding them beside the receiver's, a generic constructor,
  an own lane binder, and a `comptime if` or `comptime for` over its own
  binders included. The probes' own per-call clones read 0 (`S.show[T]`
  from 3, `Box[T].pair[U]` from 3 per-instantiation-filed ones). The census
  now files a per-call clone in its class wherever it was minted, so Hello
  World's `method_per_call` reads 75 (the 14 `Fnv1a` leaves, `AHasher`'s 14,
  and 47 unrolled members of the variadic specializations) beside
  `struct_dtype_vector 8` and `struct_variadic 128`, its total unchanged at
  211. A method keyed on a type pack of its own followed with R307:
  `assets/ok/pack_forwarding_method.mojo` and the bundled
  `FormatStruct.params` read 75 too. The hashers' `SIMD[_, _]` leaf
  followed with R259 (2026-10-05): `_update_with_simd` is a generator over
  an infer-only dtype and `SIMDLength` binder pair, the 28 eager leaves
  are gone, and Hello World reads `method_per_call 47`, total 183. What
  still clones per call is R310 (a body only a clone serves).

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
- Module-scope `comptime` values follow decision D3: a constant the
  `ParamExpr` folder closes is folded before the check; one whose
  initializer applies anything is a typed request. The early folding of
  applied constants goes here.

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
- **D2. Where the elaborator lives.** Decided 2026-10-02
  ([`docs/notes/ctfe-request-path.md`](notes/ctfe-request-path.md) §D2):
  `native::mono` stays in `mojito-native`, the root driver calls it for both
  backends, and `mojito-native` gained the edge to `mojito-vm` with the
  request path (2026-10-03), so the elaborator owns its executor as
  upstream's `Elaborator` owns its interpreter. `mojito-comptime` never
  depends on `mojito-native`; its AST route is deleted at P4, not bridged.
- **D3. Module-scope `comptime` values.** Decided 2026-10-02 (same note,
  §D3), from the pin: a module constant is folded before the check when the
  `ParamExpr` folder closes its initializer (literals, constants,
  operators, type names, predicates); one whose initializer applies
  anything is a typed request, with the symbolic value `ParamKind::Apply`,
  equal by structure and evaluated only by the elaborator, so
  `SIMD[DType.float32, C]` matches itself and not a folded width. Landed
  2026-10-03 with the register types: the applied constant keeps its name
  in every type argument and its node carries the folded value, so the
  early folding stays as an implementation until P4 without losing the
  identity.
- **D4. The budget.** Proposed, for the owner to set:
  - A named workload set: the three P0 programs, the P3 benchmark, and one many-instantiation program.
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
- **D5. The MIR text schema.** Decided at P3a (2026-10-03): the bumps are
  allowed, and every earlier minor stays readable. Schema 1.15 carries the
  `comptime_branch` terminator and 1.16 the `comptime_for` header, which
  1.25 generalizes to a sequence; the reader accepts every minor through the
  current one.

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
