# A generic struct's constructors clone as one overload set: every `__init__`
# signature reaches its own per-instantiation clone, so a `comptime if Self.T`
# folds in each of them and a call to a compile-time-keyed `def` runs, whether
# the construction is written out or reached through an `@implicit` conversion.
# requires: discovery

def show[T: Copyable](x: T):
    comptime if T == Int:
        print("int")
    else:
        print("other")

struct Box[T: Copyable & Deinitable](Deinitable):
    var x: Self.T

    @implicit
    def __init__(out self, x: Self.T):
        print("one")
        show(x)
        self.x = x.copy()

    def __init__(out self, x: Self.T, twice: Bool):
        print("two")
        comptime if Self.T == Int:
            print("int")
        else:
            print("other")
        self.x = x.copy()

    def __deinit__(deinit self):
        print("deinit")

def take(b: Box[Int]):
    print(b.x)

def main():
    var a = Box[Int](3)
    var b = Box[Int](4, True)
    print(a.x, b.x)
    var c = Box[Float64](1.5)
    var d = Box[Float64](2.5, False)
    print(c.x, d.x)
    take(5)
