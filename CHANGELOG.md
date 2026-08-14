# Changelog

All notable changes to Mojito will be documented in this file. The project uses
Semantic Versioning while its public Rust API and supported Mojo subset continue
to evolve under the `0.x` compatibility rules.

## [Unreleased]

### Changed

- Views and strict bounds (nightly §5): `Span(list)` and `StringSpan` are
  borrowed contiguous views — multi-element origin-bearing pointers (a new
  origin capability: an interior-generation-projected pointer origin
  legally addresses many elements, minted by the new `origin_cast` rebind
  and `List.unsafe_ptr()`, staled by source mutation) plus a length, with
  construction lending the source's place so mutation conflicts while any
  view lives. Contiguous List/Span slices and the new String/StringSpan
  `byte=`/`codepoint=`(/`grapheme=` on StringSpan) keyword slices are
  strict: negative, out-of-range, or reversed bounds abort through the new
  uncatchable `os.abort` trap, and byte endpoints must fall on UTF-8
  codepoint boundaries; strided List slicing keeps `StridedSlice.indices()`
  normalization and copied results. Positional String slicing (contiguous
  and strided) now rejects with a keyword-slice hint (StringLiteral keeps
  the builtin literal slice pending a probe), `StringSlice` is accepted as
  a never-emitted alias of `StringSpan`, and ordinary String, StringSpan,
  and StringLiteral iteration yields borrowed grapheme-cluster StringSpan
  views. Keyword slices (`x[name=a:b]`, omitted bounds preserved) are a
  general subscript form binding keyword-only slice-descriptor parameters;
  `MirInstr::MultiIndex` keyword arguments now carry slice descriptors.
  The `roadmap.md` task lists were also reformatted into nested bullets.
- Pointer/allocation model closure (nightly §4): the bundled collections,
  String, and fixtures now allocate through `std.memory` (`unsafe_alloc` +
  `unsafe_free`), the compiler-private `take(i)`/`destroy(i)` pointer methods
  are deleted in favor of their public `unsafe_offset(i).unsafe_take_pointee()`
  / `unsafe_deinit_pointee()` replacements (the `PointerStorageTake`/`Destroy`
  MIR operations are unchanged), the static-alloc heap primitive is narrowed
  to `std/memory.mojo` alone, and `Pointer[T].unsafe_dangling()` replaces the
  removed `dangling()` spelling (rename hint on the old name). The
  `pointers.unsafe` parity row moves from divergence to subset with the
  remaining upstream surface (address spaces, SIMD load/store families,
  casts, `ManagedAllocation`, `Layout.alignment()`) recorded as deferred;
  `current-pointer-allocation` becomes a shared differential run case and
  the static-alloc rejection gains a reject case. Test threads get a
  16 MiB stack via `.cargo/config.toml` — checker recursion depth scales
  with the linked program, and the grown prelude pushed two
  subscript-contract tests past the old 2 MiB default. Fixed a
  pre-existing specializer bug this migration exposed: a module-level
  `comptime` constant substituted into a same-named type parameter retained
  on a specialized generic def clone (the clone body is materialized as a
  bare statement list, so the declaration's own compile-time parameter
  names must be removed from the substitution); with prelude-linked
  `unsafe_alloc[T]`, any user constant named `T` used to corrupt its
  `Pointer[T, MutUntrackedOrigin]` annotation.

- Layout-based allocation (nightly §4): new self-hosted `std.memory` module
  with `Layout[T]` (`count=`/`alignment=` keyword construction, `count()`),
  linear `ThinAllocation[T]`/`Allocation[T]` (`Deinitable where False` +
  `@explicit_destroy` — implicit drop is a checker error), `unsafe_ptr()`,
  `layout()`, `into_thin()`, `unsafe_leak()`, `unsafe_with_layout()`, free
  `alloc(Layout[T](count=n)) -> Allocation[T]` (prelude-exported),
  `dealloc(allocation^)`, and raw `unsafe_alloc[T](count, alignment=…)`.
  The legacy static `UnsafePointer[T].alloc[_aligned]` surface is retired
  from user code with a migration-hint diagnostic; it survives only as the
  compiler-private heap primitive inside bundled standard-library sources
  (fixtures and linked tests migrated to the current vocabulary). Alignment
  0 means natural element alignment; `Layout.alignment()` is deferred until
  an `align_of` builtin exists, and `unsafe_ptr()` returns an untracked
  origin (`origin_of(field)` pointer results are not yet expressible).

- Keyword pointer subscript (nightly §4): `ptr[unsafe_offset=i]` reads the
  pointee at offset `i` — current Mojo's replacement spelling for the
  deprecated positional `ptr[i]` read, which stays accepted as a bridge.
  It lowers exactly like the positional read (place substitution or handle
  read on origin-bearing pointers, the pointer-intrinsic `Index` otherwise);
  keyword subscripts remain read-only, so stores keep `ptr[i] = v` and
  `unsafe_write`.

- The `unsafe_*` pointer operation vocabulary (nightly §4): `unsafe_offset(i)`
  (provenance-preserving arithmetic), `unsafe_write(value)` and
  `unsafe_write(copy=v)` (move/copy pointee initialization at offset 0 —
  the first keyword-accepting builtin pointer method), `unsafe_take_pointee()`
  and `unsafe_deinit_pointee()` (zero-argument public forms of the raw
  initialized-slot operations, fixed to offset 0 and gated to
  allocation-owning untracked pointers; deinit requires a Deinitable
  element), and `unsafe_free()` (`free()` stays accepted as upstream's
  deprecated bridge). Writes work through place-origin pointers with mutable
  provenance too, reusing the `ptr[] = v` store lowering; chained
  `p.unsafe_offset(i).unsafe_write(x)` receivers store through a synthetic
  binding. All lower onto existing MIR operations — no schema changes.

- Empty-subscript pointer dereference (nightly §4): `ptr[]` reads, and
  `ptr[] = e` / `ptr[] += e` write, the pointee — offset-0 access on heap
  pointers and direct pointee access on `Pointer(to=place)` handles. The
  parser emits a dedicated marker for empty brackets (grammar: the subscript
  argument list may now be empty), so a source `p[None]` can never pose as a
  dereference, and a non-pointer receiver rejects with a contextual
  empty-subscript error instead of dispatching an accessor.

- Pointer naming and origin unification (nightly §4, first slice): `Pointer` is
  now the canonical spelling of the builtin pointer type — type display,
  runtime value display, and every checker/VM diagnostic say `Pointer`, while
  `UnsafePointer` stays accepted as upstream's deprecated alias (internal
  callable mangling intentionally keeps the stable `UnsafePointer$…` identity).
  The internal `Legacy` pointer provenance is retired: the one-argument
  `Pointer[T]`/`UnsafePointer[T]` spelling now resolves to
  `MutUntrackedOrigin`, the origin of heap allocations, so heap pointers type
  as `Pointer[T, MutUntrackedOrigin]` exactly like upstream's `unsafe_alloc`
  result. New `MutPointer`/`ImmPointer` aliases validate a statically known
  origin mutability and reject a mismatch; `Pointer(to=place)` joins
  `UnsafePointer(to=place)` as the place-pointer constructor spelling; and
  writes through any statically immutable provenance (untracked, unsafe-any,
  or static, in addition to the existing place/param rejections) now reject at
  the checker.

- Lifecycle-predicate rename alignment (post-pin follow-up to nightly §0):
  the comptime predicates are now spelled `IsTriviallyMovable[T]`/
  `IsTriviallyCopyable[T]`/`IsTriviallyDeinitable[T]`, matching upstream
  `22b5036987` (one day after the `ae386d1b204` audit pin), which
  hard-renamed them with no deprecated aliases — the pre-rename `Trivially*`
  spellings no longer resolve anywhere (compiler recognition, `std.traits`
  exports, diagnostics). The same upstream change made
  `conforms_to(T, TrivialRegisterPassable)` a sufficient first disjunct:
  a declared `TrivialRegisterPassable` conformance or parameter bound now
  satisfies the predicates ahead of the structural check (which continues to
  prove primitives and ordinary structs). The shallow builtin-marker
  `conforms_to` default is deliberately not consulted for that disjunct.

