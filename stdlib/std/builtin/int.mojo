# Upstream's `std/builtin/int.mojo`, holding only `IntableRaising`: `Int` is
# a compiler scalar and `Intable` a compiler-known trait (R1360). The prelude
# exports `IntableRaising`, as upstream's does.


trait IntableRaising:
    def __int__(self) raises -> Int:
        ...
