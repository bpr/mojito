# A def may take a [dtype: DType] value parameter and use it in
# Scalar[...]/SIMD[...] positions: each call monomorphizes, so the body
# checks and executes at the concrete dtype (wrapping bit-accurately).
def convert[dt: DType](x: Int) -> Scalar[dt]:
    return Scalar[dt](x)

def splat_two[dt: DType](x: Int) -> SIMD[dt, 2]:
    return SIMD[dt, 2](x)

def main():
    print(convert[DType.uint8](300))
    print(convert[DType.float32](3))
    print(splat_two[DType.int16](70000))
    var b: Byte = convert[DType.uint8](261)
    print(b)
