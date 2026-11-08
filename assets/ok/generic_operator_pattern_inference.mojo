# A value-parameter pattern that is an operator expression unifies with an
# actual of the same operator node by node: `B[w * 2]` against `B[n * 2]`
# binds `w` to `n`, as does the `SIMDLength(w * 2)` a SIMD width holds.


@fieldwise_init
struct B[k: Int](Copyable):
    pass


def scaled[w: Int](x: B[w * 2]):
    print(w)


def shifted[w: Int](x: B[w + 1]):
    print(w)


def show[w: Int](x: SIMD[DType.int32, w * 2]):
    print(w, x)


def f[n: Int]():
    scaled(B[n * 2]())
    shifted(B[n + 1]())
    show(SIMD[DType.int32, n * 2](1))


def main():
    f[2]()
    f[3]()
