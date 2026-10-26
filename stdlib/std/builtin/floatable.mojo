# Upstream's `std/builtin/floatable.mojo`, without `Floatable`, which the
# compiler still knows by name (R1360). The prelude exports
# `FloatableRaising`, as upstream's does.


trait FloatableRaising:
    def __float__(self) raises -> Float64:
        ...
