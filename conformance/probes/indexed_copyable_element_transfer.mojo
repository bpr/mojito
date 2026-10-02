# PROBE (divergence): a `^` transfer of an indexed element whose type is
# implicitly copyable but not a trivial register value.
#
# An indexed element is not independently movable storage. The pin rejects
# `x[0]^` unless the element is a trivial register value (`Int`), where it
# only warns. Mojito accepts any implicitly copyable element and copies it.
#
# Observed 2026-10-02 against `Mojo 1.2.0.dev2026092105 (e9569894)`:
#   mojo:   error: expression does not designate a value with an origin
#   mojito: 1
@fieldwise_init
struct P(ImplicitlyCopyable):
    var a: Int
    var b: Int


def first(x: List[P]) -> P:
    return x[0]^


def main():
    print(first([P(1, 2)]).a)
