# `Int`, `UInt`, and `Float64` are width-one `SIMD` values: every
# compiler-known `SIMD` method and lane subscript serves them, as it does
# `Float32`.


@no_inline
def not_a_number(x: Float64) -> Float64:
    return x / x


def total[dt: DType](v: Scalar[dt]) -> Scalar[dt]:
    return v.reduce_add()


struct Holder:
    var i: Int
    var f: Float64

    def __init__(out self, i: Int, f: Float64):
        self.i = i
        self.f = f


def bump(mut x: Int):
    x[0] = x[0] + 10


def main():
    print(total(Float64(2.5)), total(Int(3)), total(UInt(4)))
    print(Float64(2.5).reduce_add(), Int(-4).reduce_min(), UInt(7).reduce_max())
    print(Float64(2.0).reduce_mul())

    var x = Float64(1.5)
    var y = Float64(2.5)
    print(x.ne(y), x.lt(y), x.ge(y))
    print(Int(5).eq(5), UInt(5).lt(UInt(6)), Int(3).ge(4), Float64(3.0).gt(2.0))
    # A NaN is unequal to nothing under the ordered `ne`.
    var n = not_a_number(0.0)
    print(n.ne(n), n.eq(n))

    print(Int(5).cast[DType.int32](), UInt(300).cast[DType.uint8]())
    print(Int(-1).cast[DType.uint16](), Float64(1.5).cast[DType.float32]())
    print(Int(5).join(Int(6)), Int(5).slice[1](), Int(5).shuffle[0]())

    print(Int(5)[0], Float64(1.5)[0], UInt(9)[0])
    var h = Holder(3, 4.5)
    print(h.i[0], h.f[0])
    h.f[0] = 9.25
    h.i[0] = 7
    print(h.i, h.f)
    var k = 1
    bump(k)
    print(k)
    var u = UInt(3)
    u[0] = 8
    print(u, u[0])
