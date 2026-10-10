# An importer asks `conforms_to` of library types over library traits,
# compiler-known traits the library never asks, and traits the importer
# declares: each is decided from the type's declared conformances and the
# trait refinement graph when the clause asks it.
from std.os import PathLike


trait Named:
    def name(self) -> String:
        ...


trait Titled(Named):
    def title(self) -> String:
        ...


@fieldwise_init
struct Book(Copyable, Movable, Titled):
    var n: Int

    def name(self) -> String:
        return "book"

    def title(self) -> String:
        return "Book"


def traits_of[T: AnyType]() -> String:
    var s = String("")
    comptime if conforms_to(T, Hashable):
        s += "Hashable "
    comptime if conforms_to(T, Writable):
        s += "Writable "
    comptime if conforms_to(T, Copyable):
        s += "Copyable "
    comptime if conforms_to(T, Movable):
        s += "Movable "
    comptime if conforms_to(T, Absable):
        s += "Absable "
    comptime if conforms_to(T, PathLike):
        s += "PathLike "
    comptime if conforms_to(T, Named):
        s += "Named "
    return s


def iterable[T: AnyType]() -> Bool:
    comptime if conforms_to(T, Iterable):
        return True
    else:
        return False


def main():
    print(traits_of[List[Int]]())
    print(traits_of[Optional[Int]]())
    print(traits_of[Dict[String, Int]]())
    print(traits_of[String]())
    print(traits_of[Book]())
    print(traits_of[Int]())
    print(iterable[List[Int]](), iterable[Dict[String, Int]](), iterable[Book]())
    print(Book(1).title(), Book(2).name())
