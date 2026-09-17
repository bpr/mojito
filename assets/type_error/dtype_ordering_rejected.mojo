# expect: operator '<' is not defined for DType and DType
# A `DType` is only `Equatable`: it has no ordering.
def main():
    var d = DType.int
    print(d < DType.float32)
