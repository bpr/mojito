# A callee signature that reads a bound's value member (`Box[T.K]`,
# `SIMD[DType.int32, T.K]`) closes at a call with a concrete conformer: in a
# function's parameter, a method's parameter, and a function's result. A
# method of a struct bounded the same way reads it through its own parameter
# (`Box[Self.T.K]`) at a concrete receiver, and so does an initializer —
# written, fieldwise, or `@implicit` — at a concrete instance, and a static
# method at its spelled owner (`Holder[S].make`).
trait Tr:
    comptime K: Int


struct S(Tr):
    comptime K: Int = 3


struct Four(Tr):
    comptime K: Int = 4


struct Box[n: Int](Movable):
    def __init__(out self):
        pass


def f[T: Tr](x: Box[T.K]) -> Int:
    return T.K


def lanes[T: Tr](x: SIMD[DType.int32, T.K]) -> Int:
    return T.K


def g[T: Tr]() -> Box[T.K]:
    return Box[T.K]()


struct W:
    def __init__(out self):
        pass

    def f[T: Tr](self, x: Box[T.K]) -> Int:
        return T.K


struct Holder[T: Tr](Movable):
    def __init__(out self):
        pass

    def get(self, var b: Box[Self.T.K]) -> Int:
        _ = b^
        return Self.T.K

    def count(self, *bs: Box[Self.T.K]) -> Int:
        return len(bs)

    @staticmethod
    def make(var b: Box[Self.T.K]) -> Int:
        _ = b^
        return Self.T.K + 1


struct Built[T: Tr](Movable):
    var b: Box[Self.T.K]

    def __init__(out self, var b: Box[Self.T.K], tag: Int = 0):
        self.b = b^


@fieldwise_init
struct Fields[T: Tr](Movable):
    var b: Box[Self.T.K]


struct Converted[T: Tr](Movable):
    var n: Int

    @implicit
    def __init__(out self, var b: Box[Self.T.K]):
        _ = b^
        self.n = Self.T.K


def main():
    print(f[S](Box[3]()))
    print(lanes[Four](SIMD[DType.int32, 4](1)))
    print(W().f[S](Box[3]()))
    var b: Box[3] = g[S]()
    _ = b^
    print(Holder[S]().get(Box[3]()))
    print(Holder[S]().count(Box[3](), Box[3]()))
    print(Holder[S].make(Box[3]()))
    print(Holder[Four].make(Box[4]()))
    _ = Built[S](Box[3]())
    _ = Built[S](b=Box[3](), tag=1)
    _ = Fields[S](Box[3]())
    var c: Converted[S] = Box[3]()
    print(c.n)
    print("ok")
