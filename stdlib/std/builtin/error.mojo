# Upstream's `std/builtin/error.mojo`, in part: `struct Error` over its
# message.
#
# `StackTrace`, the `_stack_trace` field, and `get_stack_trace` are omitted:
# collecting a trace calls the compiler runtime's
# `KGEN_CompilerRT_GetStackTrace`, which Mojito's runtime does not provide,
# and the field would change the native `MjError` layout raises share
# (R2011). The `depth` keyword of the `String` initializer is kept and
# ignored. `__mojo_debugger_raise_hook` is omitted.
"""Implements the Error class.

These are Mojo built-ins, so you don't need to import them.
"""

import std.format._utils as fmt
from std.string import String


struct Error(
    ImplicitlyCopyable,
    Writable,
):
    """This type represents an Error.

    An `Error` is cheap to copy: the message and the optional stack trace are
    both reference counted, so re-raising a caught error with `raise e` costs a
    reference count increment. Transfer with `raise e^` to avoid even that.
    """

    # ===-------------------------------------------------------------------===#
    # Fields
    # ===-------------------------------------------------------------------===#

    var _error: String
    """The backing error message."""

    # ===-------------------------------------------------------------------===#
    # Life cycle methods
    # ===-------------------------------------------------------------------===#

    @inline(.always)
    @implicit
    def __init__(out self, var value: String, *, depth: Int = -1):
        """Construct an Error object with a given String.

        Args:
            value: The error message.
            depth: The depth of the stack trace to collect. When negative,
                no stack trace is collected.
        """
        self._error = value^

    @inline(.always)
    @implicit
    def __init__(out self, value: StringLiteral):
        """Construct an Error object with a given string literal.

        Args:
            value: The error message.
        """
        self._error = String(value)

    @implicit
    @inline(.never)
    def __init__[*Ts: Writable](out self, *args: *Ts):
        """Construct an Error by concatenating a sequence of Writable arguments.

        Args:
            args: A sequence of Writable arguments.

        Parameters:
            Ts: The types of the arguments to format. Each type must be satisfy
                `Writable`.
        """
        self = Error(String(*args), depth=0)

    # ===-------------------------------------------------------------------===#
    # Trait implementations
    # ===-------------------------------------------------------------------===#

    @inline(.never)
    def write_to(self, mut writer: Some[Writer]):
        """
        Formats this error to the provided Writer.

        Args:
            writer: The object to write to.
        """
        self._error.write_to(writer)

    @inline(.never)
    def write_repr_to(self, mut writer: Some[Writer]):
        """
        Formats this error to the provided Writer.

        Args:
            writer: The object to write to.
        """
        fmt.FormatStruct(writer, "Error").fields(fmt.Repr(self._error))
