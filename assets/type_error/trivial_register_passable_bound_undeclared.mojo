# expect: does not conform to trait 'TrivialRegisterPassable'
# A `T: TrivialRegisterPassable` bound rejects a `Movable` struct that does
# not declare the trait, as the pin does.
@fieldwise_init
struct S(Movable):
    var x: Int


def g[T: TrivialRegisterPassable](x: T) -> Int:
    return 1


def main():
    print(g(S(1)))
