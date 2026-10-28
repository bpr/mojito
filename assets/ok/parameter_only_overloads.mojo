# Overloads that differ only in their compile-time parameters are distinct
# callables, as `bit_width_of[type: ...]()` beside `bit_width_of[dtype: ...]()`
# are at the pin; a method parameter defaulting to its owner's parameter
# reads the receiver's argument; and a leading-dot `DType` member or a
# `DType`-valued compile-time call names a `SIMD` dtype.


def width_of[type: AnyType]() -> Int:
    return 1


def width_of[dtype: DType]() -> Int:
    return 2


struct Holder[dtype: DType, k: Int](TrivialRegisterPassable):
    var x: Int

    def __init__(out self, x: Int):
        self.x = x

    @staticmethod
    def kind[type: AnyType]() -> Int:
        return 3

    @staticmethod
    def kind[dt: DType]() -> Int:
        return 4

    def count[n: Int = Self.k](self) -> Int:
        return n

    def lanes[t: DType = Self.dtype](self) -> Int:
        return 7 if t == Self.dtype else 8


@always_inline("builtin")
def wide_unsigned[width: Int]() -> DType:
    return DType.uint8 if width == 8 else DType.uint64


def main():
    print(width_of[Int](), width_of[DType.int8](), width_of[dtype=DType.int8]())
    print(Holder[DType.int8, 1].kind[String](), Holder[DType.int8, 1].kind[DType.bool]())
    var h = Holder[DType.int8, 5](0)
    print(h.count(), h.count[n=2](), h.lanes(), h.lanes[t=DType.int16]())
    var x: SIMD[.int32, 2] = 3
    var y: SIMD[.float64, 1] = 3
    comptime w = 64
    print(x, y, UInt8(250).cast[wide_unsigned[w]()]())
