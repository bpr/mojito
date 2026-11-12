# A `comptime` alias of an unapplied parametric struct names that struct's
# generator: an application through the alias, with a hole, an argument, a
# defaulted parameter, or an origin, is the struct's own application, and a
# struct whose open parameters include an origin (`Span`, `Origin`) binds
# the same way.
struct P[n: Int, m: Int = 3]:
    def __init__(out self):
        pass


comptime Q = P
comptime S = Span
comptime O = Origin


def holes(x: Q[_]):
    print(x.n, x.m)


def applied(x: Q[2]):
    print(x.n, x.m)


def bare(x: Q):
    print(x.n, x.m)


def view(x: StringSlice[_]):
    print(x)


def spanned(x: S[Int, _]):
    print(len(x))


def by_origin[o: O](ref[o] x: Int):
    print(x)


def main():
    holes(P[2]())
    applied(P[2]())
    bare(P[1, 2]())
    var q = Q[5]()
    print(q.n, q.m)
    view(StaticString("a"))
    var s: StringSlice[ImmStaticOrigin] = "b"
    print(s)
    print("c" + String("d"))
    var l = [1, 2, 3]
    spanned(Span(l))
    var n = 4
    by_origin(n)
