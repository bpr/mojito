# Upstream's `std/builtin/floatable.mojo`: the `Floatable` and
# `FloatableRaising` traits. The prelude exports both, as upstream's does.


trait Floatable:
    def __float__(self) -> Float64:
        ...


trait FloatableRaising:
    def __float__(self) raises -> Float64:
        ...
