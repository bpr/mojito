# The `external_call` builtin over the libc allowlist: a raw `write(1, ...)`
# interleaves in order with `print`, `strlen` over a `CStringSlice`, `getcwd`
# into an `Array` buffer, and a null `getenv` result reads as address 0.
from std.ffi import CStringSlice, c_char, c_int, c_size_t, c_ssize_t, external_call


def main():
    var line = String("raw line\n")
    print("before")
    var written = external_call["write", c_ssize_t](
        c_int(1), line.unsafe_ptr(), c_size_t(line.byte_length())
    )
    print("after", written)
    var text = String("hello")
    var view = text.as_c_string_slice()
    print(external_call["strlen", c_size_t](view), view.byte_length(), len(view))
    var buffer = Array[c_char, 1024](fill=0)
    var cwd = external_call["getcwd", Pointer[c_char, MutUntrackedOrigin]](
        buffer.unsafe_ptr(), c_size_t(1024)
    )
    print(Int(cwd) != 0)
    var unset = String("MOJITO_DEFINITELY_UNSET_VARIABLE")
    var missing = external_call["getenv", Pointer[UInt8, ImmUntrackedOrigin]](
        unset.as_c_string_slice()
    )
    print(Int(missing) == 0)
