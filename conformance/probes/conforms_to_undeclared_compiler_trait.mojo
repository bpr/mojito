# Probe: a struct asked a compiler-known trait it never declares.
#
# The pin (2026-10-10) prints an empty line for `Plain`, `Boolable` alone
# for `Optional[Int]`, and `Boolable` alone for `String`. Mojito prints
# `Boolable` for all three: a struct conforms to every compiler-known trait
# outside its rule's declared, lifecycle, and operation classes
# (`mojito_types::conformance::StructTraitRule::Shallow`), declared or not.
# `Sized`, now the bundled library's trait, answers as the pin does (R1051).
# docs/roadmap.md R577.
@fieldwise_init
struct Plain(Copyable, Movable):
    var x: Int


def traits_of[T: AnyType]() -> String:
    var s = String("")
    comptime if conforms_to(T, Boolable):
        s += "Boolable "
    comptime if conforms_to(T, Sized):
        s += "Sized "
    return s


def main():
    print(traits_of[Plain]())
    print(traits_of[Optional[Int]]())
    print(traits_of[String]())
