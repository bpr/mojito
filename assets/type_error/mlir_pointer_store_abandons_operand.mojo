# `pop.store` copies its operand's bits and leaves the owned operand to be
# destroyed after the op, so an operand the body cannot destroy is
# abandoned, as the pin reports it.
# expect: '(anonymous value)' abandoned without being explicitly destroyed: unhandled explicitly destroyed type 'RegisterPassable'
def store[T: RegisterPassable](p: MutPointer[T, _], var value: T):
    __mlir_op.`pop.store`[
        isVolatile=False.__mlir_i1__(),
        isNonTemporal=False.__mlir_i1__(),
    ](value^, p._mlir_value)


def main():
    var x = 1
    store(Pointer(to=x), 2)
    print(x)
