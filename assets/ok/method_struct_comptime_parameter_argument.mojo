# A method's bracket argument naming a struct-valued `comptime` binding binds
# the binding's value, as a free function's does: a local or module binding,
# on an instance or a static method, and a generic struct's method
# forwarding a `StaticTuple` it built from its own pack, as
# `SIMD._shuffle_variadic` forwards its mask to `_shuffle_list`. So does a
# local binding of an application (`v.cast[u]()` over `comptime u =
# pick[d]()`).
from std.utils import StaticTuple


@fieldwise_init
struct P(ImplicitlyCopyable, RegisterPassable):
    var a: Int


comptime Q = P(4)


struct V:
    def __init__(out self):
        pass

    def first[p: P](self) -> Int:
        return p.a

    @staticmethod
    def static_first[p: P]() -> Int:
        return p.a

    def lane[n: SIMDLength, mask: StaticTuple[SIMDLength, n]](self) -> Int:
        return Int(mask[0])

    def via[*mask: SIMDLength](self) -> Int:
        comptime tup = StaticTuple[SIMDLength, 2].__init__[*mask]()
        return self.lane[2, tup]()


struct G[T: ImplicitlyCopyable & RegisterPassable]:
    def __init__(out self):
        pass

    def second[n: SIMDLength, mask: StaticTuple[SIMDLength, n]](self) -> Int:
        return Int(mask[1])

    def via[*mask: SIMDLength](self) -> Int:
        comptime tup = StaticTuple[SIMDLength, 2].__init__[*mask]()
        return self.second[2, tup]()


def pick[d: DType]() -> DType:
    return d


def recast[d: DType, w: SIMDLength](v: SIMD[d, w]):
    comptime u = pick[d]()
    print(v.cast[u]())


def main():
    comptime q = P(3)
    print(V().first[q]())
    print(V().first[Q]())
    print(V.static_first[Q]())
    print(V.static_first[q]())
    print(V().via[3, 0]())
    print(G[Int]().via[5, 6]())
    recast(SIMD[DType.int32, 2](1, 2))
