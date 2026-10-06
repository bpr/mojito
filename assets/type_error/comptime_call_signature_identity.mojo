# A signature's application of a function to a parameter is its own value
# at a call: `make[2]()` returns a vector of `h(2)` lanes, which is not the
# `4` the function computes. The pin keeps the call symbolic through the
# check, so the two widths differ.

def h(n: Int) -> Int:
    return n * 2

def make[n: Int]() -> SIMD[DType.int32, h(n)]:
    return SIMD[DType.int32, h(n)](7)

def main():
    var v: SIMD[DType.int32, 4] = make[2]()
    print(v)
