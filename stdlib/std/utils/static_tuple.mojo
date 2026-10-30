# Upstream's `std/utils/static_tuple.mojo`, in its order, with these
# departures:
# - `comptime _StaticTupleTraits` is omitted and each bound spells its
#   composition out, since a trait composition is no `comptime` binding
#   (R1171).
# - The run-time `assert`s of `__init__(*elems)`, `__getitem__`, and
#   `__setitem__` are omitted, since the `assert` statement does not parse
#   (R1598); the pin's default `-D ASSERT` mode skips them.
# - The `DevicePassable` conformance, `_DeviceElementType`, `device_type`,
#   `_to_device_type`, and `get_type_name` are omitted with their
#   `std.builtin.device_passable` and `std.builtin.rebind` imports: a static
#   trait requirement is rejected (R942) and `downcast` is not bundled
#   (R223).
# - `String` and `index` are imported, which upstream's prelude provides,
#   since a module `std.simd` imports sees no prelude names (R894).
"""Implements StaticTuple, a statically-sized uniform container.

You can import these APIs from the `utils` package. For example:

```mojo
from std.utils import StaticTuple
```
"""

from std.builtin.int import index
from std.reflection import reflect
from std.string import String
from std.traits import (
    IsTriviallyCopyable,
    IsTriviallyDeinitable,
    IsTriviallyMovable,
)

# ===-----------------------------------------------------------------------===#
# StaticTuple
# ===-----------------------------------------------------------------------===#


def _static_tuple_construction_checks[
    T: ImplicitlyCopyable & Deinitable & RegisterPassable, size: Int
]():
    """Checks if the properties in `StaticTuple` are valid.

    Validity right now is just ensuring the number of elements is > 0.

    Parameters:
      T: The StaticTuple's element type.
      size: The number of elements.
    """
    comptime assert (
        IsTriviallyMovable[T]
        and IsTriviallyCopyable[T]
        and IsTriviallyDeinitable[T]
    ), String(
        (
            "`StaticTuple` element type must have a trivial move/copy"
            " constructor and destructor: "
        ),
        reflect[T].name(),
    )
    comptime assert (
        size >= 0
    ), "number of elements in `StaticTuple` must be >= 0"


