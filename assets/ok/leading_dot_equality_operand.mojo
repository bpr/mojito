# A leading-dot right operand of `==` and `!=`, and a leading-dot case head
# over a subject that is not `EnumLike`, take the `__eq__` parameter's type as
# their context: a struct's `Self`, and `DType`'s (upstream's `DType.__eq__(
# self, rhs: Self)`), in an annotated binding, a `[dt: DType]` binder's
# `comptime if`, and a match over a type's member included.
struct K(Equatable, ImplicitlyCopyable):
    var v: Int
    comptime X = K(1)

    def __init__(out self, v: Int):
        self.v = v

    def __eq__(self, other: Self) -> Bool:
        return self.v == other.v


def kind[dt: DType]() -> String:
    comptime if dt == .float32:
        return "f32"
    elif dt != .int8:
        return "not i8"
    return "i8"


def is_i8(d: DType) -> Bool:
    return d == .int8


def main():
    var d = DType.int64
    print(d == .int64, d != .int64, d == .int8, d != .bool)
    var e: DType = .uint16
    print(e)
    print(kind[DType.float32](), kind[DType.int16](), kind[DType.int8]())
    print(is_i8(DType.int8), is_i8(d))
    comptime c = DType.uint8 == .uint8
    print(c)
    match d:
        case .int64:
            print("i64")
        case _:
            print("other")
    match DType.float64:
        case .int64:
            print("i64")
        case .float64:
            print("f64")
        case _:
            print("other")
    var k = K(1)
    print(k == .X, k != .X)
    match k:
        case .X:
            print("x")
        case _:
            print("other")
    match K.X:
        case .X:
            print("x")
        case _:
            print("other")
