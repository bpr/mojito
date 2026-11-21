# A pointer offset by a literal through upstream's `unsafe_offset`, which
# reads `index(offset)`, stored back over a local, a struct's pointer field,
# and a `mut` pointer parameter, which the assignment stores through. The
# augmented `p += 1` is rejected, as upstream's `Pointer` declares no
# `__iadd__` (`type_error/pointer_augmented_offset_rejected.mojo`).
# Upstream prints `4`, `5`, and `4`.
from std.memory.alloc import unsafe_alloc


struct H:
    var p: Pointer[Int, MutUntrackedOrigin]

    def __init__(out self, p: Pointer[Int, MutUntrackedOrigin]):
        self.p = p


def advance(mut p: Pointer[Int, MutUntrackedOrigin]):
    p = p.unsafe_offset(1)


def main():
    var buf = unsafe_alloc[Int](3)
    buf[0] = 3
    buf[1] = 4
    buf[2] = 5
    var p = buf
    p = p.unsafe_offset(1)
    print(p[])
    var h = H(buf)
    h.p = h.p.unsafe_offset(2)
    print(h.p[])
    var q = buf
    advance(q)
    print(q[])
    buf.unsafe_free()
