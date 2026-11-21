# std.memory.pointer — upstream's `Pointer` and its aliases, ported with
# these departures. The compiler folds every application of the struct into
# its own pointer type, and its rules still answer `Pointer(to=)`, `p[]`
# (R2208), `write`, and `unsafe_free`; a member or an initializer keyword
# no rule answers, the casts, address members, and comparisons included,
# reaches its body here. Conformances `DevicePassable`,
# `UnsafeCustomNicheStorage`, and `UnsafeSingleNicheable` are absent from
# the bundle. `unsafe_deinit_pointee_with` and `unsafe_write(init_with=)`
# take an explicit `F` binder where upstream's parameter is
# `Some[def(...)]` (R813).
# Members omitted, by blocker:
# - `__merge_with__`: its result `origin_of(Self.origin, other_type.origin)`
#   is `origin_of` over origin values (R1299), and its `other_type` bound is
#   a struct metatype (R1176).
# - the implicit mut-to-imm `__init__(other: Pointer, …)`: the compiler's
#   builtin mut-to-imm coercion must retire with it (R2443).
# - `as_unsafe_any_origin`: `UnsafeAnyOrigin[mut=Self.mut]` keeps no
#   symbolic `mut` (R2384).
# - `__bool__`: `@unavailable` on a method is rejected (R1162).
# - the memory members `write`, `unsafe_free`, `swap_pointees`,
#   `unsafe_load`, `unsafe_store`, and the strided, gather, and scatter
#   forms: the compiler's rules answer `write` and `unsafe_free`, and
#   `@__allow_legacy_custom_self_type` is not modelled (R2468).
# - the indexed and keyword `__getitem__`s and
#   `_get_ref_with_unsafe_interior_origin`: the subscript rule (R2208).
# - `unsafe_as_noalias` and `_as_origin_superset`: `where O.contains[…]`
#   (R2381); `unsafe_dangling`: the plugin hook.
# - `NicheStorage`, `write_niche`, `isa_niche`, the `DevicePassable`
#   members, and the `PythonObject` downcast initializer: their traits and
#   `PythonObject` are not bundled.
# `@stable` and `@deprecated` on each alias are omitted, since a decorator
# may not precede a `comptime` declaration yet (R1925), so the three
# `Optional*Pointer` aliases do not warn as the pin does.
"""Implements `Pointer`, Mojo's primary pointer type for indirect memory
access.

`Pointer` is safe when constructed from and used to access an existing
value. It also provides `unsafe_`-prefixed methods for working with
dynamically-allocated or uninitialized memory; the caller is responsible
for tracking initialization state and memory ownership.

You can import these APIs from the `memory` package. For example:

```mojo
from std.memory import Pointer
```
"""

from std.collections.optional import Optional
from std.builtin.int import Indexer, index
from std.sys import bit_width_of, size_of
from std.builtin.debug_assert import debug_assert
from std.memory.address_space import AddressSpace
from std.builtin.format_int import _write_int
from std.format._utils import FormatStruct, Named, TypeNames


comptime MutPointer[
    T: AnyType,
    origin: MutOrigin,
    *,
    address_space: AddressSpace = .GENERIC,
] = Pointer[T, origin, address_space=address_space]
"""A mutable pointer.

Parameters:
    T: The pointee type.
    origin: The origin of the pointer.
    address_space: The address space of the pointer.
"""


comptime ImmPointer[
    T: AnyType,
    origin: ImmOrigin,
    *,
    address_space: AddressSpace = .GENERIC,
] = Pointer[T, origin, address_space=address_space]
"""An immutable pointer.

Parameters:
    T: The pointee type.
    origin: The origin of the pointer.
    address_space: The address space of the pointer.
"""


