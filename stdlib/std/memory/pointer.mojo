# std.memory.pointer — upstream's `Pointer` and its aliases, ported with
# these departures. The compiler folds every application of the struct into
# its own pointer type, and its rules still answer `Pointer(to=)`, `p[]`,
# the casts, the address members, and the operators (R2206–R2208); a member
# or an initializer keyword no rule answers reaches its body here. Conformances
# `DevicePassable`, `UnsafeCustomNicheStorage`, and `UnsafeSingleNicheable`
# are absent from the bundle, and `Comparable` waits on the members below.
# Members omitted, by blocker:
# - `__merge_with__`: a `Pointer` cannot hold the union
#   `origin_of(Self.origin, other_type.origin)` (R2429).
# - the implicit mut-to-imm `__init__(other: Pointer, …)`: a bare `Pointer`
#   parameter's origin is unsafe-any, so `ImmOrigin(other.origin)` is
#   rejected (R2419).
# - `as_unsafe_any_origin`: `UnsafeAnyOrigin[mut=Self.mut]` keeps no
#   symbolic `mut` (R2384).
# - `write_repr_to`: `Named("mut", Self.mut)` binds a compile-time value to
#   a `ref` argument (R2425).
# - `__bool__`: `@unavailable` on a method is rejected (R1162).
# - the comparisons, `__add__`, `__sub__`, `offset_from`, the memory
#   members (`unsafe_write*`, `unsafe_take_pointee`, `unsafe_deinit_pointee*`,
#   `unsafe_free`, `swap_pointees`, `unsafe_load`, `unsafe_store`, and the
#   strided, gather, and scatter forms): the compiler's rules answer them
#   (R2207), and `@__allow_legacy_custom_self_type` is not modelled.
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
from std.sys import bit_width_of
from std.builtin.debug_assert import debug_assert
from std.memory.address_space import AddressSpace
from std.builtin.format_int import _write_int


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

    @inline(.never)
    def write_to(self, mut writer: Some[Writer]):
        """Formats this pointer address to the provided Writer.

        Args:
            writer: The object to write to.
        """
        _write_int[radix=16](writer, Int(Int(self)), prefix="0x")
