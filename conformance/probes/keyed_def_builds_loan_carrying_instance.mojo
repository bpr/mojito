# Probe: does a generic `def` that calls a compile-time-keyed method run when
# only its own body names the instance, and the argument carries a loan?
#
# `use` reaches `Box.kind`, whose body is a `comptime if`, so `use` keeps its
# clone, spelled over an origin binder. `Box[Pointer[...]]` is named only
# inside that clone.
#
# Pin (2026-09-21): prints `2`.
# Mojito: stops at run time ("Box.kind: unspecialized type-keyed method"):
# the instance the clone names over its origin binder is never requested.
struct Box[T: Copyable & Deinitable](Movable):
    var item: Self.T

    def __init__(out self, var item: Self.T):
        self.item = item^

    def kind(self) -> Int:
        comptime if conforms_to(Self.T, Hashable):
            return 1
        else:
            return 2


def use[T: Copyable & Deinitable](value: T) -> Int:
    var b = Box[T](value.copy())
    return b.kind()


def main():
    var xs: List[Int] = [7, 8, 9]
    var p: Pointer[List[Int], ImmOrigin(origin_of(xs))] = Pointer(to=xs)
    print(use(p))
