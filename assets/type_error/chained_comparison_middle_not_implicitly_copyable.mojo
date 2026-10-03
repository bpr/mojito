# expect: middle operand of a chained comparison
# A chained comparison copies its middle operand for the second link, so the
# operand must be `ImplicitlyCopyable`; a `Copyable` struct is not.
@fieldwise_init
struct P(Copyable, Movable):
    var v: Int

    def __eq__(self, other: Self) -> Bool:
        return self.v == other.v

    def __lt__(self, other: Self) -> Bool:
        return self.v < other.v


def main():
    var b = P(1)
    var c = P(1)
    var d = P(2)
    print(b == c < d)
