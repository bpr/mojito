# `Scalar` and the sized aliases are the prelude's `comptime` aliases, so a
# program's own declaration of one of those names shadows it, in an
# annotation and a construction alike.
struct Scalar(Copyable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v


struct Int8(Copyable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v


def doubled(x: Scalar) -> Int:
    return x.v * 2


def tripled(x: Int8) -> Int:
    return x.v * 3


def main():
    print(doubled(Scalar(4)))
    print(tripled(Int8(4)))
