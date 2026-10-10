# Probe: a compiler-known trait named without the import upstream needs.
#
# The pin (2026-10-10) rejects `Representable` ("use of unknown declaration
# 'Representable'") and `Ceilable` ("did you mean to import it from
# 'std.math'?"). Mojito resolves both from its compiler-known trait list and
# prints `repr ceil`. docs/roadmap.md R579.
def has[T: AnyType]() -> String:
    var s = String("")
    comptime if conforms_to(T, Representable):
        s += "repr "
    comptime if conforms_to(T, Ceilable):
        s += "ceil"
    return s


def main():
    print(has[Int]())
