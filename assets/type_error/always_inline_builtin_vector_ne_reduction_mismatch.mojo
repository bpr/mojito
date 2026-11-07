# expect: type mismatch
# A vector `v != w` is `__ne__`'s `not self == rhs`; the pin does not equate
# it with any lane of `v.ne(w)`, as it does for one lane.


struct Tag[b: Bool](Movable):
    def __init__(out self):
        pass


@always_inline("builtin")
def any_lane(m: SIMD[DType.bool, 4]) -> Bool:
    return Bool(mlir_value=__mlir_op.`pop.simd.reduce_or`(m._mlir_value))


def f[v: SIMD[DType.int32, 4], w: SIMD[DType.int32, 4]]():
    var x: Tag[any_lane(v.ne(w))] = Tag[v != w]()
    _ = x^


def main():
    f[SIMD[DType.int32, 4](1), SIMD[DType.int32, 4](2)]()
