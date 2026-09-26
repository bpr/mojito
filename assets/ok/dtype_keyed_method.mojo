# A method may take its own [dt: DType] or [w: SIMDLength] value parameter
# and use it in Scalar[...]/SIMD[...] positions, exactly as a free def does:
# each call mints a clone whose signature spells the bound value, whether the
# lane is applied explicitly or read off the argument, and whether the method
# is an instance method, a static one, or a method of a generic struct. The
# body may also construct a vector at that lane (`Scalar[dt](x)`).
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

    def make[dt: DType](self, x: Int) -> Scalar[dt]:
        return Scalar[dt](x + self.tag)

    def splat[w: SIMDLength](self, x: Int32) -> SIMD[DType.int32, w]:
        return SIMD[DType.int32, w](x)

    @staticmethod
    def zero[dt: DType]() -> Scalar[dt]:
        return Scalar[dt](0)


struct Holder[T: Copyable & Movable & Deinitable]:
    var value: Self.T

    def __init__(out self, value: Self.T):
        self.value = value.copy()

    def double[dt: DType](self, a: Scalar[dt]) -> Scalar[dt]:
        return a + a

    def make[dt: DType](self, x: Int) -> Scalar[dt]:
        return Scalar[dt](x)


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
    print(lanes.make[DType.int32](6), lanes.make[DType.float64](2))
    print(lanes.splat[4](3))
    print(Lanes.zero[DType.uint8]())
    print(holder.make[DType.int16](5))
