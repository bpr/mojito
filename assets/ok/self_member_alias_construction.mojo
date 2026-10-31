# A call of a struct's own parameterized type member through `Self`
# constructs the type the member names: a partially bound member and one
# declared over its own parameter.
struct M[dtype: DType, width: Int]:
    comptime _T = SIMD[Self.dtype, _]
    comptime _U[w: Int] = SIMD[Self.dtype, w]
    var v: SIMD[Self.dtype, Self.width]
    var u: SIMD[Self.dtype, Self.width]

    def __init__(out self):
        self.v = Self._T[Self.width](1)
        self.u = Self._U[Self.width](2)


def main():
    var m = M[DType.int32, 4]()
    print(m.v)
    print(m.u)
