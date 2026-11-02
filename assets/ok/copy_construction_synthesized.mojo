# `S(copy=s)` and a generic `T(copy=x)` over a struct whose copy is
# synthesized copy a trivial struct by its value and a struct with an owning
# field through the synthesized copy initializer; `T()` runs the `__init__`
# the struct's `Defaultable` conformance names. Each copy is independent of
# its source. A `SIMD` copy (`Int32(copy=x)`, `SIMD[dt, n](copy=v)`) is the
# synthesized trivial copy, and the synthesized copy converts its argument to
# `Self` (`Float64(copy=2)`, `Wrapped(copy=t)` through an `@implicit`
# initializer).


@fieldwise_init
struct Point(Copyable, Movable):
    var x: Int
    var y: Int


@fieldwise_init
struct Named(Copyable, Movable):
    var id: Int
    var name: String


struct Origin(Copyable, Defaultable, Movable):
    var at: Point

    def __init__(out self):
        self.at = Point(1, 2)

    def __init__(out self, x: Int):
        self.at = Point(x, x)


@fieldwise_init
struct Trivial(TrivialRegisterPassable):
    var v: Int


struct Wrapped(Copyable, Movable):
    var v: Int

    @implicit
    def __init__(out self, t: Trivial):
        self.v = t.v


def dup[T: Copyable](x: T) -> T:
    return T(copy=x)


def make[T: Defaultable]() -> T:
    return T()


def main():
    var p = Point(3, 4)
    var q = Point(copy=p)
    q.x = 30
    var r = dup(p)
    r.y = 40
    print(p.x, p.y, q.x, q.y, r.x, r.y)
    var n = Named(1, "one")
    var m = Named(copy=n)
    m.name += "!"
    var k = dup(n)
    k.id = 2
    print(n.id, n.name, m.id, m.name, k.id, k.name)
    var o = make[Origin]()
    print(o.at.x, o.at.y, Origin(5).at.y)
    var x: Int32 = 5
    var v = SIMD[DType.float32, 4](1.5)
    var t = Trivial(3)
    print(Int32(copy=x), SIMD[DType.float32, 4](copy=v), Float64(copy=2))
    print(Trivial(copy=t).v, Wrapped(copy=t).v)
