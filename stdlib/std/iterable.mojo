# Core iteration protocols.
#
# The owned contract uses current Mojo's monomorphic `IteratorOwnedType`: a
# consuming iterator owns its storage, so it needs no origin parameter.
#
# The borrowed contract uses current Mojo's origin-parameterized
# `IteratorType[iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]]`
# with `__iter__(ref self) -> Self.IteratorType[origin_of(self)]`. The List and
# Set iterators borrow their source through a parametric-mut origin and yield
# element references, resolved to the source's mutability at each loop site;
# the mapping iterators still snapshot their entries and yield copies (tracked
# under mapping invalidation and borrowed-iteration safety).

@fieldwise_init
struct StopIteration:
    pass

trait Iterator:
    comptime Element: Movable

    def __next__(mut self) raises StopIteration -> Self.Element:
        ...

trait Iterable:
    comptime Element: Movable
    comptime IteratorType[
        iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]
    ]: Iterator

    def __iter__(ref self) -> Self.IteratorType[origin_of(self)]:
        ...

trait IterableOwned:
    comptime Element: Movable
    comptime IteratorOwnedType: Iterator

    def __iter__(var self) -> Self.IteratorOwnedType:
        ...
