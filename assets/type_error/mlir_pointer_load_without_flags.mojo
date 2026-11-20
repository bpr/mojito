# `pop.load` holds its `isInvariant`, `isNonTemporal`, and `isVolatile`
# flags as required attributes; the pin's verifier rejects a load without
# them.
# expect: invalid call to 'pop.load': op requires attribute 'isInvariant'
def main():
    var x = 4
    var p = Pointer(to=x)
    var v = __mlir_op.`pop.load`(p._mlir_value)
    print(v)
