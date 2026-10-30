# A `pop.array` op over an element type parameter whose bounds do not imply
# `RegisterPassable` cannot load the element into a register.
# expect: cannot load non-register passable type into SSA register
struct A[T: ImplicitlyCopyable & Deinitable, n: Int]:
    comptime _mlir_type = __mlir_type[
        `!pop.array<`, Self.n.__mlir_index__(), `, `, Self.T, `>`
    ]
    var a: Self._mlir_type

    def __init__(out self, fill: Self.T):
        self.a = __mlir_op.`pop.array.repeat`[_type=Self._mlir_type](fill)

    def first(self) -> Self.T:
        return __mlir_op.`pop.array.get`[_type=Self.T, index=(0).__mlir_index__()](self.a)


def main():
    var x = A[Int, 2](3)
    print(x.first())
