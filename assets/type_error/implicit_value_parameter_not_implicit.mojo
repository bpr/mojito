# expect: type mismatch for value parameter 'w'
# A constructor that is not `@implicit` converts no parameter argument.
struct W(ImplicitlyCopyable):
    var v: Int

    def __init__(out self, value: Int, /):
        self.v = value * 2


def f[w: W]() -> Int:
    return w.v


def main():
    print(f[3]())
