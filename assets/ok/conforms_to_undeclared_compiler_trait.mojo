# A struct conforms to a compiler-known trait such as `Boolable` only where
# it declares it, as at the pin: `Plain` prints an empty line, and `Truthy`,
# whose `__bool__` is not a declaration, does too. `Optional[Int]`,
# `List[Int]`, and `String` declare `Boolable`, as upstream's do.
@fieldwise_init
struct Plain(Copyable, Movable):
    var x: Int


@fieldwise_init
struct Truthy(Copyable, Movable):
    var x: Int

    def __bool__(self) -> Bool:
        return self.x != 0


def traits_of[T: AnyType]() -> String:
    var s = String("")
    comptime if conforms_to(T, Boolable):
        s += "Boolable "
    comptime if conforms_to(T, Identifiable):
        s += "Identifiable "
    comptime if conforms_to(T, Sized):
        s += "Sized "
    return s


def main():
    print(traits_of[Plain]())
    print(traits_of[Truthy]())
    print(traits_of[Optional[Int]]())
    print(traits_of[List[Int]]())
    print(traits_of[String]())
    print(traits_of[Int]())
    if Truthy(1):
        print("truthy")
