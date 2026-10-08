# A compile-time initializer is the lifted thunk's `return` operand: a field
# or an element of a temporary it builds is copied out before the thunk
# drops the temporary, and a `Copyable` element crosses without being
# `ImplicitlyCopyable`. A tuple value holding a `String` is constructed at
# its `Tuple` instance, and a field read of a module struct constant is a
# compile-time projection of it.
@fieldwise_init
struct Q(Copyable, Movable):
    var a: Int
    var s: String

@fieldwise_init
struct W(Copyable, Movable):
    var q: Q
    var t: String

@fieldwise_init
struct NC(Movable):
    var a: Int

@fieldwise_init
struct H(Movable):
    var n: NC

def mk(n: Int) -> Q:
    return Q(n, String(n) + "!")

def twice(n: Int) -> Int:
    return n * 2

comptime q0 = mk(3)

def projections():
    comptime first = [Q(1, "x"), mk(7)][0].s
    print(first)
    comptime q = [Q(1, "x"), mk(3)][1]
    print(q.s, q.a)
    comptime qq = W(mk(4), "out").q
    print(qq.s)
    comptime ss = W(mk(5), "out").q.s
    print(ss)

def moved():
    comptime r = [NC(1), NC(2)][0]
    comptime h = H(NC(4)).n
    print(r.a, h.a)

def tuple_value():
    comptime T = (twice(1), "a")
    print(T[0], T[1])

def generic[n: Int]():
    comptime first = [Q(n, "x"), mk(7)][0].s
    comptime q = [Q(1, "x"), mk(n)][1]
    print(first, q.s, q.a)

def main():
    projections()
    moved()
    tuple_value()
    generic[3]()
    generic[5]()
    print(q0.s, q0.a)
