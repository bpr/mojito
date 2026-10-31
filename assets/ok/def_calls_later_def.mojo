# A module `def` calls one declared below it, as at the pin: a plain call,
# mutual recursion, a generic callee, an overload set completed below the
# call, a method of a struct declared above its callee (a generic one
# calling a generic `def` too), a body's
# compile-time call, and a module constant applying a later function.


def triple_of(n: Int) -> Int:
    return triple(n)


def is_even(n: Int) -> Bool:
    if n == 0:
        return True
    return is_odd(n - 1)


def is_odd(n: Int) -> Bool:
    if n == 0:
        return False
    return is_even(n - 1)


def bump_of[T: Intable](x: T) -> Int:
    return bump[4]() + bumped(x)


def picks() -> Int:
    return pick(1) + pick(2.0)


def pick(x: Int) -> Int:
    return x


struct Counter:
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def doubled(self) -> Int:
        return double(self.n)


struct Shelf[T: Copyable & Deinitable](Movable):
    var items: List[Self.T]

    def __init__(out self):
        self.items = List[Self.T]()

    def size(self) -> Int:
        return tally(self.items)


def describes[n: Int]() -> String:
    comptime assert n > 0, describe(n)
    comptime text = describe(n)
    return text


comptime NINE = triple(3)


def triple(n: Int) -> Int:
    return n * 3


def bump[k: Int]() -> Int:
    return k + 1


def bumped[T: Intable](x: T) -> Int:
    return Int(x) + 1


def pick(x: Float64) -> Int:
    return 10


def double(n: Int) -> Int:
    return n * 2


def describe(n: Int) -> String:
    return String("n=") + String(n)


def tally[T: Copyable & Deinitable](items: List[T]) -> Int:
    return len(items)


def main():
    print(triple_of(2), is_even(10), is_odd(7))
    print(bump_of(4), picks(), Counter(5).doubled())
    var shelf = Shelf[Int]()
    shelf.items.append(8)
    print(describes[3](), NINE, shelf.size())
