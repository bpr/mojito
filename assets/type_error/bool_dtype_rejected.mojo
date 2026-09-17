# expect: has no field 'dtype'
# `Bool` is not a `SIMD` type, so it has no `dtype`.
def main():
    var b = True
    print(b.dtype)
