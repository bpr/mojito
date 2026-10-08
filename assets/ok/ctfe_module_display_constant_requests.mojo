# A module constant whose initializer is a display, a tuple, or a subscript
# that applies a callable waits for its first demand like every other
# applied constant: a `comptime for`, a body's `comptime` binding, a
# compile-time condition, a type argument, `materialize`, and a method of a
# dictionary display all read it.
@fieldwise_init
struct Q(Copyable, Movable):
    var a: Int
    var s: String

def mk(n: Int) -> Q:
    return Q(n, String(n) + "!")

def twice(n: Int) -> Int:
    return n * 2

def f(x: Int) -> Int:
    return x * 2

comptime QS = [Q(1, "x"), Q(2, "y"), mk(7)]
comptime first = QS[0].s
comptime XS = [1, twice(3), 5]
comptime Y = XS[1]
comptime N = len(XS)
comptime T = (twice(1), "a")
comptime Z = T[0]
comptime D = {"a": f(1), "b": 3}
comptime A = D.get("a").value()

def show[n: Int]():
    print(n)

def main():
    comptime for q in QS:
        print(q.a, q.s)
    print(first)
    comptime q2 = QS[2]
    print(q2.s, q2.a)
    comptime n = XS[2]
    print(n, Y, Z)
    print(T[0], T[1])
    show[N]()
    comptime if XS[0] == 1:
        print("one")
    var xs = materialize[XS]()
    print(xs[1])
    print(A)
    comptime b = D.get("b").value()
    print(b)
