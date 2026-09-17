# expect: operator 'is' is not defined for DType and DType
# A `DType` has no identity comparison.
def main():
    var d = DType.int
    print(d is DType.float64)
