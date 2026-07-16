# Mojito Roadmap

This is the project's single task tracker. It records the project's direction,
current capabilities, and a mostly dependency-ordered list of unfinished work.
The ordered list contains only pending or demand-driven tasks; completed tasks
do not remain there as checked boxes.

The north star is self-hosting: useful standard-library code should expose the
next missing compiler capability. Prefer the smallest honest language change
that unlocks a real library pattern, with positive and negative tests.

## Current State

- [x] **Register VM pipeline** — HIR, MIR, ownership analysis, drop elaboration,
  and the register VM are the only execution path.
- [x] **Source modules and packages** — dotted, relative, qualified, aliased, and
  lexically scoped imports resolve through configurable roots; package
  `__init__.mojo` files re-export public declarations, while module-qualified
  internal identities prevent collisions after flat linking.
- [x] **Compile-time elaboration and CTFE** — compile-time constants and control
  flow, value specialization, type predicates, associated facts, and fuel-bounded
  VM execution work for the supported subset.
- [x] **Generic traits and associated facts** — user trait requirements,
  associated `comptime` members, iteration, comparison, sizing, hashing, numeric
  operation traits, and lifecycle marker traits have useful semantics.
- [x] **Signature-aware overloading** — functions, methods, and constructors use
  checker-selected overloads and canonical lowered symbols from `src/symbol.rs`.
- [x] **Mojo-shaped free-function arguments** — positional-only, keyword-only,
  defaults, keyword calls, and homogeneous `*args` work for ordinary functions.
- [x] **Self-hosted collection base** — `Optional`, `List`, `Set`, list-backed
  `Dict`, and experimental hash-backed `HashSet` are covered by self-host tests.
- [x] **Self-hosted algorithms and math** — generic iteration, direct compile-time
  facts, hashing helpers, and numeric rounding helpers exercise the compiler.
- [x] **Stabilization checkpoint** — strict local checks, named compiler records,
  canonical overload symbols, MIR declaration metadata, and actionable trait
  diagnostics are in place.
- [x] **Origin and reference semantics** — origin-bearing parameters, receivers,
  returns and unions use stable checked identities, persistent field-sensitive
  CFG loans, interprocedural substitution and escape checking, and executable
  frame/slot reference handles with captured projections.
- [x] **Overload rejection hardening** — duplicate, ambiguity, no-match,
  generic-ranking, bound-symbol, nested-def, and namespace regressions are pinned
  by the required overload rejection suite.
- [x] **Versioned CPU-parity ledger** — the pinned Mojo 1.0.0b2 manual inventory
  classifies each feature family as parity, strict subset, divergence,
  representation difference, exclusion, or stretch, with validated evidence.
- [x] **Differential CPU conformance baseline** — shared fixtures exercise every
  implemented first-pass match plus representative subset and divergence edges
  against the pinned Mojo build; manifest validation requires executable evidence
  for parity and divergence claims.
- [x] **Trait refinement and protocol foundation** — refined traits inherit
  requirements and capabilities, default bodies materialize statically with
  override/ambiguity rules, opaque bounded indexing dispatches, incremental
  hashing has a self-hosted proof, and user formatting is capability-gated.

## Ordered Work

The order below expresses dependencies, not a promise that every item must be
implemented. Demand-driven items should be promoted only when a concrete stdlib
or user program needs them.

### 1. Close Lifecycle And Call-Semantic Gaps

- [x] **Current lifecycle trait vocabulary** — implement
  `ImplicitlyDestructible` throughout generic constraint and explicit-destroy
  checking, migrate bundled sources away from `ImplicitlyDeletable`, and then
  reject the obsolete trait name. Legacy `__copyinit__`/`__moveinit__` may remain
  a documented compatibility extension while bundled code migrates to unified
  `__init__(copy=)`/`__init__(move=)` declarations.
- [x] **User-defined implicit conversions** — retain and validate `@implicit`
  constructors, include their conversion cost in overload ranking, lower the
  selected conversion explicitly, and reject ambiguous or effect-incompatible
  conversions. This is independent of explicit-destroy analysis and can land
  before that larger chain.
- [x] **Explicit-destroy declaration semantics** — retain `@explicit_destroy`
  and its diagnostic message in checked/MIR type metadata; require at least one
  named `deinit self` method; attach explicit-destroy type facts to MIR variables;
  suppress automatic `DropVar` destruction.
- [x] **Explicit-destroy obligation analysis** — track obligations for initialized
  values through moves, partial moves, branches, loops, returns, and exceptional
  regions; discharge only through an explicit consuming destructor; reject
  abandonment, conditional destruction, and double destruction.
- [x] **Raising explicit destruction** — preserve an obligation when a raising
  destructor fails so an `except` path can select another destructor; verify that
  every normal and handled-error exit discharges exactly once.

### 2. Finish Traits And Core Protocol Contracts

- [ ] **Associated-type equality and composition** — support constraints tying
  associated types across bounds and composed traits, with conflict diagnostics.
- [ ] **Conditional conformance clauses** — represent and solve conformance
  conditions independently of a generic struct's declaration bounds.
