# A `rebind` keys specialization as a `comptime if` does, so an overloaded
# `def` whose bodies hold one is a compile-time-keyed family too: each
# declaration is stubbed, and each call is served from the checker's recorded
# instantiation naming the selected overload.


def as_int[T: Copyable](x: T) -> Int:
    return rebind[Int](x)


def as_int[T: Copyable](x: T, y: T) -> Int:
    return rebind[Int](x) + rebind[Int](y)


def main():
    print(as_int(3))
    print(as_int(3, 4))
    print(as_int[Int](5))
    print(as_int[Int](5, 6))
