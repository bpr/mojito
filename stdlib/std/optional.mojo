# A self-hosted, generic `Optional[T]` over any element (`T: AnyType`),
# matching current Mojo's owning-container model: lifecycle capabilities are
# conditional on the element's, construction offers `init_with=` placement
# (no `Movable` requirement — the factory result lands directly in storage),
# and teardown offers `deinit_with` (handler-consumed payload) plus
# `deinit_assert_empty` (the linear-Optional named destructor). The payload
# lives in one owned heap slot; `data` is initialized exactly when
# `_size == 1`, and `unsafe_take_pointee`/`unsafe_deinit_pointee` are the raw
# vocabulary over that storage, as in List and Array.

from std.iterable import Iterable, IterableOwned, Iterator, StopIteration

from std.memory import unsafe_alloc

@fieldwise_init
struct _OptionalIter[
    iterable_mut: Bool, //, T: AnyType, iterable_origin: Origin[mut=iterable_mut]
](Iterator where conforms_to(T, Copyable)):
    comptime Element = Self.T

    var src: ref[iterable_origin] Optional[Self.T]
    var index: Int

    def __len__(self) -> Int:
        return self.src._size - self.index

    # Yields owned copies: chaining the named `value()` interior reference
    # through the iterator is not supported, so Optional `for ref` iteration
    # stays a recorded subset gap while plain borrowed iteration copies.
    def __next__(mut self) raises StopIteration -> Self.T where conforms_to(
        Self.T, Copyable
    ):
        if self.index >= self.src._size:
            raise StopIteration()
        self.index += 1
        return self.src.data[0].copy()

struct _OptionalOwnedIter[T: AnyType](
    Deinitable where conforms_to(T, Deinitable),
    Iterator where conforms_to(T, Movable),
    Movable,
):
    comptime Element = Self.T

    var data: UnsafePointer[Self.T]
    var size: Int
    var index: Int

    def __init__(out self, data: UnsafePointer[Self.T], size: Int):
        self.data = data
        self.size = size
        self.index = 0

    def __len__(self) -> Int:
        return self.size - self.index

    def __next__(mut self) raises StopIteration -> Self.T where conforms_to(
        Self.T, Movable
    ):
        if self.index >= self.size:
            raise StopIteration()
        var result = self.data.unsafe_offset(self.index).unsafe_take_pointee()
        self.index += 1
        return result^

    def __deinit__(deinit self) where conforms_to(Self.T, Deinitable):
        var i = self.index
        while i < self.size:
            self.data.unsafe_offset(i).unsafe_deinit_pointee()
            i += 1
        self.data.unsafe_free()

