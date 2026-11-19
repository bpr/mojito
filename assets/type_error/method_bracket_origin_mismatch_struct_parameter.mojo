# expect: invalid call to 'view'
# A method's bracket binds its origin binder in its declared slot, numbered
# past its struct's parameters as its signature numbers it, so the bound
# origin must hold the argument's place. Both compilers reject (pinned Mojo:
# "value passed to 'p' cannot be converted from 'Pointer[Int,
# origin_of(b)]' to 'Pointer[Int, origin_of(a)]'").
struct G[T: ImplicitlyCopyable & Deinitable]:
    var v: Self.T

    def __init__(out self, v: Self.T):
        self.v = v

    def view[o: ImmOrigin](self, p: Pointer[Int, o]) -> Pointer[Int, o]:
        return p


def main():
    var a = 1
    var b = 2
    var g = G[Int](0)
    print(g.view[origin_of(a)](Pointer(to=b))[])
