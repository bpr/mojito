# PROBE (native): `Tuple.concat` whose argument holds a `String` element.
#
# The pinned Mojo and Mojito's VM print the result. Mojito's native backend
# prints it and then traps with "vm: double free of Pointer allocation"; a
# `String` in the receiver, and `reverse` over one, run natively
# (`assets/ok/tuple_template_served.mojo`), and so does an argument of
# `Int`s (roadmap R277).
#
# Observed 2026-10-06 against `Mojo 1.2.0.dev2026092105 (e9569894)`:
#   mojo:   x 4
#   mojito: VM matches; `--backend pliron` traps
#
# When fixed: fold into `assets/ok/tuple_template_served.mojo`.


def main():
    var u = (7, 2)
    var c = u^.concat((String("x"), 3.5))
    print(c[2], len(c))
