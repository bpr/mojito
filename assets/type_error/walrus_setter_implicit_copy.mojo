# expect: cannot be implicitly copied
# A subscript stored through its `__setitem__` is no memory lvalue, so a
# walrus over it yields an implicit copy of its right-hand side, which a
# non-ImplicitlyCopyable type refuses.
@fieldwise_init
struct C(Copyable, Movable):
    var v: Int


struct Box(Movable):
    var c: C

    def __init__(out self):
        self.c = C(0)

    def __getitem__(self, i: Int) -> C:
        return self.c.copy()

    def __setitem__(mut self, i: Int, var value: C):
        self.c = value^


def main():
    var b = Box()
    print((b[0] := C(4)).v)
