# expect: type mismatch for variable 'd'
# A struct-valued bracket argument that calls a function is that
# application, not the value it computes: `Tagged[Extent.square(4)]` is not
# `Tagged[Extent(4, 4)]`, as at the pin.
@fieldwise_init
struct Extent(ImplicitlyCopyable, Movable):
    var rows: Int
    var cols: Int

    @staticmethod
    def square(n: Int) -> Extent:
        return Extent(n, n)


struct Tagged[e: Extent](ImplicitlyCopyable, Movable):
    var scale: Int

    def __init__(out self, scale: Int):
        self.scale = scale


def main():
    var c = Tagged[Extent.square(4)](2)
    var d: Tagged[Extent(4, 4)] = c
    print(d.scale)
