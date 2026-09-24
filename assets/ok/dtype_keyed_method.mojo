# A method may take its own [dt: DType] or [w: SIMDLength] value parameter
# and use it in Scalar[...]/SIMD[...] positions, exactly as a free def does:
# each call mints a clone whose signature spells the bound value, whether the
# lane is applied explicitly or read off the argument, and whether the method
# is an instance method, a static one, or a method of a generic struct.
# requires: discovery
struct Lanes:
    var tag: Int

    def __init__(out self, tag: Int):
        self.tag = tag

    def double[dt: DType](self, a: Scalar[dt]) -> Scalar[dt]:
        return a + a

    def total[w: SIMDLength](self, v: SIMD[DType.int32, w]) -> Int32:
        return v.reduce_add()

    @staticmethod
    def widen[dt: DType](a: Scalar[dt]) -> Scalar[dt]:
        return a * 3


struct Holder[T: Copyable & Movable & Deinitable]:
    var value: Self.T

    def __init__(out self, value: Self.T):
        self.value = value.copy()

    def double[dt: DType](self, a: Scalar[dt]) -> Scalar[dt]:
        return a + a


def main():
    var lanes = Lanes(1)
    print(lanes.tag)
    print(lanes.double[DType.int32](3))
    print(lanes.double(Int16(5)))
    print(lanes.double(Float32(1.5)))
    print(lanes.total[4](SIMD[DType.int32, 4](1, 2, 3, 4)))
    print(Lanes.widen(UInt8(100)))
    var holder = Holder[Int](7)
    print(holder.value, holder.double[DType.int64](9))
