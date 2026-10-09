# Probe: assigning through a whole `rebind` whose operand's type is bound by
# `Copyable` alone.
#
# The pin (2026-10-08, `1.6.0.dev2026092105`) prints `9`: the assignment goes
# through the reference its `ref` overload returns, typed `S`, so the old
# value is destroyed as an `S`, which is `Deinitable`. Mojito rewrites the
# statement into the plain assignment `x = S(9)` and rejects it at `T`
# ("'x' abandoned without being explicitly destroyed: unhandled explicitly
# destroyed type 'AnyType'"). docs/roadmap.md R515.
@fieldwise_init
struct S(Copyable):
    var f: Int


def write[T: Copyable](mut x: T):
    rebind[S](x) = S(9)


def main():
    var s = S(4)
    write(s)
    print(s.f)
