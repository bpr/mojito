# Multiple comma-separated context managers, one without an `as` binding.
from std.os import remove, rmdir
from std.os.path import exists, getsize, join
from std.tempfile import mkdtemp


def copy(src: String, dst: String) raises:
    with open(src, "r") as f_in, open(dst, "w") as f_out:
        f_out.write(f_in.read())


def touch(path: String) raises:
    with open(path, "w"):
        pass


def main() raises:
    var scratch = mkdtemp(prefix="mojito_wm_")
    var src = join(scratch, "src.txt")
    var dst = join(scratch, "dst.txt")
    with open(src, "w") as f:
        f.write("payload")
    copy(src, dst)
    print(getsize(src), getsize(dst))
    var empty = join(scratch, "empty.txt")
    touch(empty)
    print(exists(empty), getsize(empty))
    remove(src)
    remove(dst)
    remove(empty)
    rmdir(scratch)
