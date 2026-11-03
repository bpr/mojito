# An `IntLiteral` converts into a `bool` lane through upstream's `@implicit
# SIMD.__init__(IntLiteral)`, as at the pin.
def main():
    var t: Scalar[DType.bool] = 1
    var f: Scalar[DType.bool] = 0
    print(t, f)
