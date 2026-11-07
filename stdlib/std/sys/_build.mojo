# Upstream's `std/sys/_build.mojo`, verbatim.

"""Provides functions to examine build configuration."""

from .defines import get_defined_string, is_defined
from std.string import StaticString


@inline(.nodebug)
def _build_type() -> StaticString:
    comptime assert is_defined["BUILD_TYPE"](), "the build type must be defined"
    return get_defined_string["BUILD_TYPE"]()


@inline(.nodebug)
def is_debug_build() -> Bool:
    """
    Returns True if the build is in debug mode.

    Returns:
        Bool: True if the build is in debug mode and False otherwise.
    """

    comptime if is_defined["DEBUG"]():
        return True
    elif is_defined["BUILD_TYPE"]():
        return _build_type() == "debug"
    else:
        return False


@inline(.nodebug)
def is_release_build() -> Bool:
    """
    Returns True if the build is in release mode.

    Returns:
        Bool: True if the build is in release mode and False otherwise.
    """

    comptime if is_defined["DEBUG"]():
        return False
    elif is_defined["BUILD_TYPE"]():
        comptime build_type = _build_type()
        return (
            build_type == "release"
            or build_type == "relwithdebinfo"
            or build_type == "minsizerel"
        )
    else:
        return True
