# A struct's type and origin parameters read through a value: in a body
# (`s.origin`, `type_of(s).T`, `var z: s.T`) and in a signature over an
# auto-parameterized argument (`other.T`, `ImmOrigin(other.origin)`).
struct S[mut: Bool, //, T: AnyType, origin: Origin[mut=mut], *, k: Int = 1](
    ImplicitlyCopyable
):
    var a: Int

    def __init__(out self):
        self.a = 0


struct R[T: AnyType, *, k: Int = 1](ImplicitlyCopyable):
    var a: Int

    def __init__(out self):
        self.a = 0


def widen(other: R) -> R[other.T, k=other.k]:
    return R[other.T, k=other.k]()


def freeze(other: S) -> S[other.T, ImmOrigin(other.origin), k=other.k]:
    return S[other.T, ImmOrigin(other.origin), k=other.k]()


def main():
    var x = 5
    var s = S[Int, origin_of(x)]()
    print(s.mut, s.k)
    var o = s.origin
    _ = o
    comptime T = type_of(s).T
    var y: T = 3
    var z: s.T = 4
    print(y, z)
    var t = freeze(s)
    print(t.a, t.k)
    var r = widen(R[Int, k=3]())
    print(r.a, r.k)
