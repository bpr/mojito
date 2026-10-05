# A struct whose field is a vector over its own value parameter is a
# generator the template serves, so an application at a symbolic argument
# names the template's fields: `Lanes[n]` in a `def` keyed on `n`, and
# `Lanes[i]` at a `comptime for` index.
struct Lanes[w: Int](Copyable, Movable):
    var v: SIMD[DType.int32, Self.w]

    def __init__(out self, x: Int32):
        self.v = SIMD[DType.int32, Self.w](x)


def owned[n: Int]():
    print(Lanes[n](3).v)


def main():
    owned[2]()
    comptime for i in range(1, 3):
        print(Lanes[i](5).v)
