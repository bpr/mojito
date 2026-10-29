# Upstream's `StringLiteral[value: __mlir_type.`!kgen.string`]` struct: an
# application constructs it, and its members read the KGEN string
# parameter, at run time and at compile time.
comptime v = "None".value


def show[s: __mlir_type.`!kgen.string`]():
    print(StringLiteral[s](), StringLiteral[s]().byte_length())


def main():
    print(StringLiteral["None".value]())
    print(StringLiteral[v]().byte_length())
    print(Bool(StringLiteral[v]()), Bool(StringLiteral["".value]()))
    print(StringLiteral["héllo".value]().byte_length())
    print(StringLiteral[v]().as_string_slice())
    print(len(StringLiteral[v]().as_bytes()))
    print(repr(StringLiteral["a\nb".value]()))
    comptime n = StringLiteral["abc".value]().byte_length()
    print(n)
    show["xyz".value]()
    var w = String()
    StringLiteral[v]().write_to(w)
    print(w)
    print(len(Span[Byte, ImmStaticOrigin](unsafe_ptr="abc".ptr(), length=3)))
