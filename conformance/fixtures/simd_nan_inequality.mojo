# `SIMD.ne` against a NaN receiver: Mojito answers IEEE (a NaN is unequal to
# everything, itself included), while the pin answers False for a NaN against
# a number and True for a NaN against itself — which is neither the ordered
# nor the unordered predicate.
def main():
    var zero = SIMD[DType.float64, 4](0.0)
    var nan = zero / zero
    var f = SIMD[DType.float64, 4](1.0, -1.0, 0.0, 2.5)
    print(nan.ne(f), nan.ne(nan))
