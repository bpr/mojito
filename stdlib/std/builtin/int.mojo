# Upstream's `std/builtin/int.mojo`, holding its traits and `index`: `Int`
# is a compiler scalar. The prelude exports `Indexer`, `index`, `Intable`,
# and `IntableRaising`, as upstream's does.


trait Indexer:
    def __mlir_index__(self) -> __mlir_type.index:
        ...


@inline(.nodebug)
def index[T: Indexer](idx: T, /) -> Int:
    """Returns the value of `__mlir_index__` for the given value.

    Parameters:
        T: A type conforming to the `Indexer` trait.

    Args:
        idx: The value.

    Returns:
        An `__mlir_type` representing the index value.
    """
    return Int(SIMDLength(mlir_value=idx.__mlir_index__()))


trait Intable:
    def __int__(self) -> Int:
        ...


trait IntableRaising:
    def __int__(self) raises -> Int:
        ...


trait _FromInt:
    def __init__(out self, *, from_int: Int):
        ...