comptime OptionalPointer[
    mut: Bool,
    //,
    T: AnyType,
    origin: Origin[mut=mut],
    *,
    address_space: AddressSpace = .GENERIC,
] = Optional[Pointer[T, origin, address_space=address_space]]
"""Deprecated: An optional (nullable) `Pointer`.

Use `Optional[Pointer[T, origin]]` instead.

Parameters:
    mut: The mutability of the pointer.
    T: The type of the pointee.
    origin: The origin of the pointer.
    address_space: The address space of the pointer.
"""


comptime OptionalMutPointer[
    T: AnyType,
    origin: MutOrigin,
    *,
    address_space: AddressSpace = .GENERIC,
] = Optional[MutPointer[T, origin, address_space=address_space]]
"""Deprecated: An optional (nullable) mutable `Pointer`.

Use `Optional[MutPointer[T, origin]]` instead.

Parameters:
    T: The pointee type.
    origin: The origin of the pointer.
    address_space: The address space of the pointer.
"""


comptime OptionalImmPointer[
    T: AnyType,
    origin: ImmOrigin,
    *,
    address_space: AddressSpace = .GENERIC,
] = Optional[ImmPointer[T, origin, address_space=address_space]]
"""Deprecated: An optional (nullable) immutable `Pointer`.

Use `Optional[ImmPointer[T, origin]]` instead.

Parameters:
    T: The pointee type.
    origin: The origin of the pointer.
    address_space: The address space of the pointer.
"""

comptime OpaquePointer[
    mut: Bool,
    //,
    origin: Origin[mut=mut],
    *,
    address_space: AddressSpace = .GENERIC,
] = Pointer[NoneType, origin, address_space=address_space]
"""An opaque pointer, equivalent to the C `(const) void*` type.

Parameters:
    mut: Whether the pointer is mutable.
    origin: The origin of the pointer.
    address_space: The address space of the pointer.
"""

comptime MutOpaquePointer[
    origin: MutOrigin,
    *,
    address_space: AddressSpace = .GENERIC,
] = OpaquePointer[origin, address_space=address_space]
"""A mutable opaque pointer, equivalent to the C `void*` type.

Parameters:
    origin: The origin of the pointer.
    address_space: The address space of the pointer.
"""

comptime ImmOpaquePointer[
    origin: ImmOrigin,
    *,
    address_space: AddressSpace = .GENERIC,
] = OpaquePointer[origin, address_space=address_space]
"""An immutable opaque pointer, equivalent to the C `const void*` type.

Parameters:
    origin: The origin of the pointer.
    address_space: The address space of the pointer.
"""


