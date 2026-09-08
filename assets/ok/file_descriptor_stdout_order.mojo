# `FileDescriptor` is a `Writer` over a raw descriptor: writes to descriptor
# 1 interleave in order with `print`, `print(file=)` routes through it, and
# a descriptor over an open `FileHandle` writes to that file.
from std.os import remove, rmdir
from std.os.path import join
from std.sys import stdout
from std.tempfile import mkdtemp


def main() raises:
    print("one")
    var out = FileDescriptor(1)
    out.write_string("two\n")
    out.write("three", 3, "\n")
    print("four", file=out)
    print("five", file=stdout)
    var scratch = mkdtemp(prefix="mojito_fd_")
    var path = join(scratch, "log.txt")
    var handle = open(path, "w")
    var into_file = FileDescriptor(handle)
    print("six", 6, file=into_file)
    into_file.write_string("seven\n")
    handle.close()
    var reader = open(path, "r")
    print(reader.read(), end="")
    reader.close()
    remove(path)
    rmdir(scratch)
    print(FileDescriptor().value, stdout.value)
