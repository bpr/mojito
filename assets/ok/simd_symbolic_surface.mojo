# The SIMD surface a symbolic lane licenses at the template: each body is
# keyed on a `DType` or width parameter, holds a `comptime if`, and is
# validated once with `Scalar[dt]`/`SIMD[dt, width]` symbolic before any
# clone is minted — a dtype gate (`&` on an integer lane, `/` on a float
# lane) is the instantiation's to check, an integer or float literal splats
# into the symbolic lane, a compile-time binding of the symbolic dtype keys
# a lane of its own, and a width expression (`width * 2`, `n + 1`) compares
# in the pin's normal form. A call that spells no lane takes it from the
# argument's own type, whether or not the body holds a `comptime if`.
# requires: discovery


def bit_and[dt: DType](a: Scalar[dt], b: Scalar[dt]) -> Scalar[dt]:
    comptime if dt == DType.bool:
        return a
    return a & b


def guarded[dt: DType](a: Scalar[dt], b: Scalar[dt]) -> Scalar[dt]:
    comptime if dt.is_integral():
        return ((a << 1) | (b >> 1)) % 7
    else:
        return a / b


def literals[dt: DType](a: Scalar[dt]) -> Scalar[dt]:
    comptime if dt.is_floating_point():
        var x: Scalar[dt] = 2.5
        return a + x + 1.5
    var y: Scalar[dt] = 3
    return -a * y + 1


def rebound[dt: DType](a: Scalar[dt]) -> Scalar[dt]:
    comptime lane = dt
    comptime if dt == DType.bool:
        return a
    var doubled: Scalar[lane] = a + a
    return Scalar[lane](doubled) + Scalar[lane](1)


def inferred_lane[dt: DType](a: Scalar[dt], b: Scalar[dt]) -> Scalar[dt]:
    return a * b + 1


def conversions[dt: DType](a: Scalar[dt]) -> String:
    comptime if dt == DType.float64:
        return String(Float64(a))
    return String(Int(a)) + " " + String(Bool(a)) + " " + String(a.cast[DType.int32]())


def lanes[dt: DType, width: Int](v: SIMD[dt, width]) -> Scalar[dt]:
    comptime if width == 1:
        return v[0]
    return v[0] + v[width - 1] + v.reduce_add()


def mask[dt: DType, width: Int](v: SIMD[dt, width]) -> SIMD[DType.bool, width]:
    comptime if width == 1:
        return v == v
    return v.lt(v + 1)


def doubled[dt: DType, width: Int](v: SIMD[dt, width]) -> SIMD[dt, width * 2]:
    comptime if width == 1:
        return v.join(v)
    return v.join(v)


def reordered[dt: DType, n: Int](x: SIMD[dt, n + 1]) -> SIMD[dt, 1 + n]:
    comptime if n == 0:
        return x
    return x


def sized[dt: DType, width: Int](v: SIMD[dt, width]) -> Int:
    comptime if width == 1:
        return 1
    return v.length + width


def compared[dt: DType](a: Scalar[dt], b: Scalar[dt]) -> Bool:
    comptime if dt == DType.bool:
        return Bool(a == b)
    return Bool(a < b) and Bool(a <= b) or Bool(a != b)


def splat[dt: DType, width: Int]() -> SIMD[dt, width]:
    comptime if width == 1:
        return SIMD[dt, width](0)
    return SIMD[dt, width](1) + Scalar[dt](1)


def shown[dt: DType, width: Int](v: SIMD[dt, width]):
    comptime if width == 1:
        print(v)
    else:
        print(v, String(v[0]), hash(v[0]) == hash(v[0]))


def main():
    print(bit_and[DType.int32](6, 3), bit_and(Int32(6), Int32(3)))
    print(inferred_lane(Int32(6), Int32(3)), inferred_lane(Float32(1.5), Float32(2.0)))
    print(guarded[DType.int32](6, 3), guarded[DType.float64](6, 3))
    print(literals[DType.float32](1.0), literals[DType.int16](2))
    print(rebound[DType.int32](3), rebound[DType.float32](1.5))
    print(conversions[DType.float32](1.5), conversions[DType.int8](7))
    var v = SIMD[DType.int32, 4](1, 2, 3, 4)
    print(lanes[DType.int32, 4](v), lanes[DType.int32, 1](Int32(9)))
    print(mask[DType.int32, 4](v))
    print(doubled[DType.int32, 4](v))
    print(reordered[DType.int64, 3](SIMD[DType.int64, 4](7)))
    print(sized[DType.int32, 4](v))
    print(compared[DType.float32](1.5, 2.5))
    print(splat[DType.uint8, 2]())
    shown[DType.int32, 4](v)
