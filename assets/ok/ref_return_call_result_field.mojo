# A field of a reference-returning call's result is a place within the
# call's result origin, as at the pin: `inner(w).t` under `ref[w] W` is
# `w.t`, so it returns under `ref[w.t]` (or `origin_of(w.t)`) and writes
# through to the caller's storage, from free functions and methods alike.
@fieldwise_init
struct W(Copyable, Movable):
    var t: Int
    var u: Int

    def whole(ref self) -> ref[self] W:
        return self

    def second(ref self) -> ref[self.u] Int:
        return self.whole().u


def inner(ref w: W) -> ref[w] W:
    return w


def outer(ref w: W) -> ref[w.t] Int:
    return inner(w).t


def read_inner(w: W) -> ref[origin_of(w)] W:
    return w


def read_outer(w: W) -> ref[origin_of(w.t)] Int:
    return read_inner(w).t


def main():
    var w = W(3, 4)
    print(outer(w))
    outer(w) = 9
    print(w.t)
    print(read_outer(w))
    w.second() += 10
    print(w.u, w.second())
