# PROBE (divergence): a comprehension element transfers its own binder
# (`x^`) out of an owned iteration.
#
# The pinned Mojo rejects the transfer: an owned comprehension binder does
# not designate a value with an origin, so it cannot be the operand of `^`.
# The element must name the binder bare (`[x for x in items^]`), which both
# run. Mojito accepts the transfer and prints `2`.
#
# Observed 2026-09-25 against `Mojo 1.2.0.dev2026092105 (e9569894)`:
#   mojo:   error: expression does not designate a value with an origin
#   mojito: 2


def main():
    var items: List[String] = [String("a"), String("b")]
    var moved = [x^ for x in items^]
    print(len(moved))
