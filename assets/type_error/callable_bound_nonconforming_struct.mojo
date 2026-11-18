# A struct that declares no `def(...)` conformance does not conform to a
# `def(...)` bound, whatever methods it defines, as at the pin: the argument
# converts to no type that does.
# expect: value passed to 'f' cannot be converted from 'S' to 'F', argument type 'S' does not conform to trait 'def(Int) -> Int'


struct S:
    var v: Int

    def __init__(out self, v: Int):
        self.v = v


def apply[F: def(Int) -> Int](f: F, x: Int) -> Int:
    return f(x)


def main():
    print(apply(S(1), 2))
