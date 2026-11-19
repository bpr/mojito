# A result-form initializer's declared result defines every slot of the
# struct it constructs, its origin binder and that binder's `mut=` included:
# `Origin`'s `@implicit __init__(v: Origin) ->
# ImmOrigin[_mlir_origin=v._mlir_origin]` constructs an immutable origin
# explicitly as it does by conversion, through every spelling of `Origin`
# and a user struct declaring the same shape.
from std.origin import _lit_origin_type_of_mut


struct O2[mut: Bool, _mlir_origin: _lit_origin_type_of_mut[mut], //]:
    def __init__(out self):
        pass

    @implicit
    def __init__(v: O2) -> O2[mut=False, _mlir_origin=v._mlir_origin]:
        return {}


def main():
    var a = ImmOrigin(MutUntrackedOrigin)
    print(a.mut)
    var b = Origin[mut=False](MutUntrackedOrigin)
    print(b.mut)
    var c = Origin(MutUntrackedOrigin)
    print(c.mut)
    comptime k = ImmOrigin(MutUntrackedOrigin)
    print(k.mut)
    var e: ImmOrigin[
        _mlir_origin=MutUntrackedOrigin._mlir_origin
    ] = ImmOrigin(MutUntrackedOrigin)
    print(e.mut)
    var o = O2(O2[mut=True, _mlir_origin=MutUntrackedOrigin._mlir_origin]())
    print(o.mut)
