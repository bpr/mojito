# PROBE: is a `Tuple` element assignable through its compile-time-index hook?
#
# The pin accepts `t[0] = 9` and prints `9`: `Tuple.__getitem__[idx](ref self)`
# returns `ref [self]`, so the subscript is a mutable place. Mojito rejects it
# with "invalid assignment target: Tuple elements are immutable", a checker
# rule that predates the reference-returning `__getitem_param__` the bundled
# `std/builtin/tuple.mojo` now declares.
#
# Mojito is stricter here, so the divergence is legal under the subset rule but
# is on the ledger: `docs/roadmap.md` §3, `tuple-element-write`. Withdrawing
# the rule also decides `std/collections/pack_tuple.mojo`, whose accessor
# returns a copied value precisely to keep the write rejected
# (`tests/self_host_test.rs::self_hosted_pack_tuple_preserves_tuple_restrictions`).
#
# Observed 2026-09-20 against `Mojo 1.1.0.dev2026082605 (dd957314)`.
#
# Run:    mojo run tuple_element_write.mojo
#         cargo run -- run conformance/probes/tuple_element_write.mojo
def main():
    var t = Tuple(1, True)
    t[0] = 9
    print(t[0])
