# An annotated module constant that applies a callable waits for its first
# demand, as an unannotated one does: a body's read is a request the
# elaborator below MIR serves. A constant nothing reads is never evaluated,
# and its name does not reach a body where a local of that name shadows it.
def f(x: Int) -> Int:
    return x + 1

comptime C: Int = f(1)
comptime E: Int = f(3) + C
comptime p = f(9)

def main():
    print(C, E)
    comptime y: Int = f(5)
    print(y)
    var a: SIMD[DType.int32, 1] = 0
    print(a)
