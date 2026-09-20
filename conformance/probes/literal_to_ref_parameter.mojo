# PROBE (divergence): a literal passed to a `ref` parameter.
#
# The pinned Mojo materializes the literal and binds the parameter to the
# temporary. Mojito accepts the program and then stops at run time with
# "unsupported feature: reference binding to a non-place expression".
#
# Observed 2026-09-20 against `Mojo 1.1.0.dev2026082605 (dd957314)`:
#   mojo:   3
#   mojito: run error
#
# When fixed: promote to `assets/ok`.
def look(ref other: Int) -> Int:
    return other


def main():
    print(look(3))
