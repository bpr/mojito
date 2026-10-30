# expect: violated constraint
# A concrete call whose dtype makes a `where` clause's `DType` predicate
# `False` violates the clause.
def lane_kind[dtype: DType, //](x: Scalar[dtype]) -> Int where dtype.is_floating_point():
    return 1


def main():
    print(lane_kind(Int32(1)))
