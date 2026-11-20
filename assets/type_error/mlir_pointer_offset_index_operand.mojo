# `pop.offset` counts its elements in a `!kgen.scalar<index>`, an `Int`'s
# `_mlir_value`; the pin's verifier rejects a bare `index`.
# expect: invalid call to 'pop.offset': operand #1 must be __mlir_type.`!kgen.scalar<index>`, but got '__mlir_type.index'
def main():
    var x = 4
    var p = Pointer(to=x)
    var q = __mlir_op.`pop.offset`(p._mlir_value, Int(0).__mlir_index__())
    print(Pointer[Int, origin_of(x)](_mlir_value=q)[])
