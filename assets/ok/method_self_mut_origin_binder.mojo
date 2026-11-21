# A method's origin binder typed `Origin[mut=Self.mut]` takes the receiver's
# `mut`: a mutable receiver takes a mutable origin, an immutable one takes
# either (a mutable origin converts to its immutable form), and inside the
# struct `Self.o` has the binder's own mutability.
struct S[mut: Bool, //, o: Origin[mut=mut]]:
    var x: Int

    def __init__(out self, x: Int):
        self.x = x

    def g[t: Origin[mut=Self.mut]](self):
        print(self.x)

    def h(self):
        self.g[Self.o]()

def main():
    var y = 1
    var s = S[origin_of(y)](3)
    s.g[origin_of(y)]()
    s.h()
    var r = S[ImmOrigin(origin_of(y))](4)
    r.g[origin_of(y)]()
    r.g[ImmOrigin(origin_of(y))]()
    r.h()
