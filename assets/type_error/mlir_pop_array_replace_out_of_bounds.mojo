# `pop.array.replace`'s index is below the array's size, so element 5 of a
# three-element array is the pin's verifier error.
# expect: invalid call to 'pop.array.replace': op array index out of bounds: 5
def main():
    var a = __mlir_op.`pop.array.repeat`[
        _type=__mlir_type[`!pop.array<3, `, Int, `>`]
    ](Int(7))
    var b = __mlir_op.`pop.array.replace`[
        _type=__mlir_type[`!pop.array<3, `, Int, `>`],
        index=(5).__mlir_index__(),
    ](Int(9), a)
    print(__mlir_op.`pop.array.get`[_type=Int, index=(0).__mlir_index__()](b))
