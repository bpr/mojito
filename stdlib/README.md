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
from std.collections.optional import Optional
from std.math import floor, ceil
```

A program always imports the library as `std`, wherever its search roots
point, as upstream's toolchain does.

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
  `ref[iterable_origin] T` element references; the iterator's `__len__`
  serves `len()` and never drives a loop). Exhaustion raises `StopIteration`. Growth
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
- `std/builtin/tuple.mojo` — the public heterogeneous `Tuple[*Ts]`, with
  current `__getitem_param__` indexing and element-conditional lifecycle,
  comparison, formatting, concatenation, reversal, and consuming APIs. Its
  `__RuntimeTuple[*Ts]` field is compiler-private heterogeneous pack storage;
  public Tuple is nominal and is not a method-free runtime iterable. Its
  default initializer stores each element's default construction to that
  storage, available where every element is `Defaultable`.
- `std/builtin/len.mojo` — upstream's `Sized` and `SizedRaising` traits,
  each requiring `__len__`, and upstream's `len` overload set over them
  (an `@inline(.always)` `Sized` overload, a raising `SizedRaising` one,
  and an `@unavailable` one over a string); the prelude exports all three.
  A struct conforms by
  declaring one, and the compiler-modelled `SIMD` scalars, tuples, and packs
  conform as the upstream structs they stand in for declare `Sized`.
- `std/builtin/comparable.mojo` — upstream's `Equatable` and
  `Comparable(Equatable)` traits: `__ne__` defaulted through `__eq__`, and
  `__gt__`, `__le__`, and `__ge__` through `__lt__`; the prelude exports
  both. Upstream's reflective `__eq__` default is a stand-in the compiler
  synthesizes per conformer until its body can read each field through
  `field_ref` (R1481), so `__eq__` stays a requirement here. The
  compiler-modelled scalars and `Bool` conform to both, and `DType` to
  `Equatable`, as the upstream structs they stand in for declare.
- `std/builtin/bool.mojo`, `std/builtin/int.mojo`,
  `std/builtin/floatable.mojo`, and `std/builtin/identifiable.mojo` —
  upstream's conversion traits: `Boolable`, `Indexer`, `Intable`,
  `IntableRaising`, `Floatable`, `FloatableRaising`, and `Identifiable`,
  whose `__isnot__` defaults through `__is__`; the prelude exports all
  seven. The compiler-modelled scalars conform as the upstream structs they
  stand in for declare: `SIMD`, `Bool`, and the literals are `Boolable`;
  `SIMD`, `Bool`, and the numeric literals `Intable`; only `Bool` and
  `FloatLiteral` are `Floatable`; `SIMD` and `IntLiteral` are `Indexer`.
  Only a scalar `SIMD` is `Intable`, and only an integral one `Indexer`,
  since upstream's methods assert as much (R1397). A scalar answers a
  requirement through its builtin conversion. `bool.mojo` also holds
  upstream's `SIMD` overloads of `any` and `all`, which the prelude
  exports; the `Iterable` overloads wait on R1605.
- `std/builtin/simd_length.mojo` — upstream's `SIMDLength`, verbatim, the
  type of `SIMD`'s width: a wrapper of the MLIR `index` that a literal or an
  `Int` converts to, and that converts to any `SIMD` through the compiler
  `SIMD`'s `@implicit __init__(SIMDLength)`. The prelude exports it.
- `std/builtin/string_literal.mojo` — upstream's `StringLiteral[value]`
  struct, in part: the type of a string literal (`"ab"` is
  `StringLiteral["ab"]`) and of an explicit application, with the members
  that read its `!kgen.string` through `pop.string.size`,
  `pop.string.address`, and `#kgen.param.identical`. The compiler holds an
  application as its literal type (roadmap R1660), and a bare run-time
  `StringLiteral` argument is still a value-less compiler type (R1658).
  The prelude exports it.
- `std/memory/unsafe.mojo` — upstream's `bitcast` and `pack_bits` over
  `pop.bitcast`, re-exported by `std.memory`; `std/builtin/dtype.mojo`
  bundles `_uint_type_of_width`, `_integral_type_of`, and
  `_unsigned_integral_type_of`.
- `std/builtin/value.mojo` — upstream's `Defaultable`, `RegisterPassable`,
  and `TrivialRegisterPassable` traits; the prelude exports all three.
  `TrivialRegisterPassable` refines `Deinitable`, `ImplicitlyCopyable`,
  `Movable`, and `RegisterPassable`, so a struct declaring it is copyable.
  `Defaultable.__init__` stays a requirement until a trait default can
  construct `self` field by field, as upstream's reflective default does
  (R1119). The compiler-modelled scalars, `Bool`, `NoneType`, and `DType`
  conform as the upstream structs they stand in for declare.
