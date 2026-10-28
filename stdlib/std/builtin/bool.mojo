# Upstream's `std/builtin/bool.mojo`, holding `Boolable` and the `SIMD`
# overloads of `any` and `all`: `Bool` is a compiler scalar. The prelude
# exports `Boolable`, `any`, and `all`, as upstream's does.
#
# Upstream's `Iterable` overloads of `any` and `all` are absent: their
# `where conforms_to(IterableType.IteratorType[origin_of(iterable)].Element,
# Boolable & Deinitable)` clause takes a projection subject (R792).


trait Boolable:
    def __bool__(self) -> Bool:
        ...


def any(value: SIMD) -> Bool:
    """Checks if **any** element in the simd vector is truthy.

    Args:
        value: The simd vector to check.

    Returns:
        `True` if **any** element in the simd vector is truthy, `False`
        otherwise.
    """
    return value.cast[.bool]().reduce_or()


def all(value: SIMD) -> Bool:
    """Checks if **all** elements in the simd vector are truthy.

    Args:
        value: The simd vector to check.

    Returns:
        `True` if **all** elements in the simd vector are truthy, `False`
        otherwise.
    """
    return value.cast[.bool]().reduce_and()
