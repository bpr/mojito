# std.memory.owned_pointer — the owning single-slot smart pointer.

from std.memory.alloc import unsafe_alloc


# An owning smart pointer over one heap slot: current Mojo's `OwnedPointer`
# proof subset with its current naming from day one (`into_inner`, never the
# pre-rename `take`). Deletion conformance is conditional on the pointee's,
# so an `OwnedPointer` of a linear value is itself linear and must be
# consumed through `into_inner`.
struct OwnedPointer[T: AnyType](
    Movable,
    Deinitable where conforms_to(T, Deinitable),
):
    var _ptr: Pointer[Self.T, MutUntrackedOrigin]

    def __init__(out self, var value: Self.T, /) where conforms_to(Self.T, Movable):
        self._ptr = unsafe_alloc[Self.T](1)
        self._ptr[0] = value^

    # Upstream's borrowed dereference is the empty subscript (`p[]`), which
    # Mojito reserves for raw pointers — a recorded subset gap. Borrowed
    # access goes through `unsafe_ptr()[0]`: the interior-generation origin
    # keeps the OwnedPointer alive and stales the view when it is consumed.
    def unsafe_ptr(ref self) -> Pointer[
        Self.T, origin_of(self)._get_owned_interior["element"]
    ]:
        return self._ptr.unsafe_origin_cast[
            origin_of(self)._get_owned_interior["element"]
        ]()

    def into_inner(deinit self) -> Self.T where conforms_to(Self.T, Movable):
        var result = self._ptr.unsafe_take_pointee()
        self._ptr.unsafe_free()
        return result^

    def __deinit__(deinit self) where conforms_to(Self.T, Deinitable):
        self._ptr.unsafe_deinit_pointee()
        self._ptr.unsafe_free()
