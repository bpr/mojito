# `UInt`'s `//` by a run-time zero is `SIMD.__floordiv__`, which masks the
# zero divisor and yields 0 rather than trapping.

def divisor(n: Int) -> Int:
    return n - n

def main():
    var x = UInt(10) // UInt(divisor(3))
    print(x)
