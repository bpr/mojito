# expect: constraint failed: mismatch in the number of elements
# A shuffle mask has one index per receiver lane, since `shuffle` returns
# `Self`: `_shuffle_variadic` builds the mask's `StaticTuple` at the
# receiver's width, whose initializer asserts the count, as at the pin.
# Widening is `join`.
def main():
    var v = SIMD[DType.int32, 2](10, 20)
    print(v.shuffle[0, 1, 1, 0]())
