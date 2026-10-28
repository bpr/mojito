# The prelude's `any` and `all` over a `SIMD` vector reduce its lanes'
# truthiness, as upstream's `std/builtin/bool.mojo` overloads do.


def main():
    var mask = SIMD[DType.bool, 4](True, False, False, False)
    print(any(mask), all(mask))
    var ints = SIMD[DType.int32, 4](1, 2, 3, 4)
    print(any(ints), all(ints))
    var zeros = SIMD[DType.float32, 2](0.0, 0.0)
    print(any(zeros), all(zeros))
    print(any(Int8(0)), all(UInt8(3)))
    print(any(ints.eq(3)), all(ints.gt(0)), all(ints.gt(1)))
    if any((ints % 2).eq(0)):
        print("some lane is even")
