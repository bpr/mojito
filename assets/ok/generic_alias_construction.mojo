# A call of a generic comptime alias's application constructs the type it
# names, as does a static call through it: an alias over its own parameters,
# a type pack, a value pack, a partially bound alias, and a closed alias of
# another alias's application.
@fieldwise_init
struct Pt[T: AnyType](Copyable):
    var x: Int


struct W[k: Int](ImplicitlyCopyable):
    def __init__(out self):
        pass

    @staticmethod
    def get() -> Int:
        return Self.k


struct D[dt: DType, k: Int]:
    @staticmethod
    def get() -> Int:
        return Self.k


comptime P[T: AnyType] = Pt[T]
comptime T2[*Ts: Copyable] = Tuple[*Ts]
comptime TT = T2[Int, Bool]
comptime V[*vs: Int] = ParameterList[vs.values]
comptime A[k: Int] = W[k]
comptime Bytes = SIMD[DType.uint8, _]
comptime Y = D[DType.int8, _]
comptime Q = A[6]


def main():
    var p = P[Int](8)
    print(p.x)
    var t = T2[Int, Bool](1, True)
    print(t[0], t[1])
    var u = TT(2, False)
    print(u[0], u[1])
    comptime for v in V[1, 2, 3]():
        print(v, end=" ")
    print()
    comptime a = A[4]()
    print(a.get())
    var b = Bytes[4](1, 2, 3, 4)
    print(b)
    print(Y[5].get())
    print(A[7].get())
    print(Q.get())
