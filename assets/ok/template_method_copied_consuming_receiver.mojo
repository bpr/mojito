# A per-instantiation method clone inherits its checked template's facts when
# the body calls a consuming method on a named place it does not own, which
# the call copies first (`docs/notes/instantiation-from-template.md`, class
# MethodBody, feature `copied receivers`): `or_else` on a field of a read
# parameter, on a field of `self`, on a local, and on a parameter whose type
# is built over the struct's parameter, and a `var self` requirement through
# a bound on a parameter and on a field of `self`. Whether a call copies its
# receiver is decided by the receiver's syntax and the callee's convention;
# an instance owes the copy at its own type. A direct call of a module
# function taking scalars selects the same declaration in every instance.
def check_range(start: Int, end: Int, length: Int):
    if start < 0 or end > length or start > end:
        print("bad range")


@fieldwise_init
struct Bounds(ImplicitlyCopyable):
    var start: Optional[Int]
    var end: Optional[Int]


struct Shelf[T: ImplicitlyCopyable & Deinitable](Movable):
    var size: Int
    var limit: Optional[Int]

    def __init__(out self, size: Int, limit: Optional[Int]):
        self.size = size
        self.limit = limit

    def count(self, bounds: Bounds) -> Int:
        var start = bounds.start.or_else(0)
        var end = bounds.end.or_else(self.size)
        check_range(start, end, self.size)
        return end - start

    def capped(self, n: Int) -> Int:
        var cap = self.limit.or_else(n)
        var floor = Optional[Int](1)
        return cap + floor.or_else(0)

    def pick(self, choice: Optional[Self.T], var fallback: Self.T) -> Self.T:
        return choice.or_else(fallback^)


trait Spendable:
    def spend(var self) -> Int:
        ...


@fieldwise_init
struct Coin(ImplicitlyCopyable, Spendable):
    var value: Int

    def spend(var self) -> Int:
        return self.value


@fieldwise_init
struct Token(ImplicitlyCopyable, Spendable):
    var value: Int

    def spend(var self) -> Int:
        return self.value * 10


struct Purse[T: Spendable & ImplicitlyCopyable & Deinitable](Movable):
    var held: Self.T

    def __init__(out self, held: Self.T):
        self.held = held

    def total(self, coin: Self.T) -> Int:
        return coin.spend() + self.held.spend()


def main():
    var ints = Shelf[Int](4, Optional[Int](2))
    var words = Shelf[String](2, None)
    print(ints.count(Bounds(Optional[Int](1), None)), words.count(Bounds(None, None)))
    print(ints.capped(5), words.capped(5))
    print(ints.pick(Optional[Int](7), 8), ints.pick(None, 9))
    print(words.pick(Optional[String]("x"), "y"), words.pick(None, "z"))
    var coins = Purse[Coin](Coin(1))
    var tokens = Purse[Token](Token(2))
    print(coins.total(Coin(3)), tokens.total(Token(4)))
