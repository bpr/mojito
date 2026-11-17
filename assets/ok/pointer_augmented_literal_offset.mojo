# An augmented pointer offset converts its literal operand at `Int`, as
# upstream's `UnsafePointer.__iadd__` reads `index(offset)`: over a local,
# a struct's pointer field, and a `mut` pointer parameter, which the
# assignment stores through. Upstream prints `4`, `5`, and `4`.
from std.memory.alloc import unsafe_alloc


struct H:
    var p: UnsafePointer[Int, MutUntrackedOrigin]

    def __init__(out self, p: UnsafePointer[Int, MutUntrackedOrigin]):
        self.p = p


def advance(mut p: UnsafePointer[Int, MutUntrackedOrigin]):
    p += 1


def main():
    var buf = unsafe_alloc[Int](3)
    buf[0] = 3
    buf[1] = 4
    buf[2] = 5
    var p = buf
    p += 1
    print(p[])
    var h = H(buf)
    h.p += 2
    print(h.p[])
    var q = buf
    advance(q)
    print(q[])
    buf.unsafe_free()
