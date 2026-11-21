# A pointer offsets backwards through upstream's `Pointer.__sub__`, which
# reads `index(offset)`, and Mojito's `-=`: `p - 1` over a local,
# `-=` over a local, a struct's pointer field, and a `mut` pointer
# parameter, and `-` by a runtime `Int`. Upstream prints `4`, `3`, `4`,
# `4`, and `3`.
from std.memory.alloc import unsafe_alloc


struct H:
    var p: Pointer[Int, MutUntrackedOrigin]

    def __init__(out self, p: Pointer[Int, MutUntrackedOrigin]):
        self.p = p


def retreat(mut p: Pointer[Int, MutUntrackedOrigin]):
    p -= 1


def main():
    var buf = unsafe_alloc[Int](3)
    buf[0] = 3
    buf[1] = 4
    buf[2] = 5
    var end = buf + 2
    var p = end - 1
    print(p[])
    var q = end
    q -= 2
    print(q[])
    var h = H(end)
    h.p -= 1
    print(h.p[])
    var r = end
    retreat(r)
    print(r[])
    var n = 2
    print((end - n)[])
    buf.unsafe_free()
