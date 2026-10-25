# A scalar conversion reads its argument as a read parameter does: `Bool(x)`,
# `Int(x)`, and `Float64(x)` over a named `x: T` bounded by the conversion's
# trait read `x` where it lies and copy nothing, and a temporary argument
# (`Bool(make())`) dies right after the conversion that reads it.
struct S(Boolable, Copyable, Floatable, Intable, Movable):
    var n: Int
    var s: String

    def __init__(out self, n: Int):
        self.n = n
        self.s = String("v")

    def __init__(out self, *, copy: Self):
        print("copy", copy.n)
        self.n = copy.n + 10
        self.s = copy.s.copy()

    def __bool__(self) -> Bool:
        return self.n > 0

    def __int__(self) -> Int:
        return self.n

    def __float__(self) -> Float64:
        return Float64(self.n)

    def __deinit__(deinit self):
        print("del", self.n)


def peek[T: Boolable & Intable & Floatable & Copyable & Deinitable](x: T) -> Int:
    var b = Bool(x)
    var i = Int(x)
    var f = Float64(x)
    print(b, i, f)
    return i


def make() -> S:
    return S(4)


def conv[T: Boolable & Intable & Floatable & Copyable & Deinitable](var x: T):
    print(Bool(x), Int(x))


def main():
    var a = S(1)
    print(peek(a))
    print(Bool(make()), Int(make()), Float64(make()))
    conv(S(7))
    print("end")
