# PROBE (API-shape pinning): Variant's owning operations at the audited
# head — `unwrap`/`unsafe_unwrap` semantics (does `unwrap` trap or raise on
# a tag mismatch?), the exact `set(init_with=…)` spelling, whether
# `deinit_with` takes a generic handler (`fn[T](deinit T)`) or something
# else, and whether `set` really requires every alternative Deinitable.
#
# Context: nightly §6. Mojito: `v.unwrap[T]()` traps on mismatch,
# `v.set[T](init_with=factory)` with a zero-parameter factory,
# `v.deinit_with(handler)` accepting a monomorphic or generic consuming
# handler checked against every alternative, and an all-alternatives
# `Deinitable` gate on both `set` forms. Dispatch lives in
# src/checker/inference.rs (`infer_variant_method`).
#
# Run:    mojo run variant_owning_surface.mojo
#         cargo run -- run conformance/probes/variant_owning_surface.mojo
#
# If the spellings differ: adjust the intrinsic name list, the fixtures
#   assets/ok/variant_owning_api.mojo /
#   assets/type_error/variant_set_linear_alternative.mojo, and parity row
#   types.variant.
from utils import Variant

def consume[T: Movable](deinit element: T):
    print("consumed")

def main():
    var v: Variant[Int, String] = Variant[Int, String](5)
    print(v.unwrap[Int]())
    var w: Variant[Int, String] = Variant[Int, String](1)
    w.set[Int](init_with=lambda () -> Int: 42)
    w.deinit_with(consume)
