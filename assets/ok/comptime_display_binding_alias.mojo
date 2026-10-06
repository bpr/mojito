# A local `comptime` alias of a display binding (`comptime A = L`) is
# another name of the binding: a `comptime for` over it, a `comptime if` or
# a type argument reading an element or the length, and `materialize[A]()`
# read the display itself, in a generic struct's method as in a generic
# `def`.
def g[k: Int]():
    print("g", k)


struct G[n: Int]:
    def __init__(out self):
        pass

    def show(self):
        comptime L = [Self.n, Self.n + 1, Self.n * 3]
        comptime A = L
        comptime B = A
        comptime for x in B:
            print(x)
        comptime if len(B) == 3:
            comptime e = B[2]
            print("three", e)
        var m = materialize[B]()
        print(m[0] + m[1])


def reads[n: Int]():
    comptime L = [n, n * 2]
    comptime A = L
    comptime if A[0] > 2:
        print("big")
    comptime for i in range(len(A)):
        comptime e = A[i]
        print(e)
    g[A[1]]()
    g[len(A)]()


def set_alias[n: Int]():
    comptime S = {n, n + 1}
    comptime T = S
    comptime for x in T:
        print(x)


def main():
    G[4]().show()
    reads[3]()
    set_alias[7]()
