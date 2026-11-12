# `Self`, a `Self` parameter, and a `Self` type member in a bracket are
# expressions like any other operand, which the checker classifies as the
# types or values they denote, as upstream's subscript does.


def pick[T: AnyType]() -> Int:
    return 1


struct S[T: Copyable & Deinitable, n: Int](Copyable):
    comptime Q = Int
    var xs: List[Self.T]

    def __init__(out self):
        self.xs = List[Self.T]()

    def k(self) -> Int:
        return pick[Self]() + pick[Self.Q]() + pick[Self.T]() + Self.n

    def me(self) -> List[Self]:
        return List[Self]()


def main():
    var s = S[Int, 3]()
    s.xs.append(7)
    print(s.k(), len(s.me()))
