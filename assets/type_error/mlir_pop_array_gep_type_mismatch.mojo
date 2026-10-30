# A `_type` on `pop.array.gep` must be the `!kgen.pointer` to the array's
# element type.
# expect: invalid call to 'pop.array.gep': op failed to verify that pointer to array element type
def main():
    var a = __mlir_op.`pop.array.repeat`[
        _type=__mlir_type[`!pop.array<2, `, Int, `>`]
    ](Int(1))
    var p = __mlir_op.`pop.array.gep`[
        _type=__mlir_type[`!kgen.pointer<`, Float64, `>`]
    ](Pointer(to=a)._get_kgen_pointer(), Int(1).__mlir_index__())
