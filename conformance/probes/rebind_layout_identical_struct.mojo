# Probe: `rebind` between two distinct structs of the same lowered layout.
#
# The pin (2026-10-08, `1.6.0.dev2026092105`) prints `4`, then `9`, then `4`:
# its `kgen.rebind` compares lowered KGEN types, so `U` (one `Int` field,
# under any name) rebinds to `S`, by reference and by value. Mojito judges
# the rebind's two types nominally and fails each instance with "rebind
# input type 'U' does not match result type 'S'". docs/roadmap.md R514.
@fieldwise_init
struct S(Copyable):
    var f: Int


@fieldwise_init
struct U(Copyable):
    var g: Int


@fieldwise_init
struct V(TrivialRegisterPassable):
    var f: Int


@fieldwise_init
struct W(TrivialRegisterPassable):
    var f: Int


def read[T: Copyable](mut x: T) -> Int:
    return rebind[S](x).f


def write[T: Copyable & Deinitable](mut x: T):
    rebind[S](x) = S(9)


def by_value[T: TrivialRegisterPassable](x: T) -> Int:
    return rebind[V](x).f


def main():
    var u = U(4)
    print(read(u))
    write(u)
    print(u.g)
    print(by_value(W(4)))