struct Optional[T: AnyType](
    Boolable,
    Copyable where conforms_to(T, Copyable),
    Defaultable,
    Deinitable where conforms_to(T, Deinitable),
    Equatable where conforms_to(T, Equatable),
    Hashable where conforms_to(T, Hashable),
    ImplicitlyCopyable where conforms_to(T, ImplicitlyCopyable),
    Iterable where conforms_to(T, Copyable),
    IterableOwned where conforms_to(T, Movable) and conforms_to(T, Deinitable),
    Movable where conforms_to(T, Movable),
    Writable where conforms_to(T, Writable),
):
    comptime Element = Self.T
    comptime IteratorType[
        iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]
    ] = _OptionalIter[Self.T, iterable_origin]
    comptime IteratorOwnedType = _OptionalOwnedIter[Self.T]

    var data: UnsafePointer[Self.T]
    var _size: Int

    def __init__(out self):
        self._size = 0
        self.data = unsafe_alloc[Self.T](1)

    # `None` implicitly converts to the empty Optional (upstream's convention):
    # this constructor is what makes `var x: Optional[T] = None`, `f(arg=None)`,
    # and `arg: Optional[T] = None` defaults coerce. Body mirrors the nullary
    # constructor (Mojito builds `self` by field assignment, not `self = Self()`).
    @implicit
    def __init__(out self, value: NoneType):
        self._size = 0
        self.data = unsafe_alloc[Self.T](1)

    # Upstream's `@implicit` value constructor: `var x: Optional[Int] = 5` and
    # `f(5)` for an `Optional[Int]` parameter both convert.
    @implicit
    def __init__(out self, var value: Self.T) where conforms_to(Self.T, Movable):
        self._size = 1
        self.data = unsafe_alloc[Self.T](1)
        self.data[0] = value^

    def __init__(out self, *, init_with: def() capturing[_] -> Self.T):
        self._size = 1
        self.data = unsafe_alloc[Self.T](1)
        self.data[0] = init_with()

    def __init__(out self, *, copy: Self) where conforms_to(Self.T, Copyable):
        self._size = copy._size
        self.data = unsafe_alloc[Self.T](1)
        if copy._size == 1:
            self.data.unsafe_write(copy=copy.data[0])

    def copy(self) -> Self where conforms_to(Self.T, Copyable):
        return Optional(copy: self)

    def __init__(out self, *, deinit move: Self):
        self._size = move._size
        self.data = move.data^

    def __deinit__(deinit self) where conforms_to(Self.T, Deinitable):
        if self._size == 1:
            self.data.unsafe_deinit_pointee()
        self.data.unsafe_free()

    def __bool__(self) -> Bool:
        return self._size == 1

    # `~opt` is the negated truth value (upstream's `__invert__`).
    def __invert__(self) -> Bool:
        return self._size == 0

    # `opt is None` / `opt is not None` (upstream's identity dunders).
    def __is__(self, other: NoneType) -> Bool:
        return self._size == 0

    def __isnot__(self, other: NoneType) -> Bool:
        return self._size == 1

    # `rhs` is spelled `Optional[Self.T]` rather than `Self` so the native
    # monomorphizer sees the substituted receiver type (the bare `Self`
    # spelling erases the argument list on the raw seam).
    def __eq__(self, rhs: Optional[Self.T]) -> Bool where conforms_to(Self.T, Equatable):
        if self._size == 1:
            if rhs._size == 1:
                return self.data[0] == rhs.data[0]
            return False
        return rhs._size == 0

    def __ne__(self, rhs: Optional[Self.T]) -> Bool where conforms_to(Self.T, Equatable):
        return not (self == rhs)

    # Upstream feeds a `UInt8` presence tag before the payload.
    def __hash__[H: Hasher](self, mut hasher: H) where conforms_to(Self.T, Hashable):
        if self._size == 1:
            hasher.update(UInt8(1))
            hasher.update(self.data[0])
        else:
            hasher.update(UInt8(0))

    # Writes the payload's text or `None` (upstream's `_write_to`).
    def write_to(self, mut writer: Some[Writer]) where conforms_to(Self.T, Writable):
        if self._size == 1:
            writer.write(self.data[0])
        else:
            writer.write("None")

    # Consuming, like upstream: the payload (or `default`) moves out and the
    # slot is released either way.
    def or_else(deinit self, var default: Self.T) -> Self.T where conforms_to(
        Self.T, Movable
    ) and conforms_to(Self.T, Deinitable):
        if self._size == 1:
            var result = self.data.unsafe_take_pointee()
            self.data.unsafe_free()
            return result^
        self.data.unsafe_free()
        return default^

    def value(ref self) -> ref[origin_of(self)._get_owned_interior["element"]] Self.T:
        if self._size == 0:
            _mojito_abort("Optional.value on an empty Optional")
        return self.data[0]

    # Unchecked read: an empty Optional is a deterministic abort on the VM
    # rather than upstream's undefined behavior.
    def unsafe_value(ref self) -> ref[origin_of(self)._get_owned_interior["element"]] Self.T:
        return self.data[0]

    def take(mut self) -> Self.T where conforms_to(Self.T, Movable):
        if self._size == 0:
            _mojito_abort("Optional.take on an empty Optional")
        self._size = 0
        return self.data.unsafe_take_pointee()

    def unsafe_take(mut self) -> Self.T where conforms_to(Self.T, Movable):
        self._size = 0
        return self.data.unsafe_take_pointee()

    # Iterator-bounds protocol: exactly `_size` elements.
    def bounds(self) -> Tuple[Int, Optional[Int]]:
        return (self._size, Optional[Int](self._size))

    def deinit_assert_empty(deinit self):
        if self._size == 1:
            _mojito_abort("Optional.deinit_assert_empty on a non-empty Optional")
        self.data.unsafe_free()

    def deinit_with(deinit self, elt_handler: def(var element: Self.T) capturing[_], /):
        if self._size == 1:
            elt_handler(self.data.unsafe_take_pointee())
        self.data.unsafe_free()

    def map[U: Movable](
        deinit self, f: def(var element: Self.T) capturing[_] -> U, /
    ) -> Optional[U]:
        if self._size == 1:
            var result = Optional[U](f(self.data.unsafe_take_pointee()))
            self.data.unsafe_free()
            return result^
        self.data.unsafe_free()
        return Optional[U]()

    def and_then[U: Movable](
        deinit self, f: def(var element: Self.T) capturing[_] -> Optional[U], /
    ) -> Optional[U]:
        if self._size == 1:
            var result = f(self.data.unsafe_take_pointee())
            self.data.unsafe_free()
            return result^
        self.data.unsafe_free()
        return Optional[U]()

    def __iter__(ref self) -> Self.IteratorType[origin_of(self)] where conforms_to(
        Self.T, Copyable
    ):
        ref source = self
        return _OptionalIter(source, 0)

    def __iter__(var self) -> _OptionalOwnedIter[Self.T] where conforms_to(
        Self.T, Movable
    ) and conforms_to(Self.T, Deinitable):
        var result = _OptionalOwnedIter[Self.T](self.data, self._size)
        self.data = unsafe_alloc[Self.T](0)
        self._size = 0
        return result^
