# A `comptime for` over tuple elements runs its body once per tuple, in a
# generic `def`, a struct's method, and a plain `def` alike: a module or local
# list of tuples, a display whose tuples read a parameter, and a dictionary's
# keys beside tuple values. A tuple of numbers is a parameter the template's
# loop binds; a tuple with a string element is no constant MIR carries, so the
# elaborator unrolls that loop in the AST. A tuple display subscripted as a
# temporary (`(1, 2)[0]`) reads its elements materialized. Output matches the
# pin.

comptime PAIRS = [(1, "a"), (2, "b")]
comptime D = {1: (2, 3), 4: (5, 6)}


def generic[n: Int]():
    comptime L = [(1, 2), (3, 4)]
    comptime for p in L:
        print(p[0] * n, p[1])
    comptime for p in PAIRS:
        print(p[0], p[1], n)
    comptime for p in [(1, 2), (3, n)]:
        print(p[0] + p[1])
    comptime for k in D:
        print(k + n)


def plain():
    comptime for p in PAIRS:
        print(p[1], p[0])


struct S[n: Int]:
    @staticmethod
    def show():
        comptime for p in PAIRS:
            print(p[1], p[0] + Self.n)


def main():
    generic[3]()
    plain()
    S[100].show()
    var x = (7, 8)[1]
    print(x, (1, 2)[0])
