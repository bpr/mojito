# expect: type mismatch for variable 'v'
# A scalar splats only into a vector of its own dtype: `Int32` does not
# convert to an `int64` lane first.
def main():
    var x = Int32(1)
    var v: SIMD[DType.int64, 4] = x
    print(v)
