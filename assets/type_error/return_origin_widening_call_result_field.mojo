# expect: cannot return reference with incompatible origin
# A field of a reference-returning call's result is a place within the
# call's result origin, so `inner(w).t` is `w.t`: a different origin from
# `w`, and `ref[origin_of(w)]` rejects it, as at the pin.
@fieldwise_init
struct W(Copyable, Movable):
    var t: Int
    var u: Int


def inner(ref w: W) -> ref[w] W:
    return w


def outer(ref w: W) -> ref[origin_of(w)] Int:
    return inner(w).t


def main():
    var w = W(3, 4)
    print(outer(w))