- Fixed-size `Array[T, length]` and the list-display retarget (nightly §3): a
  new prelude-exported, self-hosted `std.collections.array.Array` declares
  conditional `Copyable`/`Movable`/`Deinitable`/`Equatable`/`Iterable`/
  `IterableOwned`/`Writable` conformances over a `T: AnyType` element and an
  `Int` value parameter, with keyword `fill`/`copy:`/`deinit move:`
  construction, by-reference `__getitem__` (no `__setitem__`, matching the
  audited head), `__len__`/`__eq__`/`__ne__`/`__contains__`, and borrowed plus
  owned iteration; it is neither `ImplicitlyCopyable` nor `Defaultable`. An
  uncontextualized `[1, 2, 3]` now materializes as `Array[Int, 3]` through a
  single nominal variadic literal-constructor call (a new
  `ConstructArrayLiteral` checked fact — no new MIR instructions); an expected
  type with a list-literal constructor (notably `List[T]`) still controls
  contextual materialization, assignment targets now count as expected context,
  and comprehensions still produce `List`. Supporting generalizations: value
  parameters resolve symbolically in struct bodies and infer from argument
  types, generic-struct constructors accept keyword arguments, constructor
  declarations carry their struct's compile-time parameters for value-param
  reification, same-type lifecycle constructors inherit the source's reified
  value parameters, and `unify` solves through reference patterns. Plain
  subscript assignment on a receiver with no `__setitem__` now writes through
  a mutable-reference-returning `__getitem__` (upstream Array's contract) and
  lowers to a direct reference write. Recorded subset gaps: Array's
  `Hashable`, `unsafe_ptr`, `__getitem_param__`, `uninitialized:`
  construction, `deinit_with`, and expected-type context for displays inside
  tuple-unpack right-hand sides and `Variant` constructor arguments.

- Repeated declaration constraints and generic comptime aliases (nightly §1
  follow-up): every declaration family that accepts a trailing `where` clause —
  functions, methods, structs, trait requirements, associated and trait
  comptime members, and comptime declarations — now retains its full clause
  list (`where (c1, "m1") where (c2, "m2")`) through a plural checked
  constraint contract; each clause validates independently and the first
  failing clause reports its own retained message. Per-trait
  conditional-conformance conditions stay single-clause. Generic top-level
  `comptime Alias[params]: Type where ... = ...` declarations lower once into
  a checked alias registry (classified parameters plus the symbolic template
  machinery shared with parameterized associated members) and expand per
  application in any type position — directly, through another alias, or as a
  generic argument, including across module imports — validating arity,
  bounds, defaults, and the alias's own clauses through the same contract as
  a struct application. Bare primitive names (`Int`, `UInt`, `Bool`,
  `Float64`) now resolve as types in expression-derived type positions such
  as alias and associated-member bodies. Recorded subset gaps: value-bodied
  generic aliases, origin-parameterized aliases, function-body generic
  aliases, and constructor-through-alias calls reject with contextual
  diagnostics.

- Mojo dev-branch parity catch-up (nightly §1): keyword collectors now require
  canonical `var **kwargs` syntax in declarations and function types (bare
  `**kwargs` rejects), and the checked/lowered callable identity retains a
  distinct keyword-variadic slot across free, generic, method, static, bounded,
  and indirect calls. The source linker now rejects two different explicit
  imports binding one local name and diagnoses exact canonical self-imports,
  while preserving idempotent imports, prelude shadowing, and real mutual
  cycles through provisional exports. Zero-step ranges are empty in direct
  compile-time unrolling, VM-backed CTFE, and nominal runtime iteration. The
  existing rejection of declarations differing only by `imm`/`mut` is now a
  differential conformance pin.

- Trailing `(condition, "message")` constraints retain the diagnostic in the
  checked algebra across functions/methods, structs, conditional conformances,
  associated and trait comptime members, and non-generic top-level comptime
  declarations. Messages survive specialization, concrete associated-member
  projection, conformance/lifecycle failure, and origin-mutability inference,
  but remain irrelevant to generic identity and logical implication. The
  future-syntax identifiers `class`, `del`, `match`, and `yield` are rejected
  contextually as free or nested function names without becoming lexer
  keywords; ordinary variable/parameter use and current method spellings remain
  legal. Repeated trailing constraints and generic top-level comptime aliases
  are now recorded separately instead of being overstated as implemented.

- Lifecycle canonicalization (Mojo parity catch-up, slice B / nightly §0):
  the internal vocabulary is now `Deinitable` + `__deinit__` end to end
  (builtin-trait registry, checker capability queries and diagnostics, MIR
  drop commentary, the VM's destructor-symbol lookup, bundled stdlib,
  fixtures, tests, and docs); the upstream-deprecated `ImplicitlyDeletable`
  and `__del__` spellings stay accepted and normalize at parse time (the
  `read` → `imm` precedent), pinned by a dedicated compat fixture. A missed
  normalization path fails loudly as `UnknownTrait` because `BUILTIN_TRAITS`
  lists only the canonical name. Declared conditional `Movable` conformance
  is now effective: `Movable where False` rejects `^` transfers, `var`
  parameters and receivers, and move/copy captures, while `deinit`
  consumption (destructors and named destructors) stays legal and Copyable
  pass-by-value is untouched. Added the `TriviallyMovable[T]`/
  `TriviallyCopyable[T]`/`TriviallyDeinitable[T]` comptime predicates
  (semantics pinned from the audited head's `std/traits/*.mojo`: base
  capability plus a compiler-generated lifecycle operation with recursively
  trivial fields), usable in comptime control/bindings, `where` clauses, and
  conformance conditions — and rejected as bounds. New docstring-only
  `std.traits`/`std.origin` module homes export the builtin identities
  (named, aliased, and wildcard imports resolve; unknown names stay
  `NameNotFound`), mirroring the audited upstream export surface.


- Extension alignment sweep (Mojo parity catch-up, slice A): Mojito now
  accepts what the audited Mojo head accepts within its subset — extensions
  remain tolerable only as upstream-tracked deprecation bridges or as cited
  implementations of features on Mojo's own roadmap/proposals — and every
  recorded acceptance in neither category became a rejection with a
  contextual diagnostic plus pinning fixtures. Removed acceptances: the legacy
  `unified {...}` capture spelling (parse error); the bare `move:`
  initializer parameter (migration diagnostic naming
  `__init__(out self, *, deinit move: Self)`); the competing
  positional/keyword-only `__setitem__` overload pair (declaration-time
  rejection — lone setters of either shape still work); `def(...)`-typed
  struct fields and collection elements, including `capturing[...]` storage
  and display-inferred callable elements (the FieldInvocation machinery
  remains internal-only); materializing an explicit Origin specialization of
  a capturing nested function (direct specialized calls still execute); and
  binding a capturing closure to an unqualified `def(...)` value contract
  (the checker now hints the `capturing[...]` spelling; comptime callable
  bounds still ground capturing values, matching upstream). `SIMDSize` stays
  accepted because the audited head still ships it as a deprecated alias.
  The bare `objs[0](args)` element-call spelling is recorded as a subset gap
  with a parenthesization-hint diagnostic; real element-call dispatch is a
  new roadmap item. `conformance/parity.tsv` rows moved from `divergence`
  to `match` accordingly.

### Added

- Cross-call transfer residues closed — every call shape now replays
  loan-transfer effects, ending the MIR-schema-prerequisite work. Checked
  function types carry identity-transparent inferred effects, replayed when
  a `def` value is called indirectly; callable-struct calls replay
  `Struct.__call__`; overloaded call sites replay the shared entry (a
  previously silent gap); abstract trait dispatch replays the union over
  conforming implementations; and higher-order bodies calling through their
  own callable parameters record call-through residues that each call site
  resolves against the concrete callable, composing through forwarding
  chains. The capture channel records concrete `Bound`-destination effects
  for stores through captured owners (including enclosing locals, closing a
  soundness hole where a closure-local reference stored into a captured
  local crashed the VM at runtime), stored closures loan their reference
  captures' owners while the storage lives, and stored callables invoke
  through the new field-invocation channel (`holder.callback(1)`, capturing
  environments included) — which also fixed the pre-existing
  "checked nominal subscript receiver is None" crash and a phantom-variable
  lowering bug that made a `def` name unusable as a value after appearing
  in a list display. Transfer destinations are interior-precise:
  `EstablishLoans` carries a destination domain, sibling fields keep
  independent generations, and rebinding the exact interior place releases
  its transferred loans.

- CPU Layout and LayoutTensor semantics. A new bundled `layout` package
  (import-only, never prelude) self-hosts the CPU layout core: a flat
  rank-≤4 `IntTuple`, `Layout` with `row_major`/`col_major` factories,
  `rank`/`size`/`cosize`, equality, printing, and the callable
  coordinate-to-linear mapping via `def(IntTuple) -> Int` conformance;
  and `LayoutTensor[dtype: DType, layout: Layout]`, a layout-aware view
  over a caller-managed `UnsafePointer[Scalar[dtype]]` buffer with
  rank-1/rank-2 indexing, `size()`/`dim(i)`, and write-through
  assignment. Underneath it, two new compile-time parameter forms
  monomorphize their declarations before checking (no MIR schema
  change): `[dtype: DType]` value parameters on defs and structs
  (`Scalar[dtype]`/`SIMD[dtype, w]` positions resolve concretely per
  application), and struct-typed value parameters on structs
  (`[layout: Layout]`) whose arguments — constructor or static-method
  calls like `Layout.row_major(2, 3)` — evaluate through VM-backed CTFE
  and freeze as `CtValue::Struct` specialization keys (fieldwise
  construction and recursively pointer-free fields required; frozen
  field reads fold to constants; `comptime L = Layout.row_major(2, 3)`
  works as a module constant too). Struct monomorphization generalizes
  from type packs to mixed scalar/dtype/struct value declarations with
  retained `mut`/Origin binders, and value parameters now bake into
  signature/field type positions. Also fixed en route: overloaded
  methods with `Scalar[DType.x]`/`SIMD[...]`-annotated parameters
  produced mismatched overload symbols between declaration and call
  sides (now both use the canonical checked spelling). Deferred and
  documented: origin-parameterized borrowed tensor views, the GPU
  surface, tile/slice views, SIMD load/store, the layout algebra, and
  recursive IntTuple.

- SIMD semantic completion. `SIMDLength` is now the width-parameter
  spelling (`SIMDSize` stays a deprecated, never-emitted compatibility
  alias, and the comptime elaborator now classifies both as value
  parameters). Explicit scalar/SIMD construction converts runtime values —
  integers wrap to any integer width and convert to float lanes, floats
  adjust precision, and any `Intable` value (bounded parameter or
  conforming struct, whose builtin-trait conformance the checker now
  recognizes for declaring structs) constructs integer scalars through
  `__int__` — while implicit contexts stay literal-exact, and
  `Scalar[DType.x](arg)` now lowers as ordinary width-1 SIMD construction
  (previously a MIR-verify failure for non-canonical dtypes). New
  CPU-visible operations: unary negation, `cast[DType.target]()`
  (elementwise; float→int truncates toward zero; bool casts deferred),
  bool-mask `select(t, f)`, the `reduce_add/mul/min/max` and mask
  `reduce_and/or` reductions (collapsing to canonical native scalars),
  and `shuffle[*mask]()` with compile-time lane indices — the latter two
  compile-time-payload methods ride new verified `SimdCast`/`SimdShuffle`
  MIR instructions. A `def` may take its width as a `[w: SIMDLength]`
  value parameter: each call monomorphizes (value parameters now bake
  into signature type positions), so an invalid bound width rejects
  during checked elaboration, with a MIR-verify width backstop for
  assembled artifacts. The runtime `Byte(Int)` conversions unlock
  `Codepoint.from_u32(scalar)` — Int-based, `None` for
  negatives/surrogates/out-of-range — whose character text is
  UTF-8-encoded in ordinary library code.

- String result APIs, non-raising slicing, and the un-annotated binding
  default. The nominal String's slice is now non-raising byte-wise library
  code with Python-normalized bounds and strides, matching the builtin
  literal slice — a cut inside a multibyte UTF-8 sequence keeps the raw
  bytes and the struct-to-literal read-back renders them lossily instead
  of erroring. New byte-offset result APIs: `find`/`rfind` (`-1` when
  absent; the empty needle matches at the search start/end),
  `startswith`/`endswith`, and `split(sep)` returning eager owned
  `List[String]` pieces (raising on an empty separator, and the first
  stdlib use of `List[String]`). With slicing and result parity in place,
  an un-annotated `var s = "lit"` binding now materializes the nominal
  String through the `@implicit` literal constructor, as in current Mojo;
  aggregate elements, `comptime` bindings, and bare literal expressions
  stay `StringLiteral`, and seams without the linked stdlib struct keep
  the literal default.

- The StringLiteral/String type split. Source `String` annotations now
  resolve to the self-hosted nominal String struct through the ordinary
  prelude/linker path (the parser no longer treats `String` as a builtin
  type keyword), while string literals and literal-only operations stay
  on the compile-time `StringLiteral` type — spellable in annotations,
  mirroring `IntLiteral`, with `Ty::StringLiteral` as the renamed checker
  variant. A literal converts wherever the nominal String is expected via
  the struct's new `@implicit` literal constructor: bindings, arguments,
  returns, tuple-display and collection elements, fieldwise and
  specialized-pack constructor fields, and mixed operator operands
  (`"a" + s` and `s == "b"` normalize onto the struct's migrated
  `__add__`/`__iadd__`/`__contains__`/comparison dunders). Conversion
  retargeting makes `String(x)` stringify, `input()`, `repr(x)`, and
  `.format(...)` produce nominal String values through the VM's
  materialize bridge; `Error(msg)`/`raise` and the Writer `write_string`
  contract accept either spelling (a nominally-declared `write_string`
  receives a materialized payload). StringDict kwargs keys and other
  literal-typed internals are respelled `StringLiteral` with an unchanged
  VM ABI. Overload symbols keep the stable `String` spelling for both
  types, so overloads differing only in that pair are rejected as
  redeclarations. Un-annotated `var s = "lit"` bindings deliberately stay
  `StringLiteral` (a recorded divergence and follow-up); unlinked seam
  programs reject bare `String` annotations explicitly.

### Fixed

- Consuming a borrowed loop binding (a `var`-convention argument such as
  `list.append(element)` inside `for element in source`) now runs the
  referent's `__copyinit__` instead of aliasing its owning storage — a
  pre-existing double free for any pointer-owning Copyable struct,
  exposed by nominal String keys flowing through dict iteration.

- Variant projections spelled with a struct-typed alternative
  (`v[String]`) resolve and lower like the type-token spelling, and
  implicit conversions now participate in constructor overload scoring
  (ranked below direct coercions) and in specialized heterogeneous pack
  argument binding.

- Variant payloads own independent values: construction, `set`, and
  `replace` deep-copy a lifecycle payload into the variant, and a
  projection value read copies a Copyable payload out of the variant's
  storage (keeping the owner alive through the copy) instead of
  aliasing a buffer the owner's drop then frees.

- Lazy captured t-strings. A `t"…"` now produces the self-hosted prelude
  `TString[*Ts: Movable & Writable]` (stdlib `std.format.tstring`) instead
  of an eagerly concatenated builtin `String`: the whole-program compiler's
  discovery fixpoint types each occurrence, materializes the concrete
  variadic specialization, and rewrites the expression into its
  construction, whose interleaved pack captures literal segments and
  interpolation snapshots at creation. Formatting defers to Writable
  `write_to`, so `print(t"…")` and explicit `String(t"…")` flow through
  the ordinary machinery, and t-strings nest. Capture is by typed value
  snapshot — copyable interpolations copy in, non-Copyable places
  snapshot as creation-time formatted strings — a documented deviation
  from real Mojo's borrow-holding `TString` (its exclusivity rejects
  mutating a captured value before use; Mojito prints the snapshot).
  Assigning a t-string to a `String` annotation is now a type error, and
  `TString` values reject copy/concatenation/equality. The raw
  parse-then-check seam and retained abstract generic bodies keep an
  output-identical eager-concatenation fallback, and generic applications
  inside interpolations now monomorphize (previously a latent gap).

