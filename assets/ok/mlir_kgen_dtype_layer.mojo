# Upstream's layer under `DType`: the `!kgen.dtype` type, its
# `#kgen.dtype.constant<…>` attributes, the builtin `ui8`, and the
# `pop.dtype.to_ui8` and `pop.dtype.from_ui8` ops between a dtype and its
# one-byte code, with `pop.cast_from_builtin` and `pop.cast_to_builtin`
# between `ui8` and a `UInt8`'s `!kgen.scalar<ui8>` storage, at run time,
# in a `comptime` value, and through `!kgen.dtype` parameters.


def code(d: __mlir_type.`!kgen.dtype`) -> UInt8:
    return UInt8(
        mlir_value=__mlir_op.`pop.cast_from_builtin`[
            _type=__mlir_type.`!kgen.scalar<ui8>`
        ](__mlir_op.`pop.dtype.to_ui8`(d))
    )


def from_code(c: UInt8) -> __mlir_type.`!kgen.dtype`:
    return __mlir_op.`pop.dtype.from_ui8`(
        __mlir_op.`pop.cast_to_builtin`[_type=__mlir_type.ui8](c._mlir_value)
    )


struct S[d: __mlir_type.`!kgen.dtype`]:
    @staticmethod
    def show():
        print(code(Self.d))


def f[d: __mlir_type.`!kgen.dtype`]() -> UInt8:
    return code(d)


def main():
    comptime a = __mlir_attr.`#kgen.dtype.constant<si64> : !kgen.dtype`
    var x: __mlir_type.`!kgen.dtype` = a
    print(code(x))
    print(code(__mlir_attr.`#kgen.dtype.constant<f32> : !kgen.dtype`))
    print(code(__mlir_attr.`#kgen.dtype.constant<bool> : !kgen.dtype`))
    print(code(__mlir_attr.`#kgen.dtype.constant<index> : !kgen.dtype`))
    var y = from_code(UInt8(136))
    print(code(y))
    var u = UInt8(141)
    var b = __mlir_op.`pop.cast_to_builtin`[_type=__mlir_type.ui8](
        u._mlir_value
    )
    print(
        UInt8(
            mlir_value=__mlir_op.`pop.cast_from_builtin`[
                _type=__mlir_type.`!kgen.scalar<ui8>`
            ](b)
        )
    )
    comptime c = code(__mlir_attr.`#kgen.dtype.constant<f64> : !kgen.dtype`)
    print(c)
    S[__mlir_attr.`#kgen.dtype.constant<ui16> : !kgen.dtype`].show()
    print(f[__mlir_attr.`#kgen.dtype.constant<si8> : !kgen.dtype`]())
