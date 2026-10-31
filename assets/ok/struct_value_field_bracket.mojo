# A field of a struct-typed value parameter, `Self.e.rows`, binds a value
# parameter of another struct's application, as at the pin.
@fieldwise_init
struct Extent(ImplicitlyCopyable, Movable):
    var rows: Int
    var cols: Int


struct Width[n: Int]:
    def __init__(out self):
        pass


struct Tagged[e: Extent](ImplicitlyCopyable, Movable):
    def __init__(out self):
        pass

    def rows(self) -> Int:
        return Width[Self.e.rows]().n


def main():
    print(Tagged[Extent(2, 3)]().rows())
