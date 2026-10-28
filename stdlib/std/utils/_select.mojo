# Upstream's `std/utils/_select.mojo`, verbatim.


@always_inline("builtin")
def _select_register_value[
    T: TrivialRegisterPassable
](condition: Bool, lhs: T, rhs: T) -> T:
    """Choose one value based on a condition, without IR-level branching.
    Use this over normal `if` branches to reduce the size of the generated IR.

    Parameters:
        T: The type of the lhs and rhs.

    Args:
        condition: The condition to test.
        lhs: The value to select if the condition is met.
        rhs: The value to select if the condition is not met.

    Returns:
        The value selected based on the condition.

    Notes:
        This should not be used pervasively in general code. It should only be
        used in very low level libraries where IR size/compile time matters
        because it flattens if statements early in the compiler pipeline.
    """
    return __mlir_op.`pop.select`(condition.__mlir_bool__(), lhs, rhs)
