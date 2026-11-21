# expect: expected a mutable origin for parameter 'o' of 'V'
# A construction's bracket binds `m=True` before the initializer's arguments
# bind the origin `o` it governs, so an immutable pointer argument does not
# convert (pinned Mojo: "no matching function in initialization").
struct V[m: Bool, //, o: Origin[mut=m]]:
    var p: Pointer[Int, Self.o]

    def __init__(out self, p: Pointer[Int, Self.o]):
        self.p = p


def g(q: Pointer[Int, ImmUntrackedOrigin]) -> Int:
    var v = V[m=True](q)
    return v.p[]


def main():
    var x = 3
    print(g(Pointer(to=x).as_imm().unsafe_origin_cast[ImmUntrackedOrigin]()))
    _ = x
