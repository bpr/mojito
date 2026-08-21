# Backend-Side Monomorphization Plan

## Goal

Complete the next Pliron Stage 5 slice by turning the entry-reachable portion
of verified, drop-elaborated MIR into a backend-private, fully concrete program
before capability checking and Pliron lowering.

The pass must specialize retained generic functions, methods, and structs from
facts already present in MIR. It must not import AST, HIR, or checker state; it
must not mutate the compiler's cached `MirProgram`, canonical `.mir` output, or
VM execution. The VM remains the semantic oracle.

This work is independent of the concurrent narrow-scalar slice. Narrow scalar
and literal types are treated as ordinary concrete leaves. After that slice
lands, reconcile only the concrete-type classifier and shared capability/table
edits; do not duplicate its arithmetic, layout, formatting, or conversion work.

## Scope and Non-Goals

In scope:

- Entry-rooted discovery of concrete function and struct instances.
- Inference of type and compile-time value bindings from concrete receivers,
  runtime argument types, retained type applications, and `MirParamArg` facts.
- Substitution throughout cloned MIR functions and declarations.
- Resolution of abstract method targets against concrete receiver types using
  policy shared with the VM.
- Deterministic instance identity, naming, ordering, recursion handling, and
  diagnostics.
- Integration with the existing reachability, lifecycle-edge, capability,
  lowering, parity, and artifact-invariance tests.

Not in scope:

- Changing source-level generic discovery or compiler specialization.
- Re-checking generic bounds or language semantics below MIR.
- Rewriting the canonical artifact or teaching `mojito exec` a new format.
- Iterator instructions, indirect calls and closure environments, collection
  subscripts, pointer/uninit intrinsics, Variant operations, or vector-width
  SIMD lowering. Those retain their later Stage 5 owners.
- Native scalar representation or operations owned by the preceding narrowing
  slice.
- Making Pliron or LLVM part of the default feature graph.

## Design Contract

### Placement

Add `src/native/mono.rs` as an ungated, backend-independent native pass exposed
from `src/native.rs`. Its public entry point accepts `&MirProgram` and entry
names and returns an owned concrete `MirProgram` plus an entry-name mapping.
The input is borrowed and cloned selectively; callers can prove it is unchanged.

The Pliron compile path becomes:

```text
cached post-drop MirProgram
    -> mir::verify (existing producer/load contract)
    -> native::mono::specialize(entries)
    -> concrete-program validation
    -> Pliron reachability/capability/lowering
```

Do not run ownership analysis or drop elaboration again. The specialization
pass rewrites types and callable identities only; it preserves blocks,
instructions, terminators, cleanup structure, spans, and ownership facts.

### Concrete instance identity

Represent an instance as typed data, not a formatted string:

- Template MIR symbol.
- Ordered concrete type bindings.
- Ordered frozen compile-time value bindings.
- Receiver/struct application when it contributes bindings.
- Origin arguments retained for diagnostics if useful but erased from native
  ABI identity, consistent with existing symbol and layout rules.

Use declaration `ParamDecl` order to canonicalize bindings. Reject a duplicate
parameter solution that disagrees structurally. An instance is admissible only
when every runtime-relevant type and value parameter used by its signature,
locals, registers, places, fields, or call edges resolves concretely.

Name instances deterministically through a new symbol-layer formatter built on
the existing canonical type/value spellings, then pass the resulting MIR name
through `native::mangle` only during LLVM declaration. Do not parse generated
instance strings to recover semantic bindings. Preserve a reverse map for
diagnostics and `NativeModule::mangled_name` entry lookup.

Recommended identity shape:

```text
InstanceKey { template, ordered_args }
InstanceName = symbol::instance_symbol(template, ordered_args)
```

Plain monomorphic declarations keep their existing MIR names. A concrete
instance name must not collide with a source symbol, overload suffix, nested
lifted symbol, or another type/value environment.

### Binding inference

Build one structural unifier for MIR types. It should:

- Bind `Ty::Param` from pattern/actual pairs.
- Bind `CtValue::Param` appearing in `TyArg::Val` positions.
- Recurse through structs, tuples, runtime/variadic packs, pointers,
  references, functions, variants, SIMD, associated applications, and
  dependent indexed types.
- Treat reference origins as semantic metadata that does not create distinct
  native instances.
- Require nominal constructors, arity, conventions, callable environment,
  raising shape, and reference-result shape to agree.
- Resolve dependent indexes after value substitution and reject an unknown or
  out-of-range index contextually.
- Resolve associated types only from concrete declaration facts already
  retained in MIR; if MIR lacks enough information, reject as an unsupported
  monomorphization gap instead of consulting the checker or guessing.

Seed inference from all facts available at a call site, in this order:

1. Explicit concrete receiver type for method calls.
2. Concrete runtime argument register types against declaration parameter
   patterns, after the existing structural call binder maps positional,
   keyword, default, variadic, and erased parameter arguments to slots.
3. Retained `MirParamArg` type/value application facts and concrete value
   arguments that can be proven compile-time constants.
4. The expected destination/register type against the declaration return type
   when it uniquely solves remaining parameters.

Defaults are declaration facts, not new inference policy. Reuse the shared
call-slot binder; do not reproduce keyword/default/variadic binding in the
monomorphizer. Conflicting, incomplete, non-constant, or cyclic solutions must
produce a source-located `PlironErrorKind::Unsupported` diagnostic naming the
template and unresolved parameter.

### Type and MIR substitution

Implement an exhaustive, public-first substitution layer local to
`native::mono`; checker-private substitution helpers are not an acceptable
backend dependency. Substitute:

- Function parameter, return, error, variable, and register types.
- Every `MirPlace`: root type, projection types, and result type, including
  places nested in calls, reference operations, stores, drops, and try regions.
- Instruction-owned type metadata, subscript contracts, iterator metadata,
  callable types, capture metadata, and nested `Try` regions.
- Terminator return/error metadata where present.
- Function declarations: parameter/variadic/error/return types and frozen
  defaults whose types contain parameters.
- Struct declarations and all field types, including transitive concrete
  struct applications.

Erase satisfied compile-time-only binders and call arguments from the cloned
runtime ABI while preserving runtime value parameters and their original
conventions. Keep all vector lengths and aligned parameter arrays consistent:
`n_params`, `param_types`, ownership/deinit/ref flags, declaration parameter
names/types/defaults/required flags, and variable-slot metadata.

After substitution, validate that no runtime-relevant `Ty::Param`, unresolved
`Ty::Assoc`, `Ty::Dependent`, `Ty::SelfType`, `Ty::GenericFunc`, symbolic
`CtValue::Param`, or unresolved variadic pack reaches Pliron. Semantic-only
types that occur solely in erased metadata may be discarded explicitly, never
silently accepted by layout or lowering.

### Worklist and graph rewriting

Replace the current name-only `reachable_set` as the source of truth with an
instance worklist:

1. Seed each requested entry as its monomorphic identity; reject an entry that
   itself requires unsolved parameters.
2. Insert a key into an `in_progress`/`complete` cache before cloning its body,
   allowing direct and mutual recursion to point at the eventual name.
3. Clone and substitute the function and its declaration.
4. Walk every block and nested try region, resolving direct calls,
   constructor calls, method calls, and lifecycle edges to concrete instance
   keys and rewriting their targets.
5. Discover concrete struct declarations from all substituted types and clone
   each once per concrete application.
6. Queue `__init__`, `__copyinit__`, `__moveinit__`, and `__deinit__` edges when
   the existing execution/lowering rules can invoke them.
7. Emit functions and structs in deterministic discovery order with stable
   declaration ordering; hash-map iteration must not affect artifacts.

Keep builtin/intercepted calls unchanged for their later lowering. Keep an
unsupported indirect call rejected by the existing callable slice rather than
inventing an instance through runtime values.