- Grapheme segmentation and a `Codepoint` result type for the
  self-hosted String. `s[codepoint=i]` now yields a prelude-exported
  `Codepoint` carrying the decoded scalar plus the character's text
  (captured through the struct-to-literal bridge): `Int(cp)` via
  `Intable`, scalar-ordered comparison and equality, Writable printing
  as the character, `is_ascii()`, and `utf8_byte_length()`; direct
  construction is rejected until runtime scalar conversions land.
  `s[grapheme=i]` returns the extended grapheme cluster as a `String`
  substring and `grapheme_count()` walks the whole buffer, both raising
  on out-of-range indexes and truncated UTF-8. Segmentation implements
  a documented UAX #29 subset — hand-maintained Control/Extend/
  SpacingMark essentials ranges, regional-indicator pairing, and fully
  arithmetic Hangul — with GB11 simplified to "never break after ZWJ"
  (common emoji ZWJ sequences join) and GB9b (Prepend) omitted.

- Self-hosted String core. The stdlib gains a nominal UTF-8 `String`
  (byte buffer over `UnsafePointer[Byte]`), constructed explicitly from
  a literal (`String("...")`); annotations and non-literal `String(x)`
  conversions keep the builtin compile-time string until the type-split
  migration. The struct supports byte-length `len`, copy/move/drop,
  byte-wise equality and ordering, DJB2 hashing (Dict/HashDict keys),
  `print`/`repr` through Writable, explicit `s[byte=i]` and
  `s[codepoint=i]` access (pure UTF-8 leading-byte decode, raising
  bounds and validity errors; positional `s[i]` stays rejected), and
  boundary-checked contiguous slicing that raises on mid-sequence
  splits. En route, three general features landed: keyword subscripts
  (`x[name=i]` over value bases, dispatching keyword-only `__getitem__`
  overloads; named brackets over type names remain parameter
  application), keyword-only parameter names as part of overload
  identity, and scalar conversions (`Int`/`UInt`/`Bool`) accepting
  width-1 SIMD scalar aliases like `Byte`. Graphemes, a `Codepoint`
  type, lazy TString, and the literal-operations migration are recorded
  follow-ups.

