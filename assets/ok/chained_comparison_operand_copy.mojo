# A chained comparison reads its two end operands where they lie and copies
# its middle operand once, since both links read it; a temporary middle
# operand moves in without a copy. Each line prints one `copy` at most, as
# upstream.
@fieldwise_init
struct P(ImplicitlyCopyable, Movable):
    var v: Int

    def __init__(out self, *, copy: Self):
        self.v = copy.v
        print("copy")

    def __eq__(self, other: Self) -> Bool:
        return self.v == other.v

    def __lt__(self, other: Self) -> Bool:
        return self.v < other.v


def make(v: Int) -> P:
    return P(v)


def main():
    var b = P(1)
    var c = P(1)
    var d = P(2)
    print(b == c < d)
    print(b == make(1) < d)
    print(b < c == d)
    print(b < d < d)
    print(b.v, c.v, d.v)
