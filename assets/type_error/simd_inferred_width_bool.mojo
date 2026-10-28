# expect: requires an integral dtype
# A `Bool` is no `SIMD` value, so it binds `_` through upstream's
# `__init__[T: Intable, target_dtype]`, whose assert wants an integral dtype.
def main():
    var b = True
    print(SIMD[DType.bool, _](b))
