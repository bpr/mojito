# `std.tempfile`: `gettempdir` names a usable directory, `mkdtemp` creates a
# fresh directory with the requested prefix and suffix, and
# `TemporaryDirectory` removes its tree on a normal exit and on an error
# (its `__exit__(self, err)` cleans up and swallows the error).
from std.os import rmdir
from std.os.path import basename, exists, isdir
from std.pathlib import Path
from std.tempfile import TemporaryDirectory, gettempdir, mkdtemp


def fail_inside(mut kept: String) raises:
    with TemporaryDirectory(prefix="mojito_td_") as tmp:
        kept = tmp
        Path(tmp).joinpath("inner.txt").write_text("x")
        raise Error("inside")
    print("swallowed")


def main() raises:
    var default_dir = gettempdir()
    print(Bool(default_dir), isdir(default_dir.value()))
    var made = mkdtemp(prefix="mojito_mk_", suffix="_end")
    var name = basename(made)
    print(isdir(made), name.startswith("mojito_mk_"), name.endswith("_end"), name.byte_length())
    rmdir(made)
    print(exists(made))
    var kept = String("")
    with TemporaryDirectory(prefix="mojito_td_") as tmp:
        kept = tmp
        var nested = Path(tmp) / "sub" / "deep.txt"
        nested.write_text("y")
        print(exists(tmp), nested.exists())
    print(exists(kept))
    var other = String("")
    fail_inside(other)
    print(exists(other))
