# PROBE (native): `Tuple.reverse` and `Tuple.concat` over a `String` element.
#
# The pinned Mojo and Mojito's VM print both results. Mojito's native backend
# prints the first and then traps with "vm: double free of Pointer
# allocation"; an `Int`/`Bool` tuple runs natively (roadmap R277).
#
# Observed 2026-10-04 against `Mojo 1.2.0.dev2026092105 (e9569894)`:
#   mojo:   x 1 / True x
#   mojito: VM matches; `--backend pliron` traps
#
# When fixed: fold into `assets/ok/tuple_annotated_binding.mojo`.


def flip(t: Tuple[Int, String]) -> Tuple[String, Int]:
    return t.reverse()


def main():
    var t: Tuple[Int, String] = (1, "x")
    var f = flip(t)
    print(f[0], f[1])
    var c = t.concat(Tuple(True))
    print(c[2], t[1])
