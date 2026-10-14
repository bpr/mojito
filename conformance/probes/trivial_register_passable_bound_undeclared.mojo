# Probe: a `T: TrivialRegisterPassable` bound accepts a `Movable` struct.
#
# The pin (2026-10-14) rejects the call: "argument type 'S' does not conform
# to trait 'TrivialRegisterPassable'". Mojito accepts it and prints `1`, as it
# accepts `g([1])`. docs/roadmap.md R862.
@fieldwise_init
struct S(Movable):
    var x: Int


def g[T: TrivialRegisterPassable](x: T) -> Int:
    return 1


def main():
    print(g(S(1)))
