# `Origin` declares no conversion to a mutable origin, so `MutOrigin(o)`
# matches no initializer.
# expect: no constructor overload matches the supplied arguments


struct S[mut: Bool, //, o: Origin[mut=mut]]:
    def __init__(out self):
        pass


def f[o: MutOrigin](ref[o] x: Int):
    var a: S[MutOrigin(o)] = S[o]()


def main():
    var x = 1
    f(x)
