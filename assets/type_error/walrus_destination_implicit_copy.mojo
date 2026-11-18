# expect: cannot be implicitly copied
# A walrus over a field yields its destination borrowed, so binding it to a
# new variable copies it, which a non-ImplicitlyCopyable type refuses.
@fieldwise_init
struct C(Copyable):
    var v: Int


struct H(Movable):
    var c: C

    def __init__(out self):
        self.c = C(1)


def main():
    var h = H()
    var d = (h.c := C(6))
    print(d.v)
