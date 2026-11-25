# expect: value of type 'AnyStruct[Int]' is not compatible with value of type 'Int'
# An annotation the compile-time builder cannot read is typed as an ordinary
# expression with the declaration's value binders in scope: `n` reads as the
# `Int` it binds, and the conditional's type arm meets the call's value arm.
def g() -> Int:
    return 1


def f[n: Int](x: Int if n > 0 else g()):
    pass


def main():
    f[1](2)
