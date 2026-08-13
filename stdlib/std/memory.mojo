# std.memory — the current layout-based allocation model.
#
# `alloc(Layout[T](count=n))` returns an `Allocation[T]` that owns its heap
# storage through a `ThinAllocation[T]` and retains the `Layout[T]` used to
# allocate it; `dealloc(allocation^)` releases it. Both owners are linear
# (`Deinitable where False`): implicit drop is a checker error, so every
# allocation is explicitly deallocated or explicitly leaked. `unsafe_alloc`
# is the raw-pointer migration spelling of the deprecated layout-less
# `alloc[T](count)`.
#
# The heap primitive is the compiler's static allocation surface, reachable
# only from bundled standard-library sources; user code allocates through
# this module. Alignment 0 means the element's natural alignment; the VM
# validates an explicit alignment when the storage is reserved.

struct Layout[T: AnyType](ImplicitlyCopyable, Movable):
    var _count: Int
    var _alignment: Int

    def __init__(out self, *, count: Int):
        self._count = count
        self._alignment = 0

    def __init__(out self, *, count: Int, alignment: Int):
        self._count = count
        self._alignment = alignment

    def count(self) -> Int:
        return self._count


@explicit_destroy("a ThinAllocation owns heap storage; free it through dealloc/unsafe_free or leak it explicitly")
struct ThinAllocation[T: AnyType](Movable, Deinitable where False):
    var _ptr: Pointer[Self.T, MutUntrackedOrigin]

    def __init__(out self, *, unsafe_owned_ptr: Pointer[Self.T, MutUntrackedOrigin]):
        self._ptr = unsafe_owned_ptr

    def unsafe_ptr(self) -> Pointer[Self.T, MutUntrackedOrigin]:
        return self._ptr

    def unsafe_leak(deinit self) -> Pointer[Self.T, MutUntrackedOrigin]:
        return self._ptr

    def unsafe_with_layout(var self, layout: Layout[Self.T]) -> Allocation[Self.T]:
        return Allocation[Self.T](_alloc=self^, _layout=layout)


@explicit_destroy("an Allocation owns heap storage; release it with dealloc(allocation^) or leak it explicitly")
struct Allocation[T: AnyType](Movable, Deinitable where False):
    var _alloc: ThinAllocation[Self.T]
    var _layout: Layout[Self.T]

    def __init__(out self, *, var _alloc: ThinAllocation[Self.T], _layout: Layout[Self.T]):
        self._alloc = _alloc^
        self._layout = _layout

    def unsafe_ptr(self) -> Pointer[Self.T, MutUntrackedOrigin]:
        return self._alloc.unsafe_ptr()

    def layout(self) -> Layout[Self.T]:
        return self._layout

    def into_thin(deinit self) -> ThinAllocation[Self.T]:
        return self._alloc^

    def unsafe_leak(deinit self) -> Pointer[Self.T, MutUntrackedOrigin]:
        return self._alloc^.unsafe_leak()


def alloc[T: AnyType](layout: Layout[T], /) -> Allocation[T]:
    var thin = ThinAllocation[T](unsafe_owned_ptr=_RawAlloc[T](layout._count, layout._alignment).ptr)
    return thin^.unsafe_with_layout(layout)


def dealloc[T: AnyType](var allocation: Allocation[T], /):
    var ptr = allocation^.unsafe_leak()
    ptr.unsafe_free()


def unsafe_alloc[T: AnyType](count: Int, *, alignment: Int = 0) -> Pointer[T, MutUntrackedOrigin]:
    return _RawAlloc[T](count, alignment).ptr


# Inline possibly-uninitialized storage for one T. Every method is unsafe:
# the caller tracks whether the memory is initialized. The destructor is a
# no-op — discarding an initialized value leaks it (call unsafe_deinit()
# first); moving/copying transfers the raw bits, so both are available only
# for trivially movable/copyable payloads. The compiler traps deterministically
# where upstream leaves undefined behavior (reading uninitialized storage).
struct UnsafeMaybeUninit[T: AnyType](
    Defaultable,
    Movable where IsTriviallyMovable[T],
    ImplicitlyCopyable where IsTriviallyCopyable[T],
    RegisterPassable where conforms_to(T, RegisterPassable),
):
    var _storage: __UninitStorage[Self.T]

    def __init__(out self):
        self._storage = __UninitStorage[Self.T]()

    def __init__(out self, var value: Self.T, /) where conforms_to(Self.T, Movable):
        self._storage = __UninitStorage[Self.T](value^)

    def unsafe_write(mut self, var value: Self.T, /) where conforms_to(Self.T, Movable):
        self._storage.unsafe_write(value^)

    def unsafe_assume_init(deinit self) -> Self.T where conforms_to(Self.T, Movable):
        return self._storage^.take()

    def unsafe_assume_init(ref self) -> ref[origin_of(self)] Self.T:
        return self._storage[0]

    def unsafe_deinit(deinit self) where conforms_to(Self.T, Deinitable):
        self._storage^.destroy()

    def unsafe_forget(deinit self):
        pass


# The single crossing to the compiler's heap primitive. A constructor rather
# than a free helper because expression-position `UnsafePointer[T]` with a
# bare generic parameter parses as runtime indexing; `Self.T` in a struct
# body does not.
struct _RawAlloc[T: AnyType](Movable):
    var ptr: Pointer[Self.T, MutUntrackedOrigin]

    def __init__(out self, count: Int, alignment: Int):
        if alignment == 0:
            self.ptr = UnsafePointer[Self.T].alloc(count)
        else:
            self.ptr = UnsafePointer[Self.T].alloc_aligned(count, alignment)
