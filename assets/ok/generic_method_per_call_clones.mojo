# Generic methods specialized per call on every struct: a method-level type
# parameter (`kind[U]`) on an ordinary generic instance (`Box[Int]`), on a
# non-generic struct, and through a static receiver, plus a method-level
# type pack (`describe[*Ts](self, *args: *Ts)`) inferred from the overflow
# arguments. Each call retargets to a clone with the parameters baked, so a
# `comptime if U == Int` and a `comptime for` over the pack fold per call.
# requires: discovery
struct Box[T: Copyable & Deinitable](Copyable, Movable):
    var value: Self.T

    def __init__(out self, var value: Self.T):
        self.value = value^

    def kind[U: AnyType](self) -> Int:
        comptime if U == Int:
            return 1
        comptime if U == Self.T:
            return 2
        return 0

    def describe[*Ts: Writable](self, *args: *Ts) -> Int:
        var n = 0
        comptime for i in range(Ts.length):
            print(args[i])
            n += 1
        return n

    @staticmethod
    def accepts[U: AnyType]() -> Bool:
        comptime if U == Self.T:
            return True
        return False


struct Plain(Movable):
    var n: Int

    def __init__(out self):
        self.n = 0

    def kind[U: AnyType](self) -> Int:
        comptime if U == Bool:
            return 1
        return 0

    def fields[*Ts: Writable](mut self, *args: *Ts):
        comptime for i in range(Ts.length):
            self.n += 1


def main():
    var b = Box[Int](3)
    print(b.kind[Int](), b.kind[Bool]())
    var c = Box[Bool](True)
    print(c.kind[Int](), c.kind[Bool]())
    print(b.describe(1, "a", True))
    print(c.describe(2.5))
    print(Box[Int].accepts[Int](), Box[Int].accepts[Bool]())
    var p = Plain()
    print(p.kind[Int](), p.kind[Bool]())
    p.fields(1, "a", True)
    p.fields(False)
    print(p.n)