- `std/builtin/_stubs.mojo` — upstream's `paramfor_has_next`,
  `paramfor_next_iter`, and `paramfor_next_value`, which step a `comptime
  for` iterator at compile time. The prelude links them for their bodies:
  the compiler names them by module path, and no user spelling reaches
  them.
- `std/builtin/builtin_slice.mojo` — upstream's `Slice`, the three
  `slice(...)` functions, `ContiguousSlice`, and `StridedSlice`, each struct
  with the `__slice_literal__` constructor a slice literal calls for a
  parameter of its type; `Slice` writes through `FormatStruct`/`Named`, the
  two kinds delegate `write_to`/`write_repr_to` to it, and `indices` is
  two-element for `ContiguousSlice`; `StridedSlice` converts implicitly from
  a `Slice`. The prelude binds `Slice` and `slice`; only an import binds the
  two kinds.
- `std/builtin/variadics.mojo` — upstream's `_MLIR.KGENTypeType`
  (`!kgen.type`) and `_MLIR.KGENParamListType[elt_type: Self.KGENTypeType]`
  raw-list alias, `ParameterList[type: AnyType, //, values:
  _MLIR.KGENParamListType[type]]`, the zero-sized value a homogeneous value
  pack is when read at run time (`ParameterList[vs.values]()`), and its
  `_ParameterListIter`. Its elements live in a constant array laid out once
  per program; `get_span` addresses it through the compiler-private
  `__param_list_address[values]()`, admitted only in this module, where
  upstream calls `global_constant`. It is `Writable` where its element type
  is; `write_repr_to` writes upstream's `FormatStruct` text directly.
- `std/collections/optional.mojo` — a generic `Optional[T]` using zero-or-one value storage,
  including an empty constructor for generic absent values.
