# expect: function instantiation of `leaf` failed: constraint failed:
# `leaf` calls `w`, a `nodebug` wrapper of the recursive `rec`, so it reaches
# a cycle of `nodebug` calls and `inline-parametric` keeps its call, as the
# pin's `InlineParametric` does: the failed constraint names `leaf`'s own
# instance, where an inlined `leaf` would name `gen`'s.


@always_inline("nodebug")
def rec[t: Int](x: Int) -> Int:
    comptime if t == 0:
        return rec[1](x)
    return x + t


@always_inline("nodebug")
def w(x: Int) -> Int:
    return rec[1](x) + 1


@always_inline("nodebug")
def leaf[k: Int](x: Int) -> Int:
    comptime assert k < 3
    _ = w(x)
    return 1


def gen[n: Int](x: Int) -> Int:
    var r = leaf[n](x)
    return r


def main():
    print(gen[5](1))
