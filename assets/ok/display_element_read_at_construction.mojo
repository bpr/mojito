# A display is its constructor's call, so each element is evaluated where
# written and read at that call's setup, after the last element, as the pinned
# Mojo reads it: a later element's write to an earlier element's place is
# seen by the earlier one. A tuple, a default `Array`, a `List`, a `Set`, and
# `Dict` displays (whose keys are all evaluated before their values), nested
# tuple and list displays, `String` elements, and a tuple element with a
# reference result.
struct U(Movable):
    var t: Int

    def __init__(out self, t: Int):
        self.t = t


def bump(mut u: U) -> Int:
    u.t += 100
    return 0


def app(mut s: String) -> Int:
    s += "!"
    return 0


def bt(mut t: Tuple[Int, Int]) -> Int:
    t[0] = 77
    return 0


def f(n: Int) -> Int:
    print("f", n)
    return n


def main():
    var u = U(8)
    var p = (u.t, bump(u))
    print(p[0], p[1])
    var v = U(9)
    var l = [v.t, bump(v)]
    print(l[0], l[1])
    var w = U(7)
    var a: List[Int] = [w.t, bump(w)]
    print(a[0], a[1])
    var x = U(6)
    var s = {x.t, bump(x)}
    print(len(s), 106 in s, 0 in s)

    var y = U(5)
    var d = {y.t: bump(y), bump(y): y.t}
    for e in d.items():
        print(e.key, e.value)
    var y2 = U(5)
    var d2 = {y2.t: y2.t, bump(y2): 0}
    for e in d2.items():
        print(e.key, e.value)
    var z = U(5)
    var d3 = {1: z.t, bump(z): 3}
    for e in d3.items():
        print(e.key, e.value)
    var d4 = {f(1): f(2), f(3): f(4)}
    print(len(d4))

    var n = U(4)
    var q = (n.t, (n.t, bump(n)), n.t)
    print(q[0], q[1][0], q[2])
    var m = U(5)
    var r = [m.t, [m.t, bump(m)][0], m.t]
    print(r[0], r[1], r[2])

    var str = String("hi")
    var ws: List[String] = [str, String(app(str))]
    print(ws[0], ws[1])
    var s2 = String("yo")
    var ts = (s2, app(s2), s2)
    print(ts[0], ts[2])

    var t = (1, 2)
    var rt = (t[0], bt(t))
    print(rt[0], rt[1])
