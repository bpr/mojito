# An element of a local `comptime` display binding is its own parameter
# expression in a type: `vals[0] * 2` is not `vals[1]`, whatever the display
# spells, so the two vector types differ.

def widths[n: Int]():
    comptime vals = [n, n * 2]
    var a: SIMD[DType.int32, vals[0] * 2] = SIMD[DType.int32, vals[1]](1)
    print(a)

def main():
    widths[2]()
