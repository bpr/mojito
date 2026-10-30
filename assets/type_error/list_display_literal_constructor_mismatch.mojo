# expect: no constructor overload matches
# A list display at `SIMD` is its `__list_literal__` construction, whose
# overload resolution rejects elements no lane scalar converts from.
def main():
    var v: SIMD[DType.int32, 2] = ["a", "b"]
    print(v)