- Cross-call transfer hardening. Transfer-effect visibility is now
  declaration-order independent: the checker reruns with the prior
  round's committed effects whenever a call site observed a stale callee
  entry, so a method calling a later-declared (or mutually recursive)
  storing method carries its effect to every caller, converging in one
  round for programs without order-sensitive effects. The store-outward
  escape rule now also covers stores inside nested `def`s through
  captured `self`/parameters (previously a diagnosed bypass that reached
  a stale-frame crash at runtime), unpack-into-place targets (the
  transferred-tuple shape now rejects with the escape diagnostic), and —
  through ordinary method selection — user in-place dunders
  (`sink += carrier` replays `__iadd__`'s transfer effects). Remaining
  residues (indirect-call effects, interior-precise destinations, the
  capture-effect channel) are recorded on the roadmap.

- Owned iteration of linear elements. `for var item in xs^` and owned
  comprehensions now accept a `List` of non-`ImplicitlyDeletable` elements
  when every element is transferred by guaranteed exhaustion: the bundled
  owned iterator's `ImplicitlyDeletable` gates are lifted, and only
  abandoning control-flow paths are rejected — `break`/`return`/`raise`, a
  raising call whose handler sits outside the loop (a `try` inside the body
  contains its error), and comprehension filters over a linear binder — each
  naming the element's `@explicit_destroy` obligation. With linear elements
  the iterator itself is linear (no `__del__`), so the exhaustion edge
  consumes it through the checker-selected `_finish(deinit self)` named
  destructor, an ordinary method call that frees the buffer as visible
  library code; a user-defined linear owned iterator without a finisher or
  unconditional destructor is rejected contextually. Deletable-element
  behavior is unchanged, including residual drops on early exit.

- Cross-call reference lifecycle and loan transfer. A callee's accepted
  store of a loan-carrying value into `self` or a `mut`/`ref` parameter
  is now a checker-recorded transfer effect replayed at every call:
  the store-outward escape rule fires across the call boundary, the
  caller's own escape analysis sees callee-installed loans, wrapper
  callables carry effects transitively outward, and MIR installs the
  transferred loans on the destination actual so ownership analysis
  rejects mutating or dropping the loan root while the stored alias
  lives (naming both variables) and keeps a borrowed source alive while
  a carrier collection holds a reference to it. Borrowed (`mut`/`ref`)
  parameter sources loan the actual's own storage; owned parameters only
  forward loans their moved values carry. The bundled
  `List.append`/`insert`/`__setitem__` are seeded, so appending a
  reference-carrying struct to a `List` is fully tracked. Alongside:
  value writes through a `ref`-typed field run the referent's copy
  lifecycle (non-`Copyable` writes reject, explicit `^` transfer stays
  raw), chained element-referent subscripts (`sink[0].value[0]`) verify
  and execute, and `capturing[_]` fields/elements retain the stored
  closure's concrete capture-origin set so escaping captured locals
  reject. v1 limits (permissive, recorded on the roadmap): effects use
  declaration-order visibility, indirect calls and abstract dispatch
  carry no effects, and destinations are tracked at root granularity.

- Reference-carrier lowering fixes. A bare struct name as an explicit
  compile-time type argument (`List[RefBox]()`) no longer emits a
  phantom runtime value register, so locals binding collections of
  origin-erased carrier structs construct and execute instead of failing
  MIR invariants. Assignment through a `ref`-typed field behind an
  aliased root now writes through the stored handle into the referent
  (cross-frame scalar write-through executes), and returned aggregates,
  `mut` writebacks, and cross-frame stores re-root their interior
  reference handles before the owning frame dies. Capturing closures no
  longer erase their environment into plain `def(...)` storage: fields
  and collection elements reject the coercion (with the environment
  shown in the diagnostic) while `capturing[...]`-annotated storage and
  call-position downward funargs are unchanged. The remaining
  call-boundary loan/lifecycle residue is consolidated into one roadmap
  item.

- Reference-escape analysis beyond returns. Stores into storage that
  outlives the frame — fields of `self`, parameter-rooted places, and
  `ref`-field rebinds — now reject frame-locally rooted loans at check
  time ("stored reference escapes storage outside its declared origin"),
  closing a checker-accepted use-after-free; parameter-rooted loans store
  outward freely, and both escape-context builders now include variadic
  collector parameters. The collection-store and closure edges are pinned:
  `List[ref T]` offers no handle-installing channel, and a stored closure
  can never be invoked after its captured referent dies. Reference result
  signatures accept the declaration-level immutable-origin cast
  (`ref[Origin[mut=False].cast_from[o]] T`, upgrade direction rejected),
  and mapping key iteration over Dict, HashDict, and StringDict now
  yields immutable key references (Mojo parity; writes through a `for
  ref` key binding reject, reads stay live borrowed references).

- Bound-generic monomorphization, Stage E: erased-dispatch retirement
  confirmed and the residue re-pinned through the authoritative pipeline.
  New Compiler-driven witnesses assert that a `comptime for`-conflict-
  retained template keeps the `__iterator_dispatch` protocol and its
  `CopyIteratorReference` adapter in MIR and executes them end to end, and
  that a bound generic used as a function value retains its template and
  invokes through runtime retargeting; the deliberate raw-seam machinery
  pins are annotated so a schema-freeze audit reads them correctly. The
  roadmap item is closed: erased dispatch survives only for the designed
  residue (function values/indirect calls, overloaded generic names,
  generic methods, comptime-class inferred calls, open instantiations,
  conflicting unrolled occurrences, and abstract-body pre-checks), and the
  six `mir::verify` abstract-dispatch witnesses to re-confirm at the
  backend-ready MIR checkpoint are now named on that roadmap item.

- Bound-generic monomorphization, Stage D: inferred applications. The
  compiler's pipeline now iterates discover→elaborate→check to a fixpoint,
  replaying each closed checker-recorded generic instantiation
  (`DefSpecializationRequest`) at its exact call occurrence, so inferred
  calls like `first_or(range(3, 7), -1)` run through concrete clones with
  no erased dispatch at the call site. A request can only upgrade a call:
  misaligned or non-closed arguments, occurrence conflicts from `comptime
  for` unrolling, and drifted spans all keep the abstract path, and a hard
  round cap reports inferred polymorphic recursion as the new
  `CompilerError::SpecializationDivergence` diagnostic. A retained
  bound-generic template now precedes its clones so an inferred recursive
  clone can reference it under sequential name binding. The stage-composed
  test seam stays request-free (Compiler-only machinery). Landing this
  surfaced and fixed three latent concrete-path bugs that erased dispatch
  had been masking: the drops pass now retains a direct ref-returning
  method call's receiver storage until the handle's last register use (a
  use-after-free reachable in plain concrete code); a ref-returning
  method's receiver — including a read receiver — passes as a reference so
  the returned handle roots in the caller's live slot; concrete scalar
  receivers resolve the rounding dunders (`__floor__`/`__ceil__`/
  `__trunc__`/`__ceildiv__`) to the same VM intrinsic the abstract
  Floorable-family dispatch uses; and type-binding substitution now
  rewrites the parser's bare-identifier (`Tuple[T, T]`) value-encoded
  arguments in annotations, fixing explicit applications of generics with
  parameterized signature types.

- Bound-generic monomorphization, Stages A-C. Explicit concrete generic
  applications now clone and re-check the body with the concrete type
  substituted, matching real Mojo's per-instantiation model. Every comptime
  specialization bakes its concrete type arguments into the clone
  (annotations, compile-time argument lists, and constructor heads) and drops
  them from the residual signature and rewritten calls, with each dropped
  parameter's trait bounds enforced at the requesting call via the new
  contextual `ComptimeError::GenericBound`; this fixes the explicit
  non-generic type-argument bug (`pick[Plain]`, `first_or[Range]`). Plain
  trait-bound generics with a unique top-level name join the specialization
  registry with soft resolution: unresolvable references (inferred calls,
  symbolic arguments, function values) stay on the retained template's
  abstract erased-dispatch path, and a dead template keeps its abstract
  pre-check. Generic-bound reference iteration now works through explicit
  application: the clone iterates concretely with ordinary borrowed loans.
  The checker also records every resolved generic instantiation
  (`CheckedProgram::generic_instantiations`, groundwork for monomorphizing
  inferred calls), `Ty::Assoc` copyability now derives from declared
  associated-member bounds instead of defaulting to copyable (the stdlib
  `Iterable.Element` bound strengthened to `Copyable & Movable`
  accordingly), and the stage-composed test seam's `VmBackend::run` enforces
  the pre-drop ownership analysis, closing the raw-path divergence.

- Mapping invalidation and borrowed-iteration safety (core). The Dict,
  HashDict, and StringDict key iterator (`_DictKeyIter`) now borrows the
  entries list through a parametric-mut struct origin and yields key
  references, replacing the snapshot copy, and every bundled borrowed
  iterator declares its yielded reference at
  `_get_owned_interior["element"]` granularity — the checker derives each
  borrowed source loan's granularity from that declared projection, retiring
  the List/Set/Dict collection-name whitelist from the production path (an
  unregistered-collection shim remains for the focused checker). Mapping
  mutation during iteration is now a defined, lazily rejected error across
  all three mappings — previously HashDict/StringDict were unprotected —
  while `d[key]` value reads stay legal through the sibling `value`
  generation and `keys`/`values`/`items` views remain eager snapshots by
  design. Signature lowering gained the two pieces the derivation needed: an
  uncarried origin parameter lowers to its checked semantic binder rather
  than an inferred union, and an interior projection off a parametric-mut
  origin parameter carries that parameter's declared mutability. New coverage
  pins the mapping rejections (statement and comprehension), value reads and
  coexisting shared iterations, the lazy-discard model, and the sound
  reference-escape rejections (returning a loop reference or a loan-carrying
  closure, mutating a source under a manually held borrowing iterator).
  Deferred to their own roadmap items: generic-bound reference iteration and
  store-outward escape analysis.

### Changed

- Developer infrastructure: the whole-corpus fixture sweeps are now one
  generated test per fixture in the `tests/corpus_test.rs` binary
  (libtest-mimic, the project's first dev-dependency), preserving each sweep's
  distinct pipeline entry path while letting the test runner schedule fixture
  compiles across cores; `scripts/check` runs the suite through
  `cargo nextest run`, and the `quick` nextest profile excludes the corpus
  binary for the iteration loop.

### Added

- Generic reference-yielding collection iteration. The bundled List and Set
  iterators now genuinely borrow their source and yield element references:
  `_ListIter[iterable_mut: Bool, //, T, iterable_origin:
  Origin[mut=iterable_mut]]` holds `ref[iterable_origin] List[T]` and its
  `__next__` returns `ref[iterable_origin] T`, with the reference's mutability
  resolved from the source at each loop or comprehension site — a mutable
  source yields writable `for ref` handles (named or temporary sources alike)
  while a read parameter yields immutable ones. The List-only `for ref` index
  desugaring is gone: reference iteration runs the ordinary checked
  `__iter__`/`__next__` protocol end-to-end, Set reference iteration works for
  the first time (through its delegated borrowed `_ListIter`, two borrow
  frames deep), and the iterator's source loans re-establish on each yielded
  binding so a structural invalidation names the user's variable. A `ref`
  loop target over an abstract generic `Iterable` bound is now rejected with a
  clear error (the abstract `Iterator.__next__` contract yields `Element`
  values; the previous behavior silently mutated a per-iteration copy).
  Compiler capabilities that landed with the migration: top-level struct and
  trait declarations register order-independently (shells, member types, and
  method signatures precede body checking, so same-module structs may
  reference each other in either order — with by-value self-containment now
  rejected explicitly), an infer-only `Bool` parameter binding a sibling
  origin's `mut=` erases from a struct's runtime parameters like the origin
  itself, generic substitution descends into `ref` field referents, and the
  specialization conformance oracle registers struct names before resolving
  field types. Mapping iterators still snapshot entries and yield copies —
  mapping invalidation is the next roadmap item.

- The bundled borrowed `Iterable` protocol is now origin-parameterized, as in
  current Mojo: `std.iterable` declares
  `comptime IteratorType[iterable_mut: Bool, //, iterable_origin:
  Origin[mut=iterable_mut]]: Iterator` with
  `def __iter__(ref self) -> Self.IteratorType[origin_of(self)]`, replacing the
  legacy monomorphic `Iter` member, and Range, List, Set, Dict, HashDict, and
  StringDict conform through the parameterized member with the
  application-spelled `ref self` return. The bundled conformers erase the
  origin in their member templates, so borrowed iterators still yield element
  copies; borrowing the source through the origin parameter and yielding
  references is the next generic-reference-iteration subtask. Two compiler
  gaps closed with the migration: trait conformance now enforces a
  parameterized associated member's declared bound (instantiating the
  definition's template with placeholder explicit arguments, discharging
  conditional cases through the struct's conformance assumption, and keeping
  the arity-only contract when an explicit value parameter has no fabricable
  witness), and generic borrowed `__iter__` dispatch now reaches a conformer
  whose borrowed receiver spelling (`self` vs `ref self`) differs from the
  abstract dispatch symbol — previously a `RuntimeError` for an overloaded
  borrowed/owned `__iter__` pair like migrated List's.

- Borrowed iteration sources now lower uniformly, in `for` statements and
  comprehensions alike: every borrowed named source is bound as a genuine
  reference (`MakeRef`) into a retained-source slot, the iterator object is
  normalized into a distinct slot, and whole-source versus interior borrowing
  is expressed only as loan granularity — a whole-place shared loan for a
  user iterable, an interior `element` generation for a concrete
  List/Set/Dict place — re-established on the long-lived iterator slot. The
  former collection-only `LoadPlace` single-slot bridge is gone.
  Comprehensions gain the statement loop's semantics they previously lacked: a
  comprehension over a named user iterable borrows its source instead of
  copying it (one `__del__`, source usable afterward), mutating a borrowed
  source mid-comprehension is now rejected as a loan conflict or interior
  invalidation (previously a silent copy permitted it), and a comprehension
  over a borrowed user-iterable temporary no longer leaks the source — it is
  retained in its own slot and destroyed exactly once, after the
  comprehension.
- Loop and comprehension targets now model their binding convention —
  unadorned (immutable), `var`, and `ref` — independently of whether the source
  is borrowed (`for x in xs`) or consumed (`for x in xs^`). Each of the six
  combinations carries an explicit checked requirement: a `var` target moves an
  owned result or lifecycle-copies a yielded reference (requiring an
  `ImplicitlyCopyable` referent); an unadorned or `ref` target binds an owned
  result directly into per-iteration storage (requiring a droppable element —
  `ImplicitlyDeletable` or `Copyable` — dropped each iteration) or retains a
  yielded reference handle to read/write through the borrowed referent. The raw `__next__` result is retained in a
  compiler-owned slot and adapted to the target only on the yielded edge, so
  moves and copies never run on the `StopIteration` path. Mutating an immutable
  target, and a copying `var` target over a non-`ImplicitlyCopyable` reference
  result, are rejected with contextual checker errors.
- Trait conformance now accepts Mojo's directional `__next__` result
  refinement: a concrete `ref[o] T` result may implement an abstract value
  result `T` only when the referent matches exactly and `T` is proven
  `Copyable`. The reverse direction, mismatched referents, and non-`Copyable` values
  remain errors. Abstract method calls and generic loop advancement retain an
  explicit checked/HIR/MIR adapter; after runtime retargeting, the VM consults
  the concrete declaration ABI and performs the reference read plus lifecycle
  copy. Caller and just-returned iterator frames remain reachable while user
  copy code runs, so a `Copyable` element containing reference handles can read
  its nested referents. Registered `Iterator` declarations are now authoritative
  instead of being bypassed by the focused checker's builtin compatibility
  marker.
- Structural iterator selection now preserves a reference-returning
  `__next__` as an origin-bearing `Ty::Ref` result through the checked protocol,
  HIR, typed MIR, verification, and VM dispatch. Previously the VM happened to
  carry the reference handle through a register statically typed as the
  referent, leaving the checked/MIR boundary inconsistent even though simple
  reference-yielding loops executed.
- A method may now return the *dereference* of an origin-bearing pointer field
  whose origin is a struct/callable parameter (`def get(self) -> ref[o] Int:
  return self.p[0]` on `struct Borrow[o: Origin[...]]` with `var p:
  UnsafePointer[Int, Self.o]`). `UnsafePointer(to=v)` is a runtime handle straight
  at `v`, so the returned `ref[o]` re-roots at the single pointee at the return
  boundary and the VM forwards the pointer's offset-0 index as the identity deref
  of that pointee — an immutable origin reads, a mutable origin writes through the
  caller's storage. Previously the checker rejected this shape (`escapes storage`)
  because the residual offset-0 index had no runtime forwarding.
- A `for` loop over a user-defined iterator whose `__next__` returns a *reference*
  into the borrowed source now executes: the yielded reference flows through the
  loop as a handle. The loop invokes `__iter__`/`__next__` with the loop frame
  reachable — previously the synchronous protocol-call path drove the callee with
  its caller popped off the frame stack, so a user iterator holding a `ref` into the
  loop frame could not dereference it (`vm: stale reference to frame N`) — and a
  borrowed `__iter__(ref self)` receives a `ref self` handle so the iterator's
  borrow roots at the live loop frame. Supported for an owned-temporary source
  (`for x in Numbers(3)`), retained and dropped exactly once after the loop. A
  *named* source (`for x in nums`) is now borrowed rather than copied: the source
  slot binds a genuine reference (`MakeRef`) and the whole-source dependency is
  recorded as a shared loan on the iterator, so the source is not copied, stays live
  through the loop without the `KeepAlive` liveness hack, and mutating it during
  iteration (a `mut self` call, reassignment, …) is rejected as a loan conflict.
- A method may now return a `ref[origin] T` field or binding whose origin is a
  struct/callable origin *parameter* (for example `def get(self) -> ref[o] Int:
  return self.slot` on `struct Cell[o: Origin[...]]`). The stored handle already
  names its borrowed region, so returning it stays within the declared origin;
  previously the return contract re-synthesized the handle's storage as a place
  rooted at the receiver and rejected it as an escape. Immutable origins yield a
  read-only borrow and a mutable origin returns a write-through handle to the
  caller's storage. This is a foundation piece for generic borrowed reference
  iteration.
- A method may now also return a reference obtained by *indexing/projecting
  through* a `ref[origin] <aggregate>` field (for example `def at(self, i: Int) ->
  ref[o] Int: return self.src[i]` on a `ref[o] List[Int]` field). The VM re-roots
  the returned handle at the borrowed storage — following the stored `ref`/pointer
  handle across frames, including through a `mut`/`ref self` receiver — so it
  survives the accessor frame instead of dangling (`vm: stale reference to frame
  N`). Dereferencing an origin-bearing *pointer* field and returning it
  (`self.p[0]`) remains rejected: its place lowering keeps an offset-0 index the
  runtime cannot yet forward.

### Fixed

- A reference returned from a struct method whose declared origin is a struct
  origin parameter, then bound to a `ref` local, now keeps its ultimate source
  alive instead of dangling (`invalid reference projection … on None`). The
  caller-side origin resolution maps the struct origin parameter to the origin the
  receiver's `ref[o]` field borrows (recorded at construction), so the returned
  reference records a loan on the owner and drop elaboration keeps it live while
  the reference is used — previously the abstract parameter was dropped by the loan
  machinery and the owner was freed early. A `mut self` reference-yielding accessor
  (`def take(mut self) -> ref[o] Int: return self.src[i]`) bound to `ref` locals
  now reads and writes through end-to-end.
- Reading a `ref[origin] <aggregate>` field's referent under a `mut self`/`ref
  self` receiver (subscript `self.src[i]`, `len(self.src)`, …) no longer fails with
  `vm: checked nominal subscript receiver is ref`. A borrowed receiver is a runtime
  alias, so the `LoadPlace` fast-path reached the field's stored handle but skipped
  the `ref`-typed post-dereference the by-value path applies; the value load now
  yields the referent under every receiver convention. (Value reads only — a
  *returned* reference bound to a `ref` local still needs its source loan, tracked
  as later borrowed-iteration work.)
- Borrowed iteration over a temporary that owns its storage no longer leaks the
  source. `for x in Numbers(3)` (or `for x in make_list()`) normalized the
  iterable to an iterator *in place*, overwriting the source in its only slot, so
  its `__del__` never ran and a borrowing iterator aliased freed storage. The
  borrowed-iteration source and iterator now occupy distinct slots (`GetIter`
  reads the source, writes the iterator into its own slot); the source stays live
  through the loop via a liveness anchor and is destroyed exactly once after it,
  including on early `break`/`return`. Owned iteration keeps the single slot
  (`__iter__(var self)` consumes the source), and concrete List/Set/Dict borrowed
  iteration is unchanged (its named place is retained by an external loan). This
  also gives a future origin-bearing iterator a live source to loan.

### Changed

- The bundled owned-iteration protocol now uses current Mojo's monomorphic
  `IteratorOwnedType`. `IterableOwned`'s associated iterator member (and `List`'s
  conformance) is renamed from the legacy `OwnedIter` to `IteratorOwnedType`; a
  consuming iterator owns its storage, so the member needs no origin parameter.
  The borrowed `Iterable` trait still uses the legacy monomorphic `Iter` member —
  migrating it to origin-parameterized `IteratorType[origin_of(self)]` needs
  self-origin resolution and lands with generic borrowed reference iteration.

### Added

- A parameterized associated-type application on a concrete struct base now
  resolves. A conformer may spell the application directly as its own return type
  (`def __iter__(ref self) -> Self.IteratorType[origin_of(self)]`): with the
  concrete struct substituted for `Self`, the indexed application routes through
  the struct's parameterized member instead of failing as a dependent type index.
  This is the faithful current-Mojo `__iter__` shape for the borrowed `Iterable`
  conformers. A generic free function returning `C.IteratorType[origin_of(c)]`
  remains later work (a declaration-time value-parameter-origin gap).

- Self-origin resolution for a parameterized associated type. A trait method's
  abstract `Self.IteratorType[origin_of(self)]` has no bound `self` place; its
  receiver origin now lowers to the symbolic `Origin::SelfParam` (the
  `Origin`-level analogue of the signature contract's `SigOrigin::Self_`), so the
  application carries its origin argument instead of collapsing to zero args. A
  conforming struct then resolves the origin-parameterized member concretely (the
  origin erasing from the runtime ABI like a pointer origin), so a requirement
  returning `Self.IteratorType[origin_of(self)]` is satisfiable and conformance
  succeeds. This is the self-origin prerequisite for migrating the borrowed
  `Iterable` protocol; the reference-yielding iteration runtime and the stdlib
  migration remain later work.

- Concrete parameterized-associated-type substitution. The checked `Ty::Assoc`
  now carries a parameterized application's arguments — `TyArg` gained a
  first-class `Origin` variant, so an origin argument participates in checked type
  identity while erasing from the runtime ABI like a `Ty::Pointer` origin. When a
  conforming struct instantiates a parameterized member, the application resolves
  concretely by substituting the arguments into the member's lowered template: a
  type-parameterized member (`C.Wrap[T]` → `List[T]`) resolves end-to-end through
  checked declarations, specialization, HIR, verified MIR, and the register VM.
  (An `origin_of(self)` origin argument now resolves too — see the self-origin
  entry above. Forwarding a value parameter into another parameterized struct
  remains blocked by a pre-existing generic value-forwarding gap.)

- Parameterized associated types (foundation). Trait and struct compile-time
  members now retain a parameter list — type, value, and origin parameters with
  the `//` infer-only boundary — so current Mojo's
  `comptime IteratorType[iterable_mut: Bool, //, iterable_origin:
  Origin[mut=iterable_mut]]: Iterator` parses and checks. A parameterized
  application such as `Self.IteratorType[origin_of(self)]` (spelled like a
  dependent index but naming a parameterized member) is recognized, validated
  against the declared explicit-parameter arity, and resolves to a symbolic
  associated type.

- Augmented assignment on a user-defined value now dispatches to its dedicated
  in-place dunder — `x += y` selects `__iadd__(mut self, y)` (and `__isub__`,
  `__imul__`, `__itruediv__`, `__ifloordiv__`, `__imod__`, `__ipow__`) as a
  checked `mut self` method call carried through checked HIR and verified MIR,
  mutating the receiver in place for variable, projected-field, and
  nominal-subscript-element targets. Mojo no longer falls back to the ordinary
  `__add__` family: a missing in-place dunder is a hard error, an immutable
  receiver and a mismatched right-hand side are rejected, and a raising in-place
  dunder participates in ordinary `try` handling. A nominal-subscript element
  dispatches through both getter paths — a value getter materializes the element
  into a mutable temporary and writes the result through `__setitem__`, while a
  mutable-reference getter applies the in-place dunder through the handle. Native
  scalar targets keep the builtin read-modify-write.

- Method-dispatched nominal `Index`, `Slice`, `MultiIndex`, and `MultiSet` now
  carry one complete
  checker-selected method-call contract through checked HIR and verified MIR:
  the exact target, executable result, and typed error; receiver and argument
  conventions and caller places, capture accesses, generic value arguments,
  reference-result origin,
  and setter write-back. Raising accessors participate in ordinary `try`
  handling; receiver/argument alias checks and persistent loans use the
  effective access convention. A reference-returning subscript is evaluated
  once into a hidden reference slot with its owner loans, so it can serve
  directly as a chained receiver or place, including through a reference-valued
  aggregate and when passed dynamically to a `mut`/`ref` parameter. Setter
  overload selection distinguishes positional and keyword-only `value` shapes,
  validates the actual right-hand-side type, and preserves source evaluation
  order (receiver, indices/bounds, then right-hand side); heterogeneous
  variadic Indexer positions normalize against their own selected element type.
  Ordinary index operands execute selected user `@implicit` conversions, while
  synthesized slice descriptors permit only descriptor-family widening and
  reject arbitrary user wrapping before MIR. Owned nominal element reads retain
  their selected accessor and copy lifecycle in verified MIR.
  A field projected below a nominal reference-returning accessor extends the
  one-evaluation hidden handle, so direct `ref` bindings and dynamic `mut`/`ref`
  actuals never flatten the accessor into a raw nominal index place. Reference
  returns use the same handoff, and subsequent Pointer, SIMD, and private Tuple
  indices retain typed projection metadata. The MIR verifier checks hidden
  handle slots, referent-versus-handle terminal types, dynamic index-register
  types, and projected element types while allowing analytical loan paths to
  retain nominal interior-origin projections.
  Augmented nominal subscripts retain call-local getter and setter conversions
  and effects without reevaluating the receiver or indices. A value getter
  follows the pinned receiver/raw-index, RHS, getter-conversion/getter,
  operator, setter-conversion/setter order and reloads a getter-mutated caller
  place before the setter. A mutable-reference getter instead establishes the
  lvalue before the RHS and writes directly through its handle without calling
  a setter. Keyword-only setter values retain their selected implicit
  conversion.
  Write-through assignment to a runtime reference preserves the handle slot's
  `ref` type, including free-function returns with union origins. Borrowing a
  reference-valued aggregate field now receives an outer `ref (ref T)` type,
  while projections through an existing reference capability preserve its
  mutability instead of manufacturing a mutable handle. Substituted local
  aliases likewise retain their checked `ref T` capability type even though no
  runtime handle is stored. Cloned declarations in sibling compile-time scopes
  receive distinct typed HIR/MIR slots, so heterogeneous specializations cannot
  overwrite one another's place type. Indexing `List[ref T]` peels the outer
  reference to the element slot before an augmented write or chained method
  call, so the operation reaches the stored referent instead of replacing its
  handle. Bare-name augmented assignment in a structured region resolves its
  checked binding identity for both halves, including same-spelled sibling
  `ref` declarations.
  Retained reference receiver/argument places are revalidated after every later
  index argument has run, without rereading ordinary copied arguments.
  Resolving a competing positional-only/keyword-only `value` overload pair is
  an explicit Mojito extension because the pinned nightly currently rejects
  that focused pair.
  MIR declarations retain receiver presence and exact fixed-parameter
  conventions, while verification checks concrete declaration contracts and
  the internal consistency of abstract trait-bound subscript results. Call-less indexing/slicing carries an explicit
  intrinsic family discriminator, so the VM never infers Tuple/pack, SIMD,
  pointer, compile-time-list, or String-slice semantics from a runtime value.
  The narrow nominally typed `Slice.indices()` result explicitly uses the
  private Tuple-storage bridge.

- Public `List`, `Set`, `Dict`, `Range`, and heterogeneous `Tuple` values are
  now nominal self-hosted structs supplied by the implicit prelude. Collection
  displays preserve contextual element inference and lower to ordinary
  constructors; comprehension leaves call `append`, `add`, or `__setitem__`;
  supported indexing, sizing, and containment use the same checked methods as
  user structs. Borrowed List/Set/Dict/Range iteration and owned List iteration
  likewise use selected nominal iterator methods; public Tuple has no runtime
  iteration contract. An `Indexer` argument is evaluated once and normalized
  through the checker-selected `__mlir_index__` only when the selected subscript
  needs `Int`; a direct overload accepting the source index type takes
  precedence. Checked single-index assignment retains its exact selected setter
  and receiver place through `MultiSet`, including a receiver rooted in a
  `mut`/`ref` parameter, so nested collection mutation cannot bypass lifecycle
  behavior by overwriting raw backing storage. Nominal `in` and `not in` retain
  the container place through the selected `__contains__` call, and the
  `Writable` `print`/`String`/`repr` formatting paths retain nominal argument
  places through the intrinsic call; pointer-backed collection storage is
  therefore borrowed rather than copied into a short-lived shallow owner.
  Concrete borrowed List, Set, and Dict place iteration retains the live owner's
  interior generation. Dict lookup replaces exactly its `value` owned-interior
  generation and all ordinary projections below it, matching Mojo without
  invalidating the sibling `element` generation used by key iteration. List
  additionally observes nonstructural element replacement and rejects a later
  iterator use after structural invalidation;
  List `for ref` writes through checked element handles. These are concrete
  compiler bridges, while current Mojo's origin-parameterized associated `IteratorType[...]`
  remains roadmap work. Owned List iteration also still requires
  `ImplicitlyDeletable` elements, including when a loop is statically guaranteed
  to exhaust a linear List. Exhaustion and `break` destroy the synthetic owned
  iterator at their common exit; a `return` materializes its result, runs pending
  `finally` regions, then destroys current and residual elements from the
  innermost loop outward. An ordinary value read through a returned reference
  now requires a `Copyable` referent and lowers to explicit typed `CopyValue`, so
  binding a Copyable nested collection runs its copy lifecycle while an explicit
  `ref` binding retains even a linear referent without duplicating it.
  HashSet's implicit deletion and bucket-replacing `add` operation are
  conditionally available only when its element type is
  `ImplicitlyDeletable`, matching the nested List setter's lifecycle contract.
  Checker-proven consuming reads of projected Copyable fields likewise lower to
  `CopyValue`; returning or assigning a pointer-owning field now invokes its
  nested copy initializer instead of creating a shallow second owner, while
  borrowed receiver and formatting reads remain place-preserving.
  The native List/Set/Dict/Range runtime variants and
  dedicated collection-construction MIR operations have been removed. Legacy
  flat standard-library modules are now thin public re-export facades over the
  authoritative `std.*` modules; explicit named re-exports can deliberately
  overlap an implicit-prelude name without causing ordinary modules to re-export
  the prelude.
  `Ty::ComptimeList`/`Value::ComptimeList` remain only at the CTFE bridge, while
  `Ty::Tuple`/`Value::Tuple` and `MakeTuple` remain only as private heterogeneous
  storage behind `__RuntimeTuple` and the specialized runtime-pack ABI. Static
  private-pack element projections retain distinct move state, so whole-pack
  transfer relocates linear elements without double destruction. Public Tuple
  specializations retain exact generic callable element contracts through
  opaque compiler-only annotation ids and a checker-seeded semantic map, rather
  than synthesizing a lossy source function type. They provide comparison,
  membership, reversal,
  concatenation, and dependent-handler `consume_elements`. Their lifecycle
  conformances are element-conditional: consuming transforms implicitly copy
  only `ImplicitlyCopyable` tuples and otherwise require `^`; indexed transfer
  of a non-`ImplicitlyCopyable` public element remains rejected in parity with
  Mojo.
- Current Mojo closure declarations now accept brace capture lists directly
  after effects, including `imm`, `mut`, `ref`, `var`, move, and default forms;
  the removed `unified {...}` position remains an explicit Mojito-only
  compatibility spelling. Closure environments materialize when their nested
  declaration executes, so owned captures snapshot or transfer at declaration
  while reference captures retain live frame/slot handles without imposing a
  declaration-to-call loan. Lexical declarations
  lift recursively at arbitrary depth with exact shadowing, permitted
  intermediate forwarding, effects, argument markers, named results, and
  reference-return ABIs. Explicit Origin arguments now participate in direct
  overloaded/generic calls and in contextually selected overloaded function
  values; their `mut`/`ref` conventions and origin-bearing results survive
  indirect calls. Callable structs with same-arity `__call__` overloads now
  retain the contract-matching lowered target through typed MIR and both VM
  indirect-call paths instead of reconstructing a target from arity. Captured
  nested Origin specialization also runs as a documented Mojito extension over
  the pinned nightly.
- Generic `F: def(T) -> T` bounds now retain a complete dependent callable
  contract, type calls through `F` inside the generic body, and validate each
  specialization against a monomorphic function or nominal callable struct with
  directional convention/effect compatibility. Anonymous contracts and callable
  values may also declare their own alpha-equivalent `def[...]` binders,
  including value defaults that govern invocation without becoming conformance
  identity. An explicit `thin` or `capturing[...]` qualifier creates a
  callable-value parameter; `OriginSet`, `capturing[_]`,
  `capturing[origins]`, and infer-only `//` syntax survive checking, and a
  supplied function or `@parameter` closure is reified as a hidden typed MIR
  local for indirect execution. Omitted callable arguments use a symbolic
  declaration-order plan for a selected function, an earlier callable
  parameter, or a compile-time conditional; generic specializations retain and
  reify these values rather than storing a function or closure in `CtValue`.
  Generic indirect-call MIR preserves named bracket arguments and the selected
  anonymous contract, so its scalar and callable defaults govern partial calls
  even when the concrete implementation declares different defaults; the
  verifier checks that parameter, effect, and reference-result metadata.
  Scalar-controlled `comptime` specialization can therefore residualize a
  captured callback, and variadic type/value packs may be interleaved with
  explicit positional or named `Origin` arguments. Callable types retain
  default/thin/capturing environments and canonical concrete capture origins
  with read/write access. Checked call adjustments and MIR call instructions
  carry those accesses into persistent-loan analysis, including calls through a
  non-escaping callable argument. This does not add arbitrary callable CTFE or
  escaping closures; unqualified stateful downward funargs remain a
  pinned-nightly Mojito extension.
- Checked syntax now has collision-free concrete occurrence identities after
  compile-time and trait-default cloning. Equal source provenance no longer
  aliases expression, declaration, overload, or type facts; MIR source maps
  still expose only file/byte provenance. Nested captures retain exact owner,
  storage type, and convention facts, including explicit-unused entries and
  intermediate forwarding. Runtime loop, tuple-unpack, and exception-handler
  binders keep distinct typed owner slots through HIR and structured regions, so
  same-name shadows and nested handler closures cannot overwrite outer values.
- Reference-bearing calls now use one caller-handle ABI in ordinary frames and
  synchronous `try` regions. Direct, indirect, method, callable-struct, and
  handwritten-constructor paths retain positional and keyword places; a
  temporary mirror of the caller frame preserves projected and aggregate
  reference returns plus mutations on raising paths. Reference navigation also
  crosses `UnsafePointer` and self-hosted nested-List storage without losing the
  caller identity.
- Nested heterogeneous-pack functions now specialize in their lexical context
  with scope-qualified declaration identities and are emitted at their original
  declaration site. Independent empty and nonempty instances, compile-time
  value parameters, explicit captures (including an outer runtime pack),
  defaults, keywords, named results, and sibling pack forwarding retain their
  selected ABI through recursively nested lifting. Whole-pack forwarding after
  a fixed positional prefix moves its Tuple collector as one value, so linear
  elements are not copied or illegally transferred through tuple indexing;
  keyword/default tails retain their normal slots, and call inference selects
  only the variadic overflow. Multiple spreads and explicit positional overflow
  after a spread are rejected in parity with the pinned nightly.
- Compile-time pack rewriting now assigns private, monotonic binding identities
  in separate value and type namespaces. Specialized `$pack[...]` parameters
  are recognized from their declaration rather than a source-name table, so
  block, loop, comprehension, nested-function, nested-type-parameter, and
  sibling-method shadowing cannot be mistaken for the outer pack. Empty packs
  remain distinguishable from non-pack values, and HIR loop lowering now gives
  loop binders lexical slots and restores an outer same-named binding after the
  loop.
- Collection-owned interior references now carry named, field-sensitive
  generations from checked HIR into typed MIR. `EstablishLoans` groups every
  dependency of a reference-bearing binding, while `InvalidateInteriors`
  records structural List mutation, Dict generation-defining lookup, Variant
  replacement, mutable/ref calls, whole-owner replacement through direct,
  reference-field, and pointer access, and replacement of interiors that own
  deeper interiors. Forward dataflow rejects a later stale use across branches,
  loops, and exact normal/raising/return-or-escape paths through nested
  try/except/else/finally regions and points to both the use and invalidation;
  ordinary owner reads, overlapping element aliases, direct List element
  writes, freshly rebound ordinary or interior generations, union-valued
  interior returns, and ordinary reborrows through a parent reference remain
  valid. Current
  `origin_of(place)._get_owned_interior["tag"]` return contracts are parsed and
  retain the complete projected receiver path. Drop liveness follows the
  reaching grouped generation, so rebinding a reference-bearing aggregate
  releases its old owner and retains its replacement without double-drops;
  transient MIR-register provenance keeps the selected owner alive through the
  complete consuming expression and no longer than that.
  Bare `ref` parameters/receivers now preserve parametric mutability without
  granting body writes, and immutable references cannot be escalated through an
  explicitly mutable origin contract.
- Heterogeneous function-pack bounds are now checked before specialization by a
  declaration-only conformance oracle shared with the checker. A failed call
  identifies the one-based pack element, its concrete type, the pack and trait,
  and the requesting instantiation instead of failing later in a generated
  body.
- The private heterogeneous pack carrier follows Mojo's left-to-right
  destruction order under conservative owned-root drop elaboration, including
  public-Tuple backing fields and exceptional edges. Specialized heterogeneous
  `*args^` forwarding relocates its whole moved pack through an explicit checked
  ABI, so the callee's source cleanup cannot destroy an element a second time;
  ordinary tuple-valued homogeneous variadics remain nominal List collectors.
  Transfers from non-implicitly-copyable indexed public tuple values are
  rejected instead of being silently copied and destroyed twice.
- Arbitrary-precision numeric literals now survive the whole compiler pipeline:
  integer spellings use `BigInt`, finite floating spellings use exact rationals
  with signed-zero preservation, literal-only arithmetic and CTFE remain exact,
  and typed MIR carries exact constants plus an explicit `MaterializeLiteral`
  boundary. Integer scalar/lane materialization wraps to the destination width;
  binary32/binary64 materialization rounds once from the exact value. Checked
  generic value-parameter declarations now cross into MIR/VM metadata so
  reification materializes at the declared type instead of leaking a literal
  value into an erased runtime slot. The differential arbitrary-precision case
  now matches Mojo.
- The tracked nightly target advances to Mojo 1.0.0b3.dev2026072505. Newly
  exposed interior-origin invalidation, collection-literal initializer
  inference, origin-parameterized associated iterator types, and the
  `SIMDLength` rename are recorded in dependency order. Interior origins,
  nominal collection inference, and concrete borrowed List provenance now run;
  general parameterized associated iterator types precede Unicode String and
  SIMD work in the MIR-schema-prerequisite roadmap.
- Variadic-generic structs: `struct S[*Ts: Bound]` declarations are specialized
  by compile-time elaboration per explicit instantiation (`S[Int, Bool](...)`),
  mirroring pack functions. Pack-typed members such as `var storage: Tuple[*Ts]`
  expand to the concrete element list, per-index reads (`s.storage[0]`) carry
  the exact element type, and specializations construct, copy, move, and drop as
  ordinary concrete structs. One trailing type pack (and no other compile-time
  parameters) is supported; instantiation requires explicit bracket arguments;
  a bare or argument-less template use and runtime-varying pack indexing are
  rejected with contextual errors. Struct annotation sites are now identified by
  the struct's unique name and each specialization carries a distinct source
  tag, so checked facts no longer collide across specializations sharing the
  template's spans.
- Variadic struct methods bind heterogeneous packs: real Mojo's pack
  constructor `def __init__(out self, var *args: *Ts)` (with the scoped
  `Tuple(*args^)` spread) specializes per instantiation, each constructor
  argument is checked against its per-index element type with exact pack arity,
  and method bodies can use `len(args)`/`args[i]`/`comptime for` over the pack.
  Method calls with a specialized heterogeneous variadic now score per-position
  everywhere (previously every overflow argument checked against one erased
  element type).
- Compile-time parameter subscripts use current Mojo's
  `def __getitem_param__[i: Int](...)` hook. General structs pass the source
  index as a checked value parameter, while a dependent variadic accessor
  returning `Ts[i]` unrolls into one concrete accessor per element at
  specialization. Public Tuple now defines only the current hook; the earlier
  `__getitem__` spelling remains an intentional compatibility fallback for
  user templates. `s[k]` requires a compile-time-constant in-range index, is
  typed by that element's exact type, and carries the checker-resolved accessor
  on MIR `Index`, so the VM does not guess a name. Reference-returning current
  hooks also receive value twins for implicitly-copyable rvalue subscripts, and
  explicit `ref` bindings retain the checked returned handle.
- Builtin scalar operators, comparisons, conversions, and rounding are typed
  through checked operation traits rather than ad-hoc numeric rules. Per-operator
  traits (`Addable`, `Subtractable`, `Multipliable`, `Divisible`,
  `FloorDivisible`, `Modable`, the bitwise/shift set, and `Negatable`) join the
  existing `Comparable`/`Equatable`/`Intable`/`Floatable`/`Boolable`/`Absable`/
  `Roundable`/`Powable`, so generic numeric code (`def f[T: Addable](a: T, b: T)
  -> T: return a + b`) type-checks and a struct declaring an operation trait must
  define its dunder. User structs now dispatch prefix operators (`-x` →
  `__neg__`, `not x` → `__bool__`) and concrete `Int()`/`Float64()`/`Bool()`
  conversions and `abs()`/`round()` through their dunders, matching the paths
  opaque generic parameters already used. Result types and execution are
  unchanged for existing programs; scalar execution stays primitive.


## [0.2.0] - 2026-07-19

Current-Mojo alignment through the pinned 1.0.0b3 nightly, executable origin
and pointer loans, and completely typed, semantically verified MIR — the
milestone gating the textual MIR/VM schema and native-backend work.

### Changed

- The public `Backend` trait object is now a statically dispatched enum over
  concrete implementations. `BackendKind` recognizes the planned
  `vm`/`cranelift`/`ebpf`/`llvm`/`mlir` seams; `BackendKind::make` parses a
  backend name and constructs it, and recognized-but-unimplemented backends
  refuse construction.

### Added

- MIR is completely value- and instruction-typed: every register — expression
  results, synthetic handles, markers, control-flow and iterator temporaries —
  carries a checked type, recorded at emission or copied from existing
  instruction facts by a closing pass that never re-implements checker
  inference. Functions and callable declarations retain their checked return
  types, raising contracts, and per-slot types; the last source-annotation
  reads left MIR lowering, and parameter slot types now come from checked
  declaration facts instead of name-matched body expressions.
- `mir::verify` is the standalone semantic verifier of record over MIR plus
  checked declaration metadata: place and projection consistency, register
  bounds and type completeness, store/binding/return/call-argument type
  consistency through the checker's coercion predicate, CFG-edge validity,
  effect protection for raising sites, and reference write-back invariants.
  The compiler pipeline gains a dedicated verification stage
  (`CompilerError::Verify`) composed with ownership analysis over one lowered
  program, the VM re-verifies the drop-elaborated program it executes, and the
  CLI `check`/`own` commands consume the same checked pipeline instead of
  silently re-checking.

- `UnsafePointer(to=place)` infers an origin-bearing pointer whose provenance is
  the concrete source place, with mutability taken from the owner binding. The
  checked pointer type retains the origin through HIR and MIR; the VM represents
  the value as an origin-free frame/slot handle. Pointer bindings and
  pointer-storing aggregates carry executable owner loans: the owner stays alive
  through the pointer's last use, and overlapping access, owner invalidation,
  and dangling escapes (`PointerEscapesOrigin`) are rejected statically. A place
  pointer binds a declared field origin parameter at aggregate-storage sites
  without inventing mutable capability, and non-zero offsets, arithmetic,
  comparison, and `free()` on origin-bearing pointers are rejected as a strict
  subset.

- Source imports now follow the current source-side namespace rules: source
  packages beat same-named source modules, ordinary directories can form dotted
  namespace paths, every dotted prefix binds, and submodules require explicit
  import or package-initializer re-export. Compiled `.mojoc`/`.mojopkg` lookup is
  reserved for the versioned artifact work.

- Homogeneous `**kwargs` collectors now use the self-hosted, insertion-ordered
  `StringDict[T]`. A final `**kwargs^` consumes and forwards its entries through
  the shared call binder with duplicate and element-type checking.

- Slice syntax now distinguishes `ContiguousSlice` and `StridedSlice`, preserves
  optional/negative bounds, implements `indices(length)`, and dispatches checked
  mixed or variadic `__getitem__` and `__setitem__` arguments, including slice
  assignment. Built-in collection view/API parity remains standard-library work.

- `std.utils.Variant` now supports compile-time type-membership queries,
  checked and unchecked consuming extraction, and checked and unchecked
  ownership-returning replacement. Unsupported arms reject statically, checked
  operations validate runtime tags, and `take` participates in use-after-move
  analysis.

- Current Mojo literal spellings now include leading/trailing-point floats,
  exponent forms, repeated/trailing digit separators, raw and case-insensitive
  string prefixes, one-to-three-digit octal escapes, triple-string line
  suppression, adjacent ordinary and t-string forms, nested interpolation
  boundaries, and the `Byte == UInt8` alias. Mojo does not define a distinct
  byte-string literal family.

- `CheckedProgram` now exposes stable checked expression and declaration arenas
  with child identities, resolved types, value/place/type categories, binding
  owners, extensible effect facts, and explicit semantic adjustments. Call,
  conversion, move, and explicit-destruction decisions are canonical node data.
  VM CTFE now passes rewritten fragments through the authoritative checker, and
  MIR retains checked types for source-derived registers.

- Checked HIR now retains stable checked-node identity, resolved type, value
  category, and semantic adjustments through function and exception-region CFGs.
  MIR consumes checked call/conversion/destruction decisions directly. Stored
  origin-parametric reference fields preserve frame/slot handles and owner loans,
  and user-defined slicing dispatches a checked `Slice` through `__getitem__`;
  slice-descriptor selection coexists with the canonical selected-call
  adjustment instead of replacing it.

- Checked HIR and MIR places now retain root, per-projection, and final storage
  types. Production lowering verifies complete typed-place metadata before VM
  execution, and reference field reads/writes use the checked storage type rather
  than rediscovering reference semantics from runtime values.

- Unsafe pointers now retain allocation provenance and typed offsets, support
  arithmetic, same-allocation subtraction, equality, aligned allocation and
  non-null dangling placeholders, and diagnose out-of-bounds access, invalid
  frees, double frees, and use after free. Static, untracked, and unsafe-any
  reference origins now lower into checked contracts, and local reborrows retain
  executable reference handles. The differential manifest now records that the
  pinned nightly has replaced static `UnsafePointer.alloc[_aligned]` and
  `pointer[0]` dereference with free `alloc[T](...)` and `pointer[]`; migrating
  that public spelling remains standard-library/syntax work.

- CPU-language surface work now includes definite late initialization,
  function-scoped implicit and walrus bindings, context-manager elaboration,
  loop `else`, list `for ref`, declaration destructuring, Writable-backed
  t-strings, integer bitwise/shift operators, and `__matmul__` dispatch.

- Callable and closure semantics now include contextually selected overloaded
  function values with effects, generic callable specialization, explicit
  unified capture conventions, sibling and generic nested calls, reference-backed
  closure environments, and nominal `def(...)` callable structs. Escaping
  closures remain statically rejected.

- A versioned Mojo nightly audit now tracks 1.0.0b3.dev2026071705 and records
  breaking drift affecting immutable conventions, linear deletion, constraints,
  closures, reflection, scalar/SIMD types, origins, imports, and keyword
  variadics.

- Compile-time parameters support typed scalar and aggregate values, type/value
  defaults, named arguments, infer-only parameters, dependent defaults and
  predicates, and heterogeneous type/value packs with per-index types.
- Generic constraints cover parameter and trailing `where` clauses, boolean and
  comparison predicates, `conforms_to`, conditional methods, and conditional
  conformance.
- Specialization uses structural cache keys, a deduplicated shared-fuel worklist,
  and source-located quota diagnostics.
- Current `reflect[T]` handles expose compile-time struct detection, field
  counts, names, types, named field indexes, and chainable `.field[name]` /
  `.field_at[index]` reflected handles whose selected type is `.T`. The removed
  `field_type` spelling is rejected, and reflection can drive
  declaration-producing compile-time branches.
- Generic-target `@implicit` conversions substitute concrete target parameters
  before constructor matching.

- Trait associated-type requirements compose bounds across refinements, and
  conditional conformance predicates are evaluated after type/value specialization.
- Current Indexer normalization, incremental caller-provided hashing, UTF-8 Writer
  buffering, Writable display/repr hooks, reflective formatting defaults, and
  String replacement fields replace the former direct `__str__` formatter path.

- Current Mojo consuming parameters use `var`; the removed `owned` spelling is
  rejected, and the convention is represented as `Var` throughout the compiler.
- Unified `__init__(out self, *, copy: Self)` and current
  `__init__(out self, *, deinit move: Self)` lifecycle declarations drive copy
  and move construction through the existing checked MIR and VM lifecycle
  machinery. A bare `move:` parameter remains a compatibility spelling.
- Calls materialize Copyable `imm` arguments before overlapping `mut`/`ref`
  access, allowing calls such as `f(mut x, x)` while retaining alias errors for
  non-Copyable values and multiple exclusive accesses.
- Current `ImplicitlyDeletable` lifecycle vocabulary replaces the superseded
  `ImplicitlyDestructible` spelling in bundled sources and generic checking.
- Validated, nonraising `@implicit` constructors now provide explicit MIR-lowered
  conversions for typed bindings, arguments, returns, and overload selection.
- `ImplicitlyDeletable where False`, rather than `@explicit_destroy`, now makes
  a type linear. The decorator requires a string and only supplies its
  diagnostic. Field-sensitive obligations preserve partial moves and projected
  destruction while rejecting whole destruction of incomplete aggregates,
  double and conditional destruction; raising destructors preserve the value
  for an `except` fallback, and automatic VM destruction is suppressed.
- Generic constraints now use only trailing `where`, compare types with
  `==`/`!=`, and accept pack-wide `conforms_to(Ts.values, Trait)`. `Int` is the
  canonical VM representation of `Scalar[DType.int]`; `SIMDSize` width values
  and `_` construction-width inference follow the pinned nightly vocabulary.

## [0.1.0] - 2026-07-15

Initial crates.io release.

### Added

- Indentation-sensitive lexer, Pratt parser, semantic checker, HIR and flattened
  MIR pipeline, ownership analysis, drop elaboration, and register VM.
- Functions, methods, structs, traits, generics, overloads, compile-time
  elaboration and VM-backed CTFE for the supported subset.
- Move checking, partial moves, ASAP destruction, stable origins, persistent
  loans, local and cross-call references, reference returns, and frame/slot
  runtime handles.
- Scalar, string, list, tuple, range, exception, iterator, unsafe-pointer, and
  VM-emulated `SIMD[...]` lane-vector semantics needed by the bundled self-hosted
  standard-library proofs. The VM executes lanes serially; hardware SIMD and
  native vector code generation are not included.
- Dotted, relative, qualified, and aliased source-module imports; package
  `__init__.mojo` discovery and re-exports; collision-free linked identities;
  and bundled `std` search roots.
- CLI stages for lexing, parsing, checking, ownership verification, and running
  `.mojo` source files.
- A versioned CPU-parity manifest and Pixi-driven differential harness for
  matching execution output and matching compiler rejection against a pinned
  Mojo reference build.
- A validated Mojo 1.0.0b2 manual inventory that distinguishes parity,
  strict-subset gaps, divergences, representation differences, exclusions, and
  stretch goals; every recorded divergence has an executable differential case.
- An expanded differential corpus covering the implemented first-pass parity
  surface with matching execution, matching rejection, strict-subset,
  acceptance-divergence, and output-divergence modes. The comparison also pins
  lowercase Bool formatting and Mojito's conservative same-place mutable-call
  rejection as known differences from the reference build.
- Mojo-compatible module-scope validation: production compilation rejects
  executable file-scope statements and enters runtime code through `main()`.
- Source package namespace completion includes wildcard privacy for
  underscore-prefixed declarations and isolates same-named declarations and
  overload sets from different modules.
- Module namespaces now preserve lexical shadowing, support imports inside
  functions and nested blocks, resolve unaliased full dotted paths and exported
  types, and implement dots-only relative sibling-module imports.
- User-defined static methods now type-check, participate in overload selection,
  lower without an implicit receiver, and execute with default and keyword arguments.
- `raise` now requires a surrounding handler or a `raises` function/method, and
  direct calls to raising free functions must be handled or propagated.
- Raising instance and static methods now retain their effect through method
  overload selection, so calls must likewise be handled or propagated.
- Non-capturing functions are runtime values with checked function types and can
  be stored, passed as arguments, and invoked through MIR indirect calls.
- Function types retain their `raises` effect; selected free-function overloads
  and indirect callable calls now require effect handling or propagation.
- Typed and parametric errors now survive parsing and checking through direct,
  overloaded, method, and indirect calls. Handlers receive the inferred typed
  error value, and `Never` acts as the bottom and nonraising error type.
- Free functions support a single named `out` result with caller-transparent
  invocation, checked initialization, and direct VM return-slot execution.
- Generic free functions accept heterogeneous `*args: *ArgTypes` packs, check
  every supplied type against the pack bound, and execute type-erased pack
  length queries. Compile-time loops can specialize literal/constructed packs,
  query `args.__len__()`, and index elements through their common bound.
- Expected function types contextually specialize non-overloaded generic function
  values for checked indirect invocation. Hand-written constructors now share
  default and keyword argument binding with free and method calls.
- Overload selection now follows first-pass Mojo precedence across conversion
  counts, fixed versus variadic candidates, signature length, and generic ties;
  defaulted and variadic declarations can participate in overload sets while
  overlapping defaulted calls retain ambiguity.
- Trait refinement now inherits method and associated-member requirements, and
  executable defaults are statically materialized with override/ambiguity rules.
- Lifecycle definite initialization follows normal, returning, raising,
  branching, looping, and protected exceptional paths instead of collecting
  assignments flow-insensitively.
- Opaque trait-bounded indexing dispatches through `__getitem__`; the self-hosted
  library includes an incremental hasher proof; and user-defined printed values
  must opt into Writable/Representable formatting. Bool output is `True`/`False`.

### Scope

- Targets an evolving single-threaded CPU subset of Mojo.
- GPU execution, concurrency/parallelism, distributed execution, Python
  interoperability, MLIR, and optimized native code generation are not included.

[0.2.0]: https://github.com/bpr/mojito/releases/tag/v0.2.0
[0.1.0]: https://github.com/bpr/mojito/releases/tag/v0.1.0
