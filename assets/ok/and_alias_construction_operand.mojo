# A construction spelled through an alias, as `and`'s left operand, is
# tested through its `Bool(x)` like any other operand.
comptime M4 = SIMD[DType.bool, 4]


struct W[w: Int]:
    comptime _Mask = SIMD[DType.bool, Self.w]

    @staticmethod
    def k(rhs: SIMD[DType.int32, Self.w]) -> SIMD[DType.bool, Self.w]:
        return Self._Mask(fill=True) and rhs.eq(0)


def g[w: Int](rhs: SIMD[DType.int32, w]) -> SIMD[DType.bool, w]:
    comptime M = SIMD[DType.bool, w]
    return M(fill=False) or rhs.eq(0)


def main():
    var v = SIMD[DType.int32, 4](2, 0, 3, 0)
    print(M4(fill=True) and v.eq(0))
    print(W[4].k(v), g[4](v))
