# expect: does not conform to trait 'Roundable'
# `round(x)` is upstream's `round[T: Roundable, //]`: a struct with both
# `__round__` methods that does not declare `Roundable` is no argument for it.
@fieldwise_init
struct S(ImplicitlyCopyable):
    var v: Int

    def __round__(self) -> Self:
        return self

    def __round__(self, ndigits: Int) -> Self:
        return self


def main():
    print(round(S(3)).v)
