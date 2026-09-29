# A SIMD value constructs with no arguments: upstream's `SIMD.__init__()`
# zeroes every lane, and `SIMD` conforms to `Defaultable`, so a bound
# `T()` builds one too.
def make[T: Defaultable & Writable]() -> T:
    return T()


def main():
    print(Float32())
    print(UInt8())
    print(Scalar[DType.int16]())
    print(SIMD[DType.int32, 2]())
    print(SIMD[DType.float64, 4]())
    print(SIMD[DType.bool, 2]())
    var x: Float32 = make[Float32]()
    x += 1.5
    print(x)
    print(make[Int8](), make[SIMD[DType.uint16, 4]]())
