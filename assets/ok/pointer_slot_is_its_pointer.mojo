# The whole of a slot holding a pointer is the pointer, a value: a type
# parameter's copy construction and requirement call over a pointer
# instance pass the pointer slot's address and copy the pointer, a
# one-field register-passable struct of a pointer field is held as that
# pointer, and a `mut` pointer parameter reseats the caller's pointer.
from std.memory.alloc import unsafe_alloc


def make[T: Copyable & Movable](x: T) -> T:
    return T(copy=x)


def cp[T: Copyable](a: T) -> T:
    return a.copy()


@fieldwise_init
struct S[o: MutOrigin](TrivialRegisterPassable):
    var p: Pointer[Int, Self.o]

    def get(self) -> Int:
        return self.p[]


def reseat(
    mut p: Pointer[Int, MutUntrackedOrigin],
    q: Pointer[Int, MutUntrackedOrigin],
):
    p = q


def bump(mut p: Pointer[Int, MutUntrackedOrigin]):
    p[] += 10
    p = p + 1


def main():
    var x = 4
    var p = Pointer(to=x)
    var q = make(p)
    var r = cp(p)
    print(q[], r[])
    var y = 5
    var s = S(Pointer(to=y))
    print(s.get())
    var buf = unsafe_alloc[Int](2)
    buf[0] = 1
    buf[1] = 2
    var a = buf
    reseat(a, buf + 1)
    print(a[])
    var b = buf
    bump(b)
    print(buf[0], b[])
    buf.unsafe_free()
