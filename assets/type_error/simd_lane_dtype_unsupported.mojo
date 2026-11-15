# expect: SIMD lanes of DType.bfloat16
# A `DType.bfloat16` value is supported, but no `SIMD` lane of it is yet:
# the instance holding one is refused where it is elaborated. The pin runs
# it.
def main():
    print(SIMD[DType.bfloat16, 2](1.5))
