# `pop.load` and `pop.store` move any register-passable element, not only a
# trivial one: the load is a bitwise copy that yields an owned value, and the
# store writes its operand's bits without destroying the element it
# overwrites, leaving the operand to be destroyed after the op. Upstream
# prints `3`, `del 3`, `del 5`, `5`, `4`, `del 4`, `del 7`, `7`, and `del 7`.
struct R(RegisterPassable, Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __deinit__(deinit self):
        print("del", self.v)


def load[T: RegisterPassable & Deinitable](p: Pointer[T, _]) -> T:
    return __mlir_op.`pop.load`[
        isVolatile=False.__mlir_i1__(),
        isInvariant=False.__mlir_i1__(),
        isNonTemporal=False.__mlir_i1__(),
    ](p._mlir_value)


def store[T: RegisterPassable & Deinitable](p: MutPointer[T, _], var value: T):
    __mlir_op.`pop.store`[
        isVolatile=False.__mlir_i1__(),
        isNonTemporal=False.__mlir_i1__(),
    ](value^, p._mlir_value)


def main():
    var r = R(3)
    var s = __mlir_op.`pop.load`[
        isVolatile=False.__mlir_i1__(),
        isInvariant=False.__mlir_i1__(),
        isNonTemporal=False.__mlir_i1__(),
    ](Pointer(to=r)._mlir_value)
    print(s.v)
    __mlir_op.`pop.store`[
        isVolatile=False.__mlir_i1__(),
        isNonTemporal=False.__mlir_i1__(),
    ](R(5), Pointer(to=r)._mlir_value)
    print(r.v)
    var t = load(Pointer(to=r))
    t.v = 4
    print(t.v)
    store(Pointer(to=r), R(7))
    print(r.v)
