# Core iteration protocols, homed at `std.iter` as in current Mojo, whose
# prelude also exports `Iterator`, `Iterable`, `IterableOwned`, and
# `StopIteration`. A `for` loop advances through `__next__`, which raises
# `StopIteration` at exhaustion.
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
# `keys`/`values`/`items` return self-iterable, non-indexable borrowing
# views that hold a loan on their source while they live.

from std.builtin.rebind import rebind_var
from std.collections.optional import Optional

@fieldwise_init
struct StopIteration(TrivialRegisterPassable, Writable):
    def write_to(self, mut writer: Some[Writer]):
        writer.write("StopIteration")

trait Iterator(Deinitable, Movable):
    comptime Element: Movable

    def __next__(mut self) raises StopIteration -> Self.Element:
        ...

    # Bounds `[lower, upper]` on the remaining length, a pre-allocation hint
    # only; `None` means unknown (or above `Int.MAX`).
    def bounds(self) -> Tuple[Int, Optional[Int]]:
        return (0, None)

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


# `iter(iterable)`: the builtin spelling of `__iter__`. The owned overload
# consumes an `IterableOwned` source; the borrowed one borrows an `Iterable`
# at the argument's origin, as upstream declares them.
@inline(.always)
def iter(
    var iterable: Some[IterableOwned],
) -> type_of(iterable).IteratorOwnedType:
    return iterable^.__iter__()


@inline(.always)
def iter[
    IterableType: Iterable
](ref iterable: IterableType) -> IterableType.IteratorType[origin_of(iterable)]:
    return iterable.__iter__()


# `next(iterator)`: the builtin spelling of `__next__`. Exhaustion raises
# `StopIteration`, which a plain `raises` caller converts to `Error` through
# its `@implicit` initializer, as upstream's does.
@inline(.always)
def next[
    IteratorType: Iterator
](mut iterator: IteratorType) raises StopIteration -> IteratorType.Element:
    return iterator.__next__()


# `map[function](iterable)`: a lazy iterator applying `function` to each
# element of `iterable`, as upstream's `_MapIterator`.
@fieldwise_init
struct _MapIterator[
    OutputType: Copyable,
    InnerIteratorType: Iterator,
    //,
    function: def(var InnerIteratorType.Element) thin -> OutputType,
](
    Copyable where conforms_to(InnerIteratorType, Copyable),
    Iterable where conforms_to(InnerIteratorType, Copyable),
    IterableOwned,
    Iterator,
):
    comptime Element = Self.OutputType
    comptime IteratorType[
        iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]
    ]: Iterator = Self
    comptime IteratorOwnedType: Iterator = Self

    var _inner: Self.InnerIteratorType

    def __iter__(
        ref self,
    ) -> Self.IteratorType[origin_of(self)] where conforms_to(
        Self.InnerIteratorType, Copyable
    ):
        return self.copy()

    @inline(.always)
    def __iter__(var self) -> Self.IteratorOwnedType:
        return self^

    def __next__(mut self) raises StopIteration -> Self.Element:
        return Self.function(next(self._inner))

    def bounds(self) -> Tuple[Int, Optional[Int]]:
        return self._inner.bounds()


@inline(.always)
def map[
    origin: ImmOrigin,
    IterableType: Iterable,
    ResultType: Copyable,
    //,
    function: def(
        var IterableType.IteratorType[origin].Element
    ) thin -> ResultType,
](ref[origin] iterable: IterableType) -> _MapIterator[function]:
    return {
        rebind_var[_MapIterator[function].InnerIteratorType](iter(iterable))
    }


@inline(.always)
def map[
    IterableType: IterableOwned,
    ResultType: Copyable,
    //,
    function: def(
        var IterableType.IteratorOwnedType.Element
    ) thin -> ResultType,
](var iterable: IterableType) -> _MapIterator[function]:
    return {
        rebind_var[_MapIterator[function].InnerIteratorType](iter(iterable^))
    }
