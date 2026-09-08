# `std.pathlib.Path`: composition with `/` and `joinpath`, `suffix`/`name`/
# `parts`, comparisons and hashing, `write_text`/`read_text`/`read_bytes`/
# `write_bytes`, `exists`/`is_dir`/`is_file`/`listdir`, and `cwd`, over a
# per-run temporary directory; nothing path-dependent prints.
from std.os import remove, rmdir
from std.pathlib import DIR_SEPARATOR, Path, cwd
from std.tempfile import mkdtemp


def main() raises:
    var p = Path("a") / "b" / "c.txt"
    print(p, repr(p), DIR_SEPARATOR)
    print(p.suffix(), p.name(), len(p.parts()), Path("archive.tar.gz").suffix())
    print(Path("/a/.hidden").suffix() == "", Path("noext").suffix() == "")
    print(p == "a/b/c.txt", p == Path("a/b/c.txt"), Path("x") < Path("y"), Bool(Path("")))
    print(hash(Path("same")) == hash(Path("same")), hash(Path("one")) == hash(Path("two")))
    var joined = Path("a/b").joinpath("c", "d")
    var trailing = Path("a/b/").joinpath("c")
    print(joined, trailing, Path("a").joinpath())
    var scratch = Path(mkdtemp(prefix="mojito_path_"))
    print(scratch.exists(), scratch.is_dir(), scratch.is_file())
    var note = scratch / "note.txt"
    note.write_text("hello")
    print(note.read_text(), note.is_file(), note.exists())
    var raw = String("bytes")
    note.write_bytes(raw.as_bytes())
    print(len(note.read_bytes()), note.read_text())
    var listed = scratch.listdir()
    print(len(listed), listed[0].name())
    remove(note)
    rmdir(scratch)
    print(scratch.exists())
    var here = cwd()
    print(Bool(here), here.is_dir(), Path().exists())
