# A `def`'s or method's own type pack queried in a runtime position:
# `Us.length`, `len(Us)`, `Us.contains[X]()`, and `Us.all_conforms_to[T]()`
# read as values. A template-served body carries each query as a parameter
# constant the elaborator folds per instance; a cloned body (a value
# parameter beside the pack) folds it at the clone.
struct Plain:
    var n: Int

    def __init__(out self):
        self.n = 1

    def tally[*Us: Movable](self, var *extra: *Us) -> Int:
        return self.n + Us.length


struct Box[T: Writable]:
    var base: Int

    def __init__(out self, base: Int):
        self.base = base

    def tally[*Us: Writable](self, *extra: *Us) -> Int:
        return self.base + len(Us)


def count[*Us: Movable](var *extra: *Us) -> Int:
    return 2 + Us.length


def count_explicit[*Us: Movable]() -> Int:
    return Us.length * 10


def measured[*Us: Writable](*extra: *Us) -> Int:
    return len(Us) + extra.__len__()


def has_int[*Us: Writable](*extra: *Us) -> Bool:
    return Us.contains[Int]()


def has_float[*Us: Writable](*extra: *Us) -> Bool:
    return Us.contains[Float64]()


def has_own[T: Copyable, *Us: Writable](x: T, *extra: *Us) -> Bool:
    return Us.contains[T]()


def all_writable[*Us: Movable](var *extra: *Us) -> Bool:
    return Us.all_conforms_to[Writable]()


def scaled[n: Int, *Us: Writable](*extra: *Us) -> Int:
    return n * Us.length


def inner[*Us: Writable](*extra: *Us) -> Int:
    return Us.length


def outer[*Us: Writable](*extra: *Us) -> Int:
    return inner(*extra) * 10 + Us.length


def main():
    print(Plain().tally(7, "x", False))
    print(Plain().tally())
    print(Box[Int](3).tally("a", 2.5))
    print(count(7, "x", False))
    print(count_explicit[Int, String, Bool, Int]())
    print(measured(1, "two"))
    print(has_int(7, "x", False))
    print(has_int("x", False))
    print(has_float(7, "x", False))
    print(has_own(1, 7, "x"))
    print(has_own(1.5, 7, "x"))
    print(all_writable(7, "x", False))
    print(scaled[3](7, "x"))
    print(outer(1, "y", True))
