# `SIMD.write_to` and `write_repr_to` are the declared Mojo members: a writer
# sees each piece upstream's `_write_int` writes, its `writer.write(prefix)`
# call and the zero branch's `writer.write` included, and the scalar repr
# takes the alias `_scalar_repr_alias` names.
struct Buf(Writer):
    var text: String

    def __init__(out self):
        self.text = String()

    def write_string(mut self, string: StringSlice):
        self.text += "|"
        self.text += string

    def write[*Ts: Writable](mut self, *args: *Ts):
        self.text += "<"
        comptime for i in range(args.__len__()):
            args[i].write_to(self)
        self.text += ">"


def main():
    var b = Buf()
    Int(12).write_to(b)
    print(b.text)
    b.text = String()
    Int(0).write_to(b)
    print(b.text)
    b.text = String()
    SIMD[DType.int8, 2](-3, 4).write_to(b)
    print(b.text)
    b.text = String()
    UInt8(7).write_repr_to(b)
    print(b.text)
    var t = String()
    Int8(4).write_repr_to(t)
    t += " "
    Float32(1.5).write_repr_to(t)
    t += " "
    Int8(4).write_to(t)
    t += " "
    SIMD[DType.int32, 4](1, 2, 3, 4).write_repr_to(t)
    t += " "
    SIMD[DType.uint8, 2](255, 0).write_to(t)
    t += " "
    UInt(3).write_repr_to(t)
    t += " "
    Float64(-2.25).write_to(t)
    t += " "
    Scalar[DType.bool](True).write_repr_to(t)
    print(t)
    t = String()
    (Int(-9223372036854775807) - 1).write_to(t)
    t += " "
    UInt64(18446744073709551615).write_to(t)
    t += " "
    Int64(-5).write_repr_to(t)
    print(t)
