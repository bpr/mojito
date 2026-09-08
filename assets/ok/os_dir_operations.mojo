# Directory operations from `std.os` over a per-run scratch directory:
# `mkdir`/`makedirs`/`listdir`/`remove`/`rmdir`/`removedirs` with upstream's
# raise texts. The scratch name carries random digits so concurrent runs of
# this fixture never share a directory, and nothing path-dependent prints.
from std.ffi import c_int, c_size_t, c_ssize_t, external_call
from std.os import getenv, listdir, makedirs, mkdir, remove, removedirs, rmdir
from std.os.path import exists, isdir, isfile, join


def _random_digits(count: Int) raises -> String:
    var device = String("/dev/urandom")
    var fd = external_call["open", c_int, num_fixed_args=2](
        device.as_c_string_slice(), c_int(0), c_int(0)
    )
    if Int(fd) < 0:
        raise Error("cannot open /dev/urandom")
    var buffer = String(unsafe_uninit_length=count)
    var got = external_call["read", c_ssize_t](fd, buffer.unsafe_ptr_mut(), c_size_t(count))
    var closed = external_call["close", c_int](fd)
    if got != count:
        raise Error("short read from /dev/urandom")
    var digits = String("")
    var bytes = buffer.as_bytes()
    for i in range(count):
        digits += String(Int(bytes[i]) % 10)
    return digits


def _touch(var path: String) raises:
    # open(path, O_WRONLY | O_CREAT | O_TRUNC, 0o644) then close.
    var fd = external_call["open", c_int, num_fixed_args=2](
        path.as_c_string_slice(), c_int(0o1101), c_int(0o644)
    )
    if Int(fd) < 0:
        raise Error("cannot create the marker file")
    var closed = external_call["close", c_int](fd)


def main() raises:
    var scratch = join(getenv("TMPDIR", "/tmp"), String("mojito_os_dir_") + _random_digits(8))
    mkdir(scratch, mode=0o700)
    print(isdir(scratch))
    var deep = join(scratch, "a", "b", "c")
    makedirs(deep, exist_ok=True)
    makedirs(deep, exist_ok=True)
    print(isdir(deep))
    try:
        makedirs(deep)
    except e:
        print(String(e).endswith("set `makedirs(path, exist_ok=True)` to allow existing dirs"))
    var marker = join(scratch, "a", "marker.txt")
    _touch(marker)
    print(isfile(marker), exists(marker))
    var names = listdir(join(scratch, "a"))
    print(len(names))
    var saw_b = False
    var saw_marker = False
    for name in names:
        if name == "b":
            saw_b = True
        if name == "marker.txt":
            saw_marker = True
    print(saw_b, saw_marker)
    remove(marker)
    print(exists(marker))
    try:
        remove(marker)
    except e:
        print(String(e).startswith("Can not remove file: "))
    removedirs(deep)
    print(exists(scratch))
    try:
        rmdir(scratch)
    except e:
        print(String(e).startswith("Can not remove directory: "))
