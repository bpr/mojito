# expect: does not match the signature required by trait 'Reads'
# A trait requirement's own origin binders compare by position: a witness
# naming its two binders in the other order is another signature.
trait Reads:
    def both[a: ImmOrigin, b: ImmOrigin](
        self, x: Pointer[Int, a], y: Pointer[Int, b]
    ) -> Int:
        ...


struct B[so: ImmOrigin](Reads):
    def __init__(out self):
        pass

    def both[a: ImmOrigin, b: ImmOrigin](
        self, x: Pointer[Int, b], y: Pointer[Int, a]
    ) -> Int:
        return x[] + y[]


def main():
    var y = 3
    var b = B[ImmutAnyOrigin]()
    print(b.both(Pointer(to=y), Pointer(to=y)))
