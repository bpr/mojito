# expect: function instantiation of `widen` failed: constraint failed: must be a scalar; use the `fill` keyword instead for explicit splatting
# `Bool` converts to `SIMD[DType.bool, n]` through upstream's implicit
# constructor, which asserts one lane: `widen[1]` holds it, `widen[4]`
# fails it.

def widen[width: Int](b: Bool) -> SIMD[DType.bool, width]:
    return b


def main():
    print(widen[1](True))
    print(widen[4](True))
