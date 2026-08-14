# PROBE (API-shape pinning): Optional's exact owning surface at the audited
# head — which of `is_some`/`__bool__` exist, the `init_with=` constructor's
# exact spelling (runtime keyword vs comptime parameter; can the factory
# raise?), `take`'s bound and empty-Optional behavior, and whether the
# borrowed iterator yields references (Mojito's yields copies; `for ref`
# over Optional is a recorded subset gap).
#
# Context: nightly §6. Mojito ships `Optional[T: AnyType]` with
# `Optional(value)`, `Optional(init_with=factory)` (zero-parameter,
# non-raising factory), `is_some`/`__bool__`/`or_else`/`value`/`take`,
# `deinit_with`, `deinit_assert_empty`, `map`/`and_then`, borrowed
# (value-yielding) and owned iteration. stdlib/std/optional.mojo owns the
# surface; parity row types.collections records the claim.
#
# Run:    mojo run optional_owning_surface.mojo
#         cargo run -- run conformance/probes/optional_owning_surface.mojo
#
# If a spelling differs (e.g. no `is_some`, a comptime `init_with`, an
#   Iterator conformance): rename/adjust stdlib/std/optional.mojo and the
#   fixtures assets/ok/optional_owning_api.mojo names accordingly.
def main():
    var present = Optional[Int](init_with=lambda () -> Int: 7)
    print(present.is_some(), Bool(present))
    for x in present:
        print(x)
    print(present.take())
