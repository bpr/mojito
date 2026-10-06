# A `comptime for` over a list, set, or dictionary display of scalar
# expressions over the binders is served by the template: MIR lifts the
# display as a function over the binders in scope — the owner's, then the
# enclosing loops' variables — and the elaborator runs it per instance and
# unrolls the loop over the value it builds.
@fieldwise_init
struct Pair[n: Int]:
    var base: Int

    def show(self):
        comptime for x in [Self.n, Self.n + 1]:
            print(self.base + x)


def around[n: Int]():
    comptime for x in [n, n + 1]:
        print(x)


def nested[n: Int]():
    comptime for i in range(n):
        comptime for x in [i, i + 10]:
            print(x)


def flags[b: Bool]():
    comptime for x in [b, not b]:
        comptime if x:
            print("yes")
        else:
            print("no")


def compared[n: Int]():
    comptime for big in [n > 2, n == 2]:
        comptime if big:
            print("holds")
        else:
            print("fails")


def keyed[n: Int]():
    comptime for k in {n: 1, n + 1: 2}:
        print(k)


def distinct[n: Int]():
    comptime for x in {n, n + 1, n}:
        print(x)


def strings[n: Int]():
    comptime for a in ["x", "y"]:
        comptime for b in [a, "z"]:
            print(b, n)


def scaled[n: Int]():
    comptime for i in range(n):
        comptime for x in [i * n]:
            print(x)
    print("done")


def main():
    around[1]()
    around[5]()
    nested[2]()
    flags[True]()
    compared[2]()
    keyed[3]()
    distinct[4]()
    strings[1]()
    scaled[0]()
    scaled[3]()
    Pair[4](0).show()
