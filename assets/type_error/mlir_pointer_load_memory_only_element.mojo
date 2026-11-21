# `pop.load` yields only a register-passable value: the pin rejects a load
# of a memory-only struct.
# expect: invalid call to 'pop.load': 'M' cannot be returned directly from __mlir_op as it is not a 'RegisterPassable' type
struct M(Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v


def main():
    var m = M(3)
    var s = __mlir_op.`pop.load`[
        isVolatile=False.__mlir_i1__(),
        isInvariant=False.__mlir_i1__(),
        isNonTemporal=False.__mlir_i1__(),
    ](Pointer(to=m)._mlir_value)
    print(s.v)
