# An importer asks `conforms_to` of a trait it declares and of a builtin
# trait the bundled library never asks, over its own struct and over
# library types.
trait Greeter:
    def greet(self) -> String:
        ...


@fieldwise_init
struct Hi(Copyable, Greeter, Movable):
    var n: Int

    def greet(self) -> String:
        return "hi"


def greets[T: AnyType]() -> String:
    comptime if conforms_to(T, Greeter):
        return "greeter"
    else:
        return "plain"


def absolute[T: AnyType]() -> String:
    comptime if conforms_to(T, Absable):
        return "absable"
    else:
        return "no"


def main():
    print(greets[Hi]())
    print(greets[List[Int]]())
    print(greets[Int]())
    print(absolute[List[Int]]())
    print(absolute[Int]())
    print(absolute[String]())
