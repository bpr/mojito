# A mask splats one `Bool` with the `fill` keyword: `Bool` is not a `Scalar`,
# so neither compiler takes the positional splat `SIMD[DType.bool, 4](True)`.
def main():
    print(SIMD[DType.bool, 4](fill=True))
    var off = False
    print(SIMD[DType.bool, 2](fill=off), SIMD[DType.bool, 1](fill=True))
