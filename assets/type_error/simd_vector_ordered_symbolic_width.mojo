# expect: function instantiation of `ordered` failed: constraint failed: Less than or equal is only defined for `Scalar`s; did you mean to use `SIMD.le(...)`?
# A comparison over a symbolic width is judged per instance: `ordered[1]`
# holds `SIMD.__le__`'s one-lane assertion, and `ordered[2]` fails it.

def ordered[w: Int](left: SIMD[DType.int32, w], right: SIMD[DType.int32, w]) -> Bool:
    return left <= right


def main():
    print(ordered[1](Int32(1), Int32(2)))
    print(ordered[2](SIMD[DType.int32, 2](1), SIMD[DType.int32, 2](2)))
