# A field type or a module alias applying a callable whose `where` clause
# reads a generic comptime alias checks the clause before the callee's
# signature registers: the field types and aliases resolve first, as the
# pin resolves the callee's signature on demand.
comptime N = 2
comptime IsN[n: Int] = n == N
comptime W = SIMD[DType.int32, g[2]()]


def g[n: Int]() -> Int where IsN[n]:
    return n


struct S[k: Int]:
    @staticmethod
    def get() -> Int where IsN[Self.k]:
        return Self.k


struct F:
    var v: SIMD[DType.int32, S[2].get()]
    var w: SIMD[DType.int32, g[2]()]

    def __init__(out self):
        self.v = SIMD[DType.int32, S[2].get()](7)
        self.w = SIMD[DType.int32, g[2]()](8)


def main():
    var f = F()
    print(f.v[1], f.w[0])
    print(W(4)[1])
