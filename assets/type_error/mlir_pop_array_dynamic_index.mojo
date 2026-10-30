# A `pop.array` op's `index` is an attribute, so a run-time `Int`'s
# `__mlir_index__()` is the pin's dynamic-operand error.
# expect: cannot use a dynamic value in MLIR magic
def main():
    var a = __mlir_op.`pop.array.repeat`[
        _type=__mlir_type[`!pop.array<3, `, Int, `>`]
    ](Int(7))
    var i = 1
    print(__mlir_op.`pop.array.get`[_type=Int, index=i.__mlir_index__()](a))
