# `T()` over a parameter bound to a SIMD type is the zero vector: `Array`'s
# default constructor fills its elements with it.
from std.ffi import c_char


def main():
    var buffer = Array[c_char, 4]()
    print(buffer[0], buffer[3])
    var wide = Array[SIMD[DType.float32, 4], 2]()
    print(wide[1])
    var flags = Array[SIMD[DType.bool, 2], 1]()
    print(flags[0])
