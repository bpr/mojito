# A variadic struct's method names an element of the struct's pack through a
# local alias inside a `comptime for` (`comptime T = Self.Ts[i]`) and decides
# a `comptime if` over the alias, beside one over the element spelled out.
# The struct's template serves the method: each instance unrolls the loop and
# selects the arms for its own elements.
struct Bag[*Ts: Movable & Writable](Movable):
    var n: Int

    def __init__(out self):
        self.n = 0

    def strings(self) -> Int:
        var total = 0
        comptime for i in range(Self.Ts.length):
            comptime if Self.Ts[i] == String:
                total += 1
        return total

    def ints(self) -> Int:
        var total = 0
        comptime for i in range(Self.Ts.length):
            comptime T = Self.Ts[i]
            comptime if T == Int:
                total += 10
        return total


def main():
    var x = Bag[Int, String, Int]()
    print(x.strings(), x.ints())
    var y = Bag[String]()
    print(y.strings(), y.ints())
