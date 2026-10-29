# Upstream's storage of a `Bool`: its `!kgen.scalar<bool>` `_mlir_value`,
# the MLIR builtin `__mlir_type.i1` that `__mlir_i1__` gives and the
# `@implicit` initializer takes, the `pop.cast_from_builtin` and
# `pop.cast_to_builtin` casts between them, and typed scalar attribute
# constants, at run time and in compile-time positions.


def take(x: __mlir_type.i1) -> Bool:
    return Bool(x)


def implicit(x: __mlir_type.i1) -> Bool:
    return x


def invert(b: Bool) -> Bool:
    return Bool(
        mlir_value=__mlir_op.`pop.simd.xor`(
            b._mlir_value,
            __mlir_attr.`#kgen.simd<true> : !kgen.scalar<bool>`,
        )
    )


def round_trip(b: Bool) -> Bool:
    var i = __mlir_op.`pop.cast_to_builtin`[_type=__mlir_type.i1](
        b._mlir_value
    )
    var s = __mlir_op.`pop.cast_from_builtin`[
        _type=__mlir_type.`!kgen.scalar<bool>`
    ](i)
    return Bool(mlir_value=s)


def main():
    var t = True
    var f = False
    print(take(t.__mlir_i1__()), implicit(f.__mlir_i1__()))
    print(Bool(value=t.__mlir_i1__()), Bool(value=f))
    print(invert(t), invert(f))
    print(round_trip(t), round_trip(f))
    print(
        Bool(mlir_value=__mlir_op.`pop.simd.and`(t._mlir_value, f._mlir_value)),
        Bool(mlir_value=__mlir_op.`pop.simd.or`(t._mlir_value, f._mlir_value)),
    )
    comptime e = __mlir_attr.`#kgen.simd<false> : !kgen.scalar<bool>`
    comptime g = __mlir_attr.`#kgen.simd<3> : !kgen.scalar<si32>`
    print(Scalar[DType.bool](mlir_value=e), Int32(mlir_value=g))
    comptime c = invert(True)
    comptime r = round_trip(False)
    print(c, r)
