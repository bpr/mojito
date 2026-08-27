# Core iteration protocols.
#
# The owned contract uses current Mojo's monomorphic `IteratorOwnedType`: a
# consuming iterator owns its storage, so it needs no origin parameter.
#
# The borrowed contract uses current Mojo's origin-parameterized
# `IteratorType[iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]]`
# with `__iter__(ref self) -> Self.IteratorType[origin_of(self)]`. Every
# bundled borrowed iterator borrows its source through a parametric-mut origin
# and yields element references declared at `_get_owned_interior["element"]`
# granularity, resolved to the source's mutability at each loop site; mapping
# mutation during iteration lazily invalidates the iteration generation, and
# `keys`/`values`/`items` return self-iterable, non-indexable snapshot
# iterators.

@fieldwise_init
struct StopIteration:
    pass

trait Iterator:
    comptime Element: Movable

    def __next__(mut self) raises StopIteration -> Self.Element:
        ...

trait Iterable:
    # Borrowed iteration yields element references whose ordinary value reads
    # copy, so the borrowed protocol's element is Copyable — matching the
    # bundled conformers' existing conditional conformances. (The consuming
    # `IterableOwned` protocol keeps `Movable` so owned iteration can move
    # non-Copyable elements.)
    comptime Element: Copyable & Movable
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
