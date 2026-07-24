def main():
    comptime huge = 2 ** 200
    var reduced: Int = (huge + 1) - huge
    var wrapped: Int = huge + 7
    var byte: UInt8 = 256 + 255
    var exact: Float64 = 3.0 * (4.0 / 3.0 - 1.0)
    print(reduced, wrapped, byte, exact)
