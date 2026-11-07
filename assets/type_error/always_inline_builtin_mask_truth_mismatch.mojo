# expect: type mismatch
# A one-lane `v < w` is `__lt__`'s `self.lt(rhs).__bool__()`, whose `!= 0`
# over the mask the pin keeps, so it is not the bare reduction of `v.lt(w)`.


struct Tag[b: Bool](Movable):
    def __init__(out self):
        pass


@always_inline("builtin")
def any_lane(m: Scalar[DType.bool]) -> Bool:
    return Bool(mlir_value=__mlir_op.`pop.simd.reduce_or`(m._mlir_value))


def f[v: Scalar[DType.int32], w: Scalar[DType.int32]]():
    var x: Tag[any_lane(v.lt(w))] = Tag[v < w]()
    _ = x^


def main():
    f[Scalar[DType.int32](1), Scalar[DType.int32](2)]()
