# expect: lacking evidence to prove correctness
# A generic caller with no `comptime if` arm or `where` clause of its own
# has no evidence for the callee's `DType` predicate.
def lane_kind[dtype: DType, //](x: Scalar[dtype]) -> Int where dtype.is_floating_point():
    return 1


def unguarded[dtype: DType](x: Scalar[dtype]) -> Int:
    return lane_kind(x)


def main():
    print(unguarded(Float32(1.5)))