### Shared runtime dispatch policy

Move the pure policy currently split between `Prog::overload_name` and
`Prog::runtime_method_name` into public functions in `src/symbol.rs`. The API
should accept a declaration-existence/candidate view instead of depending on
VM `Prog` or backend state. It must preserve:

- Exact checker-selected concrete targets.
- Abstract receiver-prefix retargeting with the exact overload suffix.
- Borrowed iterator receiver-alternate probing.
- Unique arity fallback only where the VM currently permits it.
- Failure as an unresolved target when no unique declaration exists.

Refactor the VM to call this shared policy without changing behavior, and use
the same API from `native::mono`. Add symbol tests proving the VM and native
views select the same target for plain, overloaded, receiver-overloaded,
abstract-trait, keyword-only, and ambiguous cases.

## Implementation Sequence

### 1. Freeze the handoff and overlap boundary

- Rebase after the narrow-scalar slice lands.
- Review its changes to `lower.rs`, `capability.rs`, Stage 5 notes, parity TSV,
  guard counts, roadmap, changelog, and `commit_msg.txt`.
- Record all newly supported scalar/literal `Ty` variants as concrete leaves in
  monomorphization tests; make no scalar-lowering changes.
- Capture baseline parity counts and current exclusions caused primarily by
  generic/associated/parameterized types.

### 2. Extract symbol resolution

- Add declaration-view and shared resolution APIs to `src/symbol.rs`, keeping
  public items above private helpers per repository ordering rules.
- Replace VM-local overload/method policy with calls to the shared API.
- Add focused `tests/symbol_test.rs` coverage and run VM/symbol tests to prove
  the refactor is behavior-neutral before adding monomorphization.

### 3. Add native instance and substitution infrastructure

- Create `src/native/mono.rs` with public result/error/entry APIs followed by
  private instance keys, environments, substitution, and walkers.
- Add structural type/value inference and exhaustive substitution tests over
  every `Ty` constructor and nested `MirPlace`/`Try` location.
- Add deterministic instance naming to `src/symbol.rs` with collision and
  origin-erasure tests.

### 4. Specialize function and struct graphs

- Implement the entry-rooted worklist, recursion cache, declaration cloning,
  call rewriting, concrete struct cloning, and lifecycle discovery.
- Preserve source spans and original template names in diagnostics.
- Add synthetic MIR unit tests for generic identity, multiple instances,
  nested generic types, value parameters, recursive and mutually recursive
  functions, generic methods, and conflicting/incomplete inference.
- Run canonical disassembly before and after specialization and assert the
  input artifact bytes are unchanged.

### 5. Integrate Pliron

- Invoke specialization at the start of `backend::pliron::compile`, before
  reachability and declaration/layout construction.
- Use the returned concrete entry map for executable wrapper and JIT lookup so
  public entry names remain stable.
- Make existing reachability operate on the concrete program or fold its
  residual lifecycle/builtin logic into the instance worklist; avoid two
  divergent call-graph policies.
- Run capability checks and lowering solely against the concrete program.
- Ensure errors render the source template/call-site location rather than only
  an opaque instance name.

### 6. Land vertical fixtures and ratchet gates

Add small fixtures that isolate this slice before relying on large stdlib
clusters:

- A generic identity/function inferred at two concrete types.
- A generic function with a frozen integer or `DType` value parameter.
- A generic struct with field layout, constructor, method, copy, and drop.
- A generic method whose receiver and argument jointly infer parameters.
- An associated-type/parameterized-associated-type case supported by retained
  MIR facts.
- Recursive and mutually recursive concrete generic calls.
- A trait-selected method whose abstract symbol retargets to a concrete
  overload.
- Negative cases for an unresolved parameter, conflicting solutions,
  non-constant required value, missing associated-type fact, and instance
  recursion that continually changes arguments.

