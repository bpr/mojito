# A member read through a bound type parameter names a member the bound
# requires; `Z` is not one of `HasK`'s, so the pin rejects the read.
# expect: 'HasK' value has no attribute 'Z'
trait HasK:
    comptime K: Int


@fieldwise_init
struct A(HasK):
    comptime K = 7


def read[T: HasK]():
    print(T.Z)


def main():
    read[A]()
