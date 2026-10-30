# `pop.array.gep` infers its result from a `!kgen.pointer` to a
# `!pop.array`, so one to an `Int` leaves the pin no result type.
# expect: invalid call to 'pop.array.gep': unable to infer result type from MLIR operation 'pop.array.gep'
def main():
    var x = 5
    var p = __mlir_op.`pop.array.gep`(
        Pointer(to=x)._get_kgen_pointer(), Int(0).__mlir_index__()
    )