For each positive fixture, compare VM/native stdout bytes and lifecycle events
at `O0` and `O1`, including sanitized executable runs where aggregates allocate
or drop. Regenerate `conformance/pliron-parity.tsv` and
`conformance/pliron-capability.tsv`, review every status transition, increase
the differential floor, and lower the exclusion ceiling to the achieved count.
Do not predeclare which broader fixtures become executable: many will advance
only to the next iterator, collection, or callable blocker.

### 7. Documentation and completion

- Add the slice design, inference rules, instance identity, preserved
  divergences, achieved fixture clusters, and final parity counts to
  `docs/notes/pliron-stage5.md`.
- Update the native backend rows in `docs/features.md`.
- Update `docs/architecture.md` and `docs/symbol-map.md` for the new native pass
  and shared symbol-resolution owner.
- Add the user-visible capability change to `CHANGELOG.md`.
- Remove the completed backend-side monomorphization checkbox from
  `docs/roadmap.md`; checked tasks do not remain in the roadmap.
- Replace `commit_msg.txt` with the completed slice's commit message.

## Tests and Gates

During implementation:

- Focused `native::mono` and symbol unit tests.
- Focused VM tests after dispatch-policy extraction.
- Focused Pliron compile/JIT tests for each new instance form.
- Individual new corpus fixtures through VM and Pliron.
- Artifact byte-invariance and deterministic-output tests.

At completion:

1. `cargo fmt --check`
2. Focused symbol, MIR, native-monomorphization, VM, and Pliron tests.
3. Regenerated capability/parity manifests with reviewed ratchet changes.
4. `cargo nextest run --profile quick`
5. `cargo clippy --all-targets --all-features -- -D warnings`
6. `git diff --check`
7. `env RUSTC_WRAPPER= scripts/check`
8. `scripts/check-pliron`

The LLVM lane is expensive; use focused tests while iterating and schedule the
full Pliron gate only after the slice is otherwise stable. Do not skip failures
owned by this change. Clearly identify unrelated pre-existing failures without
changing their tests or weakening guards.

## Acceptance Criteria

- Pliron compiles concrete instances of retained generic functions, methods,
  and structs reachable from requested entries.
- Multiple concrete applications coexist with deterministic, collision-free
  native symbols and correct layouts.
- Direct, constructor, lifecycle, and abstract-method edges target the exact
  concrete instances selected by shared VM/native policy.
- No runtime-relevant symbolic type or value reaches layout or LLVM lowering;
  incomplete inference fails contextually at the responsible call site.
- Recursive instance graphs terminate by key reuse; expanding polymorphic
  recursion rejects with a deterministic diagnostic and bounded work.
- Canonical `.mir` bytes and VM behavior are unchanged.
- Default builds still resolve no Pliron or LLVM dependency.
- VM/native output, error category, and lifecycle order agree for every newly
  admitted fixture at `O0` and `O1`.
- Capability and parity guards ratchet forward, documentation reflects the
  actual supported surface, the roadmap entry is removed, and
  `commit_msg.txt` is current.

## Principal Risks

- MIR may not retain enough declaration information to resolve every
  associated type or value argument. Treat this as an explicit, narrow MIR
  metadata gap; do not reach upward into checker state.
- Reusing checker-private substitution would violate backend isolation and can
  silently import semantic policy. Keep the native substitution exhaustive and
  mechanical.
- A name-only reachability pass beside an instance worklist can omit lifecycle
  instances or compile the wrong generic body. Establish one graph owner.
- Erasing origins from instance identity must not erase referent mutability or
  ABI-relevant reference shape.
- Drop-elaborated MIR contains types and places in nested cleanup/try regions;
  a shallow walker will produce late layout failures or incorrect destruction.
- Polymorphic recursion can create an unbounded sequence of distinct keys.
  Detect it with an explicit instance/depth budget and report the causal edge.
- The narrowing slice modifies shared Pliron files and manifest counts. Rebase
  first and reconcile those edits rather than overwriting them.
