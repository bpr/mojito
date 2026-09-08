# `errno` after a failed libc call: `get_errno()` renders glibc's
# `strerror` text through `String(err)`, and the `std.os` raise texts embed
# it. The paths are relative and missing, so the output is host-independent.
from std.ffi import external_call, get_errno
from std.os import remove, rmdir
from std.sys._libc_errno import ErrNo


def main():
    try:
        remove(String("mojito_missing_dir/x.txt"))
    except e:
        print(e)
    try:
        rmdir(String("mojito_missing_dir"))
    except e:
        print(e)
    var missing = String("mojito_missing_dir")
    var status = external_call["rmdir", Int32](missing.as_c_string_slice())
    var err = get_errno()
    print(Int(status), String(err), err == ErrNo(2), err != ErrNo(9))
