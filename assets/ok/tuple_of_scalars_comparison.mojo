# SIMD scalars conform to `Equatable` and `Comparable`, so a tuple of them
# compares and a bounded generic accepts one; a scalar comparison converts
# implicitly to the `Bool` a return, an argument, or `not` wants.


def same[T: Equatable](x: T, y: T) -> Bool:
    return x == y


def less[T: Comparable](x: T, y: T) -> Bool:
    return x < y


def main():
    var a = (UInt64(1), UInt64(2))
    var b = (UInt64(1), UInt64(3))
    print(a < b, a == b, a != b, a <= b, a > b, a >= b)
    var c = (UInt8(1), Float32(2.5))
    print(c == c, c < (UInt8(1), Float32(3.5)))
    print(same(UInt64(1), UInt64(1)), less(Float32(2.5), Float32(1.5)))
    var flag: Bool = Int32(4) > Int32(3)
    print(flag, not (UInt8(1) == UInt8(2)))
    var xs: List[UInt8] = [3, 1, 2]
    var ys: List[UInt8] = [3, 1, 2]
    print(xs == ys, UInt8(2) in xs)
