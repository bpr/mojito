# A literal constant's read stays the literal, and each use converts it as
# it converts a literal: a splat into a vector, and two reads of one
# constant at two types in one function.
comptime K = 3
comptime HALF = 0.5


struct A:
    comptime K = 7


def show[T: Writable & Copyable](x: T):
    print(x)


def scaled[dtype: DType, width: Int](x: SIMD[dtype, width]) -> SIMD[dtype, width]:
    return x.fma(HALF, 0.25) * K


def main():
    print(SIMD[DType.int32, 2](1) * K, K * SIMD[DType.int8, 2](1))
    show(K)
    var l = List[Float32]()
    l.append(K)
    var f: Float32 = K
    print(l[0], f, Int32(1) * K)
    var x = SIMD[DType.int32, 4](1)
    x += K
    print(x, A.K * SIMD[DType.int8, 2](1), Int8(1) * A.K)
    print(scaled[DType.float32, 2](SIMD[DType.float32, 2](2.0)))
    print(scaled[DType.float64, 1](1.0))
