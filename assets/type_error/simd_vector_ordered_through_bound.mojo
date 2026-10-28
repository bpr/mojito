# expect: constraint failed: Greater than or equal is only defined for `Scalar`s; did you mean to use `SIMD.ge(...)`?
# A four-lane vector is `Comparable`, so it binds `T`, and the bound's `>=`
# is its `SIMD.__ge__`, whose one-lane assertion the instance fails.

def at_least[T: Comparable](left: T, right: T) -> Bool:
    return left >= right


def main():
    var v = SIMD[DType.int32, 4](1, 2, 3, 4)
    print(at_least(v, v))
