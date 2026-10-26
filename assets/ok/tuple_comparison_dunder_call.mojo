# A tuple operator is the call of the bundled `Tuple`'s dunder under its
# `where conforms_to` clause: elements of a user `Comparable` struct order
# through its `__lt__`, a tuple satisfies a `Comparable` bound, and a
# compile-time comparison of tuple displays runs `Tuple.__lt__` too.
@fieldwise_init
struct Tag(Comparable, ImplicitlyCopyable):
    var v: Int

    def __eq__(self, other: Self) -> Bool:
        return self.v == other.v

    def __lt__(self, other: Self) -> Bool:
        return self.v < other.v


def less[T: Comparable](a: T, b: T) -> Bool:
    return a < b


comptime ORDERED = (1, 2) < (1, 3) < (1, 4)


def main():
    print((Tag(2), 1) < (Tag(2), 3), (Tag(3), 1) >= (Tag(2), 3))
    print((Tag(2), 1) == (Tag(2), 1), (Tag(2), 1) != (Tag(2), 3))
    print(less((Tag(1), 5), (Tag(2), 0)), less((2.0, 1), (1.0, 5)))
    comptime later = (2, 1) > (1, 9)
    comptime conforming = conforms_to(Tuple[Tag, Int], Comparable)
    print(ORDERED, later, conforming)
