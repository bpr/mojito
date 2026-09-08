# expect: is not in Mojito's libc allowlist
# `external_call` accepts only the allowlisted libc callees; any other name
# is a checker error, not a link-time surprise.
from std.ffi import c_int, external_call


def main():
    var command = String("true")
    var status = external_call["system", c_int](command.as_c_string_slice())
    print(Int(status))