- `std/iter.mojo` — minimal self-hosted `Iterator`, `Iterable`, and
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
- `std/format/__init__.mojo` — upstream's `Writer` and `Writable` traits:
  `Writer`'s `write_string` requirement and the default `write[*Ts:
  Writable]` that writes each argument through its `write_to`, which a
  conformer may override, and `Writable`'s `write_to`/`write_repr_to`
  requirements. The prelude exports both. Upstream's reflective defaults are
  synthesized per conformer (R1461), writing each field through
  `_reflection_write_field_to`, which every module hosting a defaulted
  conformer imports.
- `std/collections/set.mojo` — a generic, list-backed `Set[T, H: Hasher = default_hasher]` for `Hashable & Equatable &
  Copyable & Movable` elements (upstream's KeyElement bound and hasher
  parameter; the dense list preserves insertion order and never hashes). It supports `add`, membership through `in`/`__contains__`, `len`, and
  borrowed reference-yielding iteration through the backing list's borrowed
  `_ListIter`. It conforms to
  `Iterable`.
- `std/collections/dict.mojo` — a generic, hash-backed, insertion-ordered
  `Dict[K: Hashable & Equatable & Movable, V: Movable, H: Hasher = default_hasher]`: dense entries
  (each caching its `UInt64` hash under `H`) preserve order while
  `List[List[Int]]` buckets index them by the hash's low bits, doubling when
  the load factor reaches one. It supports subscripts, overloaded `get`,
  membership, key iteration, self-iterable non-indexable `keys`/`values`/`items`
  borrowing views, lazily draining `take_items`, public
  `DictEntry`, and value-semantic copying. A missing subscript raises
  `Error("missing key")`.
- `std/collections/string_dict.mojo` — the insertion-ordered owning
  `StringDict[V]` used for homogeneous `**kwargs`; a call builds it through
  its empty constructor and one `_insert` per collected keyword, as upstream's
  call does, and passes it, or the dictionary a `**kwargs^` forwards, whole.
- `std/algorithms.mojo` — small generic helpers that exercise comptime-guided library
  code: type predicates, CTFE-computed constants, value parameters, and associated
  compile-time facts. It includes `first_or[C: Iterable]`, which consumes
  `C.Element` through an opaque iterable bound.
- `std/hash/` — current Mojo's hashing package: `hash.mojo` is the
  `Hashable` home and defines `hash[T: Hashable, //, HasherType: Hasher =
  default_hasher](x) -> UInt64` and the bytes overload `hash[HasherType](bytes:
  ImmPointer[UInt8, _], n: Int)`; `hasher.mojo` is the `Hasher` home and
  defines `default_hasher` (the zero-keyed `AHasher[SIMD[DType.uint64,
  4](0)]`) and `default_comp_time_hasher` (`Fnv1a`); `_ahash.mojo` (the
  keyed `AHasher[key: U256]`, `hash_seeded`, `hash_seeded_bytes`) and
  `_fnv1a.mojo` port the two algorithms in upstream's `_update_with_simd(mut
  self, value: SIMD[_, _])` spelling over `to_bits`/`.length` (a 32-bit-limb
  folded multiply stands in for the 128-bit product; the rotation is spelled
  inline). `hash.mojo` declares upstream's `Hashable` (the prelude exports
  it) and `hasher.mojo` the `Hasher` trait with Mojito's requirement set
  (R25); the reflective `__hash__` default is still synthesized by the
  compiler (R1481). The compiler
  instantiates the `_update_with_simd` template per hashed vector type, which
  `std.simd`'s `SIMD.__hash__` calls with `-0.0` folded and which takes a
  `Bool` or `DType` leaf as `Scalar[DType.bool]` or `UInt8`, and a string's
  bytes as a `Span[Byte, _]`
  (`_update_with_bytes`). The containers (`List`, `Array`, `Set`, `Dict`,
  `Optional`, `Variant`, `Tuple`) conform to `Hashable` conditionally in
  upstream's bodies. Values match the audited head on both backends.
- `std/bit.mojo` — `rotate_bits_left[shift: Int](x: UInt64)`, the `std.bit`
  subset (a `UInt64` instance of upstream's SIMD-generic helper).
- `std/math/` — upstream's `std.math` package: `__init__.mojo` re-exports
  the ported names, `constants.mojo` holds `pi`, `e`, `tau`, and `log2e`
  (`from std.math.constants import log2e`; the package does not re-export
  it), and `math.mojo` holds upstream's nine operation traits (`Absable`,
  `Powable`, `Roundable`, `DivModable`, `Ceilable`, `Floorable`,
  `Truncable`, `CeilDivable`, `CeilDivableRaising`), the rounding helpers
  `floor`/`ceil`/`trunc`/`ceildiv`, each generic over its trait, `copysign`,
  `ldexp` over `_ldexp_impl` (with its AVX-512 `scalef` arm), `exp` over
  `_exp_impl` and `_exp_taylor`'s `polynomial_evaluate`, with the
  `_Expable` trait of its generic overload, and the libm
  constants `_SQRT2`, `_LN2`, `_LN2_HI_F64`, and
  `_LN2_LO_F64`. `polynomial.mojo` holds `polynomial_evaluate` and
  `_horner_evaluate` over compile-time `Span` coefficients
  (`from std.math.polynomial import polynomial_evaluate`; the package does
  not re-export it). The package also re-exports `std.utils.numerics`' `inf`,
  `nan`, `isnan`, `isinf`, and `isfinite`, whose classifiers call
  `llvm.is.fpclass`. The prelude
  exports `Absable`, `Powable`, and `Roundable`, as upstream's does; the rest
  must be imported (`from std.math import floor`). `abs`/`round`/`divmod` are
  still compiler builtins (R1458). Every `SIMD` declares `Absable`,
  `Ceilable`, `Floorable`, and `Truncable` in `std/simd.mojo`; for
  `Powable`, `Roundable`, `DivModable`, and `CeilDivable`, `Int`, `UInt`, and
  `Float64` conform as upstream's `SIMD` declares and answer each
  requirement intrinsically, and other `SIMD` values do not yet (R1623,
  R1567, R1582).
- `std/span.mojo` — the prelude-exported borrowed view `Span[mut: Bool, //,
  T: AnyType, origin: Origin[mut=mut]]` (upstream's bound): a multi-element origin-bearing
  pointer (`Pointer[T, origin._get_owned_interior["element"]]`) plus a
  length, constructed from `ref [origin] list: List[T]` so the source stays
  lent while any copy of the view lives. Element access is a reference
  result with abort bounds; the strict `ContiguousSlice` overload returns a
  sub-view; there is no strided slicing. It is `Boolable`, `Sized`, and
  conditionally `Writable`, with `==`/`!=` and `in` over `Equatable`
  elements. `List`'s own `ContiguousSlice` overload returns a `Span` over
  the list's owned elements, as upstream's does.
- `std/os/` — upstream's `os` package subset: `os.mojo` (`sep`, `SEEK_*`,
  `listdir` over a private `opendir`/`readdir` stream read as glibc `dirent`
  bytes, `mkdir`/`makedirs`, `remove`/`unlink`, `rmdir`/`removedirs`, and
  `abort(message)` — the uncatchable trap crossing to the VM through the
  compiler-private `_mojito_abort` primitive, which stdlib-internal trap sites
  call directly to avoid import cycles), `env.mojo` (`getenv`/`setenv`/
  `unsetenv`), `pathlike.mojo` (re-exports the `PathLike` trait declared beside
  `String`), `_linux_x86.mojo` (the scalar-only `_c_stat` record glibc's
  `__xstat`/`__lxstat` fill), and `path/path.mojo` (`exists`, `lexists`,
  `isdir`, `isfile`, `islink`, `getsize`, `is_absolute`, `join`, `split`,
  `basename`, `dirname`, `split_extension`, `splitroot`, `expandvars`,
  `expanduser`). Every host call is upstream's `external_call` over the closed
  libc table (`std/ffi`).
- `std/io/` — `file.mojo` (`FileHandle` over a raw descriptor and `open`;
  both prelude names) and `file_descriptor.mojo` (`RawFileDescriptor`, a
  `Writer` over a descriptor; `print(file=)` writes through it). `std/sys/`
  adds `_io.mojo` (`stdin`/`stdout`/`stderr` as compile-time
  `RawFileDescriptor` values). Because the prelude imports `std.io`, every
  module on its import graph (`std.os`, `std.sys`, `std.stat`, ...) loads
  while the prelude bootstraps and must import `String` explicitly (`from
  std.string import String`); `range` is not visible there either.
- `std/pathlib/` — `path.mojo` (`Path`, `cwd`, `DIR_SEPARATOR`; `joinpath`
  iterates its pack and binds each view before `/=`, the shape both backends
  run). `std/tempfile/` — `tempfile.mojo` (`gettempdir`, `mkdtemp`,
  `TemporaryDirectory`; `_get_random_name` reads `/dev/urandom` until
  `std.random` lands).
- `std/ffi/__init__.mojo` — the `c_*` scalar aliases, the `external_call`
  builtin's import home, `CStringSlice` (declared in `std/string.mojo`, which
  loads first) and `get_errno` re-exports.
- `std/sys/info.mojo` — upstream's `CompilationTarget` (`current`,
  `_unknown`, `_has_feature`, the `has_*` x86 queries), `simd_bit_width`,
  `simd_byte_width`, `is_32bit`, `is_64bit`, and the layout queries
  `size_of`, `align_of`, `bit_width_of`, and `simd_width_of` over KGEN's
  `get_sizeof` and `get_alignof`. The queries read the `!kgen.target`
  attribute the elaborator answers `_current_target()` with; the
  string-field predicates are roadmap R1635.
- `std/sys/_libc_errno.mojo` — `ErrNo` (`String(err)` renders glibc's
  `strerror` text), `get_errno`, `set_errno`. `std/stat/` — the `S_IF*`
  mode constants and `S_ISDIR`/`S_ISREG`/`S_ISLNK`. `std/time/time.mojo` —
  the C `timespec` record `_CTimeSpec`.
- `std/string.mojo` also hosts `StringSpan[mut: Bool, //, origin]` (the
  canonical string view; `StringSlice` is a never-emitted annotation alias),
  upstream's `StaticString` alias (`StringSpan[ImmStaticOrigin]`; a
  literal binds a `StaticString` compile-time parameter through the
  `@implicit` literal constructor, which the elaborator runs into a frozen
  view of the literal's bytes) and `_GraphemeIter`, the grapheme-cluster
  iterator behind ordinary String/StringSpan/StringLiteral iteration.
- `std/collections/string/string_span.mojo` — upstream's
  `_get_kgen_string[string: StaticString, *extra: StaticString]()` at its
  upstream module path, the `!kgen.string` of the views' bytes, which
  `std/sys/intrinsics.mojo`'s `llvm_intrinsic` reads its `intrin` through,
  and `get_static_string`, the `StaticString` over that string. The rest of
  upstream's module, including the `StaticString(_kgen)` initializer
  `get_static_string` calls, lives in `std/string.mojo`.

Underscore-prefixed structs such as `_ListIter` are implementation details,
following the Python convention that Mojo currently inherits. `DictEntry` is
public, matching Mojo's item-view element. Mapping views are self-iterable,
non-indexable borrowing views that hold a loan on their source while they
live: mutating the mapping while a view is live is rejected, and value/entry
yields are read-only (a conservative subset of upstream's mut-following
value references).

The register VM executes the ordinary MIR produced for these declarations;
`tests/self_host_test.rs` links and runs them. Public List/Tuple runtime variants
have already been retired. The remaining private storage bridges are
documented in the architecture and roadmap.
