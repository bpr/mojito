# expect: does not match the signature required by trait 'Reads'
# A requirement generic over an origin's `mut=` is not met by a witness
# fixing it immutable: the own `mut=` binder is part of the signature.
struct V[mut: Bool, //, T: Copyable, origin: Origin[mut=mut]]:
    var n: Int

    def __init__(out self, n: Int):
        self.n = n


trait Reads:
    def read[m: Bool, //, o: Origin[mut=m]](self, v: V[Int, o]) -> Int:
        ...


struct C(Reads):
    def __init__(out self):
        pass

    def read[o: ImmOrigin](self, v: V[Int, o]) -> Int:
        return v.n


def main():
    var v = V[Int, ImmutAnyOrigin](3)
    print(C().read(v))
