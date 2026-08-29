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
        return self.src.data[0]

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
    Copyable where conforms_to(T, Copyable),
    Deinitable where conforms_to(T, Deinitable),
    Iterable where conforms_to(T, Copyable),
    IterableOwned where conforms_to(T, Movable) and conforms_to(T, Deinitable),
    Movable where conforms_to(T, Movable),
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

    def __init__(out self, var value: Self.T, /) where conforms_to(Self.T, Movable):
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
            self.data[0] = copy.data[0]

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

    def or_else(self, default: Self.T) -> Self.T where conforms_to(Self.T, Copyable):
        if self._size == 1:
            return self.data[0]
        return default

    def value(ref self) -> ref[origin_of(self)._get_owned_interior["element"]] Self.T:
        if self._size == 0:
            _mojito_abort("Optional.value on an empty Optional")
        return self.data[0]

    def take(mut self) -> Self.T where conforms_to(Self.T, Movable):
        if self._size == 0:
            _mojito_abort("Optional.take on an empty Optional")
        self._size = 0
        return self.data.unsafe_take_pointee()

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
