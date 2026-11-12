# A generator type's own parameter list scopes its result and its later
# binders: `List[T]` names `T`, and `x: T` the earlier type binder.

comptime MkList[T: Copyable] = List[T]()
comptime One[T: Copyable, x: T] = 1


def count[G: __generator_type[T: Copyable] List[T]]() -> Int:
    comptime n = len(G[Int])
    return n


def one[G: __generator_type[T: Copyable, x: T] Int]() -> Int:
    return G[Int, 3]


def main():
    print(count[MkList]())
    print(one[One]())
