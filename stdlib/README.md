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
- `std/range.mojo` — the nominal `Range` returned by the bundled `range`
  overloads, with length, indexing, containment, formatting, and borrowed
  typed-raising iteration through `_RangeIter`.
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
  during iteration is lazily rejected; views remain eager snapshots.
- `std/collections/set.mojo` — a generic, list-backed `Set[T]` for `Equatable & Copyable & Movable`
  elements. It supports `add`, membership through `in`/`__contains__`, `len`, and
  borrowed reference-yielding iteration through the backing list's borrowed
  `_ListIter`. It conforms to
  `Iterable`.
- `std/collections/dict.mojo` — a generic, insertion-ordered, list-backed
  `Dict[K, V]`. It supports subscripts, overloaded `get`, membership, key
  iteration, eager `keys`/`values`/`items` snapshots, public `DictEntry`, and
  value-semantic copying. A missing subscript raises `Error("missing key")`.
- `std/collections/hashdict.mojo` — a hash-backed, insertion-ordered
  `HashDict[K, V]`: dense entries preserve order while `List[List[Int]]` buckets
  index them. It grows and rehashes explicitly and mirrors the `Dict` API.
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
- `std/collections/hashset.mojo` — an experimental hash-backed `HashSet[T: Hashable & Equatable &
  Copyable & Movable]`. It keeps a fixed array of buckets and only scans the bucket
  a key hashes into, so it is genuinely hash-backed (unlike the linear-scan `Set`).
  `Hashable` does not imply `Equatable`, so both bounds are named — the hash picks a
  bucket, equality resolves collisions within it. Its nested buckets use the
  self-hosted `List`; `add` stages and writes back one copied bucket, and is
  available only when `T: Deinitable` because replacement must satisfy
  the nested List setter's lifecycle contract. The bucket count remains fixed
  pending a rehashing follow-up.

Underscore-prefixed structs such as `_ListIter` are implementation details,
following the Python convention that Mojo currently inherits. `DictEntry` is
public, matching Mojo's item-view element. Mapping views are eager snapshots
rather than reference views until live view APIs are implemented.

The register VM executes the ordinary MIR produced for these declarations;
`tests/self_host_test.rs` links and runs them. Public List/Tuple runtime variants
have already been retired. The remaining private storage bridges are
documented in the architecture and roadmap.
