# Upstream's `std/builtin/bool.mojo`, holding only `Boolable`: `Bool` is a
# compiler scalar. The prelude exports `Boolable`, as upstream's does.


trait Boolable:
    def __bool__(self) -> Bool:
        ...