- [ ] **Standard Indexer family** — replace the proof-only `__getitem__` path with
  the standard associated index/output contracts, mutation, and slicing variants.
- [ ] **Incremental Hasher contract** — implement the standard byte/value writing
  surface and make Hashable implementations feed a caller-provided hasher.
- [ ] **Writer protocol** — implement the generic Writer operation surface,
  checked writes, error effects, and buffer-backed acceptance tests.
- [ ] **Formatting protocols** — build current Writable formatting on Writer,
  including display/repr operations, format specifications, and a self-hosted
  formatter; do not make deprecated Stringable/Representable a parity target,
  and retire the temporary direct `__str__` hook.

### 3. Generalize Parameters, Packs, And Specialization

- [ ] **General compile-time parameter values** — accept parameter types beyond
  Int/Bool/type/origin and define which values may materialize at runtime.
- [ ] **Complete parameter binding** — unify explicit, inferred, defaulted,
  positional, keyword, type, value, and origin parameter binding rules.
- [ ] **Variadic type and value packs** — represent packs explicitly, infer them
  from arbitrary expressions, and expose per-index concrete types.
- [ ] **Dependent parameter expressions** — type/evaluate parameter expressions
  that depend on earlier parameters, including result and field types.
- [ ] **Generic constraints** — parse and solve general compile-time predicates,
  including trailing declaration `where` constraints and conditional
  conformance; do not extend deprecated parameter-list `where` syntax.
- [ ] **Nested specialization** — specialize generic helpers called by generic
  CTFE and recursively requested specializations.
- [ ] **Specialization cache and diagnostics** — canonicalize specialization keys,
  share fuel, detect cycles, and report recursion/fuel failures at source sites.
- [ ] **Compile-time value model** — add richer aggregates/type facts with explicit
  materialization rules instead of ad-hoc evaluator cases.
- [ ] **Reflection queries** — expose the type/declaration facts required by
  concrete conformance and standard-library cases through the current unified
  `reflect[T]` model, not the removed free-function reflection API.
- [ ] **Compile-time declaration generation** — generate declarations only after
  reflection use cases establish a minimal, testable contract.

### 4. Complete Callable And Closure Semantics

- [ ] **Overloaded callable values** — select or retain overload sets in typed
  callable contexts, including effects and generic specialization.
- [ ] **Explicit capture lists** — require and represent current capture-list
  entries (`read`, `mut`, `ref`, and moved `var` captures) in checked data and
  MIR closure environments; ordinary nested functions have no implicit capture.
- [ ] **Non-escaping closure completeness** — support sibling calls, recursion,
  generics, closure values, and mutable/reference captures without write-back
  emulation, while rejecting any closure that outlives its defining scope.

### 5. Close Remaining CPU Language Surface

- [ ] **Late initialization and function-scoped implicit bindings** — track
  definite initialization and Mojo's distinct implicit-variable scope rules.
- [ ] **Context managers** — check and execute `with` through the enter/exit
  protocol, including raising exits.
- [ ] **Loop completion** — implement loop `else`, reference loop bindings, and
  the remaining iterator variants.
- [ ] **Pattern completion** — add declaration destructuring and remaining
  CPU-relevant structured patterns beyond tuple assignment.
- [ ] **String interpolation** — type, lower, and execute t-strings through the
  formatting protocols.
- [ ] **Walrus expressions** — define binding scope, ordering, and MIR execution.
- [ ] **Slice completion** — cover omitted/negative/strided bounds and
  protocol-driven user slicing.
- [ ] **Operator completion** — add bitwise operations, shifts, matrix multiply,
  and missing protocol-dispatched CPU operators.
- [ ] **Literal-family completion** — implement byte and string literal families,
  escaping/interpolation interactions, and the remaining numeric literal rules.
- [ ] **Tuple and variant completion** — general tuple packs, supported dynamic
  operations, and CPU-relevant variant construction/matching.

### 6. Complete References And Unsafe Pointers

- [ ] **Advanced origins** — implement static, untracked, and unsafe origins with
  explicit escape and access rules.
- [ ] **Reborrows and reference aggregates** — model reborrow lifetimes and permit
  references inside aggregates only where ownership rules remain sound.
- [ ] **Pointer provenance and arithmetic** — track allocation identity, typed
  offsets, bounds where promised, and pointer comparisons/conversions.
- [ ] **Pointer lifetime operations** — implement explicit deallocation,
  alignment-aware allocation, non-null dangling placeholders, and invalid/
  double-free diagnostics; nullable pointers use `Optional`, not the removed
  default `UnsafePointer()` constructor.

### 7. Stabilize Textual MIR/VM Assembly

- [ ] **Backend-ready MIR** — remove remaining source-AST reconstruction and make
  checked declarations plus verified MIR sufficient inputs before freezing a
  serialized schema.
- [ ] **Text format schema** — specify versioning, deterministic identifiers,
  declarations, blocks, instructions, constants, types, and source locations.
- [ ] **Disassembler** — print every verified MIR program deterministically and
  add stable snapshots for representative programs.
- [ ] **Assembler parser and diagnostics** — parse the text format with precise
  source errors and no dependency on Mojo source syntax.
