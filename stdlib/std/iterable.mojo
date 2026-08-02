# Core iteration protocols.
#
# The owned contract uses current Mojo's monomorphic `IteratorOwnedType`: a
# consuming iterator owns its storage, so it needs no origin parameter.
#
# The borrowed contract still keeps the older, monomorphic `Iter` name.  Current
# Mojo parameterizes `IteratorType` by the iterable origin
# (`__iter__(ref self) -> Self.IteratorType[origin_of(self)]`); migrating the
# borrowed protocol still needs the ordered source-mode, lowering, and library
# migration work tracked under generic borrowed reference iteration. Until then,
# borrowed collection iterators yield copies when their element is `Copyable`,
# and concrete `for ref` over List remains a checked compiler bridge.

@fieldwise_init
struct StopIteration:
    pass

trait Iterator:
    comptime Element: Movable

    def __next__(mut self) raises StopIteration -> Self.Element:
        ...

trait Iterable:
    comptime Element: Movable
    comptime Iter: Iterator

    def __iter__(self) -> Self.Iter:
        ...

trait IterableOwned:
    comptime Element: Movable
    comptime IteratorOwnedType: Iterator

    def __iter__(var self) -> Self.IteratorOwnedType:
        ...
