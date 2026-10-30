# expect: no constructor overload matches
# No `SIMD.__init__` overload binds an unbound `_` width from the number of
# element arguments; only a single argument's own width binds it.
def main():
    var v: SIMD[DType.int, 4] = SIMD[DType.int, _](1, 2, 3, 4)
    print(v[2])