- [ ] **Standalone verifier** — validate symbols, types, CFGs, ownership metadata,
  and instruction invariants before execution.
- [ ] **Lossless round trips** — require MIR → text → MIR equivalence across the
  full test corpus.
- [ ] **VM artifact execution** — run verified textual artifacts directly from
  the CLI.
- [ ] **Compiler/test integration** — expose dumps and use assembly snapshots and
  conformance artifacts as backend-independent contracts.

### 8. Reduce Builtins And Grow The CPU Standard Library

- [ ] **Protocolize scalar operations and conversions** — route numeric,
  comparison, conversion, and rounding behavior through checked protocols.
- [ ] **Protocolize collections and iteration** — route list/range/tuple indexing,
  sizing, containment, and iteration through the same contracts as user types.
- [ ] **Self-hosted Unicode String** — define storage, Unicode indexing/slicing,
  literal interop, comparison, hashing, and formatting without VM-only semantics.
- [ ] **Collection API parity** — grow List, Dict, HashDict, Set, HashSet, tuple,
  slice, and optional/variant APIs demand-first from conformance cases.
- [ ] **HashSet growth and rehashing** — add load-factor growth while preserving
  deterministic behavior and value semantics.
- [ ] **Filesystem and I/O slice** — port representative file/path/stream APIs on
  the Writer and explicit-destroy foundations.
- [ ] **Time, random, and testing slices** — add deterministic testable cores and
  isolate host-dependent behavior behind runtime services.
- [ ] **SIMD semantic completion** — finish dtype/literal conversions, masks,
  reductions, shuffles, and other CPU-visible VM semantics.

### 9. Packaging, Artifacts, And Developer Tooling

- [ ] **Compiled package artifacts** — define and load a versioned `.mojoc`
  representation without making modules first-class runtime values.
- [ ] **Feature and target options** — expose checked CLI/build configuration and
  record it in artifacts and diagnostics.
- [ ] **Debugging metadata and inspection** — provide stack/source diagnostics,
  MIR inspection, and debugger-oriented value rendering.
- [ ] **Testing tools** — provide Mojito-native assertions, expected-error tests,
  and integration with the differential harness.
- [ ] **Distribution reproducibility** — continuously verify that the crates.io
  archive contains everything needed to rebuild, test, document, and reproduce
  conformance results.

### 10. Native Backends And Native-Only Semantics

- [ ] **Cranelift scalar backend** — lower the verified scalar CPU subset and
  validate it differentially against the VM/textual corpus.
- [ ] **Observable CPU layout and ABI rules** — define size, alignment, field
  layout, calling convention, and layout-marker semantics against native output;
  this is intentionally not a VM-parity prerequisite.
- [ ] **Cranelift SIMD lowering** — map completed SIMD semantics to native vectors
  where supported, retaining scalar fallback behavior.
- [ ] **LLVM backend** — share the verified MIR contract and add stronger
  optimization/vectorization coverage.
- [ ] **Stretch backends** — investigate eBPF and MLIR only after Cranelift and
  LLVM are stable; neither is a first-pass parity requirement.

### Explicit Non-Goals For First-Pass Parity

- GPU programming and accelerator memory/execution models
- concurrency, parallelism, atomics, tasks, and distributed execution
- Python interoperability
- MLIR as a required compiler layer or backend
- legacy `fn`, `owned`, and other removed source spellings except for clear
  rejection diagnostics
- escaping closures and the removed `escaping` function effect; first-pass
  closure parity targets Mojo's current non-escaping capture-list model

## Task Lifecycle Policy

`roadmap.md` is the only task list. Do not create a parallel todo file.

- Unfinished work belongs in **Ordered Work** as an unchecked, outcome-oriented
  task. Add detailed design notes elsewhere only when they are needed to make a
  decision or preserve an architectural argument.
- A task is complete only when its implementation, focused positive and negative
  coverage, relevant documentation, and `scripts/check` all agree.
- In the same change that completes a task, remove it from **Ordered Work**. Add
  or update one brief capability entry in **Current State** only when it changes
  the useful high-level picture.
- Record user-visible release history in `CHANGELOG.md`; record lasting design
  invariants in `docs/architecture.md` or the relevant focused document. Delete
  obsolete implementation plans instead of retaining them as completed todos.
- Split or rewrite partially completed tasks so **Ordered Work** states only the
  remaining outcome. Never mark a broad task complete while leaving hidden
  follow-up work inside its description.
- Prefer one checkbox per independently demonstrable semantic outcome. Split a
  task when its parts require different compiler phases, can land without one
  another, have different backend dependencies, or need distinct conformance
  cases. A task may still span phases when those phases are inseparable from one
  end-to-end language guarantee.

## Working Rule

For each promoted task:

1. Start with a self-hosted library or small user-facing acceptance case.
2. Record the current failure with a focused test.
3. Implement the smallest compiler change that makes the program honest.
4. Add positive and negative coverage at the owning compiler phase.
5. Run `scripts/check` before marking the task complete.

Deferred work stays unchecked. Completion follows the lifecycle policy above.
