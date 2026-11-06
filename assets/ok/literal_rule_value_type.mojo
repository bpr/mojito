# A literal operator no declared dunder takes (a unary `-` or `~`, `/`, an
# `IntLiteral` beside a `FloatLiteral`) is typed by the attribute the
# missing dunder's signature computes, so over a symbolic `IntLiteral[_]`
# or `FloatLiteral[_]` argument the type keeps it: `type_of(-x)` is
# `IntLiteral[0 - x.value]`, and `{}` at it builds the negated value.


def neg(x: IntLiteral[_]) -> type_of(-x):
    return {}


def inv(x: IntLiteral[_]) -> type_of(~x):
    return {}


def half(x: IntLiteral[_]) -> type_of(x / 2):
    return {}


def mixed(x: IntLiteral[_], y: FloatLiteral[_]) -> type_of(x + y):
    return {}


def fneg(y: FloatLiteral[_]) -> type_of(-y):
    return {}


def main():
    print(neg(3), inv(4), half(3), mixed(1, 0.5), fneg(2.5))
