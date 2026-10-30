# `pop.array.replace`'s value is of the array's element type, so a
# `Float64` into an array of `Int` is the pin's verifier error.
# expect: invalid call to 'pop.array.replace': op failed to verify that value type matches array element type
def main():
    var a = __mlir_op.`pop.array.repeat`[
        _type=__mlir_type[`!pop.array<3, `, Int, `>`]
    ](Int(7))
    var b = __mlir_op.`pop.array.replace`[
        _type=__mlir_type[`!pop.array<3, `, Int, `>`],
        index=(1).__mlir_index__(),
    ](Float64(9), a)
    print(__mlir_op.`pop.array.get`[_type=Int, index=(0).__mlir_index__()](b))
