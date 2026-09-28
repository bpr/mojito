# PROBE: comparing tuples whose elements are SIMD scalars.
#
# **Differs.** The pin runs it and prints `True False`: `UInt64` conforms to
# `Comparable` and `Equatable`, so `Tuple[UInt64, UInt64]`'s comparisons are
# available. Mojito rejects it, "operator '<' is not defined for
# Tuple$t2[...]": its SIMD scalar conforms to neither trait, so the tuple's
# `where conforms_to(Self.Ts.values, Comparable)` clause folds false and each
# comparison is an unavailable trap stub. Filed in `docs/roadmap.md` §3. When
# Mojito runs it, promote this file to `assets/ok/` with its manifest rows.
#
# Observed 2026-09-28 against `Mojo 1.2.0.dev2026092105 (e9569894)`.
#
# Run:    mojo run tuple_of_scalars_comparison.mojo
#         cargo run -- run conformance/probes/tuple_of_scalars_comparison.mojo


def main():
    var a = (UInt64(1), UInt64(2))
    var b = (UInt64(1), UInt64(3))
    print(a < b, a == b)
