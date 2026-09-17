# A generic struct's lifecycle methods key on the instance too: the
# constructor, the copy constructor, and the destructor each reach their own
# per-instantiation clone, so a `comptime if Self.T` folds there and a call to
# a compile-time-keyed `def` runs.
# requires: discovery

def show[T: Copyable](x: T):
    comptime if T == Int:
        print("int")
    else:
        print("other")

struct Box[T: Copyable & Deinitable](Deinitable):
    var x: Self.T

    def __init__(out self, x: Self.T):
        print("init")
        show(x)
        self.x = x.copy()

    def __init__(out self, *, copy: Self):
        print("copy")
        comptime if Self.T == Int:
            print("int")
        else:
            print("other")
        self.x = copy.x.copy()

    def __init__(out self, *, deinit move: Self):
        print("move")
        show(move.x)
        self.x = move.x^

    def __deinit__(deinit self):
        print("deinit")
        show(self.x)

def main():
    var a = Box[Int](3)
    var b = Box[Int](copy=a)
    print(a.x, b.x)
    var moved = Box[Int](4)
    var landed = moved^
    print(landed.x)
    var c = Box[Float64](1.5)
    var d = Box[Float64](copy=c)
    print(c.x, d.x)
