# expect: for parameter 'IteratorType' does not conform to trait 'Copyable'
# A `comptime for` over a list display iterates its `__iter__()`, which
# requires a `Copyable` element.
@fieldwise_init
struct P:
    var a: Int
    var b: Bool


def main():
    comptime for p in [P(1, True), P(2, False)]:
        print(p.a, p.b)
