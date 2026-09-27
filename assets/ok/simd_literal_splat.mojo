# An exact literal builds a multi-lane vector wherever one is expected:
# upstream's implicit `SIMD(IntLiteral)` and `SIMD(FloatLiteral)` splat it
# across every lane.
@fieldwise_init
struct Lanes(Copyable, Movable):
    var v: SIMD[DType.int32, 4]


struct Wide[width: Int](Copyable, Movable):
    var v: SIMD[DType.float32, Self.width]

    def __init__(out self, v: SIMD[DType.float32, Self.width]):
        self.v = v


def ones() -> SIMD[DType.int32, 4]:
    return 1


def doubled(x: SIMD[DType.float64, 2] = 0.5) -> SIMD[DType.float64, 2]:
    return x * 2


def total(x: SIMD[DType.uint8, 4]) -> Int:
    return Int(x.reduce_add())


def main():
    var p = Lanes(7)
    print(p.v)
    p.v = 3
    print(p.v)
    var q: SIMD[DType.int32, 4] = 2
    print(q)
    q = 5
    print(q, ones())
    print(doubled(), doubled(1.25))
    print(total(9))
    var w = Wide[4](2.5)
    print(w.v)
    var t: SIMD[DType.int8, 8] = -1
    print(t)
