# A struct's origin argument holds an interior or subtree projection over an
# origin binder, as upstream's `_mlir_origin` attribute does, and `S[o]`
# converts to `S[o._subtree]`, the storage the subtree slot projects.
from std.reflection import reflect


struct S[o: Origin](Movable):
    def __init__(out self):
        pass


def f[o: MutOrigin](ref[o] x: Int):
    print(reflect[S[o]].field_count())
    print(reflect[S[o._get_owned_interior["e"]]].field_count())
    print(reflect[S[o._subtree]].field_count())
    var a = S[o]()
    var b: S[o._subtree] = a^
    print("converted")


def main():
    var x = 1
    f(x)
