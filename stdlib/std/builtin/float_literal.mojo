# Upstream's `std/builtin/float_literal.mojo`, in part: the
# `FloatLiteral[value]` struct with its default initializer, `is_nan`,
# `is_neg_zero`, `_is_normal`, `__int_literal__`, and `__int__`. A
# literal `2.5` is its application `FloatLiteral[2.5]`, as is an explicit
# one (`FloatLiteral[(2.5).value]()`).
# Not ported: the implicit initializer from `IntLiteral[_]`, whose
# declaration has no `out self` (R1784); the `nan`, `infinity`,
# `negative_infinity`, and `negative_zero` members, whose attributes the
# compiler's exact rational cannot hold (R1783); `__float__`, `write_to`,
# and `write_repr_to`, which read `Float64(self)` through `SIMD`'s
# undeclared `FloatLiteral` initializer (R1781); and `__bool__` and the
# operator dunders (R1753). `@__nonmaterializable(Float64)` is not read (R1651). The struct
# declares only the traits whose requirements are ported here.
"""Implements the FloatLiteral class.

These are Mojo built-ins, so you don't need to import them.
"""

# ===-----------------------------------------------------------------------===#
# FloatLiteral
# ===-----------------------------------------------------------------------===#


struct FloatLiteral[value: __mlir_type.`!pop.float_literal`](
    Defaultable,
    Intable,
    TrivialRegisterPassable,
):
    """Mojo floating point literal type.

    Parameters:
        value: The underlying infinite precision floating point value.
    """

    # ===------------------------------------------------------------------===#
    # Constructors
    # ===------------------------------------------------------------------===#

    @always_inline("builtin")
    def __init__(out self):
        """Create a FloatLiteral for any parameter value."""
        pass

    @always_inline("builtin")
    def is_nan(self) -> Bool:
        """Return whether the FloatLiteral is nan.

        Since `nan == nan` is False, this provides a way to check for nan-ness.

        Returns:
            True, if the value is nan, False otherwise.
        """
        return __mlir_attr[`#pop<float_literal_isa<nan `, self.value, `>> : i1`]

    @always_inline("builtin")
    def is_neg_zero(self) -> Bool:
        """Return whether the FloatLiteral is negative zero.

        Since `FloatLiteral.negative_zero == 0.0` is True, this provides a way
        to check if the FloatLiteral is negative zero.

        Returns:
            True, if the value is negative zero, False otherwise.
        """
        return __mlir_attr[
            `#pop<float_literal_isa<neg_zero `, self.value, `>> : i1`
        ]

    @always_inline("builtin")
    def _is_normal(self) -> Bool:
        """Return whether the FloatLiteral is a normal (i.e. not special) value.

        Returns:
            True, if the value is a normal float, False otherwise.
        """
        return __mlir_attr[
            `#pop<float_literal_isa<normal `, self.value, `>> : i1`
        ]

    # ===------------------------------------------------------------------===#
    # Conversion Operators
    # ===------------------------------------------------------------------===#

    @always_inline("builtin")
    def __int_literal__(
        self,
    ) -> IntLiteral[
        __mlir_attr[
            `#pop<float_to_int_literal<`,
            Self.value,
            `>> : !pop.int_literal`,
        ]
    ]:
        """Casts the floating point value to an IntLiteral. If there is a
        fractional component, then the value is truncated towards zero.

        Eg. `(4.5).__int_literal__()` returns `4`, and `(-3.7).__int_literal__()`
        returns `-3`.

        Returns:
            The value as an integer.
        """
        return {}

    @always_inline("builtin")
    def __int__(self) -> Int:
        """Converts the FloatLiteral value to an Int. If there is a fractional
        component, then the value is truncated towards zero.

        Eg. `(4.5).__int__()` returns `4`, and `(-3.7).__int__()` returns `-3`.

        Returns:
            The value as an integer.
        """
        return self.__int_literal__().__int__()
