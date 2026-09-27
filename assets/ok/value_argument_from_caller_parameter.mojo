# A value argument built from the caller's own value parameters is a
# compile-time constant of each caller instance: arithmetic over a free
# `def`'s parameter, a struct's `Self.n`, and a chain of such calls.
def successor[n: Int, m: Int]() -> Int where n + 1 == m:
    return m


def below[n: Int, m: Int]() -> Int where n < m:
    return m


def scaled[n: Int]() -> Int:
    return n


def by_identity[n: Int]() -> Int:
    return successor[n, 1 + n]()


def folded[n: Int]() -> Int:
    return scaled[n // 2]() + scaled[-n]() + scaled[n % 3]()


def chained[n: Int, k: Int]() -> Int:
    return folded[n * k - 1]()


struct Box[n: Int](Copyable, Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def under[k: Int](self) -> Int where Self.n < k:
        return below[Self.n, k]()

    def sibling(self) -> Int:
        var other = Box[Self.n](self.v + 1)
        return other.v + Self.n


def main():
    print(by_identity[3]())
    print(by_identity[7]())
    print(folded[9]())
    print(chained[3, 4]())
    print(Box[3](1).under[7]())
    print(Box[4](1).sibling())
