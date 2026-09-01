# Mojo Dev-Branch Tracking

Mojito tracks the latest dev branch of Mojo.

The comparison target rolls with upstream `main`; reproducible audits name an
exact upstream commit, Mojo build, audit date, and Mojito baseline. The
differential runner must likewise report its actual `mojo --version`. Advancing
the audit boundary records work to do—it does not claim that Mojito already
implements every change at that boundary.

## Current Audit — `a79fbdf59f2` (2026-08-26)

| Role | Immutable revision |
|---|---|
| Audited Mojo dev head | [`a79fbdf59f224d7a2242f2d1c29cf55d93489a91`](https://github.com/modular/modular/commit/a79fbdf59f224d7a2242f2d1c29cf55d93489a91), whose lockfiles pin `Mojo 1.1.0.dev2026082605` |
| Previous upstream audit boundary | [`ae386d1b20434e126aea8a32a2b625ed1343eaf5`](https://github.com/modular/modular/commit/ae386d1b20434e126aea8a32a2b625ed1343eaf5), `Mojo 1.1.0.dev2026080805` |
| Mojito baseline audited | `c0399a6` (working tree at re-pin, 2026-08-26) |
| Source delta | [Upstream comparison `ae386d1b204…a79fbdf59f2`](https://github.com/modular/modular/compare/ae386d1b20434e126aea8a32a2b625ed1343eaf5...a79fbdf59f224d7a2242f2d1c29cf55d93489a91) |

The evidence for this window is the accumulated post-v1.0 nightly changelog at
the pinned hash
([`nightly-changelog.md`](https://github.com/modular/modular/blob/a79fbdf59f224d7a2242f2d1c29cf55d93489a91/mojo/docs/nightly-changelog.md));
a local pinned worktree lives at `~/src/mojo/repos/modular-a79fbdf5` and the
differential Pixi environment at `~/src/mojo/envs/mojo-2026082605`. Note the
window's own churn: `Array` concatenation was introduced as `__add__` and
renamed to `concat` before the pin, so only the pinned surface counts.

This window completes the removal of the v1.0-cycle deprecated spellings —
several compatibility bridges Mojito deliberately retained now expire under the
match-or-subset rule — makes the `read` argument convention a hard error, and
adds two genuine language features: contextually inferred leading-dot member
references, and trailing `where` clauses on `thin` function types with a new
directional rule for binding constrained functions. Library movement inside the
proof subset is modest (`List` element bound loosened to `AnyType`, small
`Array`/`String` API growth); most remaining entries are outside the declared
subset and are recorded, not implemented.

**Close-out record (2026-08-26).** The full differential conformance run
passes 199/199 `conformance/cases.tsv` cases against the exact audited build:
`mojo --version` reports `Mojo 1.1.0.dev2026082605 (dd957314)` from the
`~/src/mojo/envs/mojo-2026082605` Pixi environment (pinning the conda nightly
that `a79fbdf59f224d7a2242f2d1c29cf55d93489a91`'s lockfiles pin); the Mojito
side is commit `c0399a6` plus this uncommitted pass. The full Rust gate is
green in the same state (`scripts/check`: 3371/3371 workspace tests, Clippy
with warnings denied, format and diff checks). All four changeset sections
completed in one pass; sixteen new fixtures joined `cases.tsv` (five as
documented one-sided rows) and the `SIMDSize`/`TypeList.size` probes were
promoted to rejection claims. One upstream regression surfaced during the
sweep: the head rejects its own `Tuple.consume_elements` docstring example
(`len` on the dependent element type fails overload resolution under the
Array literal retarget, canonical `Ts` and deprecated `element_types`
spellings alike), so `tuple-consume-elements` is a documented `mojito-only`
row until the head recovers — re-probe at the next re-pin.

## Prioritized Changeset

The order below is the recommended implementation order. Compatibility aliases
may be retained deliberately only while upstream itself keeps the deprecated
form; every bridge listed in section 0 expired upstream inside this window.

### 0. Remove expired compatibility spellings and the `read` convention

Status 2026-08-26: DONE — `read` rejects at every convention position with
upstream's migration diagnostic (MIR text emits/accepts only `imm`;
`ArgConvention::Imm` internally); `SIMDSize`/`TypeList.size` reject; the
origin vocabulary is unified on the surviving set with targeted diagnostics
for removed spellings; `MaybeUninit` renamed with the head's triviality-gated
conformance header; `@__parameter` accepted (`@parameter` stays a warn-era
bridge; the diverging capture model is probed by
`conformance/probes/parameter_closure_capture_model.mojo`).

Upstream made the `read` argument convention a hard error and completed the
removal of the v1.0-cycle deprecated APIs. Mojito's corresponding bridges must
flip to rejection in the same pass, with contextual diagnostics naming the
surviving spelling:

- `read` → `imm` in argument conventions and closure capture lists
  (`src/parser.rs` convention words). MIR text is Mojito-owned: emit and accept
  only `imm`, and rename the internal `ArgConvention::Read` variant to `Imm`.
- `SIMDSize` (checker/comptime alias arms) and `TypeList.size` → rejection;
  `SIMDLength`/`length` are already the canonical spellings. Upstream also
  removed `SIMD.size` and `Array.size`, which Mojito never exposed.
- Origin aliases: upstream removed `ImmutOrigin`, `ImmutUnsafeAnyOrigin`,
  `StaticConstantOrigin`, `ExternalOrigin`, `MutExternalOrigin`,
  `ImmutUntrackedOrigin`, and `ImmutExternalOrigin`. Mojito's `ref[...]`
  resolver already speaks the surviving vocabulary, but the Pointer
  type-argument resolver and the `std.origin` export table still accept and
  export the removed `Immut*`/`StaticConstantOrigin` spellings while lacking
  surviving `ImmOrigin`/`ImmUnsafeAnyOrigin`. Unify both resolvers, the type
  display, and the export table on exactly the surviving set.
- `UnsafeMaybeUninit` → `MaybeUninit` (struct, compiler-private references,
  and the checker's triviality-gated conformance table). Verify at the pinned
  head whether the old name retains a deprecated alias; mirror its state.
- The `@parameter` decorator on parametric closures is renamed `@__parameter`.
  Accept the new spelling where the checker consumes the decorator; probe the
  old spelling's fate at the head and mirror it.

### 1. Constrain function types with trailing `where` clauses

Status 2026-08-26: DONE on inline `def[...]` bounds (clauses lower onto decl
constraints, alpha-renamed identity, directional binding, call-site
evaluation); binder-less clauses and the comptime alias spelling are
recorded mojo-only rows.

A `thin` function type can now carry trailing `where` clauses constraining the
parameters it declares, and binding a constrained function to a function type
that declares no matching `where` clause is an error (the unconstrained-into-
constrained direction stays allowed and free). The clause binds to the
innermost function type; a declaration-level `where` following a function-type
result requires the result parenthesized upstream. Mojito already parses and
checks parametric function types (`def[w: Int](Int) thin -> None`) end-to-end,
and explicit specializations of callable parameters already evaluate decl
constraints, so this lands as: `where` clauses on `Type::Func` lowered onto the
anonymous callable's parameter declarations, plus a directional
constraint-subset rule (with binder alpha-renaming) in the callable-bound
acceptance check. The `comptime Kernel = def[...] ... where (...)` alias
spelling stays blocked by the pre-existing function-type comptime-alias gap and
is recorded as a divergence, not implemented here.

### 2. Contextually inferred member references

Status 2026-08-26: DONE for the first slice (static-method calls with postfix
chains in expected-type positions, via the `$contextual` sentinel resolved by
the checker and substituted in HIR); bare value members, parametric statics,
non-struct and generic expected types are recorded gaps.

A leading-dot form such as `.red` or `.hsb_to_rgb(120, 100, 50)` resolves
against the expected type of the expression; without a contextual type it is an
error. Upstream's surface covers bare `comptime` value members, static methods,
parametric static methods, parentheses, attribute chains, and typed collection
literals. Mojito's first slice: leading-dot **static method calls** (including
postfix chains) in every position that already flows an expected type —
annotated bindings and assignments, call arguments, return positions, and
typed collection-literal elements. Bare `.red` value members require struct
`comptime` associated value members, which Mojito does not have (pre-existing
subset gap); parametric statics and generic expected types are likewise
recorded as subset gaps rather than half-implemented.

### 3. Align in-subset library surfaces

Status 2026-08-26: DONE except recorded gaps — `List[T: AnyType]` with
per-API `Movable` gates; Array lexicographic `Comparable` (Defaultable and
`concat`/`repeat` are mojo-only rows: generic `Self.T()` construction and
dependent result lengths); String `()`/`capacity_bytes`/`reserve_bytes`;
`MaybeUninit.write()`. StringDict `Writable`/`StringSpan` lookup stay
demand-first.

- `List`'s element type is now bounded by `AnyType` instead of `Movable`;
  individual APIs state their own `Movable`/`Copyable` requirements. Mirror the
  pinned head's `List` declaration shape onto the bundled List.
- `Array` grows `Defaultable` (when `T` is), lexicographic `Comparable` (when
  `T` is), consuming `concat` (`Movable`), and consuming `repeat` (`Copyable`).
- `String` stabilizations: `__init__(out self)` (already present),
  `__init__(out self, *, capacity_bytes: Int)`, and
  `reserve_bytes(mut self, new_capacity_bytes: Int, /)` — capacity is
  host-managed in the VM String bridges, so these may be semantic no-ops with
  the correct signatures.
- `Pointer`/`MaybeUninit` `write()` (safe, gated on trivially-deinitable
  pointees) and the `Pointer.unsafe_write(def() -> T)` closure overload —
  implement if the unified-closure argument shape is expressible on bundled
  methods; otherwise record the gap.
- `StringDict` conforms to `Writable` when its value type does, and
  `StringDict.__getitem__` accepts a `StringSpan` — demand-first growth on the
  existing StringDict.

## Confirmed Alignment And Audit-Only Work

Do not create duplicate implementation tasks for changes the stable baseline
already covers:

- Ordinary initialized values are movable by default. Conditional `Movable`
  opt-out (`Movable where False`) is now effective at transfer, `var`
  parameter/receiver, and capture sites (2026-08-09).
- Fresh local names already require explicit `var`, which is stricter than
  upstream's current warning. Package members already require explicit imports.
- Callable structs already need nominal callable-trait conformance;
  shape-compatible `__call__` alone is insufficient.
- Candidate implicit conversion already filters to `@implicit` constructors
  before selection; upstream's latest change is a compile-time performance win,
  not a semantic gap for Mojito.
- Custom method receiver types are already rejected; Mojito's parser admits
  only the ordinary `Self` receiver forms.
- Int and `Scalar[DType.int]`, current reflection field handles, `StringDict`
  kwargs storage/forwarding, origin-parameterized borrowed `Iterable`, and
  monomorphic `IterableOwned.IteratorOwnedType` are present.
- The two-root namespace-directory example — `foo.bar` and `foo.baz` imported
  from distinct `-I` roots sharing the `foo` prefix — is pinned permanently by
  `tests/module_test.rs` `two_roots_share_a_namespace_directory_prefix` and was
  hand-verified against the audited build (`mojo run -I root_a -I root_b`
  prints `3`, 2026-08-15). Source-package precedence and package
  `__init__.mojo` boundaries are unchanged.
- `range(..., step=0)` is empty in nominal runtime iteration, direct compile-time
  unrolling, and VM-backed CTFE.
- `Error` is treated as implicitly copyable by the current checked type facts,
  and the caught-error `raise e` shape is pinned by the `raise-caught-error`
  differential case (both compilers relay and print `caught boom`; Mojito's
  `print(e)` rendering was aligned to the bare message).
- Unhandled errors already print `unhandled error: …` to **stderr** on both
  backends (VM CLI via `eprintln!`, native via `mjrt_unhandled_error`),
  matching upstream's stdout→stderr change in this window.
- Upstream's range fixes are already covered or moot: `step=0` is empty
  everywhere; Mojito's `range()` overloads are Int-only (small-scalar strided
  wraparound cannot arise) and `reversed()` on ranges is a recorded
  out-of-subset limitation, so the reversed-near-limit and
  `reversed(reversed(...))` changes have no Mojito surface.
- `ceildiv` already derives the unsigned ceiling from floor division and
  remainder (no near-max overflow), and scalar float hashing folds the sign
  of zero before the value reaches the hasher.
- Parametric `raises` with any primary-expression type (`raises Self.Assoc`)
  already parses through `parse_type` in both signatures and function types.
- The bulk of this window's Removed entries name APIs Mojito never exposed —
  `InlineArray`, `ImplicitlyDestructible`/`ImplicitlyDeletable`, `ImmutSpan`,
  `as_immutable()`/`get_immutable()`, `String.as_string_slice()` and
  `String.set_byte_length()`, the pre-unification pointer aliases
  (`MutUnsafePointer` family), the raw memory helpers (`memcpy`/`memset`/
  `memcmp`/`uninit_*_n`/`destroy_n`), the `destroy_pointee`/`init_pointee_*`
  family and `Pointer.type`, `steal_data`/`OwnedPointer.take`/`Variant.take`
  (Mojito shipped `into_inner`/`unwrap` vocabulary from day one),
  `ConditionalType` + `std.utils.type_functions`, `trait_downcast`, the
  coroutine prelude types, and `.mojopkg` loading. Audit-only: no code change.
- Bridges that survive this window because upstream still carries the
  deprecated form: `UnsafePointer` (alias of `Pointer`), Tuple's
  `element_types` alias for `Ts`, and `@parameter if`/`@parameter for`
  (which Mojito never parsed — a subset stance, not a bridge). Newly
  deprecated upstream without Mojito surface: `Pointer.mut_cast`,
  `unsafe_ptr()` on always-valid holders (→ `ptr()`), and the
  `is_trivially_*()` function spellings Mojito already rejects in favor of
  the `IsTrivially*` predicates.

## Monitored Or Deferred Movement

These changes should remain visible without displacing the CPU language work:

- `Atomic[T]` now takes a value type instead of a `DType`. Atomics and
  concurrency remain outside first-pass parity.
- Experimental FP6 encodings are packed storage formats without general
  arithmetic or conversions. Revisit them after ordinary scalar/SIMD parity.
- `external_call(..., num_fixed_args=N)`, platform C ABI forwarding, and dynamic
  library lifetime improvements are CPU-relevant but depend on a real FFI/native
  boundary. Record them with native backend and ABI work.
- `size_of` now includes alignment padding. Implement it with observable CPU
  layout/ABI semantics, not by inventing VM-only sizes.
- Address-space expansion, GPU/MAX package moves, GPU APIs, and Python
  interoperability remain outside the first-pass subset.
- `__generator_type` and coroutine internals are useful signals for possible
  future generators/coroutines, but are not yet a public parity gate. Keep HIR
  and MIR extension points general rather than implementing this internal
  spelling. (This window privatized the coroutine prelude types and the
  `std.runtime.asyncrt` task API, reinforcing the deferral.)
- The `Bench`/`Bencher` family finished migrating from compile-time parameter
  closures to unified runtime-argument closures, and the parametric
  `benchmark.run[func]()` overloads were removed. Benchmarking is outside the
  subset; the unified-closure argument shape is the part to watch.
- `CompilationTarget` gained `is_arm()`/RISC-V predicates and re-based
  `is_x86()` on the triple; with the redundant `Int` overload removals in
  `std.bit`/`std.math` (Int is `Scalar[DType.int]`, the SIMD overloads absorb
  them), these are native-target and stdlib-breadth concerns, not language
  parity.
- `strip`/`lstrip`/`rstrip` now take `ImmStringSpan` chars; Mojito has no
  strip family and no `Imm*Span` aliases (recorded divergence on the Span
  origin-slot row), so this lands with future String result-API growth.
- `atol()` now raises across the full out-of-`Int`-range and
  whitespace-only surface; Mojito has no `atol`/`Int(String)` parsing yet.
- `@align(N)` beyond natural alignment is now honored for every value
  including array/List elements — relevant to native-backend layout work,
  not the VM.

## Open Questions And Probes

Open questions, ambiguities, and known mismatches each get a minimal program
in [`conformance/probes/`](../conformance/probes/) runnable against both
compilers, with the question and the follow-up actions documented in the
file's header. Run the probes (and the re-probe list in that directory's
README) against the exact audited build at every re-pin.

## Review Policy

Before closing a language-parity milestone:

1. Fetch upstream `main` and record its full commit, commit date, and the Mojo
   version pinned by its current lockfiles. The rendered nightly page is a
   discovery aid, not the reproducible audit identity.
2. Diff from the preceding audit hash. Review every intervening language,
   library, removal, and fixed-behavior entry across both the active nightly
   changelog and any release document created by a changelog cutover.
3. Inspect the changed manual and standard-library declarations. Renames,
   conditional conformances, and compiler/stdlib handoffs are not always
   repeated in the condensed changelog.
4. Update `conformance/parity.tsv` first. A newly introduced mismatch becomes a
   documented subset/divergence until implementation and differential evidence
   justify `implemented`/`match`.
5. Update `roadmap.md`, grammar and architecture documentation, fixtures, and
   bundled Mojo sources affected by removed or renamed syntax.
6. Run differential conformance with a Pixi environment containing the exact
   audited build. Retain `mojo --version`, the upstream hash, and the Mojito hash
   with the results.

## Historical Audit — `ae386d1b204` (2026-08-08)

| Role | Immutable revision |
|---|---|
| Audited Mojo dev head | [`ae386d1b20434e126aea8a32a2b625ed1343eaf5`](https://github.com/modular/modular/commit/ae386d1b20434e126aea8a32a2b625ed1343eaf5), whose lockfiles pin `Mojo 1.1.0.dev2026080805` |
| Stable Mojito implementation audited | `9118482492edc70b8d3b1d929900f8505ac10a80` from `/home/bpr/src/rust/projects/stable/mojito` |
| Source delta | [Upstream comparison `609afcd073…ae386d1b204`](https://github.com/modular/modular/compare/609afcd0735054872ba028f27531b7abec947ddc...ae386d1b20434e126aea8a32a2b625ed1343eaf5) |

Upstream cut the accumulated nightly record over to the staged v1.0.0 release
notes on August 6, so this audit read the release document and the short
post-cutover changelog together. The pass ran as nine ordered changeset
sections, all complete before the 2026-08-26 re-pin:

- **§0 lifecycle foundation** — canonical `Deinitable`/`__deinit__`, effective
  conditional `Movable`, `IsTrivially*` predicates (hard-renamed by the
  post-pin spot alignment to upstream `22b5036987`), `std.traits`/`std.origin`
  module homes.
- **§1 small gaps** — unified `{...}` rejection, bare `move:`, competing
  `__setitem__` pair, `def(...)` fields/elements gating, captured-Origin
  specialization limits, `var **kwargs`, duplicate/self imports, `range`
  step=0, `where (cond, "msg")`, reserved words; then repeated trailing
  `where` clauses and generic top-level comptime aliases.
- **§2 lambdas** — hidden-def-at-parse lambda expressions (committed
  `dec1918`).
- **§3 Array** — nominal `Array[T, length]` with uncontextualized list
  displays retargeted to Array.
- **§4 pointer/allocation model** — `Pointer` naming, `ptr[]`, `unsafe_*`
  vocabulary, layout-based `alloc`/`Allocation`, linear `std.memory`, and
  `UnsafeMaybeUninit` inline-uninit storage.
- **§5 views and strict bounds** — `Span`/`StringSpan` borrowed views, strict
  contiguous bounds through the `os.abort` trap, keyword slices, grapheme
  iteration.
- **§6 owning containers** — `AnyType` Optional/Variant with `init_with=`,
  `deinit_with` family, displacement `insert`, `OwnedPointer` with
  `into_inner`, owned-iteration bounds tightened to `Movable & Deinitable`.
- **§7 subtree origins** — terminal `origin._subtree` with conservative
  invalidation semantics and the `@implicit` `ref [origin]` conversion channel
  (implicit `List`→`Span`).
- **§8 scalar/SIMD/range/vocabulary** — `SIMDLength` spelling, scalar-range
  checker intercept, `TypeList` marker values, predicate aliases.

**Close-out record (2026-08-15).** The full differential conformance run
passed 176/176 `conformance/cases.tsv` cases against the exact audited build:
`mojo --version` reported `Mojo 1.1.0.dev2026080805 (7ade05af)` from a Pixi
environment pinning the `mojo ==1.1.0.dev2026080805` conda nightly; the Mojito
side was commit `a6bfe27` plus the close-out change. The close-out resolved
all twenty open-question probes (answers live in `cases.tsv` claims and
parity-row notes), re-confirmed the five-item re-probe table, fixed a
discovery-path regression for dependent callable bounds (`F: def(T) -> T`
residual signatures), aligned the origin-alias vocabulary (`ImmStaticOrigin`,
`ImmUntrackedOrigin`, `MutUnsafeAnyOrigin`), and reclassified the July-era
case rows whose behavior the head changed (implicit `def`-scope declaration,
the Dict projected-value refresh, raising explicit-destroy rollback, and
untracked-ref conversions became documented one-sided rows).

## Historical Stable-Baseline Audit — `609afcd0735` (2026-07-25)

The remainder of this section records the evidence used to establish the stable
Mojito baseline. References to “the pinned nightly” below mean this historical
hash, not the rolling dev-branch target.

The focused cases pass against `Mojo 1.0.0b3.dev2026072505`: heterogeneous
pack-bound rejection, homogeneous tuple-valued variadics, private runtime-pack
destruction, non-copyable indexed Tuple-transfer rejection, and
`Tuple.consume_elements` all match. Public `List`, `Set`, `Dict`, `Range`, and
`Tuple` values are nominal self-hosted structs; focused construction and
protocol dispatch plus concrete Range iteration have differential `run`
coverage. Concrete borrowed List iteration also matches: the iterator retains
the live source, so replacing `values[1]` after yielding the first element makes
the next yield observe the replacement.
The `Copyable` iterator-result refinement also matches: a concrete
`__next__ -> ref[o] T` may implement an abstract value-returning `__next__ -> T`,
and an abstract call observes a lifecycle copy of the referent. Mojito rejects
the reverse direction, mismatched referents, and non-`Copyable` elements, and
retains the adaptation explicitly through checked HIR and verified MIR.
The bundled borrowed contract now matches the nightly's shape: `Iterable`
declares the origin- and mutability-parameterized `IteratorType[...]` and
returns `Self.IteratorType[origin_of(self)]`, and the bundled List/Set iterator
borrows its source through a parametric-mut struct origin and yields element
references — `for ref` write-through runs the ordinary protocol with no
List-only desugaring. Remaining subset boundaries: a `ref` loop target over an abstract generic
`Iterable` bound is rejected (the abstract `__next__` contract yields values),
and `keys`/`values`/`items` borrowing views yield read-only references where
upstream's `values()` follows the mapping's mutability.
Mapping iterators now borrow their entries and yield key references; mapping
mutation during iteration is a lazily rejected error, matching Mojo's
documented programmer-error contract with a static/runtime diagnostic.
The interior-origin run/rejection cases also match: overlapping List element
references and direct element writes remain valid, while a structural mutation
or a user-declared interior-return contract makes an older generation stale.
Dictionary lookup likewise defines a fresh `"value"` owned-interior generation:
a later lookup invalidates an earlier value reference and ordinary field
projections below that value without invalidating the separate key-iteration
generation. A retained `ref` subscript argument is also revalidated after later
index arguments have run, so an intervening structural mutation makes the call
reject while a copied argument remains a source-ordered value read. A field
projected from a reference-returning Dict lookup executes that lookup once and
remains a valid direct reference or `mut`/`ref` actual in both compilers. Focused
subscript contracts match receiver, index/bound, then right-hand-side assignment
evaluation order; one-evaluation reference results passed to `mut`, and abstract
trait-bound indexing. Augmented subscript evaluation has two pinned paths. A
value-returning getter orders receiver and raw indices, RHS, getter-specific
argument conversions, `__getitem__`, the operator, setter-specific conversions,
then `__setitem__`; an index mutated by the getter is reloaded from its retained
place before the setter without reevaluating its source expression. A mutable-
reference getter instead establishes the lvalue before the RHS and writes the
result through that handle without calling `__setitem__`. A sole keyword-only
setter value also matches, including its selected `@implicit` conversion of the
computed result. Ordinary index operands may use a selected user
`@implicit` conversion, but a compiler-synthesized slice literal only widens
within the descriptor family and does not invoke an arbitrary user constructor;
the pinned compiler and Mojito reject that attempted wrapping before lowering.
For an augmented assignment on a user-defined value, the pinned compiler calls
the dedicated `__iadd__`/corresponding in-place dunder rather than the ordinary
binary method. Mojito matches this for variable, projected-field, and
nominal-subscript-element targets (both the value-getter and mutable-reference-getter
subscript paths), so `current-inplace-dunders` and `inplace-subscript-dunders` run
in both.
The pinned
nightly currently rejects a competing positional-only/keyword-only
`__setitem__` overload pair that Mojito resolves from the right-hand-side type;
that focused form is recorded as a Mojito extension pending upstream
clarification.

The pointer allocation surface has moved independently of those projection
semantics. This nightly accepts free `alloc[T](...)` and empty `pointer[]`
dereference and rejects the old static `UnsafePointer[T].alloc[_aligned]`
spelling used by Mojito. The manifest therefore keeps a Mojo-only current API
case and Mojito-only legacy/projection cases until the bundled memory API is
migrated.
Mojito also matches the nightly rule that bare `ref` is parametrically mutable:
an unspecialized body cannot write through it without an explicitly mutable
origin. Scope-stable pack cases now match as well: ordinary values shadowing a
runtime pack are not spread, a nested ordinary type parameter is not replaced
by an outer type pack, empty packs remain valid, and leaving a block or loop
restores the outer pack binding.
Nested generic definitions now specialize at their lexical declaration with
scope-qualified identities. Independent empty/nonempty instances, sibling pack
forwarding, captured outer packs, defaults, keywords, and method captures run;
the shared call matcher infers only the target's variadic overflow. Recursive
lifting now follows the complete lexical path at arbitrary depth and preserves
capture forwarding, effects, argument markers, reference returns, and named
results. One exact whole-pack collector may follow the fully supplied fixed
positional prefix and precede a keyword/default tail; it moves as one value and
therefore preserves linear elements. Mojito rejects multiple spreads and an
explicit positional argument after the spread, matching the pinned compiler
instead of treating pack concatenation as a future parity target.

The pinned closure spelling is an explicit capture list after declaration
effects: `{imm x}`, `{mut x}`, `{ref x}`, `{var x}`, move/default variants, and
`{}`. `var` snapshots when the nested declaration executes; reference captures
remain tied to live outer storage without holding a declaration-to-call loan, so
an `imm` capture sees intervening outer updates. Mojito retains the removed
`unified {...}` position only as a documented acceptance divergence. Callable
probes also confirm `function[origin_of(place)]` specialization and nominal
`def[...] (ref[origin] T) -> ref[origin] T` contracts: parameter conventions,
reference-result origins, and raising effects survive indirect dispatch. The
checker now retains unqualified, `thin`, `capturing[_]`, and
`capturing[origins]` environments as distinct callable-type facts. A supplied
`@parameter` closure resolves the `OriginSet` contract to concrete read/write
owner effects, which cross indirect/downward calls into loan analysis. Generic
callable bounds (`F: def(T) -> T`) are dependent checked constraints and calls
through `F` execute for monomorphic functions and nominal callable structs;
contracts with their own `def[...]` binders accept alpha-equivalent generic
functions and preserve contract-side invocation defaults. Explicitly qualified
callable-value parameters may default to a selected function, an earlier
callable parameter, or a compile-time conditional of those plans. These defaults
stay symbolic and are reified in declaration order rather than becoming
closure-valued compile-time data. Explicit Origin arguments participate in
direct overload and generic selection; they also compose with explicit ordinary
generic arguments in a function value, and an expected `def(...)` type selects
one specialized overload value. Variadic compile-time type/value packs can
appear before or after Origin parameters; source-layout binding preserves a
named Origin suffix. A scalar-controlled `comptime` branch can specialize around
a retained captured callback, leaving that callback on the residual signature
and rewritten call. `CtValue` still has no function or closure variant, so this
is not general callable CTFE. The pinned compiler likewise rejects an
uncontextualized overload value. Mojito additionally accepts an unqualified
stateful downward funarg and materializes a captured non-overloaded nested Origin
specialization; the pinned compiler rejects those forms, so they remain recorded
extensions.
Reference-returning calls now match across the structured `try` executor as well:
the direct and combined direct/method/projected/keyword/raising cases preserve
caller storage and agree with the pinned compiler.

A complete differential run after the earlier unified reference-call slice
reported 89 passing and 13 failing pre-existing `run` cases. The failures spanned closure
fixtures, literal families, lifecycle fixtures, core protocols, context
managers, reference iteration, interpolation, advanced origins, pointer
provenance, and keyword forwarding; one closure invocation trapped the nightly
compiler. The callable slice has since migrated the legacy closure and thin
function-value fixtures and added focused current-nightly cases, so those counts
are historical rather than a current total. The remaining failures need
fixture-versus-compiler triage and must not be read as regressions in pack
identity, Tuple destruction, or interior origins. Until a fresh complete run is
recorded, a passing `scripts/check` establishes the local compiler contract,
while the full differential harness remains intentionally reported as non-green.

## Historical `609afcd0735` Capability Ledger

The following table is preserved from the 2026-07-25 audit. “Current nightly”
inside this historical ledger means `609afcd0735`, not the rolling dev head.

| Area | Current nightly | Mojito consequence |
|---|---|---|
| Interior origins | Collections can bind element references to internal origins that are invalidated by structural mutation or reallocation without borrowing the entire owner forever. | Implemented as explicit checked origin/invalidation facts, grouped MIR generations (including union-valued returns), and forward CFG analysis with distinct normal/raising/return-or-escape channels for nominal collection methods, Variant operations, and user return contracts. Self-hosted collections declare and reuse this checked boundary without native collection inference. |
| Literal initializer inference | Partially specified collection annotations such as `List[_]` or bare `List` can infer element types from a literal initializer. | Implemented contextually before displays lower to the selected nominal collection constructors and methods; no VM-native collection shortcut remains. |
| Origin capability cast | `Origin[mut=False].cast_from[origin]` (and `ImmutableOrigin.cast_from`) downcasts an origin's capability to read-only in reference signatures; upcasts are unsafe and spelled through `MutableOrigin.cast_from` only in unsafe code. | Accept the keyword `Origin[mut=False].cast_from[...]` downcast in `ref[...]` result signatures, pinning the yielded capability to `Immutable` independent of the origin parameter's parametric `mut=`; reject the `mut=True` upgrade direction. |
| Immutable convention | `imm` is the preferred spelling for the argument and closure-capture convention. `read` remains a synonym but is headed for deprecation. | Accept and emit `imm`; retain `read` only as a compatibility spelling. The linked commit `323dfd974e2f6fc83ce82a476d8fa5d51529eadf` documents this transition. |
| Linear-value trait | `ImplicitlyDestructible` was renamed to `ImplicitlyDeletable`; `is_trivially_destructible` likewise became `is_trivially_deletable`. | Reverse Mojito's previous vocabulary migration and update constraints, diagnostics, tests, and bundled sources. |
| String indexing | Current Mojo's String exposes byte/codepoint access through iterator and method APIs; the exact keyword-subscript spelling is not settled stable syntax. `Codepoint` constructs from `UInt32` scalars (`from_u32`, `to_u32`) and prints as its character; grapheme segmentation has no settled stable String API. | Mojito implements the roadmap's explicit keyword-indexed forms `s[byte=i]`/`s[codepoint=i]`/`s[grapheme=i]` over the self-hosted String (positional `s[i]` rejected as ambiguous). `codepoint=` yields a subset `Codepoint`: `Int`-based via `Intable` instead of `to_u32`/`UInt32`, Writable as its character, scalar-ordered; `Codepoint.from_u32(scalar)` constructs directly (Int-based, `None` for negatives/surrogates/beyond U+10FFFF), UTF-8-encoding its text in ordinary library code through runtime `Byte(Int)` conversions. `grapheme=`/`grapheme_count()` are Mojito-explicit spellings implementing a pinned UAX #29 subset (hand-maintained essentials classifier plus arithmetic Hangul; GB11 simplified to "never break after ZWJ"; GB9b Prepend omitted). Keyword subscripts themselves are a general Mojito feature documented in `docs/grammar.md`. |
| String type split | `String` is the standard runtime string; a `StringLiteral` implicitly converts and materializes to `String`, including for un-annotated bindings (`var s = "lit"` is a `String`). String slicing is a supported non-raising API surface. | Mojito matches the binding default: `: String` annotations resolve to the self-hosted nominal struct, a literal converts through the `@implicit` constructor, and an un-annotated `var s = "lit"` binding materializes the nominal String — narrower than Mojo's universal materialization, since aggregate elements, `comptime` bindings, and bare literal expressions stay `StringLiteral`. Nominal slicing is non-raising byte-wise library code with Python-normalized bounds and strides, matching the builtin literal slice; a cut inside a multibyte sequence keeps the raw bytes and renders lossily at the literal read-back. Result APIs are eager: `find`/`rfind`/`startswith`/`endswith` and `split(sep) raises -> List[String]` (empty separator raises) return owned values where current Mojo returns `StringSlice` views, without the `start`/`end`/`maxsplit` parameters; further APIs (`replace`/`join`/`strip`/…) grow demand-first. Overloads differing only in `StringLiteral`-vs-`String` are rejected (both mangle to the stable `String` symbol spelling). |
| Lazy template strings | Current Mojo's `TString` (std.format.tstring) is origin-parameterized — `TString[origins, //, format_string, *Ts]` holds a borrowed `VariadicPack` of interpolation references, so exclusivity rejects mutating a captured value before the template is used. | Mojito's self-hosted prelude `TString[*Ts: Movable & Writable]` captures typed value snapshots at creation instead (an origin-parameterized reference pack is not yet expressible): copyable interpolations copy in, non-Copyable places snapshot as creation-time formatted strings, and mutating a captured variable afterward prints the snapshot rather than being rejected. Formatting defers to Writable `write_to` in both. `TString[...]` annotations are not spellable in Mojito. |
| Explicit destruction | `@explicit_destroy` no longer opts a type out of implicit deletion. A type narrows or removes `ImplicitlyDeletable` through conditional conformance, commonly `ImplicitlyDeletable where False`. The decorator is optional and only supplies an explanatory diagnostic; using it without a message is an error. | Separate the linearity fact from the diagnostic decorator and derive automatic deletion from conformance. |
| Move initialization | The consuming unified initializer is `__init__(out self, *, deinit move: Self)`; a bare `move:` parameter is rejected with a migration diagnostic. | Use the `deinit` argument convention in current fixtures and documentation while retaining bare `move:` only as an explicit Mojito compatibility spelling. |
| Constraints | Parameter-list `where` clauses were removed. One or more trailing declaration `where` clauses remain, and `(condition, "message")` retains a diagnostic. Type equality now uses `==`/`!=`; `_type_is_eq` was removed. Pack operands such as `Ts.values` work with `conforms_to`. | The removed placement rejects. Repeated trailing clauses, each with its own retained diagnostic tuple, run across functions/methods, structs, trait requirements, and comptime declarations (per-trait conditional-conformance conditions stay single-clause), and generic top-level comptime aliases expand through the checked alias registry; value-bodied aliases and constructor-through-alias calls are the remaining recorded subset gaps. |
| Closures and callables | Closures use a brace capture list after effects, with `imm`, `mut`, `ref`, `var`, moves (`x^`), and optional default conventions; the `unified` keyword was removed. Explicit origin specialization materializes origin-generic function values. Generic `F: def(...)` bounds are callable constraints, while `def(...) thin` / `capturing[origins]` in a parameter list denotes a callable value and its environment contract. Callable parameters may have defaults, anonymous callable contracts may declare their own generic binders, and compile-time control may specialize while a callable remains a residual argument. User structs must explicitly conform to a convention- and origin-bearing `def(...)` closure trait; compatible `__call__` alone is not enough. | Current capture syntax, recursively lifted declaration-time environments, monomorphic and generic-anonymous callable bounds/value parameters, symbolic callable defaults, `OriginSet` inference, explicit capturing-origin sets, residual callable specialization, read/write capture effects, conventions, raising effects, and reference results are checked and execute through indirect dispatch. Explicit Origin arguments participate in overload/generic candidate selection and may be interleaved with packs. `CtValue` remains closure-free and arbitrary callable CTFE is not claimed. Legacy `unified {...}`, unqualified stateful downward funargs (call positions only: a capturing closure does not erase its environment into plain `def(...)` storage — fields and collection elements reject it unless the storage declares `capturing[...]`), and captured nested Origin-specialized values are documented Mojito extensions. |
| Reflection | `Reflected.field_type[name]` became `Reflected.field[name]`; the result is a chainable reflected handle whose type is `.T`. `field_at[index]` is the by-index counterpart. | Implemented with current `reflect[T]` syntax, nested handle chaining, named/indexed diagnostics, and rejection of `field_type`. |
| Integer/SIMD model | `Int` is an alias for `Scalar[DType.int]`. SIMD-width inference uses `SIMDLength` (briefly named `SIMDSize`), or `_` for an unbound width. | Int/Scalar identity is implemented. `SIMDLength` is the width-parameter spelling (`SIMDSize` a deprecated, never-emitted compatibility alias) and the CPU-visible surface is semantically complete for the proof subset: runtime scalar-conversion construction (any `Intable` constructs integer scalars), unary negation, `cast[DType.target]()`, mask `select`, reductions, compile-time-mask `shuffle`, and def-level generic widths validated during checked elaboration. Deferred divergences stay pinned in `docs/grammar.md`/`parity.tsv` (`// % **`, mixed-width broadcast, bool casts, two-vector shuffle family, struct-parameter widths); hardware vector lowering is native-backend work. |
| Origins and pointers | Struct fields may not hide `UnsafeAnyOrigin`; use an explicit origin parameter or `UntrackedOrigin`. Implicit widening conversions to unsafe-any origins are deprecated or removed, and pointer optionals preserve concrete origins. | Hidden unsafe-any fields are rejected and there is no implicit unsafe-origin widening. `UnsafePointer(to=place)` infers a concrete place origin with executable owner loans; a place pointer coerces only to a declared origin parameter at aggregate-storage sites. |
| Imports and artifacts | Resolution order is source package, `.mojoc`, source module, then legacy `.mojopkg`. Relative imports require `from`; dotted absolute imports bind every prefix; intra-package implicit visibility is deprecated. Duplicate explicit local bindings and exact self-imports reject. | Source precedence, prefix namespaces, explicit intra-package visibility, duplicate-binding diagnostics, and canonical self-import diagnostics are implemented; provisional exports preserve distinct mutual cycles. `.mojoc`/`.mojopkg` loading remains in Packaging, Artifacts, And Developer Tooling. |
| Keyword variadics | A declaration or function type uses `var **kwargs`; forwarding remains `**kwargs^`, and the standard owning container is `StringDict`. | Homogeneous free, generic, instance, static, bounded-trait, and indirect collectors use owned `StringDict[T]` values; bare declaration-side `**kwargs` rejects, callable identity retains the collector role, and consuming forwarding runs through the shared binder and its specialization, ownership, origin, duplicate, and effect checks. |
| Borrowed iteration | `Iterable` exposes `IteratorType[iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]]`, and `__iter__(ref self)` returns `Self.IteratorType[origin_of(self)]`, allowing yielded references to retain source origin and mutability. `IterableOwned` separately exposes monomorphic `IteratorOwnedType`. A concrete `__next__ -> ref[o] T` may refine an abstract value result `T` when `T: Copyable`; the abstract caller receives a copy. | The bundled owned protocol now uses monomorphic `IteratorOwnedType`. Parameterized associated member declarations/applications are represented, and an abstract `origin_of(self)` argument now resolves concretely (via a symbolic self-origin that erases at runtime), so a conforming struct's `Self.IteratorType[origin_of(self)]` member resolves and conformance succeeds — including when the conformer spells that application directly as its `__iter__(ref self)` return type. Directional `Copyable` `__next__` refinement is checked, retained as an explicit abstract-call adapter, and executes for bounded calls and generic loops. The borrowed `Iterable` proof protocol still retains the legacy monomorphic `Iter` member: migrating it, removing concrete borrow bridges, and deriving source/yield origins generically remain the next borrowed-iteration work. |
| Owned iteration | `for var x in collection^` supports moving non-Copyable elements; collection deletion conformance is conditional on element capabilities. | Consuming collection iteration moves the source and each element, destroys implicitly deletable residual state on early exit, and rejects any abandoning path (early exit, unhandled raising calls, comprehension filters) when linear residual elements would be abandoned, naming their explicit-destroy obligation. The exhausted linear-element iterator is consumed through a `_finish(deinit self)` named destructor selected by the checker — a Mojito-internal bundled convention modeling the linear-types proposal's named destructors (current Mojo has no owned-iteration equivalent to compare against). |
| Tuple ownership | `Tuple` lifecycle conformances are conditional; `reverse`, `concat`, and `consume_elements` have consuming receivers, and `consume_elements` transfers elements, including non-`ImplicitlyCopyable` values, one at a time to a parameterized closure. Tuple indexing and destructuring do not provide an indexed partial-move place. | The public nominal `Tuple[*Ts]` folds lifecycle conformance per element. A consuming call can implicitly copy a fully `ImplicitlyCopyable` tuple; move-only receivers and `concat` operands require `^`. The dependent `def[index: Int](var element: Ts[index])` handler consumes private pack storage left-to-right, while public indexed transfer remains rejected. |

Python/NumPy additions, GPU changes, and distributed/concurrent facilities remain
outside Mojito's declared first-pass scope.
