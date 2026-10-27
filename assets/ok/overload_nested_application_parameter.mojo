# Overloads told apart by a nested application in a parameter type.


@fieldwise_init
struct Pair[T: Copyable & Deinitable](Copyable):
    var a: Self.T


@fieldwise_init
struct Box[T: Copyable & Deinitable](Copyable):
    var b: Self.T


def f(x: List[Pair[Int]]) -> Int:
    return x[0].a


def f(x: List[Box[Int]]) -> Int:
    return x[0].b * 10


def g(p: Pointer[List[Int], _]) -> Int:
    return len(p[])


def g(n: Int) -> Int:
    return n


def main():
    print(f([Pair[Int](1)]))
    print(f([Box[Int](2)]))
    var xs: List[Int] = [1, 2, 3]
    print(g(Pointer(to=xs)))
    print(g(7))
