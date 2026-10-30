# `pop.array.gep` takes the array's pointer and the element's index.
# expect: invalid call to 'pop.array.gep': op expected 2 operands, but found 1
def main():
    var a = __mlir_op.`pop.array.repeat`[
        _type=__mlir_type[`!pop.array<2, `, Int, `>`]
    ](Int(1))
    var p = __mlir_op.`pop.array.gep`(Pointer(to=a)._get_kgen_pointer())
