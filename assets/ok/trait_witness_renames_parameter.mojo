# A witness may name a regular parameter otherwise than its requirement: a
# call through the bound binds it by position, and a direct call by the
# witness's own name.
trait Shifted:
    def shift(self, x: Int) -> Int:
        ...


struct S(Shifted):
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def shift(self, by: Int) -> Int:
        return self.n + by


def through[U: Shifted](u: U) -> Int:
    return u.shift(4)


def main():
    var s = S(3)
    print(s.shift(2))
    print(s.shift(by=5))
    print(through(s))
