# expect: cannot implicitly convert
# A `_subtree` slot accepts the storage it projects, but a subtree origin
# fits only its own slot: `S[o._subtree]` does not convert to `S[o]`.
struct S[o: Origin](Movable):
    def __init__(out self):
        pass


def f[o: MutOrigin](ref[o] x: Int):
    var a = S[o._subtree]()
    var b: S[o] = a^
    print("accepted")


def main():
    var x = 1
    f(x)
