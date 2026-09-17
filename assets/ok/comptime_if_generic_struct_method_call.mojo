# A generic struct's method may call a `def` whose `comptime if` keys on its
# own type parameter, inferred (`show(self.x)`) or explicit
# (`show[Self.T](self.x)`), directly or through another generic `def`. Every
# closed instance reaches the call through its own method clone.
# requires: discovery

def show[T: Copyable](x: T):
    comptime if T == Int:
        print("int")
    else:
        print("other")

def relay[T: Copyable & Deinitable](x: T):
    show(x)

struct Box[T: Copyable & Deinitable](Deinitable):
    var x: Self.T

    def __init__(out self, x: Self.T):
        self.x = x.copy()

    def f(self):
        show(self.x)

    def g(self):
        show[Self.T](self.x)

    def through(self):
        relay(self.x)

@fieldwise_init
struct Plain(Copyable, ImplicitlyCopyable, Movable, Deinitable):
    var n: Int

    def kind[U: Copyable](self, u: U):
        show(u)

def main():
    Box[Int](3).f()
    Box[Int](3).g()
    Box[Int](3).through()
    Box[Float64](1.5).f()
    Box[Float64](1.5).g()
    Box[Float64](1.5).through()
    Plain(1).kind(3)
    Plain(1).kind(1.5)
