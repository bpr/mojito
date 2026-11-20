# `pop.store` writes a value of the element its pointer addresses; the pin's
# verifier rejects a literal stored through an `Int` pointer.
# expect: invalid call to 'pop.store': operand #0 of type 'IntLiteral[12]' is not the element of
def main():
    var y = 5
    var p = Pointer(to=y)
    __mlir_op.`pop.store`[
        isVolatile=False.__mlir_i1__(), isNonTemporal=False.__mlir_i1__()
    ](12, p._mlir_value)
    print(y)
