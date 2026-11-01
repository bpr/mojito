# A program's own `Int`, `UInt`, and `Float64` shadow the prelude's `comptime`
# aliases of `std.simd` in annotations and constructions, as `Int8` does.


struct Int:
    var v: Bool

    def __init__(out self, v: Bool):
        self.v = v


struct UInt:
    var v: Bool

    def __init__(out self, v: Bool):
        self.v = v


struct Float64:
    var v: String

    def __init__(out self, v: String):
        self.v = v


def flag(x: Int) -> Bool:
    return x.v


def other(x: UInt) -> Bool:
    return x.v


def label(x: Float64) -> String:
    return x.v


def main():
    var i = Int(True)
    print(flag(i), other(UInt(False)), label(Float64("f64")))
    var n: Scalar[DType.int] = 3
    print(n + 1, String("abc").byte_length())
