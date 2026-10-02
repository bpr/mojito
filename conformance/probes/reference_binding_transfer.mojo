# PROBE (divergence): a `^` transfer of a `ref` binding.
#
# A reference binding does not own its referent, so the pin rejects `r^`.
# Mojito rejects the program too, but only when the specialized MIR fails
# to verify, with an internal message instead of a checker diagnostic. A
# borrowed `for x in xs:` binder transferred in the loop body fails the same
# way.
#
# Observed 2026-10-02 against `Mojo 1.2.0.dev2026092105 (e9569894)`:
#   mojo:   error: expression does not designate a value with an origin
#   mojito: Elaboration error: unsupported specialized MIR that does not
#           verify: ... binding of ref String to a slot of type String
def main():
    var s = String("a")
    ref r = s
    var t = r^
    print(t)
