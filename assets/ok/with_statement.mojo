# `with open(...) as f:` closes the file when the block ends, including on a
# `return` from inside it.
from std.os import remove, rmdir
from std.os.path import join
from std.tempfile import mkdtemp


def read_all(path: String) raises -> String:
    with open(path, "r") as f:
        return f.read()


def main() raises:
    var scratch = mkdtemp(prefix="mojito_ws_")
    var path = join(scratch, "text.txt")
    with open(path, "w") as f:
        f.write("line one\n", "line two\n")
    print(read_all(path), end="")
    remove(path)
    rmdir(scratch)
