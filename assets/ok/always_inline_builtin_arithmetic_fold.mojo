# `SIMD`'s arithmetic dunders are `@always_inline("builtin")` bodies over a
# `pop` lane op on the operands' storages, which a compile-time call folds
# where it is spelled: `v.__add__(w)` is `v + w`, `v.__neg__()` is `-v`, and
# an integer literal operand converts to `Self`, so each pair below is one
# type.


struct I[x: Int](Movable):
    def __init__(out self):
        pass


struct U[x: UInt](Movable):
    def __init__(out self):
        pass


struct S[x: Int32](Movable):
    def __init__(out self):
        pass


struct B[x: UInt8](Movable):
    def __init__(out self):
        pass


struct F[x: Float32](Movable):
    def __init__(out self):
        pass


struct V[x: SIMD[DType.float32, 4]](Movable):
    def __init__(out self):
        pass


def ints[v: Int, w: Int]() -> I[v.__add__(w)]:
    var sub: I[v.__sub__(w)] = I[v - w]()
    var mul: I[v.__mul__(w)] = I[v * w]()
    var neg: I[v.__neg__()] = I[-v]()
    var lit: I[v.__add__(1)] = I[v + 1]()
    _ = sub^
    _ = mul^
    _ = neg^
    _ = lit^
    return I[v + w]()


def uints[v: UInt, w: UInt]():
    var add: U[v.__add__(w)] = U[v + w]()
    var mul: U[v.__mul__(w)] = U[v * w]()
    _ = add^
    _ = mul^


def int32s[v: Int32, w: Int32]() -> S[v.__add__(w)]:
    var sub: S[v.__sub__(w)] = S[v - w]()
    var mul: S[v.__mul__(w)] = S[v * w]()
    var neg: S[v.__neg__()] = S[-v]()
    var lit: S[v.__add__(1)] = S[v + 1]()
    var radd: S[v.__radd__(w)] = S[w + v]()
    var both: S[v.__and__(w)] = S[v & w]()
    var either: S[v.__or__(w)] = S[v | w]()
    var shifted: S[v.__lshift__(w)] = S[v << w]()
    var nested: S[v.__add__(w).__mul__(v)] = S[(v + w) * v]()
    _ = sub^
    _ = mul^
    _ = neg^
    _ = lit^
    _ = radd^
    _ = both^
    _ = either^
    _ = shifted^
    _ = nested^
    return S[v + w]()


def bytes[v: UInt8, w: UInt8]():
    var sub: B[v.__sub__(w)] = B[v - w]()
    _ = sub^


def floats[v: Float32, w: Float32]():
    var div: F[v.__truediv__(w)] = F[v / w]()
    _ = div^


def vectors[v: SIMD[DType.float32, 4], w: SIMD[DType.float32, 4]]():
    var add: V[v.__add__(w)] = V[v + w]()
    var sub: V[v.__sub__(w)] = V[v - w]()
    var neg: V[v.__neg__()] = V[-v]()
    var lit: V[v.__add__(2)] = V[v + 2]()
    _ = add^
    _ = sub^
    _ = neg^
    _ = lit^


def main():
    _ = ints[3, 4]()
    uints[UInt(3), UInt(4)]()
    _ = int32s[3, 4]()
    bytes[UInt8(5), UInt8(2)]()
    floats[Float32(3), Float32(2)]()
    vectors[SIMD[DType.float32, 4](1.5), SIMD[DType.float32, 4](2)]()
    print(I[Int(3).__add__(4)].x, S[Int32(3).__add__(4)].x)
    comptime n = SIMD[DType.float32, 4](1.5).__neg__()
    comptime m = SIMD[DType.int32, 4](1, 2, 3, 4).__mul__(
        SIMD[DType.int32, 4](2)
    )
    print(n, m)
