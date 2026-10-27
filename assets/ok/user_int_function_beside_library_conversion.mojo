# A program's own `def Int` shadows the builtin in its own module alone: the
# bundled library's conversions, which print an `Int` through, still cast.


def Int(x: Float64) -> Float64:
    return x + 1


def main():
    print(Int(Float64(2.0)))
    print(len([1, 2, 3]), UInt(7), Float64(Int8(-3)), Bool(UInt8(0)))
