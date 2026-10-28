# expect: an element of 'values' with type 'SIMDLength' does not conform to trait 'Writable'
# `v.length` is SIMD's `SIMDLength` parameter, which is not `Writable`.
def main():
    var v = SIMD[DType.int8, 4](1)
    print(v.length)
