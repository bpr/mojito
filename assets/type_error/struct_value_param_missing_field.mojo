# expect: has no field 'depth'
# A field the struct-typed parameter's type does not declare is rejected
# where the generator reads it.
@fieldwise_init
struct Extent(ImplicitlyCopyable, Movable):
    var rows: Int
    var cols: Int


struct Tagged[e: Extent](ImplicitlyCopyable, Movable):
    var scale: Int

    def __init__(out self, scale: Int):
        self.scale = scale

    def depth(self) -> Int:
        return Self.e.depth


def main():
    var a = Tagged[Extent(2, 3)](10)
    print(a.depth())
