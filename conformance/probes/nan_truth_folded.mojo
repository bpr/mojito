# PROBE (re-probe): is `Bool` of a NaN the same whether or not the pin folds
# the compare?
#
# Found while landing R1553 (2026-10-28) against Mojo 1.2.0.dev2026101705
# (46f49943). `SIMD.__bool__` is `pop.cmp ne` against zero, then a
# reduction. The pin lowers `pop.cmp ne` over float lanes to the ordered
# `fcmp one`, so a NaN made at run time is `False`, but its compile-time
# folder answers the same compare unordered: `zero / zero` over a `var
# zero = 0.0` it can fold prints `True`. The pin prints `True` then
# `False`; Mojito folds and runs the ordered predicate and prints `False`
# twice. `assets/ok/pliron_convert_bool.mojo` builds its NaN at run time
# for that reason.
#
# Run:    mojo run nan_truth_folded.mojo
#         cargo run -- run conformance/probes/nan_truth_folded.mojo

def runtime_zero() -> Float64:
    var total = 0.0
    for i in range(2):
        total += Float64(i) - Float64(i)
    return total


def main():
    var zero = 0.0
    print(Bool(zero / zero))
    var late = runtime_zero()
    print(Bool(late / late))
