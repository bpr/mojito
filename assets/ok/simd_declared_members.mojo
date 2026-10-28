# `SIMD` is declared in `std.simd`: its `__len__`, `__getitem__`, and
# `__setitem__` are upstream's bodies, reached by explicit calls on a vector
# or scalar receiver, by `len`, through a `Sized` bound, at two widths of a
# generic function, and in compile-time positions.
def first[dt: DType, n: SIMDLength](v: SIMD[dt, n]) -> Scalar[dt]:
    return v.__getitem__(0)


def count[T: Sized](x: T) -> Int:
    return len(x)


def main():
    var n = 5
    print(n.__getitem__(0), n.__len__())
    var v = SIMD[DType.int32, 4](1, 2, 3, 4)
    v.__setitem__(0, 7)
    print(v.__getitem__(0), v.__getitem__(3), v.__len__())
    print(v)
    print(len(v), len(5), len(2.5), count(v), count(Float64(2.5)))
    print(first(v), first(SIMD[DType.float32, 2](1.5, 2.5)))
    var f = Float64(0.5)
    f.__setitem__(0, 4.25)
    print(f, f.__len__())
    comptime c = SIMD[DType.int64, 4](7, 8, 9, 10)
    comptime e = c.__getitem__(2)
    comptime l = c.__len__()
    print(e, l, len(c))