struct StaticTuple[
    element_type: ImplicitlyCopyable & Deinitable & RegisterPassable, size: Int
](Defaultable, Sized, TrivialRegisterPassable):
    """A statically sized tuple type which contains elements of homogeneous types.

    Parameters:
        element_type: The type of the elements in the tuple.
        size: The size of the tuple.
    """

    comptime _mlir_type = __mlir_type[
        `!pop.array<`, Self.size.__mlir_index__(), `, `, Self.element_type, `>`
    ]

    var _mlir_value: Self._mlir_type
    """The underlying storage for the static tuple."""

    @inline(.always)
    def __init__(out self):
        """Constructs an empty (undefined) tuple."""
        _static_tuple_construction_checks[Self.element_type, Self.size]()
        __mlir_op.`lit.ownership.mark_initialized`(__get_mvalue_as_litref(self))

    @inline(.always)
    def __init__(out self, *, mlir_value: Self._mlir_type):
        """Constructs from an array type.

        Args:
            mlir_value: Underlying MLIR array type.
        """
        _static_tuple_construction_checks[Self.element_type, Self.size]()
        self._mlir_value = mlir_value

    @inline(.always)
    def __init__(out self, *, fill: Self.element_type):
        """Constructs a static tuple given a fill value.

        Args:
            fill: The value to fill the tuple with.
        """
        _static_tuple_construction_checks[Self.element_type, Self.size]()
        self._mlir_value = __mlir_op.`pop.array.repeat`[
            _type=__mlir_type[
                `!pop.array<`,
                Self.size.__mlir_index__(),
                `, `,
                Self.element_type,
                `>`,
            ]
        ](fill)

    @inline(.always)
    def __init__(out self, *elems: Self.element_type):
        """Constructs a static tuple given a set of arguments.

        Args:
            elems: The element types.
        """
        _static_tuple_construction_checks[Self.element_type, Self.size]()
        if len(elems) == 1:
            return Self(fill=elems[0])

        self = Self()
        comptime for idx in range(Self.size):
            self[idx] = elems[idx]

    @inline(.always)
    def __init__[*values: Self.element_type](out self):
        """Creates a tuple constant using the specified values.

        Parameters:
            values: The list of values.
        """
        _static_tuple_construction_checks[Self.element_type, Self.size]()

        comptime num_values = values.size
        if num_values == 1:
            return Self(fill=values[0])

        comptime assert (
            Self.size == num_values
        ), "mismatch in the number of elements"
        self = Self()
        comptime for idx in range(Self.size):
            self[idx] = values[idx]

    @inline(.nodebug)
    def __len__(self) -> Int:
        """Returns the length of the array. This is a known constant value.

        Returns:
            The size of the list.
        """
        return Self.size

    @inline(.nodebug)
    def __getitem__[I: Indexer, //](self, idx: I) -> Self.element_type:
        """Returns the value of the tuple at the given dynamic index.

        Parameters:
            I: A type that can be used as an index.

        Args:
            idx: The index into the tuple.

        Returns:
            The value at the specified position.
        """
        return self._unsafe_ref(index(idx))

    @inline(.nodebug)
    def __setitem__[I: Indexer, //](mut self, idx: I, val: Self.element_type):
        """Stores a single value into the tuple at the specified dynamic index.

        Parameters:
            I: A type that can be used as an index.

        Args:
            idx: The index into the tuple.
            val: The value to store.
        """
        self._unsafe_ref(index(idx)) = val

    @inline(.nodebug)
    def get[index: Int](self) -> Self.element_type:
        """Returns the value of the tuple at the given index.

        Parameters:
            index: The index into the tuple.

        Returns:
            The value at the specified position.
        """
        comptime assert index < Self.size
        var val = __mlir_op.`pop.array.get`[
            _type=Self.element_type,
            index=index.__mlir_index__(),
        ](self._mlir_value)
        return val

    @inline(.nodebug)
    def _unsafe_ref(ref self, idx: Int) -> ref[self] Self.element_type:
        var ptr = __mlir_op.`pop.array.gep`(
            Pointer(to=self._mlir_value)._get_kgen_pointer(),
            idx.__mlir_index__(),
        )
        return Pointer[origin=origin_of(self)](_mlir_value=ptr)[]

    @inline(.nodebug)
    def _replace[idx: Int](self, val: Self.element_type) -> Self:
        """Replaces the value at the specified index.

        Parameters:
            idx: The index into the tuple.

        Args:
            val: The value to store.

        Returns:
            A new tuple with the specified element value replaced.
        """
        comptime assert idx < Self.size

        var array = __mlir_op.`pop.array.replace`[
            _type=__mlir_type[
                `!pop.array<`,
                Self.size.__mlir_index__(),
                `, `,
                Self.element_type,
                `>`,
            ],
            index=idx.__mlir_index__(),
        ](val, self._mlir_value)

        return Self(mlir_value=array)

    @inline(.always)
    def __eq__(
        self, other: Self
    ) -> Bool where conforms_to(
        Self.element_type, Equatable & TrivialRegisterPassable
    ):
        """Returns `True` if all elements are equal.

        Args:
            other: The tuple to compare with.

        Returns:
            True if all corresponding elements are equal.
        """
        # TODO(MOCO-4667): Compare `self[i] != other[i]` directly once a
        # subscript result used as an operand keeps the `where` refinement.
        comptime for i in range(Self.size):
            var lhs = self[i]
            var rhs = other[i]
            if lhs != rhs:
                return False
        return True

    @inline(.always)
    def __ne__(
        self, other: Self
    ) -> Bool where conforms_to(
        Self.element_type, Equatable & TrivialRegisterPassable
    ):
        """Returns `True` if any element differs.

        Args:
            other: The tuple to compare with.

        Returns:
            True if any corresponding elements differ.
        """
        return not (self == other)

    @inline(.always)
    def __lt__(
        self, other: Self
    ) -> Bool where conforms_to(
        Self.element_type, Comparable & TrivialRegisterPassable
    ):
        """Returns `True` if `self` is lexicographically less than `other`.

        Args:
            other: The tuple to compare with.

        Returns:
            True if `self` is lexicographically less than `other`.
        """
        # TODO(MOCO-4667): Compare `self[i] < other[i]` directly once a
        # subscript result used as an operand keeps the `where` refinement.
        comptime for i in range(Self.size):
            var lhs = self[i]
            var rhs = other[i]
            if lhs < rhs:
                return True
            if lhs != rhs:
                return False
        return False

    @inline(.always)
    def __le__(
        self, other: Self
    ) -> Bool where conforms_to(
        Self.element_type, Comparable & TrivialRegisterPassable
    ):
        """Returns `True` if `self` is lexicographically less than or equal to
        `other`.

        Args:
            other: The tuple to compare with.

        Returns:
            True if `self` is lexicographically less than or equal to `other`.
        """
        return not (other < self)

    @inline(.always)
    def __gt__(
        self, other: Self
    ) -> Bool where conforms_to(
        Self.element_type, Comparable & TrivialRegisterPassable
    ):
        """Returns `True` if `self` is lexicographically greater than `other`.

        Args:
            other: The tuple to compare with.

        Returns:
            True if `self` is lexicographically greater than `other`.
        """
        return other < self

    @inline(.always)
    def __ge__(
        self, other: Self
    ) -> Bool where conforms_to(
        Self.element_type, Comparable & TrivialRegisterPassable
    ):
        """Returns `True` if `self` is lexicographically greater than or equal
        to `other`.

        Args:
            other: The tuple to compare with.

        Returns:
            True if `self` is lexicographically greater than or equal to
            `other`.
        """
        return not (self < other)
