# A runtime scalar builds a multi-lane vector of its dtype wherever one is
# expected: upstream's implicit `SIMD.__init__(Scalar[dtype])` splats it
# across every lane.
@fieldwise_init
struct Lanes(Copyable, Movable):
    var v: SIMD[DType.float32, 2]
    var n: Int


struct Acc:
    var total: SIMD[DType.int64, 4]

    def __init__(out self, seed: Int64):
        self.total = seed

    def add(mut self, v: SIMD[DType.int64, 4]):
        self.total += v


def pair_sum(v: SIMD[DType.float64, 2]) -> Float64:
    return v[0] + v[1]


def ints(x: Int) -> SIMD[DType.int, 4]:
    return x


def threes(v: SIMD[DType.int32, 4] = Int32(3)) -> SIMD[DType.int32, 4]:
    return v


def widen[dt: DType](x: Scalar[dt]) -> SIMD[dt, 4]:
    return x


def fill[dt: DType, w: Int](x: Scalar[dt]) -> SIMD[dt, w]:
    var v: SIMD[dt, w] = x
    return v


def main():
    var x = Int32(9)
    var v: SIMD[DType.int32, 4] = x
    print(v)
    v = Int32(2)
    v += x
    print(v)
    var f = Float64(1.5)
    print(pair_sum(f), ints(3), threes())
    var acc = Acc(Int64(1))
    var k = Int64(3)
    acc.add(k)
    acc.add(Int64(4))
    print(acc.total)
    var p = Lanes(Float32(1.25), 2)
    print(p.v, p.n)
    print(widen(UInt8(7)), widen[DType.float64](2.0))
    print(fill[DType.int16, 8](Int16(5)))
    var xs = List[SIMD[DType.int32, 2]]()
    xs.append(x)
    print(xs[0])
    var mask = SIMD[DType.bool, 4](fill=True)
    mask = Scalar[DType.bool](False)
    print(mask)
