# With no `-D`, no compile-time define exists: each read gives its default,
# and the build is not a debug build, as at the pin run without `-D`.
from std.sys import (
    get_defined_bool,
    get_defined_dtype,
    get_defined_int,
    get_defined_string,
    is_defined,
)
from std.sys._build import is_debug_build, is_release_build


def main():
    print(is_defined["ASSERT"]())
    print(get_defined_string["ASSERT", "safe"]())
    print(get_defined_int["N", 3]())
    print(get_defined_bool["B"]())
    print(get_defined_bool["B", True]())
    print(get_defined_dtype["DT", DType.int8]())
    print(SIMD[get_defined_dtype["DT", DType.uint16](), 1](7))
    print(is_debug_build())
    print(is_release_build())
