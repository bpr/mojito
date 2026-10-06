# A type or a parameter argument cannot call a raising function.

def r(n: Int) raises -> Int:
    if n > 100:
        raise Error("big")
    return n * 2

def gen[n: Int]() -> SIMD[DType.int32, r(n)]:
    return SIMD[DType.int32, 2](7)

def main():
    print(gen[1]())
