# Upstream's `std/memory/memory.mojo`, as far as the bundle carries it:
# `forget_deinit`, which ends an owned value without running its
# deinitializer (or its fields').


def forget_deinit[T: AnyType](var value: T):
    """Takes ownership and skips running `__deinit__` deinitializers.

    This skips the destructor for the root `value` and all of its fields,
    recursively. Take care to use `^` when passing an `ImplicitlyCopyable`
    value, to avoid forgetting a copy instead of the original.

    Parameters:
        T: The type of the value to discard without running a deinitializer.

    Args:
        value: The value to discard without running a deinitializer.
    """
    __mlir_op.`lit.ownership.mark_destroyed`(__get_mvalue_as_litref(value))
