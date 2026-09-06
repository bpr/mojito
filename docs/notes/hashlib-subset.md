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

1. **`_update_with_simd(mut self, value: SIMD[_, _])` is a per-type clone
   family.** The type lattice has only concrete SIMD types, so the
   elaborator desugars the wildcard parameter to an infer-only type
   parameter bounded by the hidden `$SIMD` (any SIMD-valued type) and mints
   one clone per hashed vector type through the ordinary method-clone
   machinery (`_update_with_simd$y3:Int`). The closed width-1 leaf set is
   minted eagerly for every `Hasher` conformer — hashing reaches the hasher
   through erased paths (`hash[T]`, `update(Some[Hashable])`) that record no
   call site, and the VM-CTFE subprogram has no discovery loop — while wider
   vectors arrive through the checker's `hash_leaf_types` demand channel.
   The template body is a trap stub; every backend's leaf dispatch computes
   the exact clone name (`simd_update_clone_name`), passes the value itself
   with `-0.0` folded (upstream's `SIMD.__hash__`), and a `Bool` leaf as
   `Scalar[DType.bool]`. Bodies read lanes with `to_bits[DType.uint64]()`
   and `.length` (both compiler intrinsics), in `while` loops because
   `range` is not visible inside `std.hashlib`; pinned Mojo rejects a
   conformer that names the parameters instead (`[dtype: DType, width:
   Int]`), so the bundled bodies spell the wildcard.
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
   checked entry (folded type arguments, typed literal arguments), the purity
   walk admits the hasher protocol's method calls and the bundled hashers'
   mixing steps, and the VM-CTFE subprogram carries the keyed `default_hasher`
   clone at its template's position. Scalar, Bool, Float64, and string
   arguments fold; compile-time Dict/Set values are a roadmap task.

Two leniencies remain on the acceptance side, both recorded: `Hasher`, like
`Writer`, resolves without `from std.hashlib import Hasher`; and
`StringLiteral` satisfies `Hashable` (it hashes as the `String` it
materializes to), which `StringDict`'s literal-keyed entries rely on.
