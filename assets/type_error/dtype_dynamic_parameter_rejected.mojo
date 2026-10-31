# expect: cannot use a dynamic value in a parameter list
# A runtime `DType` value cannot key a `SIMD` type: parameter lists take
# compile-time values only.
def main():
    var y = DType.int8
    var v = SIMD[y, 1](3)
    print(v)
