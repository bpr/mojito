# An initializer list `{}` takes the type its context expects, as in current
# Mojo: a nominal struct, a builtin scalar, a `Tuple`, a `Pointer` write's
# pointee, and a type parameter each construct through their own default
# initializer; `{a, b}` passes its entries to that constructor.
struct Box(Movable, Defaultable):
    var a: Int

    def __init__(out self):
        self.a = 41

    def __init__(out self, a: Int, b: Int):
        self.a = a + b


def take(var b: Box) -> Int:
    return b.a


def make[T: Defaultable & Movable & Writable & Deinitable]():
    var p: T = {}
    print(p)


def main():
    var s: String = {}
    var f: Float64 = {}
    var n: Int = {}
    print(take({}), take({1, 2}), s.byte_length(), f, n)
    var e: Tuple[Int, Bool] = {}
    print(e[0], e[1])
    var b = Box(1, 1)
    Pointer(to=b).unsafe_write({})
    print(b.a)
    make[Int]()
    make[String]()
    var o: Optional[Int] = {}
    print(o is None)
