# A value pack's raw list `vs.values` is `ParameterList`'s one parameter, as
# upstream declares it: `ParameterList[vs.values]()` reads, sizes, and
# iterates the pack, a struct's own pack is `Self.values.values`, a
# `comptime` binding of the raw list applies as the list itself, and a
# struct's value parameter reads through its type (`S[7].n`).
struct S[n: Int]:
    def __init__(out self):
        pass


def f[*vs: Int]():
    comptime raw = vs.values
    var pl = ParameterList[vs.values]()
    print(len(pl), pl[0], ParameterList[vs.values].size)
    for v in pl:
        print(v)
    print(vs)
    print(ParameterList[raw].size, S[7].n)


def g[*vs: Int]():
    comptime pl = ParameterList[vs.values]()
    comptime n = pl.size
    print(n, vs.size, len(vs), vs[1])
    var l = ParameterList[vs.values]()
    print(l)
    print(repr(l))


struct Packed[*values: Int]:
    @staticmethod
    def show():
        var p = ParameterList[Self.values.values]()
        print(len(p), p)


def main():
    f[1, 2, 3]()
    g[4, 5, 6]()
    Packed[4, 5].show()
