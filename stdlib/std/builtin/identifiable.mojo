# Upstream's `std/builtin/identifiable.mojo`: the `Identifiable` trait, whose
# `__isnot__` defaults through `__is__`. The prelude exports it, as
# upstream's does.


trait Identifiable:
    def __is__(self, rhs: Self) -> Bool:
        ...

    @inline(.always)
    def __isnot__(self, rhs: Self) -> Bool:
        return not (self is rhs)
