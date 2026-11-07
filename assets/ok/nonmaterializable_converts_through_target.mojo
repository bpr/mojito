# A value whose struct declares `@__nonmaterializable(T)` converts to a
# parameter type through that type's `@implicit` constructor from `T`:
# it materializes to `T` first, then converts. A string literal reaches an
# `Optional[String]` argument and binding this way, a read `String`
# constructor reads the materialized `String` from its storage, and a user
# struct reaches an `Optional[Big]` argument.


struct Big(Copyable, Movable):
    var v: Int

    @implicit
    def __init__(out self, lit: Lit):
        self.v = lit.k * 10


@fieldwise_init
@__nonmaterializable(Big)
struct Lit(TrivialRegisterPassable):
    var k: Int


struct Wrap(Movable):
    var s: String

    @implicit
    def __init__(out self, s: String):
        self.s = s


def g(x: Optional[String]):
    print(x.or_else("none"))


def big(x: Optional[Big]):
    print(x.value().v)


def wrap(w: Wrap):
    print(w.s)


def main():
    g("a")
    g(None)
    big(Lit(4))
    wrap("c")
    var o: Optional[String] = "d"
    print(o.value())
