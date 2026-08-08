# Mojo Nightly Target

Mojito tracks **Mojo 1.0.0b3.dev2026072505 (2026-07-25)** as its language
comparison target. The version is taken from the official cumulative nightly
release page:

- <https://mojolang.org/releases/nightly/>

The differential runner must still report the actual locally installed
`mojo --version`; updating this document or the parity-manifest header does not
claim that every nightly change has already been implemented.

## July 25 Differential Audit

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

## Review Policy

Before closing a language-parity milestone:

1. Compare the target above with the version at the top of the nightly page.
2. Review every intervening **Language enhancements** and **Language changes**
   entry, not only the stable manual.
3. Update `conformance/parity.tsv` first. A newly introduced mismatch becomes a
   documented subset/divergence until implementation and differential evidence
   justify `implemented`/`match`.
4. Update `roadmap.md`, grammar and architecture documentation, fixtures, and
   bundled Mojo sources affected by removed or renamed syntax.
5. Run differential conformance with a Pixi environment containing the exact
   target build and retain the reported version with the results.

## 1.0.0b3 Nightly Drift From Mojito's Previous Baseline

The following CPU-language changes affect Mojito directly.

| Area | Current nightly | Mojito consequence |
|---|---|---|
| Interior origins | Collections can bind element references to internal origins that are invalidated by structural mutation or reallocation without borrowing the entire owner forever. | Implemented as explicit checked origin/invalidation facts, grouped MIR generations (including union-valued returns), and forward CFG analysis with distinct normal/raising/return-or-escape channels for nominal collection methods, Variant operations, and user return contracts. Self-hosted collections declare and reuse this checked boundary without native collection inference. |
| Literal initializer inference | Partially specified collection annotations such as `List[_]` or bare `List` can infer element types from a literal initializer. | Implemented contextually before displays lower to the selected nominal collection constructors and methods; no VM-native collection shortcut remains. |
| Origin capability cast | `Origin[mut=False].cast_from[origin]` (and `ImmutableOrigin.cast_from`) downcasts an origin's capability to read-only in reference signatures; upcasts are unsafe and spelled through `MutableOrigin.cast_from` only in unsafe code. | Accept the keyword `Origin[mut=False].cast_from[...]` downcast in `ref[...]` result signatures, pinning the yielded capability to `Immutable` independent of the origin parameter's parametric `mut=`; reject the `mut=True` upgrade direction. |
| Immutable convention | `imm` is the preferred spelling for the argument and closure-capture convention. `read` remains a synonym but is headed for deprecation. | Accept and emit `imm`; retain `read` only as a compatibility spelling. The linked commit `323dfd974e2f6fc83ce82a476d8fa5d51529eadf` documents this transition. |
| Linear-value trait | `ImplicitlyDestructible` was renamed to `ImplicitlyDeletable`; `is_trivially_destructible` likewise became `is_trivially_deletable`. | Reverse Mojito's previous vocabulary migration and update constraints, diagnostics, tests, and bundled sources. |
| String indexing | Current Mojo's String exposes byte/codepoint access through iterator and method APIs; the exact keyword-subscript spelling is not settled stable syntax. `Codepoint` constructs from `UInt32` scalars (`from_u32`, `to_u32`) and prints as its character; grapheme segmentation has no settled stable String API. | Mojito implements the roadmap's explicit keyword-indexed forms `s[byte=i]`/`s[codepoint=i]`/`s[grapheme=i]` over the self-hosted String (positional `s[i]` rejected as ambiguous). `codepoint=` yields a subset `Codepoint`: `Int`-based via `Intable` instead of `to_u32`/`UInt32`, Writable as its character, scalar-ordered; direct construction is rejected until runtime scalar conversions land (`Byte(Int)` encoding, under the SIMD-conversion roadmap item). `grapheme=`/`grapheme_count()` are Mojito-explicit spellings implementing a pinned UAX #29 subset (hand-maintained essentials classifier plus arithmetic Hangul; GB11 simplified to "never break after ZWJ"; GB9b Prepend omitted). Keyword subscripts themselves are a general Mojito feature documented in `grammar.md`. |
| String type split | `String` is the standard runtime string; a `StringLiteral` implicitly converts and materializes to `String`, including for un-annotated bindings (`var s = "lit"` is a `String`). String slicing is a supported non-raising API surface. | Mojito's `: String` annotations resolve to the self-hosted nominal struct with implicit literal conversion, but un-annotated string bindings deliberately stay `StringLiteral` until nominal slicing/result-API parity lands (a recorded follow-up). Nominal `String` slicing is boundary-checked library code that `raises` and rejects strides, unlike the byte-wise literal slice. Literal `hash(...)` uses the VM's FNV path while nominal keys hash through the struct's DJB2 `__hash__` — containers are internally consistent per key type. Overloads differing only in `StringLiteral`-vs-`String` are rejected (both mangle to the stable `String` symbol spelling). |
| Lazy template strings | Current Mojo's `TString` (std.format.tstring) is origin-parameterized — `TString[origins, //, format_string, *Ts]` holds a borrowed `VariadicPack` of interpolation references, so exclusivity rejects mutating a captured value before the template is used. | Mojito's self-hosted prelude `TString[*Ts: Movable & Writable]` captures typed value snapshots at creation instead (an origin-parameterized reference pack is not yet expressible): copyable interpolations copy in, non-Copyable places snapshot as creation-time formatted strings, and mutating a captured variable afterward prints the snapshot rather than being rejected. Formatting defers to Writable `write_to` in both. `TString[...]` annotations are not spellable in Mojito. |
| Explicit destruction | `@explicit_destroy` no longer opts a type out of implicit deletion. A type narrows or removes `ImplicitlyDeletable` through conditional conformance, commonly `ImplicitlyDeletable where False`. The decorator is optional and only supplies an explanatory diagnostic; using it without a message is an error. | Separate the linearity fact from the diagnostic decorator and derive automatic deletion from conformance. |
| Move initialization | The consuming unified initializer is `__init__(out self, *, deinit move: Self)`; a bare `move:` parameter is rejected with a migration diagnostic. | Use the `deinit` argument convention in current fixtures and documentation while retaining bare `move:` only as an explicit Mojito compatibility spelling. |
| Constraints | Parameter-list `where` clauses were removed. Only trailing declaration `where` clauses remain. Type equality now uses `==`/`!=`; `_type_is_eq` was removed. Pack operands such as `Ts.values` work with `conforms_to`. | Reject the formerly accepted parameter-list form, expand the checked predicate algebra, and update fixtures. |
| Closures and callables | Closures use a brace capture list after effects, with `imm`, `mut`, `ref`, `var`, moves (`x^`), and optional default conventions; the `unified` keyword was removed. Explicit origin specialization materializes origin-generic function values. Generic `F: def(...)` bounds are callable constraints, while `def(...) thin` / `capturing[origins]` in a parameter list denotes a callable value and its environment contract. Callable parameters may have defaults, anonymous callable contracts may declare their own generic binders, and compile-time control may specialize while a callable remains a residual argument. User structs must explicitly conform to a convention- and origin-bearing `def(...)` closure trait; compatible `__call__` alone is not enough. | Current capture syntax, recursively lifted declaration-time environments, monomorphic and generic-anonymous callable bounds/value parameters, symbolic callable defaults, `OriginSet` inference, explicit capturing-origin sets, residual callable specialization, read/write capture effects, conventions, raising effects, and reference results are checked and execute through indirect dispatch. Explicit Origin arguments participate in overload/generic candidate selection and may be interleaved with packs. `CtValue` remains closure-free and arbitrary callable CTFE is not claimed. Legacy `unified {...}`, unqualified stateful downward funargs (call positions only: a capturing closure does not erase its environment into plain `def(...)` storage — fields and collection elements reject it unless the storage declares `capturing[...]`), and captured nested Origin-specialized values are documented Mojito extensions. |
| Reflection | `Reflected.field_type[name]` became `Reflected.field[name]`; the result is a chainable reflected handle whose type is `.T`. `field_at[index]` is the by-index counterpart. | Implemented with current `reflect[T]` syntax, nested handle chaining, named/indexed diagnostics, and rejection of `field_type`. |
| Integer/SIMD model | `Int` is an alias for `Scalar[DType.int]`. SIMD-width inference uses `SIMDLength` (briefly named `SIMDSize`), or `_` for an unbound width. | Int/Scalar identity is implemented. Migrate the deprecated width spelling and finish the dtype/literal/mask/reduction surface in Task 1 before claiming scalar/SIMD parity. |
| Origins and pointers | Struct fields may not hide `UnsafeAnyOrigin`; use an explicit origin parameter or `UntrackedOrigin`. Implicit widening conversions to unsafe-any origins are deprecated or removed, and pointer optionals preserve concrete origins. | Hidden unsafe-any fields are rejected and there is no implicit unsafe-origin widening. `UnsafePointer(to=place)` infers a concrete place origin with executable owner loans; a place pointer coerces only to a declared origin parameter at aggregate-storage sites. |
| Imports and artifacts | Resolution order is source package, `.mojoc`, source module, then legacy `.mojopkg`. Relative imports require `from`; dotted absolute imports bind every prefix; intra-package implicit visibility is deprecated. | Source precedence, prefix namespaces, and explicit intra-package visibility are implemented. `.mojoc`/`.mojopkg` loading remains in Packaging, Artifacts, And Developer Tooling. |
| Keyword variadics | `**kwargs` may be forwarded with `**kwargs^`; the standard owning container is now `StringDict`. | Homogeneous free, generic, instance, static, and bounded-trait collectors use owned `StringDict[T]` values; consuming forwarding runs through the shared binder and its specialization, ownership, origin, duplicate, and effect checks. |
| Borrowed iteration | `Iterable` exposes `IteratorType[iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]]`, and `__iter__(ref self)` returns `Self.IteratorType[origin_of(self)]`, allowing yielded references to retain source origin and mutability. `IterableOwned` separately exposes monomorphic `IteratorOwnedType`. A concrete `__next__ -> ref[o] T` may refine an abstract value result `T` when `T: Copyable`; the abstract caller receives a copy. | The bundled owned protocol now uses monomorphic `IteratorOwnedType`. Parameterized associated member declarations/applications are represented, and an abstract `origin_of(self)` argument now resolves concretely (via a symbolic self-origin that erases at runtime), so a conforming struct's `Self.IteratorType[origin_of(self)]` member resolves and conformance succeeds — including when the conformer spells that application directly as its `__iter__(ref self)` return type. Directional `Copyable` `__next__` refinement is checked, retained as an explicit abstract-call adapter, and executes for bounded calls and generic loops. The borrowed `Iterable` proof protocol still retains the legacy monomorphic `Iter` member: migrating it, removing concrete borrow bridges, and deriving source/yield origins generically remain the next borrowed-iteration work. |
| Owned iteration | `for var x in collection^` supports moving non-Copyable elements; collection deletion conformance is conditional on element capabilities. | Consuming collection iteration moves the source and each element, destroys implicitly deletable residual state on early exit, and rejects any abandoning path (early exit, unhandled raising calls, comprehension filters) when linear residual elements would be abandoned, naming their explicit-destroy obligation. The exhausted linear-element iterator is consumed through a `_finish(deinit self)` named destructor selected by the checker — a Mojito-internal bundled convention modeling the linear-types proposal's named destructors (current Mojo has no owned-iteration equivalent to compare against). |
| Tuple ownership | `Tuple` lifecycle conformances are conditional; `reverse`, `concat`, and `consume_elements` have consuming receivers, and `consume_elements` transfers elements, including non-`ImplicitlyCopyable` values, one at a time to a parameterized closure. Tuple indexing and destructuring do not provide an indexed partial-move place. | The public nominal `Tuple[*Ts]` folds lifecycle conformance per element. A consuming call can implicitly copy a fully `ImplicitlyCopyable` tuple; move-only receivers and `concat` operands require `^`. The dependent `def[index: Int](var element: Ts[index])` handler consumes private pack storage left-to-right, while public indexed transfer remains rejected. |

Python/NumPy additions, GPU changes, and distributed/concurrent facilities remain
outside Mojito's declared first-pass scope.
