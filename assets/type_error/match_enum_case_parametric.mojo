# expect: cannot match on a parametric enum type
# A subject whose type is a parameter bounded by `EnumLike` has no known
# case list.
def first[T: EnumLike](value: T):
    match value:
        case .Some(y):
            pass
        case _:
            pass


def main():
    first(Optional[Int](3))
