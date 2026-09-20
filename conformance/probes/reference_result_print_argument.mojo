# PROBE (divergence): a reference-returning `def` called as a `print`
# argument.
#
# The pinned Mojo reads through the returned reference. Mojito reports that
# `ref String` does not conform to `Writable`; the same call on a method
# reads through and prints.
#
# Observed 2026-09-20 against `Mojo 1.1.0.dev2026082605 (dd957314)`:
#   mojo:   w
#   mojito: invalid call to 'print': an element of 'values' with type
#           'ref String' does not conform to trait 'Writable'
#
# When fixed: promote to `assets/ok`.
def pick[o: Origin](ref[o] x: String) -> ref[o] String:
    return x


def main():
    var w = String("w")
    print(pick(w))
    print(w)
