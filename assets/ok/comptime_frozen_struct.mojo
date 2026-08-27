# A fieldwise-constructible struct value freezes at compile time: the
# factory call runs through VM-backed CTFE, field reads fold to constants,
# and the frozen instance materializes back as an ordinary construction.
@fieldwise_init
struct Dims(Copyable, Movable):
    var d0: Int
    var d1: Int

@fieldwise_init
struct Grid(Copyable, Movable):
    var shape: Dims
    var scale: Int

    @staticmethod
    def unit(n: Int) -> Grid:
        return Grid(Dims(n, n + 1), 2)

comptime G = Grid.unit(4)
comptime S0 = G.shape.d0
comptime S1 = G.shape.d1

def main():
    var g = G
    print(g.scale)
    print(S0)
    print(S1)
