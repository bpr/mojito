# The Hasher Protocol Subset: Where Mojito's `std.hashlib` Narrows Mojo's

Status: implemented behavior as of the hasher-based `Hashable` alignment
(2026-09). Book destination: source material for *Mojito Internals* chapters
on trait intrinsics and monomorphization, and a Part VIII case study in
matching values while subsetting spellings.

`hash(x)` in Mojito prints the same number the pinned upstream Mojo prints
for scalars, strings, literals, and user conformers in both accepted
spellings (`conformance/cases.tsv` rows `hashlib-values`,
`hashlib-explicit-hashers`, `hashable-user-struct`). The protocol and module
identity are upstream's. Four spellings underneath are narrower; each is a
deliberate subset position rather than a fork:

1. **`_update_with_simd(mut self, value: UInt64)`** instead of
   `SIMD[_, _]`. Mojito cannot spell wildcard SIMD parameters and has no
   `to_bits()`. Both upstream hashers reduce every lane of at most eight
   bytes to one `u64` mix (Fnv1a: `rounds = max(1, size // 8) = 1`; AHasher:
   `to_bits[.uint64]()[0]`), so the compiler normalizes each leaf to its
   unsigned bit pattern zero-extended to `UInt64` (`-0.0` folded to `0.0`,
   as upstream folds on the bit pattern) and the values are identical. A
   user hasher written in the upstream spelling is rejected (a subset gap);
   one written in Mojito's spelling would not conform upstream (recorded in
   `docs/features.md`). Width-greater-than-one SIMD is not hashable here.
2. **Key-less `AHasher`.** Upstream's `AHasher[key: U256]` needs a
   SIMD-typed value parameter; Mojito's `AHasher` folds the key to zero,
   which is `default_hasher`'s key, and keeps the seeded initializer and
   `hash_seeded`. `AHasher[U256(0)]` spelled by a user is rejected.
3. **Pure-Mojo `_folded_multiply`.** No 128-bit dtype exists, so the
   128-bit product is assembled from 32-bit limbs with wrapping `UInt64`
   arithmetic; the rotation is spelled inline (a value-parameterized helper
   cannot cross the compile-time execution boundary). Both were checked
   bit-for-bit against upstream's test vectors.
4. **`StringSpan.__hash__` copies its bytes** into a `List[Byte]` before
   `_update_with_bytes`, because `Span` has no pointer-backed constructor
   yet; `String.__hash__` delegates to `StringSpan` as upstream does.

Two leniencies remain on the acceptance side, both recorded: `Hasher`, like
`Writer`, resolves without `from std.hashlib import Hasher`; and
`StringLiteral` satisfies `Hashable` (it hashes as the `String` it
materializes to), which `StringDict`'s literal-keyed entries rely on.
