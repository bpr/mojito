# Upstream's `std/builtin/_stubs.mojo`: the functions the compiler calls by
# module path, as upstream's `getBuiltinFunction` names them. The list
# pattern half tests a sequence pattern's subject length; the `comptime for`
# half steps a compile-time iterator, each `comptime for x in seq` iterating
# `seq.__iter__()` by these.
#
# Mojito's deviations, each removed by the entries named beside it:
# - `Sized` is imported, since this module loads before the prelude that
#   brings it into scope upstream.
# - The bound adds `& Deinitable`, since `Iterator` does not refine it yet
#   (R669, R798).
# - The `where conforms_to(IteratorType.Element, Movable & Deinitable)`
#   clause is absent, since `conforms_to` takes neither a projection nor a
#   composition yet (R792).
# - An exhausted iterator traps through `_mojito_abort` rather than
#   `_abort_base()`, since a `-> Never` call ends no path yet (R50, R799).

from std.builtin.len import Sized
from std.iter import Iterator

# ===-----------------------------------------------------------------------===#
# List pattern implementation details
# ===-----------------------------------------------------------------------===#


def list_length_matches[
    T: Sized
](value: T, minimum_length: Int, is_exact: Bool) -> Bool:
    var actual_length = value.__len__()
    if is_exact:
        return actual_length == minimum_length
    return actual_length >= minimum_length


def check_list_length[
    T: Sized
](value: T, minimum_length: Int, is_exact: Bool) raises:
    if not list_length_matches(value, minimum_length, is_exact):
        raise Error("list pattern does not match value length")


# ===-----------------------------------------------------------------------===#
# comptime for (was @parameter for) implementation details
# ===-----------------------------------------------------------------------===#


def paramfor_has_next[
    IteratorType: Iterator & Copyable & Deinitable
](it: IteratorType) -> Bool:
    var result = it.copy()
    try:
        var elem = result.__next__()
        _ = elem^
        return True
    except:
        return False


def paramfor_next_iter[
    IteratorType: Iterator & Copyable & Deinitable
](it: IteratorType) -> IteratorType:
    # NOTE: This function is called by the compiler's elaborator only when
    # paramfor_has_next will return true. This is needed because the interpreter
    # memory model isn't smart enough to handle mut arguments cleanly.
    var result = it.copy()
    # This intentionally discards the value, but this only happens at comptime,
    # so recomputing it in the body of the loop is fine.
    try:
        var elem = result.__next__()
        _ = elem^
        return result.copy()
    except:
        _mojito_abort("paramfor_next_iter: the iterator is exhausted")


def paramfor_next_value[
    IteratorType: Iterator & Copyable & Deinitable
](it: IteratorType) -> IteratorType.Element:
    # NOTE: This function is called by the compiler's elaborator only when
    # paramfor_has_next will return true. This is needed because the interpreter
    # memory model isn't smart enough to handle mut arguments cleanly.
    try:
        var result = it.copy()
        return result.__next__()
    except:
        _mojito_abort("paramfor_next_value: the iterator is exhausted")
