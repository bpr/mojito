def mixed(a: Int32, b: Int32) -> Int32:
    return a << 1 | b >> 1


def main():
    var a = Int32(6)
    var b = Int32(3)
    print(mixed(a, b))
    print(a | b & 1, a ^ b | 8, a & b ^ 5)
    print(1 + 2 << 3, 1 << 2 + 3, 5 & 3 + 4, 6 & 3 << 1)
    print(a - b & 2, 7 ^ 2 + 1 << 1)
    print(1 | 2 == 3, 4 & 6 != 0, 1 << 3 > 7)
    var n = 12
    print(n >> 1 << 2, n | 1 ^ 3 & 5, n ^ n >> 2)
