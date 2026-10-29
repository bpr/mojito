# expect: no constructor overload matches the supplied arguments
# `fill=` is the initializer of a `DType.bool` mask only: its `out self`
# refines the dtype to `bool`.
def main():
    print(SIMD[DType.int32, 4](fill=True))
