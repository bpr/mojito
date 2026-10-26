# expect: does not conform to trait 'Absable'
# `abs(x)` is upstream's `abs[T: Absable]`: a struct with an `__abs__` method
# that does not declare `Absable` is no argument for it.
@fieldwise_init
struct S(ImplicitlyCopyable):
    var v: Int

    def __abs__(self) -> Self:
        return S(-self.v)


def main():
    print(abs(S(-3)).v)
