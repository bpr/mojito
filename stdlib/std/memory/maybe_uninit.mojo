# std.memory.maybe_uninit — inline possibly-uninitialized storage.

# Inline possibly-uninitialized storage for one T. Every method is unsafe:
# the caller tracks whether the memory is initialized. Lifecycle conformances
# mirror the pinned upstream header: moving/copying/destroying a MaybeUninit
# only touches its raw bits, never the payload's lifecycle methods, so each is
# available only for a trivially movable/copyable/deinitable payload (a
# non-trivially-deinitable payload makes the wrapper linear — unsafe_deinit()
# or unsafe_forget() it explicitly). The compiler traps deterministically
# where upstream leaves undefined behavior (reading uninitialized storage).
struct MaybeUninit[T: AnyType](
    Defaultable,
    Deinitable where (
        IsTriviallyDeinitable[T],
        "T must be trivially deinitable, since MaybeUninit never runs T's __deinit__",
    ),
    ImplicitlyCopyable where (
        IsTriviallyCopyable[T] and IsTriviallyMovable[T],
        "T must be trivially copyable and movable, since copying MaybeUninit only copies the underlying bits",
    ),
    Movable where (
        IsTriviallyMovable[T],
        "T must be trivially movable, since moving MaybeUninit only moves the underlying bits",
    ),
    RegisterPassable where (
        conforms_to(T, RegisterPassable) and IsTriviallyMovable[T]
    ),
):
    var _storage: __UninitStorage[Self.T]

    @inline(.always)
    def __init__(out self):
        self._storage = __UninitStorage[Self.T]()

    @inline(.always)
    def __init__(out self, var value: Self.T, /) where conforms_to(Self.T, Movable):
        self._storage = __UninitStorage[Self.T](value^)

    @inline(.always)
    def unsafe_write(mut self, var value: Self.T, /) where conforms_to(Self.T, Movable):
        self._storage.unsafe_write(value^)

    # Safe counterpart of unsafe_write for trivially-deinitable payloads
    # (2026-08): a trivial deinitializer is a no-op, so overwriting a live
    # value cannot leak a resource.
    @inline(.always)
    def write(
        mut self, var value: Self.T, /
    ) where IsTriviallyDeinitable[T] and conforms_to(Self.T, Movable):
        self._storage.unsafe_write(value^)

    @inline(.always)
    def unsafe_assume_init(deinit self) -> Self.T where conforms_to(Self.T, Movable):
        return self._storage^.take()

    @inline(.always)
    def unsafe_assume_init(ref self) -> ref[origin_of(self)] Self.T:
        return self._storage[0]

    @inline(.always)
    def unsafe_deinit(deinit self) where conforms_to(Self.T, Deinitable):
        self._storage^.destroy()

    @inline(.always)
    def unsafe_forget(deinit self):
        pass
