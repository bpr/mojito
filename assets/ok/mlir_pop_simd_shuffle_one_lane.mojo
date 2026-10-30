# A one-lane `pop.simd.shuffle` over two operands is a scalar: its mask's
# one index picks a lane of the first operand, or, past that operand's
# width, of the second. `_shuffle_list` reaches it for `Int(5).shuffle[0]()`.


def first(a: Int32, b: Int32) -> Int32:
    return Int32(
        mlir_value=__mlir_op.`pop.simd.shuffle`[
            mask=__mlir_attr.`#pop.array<0> : !pop.array<1, index>`,
            _type=Int32._mlir_type,
        ](a._mlir_value, b._mlir_value)
    )


def second(a: Int32, b: Int32) -> Int32:
    return Int32(
        mlir_value=__mlir_op.`pop.simd.shuffle`[
            mask=__mlir_attr.`#pop.array<1> : !pop.array<1, index>`,
            _type=Int32._mlir_type,
        ](a._mlir_value, b._mlir_value)
    )


def main():
    print(first(5, 6))
    print(second(5, 6))
    print(Int(5).shuffle[0]())
    print(Float64(2.5).shuffle[1](Float64(4.5)))
