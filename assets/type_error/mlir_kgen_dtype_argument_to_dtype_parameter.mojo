# expect: type mismatch for value parameter 'd': expected DType, found __mlir_type.`!kgen.dtype`
# A `#kgen.dtype.constant` is the `!kgen.dtype` a `DType` stores, not a
# `DType`: binding one to a `DType` parameter is rejected, as at the pin.
def g[d: DType]() -> Int:
    return 1


def main():
    print(g[__mlir_attr.`#kgen.dtype.constant<si64> : !kgen.dtype`]())
