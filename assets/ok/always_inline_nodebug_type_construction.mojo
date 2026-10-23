# A call to a generic `@always_inline("nodebug")` function constructing its
# type binder (`T()`, `Ts[0]()`, `T(copy=x)`) is inlined at a concrete
# binding as at a symbolic one: the inlined body holds the bound type's own
# construction, a scalar's zero, a vector's zero splat, a struct's
# initializer, or the copy. Each prints what the call computes either way.


@always_inline("nodebug")
def make[T: Defaultable]() -> T:
    return T()


@always_inline("nodebug")
def dup[T: Copyable](x: T) -> T:
    return T(copy=x)


@always_inline("nodebug")
def first[*Ts: Defaultable & Movable & Writable]() -> Ts[0]:
    return Ts[0]()


struct Counter(Copyable, Defaultable):
    var n: Int

    def __init__(out self):
        self.n = 0

    def __init__(out self, *, copy: Self):
        self.n = copy.n + 100


struct Box[T: Copyable & Deinitable & Defaultable](Copyable, Defaultable):
    var x: Self.T

    def __init__(out self):
        self.x = Self.T()

    def __init__(out self, *, copy: Self):
        self.x = Self.T(copy=copy.x)


def outer[T: Defaultable & Copyable & Writable & Deinitable](x: T) -> T:
    var y = make[T]()
    print(y)
    return dup(x)


def main():
    var s = String("ab")
    print(make[Int](), dup(s), dup(7))
    print(make[UInt](), make[Float64](), make[Bool]())
    print(make[SIMD[DType.float32, 4]]())
    var c = Counter()
    print(make[Counter]().n, dup(c).n)
    var b = Box[Int]()
    print(make[Box[Int]]().x, dup(b).x)
    print(first[Int, Bool](), first[Bool, Int]())
    print(outer(5), outer(String("q")))
