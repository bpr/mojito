# A local `comptime` binding of a display over the binders is a compile-time
# value with no runtime form, whatever reads it: a `comptime if` condition, a
# `range` bound, or another `comptime` binding reads an element or the length
# in a function the elaborator runs per instance, and `materialize[L]()`, or
# a value that is no `Int` or `Bool`, builds the display where it crosses to
# runtime.
@fieldwise_init
struct Pair[n: Int]:
    var base: Int

    def show(self):
        comptime L = [Self.n, Self.n + 10]
        comptime if L[0] == 4:
            print("four")
        comptime for i in range(len(L)):
            print(self.base + i)
        comptime for x in L:
            print(self.base + x)
        var l = materialize[L]()
        print(l[1])


def element[n: Int]():
    comptime L = [n, n * 2]
    comptime if L[0] == 3:
        print("three")
    elif L[1] == 10:
        print("ten")
    else:
        print("other")


def beside[n: Int]():
    comptime L = [n, n * 2]
    comptime if L[0] == 3:
        print("three")
    comptime for i in range(len(L)):
        print(i)
    var l = materialize[L]()
    l[0] += 100
    print(l[0], l[1], len(l))
    comptime for x in L:
        print(x)


def bound[n: Int]():
    comptime L = [n, n * 2]
    comptime e = L[1]
    comptime k = len(L)
    print(e + 1, k)
    comptime if e == 8:
        print("eight")
    comptime for i in range(k):
        print("k", i)
    comptime for i in range(1, e, 3):
        print("e", i)
    comptime for x in [e, e + k]:
        print("d", x)


def chained[n: Int]():
    comptime L = [n, n + 1]
    comptime M = [L[1], L[0] + L[1], len(L)]
    comptime for x in M:
        print(x)
    comptime if M[1] == 2 * n + 1 and 3 in M:
        print("chained")


def looped[n: Int]():
    comptime for i in range(n):
        comptime K = [i, i + n]
        comptime if K[1] == n:
            print("first", i)
        comptime for j in range(K[0], len(K)):
            print(i, j)
        var k = materialize[K]()
        print(k[1])


def flags[n: Int]():
    comptime holds = [n > 1, n > 4]
    comptime if holds[0] and not holds[1]:
        print("between")
    elif holds[1]:
        print("above")
    else:
        print("below")


def keyed[n: Int]():
    comptime S = {n, n + 1}
    comptime D = {"a": n, "b": n + 1}
    comptime if n + 1 in S:
        print("member")
    comptime if len(D) == 2:
        print("two keys")
    var s = materialize[S]()
    print(len(s))


def crossed[n: Int]():
    comptime vals = [n, n * 2]
    print(comptime(len(vals)), comptime(vals[0] + 1))
    print(materialize[vals[1]]())
    comptime for i in range(len(vals)):
        comptime v = vals[i]
        print(materialize[vals[i]]() + v)


def values[n: Int]():
    comptime L = [n, n * 2]
    comptime t = (L[0], L[1])
    print(t[0], t[1])
    comptime s = "x" if L[0] == 4 else "y"
    print(s)
    comptime M = [L[1], L[0] + 1]
    var m = materialize[M]()
    print(m[0], m[1])
    print(comptime((L[0], M[1]))[1])


def derived[n: Int]():
    comptime L = [n, n * 2]
    comptime t = (L[0], L[1])
    comptime if t[0] == 4:
        print("four")
    comptime big = n > 2
    comptime if big and len(L) == 2:
        print("big")
    comptime e = L[1]
    comptime past = e + 1
    comptime for i in range(past - 8):
        print(i)


def main():
    derived[4]()
    values[4]()
    crossed[6]()
    element[3]()
    element[5]()
    element[7]()
    beside[3]()
    beside[5]()
    bound[4]()
    chained[1]()
    looped[2]()
    flags[0]()
    flags[2]()
    flags[9]()
    keyed[3]()
    Pair[4](100).show()
