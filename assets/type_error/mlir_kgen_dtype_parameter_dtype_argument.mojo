# expect: type mismatch for value parameter 'd': expected __mlir_type.`!kgen.dtype`, found DType
# A `DType` is not its `!kgen.dtype` storage: binding one to a parameter of
# that type is rejected, as at the pin.
def f[d: __mlir_type.`!kgen.dtype`]() -> Int:
    return 1


def main():
    print(f[DType.int64]())
