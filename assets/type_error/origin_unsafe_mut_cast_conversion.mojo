# A struct over `o.unsafe_mut_cast[False]()` is a distinct type from one over
# `o`: the cast is a different origin value, so `S[o]` does not convert to it.
# expect: cannot implicitly convert
struct S[o: Origin]:
    def __init__(out self):
        pass


def f[o: Origin]():
    var b: S[o.unsafe_mut_cast[False]()] = S[o]()
    _ = b^


def main():
    var x = 1
    f[origin_of(x)]()
