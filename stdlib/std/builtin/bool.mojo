# Upstream's `std/builtin/bool.mojo`: `Boolable`, `struct Bool`, and the
# `SIMD` overloads of `any` and `all`. The prelude exports `Bool`,
# `Boolable`, `any`, and `all`, as upstream's does.
#
# `Bool` declares upstream's storage, `MIN`/`MAX`, the trivial bits, the
# initializers but the Python one, and the `__bool__`, `__mlir_bool__`,
# `__mlir_i1__`, `__int__`, `__float__`, and `__hash__` bodies. The checker
# holds the struct as its compiler `Bool` type until R1612. Still compiler
# rules: the operators and `Comparable` (R1610), `Writable` (R1611), and
# `EnumLike` (R1189). Python interop and `@lldb_formatter_wrapping_type` are
# absent.
#
# Upstream's `Iterable` overloads of `any` and `all` are absent: their
# `where conforms_to(IterableType.IteratorType[origin_of(iterable)].Element,
# Boolable & Deinitable)` clause takes a projection subject (R792).

from std.hash.hasher import Hasher
from std.utils._select import _select_register_value as select


trait Boolable:
    def __bool__(self) -> Bool:
        ...


struct Bool(
    Boolable,
    Defaultable,
    Floatable,
    Hashable,
    ImplicitlyCopyable,
    Intable,
    TrivialRegisterPassable,
):
    """The primitive Bool scalar value used in Mojo."""

    var _mlir_value: __mlir_type.`!kgen.scalar<bool>`
    """The underlying storage of the boolean value."""

    comptime MIN: Bool = False
    """The minimum value of a Bool."""

    comptime MAX: Bool = True
    """The maximum value of a Bool."""

    comptime __del__is_trivial: Bool = True
    comptime __move_ctor_is_trivial: Bool = True
    comptime __copy_ctor_is_trivial: Bool = True

    @always_inline("builtin")
    def __init__(out self):
        """Construct a default, `False` Bool."""
        self = False

    @doc_hidden
    @always_inline("builtin")
    @implicit
    def __init__(out self, value: __mlir_type.i1):
        """Construct a Bool value given a __mlir_type.i1 value.

        Args:
            value: The initial __mlir_type.i1 value.
        """
        self._mlir_value = __mlir_op.`pop.cast_from_builtin`[
            _type=__mlir_type.`!kgen.scalar<bool>`
        ](value)

    @doc_hidden
    @always_inline("builtin")
    @implicit
    def __init__(out self, mlir_value: __mlir_type.`!kgen.scalar<bool>`):
        """Construct a Bool value given a `!kgen.scalar<bool>` value.

        Args:
            mlir_value: The initial value.
        """
        self._mlir_value = mlir_value

    @inline(.nodebug)
    def __init__[T: Boolable, //](out self, value: T):
        """Set the bool representation of the object.

        Parameters:
            T: The type of the object.

        Args:
            value: The object to get the bool representation of.
        """
        self = value.__bool__()

    @always_inline("builtin")
    def __init__(out self, value: None):
        """Set the bool representation of the `None` type to `False`.

        Args:
            value: The object to get the bool representation of.
        """
        self = False

    @inline(.nodebug)
    @implicit
    def __init__(out self, value: Scalar[.bool]):
        """Convert a scalar SIMD value to a Bool.

        Args:
            value: The scalar value.
        """
        self = value.__bool__()

    @always_inline("builtin")
    def __bool__(self) -> Bool:
        """Convert to Bool.

        Returns:
            This value.
        """
        return self

    @doc_hidden
    @always_inline("builtin")
    def __mlir_bool__(self) -> __mlir_type.`!kgen.scalar<bool>`:
        """Convert this Bool to __mlir_type.`!kgen.scalar<bool>`.

        This method is a special hook used by the compiler to test boolean
        objects in control flow conditions.  It should be implemented by Bool
        but not other general boolean convertible types (they should implement
        `__bool__` instead).

        Returns:
            The underlying value for the Bool.
        """
        return self._mlir_value

    @doc_hidden
    @always_inline("builtin")
    def __mlir_i1__(self) -> __mlir_type.i1:
        """Convert this Bool to __mlir_type.i1.

        Returns:
            The underlying value for the Bool as an __mlir_type.i1.
        """
        return __mlir_op.`pop.cast_to_builtin`[_type=__mlir_type.i1](
            self._mlir_value
        )

    @always_inline("builtin")
    def __int__(self) -> Int:
        """Convert this Bool to an integer.

        Returns:
            1 if the Bool is True, 0 otherwise.
        """
        return select[Int](self, 1, 0)

    @inline(.nodebug)
    def __float__(self) -> Float64:
        """Convert this Bool to a float.

        Returns:
            1.0 if True else 0.0 otherwise.
        """
        return select[Float64](self, 1, 0)

    def __hash__[H: Hasher](self, mut hasher: H):
        """Updates hasher with the underlying bytes.

        Parameters:
            H: The hasher type.

        Args:
            hasher: The hasher instance.
        """
        hasher._update_with_simd(Scalar[.bool](self))


def any(value: SIMD) -> Bool:
    """Checks if **any** element in the simd vector is truthy.

    Args:
        value: The simd vector to check.

    Returns:
        `True` if **any** element in the simd vector is truthy, `False`
        otherwise.
    """
    return value.cast[.bool]().reduce_or()


def all(value: SIMD) -> Bool:
    """Checks if **all** elements in the simd vector are truthy.

    Args:
        value: The simd vector to check.

    Returns:
        `True` if **all** elements in the simd vector are truthy, `False`
        otherwise.
    """
    return value.cast[.bool]().reduce_and()
