"""C foreign-function interface over Mojito's closed libc callee table.

`external_call[callee, return_type, num_fixed_args=](*args)` is a compiler
builtin exported from this module: it accepts upstream's spelling for the
allowlisted libc functions only (open, read, write, close, lseek, unlink,
mkdir, rmdir, opendir, readdir, closedir, getcwd, getenv, setenv, unsetenv,
strerror, __errno_location, memcpy, strlen, __xstat, __lxstat). The VM executes each
callee in Rust with libc's return, `errno`, and buffer contract; the native
backend calls the C function directly.
"""

# C scalar aliases (Linux LP64).
comptime c_char = Int8
comptime c_uchar = UInt8
comptime c_int = Int32
comptime c_uint = UInt32
comptime c_short = Int16
comptime c_ushort = UInt16
comptime c_long = Int64
comptime c_ulong = UInt64
comptime c_long_long = Int64
comptime c_ulong_long = UInt64
comptime c_size_t = UInt
comptime c_ssize_t = Int
comptime c_float = Float32
comptime c_double = Float64
comptime c_pid_t = Int


# `CStringSlice` is declared beside `StringSpan` in `std.string` (this module
# loads before it) and re-exported here under upstream's import path.
from std.string import CStringSlice

# `errno` access lives with the libc vocabulary in `std.sys._libc_errno` and
# is re-exported here as upstream does.
from std.sys._libc_errno import get_errno
