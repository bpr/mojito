# A `DType`-keyed `def` overloaded with a concrete one and a trait-bound
# generic one that holds no compile-time construct. The `DType` member alone
# makes the name a template family, so the other two are ordinary overloads:
# the concrete `Float64` beats both generics, and between the generics an
# argument with a lane selects the `Scalar[dt]` pattern over the bare `T`.
# requires: discovery


def kind(a: Float64) -> Int:
    return 1


def kind[dt: DType](a: Scalar[dt]) -> Int:
    return 2


def kind[T: Copyable](a: T) -> Int:
    return 3


def main():
    var x: Float64 = 1.5
    print(kind(x))
    var y: Int32 = 4
    print(kind(y))
    print(kind(String("s")))
    print(kind(1.5))
    print(kind(7))
