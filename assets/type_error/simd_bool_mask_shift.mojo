# expect: constraint failed: must be an integral type
# `SIMD.__lshift__`'s `comptime assert` rejects a `bool` mask in the instance
# a run-time `<<` reaches.
def main():
    var m = SIMD[DType.bool, 2](True, False)
    print(m << m)
