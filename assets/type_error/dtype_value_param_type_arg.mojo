# expect: 'convert' parameter 'dt' has 'DType' type, but value has type 'AnyStruct[Int]'
# A [dtype: DType] value parameter takes a DType.<dt> value, not a type.
def convert[dt: DType](x: Int) -> Scalar[dt]:
    return Scalar[dt](x)

def main():
    print(convert[Int](3))
