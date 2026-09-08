"""Environment variables (`getenv`, `setenv`, `unsetenv`)."""

from std.ffi import c_int, external_call
from std.string import String


def setenv(var name: String, var value: String, overwrite: Bool = True) -> Bool:
    var status = external_call["setenv", Int32](
        name.as_c_string_slice(),
        value.as_c_string_slice(),
        Int32(1 if overwrite else 0),
    )
    return Bool(status == 0)


def unsetenv(var name: String) -> Bool:
    return Bool(external_call["unsetenv", c_int](name.as_c_string_slice()) == 0)


def getenv(var name: String, default: String = "") -> String:
    # Upstream's `OptionalPointer[UInt8, ImmUntrackedOrigin]` result is a
    # plain pointer here; null reads as address 0.
    var ptr = external_call["getenv", Pointer[UInt8, ImmUntrackedOrigin]](
        name.as_c_string_slice()
    )
    if Int(ptr) == 0:
        return default
    return String(unsafe_from_utf8_ptr=ptr)
