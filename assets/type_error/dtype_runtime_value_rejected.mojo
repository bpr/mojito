# expect: Undefined variable 'DType'
# DType.<dt> is a compile-time spelling valid only inside SIMD/Scalar
# brackets and DType value-parameter arguments — never a runtime value.
def main():
    var x = DType.float32
    print(x)
