# Core iteration protocols.
#
# The owned contract uses current Mojo's monomorphic `IteratorOwnedType`: a
# consuming iterator owns its storage, so it needs no origin parameter.
#
# The borrowed contract uses current Mojo's origin-parameterized
# `IteratorType[iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]]`
# with `__iter__(ref self) -> Self.IteratorType[origin_of(self)]`. Conforming
# collections currently erase the origin in their member templates: borrowed
# iterators still copy or point into their storage and yield element copies
# when the element is `Copyable`. Making iterators borrow their source through
# the origin parameter and yield references — and removing the concrete List
# `for ref` compiler bridge — is tracked under generic borrowed reference
# iteration.

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
