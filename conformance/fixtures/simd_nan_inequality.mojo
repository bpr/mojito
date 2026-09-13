# `SIMD.ne` is the ordered predicate: a NaN lane is unequal to nothing, itself
# included, while the infix `!=` on a scalar stays IEEE. The NaN is made at run
# time, because the pin folds a constant `0.0 / 0.0` receiver's `ne` to the
# unordered answer instead.
def runtime_zero() -> Float64:
    var total = 0.0
    for i in range(3):
        total += Float64(i) - Float64(i)
    return total

def main():
    var zero = SIMD[DType.float64, 4](runtime_zero())
    var nan = zero / zero
    var f = SIMD[DType.float64, 4](1.0, -1.0, 0.0, 2.5)
    print(nan.ne(f), nan.ne(nan), f.ne(SIMD[DType.float64, 4](1.0, 1.0, 0.0, 0.0)))
    var half = SIMD[DType.float32, 2](Float32(runtime_zero()))
    print((half / half).ne(SIMD[DType.float32, 2](1.0)))
    var s = runtime_zero() / runtime_zero()
    print(s != s, s != 1.0)
