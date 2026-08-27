# stdlib — the standard library, written in mojito itself

The public types and algorithms are ordinary mojito `.mojo` declarations rather
than compiler-owned collection variants. The core implementations still use a
few narrow compiler-supported storage and construction seams, notably
`UnsafePointer` storage operations, literal/prelude hooks, and the private
`__RuntimeTuple` pack carrier. Together they are the north-star proof that the
language is expressive enough to author its own collections and small generic
algorithms.

The preferred import shape follows Mojo's `std` package layout:

```mojo
from std.collections.list import List
from std.collections.set import Set
from std.collections.dict import Dict
from std.optional import Optional
from std.math import floor, ceil
```

The older flat files (`stdlib/list.mojo`, `stdlib/dict.mojo`, and friends) are
thin public re-export facades, so legacy examples such as `from list import
List` still work when `stdlib/` is on the search path without maintaining a
second implementation. Underscore-prefixed implementation types are available
only from their authoritative `std` modules.

- `std/collections/array.mojo` — a fixed-size `Array[T: AnyType, length: Int]`
  backed by an `UnsafePointer[T]`, with conditional
  `Copyable`/`Movable`/`Deinitable`/`Equatable`/`Iterable`/`IterableOwned`/
  `Writable` conformances. Construction is the checker-driven variadic literal
  constructor (`[a, b, c]` displays), keyword `fill`, `copy:`, and
  `deinit move:`; it is neither `ImplicitlyCopyable` nor `Defaultable`.
  Indexing is by-reference `__getitem__` only (no `__setitem__`, matching
  upstream Array); `__len__` returns the comptime `length`, while the private
  `_size` field exists so the owned iterator can neutralize the source's
  destructor. Borrowed iteration mirrors `_ListIter` (`_ArrayIter` holds
  `ref[iterable_origin] Array[T, length]`); owned iteration moves the buffer
  into `_ArrayOwnedIter`, whose methods are where-gated so the `T: AnyType`
  template stays checkable.
- `std/collections/list.mojo` — a generic, growable `List[T]` backed by an
  `UnsafePointer[T]`, with the full value-type lifecycle (ordinary `__init__`,
  `__init__(..., copy:)`, and `__init__(..., deinit move:)`), subscript read/write
  (`__getitem__`/`__setitem__`), `__len__`, and the iterator protocol
  (`__iter__(ref self)` → a borrowing
  `_ListIter[iterable_mut: Bool, //, T, iterable_origin: Origin[mut=iterable_mut]]`
  holding `ref[iterable_origin] List[T]`, whose typed-raising `__next__` yields
  `ref[iterable_origin] T` element references; `__len__` remains a
  compatibility/optimization hint). Exhaustion raises `StopIteration`. Growth
  reallocs the buffer. `_get_copy(index)` is a library-private, non-overloaded
  value accessor used by nested collection implementations until a
  reference-returning subscript can retain its full selected-call contract as a
  chained receiver/place in typed MIR. Ordinary public value reads already copy
  through the returned reference, explicit `ref` bindings retain the alias, and
  public indexing remains `__getitem__`.
- `std/range.mojo` — current Mojo's private range family
  (`_ZeroStartingRange`/`_SequentialRange`/`_StridedRange[dtype: DType =
  DType.int]`), each its own borrowed typed-raising iterator over
  `Scalar[dtype]` elements, returned by the bundled Int `range` overloads;
  scalar arguments reach the same structs through checker-inferred dtype
  specialization. Length and Int indexing run; containment, formatting,
  `reversed()`, `bounds()`, and float strided ranges are recorded subset
  gaps.
- `std/collections/tuple.mojo` — the public heterogeneous `Tuple[*Ts]`, with
  current `__getitem_param__` indexing and element-conditional lifecycle,
  comparison, formatting, concatenation, reversal, and consuming APIs. Its
  `__RuntimeTuple[*Ts]` field is compiler-private heterogeneous pack storage;
  public Tuple is nominal and is not a method-free runtime iterable.
- `std/optional.mojo` — a generic `Optional[T]` using zero-or-one value storage,
  including an empty constructor for generic absent values.
