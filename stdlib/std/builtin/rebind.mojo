# Upstream's `std/builtin/rebind.mojo`: `rebind_var`, which moves an owned
# value into the type a parameter refinement names. `rebind` itself is the
# checker builtin (`checker/rebind.rs`), not a declaration here.
#
# Upstream imports the `std.memory` package and calls
# `std.memory.forget_deinit`; that package reaches `std.sys`, which imports
# `List`, whose `List(iterable)` imports this module, and a module constant
# read across that cycle is undefined (R592). This imports the declaring
# module instead.

from std.memory.memory import forget_deinit


@inline(.nodebug)
def rebind_var[
    src_type: AnyType,
    //,
    dest_type: Movable,
](var src: src_type, out dest: dest_type):
    """Statically assert that a parameter input type `src_type` resolves to the
    same type as a parameter result type `dest_type` after function
    instantiation and "rebind" the input to the result type, returning a
    owned variable with an adjusted type.

    Unlike `rebind`, this function takes an owned variable and returns an owned
    variable via moving the value from the input to the output.

    Parameters:
        src_type: The original type.
        dest_type: The type to rebind to.

    Args:
        src: The value to rebind.

    Returns:
        An owned value rebound as `dest_type`.
    """
    ref dest_ref = rebind[dest_type](src)
    dest = Pointer(to=dest_ref).unsafe_take_pointee()
    forget_deinit(src^)
