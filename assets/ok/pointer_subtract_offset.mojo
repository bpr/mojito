# A pointer offsets backwards through upstream's `unsafe_offset` by a
# negative index: over a local, stored back over a local, a struct's pointer
# field, and a `mut` pointer parameter, and by a runtime `Int`. Upstream's
# `__sub__` takes only a pointer, so `p - 1` is rejected
# (`type_error/pointer_subtract_int_rejected.mojo`), and it declares no
# `__isub__`. Upstream prints `4`, `3`, `4`, `4`, and `3`.
from std.memory.alloc import unsafe_alloc


struct H:
    var p: Pointer[Int, MutUntrackedOrigin]

    def __init__(out self, p: Pointer[Int, MutUntrackedOrigin]):
        self.p = p


def retreat(mut p: Pointer[Int, MutUntrackedOrigin]):
    p = p.unsafe_offset(-1)


def main():
    var buf = unsafe_alloc[Int](3)
    buf[0] = 3
    buf[1] = 4
    buf[2] = 5
    var end = buf.unsafe_offset(2)
    var p = end.unsafe_offset(-1)
    print(p[])
    var q = end
    q = q.unsafe_offset(-2)
    print(q[])
    var h = H(end)
    h.p = h.p.unsafe_offset(-1)
    print(h.p[])
    var r = end
    retreat(r)
    print(r[])
    var n = 2
    print(end.unsafe_offset(-n)[])
    buf.unsafe_free()
