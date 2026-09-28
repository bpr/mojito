# PROBE (divergence): a reference-returning call as an assignment target.
#
# The pinned Mojo writes through the returned reference. Mojito stops at
# parse: the parser admits only names, fields, and subscripts as targets.
#
# Observed 2026-09-28 against `Mojo 1.2.0.dev2026092105 (e9569894)`:
#   mojo:   9 / 10
#   mojito: parse error ("invalid assignment target")
#
# When fixed: promote to `assets/ok`.
def bump(ref a: Int) -> ref[origin_of(a)] Int:
    return a


def main():
    var k = 5
    bump(k) = 9
    print(k)
    bump(k) += 1
    print(k)
