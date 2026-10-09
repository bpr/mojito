# A `comptime for` whose bound reads a type's compile-time value member
# through a binder (`T.K`, `Self.T.K`) or a field of a struct-valued
# parameter (`Self.e.w`), and one over a module constant list of struct
# values, are kept in the template of a generic struct's method or a generic
# `def` and unrolled per instance below MIR, as the pin's parameter `for` is.
# No instance clones its method.
trait HasK:
    comptime K: Int


struct A(HasK):
    comptime K = 3


struct B(HasK):
    comptime K = 2


@fieldwise_init
struct Ext(Copyable, ImplicitlyCopyable):
    var w: Int


@fieldwise_init
struct P(Copyable, ImplicitlyCopyable, Movable):
    var a: Int
    var b: Int


comptime PS = [P(1, 2), P(3, 4)]


def summed[T: HasK]() -> Int:
    var t = 0
    comptime for i in range(T.K):
        t += i
    return t


struct ByBound[T: HasK]:
    var x: Int

    def __init__(out self):
        self.x = 10

    def get(self) -> Int:
        var t = self.x
        comptime for i in range(Self.T.K):
            t += i
        return t

    def through_def(self) -> Int:
        return self.x + summed[Self.T]()


struct Plain:
    def __init__(out self):
        pass

    def listed[T: HasK](self):
        comptime for i in range(T.K):
            print(i)


struct ByField[e: Ext]:
    var x: Int

    def __init__(out self):
        self.x = 0

    def get(self) -> Int:
        var t = self.x
        comptime for i in range(Self.e.w):
            t += i
        return t


def over_structs[n: Int]():
    comptime for p in PS:
        print(p.a + n, p.b)


def main():
    print(ByBound[A]().get(), ByBound[B]().get())
    print(ByBound[A]().through_def(), ByBound[B]().through_def())
    Plain().listed[A]()
    print(ByField[Ext(3)]().get(), ByField[Ext(4)]().get())
    over_structs[10]()
