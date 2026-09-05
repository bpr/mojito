# The `~` prefix operator: two's complement on integers, logical on `Bool`,
# and a user struct's `__invert__` (Optional's is `not bool`).
@fieldwise_init
struct Mask(Copyable, Movable):
    var bits: Int
    def __invert__(self) -> Int:
        return -self.bits
def main():
    var m = Mask(5)
    print(~m)
    print(~5, ~True)
    var o = Optional[Int](3)
    print(~o)
    var n = Optional[Int]()
    print(~n)
