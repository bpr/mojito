# `and`/`or` whose left operand converts to the right operand's type, as
# upstream's `emitAndOr`: the left operand is tested at its own type, through
# its own `Bool(x)`, and converted to the common type only in the arm that
# yields it. An `Int` left over an `@implicit` constructor, a scalar left
# over a vector (splatted after the test), and a `StringLiteral` left over a
# `String` or another literal each test the unconverted value.
struct W(Copyable, Movable, Writable):
    var raw: Int

    def __init__(out self, *, raw: Int):
        self.raw = raw

    @implicit
    def __init__(out self, v: Int):
        self.raw = v * 10

    def __bool__(self) -> Bool:
        return self.raw > 30

    def write_to(self, mut writer: Some[Writer]):
        writer.write("W", self.raw)


def main():
    var n = 2
    var z = 0
    print(n or W(raw=50), z or W(raw=50))
    print(n and W(raw=50), z and W(raw=50))
    print(2 or W(raw=50), 0 or W(raw=50))

    var v = SIMD[DType.int32, 4](1, 0, 3, 4)
    print(Int32(0) or v, Int32(2) or v)
    print(Int32(0) and v, Int32(2) and v)

    var s = String("hello")
    print("" or "a", "b" and "c")
    print("" or s, "x" or s)
    print("" and s, "y" and s)

    comptime folded = "" or "a"
    comptime splat = Int32(0) or SIMD[DType.int32, 4](1, 0, 3, 4)
    print(folded, splat)
