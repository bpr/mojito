# expect: cannot implicitly convert
# A struct's origin argument holds an interior projection over an origin
# binder, as upstream's `_mlir_origin` attribute does, so `S[o]` is not
# `S[o._get_owned_interior["e"]]`.
struct S[o: Origin](Movable):
    def __init__(out self):
        pass


def f[o: MutOrigin](ref[o] x: Int):
    var a = S[o]()
    var b: S[o._get_owned_interior["e"]] = a^
    print("accepted")


def main():
    var x = 1
    f(x)
