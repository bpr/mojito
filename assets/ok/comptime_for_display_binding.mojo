# A local `comptime` binding of a list, set, or dictionary display over the
# binders, read only as the iterable of a `comptime for`, is served by the
# template: MIR lifts the display once, where the binding is declared, and
# the elaborator runs it per instance for every loop over the name. A
# display may read a local `comptime` value bound before it.
@fieldwise_init
struct Pair[n: Int]:
    var base: Int

    def show(self):
        comptime L = [Self.n, Self.n + 10]
        comptime for x in L:
            print(self.base + x)


def around[n: Int]():
    comptime L = [n, n * 2]
    comptime for x in L:
        print(x)


def twice[n: Int, m: Int]():
    comptime L = [n, n * 2]
    comptime M = {m: 1, m + 1: 2}
    comptime for x in L:
        comptime for y in M:
            print(x, y)
    comptime for x in L:
        print(x + m)


def squared[n: Int]():
    comptime L = [n, n + 1]
    comptime for x in L:
        comptime for y in L:
            comptime if x < y:
                print(x, y)


def nested[n: Int]():
    comptime for i in range(n):
        comptime K = [i, i + n]
        comptime for x in K:
            print(i, x)


def guarded[n: Int]():
    comptime k = n + 1
    comptime j = k * 2
    comptime L = [k, j, n]
    comptime if n > 2:
        comptime for x in L:
            print("big", x)
    else:
        comptime for x in L:
            print("small", x)


def flags[n: Int]():
    comptime names = ["lo", "hi"]
    comptime holds = [n > 1, n > 4]
    comptime for name in names:
        comptime for b in holds:
            comptime if b:
                print(name, "holds")
            else:
                print(name, "fails")


def keyed[n: Int]():
    comptime S = {n, n, n + 1}
    comptime D = {"a": n, "b": n + 1}
    comptime for x in S:
        print(x)
    comptime for k in D:
        print(k)


def summed[n: Int]() -> Int:
    comptime L = [n, n + 1, n + 2]
    var total = 0
    comptime for x in L:
        total += x
    return total


def header[n: Int]():
    comptime k = n + 1
    comptime for x in [k, k * 2]:
        print(x)
    comptime for i in range(2):
        comptime q = i + k
        comptime for y in [q, q * n]:
            print(y)


def main():
    around[3]()
    around[5]()
    twice[3, 7]()
    squared[4]()
    nested[2]()
    guarded[1]()
    guarded[5]()
    flags[2]()
    keyed[3]()
    print(summed[10]())
    header[1]()
    Pair[4](100).show()
