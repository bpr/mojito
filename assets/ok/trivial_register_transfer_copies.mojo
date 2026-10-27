# A `^` over a `TrivialRegisterPassable` value copies it, as upstream's
# `emitTransfer` decides: the source stays initialized, whether it is a
# local, a field, a `mut` or `var` parameter, a bounded type parameter, or a
# discard's operand.
@fieldwise_init
struct V(TrivialRegisterPassable):
    var n: Int


@fieldwise_init
struct P(Copyable):
    var v: V
    var k: Int


def take(mut v: V) -> V:
    return v^


def keep(var v: V) -> V:
    return v^


def generic_take[T: TrivialRegisterPassable](mut x: T) -> T:
    return x^


def main():
    var v = V(3)
    print(take(v).n, v.n)
    var w = v^
    print(w.n, v.n)
    var x = 1
    var y = x^
    print(x, y)
    var p = P(V(4), 5)
    var q = p.v^
    var k = p.k^
    print(q.n, k, p.v.n, p.k)
    _ = v^
    print(v.n)
    print(keep(v).n, v.n)
    var g = V(6)
    print(generic_take(g).n, g.n)
