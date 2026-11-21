# A typed `#lit.struct<{field = v}> : T` attribute is the value of the
# struct `T` with each field set from its closed attribute, as upstream's
# `AddressSpace.GENERIC` and `Scalar`'s length spell a `SIMDLength`: at
# module scope, in a struct's `comptime` member, in a local `comptime`, and
# as a run-time argument.
from std.memory import AddressSpace


@fieldwise_init
struct Lane(ImplicitlyCopyable):
    var _value: SIMDLength

    comptime ONE = Lane(
        __mlir_attr[`#lit.struct<{_mlir_value = 1}> : `, SIMDLength]
    )


comptime FOUR = __mlir_attr[`#lit.struct<{_mlir_value = 4}> : `, SIMDLength]

comptime Quad = SIMD[DType.int32, FOUR]


def main():
    comptime three = Lane(
        __mlir_attr[`#lit.struct<{_mlir_value = 3}> : `, SIMDLength]
    )
    var five = Lane(
        __mlir_attr[`#lit.struct<{_mlir_value = 5}> : `, SIMDLength]
    )
    print(Int(Lane.ONE._value), Int(three._value), Int(five._value))
    var q = Quad(7)
    print(q, Int(FOUR))
    var s: Scalar[DType.int32] = 2
    var t: SIMD[DType.int32, 1] = s
    print(t)
    print(AddressSpace.GENERIC == AddressSpace(0))
    print(AddressSpace.GLOBAL == AddressSpace(1))
