# `SIMD.reduce` takes a reduction as a closure argument (`reduce(add)`) or a
# compile-time parameter (`reduce[addi]()`), and stops at `size_out` lanes;
# `reduce_add`/`mul`/`min`/`max` are that tree over their operator.
def add[width: SIMDLength](
    lhs: SIMD[DType.int32, width], rhs: SIMD[DType.int32, width]
) -> SIMD[DType.int32, width]:
    return lhs + rhs


def addi[width: Int](
    lhs: SIMD[DType.int32, width], rhs: SIMD[DType.int32, width]
) -> SIMD[DType.int32, width]:
    return lhs + rhs


def main():
    var v = SIMD[DType.int32, 4](1, 2, 3, 4)
    print(v.reduce(add), v.reduce[2](add))

    def mul[width: SIMDLength](
        lhs: SIMD[DType.int32, width], rhs: SIMD[DType.int32, width]
    ) -> SIMD[DType.int32, width]:
        return lhs * rhs

    print(v.reduce(mul))
    print(v.reduce[addi](), v.reduce[addi, 2]())
    var w = SIMD[DType.int32, 8](5, -3, 8, 1, 9, -7, 2, 4)
    print(w.reduce_add[2](), w.reduce_max[2](), w.reduce_min[4]())
    var u = SIMD[DType.uint8, 8](200, 3, 7, 250, 1, 9, 100, 2)
    print(u.reduce_add(), u.reduce_mul(), u.reduce_min(), u.reduce_max())
    comptime total = SIMD[DType.int32, 4](1, 2, 3, 4).reduce_add()
    print(total)
