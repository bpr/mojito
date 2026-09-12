# Copying a bound `Pointer` local reads the pointer value, not the local's
# slot: into another local, an annotated binding, a constructor argument, a
# free function's origin binder, and a reassigned local that reads and
# writes through its runtime handle.
@fieldwise_init
struct Plain[o: Origin]:
    var p: Pointer[Int, Self.o]

    def get(self) -> Int:
        return self.p[]


def peek[o: Origin](p: Pointer[Int, o]) -> Int:
    var t = p
    return t[]


def main():
    var x = 5
    var q = Pointer(to=x)
    var t = q
    print(t[])
    var s: Pointer[Int, origin_of(x)] = q
    print(s[])
    var e = Plain(q)
    print(e.get())
    print(peek(q))
    var u = q
    print(u[])
    u = q
    u[] = 11
    print(u[])
    var w = u
    w[] += 1
    print(x)
