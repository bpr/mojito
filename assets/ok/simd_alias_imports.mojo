# `Scalar`, `Byte`, and the sized aliases are `std.simd`'s `comptime`
# aliases, importable under a name of the program's own.
from std.simd import Byte as B, Int32 as I32, Scalar as S


def main():
    var x: I32 = 7
    var y: S[DType.int64] = 9
    var b: B = 255
    print(x, y, b)
