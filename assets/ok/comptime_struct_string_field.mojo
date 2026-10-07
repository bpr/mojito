# A compile-time struct value whose fields are `String`s or nested fieldwise
# structs freezes field by field: a display iterated by `comptime for`, a
# module and a body binding, a field read bound as `comptime`, and a CTFE
# function result.
@fieldwise_init
struct Q(Copyable, Movable):
    var a: Int
    var s: String

@fieldwise_init
struct W(Copyable, Movable):
    var q: Q
    var t: String

def mk(n: Int) -> Q:
    return Q(n, String(n) + "!")

comptime QS = [Q(1, "x"), Q(2, "y"), mk(7)]
comptime w = W(Q(4, "in"), "out")
comptime first = QS[0].s

def show[n: Int]():
    comptime PS = [Q(1, "x"), Q(n, "y")]
    comptime for q in PS:
        print(q.a, q.s)

def main():
    print(w.q.s, w.t, w.q.a)
    print(first, first.byte_length())
    comptime for q in QS:
        print(q.a, q.s, q.s.byte_length())
    comptime r = Q(3, "z")
    print(r.a, r.s)
    comptime m = mk(9)
    print(m.s)
    show[2]()
