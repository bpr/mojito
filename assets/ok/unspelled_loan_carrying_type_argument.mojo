# A generic `def` whose loan-carrying type argument reaches only its result
# (`unsafe_alloc[Span[Int, origin_of(xs)]](n)`, a user `make[T](n) ->
# Pointer[T, MutUntrackedOrigin]`) is cloned: each origin slot of the argument
# becomes an explicit origin binder of the clone, which the call supplies
# from its own application. An explicit origin argument binds the origin
# slots of the callee's result type too (`slots[origin_of(xs)](n)`).
from std.memory.alloc import unsafe_alloc


def make[T: AnyType](n: Int) -> Pointer[T, MutUntrackedOrigin]:
    return unsafe_alloc[T](n)


def slots[o: MutOrigin](n: Int) -> Pointer[Span[Int, o], MutUntrackedOrigin]:
    return unsafe_alloc[Span[Int, o]](n)


def main():
    var xs: List[Int] = [4, 5, 6]
    var p = unsafe_alloc[Span[Int, origin_of(xs)]](2)
    p.unsafe_offset(0).unsafe_write(Span(xs))
    p.unsafe_offset(1).unsafe_write(Span(xs))
    print(len(p[0]), p[1][2])
    p.free()

    var q = make[Span[Int, origin_of(xs)]](1)
    q.unsafe_write(Span(xs))
    print(q[0][1])
    q.free()

    var r = slots[origin_of(xs)](1)
    r.unsafe_write(Span(xs))
    print(r[0][0])
    r.free()
    print(xs[2])
