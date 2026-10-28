# An `Int` width binder is solved from a vector whose width an `Int`
# expression spells: `SIMD[dt, n]` holds `SIMDLength(n)`, the implicit
# conversion of its width, and `show[w: Int]`'s `SIMD[dt, w]` holds
# `SIMDLength(w)`, so the two constructions unify to `w = n`, as at the pin.
# The width may be a binder, a product, `Int(m)` arithmetic over a
# `SIMDLength`, an `Int` alias, a struct parameter, `join`'s or `slice`'s
# result, an explicit `SIMDLength(n)`, or `Int` of a length alias, and it
# solves through a `List` element too. `Int(m)` of a `SIMDLength` is its
# lane count, so `SIMD[dt, Int(m)]` is `SIMD[dt, m]`.
def show[w: Int](x: SIMD[DType.int32, w]):
    print(w, x)


def count[w: Int](x: List[SIMD[DType.int32, w]]):
    print(w, len(x))


struct Lanes[n: Int]:
    @staticmethod
    def go():
        show(SIMD[DType.int32, Self.n](5))


def over_int[n: Int]():
    var a = SIMD[DType.int32, n](1)
    show(a)
    show(SIMD[DType.int32, n * 2](2))
    comptime k = n * 2
    show(SIMD[DType.int32, k](3))
    show(a.join(a))
    show(a.slice[n // 2]())
    var b: SIMD[DType.int32, SIMDLength(n)] = a
    show(b)
    comptime L = SIMDLength(n * 2)
    show(SIMD[DType.int32, Int(L)](4))
    var items = List[SIMD[DType.int32, n]]()
    items.append(a)
    count(items)


def over_length[m: SIMDLength]():
    show(SIMD[DType.int32, Int(m) * 2](6))
    var same: SIMD[DType.int32, m] = SIMD[DType.int32, Int(m)](7)
    var wider: SIMD[DType.int32, m + 2] = SIMD[DType.int32, Int(m) + 2](8)
    print(same, wider)


def main():
    over_int[2]()
    Lanes[4].go()
    over_length[2]()
