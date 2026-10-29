# expect: constraint failed: must be a scalar; use the `fill` keyword instead for explicit splatting
# The `Bool` initializer does not splat: its `comptime assert Self.length
# == 1` rejects a multi-lane mask, and `SIMD[DType.bool, 4](fill=True)` is
# the spelling.
def main():
    print(SIMD[DType.bool, 4](True))
