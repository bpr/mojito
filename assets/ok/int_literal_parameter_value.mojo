# A bare `IntLiteral` or `FloatLiteral` parameter is upstream's per-call
# `IntLiteral[x.value]`: its type carries the argument's value, which a
# result type reads, and an integer literal binds a `FloatLiteral` one as
# the float it converts to.
def h(x: IntLiteral) -> IntLiteral[x.value]:
    return x


def hf(x: FloatLiteral) -> FloatLiteral[x.value]:
    return x


def g(x: FloatLiteral) -> Float64:
    return Float64(x)


def k(x: IntLiteral) -> Int:
    return Int(x)


def main():
    print(h(6), hf(2.5))
    print(g(3), g(0.25))
    print(k(7), k(-12))
    var a: type_of(9) = h(9)
    print(a)
