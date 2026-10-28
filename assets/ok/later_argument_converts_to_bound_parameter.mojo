# A type parameter is bound by its first argument, and a later argument
# converts to that type through an `@implicit` constructor, as at the pin:
# `select(c, 0, length)` is over `Int`. A conversion of `Self(...)`, which
# is spelled as the struct's own construction, converts that construction.
from std.utils._select import _select_register_value as select


def pick[T: TrivialRegisterPassable](c: Bool, a: T, b: T) -> T:
    if c:
        return a
    return b


struct W(ImplicitlyCopyable, TrivialRegisterPassable):
    var x: Int

    def __init__(out self, x: Int):
        self.x = x

    @implicit
    def __init__(out self, s: S):
        self.x = s.x * 100


def take(w: W) -> Int:
    return w.x


struct S(ImplicitlyCopyable, TrivialRegisterPassable):
    var x: Int

    def __init__(out self, x: Int):
        self.x = x

    def wrapped(self) -> Int:
        return take(Self(self.x + 1))

    def picked(self) -> W:
        return pick(False, W(1), Self(5))


def main():
    print(pick(False, 0, SIMDLength(3)), select(True, 0, SIMDLength(3)))
    print(pick(False, W(1), S(2)).x, S(4).wrapped(), S(8).picked().x)
    var n = SIMDLength(7)
    print(Int(pick(True, n, 2)))
