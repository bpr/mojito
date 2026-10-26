# `conforms_to(T, TrivialRegisterPassable)` answers from the struct's
# declaration and the compiler-known types upstream declares it on: a
# `Movable` struct and `List[Int]` do not conform, a declaring struct and
# `Int` do. Prints `False True True False`, as at the pin.
@fieldwise_init
struct S(Movable):
    var x: Int


@fieldwise_init
struct P(TrivialRegisterPassable):
    var x: Int


def f[T: AnyType]() -> Bool:
    comptime if conforms_to(T, TrivialRegisterPassable):
        return True
    else:
        return False


def main():
    print(f[S](), f[P](), f[Int](), f[List[Int]]())
