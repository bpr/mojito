# expect: type mismatch for variable 'c'
# A struct construction over a binder is the application of its
# `__init__`, compared by structure: `Extent(n, n + 1)` is not
# `Extent(n + 1, n)`, as at the pin.
@fieldwise_init
struct Extent(ImplicitlyCopyable, Movable):
    var rows: Int
    var cols: Int


struct Tagged[e: Extent](ImplicitlyCopyable, Movable):
    var scale: Int

    def __init__(out self, scale: Int):
        self.scale = scale


def f[n: Int]() -> Int:
    var a = Tagged[Extent(n, n + 1)](1)
    var c: Tagged[Extent(n + 1, n)] = a
    return c.scale


def main():
    print(f[2]())
