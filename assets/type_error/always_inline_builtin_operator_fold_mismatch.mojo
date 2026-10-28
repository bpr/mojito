# expect: type mismatch for return: expected Tagged[L(n + 2)], found Tagged[(L(n) + L(1))]
# A struct operator's builtin dunder folds to the struct value it builds,
# compared by its fields: `L(n) + L(1)` is `L(n + 1)`, not `L(n + 2)`.
struct L(TrivialRegisterPassable):
    var v: Int

    @always_inline("builtin")
    def __init__(out self, v: Int):
        self.v = v

    @always_inline("builtin")
    def __add__(self, rhs: Self) -> Self:
        return Self(self.v + rhs.v)


struct Tagged[l: L](ImplicitlyCopyable):
    def __init__(out self):
        pass


def g[n: Int](x: Tagged[L(n) + L(1)]) -> Tagged[L(n + 2)]:
    return x


def main():
    print(1)
