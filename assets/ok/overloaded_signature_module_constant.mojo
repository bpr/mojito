# An overloaded function's and method's signatures spell module integer
# constants derived from a package constant: each symbol reads the value the
# check folded once.

from std.io.file import O_CREAT

comptime W = O_CREAT // 16
comptime W2 = W * 2

def f(x: SIMD[DType.uint8, W]) -> Int:
    return 1

def f(x: Int) -> Int:
    return 2

@fieldwise_init
struct S:
    def g(self, x: SIMD[DType.uint8, W2]) -> Int:
        return 3

    def g(self, x: Int) -> Int:
        return 4

def main():
    var v = SIMD[DType.uint8, W](1)
    var w = SIMD[DType.uint8, W2](1)
    var s = S()
    print(W, W2, f(v), f(3), s.g(w), s.g(1))
