# Core iteration protocols.
#
# Current Mojo parameterizes `IteratorType` by the iterable origin.  Mojito's
# trait AST cannot express parameterized associated types yet, so the bundled
# contract keeps the older, monomorphic `Iter`/`OwnedIter` names.  Borrowed
# collection iterators consequently yield copies and are available when their
# element is `Copyable`; consuming iterators move elements.  Concrete `for ref`
# over List remains a checked compiler bridge until origin-parameterized
# associated types can represent it without erasing provenance.

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
    comptime OwnedIter: Iterator

    def __iter__(var self) -> Self.OwnedIter:
        ...
