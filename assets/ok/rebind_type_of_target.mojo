# `rebind`'s `dest_type` is an ordinary type parameter, so its argument is
# any expression that denotes a type: `type_of(x)` names the operand's
# destination through another value, as upstream's `_pow` spells
# `rebind[type_of(base)](exp)`.
def h[dt: DType, w: Int](base: SIMD[dt, w], exp: SIMD[dt, w]) -> SIMD[dt, w]:
    return base * rebind[type_of(base)](exp)


def same(x: Int, y: Int) -> Int:
    return x + rebind[type_of(x)](y)


def main():
    print(same(1, 2))
    var v = SIMD[DType.float32, 4](1.5)
    print(h[DType.float32, 4](v, v))
    var s = String("a")
    var t = rebind[type_of(s)](s)
    print(t)
