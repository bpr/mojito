# `not x` over a type parameter bounded by `Boolable` is `not Bool(x)`,
# the conversion through its `__bool__` requirement.


@fieldwise_init
struct Flag(Boolable, ImplicitlyCopyable):
    var on: Bool

    def __bool__(self) -> Bool:
        return self.on


def negate[T: Boolable](x: T) -> Bool:
    return not x


def main():
    print(negate(0), negate(3), negate(True))
    print(negate(Flag(False)), negate(Flag(True)))
