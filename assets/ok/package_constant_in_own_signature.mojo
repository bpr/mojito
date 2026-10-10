# An importer's own signature spells a package integer constant, and its own
# code reads a package constant, a package type alias, and its own overloads.
from std.io.file import O_WRONLY, O_CREAT
from std.ffi import c_int

comptime WIDTH = O_CREAT // 16


def widen(v: SIMD[DType.uint8, WIDTH]) -> Int:
    return Int(v[0]) + WIDTH


def pick(x: Int) -> Int:
    return x + 1


def pick(x: String) -> Int:
    return x.byte_length()


def main():
    print(widen(SIMD[DType.uint8, WIDTH](3)))
    print(O_WRONLY | O_CREAT)
    var c: c_int = c_int(7)
    print(c)
    print(pick(1), pick("abc"))
