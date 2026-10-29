# Unary `+x` is the call `x.__pos__()`, and over a literal it is the literal.
@fieldwise_init
struct P(Copyable, Writable):
    var x: Int

    def __pos__(self) -> Self:
        return P(self.x + 100)


def main():
    var a = 7
    print(+a)
    print(+5)
    print(+2.5)
    comptime c = +3
    print(c)
    var v = SIMD[DType.float32, 4](1.5, -2.0, 3.0, 4.0)
    print(+v)
    var u: UInt8 = 9
    print(+u)
    print(+P(1).x, (+P(2)).x)
    print(-(+a), +(-a))
