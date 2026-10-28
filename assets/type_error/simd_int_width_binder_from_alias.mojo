# expect: cannot infer type parameter 'w'
# A local `comptime L = SIMDLength(n)` is an alias of the folded length: the
# slot of `SIMD[dt, L]` holds `L`, not the construction it hides, so an
# `Int` width binder is not solved through it, as at the pin. The explicit
# `SIMD[dt, SIMDLength(n)]` solves (`assets/ok/simd_int_width_binder_inferred.mojo`).
def show[w: Int](x: SIMD[DType.int32, w]):
    print(w, x)


def f[n: Int]():
    comptime L = SIMDLength(n)
    show(SIMD[DType.int32, L](1))


def main():
    f[2]()
