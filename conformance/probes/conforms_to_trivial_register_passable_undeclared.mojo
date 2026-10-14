# Probe: `conforms_to(T, TrivialRegisterPassable)` over types that never
# declare it.
#
# The pin (2026-10-14) prints `False True True False`. Mojito prints
# `True True True True`: it answers `TrivialRegisterPassable` by the shallow
# rule (`mojito_types::conformance::StructTraitRule::Shallow`), not from the
# struct's declaration. Its sibling
# `trivial_register_passable_bound_undeclared.mojo` pins the same rule at a
# bound. docs/roadmap.md R862.
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
