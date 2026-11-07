# std.memory.owned_pointer — the owning single-slot smart pointer.

from std.memory.alloc import unsafe_alloc


# An owning smart pointer over one heap slot, with the pinned `OwnedPointer`'s
# initializers and naming (`into_inner`, never the pre-rename `take`); the
# in-place `init_with=` initializer and `deinit_with` wait on R1825. Deletion
# conformance is conditional on the pointee's,
# so an `OwnedPointer` of a linear value is itself linear and must be
# consumed through `into_inner`.
@explicit_destroy(
    "Use `into_inner()` (for a `Movable` `T`) or `unsafe_take_allocation()`"
    " to consume an `OwnedPointer` whose element type is not"
    " `Deinitable`"
)
struct OwnedPointer[T: AnyType](
    Movable,
    Deinitable where conforms_to(T, Deinitable),
):
    var _ptr: Pointer[Self.T, MutUntrackedOrigin]

    def __init__(out self, var value: Self.T, /) where conforms_to(Self.T, Movable):
        self._ptr = unsafe_alloc[Self.T](1)
        self._ptr.unsafe_write(value^)

    @deprecated(
        "This `OwnedPointer` initializer is deprecated. Use the by value or"
        " in-place initializers."
    )
    def __init__(
        out self, *, copy_value: Self.T
    ) where conforms_to(Self.T, Copyable):
        self._ptr = unsafe_alloc[Self.T](1)
        self._ptr.unsafe_write(copy=copy_value)

    # The dummy `U` binder ranks this borrowing overload below the by-value
    # one, and implicitly copyable lvalues are excluded so they take the
    # by-value one instead of the deprecation.
    @deprecated(
        "This `OwnedPointer` initializer is deprecated. Use the by value or"
        " in-place initializers."
    )
    def __init__[
        _T: Copyable, U: NoneType = None
    ](out self: OwnedPointer[_T], value: _T) where not conforms_to(
        _T, ImplicitlyCopyable
    ):
        self._ptr = unsafe_alloc[_T](1)
        self._ptr.unsafe_write(copy=value)

    # Upstream's body is `self = {init_with = lambda () -> Self.T:
    # other[].copy()}`, which waits on the in-place initializer (R1825).
    def __init__(out self, *, other: Self) where conforms_to(Self.T, Copyable):
        self = Self(copy_value=other[])

    # The interior-generation origin keeps the OwnedPointer alive and
    # stales the reference when it is consumed.
    def __getitem__(ref self) -> ref[
        origin_of(self)._get_owned_interior["value"]
    ] Self.T:
        return self._ptr[0]

    # Upstream binds `origin` through `ref[origin] self`, and its
    # `unsafe_ptr` forwards to `ptr()`; both wait on R1300.
    def ptr(ref self) -> Pointer[Self.T, origin_of(self)]:
        return self._ptr.unsafe_origin_cast[origin_of(self)]()

    @doc_hidden
    @deprecated(use=ptr)
    def unsafe_ptr(ref self) -> Pointer[Self.T, origin_of(self)]:
        return self._ptr.unsafe_origin_cast[origin_of(self)]()

    def into_inner(deinit self) -> Self.T where conforms_to(Self.T, Movable):
        var result = self._ptr.unsafe_take_pointee()
        self._ptr.unsafe_free()
        return result^

    def __deinit__(deinit self) where conforms_to(Self.T, Deinitable):
        self._ptr.unsafe_deinit_pointee()
        self._ptr.unsafe_free()
