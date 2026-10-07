# A generic `def` that returns a variadic struct over its own pack
# (`-> V[*Ts]`), or calls an overloaded generic constructor, is served by
# its template for every call: the pack spread into the result closes per
# call, and the overload is selected from the template.


struct V[*Ts: Movable](Movable):
    var n: Int

    def __init__[T: Movable & Deinitable](out self, var value: T):
        comptime if Self.Ts.length > 1:
            self.n = Self.Ts.length * 10
        else:
            self.n = Self.Ts.length

    def __init__[
        T: Movable & Deinitable
    ](out self, var value: T, extra: Int):
        self.n = extra


def first[*Ts: Movable]() -> V[*Ts]:
    return V[*Ts](3)


def second[*Ts: Movable]() -> V[*Ts]:
    return V[*Ts](3, 7)


struct Box[T: Movable & Writable & Deinitable](Movable):
    var v: Self.T

    def __init__[U: Writable](out self, var v: Self.T, tag: U):
        print("tagged", tag)
        self.v = v^

    def __init__(out self, var v: Self.T):
        self.v = v^


def wrap[T: Movable & Writable & Deinitable](var x: T) -> Box[T]:
    return Box[T](x^, "t")


def main():
    print(first[Int, String]().n, first[Int]().n, second[Bool]().n)
    print(first[Int, String, Bool]().n, second[Int, String]().n)
    var b = wrap(3)
    print(b.v)
    var c = wrap(String("s"))
    print(c.v)
