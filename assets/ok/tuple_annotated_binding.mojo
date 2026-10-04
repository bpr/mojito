# A `Tuple[...]` annotation, parameter, or field keeps the tuple members
# (`reverse`, `concat`, `__len__`, membership), and a tuple display or bare
# `Tuple(...)` call infers a string element as `String`, so it binds where a
# `Tuple[Int, String]` is declared.


def flip(t: Tuple[Int, Bool]) -> Tuple[Bool, Int]:
    return t.reverse()


def extend(t: Tuple[Int, Bool]):
    var c = t.concat(Tuple(2.5))
    print(c[2])


struct Holder:
    var t: Tuple[Int, Bool]

    def __init__(out self):
        self.t = (3, False)


def main():
    var b: Tuple[Int, Bool] = (1, True)
    var r = b.reverse()
    print(r[0], r[1], 1 in b)
    var f = flip(b)
    print(f[0], f[1])
    extend(b)
    var h = Holder()
    print(h.t.reverse()[0])
    var c = h.t.concat((7, 2.5))
    print(c[2], len(h.t))
    var t: Tuple[Int, String] = (1, "x")
    print(t[0], t[1], len(t))
    var u: Tuple[Int, String] = Tuple(2, "z")
    print(u[1])
    var w = (4, "q")
    var v: Tuple[Int, String] = w
    print(v[1])
