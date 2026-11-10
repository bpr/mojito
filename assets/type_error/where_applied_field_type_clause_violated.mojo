# expect: invalid call to 'g': violated constraint
# A field type applying a callable whose `where` clause its arguments
# violate is rejected, though the field type resolves before the callee's
# signature registers.
comptime N = 2
comptime IsN[n: Int] = n == N


def g[n: Int]() -> Int where IsN[n]:
    return n


struct F:
    var w: SIMD[DType.int32, g[3]()]

    def __init__(out self):
        self.w = SIMD[DType.int32, g[3]()](8)


def main():
    print(F().w[0])
