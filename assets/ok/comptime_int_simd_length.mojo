# A `comptime` binding annotated `Int` over a `SIMDLength` value binds its
# lane count, as an `Int` parameter slot does: read directly, as an
# operator's receiver, in a bracket argument, and through `Self`.


def lanes[k: Int]() -> Int:
    return k * 10


def f[w: SIMDLength]():
    comptime n: Int = w
    comptime m = n + 1
    print(n, m, n * 2, lanes[n]())
    print(SIMD[DType.int32, n](1))


@fieldwise_init
struct S[w: SIMDLength]:
    def show(self):
        comptime n: Int = Self.w
        print(n - 1)


def main():
    f[4]()
    f[8]()
    S[2]().show()
