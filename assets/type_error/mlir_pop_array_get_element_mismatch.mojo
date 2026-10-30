# `pop.array.get`'s `_type` is the array's element type, so reading an
# array of `Int` as `Float64` is the pin's verifier error.
# expect: invalid call to 'pop.array.get': op failed to verify that value type matches array element type
def main():
    var a = __mlir_op.`pop.array.repeat`[
        _type=__mlir_type[`!pop.array<3, `, Int, `>`]
    ](Int(7))
    print(
        __mlir_op.`pop.array.get`[_type=Float64, index=(0).__mlir_index__()](a)
    )