- `std/iterable.mojo` — minimal self-hosted `Iterator`, `Iterable`, and
  `IterableOwned` proof traits. They expose associated compile-time `Element`
  facts. `IterableOwned` uses current Mojo's monomorphic `IteratorOwnedType` (a
  consuming iterator owns its storage, so it needs no origin). Borrowed
  `Iterable` uses Mojo's origin-parameterized `IteratorType[iterable_mut: Bool,
  //, iterable_origin: Origin[mut=iterable_mut]]` with
  `__iter__(ref self) -> Self.IteratorType[origin_of(self)]`; the bundled
  member templates stay origin-erased; every bundled borrowed iterator carries
  its origin as an erased struct parameter, borrows its source, and yields
  element references declared at `_get_owned_interior["element"]` granularity,
  resolved to the source's mutability at each loop site. Mapping mutation
  during iteration is lazily rejected; view iterators snapshot at the call.
- `std/collections/set.mojo` — a generic, list-backed `Set[T]` for `Hashable & Equatable &
  Copyable & Movable` elements (upstream's KeyElement bound; the dense list
  preserves insertion order). It supports `add`, membership through `in`/`__contains__`, `len`, and
  borrowed reference-yielding iteration through the backing list's borrowed
  `_ListIter`. It conforms to
  `Iterable`.
- `std/collections/dict.mojo` — a generic, hash-backed, insertion-ordered
  `Dict[K: Hashable & Equatable & Copyable & Movable, V]`: dense entries
  preserve order while `List[List[Int]]` buckets index them, doubling when
  the load factor reaches one. It supports subscripts, overloaded `get`,
  membership, key iteration, self-iterable non-indexable `keys`/`values`/`items`
  snapshot iterators, public
  `DictEntry`, and value-semantic copying. A missing subscript raises
  `Error("missing key")`.
- `std/collections/string_dict.mojo` — the insertion-ordered owning
  `StringDict[V]` used for homogeneous `**kwargs`; the VM constructs it in the
  callee frame and consumes it for `**kwargs^` forwarding.
- `std/algorithms.mojo` — small generic helpers that exercise comptime-guided library
  code: type predicates, CTFE-computed constants, value parameters, and associated
  compile-time facts. It includes `first_or[C: Iterable]`, which consumes
  `C.Element` through an opaque iterable bound.
- `std/hashing.mojo` — a tiny hash helper: `bucket_index[K: Hashable](key, bucket_count)`
  maps a key into `[0, bucket_count)` via its `__hash__` (`-> UInt`). Built-in
  scalar keys hash intrinsically; the hash is deterministic (no per-run seed).
- `std/math.mojo` — self-hosted numeric rounding helpers `floor`/`ceil`/`trunc`/`ceildiv`,
  each generic over its Mojo trait bound (`Floorable`/`Ceilable`/`Truncable`/`CeilDivable`).
  Unlike `abs`/`round`/`divmod` (Mojo prelude builtins, available bare), these mirror
  Mojo's `math` module and must be imported: `from std.math import floor`. Built-in `Int`/`Float64`
  supply the underlying dunders intrinsically.
- `std/span.mojo` — the prelude-exported borrowed view `Span[mut: Bool, //,
  T: Movable, origin: Origin[mut=mut]]`: a multi-element origin-bearing
  pointer (`Pointer[T, origin._get_owned_interior["element"]]`) plus a
  length, constructed from `ref [origin] list: List[T]` so the source stays
  lent while any copy of the view lives. Element access is a reference
  result with abort bounds; the strict `ContiguousSlice` overload returns a
  sub-view; there is no strided slicing.
- `std/os.mojo` — the `os` proof subset: `abort(message)`, the uncatchable
  trap behind strict slice bounds, crossing to the VM through the
  compiler-private `_mojito_abort` primitive (which stdlib-internal trap
  sites call directly with a literal to avoid import cycles).
- `std/string.mojo` also hosts `StringSpan[mut: Bool, //, origin]` (the
  canonical string view; `StringSlice` is a never-emitted annotation alias)
  and `_GraphemeIter`, the grapheme-cluster iterator behind ordinary
  String/StringSpan/StringLiteral iteration.

Underscore-prefixed structs such as `_ListIter` are implementation details,
following the Python convention that Mojo currently inherits. `DictEntry` is
public, matching Mojo's item-view element. Mapping views are self-iterable,
non-indexable snapshot iterators rather than borrowing views until a
ref-field struct can cross an ordinary method return on the VM.

The register VM executes the ordinary MIR produced for these declarations;
`tests/self_host_test.rs` links and runs them. Public List/Tuple runtime variants
have already been retired. The remaining private storage bridges are
documented in the architecture and roadmap.
