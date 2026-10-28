# A partial explicit application solves the parameters its brackets leave
# open from the arguments, value binders included.
struct Box[n: Int](Copyable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v


def g2[m: Int, n: Int](a: Box[n]) -> Int:
    return m + n


def h[dt: DType, n: Int](a: Box[n], b: Scalar[dt]) -> Int:
    return n


def k[dt: DType, s: String](x: Scalar[dt]) -> String:
    return s


def rotate[
    dtype: DType, size: SIMDLength, //, shift: Int
](v: SIMD[dtype, size]) -> Int:
    return shift


def f[d: DType, //, n: Int](x: Scalar[d]) -> Int:
    return n


def main():
    print(g2[1](Box[4](0)))
    print(h[DType.int8](Box[4](0), Int8(1)))
    print(k[s="yo2"](Int16(2)))
    print(rotate[1](SIMD[DType.int8, 4](0)))
    print(f[3](Int8(1)))
