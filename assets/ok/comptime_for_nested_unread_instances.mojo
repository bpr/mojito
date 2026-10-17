# A `def` declared in a `comptime for` is a generator over the loop's
# binder beside its enclosing scope's, so it keys its instances only on
# those it reads, as upstream's `RemoveUnusedParams` treats the lifted
# function: `ignores[1]` and `ignores[2]` share one `inner`, unrolled twice,
# and one instance of `ignores`, whose loop bound reads no `n`; a body
# reading the loop index keeps one per index, and one reading `n` one per
# `n`. A generic one keys on its own parameters it reads too, a `def`
# nested in one on what it reads, and a loop nested in the loop adds a
# binder it may leave unread. A struct instance's method calls the instance
# of a `def` declared in its loop, not a constructor of the struct.
def ignores[n: Int](x: Int) -> Int:
    var total = 0

    comptime for i in range(2):

        def inner(y: Int) -> Int:
            return y + 1

        total += inner(x)
    return total


def reads_index[n: Int](x: Int) -> Int:
    var total = 0

    comptime for i in range(2):

        def inner(y: Int) -> Int:
            return y + i

        total += inner(x)
    return total


def reads_outer[n: Int](x: Int) -> Int:
    var total = 0

    comptime for i in range(2):

        def inner(y: Int) -> Int:
            return y + n

        total += inner(x)
    return total


def generic_ignores[n: Int](x: Int) -> Int:
    var total = 0

    comptime for i in range(2):

        def inner[m: Int](y: Int) -> Int:
            return y * 2

        total += inner[1](x) + inner[2](x)
    return total


def generic_reads_own[n: Int](x: Int) -> Int:
    var total = 0

    comptime for i in range(2):

        def inner[m: Int](y: Int) -> Int:
            return y + m

        total += inner[1](x) + inner[2](x)
    return total


def two_loops[n: Int](x: Int) -> Int:
    var total = 0

    comptime for i in range(2):

        comptime for j in range(2):

            def inner(y: Int) -> Int:
                return y + i

            total += inner(x)
    return total


def valued[n: Int](x: Int) -> Int:
    var total = 0

    comptime for i in range(2):

        def inner(y: Int) -> Int:
            return y - 1

        var f = inner
        total += f(x)
    return total


def outer_loop[n: Int](x: Int) -> Int:
    var total = 0

    comptime for i in range(2):

        def middle(y: Int) -> Int:
            def leaf(z: Int) -> Int:
                return z + 3

            return leaf(y)

        total += middle(x)
    return total


struct Counter[k: Int]:
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def get(self) -> Int:
        var total = 0

        comptime for i in range(2):

            def bump(y: Int) -> Int:
                return y + 1

            total += bump(self.v)
        return total


def main():
    print(ignores[1](3), ignores[2](4))
    print(reads_index[1](3), reads_index[2](4))
    print(reads_outer[1](3), reads_outer[2](4))
    print(generic_ignores[1](3), generic_ignores[2](4))
    print(generic_reads_own[1](3), generic_reads_own[2](4))
    print(two_loops[1](3), two_loops[2](4))
    print(valued[1](3), valued[2](4))
    print(outer_loop[1](3), outer_loop[2](4))
    print(Counter[1](1).get(), Counter[2](2).get())
