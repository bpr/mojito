# An inlined constructor that reads no parameter of its struct leaves no
# read of the caller's parameter behind: `mkg` passes `n` only as `G[n]`'s
# compile-time argument, which the inlined `__init__` never reads, so `mkg`
# loses `n` and keeps one instance.


struct G[n: Int]:
    var v: Int

    @always_inline("nodebug")
    def __init__(out self, v: Int):
        self.v = v


def mkg[n: Int]() -> Int:
    return G[n](5).v


def main():
    print(mkg[1](), mkg[2]())
