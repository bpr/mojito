# A type-bodied parameterized `comptime` alias read unapplied is a type
# generator, upstream's `#kgen.gen` typed `__generator_type[T: AnyType]
# AnyType`; a `__generator_type` parameter takes it, and its body applies it
# in a type position (`G[Int]`), which `native::mono` folds per instance.
from std.sys import size_of

comptime Id[T: AnyType] = T
comptime MkList[T: Copyable] = List[T]
comptime Second[A: AnyType, B: AnyType] = B


struct Two:
    var a: Int
    var b: Int


comptime Const2[T: AnyType] = Two


def size_at_int[G: __generator_type[T: AnyType] AnyType]() -> Int:
    return size_of[G[Int]]()


def forwarded[G: __generator_type[T: AnyType] AnyType]() -> Int:
    return size_at_int[G]() * 10


def build[G: __generator_type[T: Copyable] AnyType]():
    comptime L = G[Int]
    print("built")


def length[G: __generator_type[T: Copyable] AnyType](x: G[Int]):
    print("took a list")


def main():
    print(size_at_int[Id](), size_at_int[Const2]())
    print(forwarded[Id]())
    print(size_at_int[Second[_, Two]]())
    comptime P = Id
    print(size_at_int[P]())
    build[MkList]()
    var l: List[Int] = [1, 2, 3]
    length[MkList](l)
    var x: Id[Int] = 5
    print(x)
