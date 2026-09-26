# A `DType`-keyed `def` overloaded with a compile-time-keyed one and a plain
# one. The `DType` member survives round one as a stub beside the keyed
# sibling, so its explicit application binds against its own signature rather
# than the sibling's; an inferred call over an argument with a lane selects the
# `Scalar[dt]` pattern over the bare `T`, and a numeric literal binds the lane
# at its default type, as the pinned Mojo does.
# requires: discovery


def kind[T: Copyable](a: T) -> Int:
    comptime if T == Int:
        return 1
    else:
        return 2


def kind[dt: DType](a: Scalar[dt]) -> Int:
    return 3


def kind(a: String) -> Int:
    return 4


def main():
    # Explicit applications name type arguments, not an overload.
    print(kind[DType.float64](1.8))
    print(kind[DType.int32](Int32(4)))
    print(kind[Int](3))
    print(kind[Bool](True))
    # A `Bool` binds `DType.bool` only by converting into
    # `Scalar[DType.bool]`, which costs what binding `T` does not.
    print(kind(True))
    # A `Float64` or a literal has one: the `Scalar[dt]` pattern wins the tie.
    var x: Float64 = 1.5
    print(kind(x))
    print(kind(3))
    print(kind(String("s")))
