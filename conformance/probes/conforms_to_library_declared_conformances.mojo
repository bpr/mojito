# Probe: a bundled library type's declared conformances, asked one by one.
#
# The pin (2026-10-10) prints `Equatable Defaultable Iterable` for
# `List[Int]` and `Equatable Defaultable` for `String`. Mojito prints
# `Iterable` alone for `List[Int]`, whose bundled declaration lists neither
# `Equatable` nor `Defaultable`, and adds `Iterable` for `String`, whose
# bundled declaration lists it where upstream's does not. docs/roadmap.md
# R578.
def traits_of[T: AnyType]() -> String:
    var s = String("")
    comptime if conforms_to(T, Equatable):
        s += "Equatable "
    comptime if conforms_to(T, Defaultable):
        s += "Defaultable "
    comptime if conforms_to(T, Iterable):
        s += "Iterable "
    return s


def main():
    print(traits_of[List[Int]]())
    print(traits_of[String]())
