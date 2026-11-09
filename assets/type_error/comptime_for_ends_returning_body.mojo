# A value-returning body that ends in a `comptime for` is rejected even when
# every iteration returns: the body is judged before unrolling, and the loop
# may unroll zero times. The pin reports "return expected at end of function
# with results".
# expect: does not return a value on every path
def first() -> Int:
    comptime for i in range(3):
        return i


def main():
    print(first())
