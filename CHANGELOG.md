# Changelog

All notable changes to Mojito will be documented in this file. The project uses
Semantic Versioning while its public Rust API and supported Mojo subset continue
to evolve under the `0.x` compatibility rules.

## [Unreleased]

### Added

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
