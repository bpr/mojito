# Mojo Dev-Branch Tracking

Mojito tracks the latest dev branch of Mojo.

The comparison target rolls with upstream `main`; reproducible audits name an
exact upstream commit, Mojo build, audit date, and Mojito baseline. The
differential runner must likewise report its actual `mojo --version`. Advancing
the audit boundary records work to do—it does not claim that Mojito already
implements every change at that boundary.

## Current Audit — `ae386d1b204` (2026-08-08)

| Role | Immutable revision |
|---|---|
| Audited Mojo dev head | [`ae386d1b20434e126aea8a32a2b625ed1343eaf5`](https://github.com/modular/modular/commit/ae386d1b20434e126aea8a32a2b625ed1343eaf5), whose lockfiles pin `Mojo 1.1.0.dev2026080805` |
| Previous upstream audit boundary | [`609afcd0735054872ba028f27531b7abec947ddc`](https://github.com/modular/modular/commit/609afcd0735054872ba028f27531b7abec947ddc), `Mojo 1.0.0b3.dev2026072505` |
| Stable Mojito implementation audited | `9118482492edc70b8d3b1d929900f8505ac10a80` from `/home/bpr/src/rust/projects/stable/mojito` |
| Source delta | [Upstream comparison `609afcd073…ae386d1b204`](https://github.com/modular/modular/compare/609afcd0735054872ba028f27531b7abec947ddc...ae386d1b20434e126aea8a32a2b625ed1343eaf5) |

Upstream cut the accumulated nightly record over to the staged
[`v1.0.0` release notes](https://github.com/modular/modular/blob/ae386d1b20434e126aea8a32a2b625ed1343eaf5/mojo/docs/releases/v1.0.0.md)
on August 6. That document is dated August 11, after this audit, so it is source
evidence rather than a claim that the final release had already occurred. The
short [post-cutover nightly changelog](https://github.com/modular/modular/blob/ae386d1b20434e126aea8a32a2b625ed1343eaf5/mojo/docs/nightly-changelog.md)
must be read together with it. The rendered
[nightly release page](https://mojolang.org/releases/nightly/) can lag `main`;
the commit and lockfile version above are authoritative for this audit.

The largest CPU-facing movement is not in MLIR or GPU support. Mojo has
consolidated lifecycle names and defaults, added lambda expressions, changed the
default meaning of list expressions to fixed-size `Array`, unified the pointer
and allocation model, tightened source and import rules, and changed standard
collection slicing and view contracts. These affect ordinary systems programs
and current standard-library source, so they take precedence over expanding the
existing proof-subset library breadth.

**Close-out record (2026-08-15).** The full differential conformance run
passes 176/176 `conformance/cases.tsv` cases against the exact audited build:
`mojo --version` reports `Mojo 1.1.0.dev2026080805 (7ade05af)` from a Pixi
environment pinning the `mojo ==1.1.0.dev2026080805` conda nightly (the version
`ae386d1b20434e126aea8a32a2b625ed1343eaf5`'s lockfiles pin); the Mojito side is
commit `a6bfe27` plus the close-out change that carries this record. The
close-out resolved all twenty open-question probes (their answers live in the
new `cases.tsv` claims and parity-row notes), re-confirmed the five-item
re-probe table, fixed a discovery-path regression for dependent callable bounds
(`F: def(T) -> T` residual signatures), aligned the origin-alias vocabulary
(`ImmStaticOrigin`, `ImmUntrackedOrigin`, `MutUnsafeAnyOrigin`), and
reclassified the July-era case rows whose behavior the head changed (implicit
`def`-scope declaration, the Dict projected-value refresh, raising
explicit-destroy rollback, and untracked-ref conversions are now documented
one-sided rows).

## Prioritized Changeset

The order below is the recommended implementation order; dependencies and
independent quick fixes are called out explicitly. Compatibility aliases may be
retained deliberately, but Mojito's own diagnostics, fixtures, and bundled
sources should use current canonical spellings.

### 0. Retarget the lifecycle foundation

Status 2026-08-09: DONE — canonical `Deinitable`/`__deinit__` vocabulary with
parse-time normalization of the deprecated spellings, effective conditional
`Movable`, the three lifecycle predicates (semantics pinned from the
audited `std/traits/*.mojo` sources), and the `std.traits`/`std.origin`
module homes (export lists mirroring the audited upstream surface).

Post-pin spot alignment 2026-08-11: one day after the audit pin, upstream
[`22b5036987`](https://github.com/modular/modular/commit/22b5036987b33d3724dd5c3a495112f684bf8fc4)
hard-renamed the predicates to `IsTriviallyMovable`/`IsTriviallyCopyable`/
`IsTriviallyDeinitable` (no deprecated aliases) and made
`conforms_to(T, TrivialRegisterPassable)` a sufficient first disjunct. Mojito
follows both: the `Is`-prefixed spellings are the only ones that resolve
(compiler recognition, `std.traits` exports, and diagnostics), and a declared
`TrivialRegisterPassable` conformance or bound satisfies the predicates ahead
of the structural check.

Canonicalize `Deinitable` and `__deinit__` through syntax, checked traits, MIR
drop metadata, VM dispatch, the proof standard library, and documentation.
`ImplicitlyDeletable` and `__del__` still exist upstream as deprecated
compatibility spellings; they must not remain Mojito's internal vocabulary.
Mojito already treats every initialized type as movable, matching the new
default for ordinary structs, but it ignores `Movable where False` and other
conditional opt-outs. Make those constraints effective. Add the current
`TriviallyMovable[T]`, `TriviallyCopyable[T]`, and
`TriviallyDeinitable[T]` comptime predicates after the canonical trait facts
exist (upstream has since `Is`-prefixed them; see the spot alignment above).
Move explicit bundled imports to the canonical `std.traits` home; move
`Origin` imports to `std.origin`. Prelude re-exports may remain compatibility
bridges. The module moves are recorded by
[`84bc36ac`](https://github.com/modular/modular/commit/84bc36aceb0f4c93efefd9fda1a6e9cdd5d56782)
and [`f23369d8`](https://github.com/modular/modular/commit/f23369d8a33fcbd606667dce318daa0b56f0111c).
Relevant lifecycle changes are
[`1beff2ac`](https://github.com/modular/modular/commit/1beff2ac41123ee873c4d95ced27636300458d0e),
[`f8678b34`](https://github.com/modular/modular/commit/f8678b34ddfcac0b0123fb1ce32a44111c4ee1dd),
[`1b7a909f`](https://github.com/modular/modular/commit/1b7a909f90331b961e830cc7cd19155359af4adc),
and [`8eb24855`](https://github.com/modular/modular/commit/8eb248550e5ae2bf9c1558f46b2b7b3f6beec3e0).

### 1. Close small accepted-source and diagnostic gaps

Status 2026-08-09: DONE for this seven-item slice — canonical keyword
collectors and their callable ABI, duplicate/self-import diagnostics, the
`imm`/`mut` overload pin, empty zero-step ranges, retained constraint messages
across declaration families, and contextual free-function-name rejection are
implemented and covered at their owning phases.

The larger adjacent surface the audit exposed — repeated trailing `where`
clauses and generic top-level `comptime` aliases — is also DONE (2026-08-10):
every declaration family retains its clause list with per-clause messages
through a plural checked-constraint contract, and generic aliases lower into a
checked alias registry expanded during type resolution (type-valued bodies
only; value-bodied aliases, origin parameters, and constructor-through-alias
calls remain recorded subset gaps).

The completed corrections are:

- Parse `var **kwargs` in declarations and reject bare `**kwargs`. Forwarding
  with `**kwargs^` and `StringDict` storage already work. Function types also
  require the current spelling, but that is not only a parser tweak: extend the
  function-type parameter representation and callable ABI identity to retain a
  keyword-variadic role.
- Reject a second same-named function imported from another module instead of
  silently replacing the first binding. Explicit intra-package imports are
  already enforced.
- Diagnose a standalone module importing its own name instead of relying on the
  linker's cycle deduplication.
- Keep the existing rejection of overloads that differ only by `imm` versus
  `mut`, and pin it with the current differential case.
- Make `range(..., step=0)` empty in both compile-time evaluators. Nominal
  runtime `Range` already does this, but the CTFE and VM intrinsic paths still
  report an error.
- Interpret `where (condition, "message")` as a constraint plus a retained
  diagnostic message in functions, structs, conditional conformances, and
  `comptime` declarations.
- Reject current reserved/predefined words `class`, `del`, `match`, and
  `yield` as free or nested function names at declaration checking. Do not
  pre-tokenize them as active syntax; preserving them as parser-level future
  extension points will make eventual class, pattern-matching, and generator
  work easier. They remain legal in ordinary identifier positions, and current
  method spellings such as `def match(self)` remain legal.

The principal upstream changes are
[`6e6392f1`](https://github.com/modular/modular/commit/6e6392f1e65a7c67c56c2422ddedc26b8c702284),
[`c924834b`](https://github.com/modular/modular/commit/c924834b9e1061cc5e2b2c8a40c99ed60016fa17),
[`c12f26a4`](https://github.com/modular/modular/commit/c12f26a49196f319f87132fc53d183fcd55e2ff1),
[`fe9829a7`](https://github.com/modular/modular/commit/fe9829a703e140bb613f8ac66e6d117bd2cb2a51),
[`04d7cc58`](https://github.com/modular/modular/commit/04d7cc582f11917dbe1186ca8562b33db3ea1acd),
and [`e73bb67c`](https://github.com/modular/modular/commit/e73bb67c4604d4849dfba218cd2f95c4c7f244a2).

### 2. Add lambda expressions through the existing callable pipeline

Implement single-expression lambdas such as
`lambda (x: Int) {} -> Int: x + 1`. The capture list and return type may each
be omitted: an omitted capture list imm-captures free variables and is thin when
there are none; an omitted return type is fixed to `None`, not inferred. A thin
lambda behaves like a named function value and comptime parameter, while a
capturing or still-generic lambda is a runtime closure instance. Add an explicit
AST/HIR node, then lower it into Mojito's existing nested-definition, capture,
callable-contract, specialization, and indirect-call machinery—do not add a
second callable representation. Current standard-library sources already use
lambdas, making this a source-porting prerequisite. See the
[lambda manual](https://github.com/modular/modular/blob/ae386d1b20434e126aea8a32a2b625ed1343eaf5/mojo/docs/manual/functions/lambda.mdx)
and commits
[`dfede0f2`](https://github.com/modular/modular/commit/dfede0f2a99c6976ecdb3da7fddc16ac7a19a4d6),
[`4123acca`](https://github.com/modular/modular/commit/4123accaaa5eb3e91b1d5226de74e135a99b5ab3),
[`9a6c037e`](https://github.com/modular/modular/commit/9a6c037ea84525b750b8e322774944027a51d513),
and [`6dfb27c9`](https://github.com/modular/modular/commit/6dfb27c95886662765f2b164561dba0349869111).

### 3. Implement `Array` and retarget list-expression inference

Add fixed-size nominal `Array[T, length]`, whose element parameter may be any
type and whose `Copyable`, `Movable`, and `Deinitable` conformances are
conditional. List-literal/move construction requires a `Movable` element;
by-reference indexing and iteration do not, and destruction is conditional on
`Deinitable`. Array itself is neither `ImplicitlyCopyable` nor `Defaultable`.
An uncontextualized `[1, 2, 3]` must become `Array[Int, 3]`; an expected type
that defines a list-literal constructor—notably `List[T]`—still controls
contextual materialization. Stable Mojito currently hard-codes the
uncontextualized result as `List`, so this is a language-semantic change, not an
`InlineArray` rename. Preserve nominal constructor lowering if it is sufficient;
only reopen the MIR/text schema if the implementation proves that Array needs a
new MIR form. The temporary `InlineArray` alias is already removed on the
audited dev head. See the current
[`Array` source](https://github.com/modular/modular/blob/ae386d1b20434e126aea8a32a2b625ed1343eaf5/mojo/stdlib/std/collections/array.mojo)
and commits
[`7657e0f0`](https://github.com/modular/modular/commit/7657e0f0dc8a8f4c81f6435962644faaa19527c3),
[`9dfc95aa`](https://github.com/modular/modular/commit/9dfc95aa2f69688aa840e981d399de2cb3a1da52),
and [`8d4fd368`](https://github.com/modular/modular/commit/8d4fd368cd95e57302610de09d3bbbf4d8747999).
The upstream implementation uses `Pointer` internally. Mojito can land Array's
semantics first over its private aggregate storage; reverse items 3 and 4 if the
chosen slice is a direct source port instead.

### 4. Move directly to the current pointer/allocation model

Do not stop at the July plan's free `alloc[T](count)` API. Canonical source now
uses `Pointer`, `MutPointer`, `ImmPointer`, `ptr[]`, and explicitly unsafe
operations such as `unsafe_offset`, `unsafe_write`, `unsafe_take_pointee`, and
`unsafe_deinit_pointee`. Heap allocation is layout-based:

```mojo
var allocation = alloc(Layout[Int](count=4))
var ptr = allocation.unsafe_ptr()
ptr.unsafe_offset(0).unsafe_write(42)
dealloc(allocation^)
```

`Allocation[T]` is explicitly destroyed, owns its heap storage through a
`ThinAllocation[T]`, and retains the `Layout[T]` used to allocate it. The
deprecated layout-less `alloc[T](count)` returns a raw Pointer; its temporary
migration spelling is `unsafe_alloc`, not another Allocation constructor.
`UnsafePointer` remains a deprecated alias of `Pointer` at this audit hash.
Reuse Mojito's typed pointer MIR, stable allocation identities, provenance, and
explicit-destroy machinery rather than inventing another heap representation.
The canonical source is in upstream
[`pointer.mojo`](https://github.com/modular/modular/blob/ae386d1b20434e126aea8a32a2b625ed1343eaf5/mojo/stdlib/std/memory/pointer.mojo)
and [`alloc.mojo`](https://github.com/modular/modular/blob/ae386d1b20434e126aea8a32a2b625ed1343eaf5/mojo/stdlib/std/memory/alloc.mojo);
the current contract is summarized in the
[pointer manual](https://github.com/modular/modular/blob/ae386d1b20434e126aea8a32a2b625ed1343eaf5/mojo/docs/manual/pointers/using-pointers.mdx).
The `UnsafeMaybeUninit` growth from
[`b324feea`](https://github.com/modular/modular/commit/b324feeaa16bc13a12c0200164d1878fcfa64a87)
is done: inline-uninit storage (a new VM capability, not a heap-slot spelling)
carries the `unsafe_write`, overloaded `unsafe_assume_init`, `unsafe_deinit`,
and `unsafe_forget` vocabulary with upstream's conformances and triviality
facts; reading/taking uninitialized storage traps deterministically where
upstream leaves it undefined. Recorded subset gaps: `zeroed()` (no byte-level
value model) and `UnsafeMaybeUninit.unsafe_ptr()` (the §5 views work landed
`origin_of(self)` as a Pointer origin argument — with interior-generation
projections and multi-element origin-bearing pointers, giving
`List.unsafe_ptr()` — but `UnsafeMaybeUninit`'s single-slot accessor still
awaits demand; the `ref self` `unsafe_assume_init` covers borrowed access).

### 5. Replace copied/clamped slices with current views and bounds

Done except the follow-ups below: `Span(list)` and `StringSpan` are borrowed
views over multi-element origin-bearing pointers; contiguous List/Span slices
and String/StringSpan keyword slices are strict (violations abort through the
uncatchable `os.abort` trap, byte endpoints on codepoint boundaries);
positional String slicing rejects with a keyword-slice hint; strided List
slicing keeps `StridedSlice.indices()` normalization; String/StringSpan/
StringLiteral iteration yields grapheme-cluster StringSpan views; and
`StringSlice` stays a never-emitted alias. Span borrowed iteration landed
with the §7 pass (`_SpanIter` on the origin-parameterized protocol, element
references with write-through). Remaining follow-ups (probed in
`conformance/probes/`): the `Imm`/`Mut` alias spellings and the upstream
fate of positional String and StringLiteral slicing. The original
specification:

Introduce `Span` and canonical `StringSpan` views (with `Imm`/`Mut` aliases).
Contiguous List and Span slices, String `byte=`/`codepoint=` slices, and
StringSpan `byte=`/`codepoint=`/`grapheme=` slices reject negative,
out-of-range, or reversed bounds instead of normalizing or clamping them. Byte
slice endpoints must also fall on UTF-8 codepoint boundaries. Preserve omitted
bounds; current String has grapheme indexing but no grapheme contiguous-slice
overload. Do not globally ban negative bounds in user-defined slice descriptors
or `StridedSlice`: strided List slicing still normalizes through
`StridedSlice.indices()` and returns a copied List. On the CPU-default assertion
configuration, invalid contiguous bounds abort. String, StringSpan, and
StringLiteral ordinary iteration should yield borrowed grapheme-cluster
StringSpan views. `StringSlice` remains an upstream compatibility alias, but
Mojito should emit `StringSpan`. See
[`017f9f82`](https://github.com/modular/modular/commit/017f9f82cb334c510ba1a3e8aeadfc05488d734e),
[`914dad70`](https://github.com/modular/modular/commit/914dad7031211669a45eb93451207fea54e53e36),
and [`3e1a67e8`](https://github.com/modular/modular/commit/3e1a67e85ff445774155c57ab8c1f169286cf5f9).

### 6. Align linear containers and owning APIs

Status 2026-08-14: DONE — owned iteration tightened to the head's
`Movable & Deinitable` bounds (the `_finish` linear-element extension is
removed; linear variadic-pack forwarding remains); Optional rebuilt as the
`T: AnyType` owning container with `init_with=` placement construction,
`deinit_with`, `deinit_assert_empty`, linear-capable `map`/`and_then`, and
Iterable (value-yielding borrowed iteration; `for ref` over Optional is a
recorded subset gap) plus owned iteration; Variant renamed to
`unwrap`/`unsafe_unwrap` with `set(init_with=…)`, an all-alternatives
`Deinitable` gate on both `set` forms, and a tag-dispatched `deinit_with`;
the family APIs (`deinit_with` on List/Array/Dict/Set/StringDict/Tuple,
`clear_with` on Dict/Set, displacement-returning `insert` on
Dict/Set/StringDict) landed; and a minimal `OwnedPointer` shipped with
`into_inner` from day one. Deque and LinkedList do not exist in Mojito, and
HashDict/HashSet stay outside the declared family. Exact upstream API
shapes are pinned by the §6 probe set in `conformance/probes/` (handler
conventions, drain order, `insert` semantics, `OwnedPointer`'s `p[]`
dereference, the mut-receiver UnsafeMaybeUninit take, the owned-iteration
declared family) — run them at the next re-pin.

Lifecycle is the prerequisite here; the Optional and Variant work can land
independently of Array and Pointer. Loosen both `Optional[T]` and
`Variant[*Ts]` to `AnyType`, add their `init_with=` placement constructors,
conditional lifecycle contracts, and `deinit_with` operations without a
`Movable` requirement. `Variant.set(init_with=...)` performs in-place
replacement. Optional is `Iterable`, not itself an `Iterator`; align its
borrowed and owned iteration bounds, `deinit_assert_empty`, and linear-capable
`map`/`and_then`.

Migrate `Variant.take`/`unsafe_take` to `unwrap`/`unsafe_unwrap` and
`OwnedPointer.take` to `into_inner`, retaining old names only under the
compatibility policy; Optional's current `take` is unchanged. Grow APIs by their
actual families: `deinit_with` on Array/List/Deque/LinkedList/Dict/Set/Tuple/
Optional/Variant/StringDict; `clear_with` on Dict and Set;
displacement-returning `insert` on Dict, Set, and StringDict; and consuming
iteration on the collections that
declare `IterableOwned` (not Tuple, which uses `consume_elements`). Quarantine
or remove Mojito's owned-iteration extension for non-`Deinitable` elements where
current Mojo requires `Movable & Deinitable`.

### 7. Add subtree-origin safety and temporary-origin inference

Status 2026-08-14: DONE. `origin._subtree` is accepted in Pointer origin
arguments and `origin_cast` targets (terminal-only; `ref [...]` clauses
reject — the wider upstream surface is pinned by
`conformance/probes/subtree_origin_surface.mojo`), carried as a terminal
`OriginSeg::Subtree`/`subtree` flag through checked HIR and verified MIR,
with the conservative semantics live in the ownership analysis: staleness
on mutation at, above, or below the base; first-write self-invalidation of
a mutable subtree reference; and consume-time interior liveness (which
Mojito's use-before-transfer architecture already enforced — pinned by
fixtures, no compiler change). `Pointer(to=…)` through a `ref` binding
mints subtree provenance. Temporary-origin inference landed as the
`@implicit` `ref [origin]` constructor channel: a `List` passes directly
where a `Span` is expected (arguments, bindings, returns), the conversion
result borrowing its source like the explicit construction; a bare list
literal stays a recorded subset gap (it types as `Array` — probed by
`conformance/probes/implicit_span_conversion.mojo`). A whole-variable move
now invalidates the interior generations rooted at the moved variable
(the owner-side dual of consume liveness), which also makes the tracked
`Allocation.unsafe_ptr()` reject use-after-dealloc statically. The
original specification:

Keep Mojito's existing named interior generations; they model precise
collection-owned storage. Add the audited dev branch's experimental
`Origin._subtree` as a separate conservative origin form. It can designate the
root or any descendant, so mutation anywhere below the root invalidates the
loan, a write through a mutable subtree reference invalidates that reference
after its first write, and consuming an aggregate containing an interior origin
requires that origin to remain live. Also allow an origin-bearing `@implicit`
conversion result to refine its origin from a register temporary. Carry the new
fact explicitly through checked HIR and verified MIR. The audited stdlib does
not yet depend on this experimental surface, so it follows the public container
work. See the current
[origin design](https://github.com/modular/modular/blob/ae386d1b20434e126aea8a32a2b625ed1343eaf5/mojo/proposals/origin-design.md)
and commits
[`fae2eef7`](https://github.com/modular/modular/commit/fae2eef7cab68c8af9ca51f391184c32de4038d5),
[`f3958309`](https://github.com/modular/modular/commit/f3958309415ab40094c2d49c620d5655c9b0bafd),
[`12441229`](https://github.com/modular/modular/commit/12441229e2bf98bb2e37a26c85ddbc7e05ab54b3),
and [`7852cfb1`](https://github.com/modular/modular/commit/7852cfb102d2444ef472b5d7749b530a068ccfc3).

### 8. Finish scalar, SIMD, range, and generic vocabulary

Continue the existing SIMD roadmap item, but use `SIMDLength` and `length`, not
the transitional `SIMDSize`/`size`. Generalize the Int-only Range proof subset
to the current Int/Scalar family; add integer-Scalar construction from any
`Intable`; migrate `TypeList.size` to `length` and variadic
`any_satisfies`/`all_satisfies` to `any`/`all`. Tuple already uses `*Ts`, so its
new public parameter name needs only a compatibility probe. Reject invalid SIMD
lengths during checked elaboration, not at a late VM operation.

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
  spelling.

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
and `keys`/`values`/`items` remain eager snapshots rather than live views.
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
| String indexing | Current Mojo's String exposes byte/codepoint access through iterator and method APIs; the exact keyword-subscript spelling is not settled stable syntax. `Codepoint` constructs from `UInt32` scalars (`from_u32`, `to_u32`) and prints as its character; grapheme segmentation has no settled stable String API. | Mojito implements the roadmap's explicit keyword-indexed forms `s[byte=i]`/`s[codepoint=i]`/`s[grapheme=i]` over the self-hosted String (positional `s[i]` rejected as ambiguous). `codepoint=` yields a subset `Codepoint`: `Int`-based via `Intable` instead of `to_u32`/`UInt32`, Writable as its character, scalar-ordered; `Codepoint.from_u32(scalar)` constructs directly (Int-based, `None` for negatives/surrogates/beyond U+10FFFF), UTF-8-encoding its text in ordinary library code through runtime `Byte(Int)` conversions. `grapheme=`/`grapheme_count()` are Mojito-explicit spellings implementing a pinned UAX #29 subset (hand-maintained essentials classifier plus arithmetic Hangul; GB11 simplified to "never break after ZWJ"; GB9b Prepend omitted). Keyword subscripts themselves are a general Mojito feature documented in `grammar.md`. |
| String type split | `String` is the standard runtime string; a `StringLiteral` implicitly converts and materializes to `String`, including for un-annotated bindings (`var s = "lit"` is a `String`). String slicing is a supported non-raising API surface. | Mojito matches the binding default: `: String` annotations resolve to the self-hosted nominal struct, a literal converts through the `@implicit` constructor, and an un-annotated `var s = "lit"` binding materializes the nominal String — narrower than Mojo's universal materialization, since aggregate elements, `comptime` bindings, and bare literal expressions stay `StringLiteral`. Nominal slicing is non-raising byte-wise library code with Python-normalized bounds and strides, matching the builtin literal slice; a cut inside a multibyte sequence keeps the raw bytes and renders lossily at the literal read-back. Result APIs are eager: `find`/`rfind`/`startswith`/`endswith` and `split(sep) raises -> List[String]` (empty separator raises) return owned values where current Mojo returns `StringSlice` views, without the `start`/`end`/`maxsplit` parameters; further APIs (`replace`/`join`/`strip`/…) grow demand-first. Literal `hash(...)` uses the VM's FNV path while nominal keys hash through the struct's DJB2 `__hash__` — containers are internally consistent per key type. Overloads differing only in `StringLiteral`-vs-`String` are rejected (both mangle to the stable `String` symbol spelling). |
| Lazy template strings | Current Mojo's `TString` (std.format.tstring) is origin-parameterized — `TString[origins, //, format_string, *Ts]` holds a borrowed `VariadicPack` of interpolation references, so exclusivity rejects mutating a captured value before the template is used. | Mojito's self-hosted prelude `TString[*Ts: Movable & Writable]` captures typed value snapshots at creation instead (an origin-parameterized reference pack is not yet expressible): copyable interpolations copy in, non-Copyable places snapshot as creation-time formatted strings, and mutating a captured variable afterward prints the snapshot rather than being rejected. Formatting defers to Writable `write_to` in both. `TString[...]` annotations are not spellable in Mojito. |
| Explicit destruction | `@explicit_destroy` no longer opts a type out of implicit deletion. A type narrows or removes `ImplicitlyDeletable` through conditional conformance, commonly `ImplicitlyDeletable where False`. The decorator is optional and only supplies an explanatory diagnostic; using it without a message is an error. | Separate the linearity fact from the diagnostic decorator and derive automatic deletion from conformance. |
| Move initialization | The consuming unified initializer is `__init__(out self, *, deinit move: Self)`; a bare `move:` parameter is rejected with a migration diagnostic. | Use the `deinit` argument convention in current fixtures and documentation while retaining bare `move:` only as an explicit Mojito compatibility spelling. |
| Constraints | Parameter-list `where` clauses were removed. One or more trailing declaration `where` clauses remain, and `(condition, "message")` retains a diagnostic. Type equality now uses `==`/`!=`; `_type_is_eq` was removed. Pack operands such as `Ts.values` work with `conforms_to`. | The removed placement rejects. Repeated trailing clauses, each with its own retained diagnostic tuple, run across functions/methods, structs, trait requirements, and comptime declarations (per-trait conditional-conformance conditions stay single-clause), and generic top-level comptime aliases expand through the checked alias registry; value-bodied aliases and constructor-through-alias calls are the remaining recorded subset gaps. |
| Closures and callables | Closures use a brace capture list after effects, with `imm`, `mut`, `ref`, `var`, moves (`x^`), and optional default conventions; the `unified` keyword was removed. Explicit origin specialization materializes origin-generic function values. Generic `F: def(...)` bounds are callable constraints, while `def(...) thin` / `capturing[origins]` in a parameter list denotes a callable value and its environment contract. Callable parameters may have defaults, anonymous callable contracts may declare their own generic binders, and compile-time control may specialize while a callable remains a residual argument. User structs must explicitly conform to a convention- and origin-bearing `def(...)` closure trait; compatible `__call__` alone is not enough. | Current capture syntax, recursively lifted declaration-time environments, monomorphic and generic-anonymous callable bounds/value parameters, symbolic callable defaults, `OriginSet` inference, explicit capturing-origin sets, residual callable specialization, read/write capture effects, conventions, raising effects, and reference results are checked and execute through indirect dispatch. Explicit Origin arguments participate in overload/generic candidate selection and may be interleaved with packs. `CtValue` remains closure-free and arbitrary callable CTFE is not claimed. Legacy `unified {...}`, unqualified stateful downward funargs (call positions only: a capturing closure does not erase its environment into plain `def(...)` storage — fields and collection elements reject it unless the storage declares `capturing[...]`), and captured nested Origin-specialized values are documented Mojito extensions. |
| Reflection | `Reflected.field_type[name]` became `Reflected.field[name]`; the result is a chainable reflected handle whose type is `.T`. `field_at[index]` is the by-index counterpart. | Implemented with current `reflect[T]` syntax, nested handle chaining, named/indexed diagnostics, and rejection of `field_type`. |
| Integer/SIMD model | `Int` is an alias for `Scalar[DType.int]`. SIMD-width inference uses `SIMDLength` (briefly named `SIMDSize`), or `_` for an unbound width. | Int/Scalar identity is implemented. `SIMDLength` is the width-parameter spelling (`SIMDSize` a deprecated, never-emitted compatibility alias) and the CPU-visible surface is semantically complete for the proof subset: runtime scalar-conversion construction (any `Intable` constructs integer scalars), unary negation, `cast[DType.target]()`, mask `select`, reductions, compile-time-mask `shuffle`, and def-level generic widths validated during checked elaboration. Deferred divergences stay pinned in `grammar.md`/`parity.tsv` (`// % **`, mixed-width broadcast, bool casts, two-vector shuffle family, struct-parameter widths); hardware vector lowering is native-backend work. |
| Origins and pointers | Struct fields may not hide `UnsafeAnyOrigin`; use an explicit origin parameter or `UntrackedOrigin`. Implicit widening conversions to unsafe-any origins are deprecated or removed, and pointer optionals preserve concrete origins. | Hidden unsafe-any fields are rejected and there is no implicit unsafe-origin widening. `UnsafePointer(to=place)` infers a concrete place origin with executable owner loans; a place pointer coerces only to a declared origin parameter at aggregate-storage sites. |
| Imports and artifacts | Resolution order is source package, `.mojoc`, source module, then legacy `.mojopkg`. Relative imports require `from`; dotted absolute imports bind every prefix; intra-package implicit visibility is deprecated. Duplicate explicit local bindings and exact self-imports reject. | Source precedence, prefix namespaces, explicit intra-package visibility, duplicate-binding diagnostics, and canonical self-import diagnostics are implemented; provisional exports preserve distinct mutual cycles. `.mojoc`/`.mojopkg` loading remains in Packaging, Artifacts, And Developer Tooling. |
| Keyword variadics | A declaration or function type uses `var **kwargs`; forwarding remains `**kwargs^`, and the standard owning container is `StringDict`. | Homogeneous free, generic, instance, static, bounded-trait, and indirect collectors use owned `StringDict[T]` values; bare declaration-side `**kwargs` rejects, callable identity retains the collector role, and consuming forwarding runs through the shared binder and its specialization, ownership, origin, duplicate, and effect checks. |
| Borrowed iteration | `Iterable` exposes `IteratorType[iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]]`, and `__iter__(ref self)` returns `Self.IteratorType[origin_of(self)]`, allowing yielded references to retain source origin and mutability. `IterableOwned` separately exposes monomorphic `IteratorOwnedType`. A concrete `__next__ -> ref[o] T` may refine an abstract value result `T` when `T: Copyable`; the abstract caller receives a copy. | The bundled owned protocol now uses monomorphic `IteratorOwnedType`. Parameterized associated member declarations/applications are represented, and an abstract `origin_of(self)` argument now resolves concretely (via a symbolic self-origin that erases at runtime), so a conforming struct's `Self.IteratorType[origin_of(self)]` member resolves and conformance succeeds — including when the conformer spells that application directly as its `__iter__(ref self)` return type. Directional `Copyable` `__next__` refinement is checked, retained as an explicit abstract-call adapter, and executes for bounded calls and generic loops. The borrowed `Iterable` proof protocol still retains the legacy monomorphic `Iter` member: migrating it, removing concrete borrow bridges, and deriving source/yield origins generically remain the next borrowed-iteration work. |
| Owned iteration | `for var x in collection^` supports moving non-Copyable elements; collection deletion conformance is conditional on element capabilities. | Consuming collection iteration moves the source and each element, destroys implicitly deletable residual state on early exit, and rejects any abandoning path (early exit, unhandled raising calls, comprehension filters) when linear residual elements would be abandoned, naming their explicit-destroy obligation. The exhausted linear-element iterator is consumed through a `_finish(deinit self)` named destructor selected by the checker — a Mojito-internal bundled convention modeling the linear-types proposal's named destructors (current Mojo has no owned-iteration equivalent to compare against). |
| Tuple ownership | `Tuple` lifecycle conformances are conditional; `reverse`, `concat`, and `consume_elements` have consuming receivers, and `consume_elements` transfers elements, including non-`ImplicitlyCopyable` values, one at a time to a parameterized closure. Tuple indexing and destructuring do not provide an indexed partial-move place. | The public nominal `Tuple[*Ts]` folds lifecycle conformance per element. A consuming call can implicitly copy a fully `ImplicitlyCopyable` tuple; move-only receivers and `concat` operands require `^`. The dependent `def[index: Int](var element: Ts[index])` handler consumes private pack storage left-to-right, while public indexed transfer remains rejected. |

Python/NumPy additions, GPU changes, and distributed/concurrent facilities remain
outside Mojito's declared first-pass scope.
