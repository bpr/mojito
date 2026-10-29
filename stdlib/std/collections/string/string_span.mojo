# Upstream's `std/collections/string/string_span.mojo` at its own module
# path, holding `_get_kgen_string` verbatim. Upstream's module also
# declares `StringSpan` itself, which Mojito keeps in `std/string.mojo`,
# whence this module imports `StaticString`.
#
# Omitted: `get_static_string`, whose `StaticString(kgen)` reads a
# `!kgen.string` at run time (roadmap R1192), and the rest of upstream's
# module, which lives in `std/string.mojo`.
"""Implements the `StringSpan` type and related utilities for efficient string operations."""

from std.string import StaticString


@inline(.nodebug)
def _get_kgen_string[
    string: StaticString, *extra: StaticString
]() -> __mlir_type.`!kgen.string`:
    """Form a `!kgen.string` from compile-time StringSpan values concatenated.

    Parameters:
        string: The first string span to use.
        extra: Additional string spans to concatenate.

    Returns:
        The string value as a `!kgen.string`.
    """
    return __mlir_attr[
        `#kgen.param.expr<data_to_str,`,
        string,
        `,`,
        extra.values,
        `> : !kgen.string`,
    ]
