# The `std.collections.string` package: upstream's import spellings for
# the string types and iterators (`from std.collections.string import
# CodepointsIter`).
#
# Omitted: `ascii`, `chr`, `ord`, `ImmStringSpan`, `ImmStringSlice`, and
# `BytesIter`, which the bundled library does not declare.
"""Provides comprehensive Unicode string functionality."""

from std.string import Codepoint, String, atof, atol
from .string_span import (
    CodepointsIter,
    GraphemeSliceIter,
    StaticString,
    StringSpan,
    StringSlice,
)
