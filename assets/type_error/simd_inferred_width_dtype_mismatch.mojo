# expect: type mismatch for variable 'v'
# The `_` hole solves only the width: the element type must still match.
def main():
    var v: SIMD[DType.int32, _] = SIMD[DType.int64, 4](1, 2, 3, 4)
    print(v)
