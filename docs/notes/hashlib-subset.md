# The Hasher Protocol: How Mojito Reaches Upstream's Spellings

Status: implemented behavior as of the hashing-parity task (2026-09). Book
destination: source material for *Mojito Internals* chapters on trait
intrinsics, per-type method clones, and monomorphization, and a Part VIII
case study in matching values while subsetting spellings.

`hash(x)` in Mojito prints the number the pinned upstream Mojo prints for
scalars, vectors, strings, literals, user conformers in both accepted
spellings, and the container conformances (`conformance/cases.tsv` rows
`hashlib-values`, `container-hashable`, `hashlib-keyed-ahasher`,
`hashlib-bytes-hash`, `comptime-hash`). The protocol, the module identity,
and the stdlib spellings are upstream's; the mechanisms underneath are
Mojito's:

1. **`_update_with_simd(mut self, value: SIMD[_, _])` is a generator.**
   The elaborator desugars the wildcard parameter to upstream's pair of
   infer-only binders, a `DType` and a `SIMDLength` width over
   `SIMD[dtype, length]`, which each hashed vector solves. The template
   checks once with both binders symbolic, and `native::mono` instantiates
   it at every leaf type it reaches (since 2026-10-05; before that the
   elaborator minted one clone per vector type, the width-1 set eagerly).
   A scalar leaf's dispatch selects the instance whose value parameter has
   the leaf's lane shape, passes the value itself with `-0.0` folded
   (upstream's `SIMD.__hash__`), and a `Bool` leaf as `Scalar[DType.bool]`;
   an erased run, compile-time evaluation included, calls the template with
   the binders read off the value. Bodies read lanes with
   `to_bits[DType.uint64]()` and `.length` (both compiler intrinsics), in
   `while` loops because `range` is not visible inside `std.hashlib`. A
   width binder declared `Int` (`[dtype: DType, width: Int]`) is not
   inferred, in the pin as in Mojito, so the bundled bodies spell the
   wildcard.
2. **`AHasher[key: U256]` is a vector-keyed value specialization.** A
   `comptime U256 = SIMD[DType.uint64, 4]` alias used as a parameter bound
   folds to the parser's value-type spelling; the struct monomorphizes per
   application like a DType-keyed one, with a compile-time SIMD value
   (`CtValue::Simd`, lane-wise `^ & |`) baking `Self.key` into clone bodies.
   A concrete application (`comptime default_hasher = AHasher[SIMD[DType.
   uint64, 4](0)]`, an `Index` parse shape) resolves to one canonical
   identity — the mangled clone — for the checker's default fill,
   `ConstructTypeParam`, monomorphization, and the specialization oracle
   (which answers for a clone through `demangle_specialization`), and
   `_unqualified_type_name` spells the clone with its baked values, so
   `repr(set)` prints upstream's `Hasher=AHasher[[0, 0, 0, 0] :
   SIMD[DType.uint64, 4]]`. The zero-keyed hasher is spelled once in
   `_ahash.mojo` for the seeded entry points.
3. **Pure-Mojo `_folded_multiply`.** No 128-bit dtype exists, so the 128-bit
   product is assembled from 32-bit limbs with wrapping `UInt64` arithmetic;
   the rotation is spelled inline. Both were checked bit-for-bit against
   upstream's test vectors. This is the one remaining narrowing.
4. **The bytes overloads bind a placeholder-origin pointer.** `ImmPointer[T,
   _]` in a free function resolves to the unsafe-any provenance at the
   alias's permission (any pointer binds it; the callee holds no loan), Span's
   pointer constructor takes `Pointer[Self.T, Self.origin]` (an unsafe-any
   pointer binds the slot untracked), and the bare `Pointer[T, _]` reads as
   the immutable alias — upstream infers the origin per call and likewise
   rejects writes through it.
5. **Compile-time hashing runs the VM.** A call to a type-parameterized free
   function with a constructible parameter routes through a synthesized
   checked entry (folded type arguments, typed literal arguments), the effect
   walk admits every deterministic body (the hasher protocol needs no
   allowlist), and the VM-CTFE subprogram carries the keyed `default_hasher`
   clone at its template's position. Scalar, Bool, Float64, and string
   arguments fold. A compile-time dictionary or set display folds with the
   default hasher, as upstream; the explicit literal constructor
   (`Dict[K, V, default_comp_time_hasher](keys, values, None)`) is the one
   spelling that carries `Fnv1a`, and the value materializes back as that
   constructor.

Two leniencies remain on the acceptance side, both recorded: `Hasher`, like
`Writer`, resolves without `from std.hashlib import Hasher`; and
`StringLiteral` satisfies `Hashable` (it hashes as the `String` it
materializes to), which `StringDict`'s literal-keyed entries rely on.
