# An `IntLiteral` reaches a `FloatLiteral` through the declared `@implicit
# FloatLiteral.__init__(IntLiteral[_])`, so an integer literal binds a
# `FloatLiteral` slot, `FloatLiteral(3)` constructs one, and `IntLiteral`'s
# `FloatLiteral`-taking `+ - *` and `/` are its declared dunders, typed
# `type_of(FloatLiteral(self) OP rhs)`, a symbolic operand included.


def radd(x: IntLiteral[_], y: FloatLiteral[_]) -> type_of(y.__radd__(x)):
    return {}


def half(x: IntLiteral[_]) -> type_of(x / 2):
    return {}


comptime THIRD = 1 / 3


def main():
    var f: type_of(4.0) = 4
    print(f, FloatLiteral(3))
    print(2 + 2.5, 2 - 0.5, 3 * 0.5, 1 / 4, 7 / 2)
    print((2).__add__(2.5), (1).__truediv__(4.0), (3).__truediv__(2))
    print(radd(1, 0.5), half(3), THIRD)
