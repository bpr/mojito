# expect: splats one Bool only as 'fill='
# `Bool` is not a `Scalar`, so a multi-lane mask does not splat a positional
# `Bool`; `SIMD[DType.bool, 4](fill=True)` is the spelling.
def main():
    print(SIMD[DType.bool, 4](True))
