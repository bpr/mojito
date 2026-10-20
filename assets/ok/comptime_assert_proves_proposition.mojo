# A `comptime assert` makes every proposition it states known for the rest of
# its block, as a `comptime if` arm knows its condition (the manual's
# "introducing knowledge"): a callee's `where n >= 0` is proven by the
# assertion, by a conjunct of it, and by an enclosing arm, as at the pin. The
# condition may be parenthesized or span lines.

def g[n: Int]() where n >= 0:
    print(n)


def by_assert[n: Int]():
    comptime assert n >= 0, "nonneg"
    g[n]()


def by_conjunct[n: Int]():
    comptime assert n >= 0 and n < 10, "range"
    g[n]()


def by_arm[n: Int]():
    comptime if n >= 0:
        g[n]()


def main():
    comptime k = 3
    comptime assert k == 3, "k is three"
    comptime assert (
        k > 1
    ), "k above one"
    comptime assert (k > 1) and (k < 5), "k in range"
    comptime assert k < 10
    by_assert[1]()
    by_conjunct[2]()
    by_arm[k]()
