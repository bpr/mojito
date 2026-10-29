# expect: must be a scalar; use the `fill` keyword
# A `Bool` into a multi-lane integer vector converts through the implicit
# `Bool` initializer into a mask, whose `comptime assert Self.length == 1`
# fails before the cast.
def main():
    print(SIMD[DType.int8, 4](True))
