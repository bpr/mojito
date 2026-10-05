# A lane gather in a `DType`- or width-keyed `def` is served by its template:
# a `shuffle`, `slice`, or `join` whose receiver width, output width, offset,
# or lane index names the `def`'s binders crosses the waist with its mask in
# the method's form, and the elaborator closes the mask per instance and
# checks it against the method's constraints, as the pin does. A gather in an
# untaken `comptime if` arm is never checked; one in a `comptime for` reads
# the loop index.
def halves[dt: DType, w: Int](v: SIMD[dt, w]) -> SIMD[dt, w // 2]:
    return v.slice[w // 2, offset = w // 2]()


def first2[dt: DType, w: Int](v: SIMD[dt, w]) -> SIMD[dt, 2]:
    return v.slice[2, offset=1]()


def swap[dt: DType, w: Int](v: SIMD[dt, w]) -> SIMD[dt, w]:
    return v.shuffle[1, 0]()


def picked[dt: DType, n: Int](v: SIMD[dt, 2]) -> SIMD[dt, 2]:
    return v.shuffle[n, 0]()


def rotated[dt: DType, w: Int](v: SIMD[dt, w]) -> SIMD[dt, w]:
    return v.shuffle[w - 1, 0]()


def joined[dt: DType, w: Int](v: SIMD[dt, w]) -> SIMD[dt, w * 2]:
    return v.join(v + 1)


def upper[dt: DType, w: Int](v: SIMD[dt, w]) -> SIMD[dt, 2]:
    comptime if w >= 4:
        return v.slice[2, offset=2]()
    else:
        return v.slice[2]()


def rotations[dt: DType, w: Int](v: SIMD[dt, w]):
    comptime for i in range(2):
        print(v.shuffle[i, 1 - i]())


def main():
    var v = SIMD[DType.int32, 4](1, 2, 3, 4)
    var pair = SIMD[DType.int32, 2](5, 6)
    print(halves[DType.int32, 4](v))
    print(first2[DType.int32, 4](v))
    print(swap[DType.int32, 2](pair))
    print(picked[DType.int32, 1](pair))
    print(rotated[DType.int32, 2](pair))
    print(joined[DType.int32, 4](v))
    print(upper[DType.int32, 4](v))
    print(upper[DType.int32, 2](pair))
    rotations[DType.int32, 2](pair)
    print(joined[DType.float64, 2](SIMD[DType.float64, 2](0.5, 1.5)))
