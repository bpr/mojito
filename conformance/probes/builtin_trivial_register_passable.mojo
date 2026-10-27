# PROBE: a built-in trivial type bound by `TrivialRegisterPassable`.
#
# **Differs.** The pin prints `7 a`: `Pointer` and `StringLiteral` declare
# `TrivialRegisterPassable` upstream. Mojito rejects `keep(p)`, "type
# 'Pointer[Int, ...]' for parameter 'T' does not conform to trait
# 'TrivialRegisterPassable'", and infers `keep("a")` at `String`: the
# modelled conformance (`modelled_trivial_register_passable`) and the
# checker's `is_trivial_register_passable` omit both kinds.
# `docs/roadmap.md` R1502. When Mojito runs it, move this to `assets/ok/`.
def keep[T: TrivialRegisterPassable](x: T) -> T:
    return x


def main():
    var x = 7
    var p = Pointer(to=x)
    print(keep(p)[], keep("a"))
