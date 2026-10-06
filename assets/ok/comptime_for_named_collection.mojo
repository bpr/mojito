# A `comptime for` over a named closed collection — a module `comptime`
# constant (a list, a dictionary's keys, strings, floats) or a local
# `comptime` binding of a literal display — is served by its generic `def`'s
# template: the loop header carries the collection's elements, and the
# elaborator unrolls it per instance. A compile-time `break` leaves the loop.
comptime L = [10, 20]
comptime D = {1: 2, 3: 4}
comptime S = ["a", "b"]
comptime F = [1.5, 2.5]


@fieldwise_init
struct Offset[n: Int]:
    var base: Int

    def show(self):
        comptime for x in L:
            print(self.base + x + Self.n)


def shifted[n: Int]():
    comptime for x in L:
        print(x + n)


def keys[n: Int]():
    comptime for k in D:
        print(k + n)


def names[n: Int]():
    comptime for s in S:
        print(s, n)


def local[n: Int]():
    comptime M = [4, 5]
    comptime for x in M:
        print(x * n)


def first[n: Int]():
    comptime for x in L:
        comptime if x == 20:
            break
        print(x - n)


def floats[n: Int]():
    comptime for x in F:
        print(x + 1.0)
    comptime for y in [0.5, 0.25]:
        print(y)


def main():
    shifted[1]()
    shifted[5]()
    keys[1]()
    names[2]()
    local[3]()
    first[1]()
    floats[1]()
    Offset[2](10).show()
