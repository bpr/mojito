# `String`, `repr`, and `Writer.write` of a `SIMD` value call its declared
# `write_to` and `write_repr_to`, as `print` does: a vector, a `Bool`
# vector, a `Scalar[DType.bool]`, a borrowed scalar, and a literal.
struct Buf(Writer):
    var text: String

    def __init__(out self):
        self.text = String()

    def write_string(mut self, string: StringSlice):
        self.text += string


def shown(ref v: Int32) -> String:
    return String(v) + " " + repr(v)


def main():
    var v = SIMD[DType.int32, 4](1, 2, 3, 4)
    print(String(v))
    print(repr(v))
    var s = String()
    s.write(v, " ", Float32(1.25), " ", UInt8(255))
    print(s)
    var b = Buf()
    b.write(SIMD[DType.float64, 2](0.5, -2.0), " ", Int16(-3))
    print(b.text)
    print(repr(SIMD[DType.bool, 2](True, False)))
    print(repr(Scalar[DType.bool](True)))
    print(shown(Int32(42)))
    print(String(7), repr(7), repr(2.5))
