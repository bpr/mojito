# `DType` is upstream's library struct over `!kgen.dtype`, bundled in
# `std.builtin.dtype`: its members are `#kgen.dtype.constant` attributes,
# and `__eq__`, the `is_*` queries, `write_to`, `write_repr_to`, and
# `__hash__` are its declared bodies, called directly, run at run time, and
# folded in a compile-time position.


struct Marking(Writer):
    var text: String

    def __init__(out self):
        self.text = String()

    def write_string(mut self, string: StringSlice):
        self.text += "|"
        self.text += String(string)


struct Box[d: DType]:
    @staticmethod
    def floating() -> Bool:
        return Self.d.is_floating_point()


def kind[d: DType]() -> String:
    comptime if d.is_integral():
        return "integral"
    elif d == DType.float32:
        return "f32"
    else:
        return "other"


def half[d: DType](x: Scalar[d]) -> Scalar[d] where d.is_floating_point():
    return x / 2


def name(d: DType) -> String:
    match d:
        case .float32:
            return "float32 case"
        case .int64:
            return "int64 case"
    return "other case"


def main():
    var d = DType.int64
    print(d.__eq__(DType.int64), d.__ne__(DType.int64), d == .int64)
    print(DType(mlir_value=__mlir_attr.`#kgen.dtype.constant<si64> : !kgen.dtype`))
    print(DType._from_ui8(UInt8(141)._mlir_value))
    print(d.is_integral(), d.is_signed(), d.is_unsigned(), d.is_floating_point())
    print(DType.uint8.is_unsigned(), DType.float16.is_half_float(), DType.bool.is_numeric())

    var m = Marking()
    SIMD[DType.uint16, 2](1, 2).write_repr_to(m)
    print(m.text)
    var n = Marking()
    DType.float64.write_to(n)
    DType.float64.write_repr_to(n)
    print(n.text)

    print(hash(DType.int64) == hash(DType.int64), hash(DType.int64) == hash(DType.int8))
    print(name(DType.float32), name(d), name(DType.bool))
    print(kind[DType.int8](), kind[DType.float32](), kind[DType.float64]())
    print(half(Float32(3)))
    print(Box[DType.float32].floating(), Box[DType.uint32].floating())
    comptime folded = DType.float32.is_floating_point()
    print(folded)
