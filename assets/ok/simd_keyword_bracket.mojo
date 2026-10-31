# A `SIMD[...]` application binds its brackets through the declaration, as
# any struct's do: a keyword `length=` or `dtype=`, in either order, in a
# signature, a binding, a construction, and a partial alias.
comptime Sc = SIMD[_, length=1]


def twice[dt: DType, //](x: SIMD[dt, length=1]) -> SIMD[dt, 1]:
    return x + x


def main():
    print(twice(Int32(3)))
    var s: Sc[DType.int32] = 4
    print(s)
    var v: SIMD[dtype=DType.int32, length=4] = 2
    print(v)
    print(SIMD[length=1, dtype=DType.int8](3))
    print(SIMD[DType.int32, length=2](1, 2))
