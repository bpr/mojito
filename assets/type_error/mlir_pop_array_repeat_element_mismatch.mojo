# `pop.array.repeat`'s value is of the array's element type, so a `Float64`
# into an array of `Int` is the pin's verifier error.
# expect: invalid call to 'pop.array.repeat': op failed to verify that value type matches array element type
def main():
    var a = __mlir_op.`pop.array.repeat`[
        _type=__mlir_type[`!pop.array<3, `, Int, `>`]
    ](Float64(7))
    print(__mlir_op.`pop.array.get`[_type=Int, index=(0).__mlir_index__()](a))
