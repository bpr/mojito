# Upstream's `std/builtin/value.mojo`: the `Defaultable`,
# `RegisterPassable`, and `TrivialRegisterPassable` traits. The prelude
# exports all three, as upstream's does.
#
# Upstream's `Defaultable.__init__` defaults through `reflect[Self]`,
# default-constructing each field in place; it stays a requirement here
# until a trait default can construct `self` field by field (R1119).
#
# `TrivialRegisterPassable` forbids a custom copy constructor or `__del__`
# and requires each field to conform, as upstream's docstring states; the
# checker does not yet reject a struct breaking either rule (R860).


trait Defaultable:
    def __init__(out self):
        ...


trait RegisterPassable(Movable):
    pass


trait TrivialRegisterPassable(
    Deinitable,
    ImplicitlyCopyable,
    Movable,
    RegisterPassable,
):
    pass