@stable(since="1.0")
struct Pointer[
    mut: Bool,
    //,
    T: AnyType,
    origin: Origin[mut=mut],
    *,
    address_space: AddressSpace = .GENERIC,
](
    Comparable,
    ImplicitlyCopyable,
    Intable,
    TrivialRegisterPassable,
    Writable,
):
    """`Pointer` represents an indirect reference to one or more values
    of type `T` consecutively in memory, and can refer to uninitialized memory.

    Parameters:
        mut: Whether the origin is mutable.
        T: The type the pointer points to.
        origin: The origin of the memory being addressed.
        address_space: The address space associated with the `Pointer` allocated memory.
    """

    comptime _mlir_lit_ref = __mlir_type[
        `!lit.ref<`,
        Self.T,
        `, `,
        Self.origin._mlir_origin,
        `, `,
        Self.address_space._value._mlir_value,
        `>`,
    ]

    comptime _mlir_type = __mlir_type[
        `!kgen.pointer<`,
        Self.T,
        `, `,
        Self.address_space._value._mlir_value,
        `>`,
    ]
    """The underlying pointer type."""

    comptime _with_origin[
        with_mut: Bool, //, with_origin: Origin[mut=with_mut]
    ] = Pointer[
        mut=with_mut, Self.T, with_origin, address_space=Self.address_space
    ]

    var _mlir_value: Self._mlir_type
    """The underlying pointer."""

    @always_inline("builtin")
    def _get_kgen_pointer(self) -> Self._mlir_type:
        """Returns the raw `kgen.pointer` MLIR value backing this pointer."""
        return self._mlir_value

    @doc_hidden
    @always_inline("builtin")
    def __init__(out self, *, _mlir_value: Self._mlir_type):
        """Create a pointer from a low-level pointer primitive.

        Args:
            _mlir_value: The MLIR value of the pointer to construct with.
        """
        self._mlir_value = _mlir_value

    @doc_hidden
    @inline(.nodebug)
    def __init__(out self, *, _mlir_value: Self._mlir_lit_ref):
        self = Self(_mlir_value=__mlir_op.`lit.ref.to_pointer`(_mlir_value))

    @inline(.always)
    def __init__(out self, *, unsafe_from_address: Int):
        """Create a pointer from a raw address.

        Args:
            unsafe_from_address: The raw address to create a pointer from.
        """
        comptime if bit_width_of[Self]() < bit_width_of[Int]():
            debug_assert(
                UInt(unsafe_from_address)
                <= (UInt(1) << UInt(bit_width_of[Self]())) - 1,
                "address ",
                unsafe_from_address,
                " does not fit in this pointer's address space",
            )
        self = Pointer(to=unsafe_from_address).unsafe_bitcast[type_of(self)]()[]

    @inline(.always)
    @doc_hidden
    def __init__(out self, *, unsafe_from_address: IntLiteral):
        """Create a pointer from a raw address.

        This checks at compile time if the address is invalid and emits a compilation error.
        """
        comptime assert type_of(unsafe_from_address)() != 0, (
            "Pointer is non-nullable. To construct a null pointer, use"
            " Optional[Pointer] to model nullability."
        )
        comptime assert (
            type_of(unsafe_from_address)() > 0
        ), "Pointer's address cannot be negative."
        self = Self(unsafe_from_address=Int(unsafe_from_address))

    @inline(.nodebug)
    @stable(since="1.3")
    def __init__(
        out self,
        *,
        ref[Self.origin, Self.address_space] to: Self.T,
    ):
        """Constructs a Pointer from a reference to a value.

        Args:
            to: The value to construct a pointer to.
        """
        self = Self(
            _mlir_value=__mlir_op.`lit.ref.to_pointer`(
                __get_mvalue_as_litref(to)
            )
        )

    @__unsafe_nested_origins_read_only
    @inline(.nodebug)
    def __getitem__(self) -> ref[Self.origin, Self.address_space] Self.T:
        """Return a reference to the underlying data.

        Returns:
            A reference to the value.
        """
        return __get_litref_as_mvalue(
            __mlir_op.`lit.ref.from_pointer`[
                _type=Self._mlir_lit_ref, name="(pointer pointee)".value
            ](self._mlir_value)
        )

    @doc_hidden
    @inline(.nodebug)
    @deprecated(use=unsafe_offset)
    def __add__[I: Indexer, //](self, offset: I) -> Self:
        """Return a pointer at an offset from the current one.

        Parameters:
            I: A type that can be used as an index.

        Args:
            offset: The offset index.

        Returns:
            An offset pointer.
        """
        return self.unsafe_offset(offset)

    @doc_hidden
    @__unsafe_nested_origins_read_only
    @inline(.always)
    def __sub__(
        self,
        rhs: Pointer[Self.T, _, address_space=Self.address_space],
    ) -> Int:
        """Returns the signed distance from `rhs` to `self` in elements of
        `T` (not bytes), such that `rhs.unsafe_offset(result)` produces a
        pointer equal to `self`.

        This is the operator form of `offset_from()`. Its documentation is
        duplicated here because LSP hover shows this operator's own
        docstring rather than the method's; keep the two in sync.

        Constraints:
            The pointee type `T` must not be zero-sized.

        Args:
            rhs: The pointer to measure the distance from.

        Returns:
            The signed element distance from `rhs` to `self`.

        Safety:

        - Both pointers must point into the same allocation (or one past
          its end); the distance between pointers into unrelated
          allocations is not meaningful.
        - The byte distance between the two pointers must be an exact
          multiple of `size_of[T]()`. Violating this is undefined
          behavior.

        Examples:

        ```mojo
        from std.memory.alloc import alloc, dealloc, Layout

        var allocation = alloc(Layout[Int32](length=4))
        var ptr = allocation.unsafe_ptr()
        var end = ptr.unsafe_offset(3)
        print(end - ptr)  # => 3
        print(ptr - end)  # => -3
        dealloc(allocation^)
        ```
        """
        return self.offset_from(rhs)

    @__unsafe_nested_origins_read_only
    @inline(.nodebug)
    @stable(since="1.3")
    def __eq__(
        self,
        rhs: Pointer[Self.T, _, address_space=Self.address_space],
    ) -> Bool:
        """Returns True if the two pointer addresses are equal.

        Args:
            rhs: The other pointer.

        Returns:
            True if the two pointer addresses are equal and False otherwise.
        """
        return Int(self) == Int(rhs)

    @__unsafe_nested_origins_read_only
    @inline(.nodebug)
    @stable(since="1.3")
    def __eq__(self, rhs: Self) -> Bool:
        """Returns True if the two pointers are equal.

        Args:
            rhs: The value of the other pointer.

        Returns:
            True if the two pointers are equal and False otherwise.
        """
        return Int(self) == Int(rhs)

    @__unsafe_nested_origins_read_only
    @inline(.nodebug)
    @stable(since="1.3")
    def __ne__(
        self,
        rhs: Pointer[Self.T, _, address_space=Self.address_space],
    ) -> Bool:
        """Returns True if the two pointers are not equal.

        Args:
            rhs: The value of the other pointer.

        Returns:
            True if the two pointers are not equal and False otherwise.
        """
        return not (self == rhs)

    @__unsafe_nested_origins_read_only
    @inline(.nodebug)
    @stable(since="1.3")
    def __ne__(self, rhs: Self) -> Bool:
        """Returns True if the two pointers are not equal.

        Args:
            rhs: The value of the other pointer.

        Returns:
            True if the two pointers are not equal and False otherwise.
        """
        return not (self == rhs)

    @__unsafe_nested_origins_read_only
    @inline(.nodebug)
    def __lt__(
        self,
        rhs: Pointer[Self.T, _, address_space=Self.address_space],
    ) -> Bool:
        """Returns True if this pointer represents a lower address than rhs.

        Args:
            rhs: The value of the other pointer.

        Returns:
            True if this pointer represents a lower address and False otherwise.
        """
        return Int(self) < Int(rhs)

    @__unsafe_nested_origins_read_only
    @inline(.nodebug)
    def __lt__(self, rhs: Self) -> Bool:
        """Returns True if this pointer represents a lower address than rhs.

        Args:
            rhs: The value of the other pointer.

        Returns:
            True if this pointer represents a lower address and False otherwise.
        """
        return Int(self) < Int(rhs)

    @__unsafe_nested_origins_read_only
    @inline(.nodebug)
    def __le__(
        self,
        rhs: Pointer[Self.T, _, address_space=Self.address_space],
    ) -> Bool:
        """Returns True if this pointer represents a lower than or equal
           address than rhs.

        Args:
            rhs: The value of the other pointer.

        Returns:
            True if this pointer represents a lower address and False otherwise.
        """
        return Int(self) <= Int(rhs)

    @__unsafe_nested_origins_read_only
    @inline(.nodebug)
    def __le__(self, rhs: Self) -> Bool:
        """Returns True if this pointer represents a lower than or equal
           address than rhs.

        Args:
            rhs: The value of the other pointer.

        Returns:
            True if this pointer represents a lower address and False otherwise.
        """
        return Int(self) <= Int(rhs)

    @__unsafe_nested_origins_read_only
    @inline(.nodebug)
    def __gt__(
        self,
        rhs: Pointer[Self.T, _, address_space=Self.address_space],
    ) -> Bool:
        """Returns True if this pointer represents a higher address than rhs.

        Args:
            rhs: The value of the other pointer.

        Returns:
            True if this pointer represents a higher than or equal address and
            False otherwise.
        """
        return Int(self) > Int(rhs)

    @__unsafe_nested_origins_read_only
    @inline(.nodebug)
    def __gt__(self, rhs: Self) -> Bool:
        """Returns True if this pointer represents a higher address than rhs.

        Args:
            rhs: The value of the other pointer.

        Returns:
            True if this pointer represents a higher than or equal address and
            False otherwise.
        """
        return Int(self) > Int(rhs)

    @__unsafe_nested_origins_read_only
    @inline(.nodebug)
    def __ge__(
        self,
        rhs: Pointer[Self.T, _, address_space=Self.address_space],
    ) -> Bool:
        """Returns True if this pointer represents a higher than or equal
           address than rhs.

        Args:
            rhs: The value of the other pointer.

        Returns:
            True if this pointer represents a higher than or equal address and
            False otherwise.
        """
        return Int(self) >= Int(rhs)

    @__unsafe_nested_origins_read_only
    @inline(.nodebug)
    def __ge__(self, rhs: Self) -> Bool:
        """Returns True if this pointer represents a higher than or equal
           address than rhs.

        Args:
            rhs: The value of the other pointer.

        Returns:
            True if this pointer represents a higher than or equal address and
            False otherwise.
        """
        return Int(self) >= Int(rhs)

    @inline(.always)
    def __int__(self) -> Int:
        """Returns the pointer address as an integer.

        Returns:
          The address of the pointer as an Int.
        """
        return Int(
            mlir_value=__mlir_op.`pop.pointer_to_index`(self._mlir_value)
        )

    @inline(.nodebug)
    def unsafe_offset[I: Indexer](self, offset: I, /) -> Self:
        """Return a pointer at an offset from the current one.

        Parameters:
            I: A type that can be used as an index.

        Args:
            offset: The offset index.

        Returns:
            An offset pointer.
        """
        return {
            _mlir_value = __mlir_op.`pop.offset`(
                self._mlir_value, index(offset)._mlir_value
            )
        }

    @__unsafe_nested_origins_read_only
    @inline(.always)
    def offset_from(
        self,
        other: Pointer[Self.T, _, address_space=Self.address_space],
    ) -> Int:
        """Returns the signed distance from `other` to `self` in elements of
        `T` (not bytes), such that `other.unsafe_offset(result)` produces a
        pointer equal to `self`.

        Constraints:
            The pointee type `T` must not be zero-sized.

        Args:
            other: The pointer to measure the distance from.

        Returns:
            The signed element distance from `other` to `self`.

        Safety:

        - Both pointers must point into the same allocation (or one past
          its end); the distance between pointers into unrelated
          allocations is not meaningful.
        - The byte distance between the two pointers must be an exact
          multiple of `size_of[T]()`. Violating this is undefined
          behavior.

        Examples:

        ```mojo
        from std.memory.alloc import alloc, dealloc, Layout

        var allocation = alloc(Layout[Int32](length=4))
        var ptr = allocation.unsafe_ptr()
        var end = ptr.unsafe_offset(3)
        print(end.offset_from(ptr))  # => 3
        print(ptr - end)  # => -3
        dealloc(allocation^)
        ```
        """
        comptime assert (
            size_of[Self.T]() > 0
        ), "offset_from() requires a non-zero-sized pointee type"
        var byte_diff = Int(self) - Int(other)
        debug_assert(
            byte_diff % size_of[Self.T]() == 0,
            "pointer difference is not a multiple of the element size",
        )
        return Int(
            mlir_value=__mlir_op.`pop.div`[isExact=__mlir_attr.`unit`](
                byte_diff._mlir_value, size_of[Self.T]()._mlir_value
            )
        )

    @always_inline("builtin")
    @stable(since="1.3")
    def unsafe_bitcast[
        U: AnyType, /
    ](self) -> Pointer[U, Self.origin, address_space=Self.address_space]:
        """Bitcasts a Pointer to a different type.

        Parameters:
            U: The target type.

        Returns:
            A new Pointer object with the specified type and the same address,
            as the original Pointer.
        """
        return {
            _mlir_value = __mlir_op.`pop.pointer.bitcast`[
                _type=Pointer[
                    U, Self.origin, address_space=Self.address_space
                ]._mlir_type,
            ](self._mlir_value)
        }

    comptime _OriginCastType[
        target_mut: Bool, //, target_origin: Origin[mut=target_mut]
    ] = Pointer[Self.T, target_origin, address_space=Self.address_space]

    @always_inline("builtin")
    def unsafe_mut_cast[
        target_mut: Bool, /
    ](self) -> Self._OriginCastType[Self.origin.unsafe_mut_cast[target_mut]()]:
        """Changes the mutability of a pointer.

        Parameters:
            target_mut: Mutability of the destination pointer.

        Returns:
            A pointer with the same type, origin and address space as the
            original pointer, but with the newly specified mutability.
        """
        return {
            _mlir_value = __mlir_op.`pop.pointer.bitcast`[
                _type=Self._OriginCastType[
                    Self.origin.unsafe_mut_cast[target_mut]()
                ]._mlir_type,
            ](self._mlir_value)
        }

    @always_inline("builtin")
    def unsafe_origin_cast[
        target_origin: Origin[mut=Self.mut]
    ](self) -> Self._OriginCastType[target_origin]:
        """Changes the origin of a pointer.

        Parameters:
            target_origin: Origin of the destination pointer.

        Returns:
            A pointer with the same type, mutability and address space as the
            original pointer, but with the newly specified origin.
        """
        return {
            _mlir_value = __mlir_op.`pop.pointer.bitcast`[
                _type=Self._OriginCastType[target_origin]._mlir_type,
            ](self._mlir_value)
        }

    @always_inline("builtin")
    def as_imm(
        self,
    ) -> Self._OriginCastType[ImmOrigin(Self.origin)]:
        """Changes the mutability of a pointer to immutable.

        Returns:
            A pointer with the mutability set to immutable.
        """
        return self.unsafe_mut_cast[False]()

    @always_inline("builtin")
    def unsafe_address_space_cast[
        target_address_space: AddressSpace = Self.address_space,
    ](self) -> Pointer[Self.T, Self.origin, address_space=target_address_space]:
        """Casts an Pointer to a different address space.

        Parameters:
            target_address_space: The address space of the result.

        Returns:
            A new Pointer object with the same type and the same address,
            as the original Pointer and the new address space.
        """
        return {
            _mlir_value = __mlir_op.`pop.pointer.bitcast`[
                _type=Pointer[
                    Self.T, Self.origin, address_space=target_address_space
                ]._mlir_type,
            ](self._mlir_value)
        }

    @inline(.always)
    def unsafe_deinit_pointee(
        self,
    ) where (
        Self.mut
        and Self.address_space == .GENERIC
        and conforms_to(Self.T, Deinitable)
    ):
        """Destroys the pointed-to value.

        This is equivalent to `_ = self.unsafe_take_pointee()` but doesn't
        require `Movable` and is more efficient because it doesn't invoke a
        move constructor.

        Safety:

        - This runs the pointee's deinitializer and leaves the pointee
          memory uninitialized. Subsequent reads of this pointer are
          invalid until a new valid value is written using an
          `unsafe_write()` method.
        - `self` must point to a valid, initialized instance of `T`.
          Calling this on a pointer to uninitialized memory is undefined
          behavior.
        """
        var this = self.unsafe_address_space_cast[.GENERIC]()
        _ = __get_address_as_owned_value(this._mlir_value)

    @inline(.always)
    def unsafe_deinit_pointee_with[
        F: def(var Self.T)
    ](self, deinit_func: F, /) where Self.mut and Self.address_space == .GENERIC:
        """Destroys the pointed-to value using a user-provided deinitializer
        function.

        This can be used to destroy non-`Deinitable` values in-place
        without moving.

        Args:
            deinit_func: A function that takes ownership of the pointee value
                for the purpose of deinitializing it.

        Safety:

        - This runs `deinit_func` on the pointee and leaves the pointee
          memory uninitialized. Subsequent reads of this pointer are
          invalid until a new valid value is written using an
          `unsafe_write()` method.
        - `self` must point to a valid, initialized instance of
          `Self.T`. Calling this on a pointer to uninitialized memory
          is undefined behavior.
        """
        var this = self.unsafe_address_space_cast[.GENERIC]()
        deinit_func(__get_address_as_owned_value(this._mlir_value))

    @inline(.always)
    def unsafe_take_pointee(
        self,
    ) -> Self.T where (
        Self.mut
        and Self.address_space == .GENERIC
        and conforms_to(Self.T, Movable)
    ):
        """Move the value at the pointer out, leaving it uninitialized.

        This performs a _consuming_ move, ending the origin of the value stored
        in this pointer memory location. Subsequent reads of this pointer are
        not valid. If a new valid value is stored using `unsafe_write()`, then
        reading from this pointer becomes valid again.

        Returns:
            The value at the pointer.

        Safety:

        - `self` must point to a valid, initialized instance of `Self.T`.
          Calling this on a pointer to uninitialized memory is undefined
          behavior.
        - This moves the pointee out without running its destructor and
          leaves the pointee memory uninitialized. Subsequent reads of this
          pointer are invalid until a new valid value is written using an
          `unsafe_write()` method.
        """
        return __get_address_as_owned_value(
            self.unsafe_address_space_cast[.GENERIC]()._mlir_value
        )

    @inline(.always)
    def unsafe_write[
        F: def() -> Self.T
    ](
        self,
        *,
        init_with: F,
    ) where Self.mut and Self.address_space == .GENERIC:
        """Initializes the pointee in place using the value returned by `f`.

        The value returned by `init_with` is constructed directly into the pointer's
        memory location rather than being constructed elsewhere and then
        moved, so `Self.T` does not need to be `Movable`.

        Example:

        ```mojo
        from std.memory.alloc import alloc, dealloc, Layout

        @fieldwise_init
        struct Pinned(not Movable):
            var value: Int

        var allocation = alloc(Layout[Pinned].single())
        var ptr = allocation.unsafe_ptr()
        ptr.unsafe_write(init_with=lambda () -> Pinned: Pinned(7))
        print(ptr[].value)  # => 7
        ptr.unsafe_deinit_pointee()
        dealloc(allocation^)
        ```

        Args:
            init_with: A function that constructs and returns the value to emplace.

        Safety:

        - `self` must point to writable memory for `Self.T` that does not
          currently hold a valid, live value. Writing into memory that
          already holds one overwrites it without running its destructor,
          leaking any resources it owned.
        """
        __get_address_as_uninit_lvalue(
            self.unsafe_address_space_cast[.GENERIC]()._mlir_value
        ) = init_with()

    @inline(.always)
    def unsafe_write(
        self, var value: Self.T, /
    ) where (
        Self.mut
        and Self.address_space == .GENERIC
        and conforms_to(Self.T, Movable)
    ):
        """Write `value` into the pointer location, moving from `value`.

        The pointer memory location is assumed to contain uninitialized data,
        and consequently the current contents of this pointer are not
        deinitialized before writing `value`.

        Example:

        ```mojo
        from std.memory.alloc import alloc, dealloc, Layout

        var allocation = alloc(Layout[String].single())
        var ptr = allocation.unsafe_ptr()
        ptr.unsafe_write("foo")
        print(ptr[])  # => foo
        ptr.unsafe_deinit_pointee()
        dealloc(allocation^)
        ```

        If `Self.T` is trivially deinitializable (for example, `Int`), prefer
        `write()` instead: it overwrites safely, since there's no
        destructor that a previous value could leak.

        Args:
            value: The value to emplace.

        Safety:

        - `self` must point to writable memory for `Self.T` that does not
          currently hold a valid, live value. Writing into memory that
          already holds one overwrites it without running its destructor,
          leaking any resources it owned.
        """
        __get_address_as_uninit_lvalue(
            self.unsafe_address_space_cast[.GENERIC]()._mlir_value
        ) = (value^)

    @inline(.always)
    def unsafe_write(
        self, *, copy: Self.T
    ) where (
        Self.mut
        and Self.address_space == .GENERIC
        and conforms_to(Self.T, Copyable)
    ):
        """Write a copy of `copy` into the pointer location.

        The pointer memory location is assumed to contain uninitialized data,
        and consequently the current contents of this pointer are not deinitialized
        before writing the copy.

        When compared to calling the positional `unsafe_write()` overload with
        a value you copied yourself, this avoids an extra move on the callee
        side.

        Args:
            copy: The value to copy.

        Safety:

        - `self` must point to writable memory for `Self.T` that does not
          currently hold a valid, live value. Writing into memory that
          already holds one overwrites it without running its destructor,
          leaking any resources it owned.
        """
        __get_address_as_uninit_lvalue(
            self.unsafe_address_space_cast[.GENERIC]()._mlir_value
        ) = copy.copy()

    @inline(.always)
    def unsafe_write_move_from(
        self, src: Pointer[Self.T, _]
    ) where (
        Self.mut
        and Self.address_space == .GENERIC
        and type_of(src).mut
        and conforms_to(Self.T, Movable)
    ):
        """Moves the value `src` points to into the memory location pointed to
        by `self`.

        The `self` pointer memory location is assumed to contain uninitialized
        data prior to this assignment, and consequently the current contents of
        this pointer are not destructed before writing the value from the `src`
        pointer.

        Ownership of the value is logically transferred from `src` into `self`'s
        pointer location.

        After this call, the `src` pointee value should be treated as
        uninitialized data. Subsequent reads of or destructor calls on the `src`
        pointee value are invalid, unless and until a new valid value has been
        written into the `src` pointer's memory location using an
        `unsafe_write()` method.

        This transfers the value out of `src` and into `self` using at most one
        move constructor call.

        Example:

        ```mojo
        from std.memory.alloc import alloc, dealloc, Layout

        var a_allocation = alloc(Layout[String].single())
        var b_allocation = alloc(Layout[String].single())
        var a_ptr = a_allocation.unsafe_ptr()
        var b_ptr = b_allocation.unsafe_ptr()

        # Initialize A pointee
        a_ptr.unsafe_write("foo")

        # Perform the move
        b_ptr.unsafe_write_move_from(a_ptr)

        # Clean up
        b_ptr.unsafe_deinit_pointee()
        dealloc(a_allocation^)
        dealloc(b_allocation^)
        ```

        Safety:

        * `src` must point to a valid, initialized instance of `Self.T`.
        * `self` must point to writable memory for `Self.T`. The pointee
          contents of `self` should be uninitialized; if `self` was previously
          written with a valid value, that value will be overwritten and its
          destructor will NOT be run.

        Args:
            src: Source pointer that the value will be moved from.
        """
        __get_address_as_uninit_lvalue(
            self.unsafe_address_space_cast[.GENERIC]()._mlir_value
        ) = __get_address_as_owned_value(src._mlir_value)

    @inline(.never)
    def write_to(self, mut writer: Some[Writer]):
        """Formats this pointer address to the provided Writer.

        Args:
            writer: The object to write to.
        """
        _write_int[radix=16](writer, Int(Int(self)), prefix="0x")

    @inline(.never)
    def write_repr_to(self, mut writer: Some[Writer]):
        """Write the string representation of the Pointer.

        Args:
            writer: The object to write to.
        """
        FormatStruct(writer, "Pointer").params(
            Named("mut", Self.mut),
            TypeNames[Self.T](),
            Named("address_space", Self.address_space),
        ).fields(self)
