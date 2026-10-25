# A struct slot read and written through its fields is split into one slot
# per field, as upstream's `SROA` splits the allocation, so `mem-2-reg`
# promotes each field: a fieldwise-built slot, an inlined constructor's
# `out self`, a nested struct, a slot read whole after a field write, a
# moved slot, and an empty struct. A register-passable struct argument read
# through its fields is stored into a slot that splits too, as upstream's
# `LowerArgConventions` stores it for `SROA` (`take_q`, read and owned); a
# memory-only one stays behind its address, as upstream leaves it (`take`).


@fieldwise_init
struct Pt(Copyable, Movable):
    var x: Int
    var y: Int


struct Q(Copyable, Movable, TrivialRegisterPassable):
    var a: Int
    var b: Int

    @always_inline
    def __init__(out self, a: Int, b: Int):
        self.a = a
        self.b = b


@fieldwise_init
struct Seg(Copyable, Movable):
    var start: Pt
    var end: Pt


@fieldwise_init
struct Empty(Copyable, Movable):
    pass


@no_inline
def take(p: Pt) -> Int:
    return p.x * 10 + p.y


@no_inline
def take_q(q: Q) -> Int:
    return q.a * 10 + q.b


@no_inline
def take_owned_q(var q: Q) -> Int:
    q.b += 1
    return q.a * 10 + q.b


@no_inline
def take_empty(e: Empty) -> Int:
    return 7


def fields(a: Int) -> Int:
    var p = Pt(a, 2)
    p.x += 1
    return p.x + p.y


def inlined(a: Int) -> Int:
    var q = Q(a, 3)
    q.b += 1
    return q.a * 100 + q.b


def nested(a: Int) -> Int:
    var s = Seg(Pt(a, 1), Pt(2, 3))
    s.end.y += a
    return s.start.x + s.end.y


def whole(a: Int) -> Int:
    var p = Pt(a, 2)
    p.y = 7
    return take(p)


def moved(a: Int) -> Int:
    var p = Pt(a, 4)
    p.x -= 1
    var r = p^
    return r.x + r.y


def empty() -> Int:
    var e = Empty()
    return take_empty(e)


def main():
    print(fields(5))
    print(inlined(5))
    print(nested(4))
    print(whole(4))
    print(moved(9))
    print(empty())
    print(take_q(Q(4, 5)))
    print(take_owned_q(Q(1, 2)))
