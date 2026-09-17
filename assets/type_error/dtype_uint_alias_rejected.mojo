# expect: DType.uint is not supported yet
# `UInt.dtype` is upstream's `DType.uint`, which has no Mojito dtype.
def main():
    print(UInt.dtype)
