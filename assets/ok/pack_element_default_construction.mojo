# A pack element's default construction, `Self.Ts[i]()` or a `def`'s own
# `Ts[i]()`, is checked once with the pack symbolic, where the pack's bound
# or a `where` clause makes each element `Defaultable`, and each instance
# constructs the element it binds: handed to `print`, or bound to a local.
# Two elements of one type (`Row[Int, Int, Bool]`) leave the copy nothing
# to tell their indices apart by, and still derive.
struct Row[*Ts: Movable & Writable & Deinitable](Movable):
    var width: Int

    def __init__(out self):
        self.width = 3

    def defaults(self) where conforms_to(Self.Ts.values, Defaultable):
        comptime for i in range(len(Self.Ts)):
            print(Self.Ts[i]())

    def locals(self) where conforms_to(Self.Ts.values, Defaultable):
        comptime for i in range(len(Self.Ts)):
            var value = Self.Ts[i]()
            print(value, self.width)


def build[*Ts: Movable & Defaultable & Writable & Deinitable]():
    comptime for i in range(len(Ts)):
        var value = Ts[i]()
        print(value)


def main():
    var row = Row[Int, Float64, Bool]()
    print(row.width)
    row.defaults()
    row.locals()
    build[Int, Optional[Int], Tuple[Int, Bool]]()
    var shared = Row[Int, Int, Bool]()
    shared.defaults()
    shared.locals()
    build[Bool, Int, Bool]()
