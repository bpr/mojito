# `pop.array.replace` keeps its array's type, so a `_type` of another size
# is the pin's verifier error.
# expect: invalid call to 'pop.array.replace': op failed to verify that all of {array, result} have same type
def main():
    var a = __mlir_op.`pop.array.repeat`[
        _type=__mlir_type[`!pop.array<3, `, Int, `>`]
    ](Int(7))
    var b = __mlir_op.`pop.array.replace`[
        _type=__mlir_type[`!pop.array<2, `, Int, `>`],
        index=(1).__mlir_index__(),
    ](Int(9), a)
    print(__mlir_op.`pop.array.get`[_type=Int, index=(0).__mlir_index__()](b))
