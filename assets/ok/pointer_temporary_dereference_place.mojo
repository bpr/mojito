# Dereferencing a pointer temporary is a place, as at the pin: it reads,
# assigns, passes as a `mut` argument, and binds a `ref` that aliases the
# pointee rather than a copy of it.
@fieldwise_init
struct S(Copyable, Movable):
    var x: Int


def bump(mut x: Int):
    x += 1


def main():
    var s = S(5)
    print(Pointer(to=s.x)[])
    Pointer(to=s.x)[] = 7
    print(s.x)
    bump(Pointer(to=s.x)[])
    print(s.x)
    ref r = Pointer(to=s.x)[]
    r = 11
    print(s.x)
    bump(Pointer(to=s).unsafe_origin_cast[origin_of(s)]()[].x)
    print(s.x)
