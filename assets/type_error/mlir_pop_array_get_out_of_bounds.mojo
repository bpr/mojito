# `pop.array.get`'s index is below the array's size, so element 3 of a
# three-element array is the pin's verifier error.
# expect: invalid call to 'pop.array.get': op array index out of bounds: 3
def main():
    var a = __mlir_op.`pop.array.repeat`[
        _type=__mlir_type[`!pop.array<3, `, Int, `>`]
    ](Int(7))
    print(__mlir_op.`pop.array.get`[_type=Int, index=(3).__mlir_index__()](a))
