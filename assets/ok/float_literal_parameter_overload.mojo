# A bare `FloatLiteral` parameter (upstream's `FloatLiteral[_]`) takes a
# literal argument, and beside a `Float64` overload a literal selects it
# while a `Float64` value selects the `Float64` one, through an implicit
# conversion or a call.
struct L(Copyable, Movable):
    var v: Float64

    @implicit
    def __init__(out self, value: FloatLiteral, /):
        self.v = 100.0 + Float64(value)

    @implicit
    def __init__(out self, value: Float64, /):
        self.v = value

    def take(self, x: FloatLiteral) -> Float64:
        return self.v + Float64(x) * 1000.0

    def take(self, x: Float64) -> Float64:
        return self.v + x


def twice(x: FloatLiteral) -> Float64:
    return Float64(x) * 2.0

def f(x: FloatLiteral) -> Float64:
    var y: Float64 = x
    return y + twice(x) + 10.0

def f(x: Float64) -> Float64:
    return x

def g(x: FloatLiteral) -> Float64:
    return Float64(x)

def g[dtype: DType](x: SIMD[dtype, 1]) -> Float64:
    return -1.0

def main():
    var n = 4.5
    var a: L = 3.5
    var b: L = n
    print(a.v, b.v)
    print(L(3.5).v, L(n).v)
    print(f(0.5), f(n), f(2.0 + 0.25), f(-1.5))
    print(a.take(2.5), a.take(n))
    print(g(0.75), g(n), g(Float32(0.75)))
